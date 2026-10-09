//! The native host: a sheet on the parent `NSWindow`.
//!
//! A sheet is AppKit's own parent-modal window. The parent stays visible
//! and redraws, but takes no input; the sheet moves, minimizes, and closes
//! with it. `beginSheet:` presents it without a modal run loop, so quark's
//! event loop keeps running. Sheets have no title bar buttons, so a bottom
//! bar carries the title and a Cancel button that Escape triggers.

use std::cell::Cell;
use std::ffi::c_void;
use std::ptr::NonNull;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameAqua, NSAppearanceNameDarkAqua,
    NSApplication, NSAutoresizingMaskOptions, NSBackingStoreType, NSButton, NSEvent,
    NSEventModifierFlags, NSTextField, NSView, NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{
    AppKitWindowHandle, HandleError, HasWindowHandle, RawWindowHandle, WindowHandle,
};

use crate::Appearance;

/// Height of the bottom bar, in points.
const BAR_HEIGHT: f64 = 44.0;
/// Inset of the bar's label and button from the sheet's sides.
const BAR_INSET: f64 = 16.0;
/// Room left around a sheet that would not fit on the parent's screen.
const SCREEN_MARGIN: f64 = 40.0;

pub(super) struct CancelIvars {
    on_cancel: Box<dyn Fn()>,
    /// Report a user cancel once; the close that follows tears the sheet
    /// down.
    fired: Cell<bool>,
}

define_class!(
    /// The Cancel button's target.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[ivars = CancelIvars]
    pub(super) struct CancelTarget;

    impl CancelTarget {
        #[unsafe(method(cancel:))]
        fn cancel(&self, _sender: Option<&AnyObject>) {
            let ivars = self.ivars();
            if !ivars.fired.replace(true) {
                (ivars.on_cancel)();
            }
        }
    }
);

define_class!(
    /// The sheet window, which also maps editing shortcuts.
    #[unsafe(super(NSWindow))]
    #[thread_kind = MainThreadOnly]
    pub(super) struct SheetWindow;

    impl SheetWindow {
        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> Bool {
            let handled: Bool = unsafe { msg_send![super(self), performKeyEquivalent: event] };
            if handled.as_bool() {
                return handled;
            }
            Bool::new(editing_shortcut(self, event))
        }
    }
);

/// The standard editing action for a key equivalent, if it is one.
fn editing_action(command_only: bool, shift: bool, key: &str) -> Option<Sel> {
    if !command_only {
        return None;
    }
    Some(match (key, shift) {
        ("x", false) => sel!(cut:),
        ("c", false) => sel!(copy:),
        ("v", false) => sel!(paste:),
        ("a", false) => sel!(selectAll:),
        ("z", false) => sel!(undo:),
        ("z", true) | ("Z", true) => sel!(redo:),
        _ => return None,
    })
}

/// wry's child webview declines every key equivalent so the app menu can
/// see them, and a winit app's menu has no Edit items, so Command-C, V, X,
/// A, and Z would reach nothing and paste would not work in a password
/// field. Send the standard editing actions to the first responder instead.
fn editing_shortcut(window: &NSWindow, event: &NSEvent) -> bool {
    let flags = event.modifierFlags() & NSEventModifierFlags::DeviceIndependentFlagsMask;
    let shift = flags.contains(NSEventModifierFlags::Shift);
    let rest = flags - NSEventModifierFlags::Shift - NSEventModifierFlags::CapsLock;
    let Some(key) = event.charactersIgnoringModifiers() else {
        return false;
    };
    let Some(action) = editing_action(
        rest == NSEventModifierFlags::Command,
        shift,
        &key.to_string(),
    ) else {
        return false;
    };
    let app = NSApplication::sharedApplication(MainThreadMarker::from(window));
    unsafe { app.sendAction_to_from(action, None, Some(window)) }
}

/// A sheet window with a content area for the webview and a bottom bar.
pub(super) struct Sheet {
    pub(super) window: Retained<SheetWindow>,
    /// The webview's superview, above the bar.
    pub(super) content: Retained<NSView>,
    /// Retained here: the button holds its target weakly.
    _cancel: Retained<CancelTarget>,
}

impl Sheet {
    /// A sheet of `size` points (clamped to `parent`'s screen) titled
    /// `title`. `on_cancel` runs once, when the user cancels.
    pub(super) fn new(
        mtm: MainThreadMarker,
        parent: &NSWindow,
        title: &str,
        size: (f32, f32),
        min_size: (f32, f32),
        appearance: Appearance,
        on_cancel: Box<dyn Fn()>,
    ) -> Self {
        let size = fit(
            NSSize::new(f64::from(size.0), f64::from(size.1)),
            parent.screen().map(|screen| screen.visibleFrame().size),
        );
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), size);
        let window: Retained<SheetWindow> = unsafe {
            msg_send![super(mtm.alloc::<SheetWindow>().set_ivars(())),
                initWithContentRect: rect,
                styleMask: NSWindowStyleMask::Titled | NSWindowStyleMask::Resizable,
                backing: NSBackingStoreType::Buffered,
                defer: false]
        };
        // SAFETY: the sheet is owned through `Retained`, not by AppKit.
        unsafe { window.setReleasedWhenClosed(false) };
        let title = NSString::from_str(title);
        window.setTitle(&title);
        window.setContentMinSize(NSSize::new(
            f64::from(min_size.0).min(size.width),
            f64::from(min_size.1).min(size.height),
        ));
        window.setAppearance(named_appearance(appearance).as_deref());

        let root = window
            .contentView()
            .expect("a new window has a content view");
        let content = NSView::initWithFrame(
            mtm.alloc(),
            NSRect::new(
                NSPoint::new(0.0, BAR_HEIGHT),
                NSSize::new(size.width, (size.height - BAR_HEIGHT).max(0.0)),
            ),
        );
        content.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        root.addSubview(&content);

        let cancel = mtm.alloc::<CancelTarget>().set_ivars(CancelIvars {
            on_cancel,
            fired: Cell::new(false),
        });
        let cancel: Retained<CancelTarget> = unsafe { msg_send![super(cancel), init] };
        let button = unsafe {
            NSButton::buttonWithTitle_target_action(
                &NSString::from_str("Cancel"),
                Some(&cancel),
                Some(sel!(cancel:)),
                mtm,
            )
        };
        button.setKeyEquivalent(&NSString::from_str("\u{1b}"));
        let button_size = button.fittingSize();
        button.setFrame(NSRect::new(
            NSPoint::new(
                size.width - button_size.width - BAR_INSET,
                (BAR_HEIGHT - button_size.height) / 2.0,
            ),
            button_size,
        ));
        button.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMinXMargin);
        root.addSubview(&button);

        let label = NSTextField::labelWithString(&title, mtm);
        let label_size = label.fittingSize();
        label.setFrame(NSRect::new(
            NSPoint::new(BAR_INSET, (BAR_HEIGHT - label_size.height) / 2.0),
            NSSize::new(
                label_size
                    .width
                    .min(size.width - button_size.width - 3.0 * BAR_INSET)
                    .max(0.0),
                label_size.height,
            ),
        ));
        label.setAutoresizingMask(NSAutoresizingMaskOptions::ViewMaxXMargin);
        root.addSubview(&label);

        Sheet {
            window,
            content,
            _cancel: cancel,
        }
    }

    pub(super) fn present(&self, parent: &NSWindow) {
        parent.beginSheet_completionHandler(&self.window, None);
    }

    /// End the sheet and hide it; idempotent.
    pub(super) fn dismiss(&self) {
        if let Some(parent) = self.window.sheetParent() {
            parent.endSheet(&self.window);
        }
        self.window.orderOut(None);
    }

    /// A window handle for the content view, for wry's child webview.
    pub(super) fn content_handle(&self) -> ViewHandle<'_> {
        ViewHandle {
            view: NonNull::from(&*self.content).cast(),
            _sheet: std::marker::PhantomData,
        }
    }
}

/// `requested`, shrunk to fit `screen` with a margin.
fn fit(requested: NSSize, screen: Option<NSSize>) -> NSSize {
    let Some(screen) = screen else {
        return requested;
    };
    NSSize::new(
        requested.width.min(screen.width - SCREEN_MARGIN).max(1.0),
        requested.height.min(screen.height - SCREEN_MARGIN).max(1.0),
    )
}

/// The `NSAppearance` for an explicit choice; `None` follows the system.
fn named_appearance(appearance: Appearance) -> Option<Retained<NSAppearance>> {
    let name = match appearance {
        Appearance::Light => unsafe { NSAppearanceNameAqua },
        Appearance::Dark => unsafe { NSAppearanceNameDarkAqua },
        _ => return None,
    };
    NSAppearance::appearanceNamed(name)
}

/// The parent window of a winit view handle.
pub(super) fn parent_window(handle: RawWindowHandle) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(handle) = handle else {
        return None;
    };
    // SAFETY: the runner keeps the parent's view alive while its webviews
    // exist, and this runs on the main thread.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}

/// A raw window handle naming a sheet's content view.
pub(super) struct ViewHandle<'a> {
    view: NonNull<c_void>,
    _sheet: std::marker::PhantomData<&'a Sheet>,
}

impl HasWindowHandle for ViewHandle<'_> {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let raw = RawWindowHandle::AppKit(AppKitWindowHandle::new(self.view));
        // SAFETY: the borrow of the sheet keeps the view alive.
        Ok(unsafe { WindowHandle::borrow_raw(raw) })
    }
}
