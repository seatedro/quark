//! macOS: `NSVisualEffectView`s behind the GPU view.
//!
//! winit's view is the window's content view, and wgpu draws into a
//! `CAMetalLayer` sublayer of its layer, so a subview of it could land
//! above the GPU content. The effect views go in the window's frame view
//! instead, below the content view: the frame view's bounds are the whole
//! window, and the content view, transparent where the app paints nothing,
//! covers them. Being below it, they never take a click, and they are no
//! accessibility elements.

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, Bool};
use objc2::{msg_send, sel};
use objc2_app_kit::{NSView, NSWindow, NSWindowOrderingMode};
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use super::{CornerResult, EffectiveBackground, MaterialKind, MaterialRect, WindowSurface};
use crate::platform::chrome::TitleAction;

/// `NSVisualEffectBlendingModeBehindWindow`.
const BEHIND_WINDOW: isize = 0;
/// `NSVisualEffectStateFollowsWindowActiveState`.
const FOLLOWS_WINDOW: isize = 0;
/// `NSViewWidthSizable | NSViewHeightSizable`.
const FILL_SUPERVIEW: usize = 2 | 16;

/// The `NSVisualEffectMaterial` for `kind`.
fn material(kind: MaterialKind) -> isize {
    match kind {
        MaterialKind::Titlebar => 3,
        MaterialKind::Menu => 5,
        MaterialKind::Popover => 6,
        MaterialKind::Sidebar => 7,
        MaterialKind::HeaderView => 10,
        MaterialKind::Sheet => 11,
        MaterialKind::Hud => 13,
        MaterialKind::Tooltip => 17,
        MaterialKind::Content => 18,
        MaterialKind::UnderWindow => 21,
        // WindowBackground, and kinds added later.
        _ => 12,
    }
}

/// A window's effect views, kept across frames.
#[derive(Default)]
pub(crate) struct NativeMaterial {
    /// Behind the whole window.
    window: Option<Retained<NSView>>,
    regions: Vec<Region>,
}

struct Region {
    rect: MaterialRect,
    view: Retained<NSView>,
}

/// The window's content view (winit's) and frame view.
fn views(window: &Window) -> Option<(Retained<NSWindow>, Retained<NSView>, Retained<NSView>)> {
    let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
        return None;
    };
    // SAFETY: winit's live NSView, used on the main thread.
    unsafe {
        let view: &NSView = handle.ns_view.cast::<NSView>().as_ref();
        let ns_window = view.window()?;
        let content = ns_window.contentView()?;
        let frame = content.superview()?;
        Some((ns_window, content, frame))
    }
}

/// A new effect view of `kind` with `radius` rounded corners.
fn effect_view(kind: MaterialKind, frame: NSRect, radius: f64) -> Option<Retained<NSView>> {
    let class = AnyClass::get(c"NSVisualEffectView")?;
    // SAFETY: NSVisualEffectView's documented initializer and setters, on
    // the main thread; it is an NSView subclass.
    unsafe {
        let view: Retained<AnyObject> = msg_send![msg_send![class, alloc], initWithFrame: frame];
        let _: () = msg_send![&view, setMaterial: material(kind)];
        let _: () = msg_send![&view, setBlendingMode: BEHIND_WINDOW];
        let _: () = msg_send![&view, setState: FOLLOWS_WINDOW];
        let _: () = msg_send![&view, setAccessibilityElement: Bool::NO];
        let view: Retained<NSView> = Retained::cast_unchecked(view);
        round(&view, radius);
        Some(view)
    }
}

/// Clip `view`'s layer to `radius` corners; 0 stops clipping.
fn round(view: &NSView, radius: f64) {
    // SAFETY: layer-backing a view and CALayer's corner properties, on the
    // main thread.
    unsafe {
        view.setWantsLayer(true);
        let layer: Option<Retained<AnyObject>> = msg_send![view, layer];
        if let Some(layer) = layer {
            let _: () = msg_send![&layer, setCornerRadius: radius];
            let _: () = msg_send![&layer, setMasksToBounds: Bool::new(radius > 0.0)];
        }
    }
}

impl NativeMaterial {
    /// Show what `surface` resolved to on `window`: a material behind the
    /// whole window, or none, and the content clipped to its corners.
    /// Returns false when the window has no AppKit views to use.
    pub(crate) fn apply(&mut self, window: &Window, surface: &WindowSurface) -> bool {
        let Some((ns_window, content, frame_view)) = views(window) else {
            return false;
        };
        let radius = match surface.corners {
            CornerResult::Clipped(radius) => f64::from(radius),
            _ => 0.0,
        };
        if let Some(view) = self.window.take() {
            view.removeFromSuperview();
        }
        if let EffectiveBackground::Material { kind, .. } = surface.background {
            let Some(view) = effect_view(kind, frame_view.bounds(), radius) else {
                return false;
            };
            // SAFETY: AppKit view hierarchy calls, on the main thread.
            unsafe {
                let _: () = msg_send![&view, setAutoresizingMask: FILL_SUPERVIEW];
                frame_view.addSubview_positioned_relativeTo(
                    &view,
                    NSWindowOrderingMode::Below,
                    Some(&content),
                );
            }
            self.window = Some(view);
        }
        round(&content, radius);
        ns_window.invalidateShadow();
        true
    }

    /// Give each of `regions` an effect view of its kind, reusing the views
    /// of regions whose id is unchanged. `rect`s are in the content view's
    /// logical points from its top-left.
    pub(crate) fn set_regions(&mut self, window: &Window, regions: &[MaterialRect]) {
        let unchanged = regions.len() == self.regions.len()
            && regions.iter().zip(&self.regions).all(|(a, b)| *a == b.rect);
        if unchanged {
            return;
        }
        let Some((_, content, frame_view)) = views(window) else {
            return;
        };
        let mut old = std::mem::take(&mut self.regions);
        for rect in regions {
            let frame = to_frame_view(&content, &frame_view, rect);
            let reused = old
                .iter()
                .position(|r| r.rect.id == rect.id && r.rect.kind == rect.kind)
                .map(|at| old.swap_remove(at));
            let view = match reused {
                Some(region) => {
                    region.view.setFrame(frame);
                    round(&region.view, f64::from(rect.corner_radius));
                    region.view
                }
                None => {
                    let Some(view) = effect_view(rect.kind, frame, f64::from(rect.corner_radius))
                    else {
                        continue;
                    };
                    frame_view.addSubview_positioned_relativeTo(
                        &view,
                        NSWindowOrderingMode::Below,
                        Some(&content),
                    );
                    view
                }
            };
            self.regions.push(Region { rect: *rect, view });
        }
        for region in old {
            region.view.removeFromSuperview();
        }
    }
}

/// `rect`, in `content`'s logical points from its top-left, in
/// `frame_view`'s coordinates.
fn to_frame_view(content: &NSView, frame_view: &NSView, rect: &MaterialRect) -> NSRect {
    let r = rect.rect;
    let bounds = content.bounds();
    let y = if content.isFlipped() {
        f64::from(r.y)
    } else {
        bounds.size.height - f64::from(r.y) - f64::from(r.height)
    };
    let local = NSRect::new(
        NSPoint::new(f64::from(r.x), y),
        NSSize::new(f64::from(r.width), f64::from(r.height)),
    );
    content.convertRect_toView(local, Some(frame_view))
}

/// The user's reduced transparency and increased contrast settings.
pub(crate) fn accessibility() -> (bool, bool) {
    let Some(class) = AnyClass::get(c"NSWorkspace") else {
        return (false, false);
    };
    // SAFETY: NSWorkspace's shared instance and its accessibility display
    // properties (macOS 10.10+).
    unsafe {
        let workspace: Retained<AnyObject> = msg_send![class, sharedWorkspace];
        let has = |selector| -> bool { msg_send![&workspace, respondsToSelector: selector] };
        let reduce = has(sel!(accessibilityDisplayShouldReduceTransparency))
            && msg_send![&workspace, accessibilityDisplayShouldReduceTransparency];
        let contrast = has(sel!(accessibilityDisplayShouldIncreaseContrast))
            && msg_send![&workspace, accessibilityDisplayShouldIncreaseContrast];
        (reduce, contrast)
    }
}

/// What a title bar double-click does: System Settings' "Double-click a
/// window's title bar to" (`AppleActionOnDoubleClick`), zoom by default.
pub(crate) fn title_double_click() -> TitleAction {
    let Some(class) = AnyClass::get(c"NSUserDefaults") else {
        return TitleAction::ToggleMaximize;
    };
    let key = NSString::from_str("AppleActionOnDoubleClick");
    // SAFETY: the standard user defaults and a string lookup.
    let value: Option<Retained<NSString>> = unsafe {
        let defaults: Retained<AnyObject> = msg_send![class, standardUserDefaults];
        msg_send![&defaults, stringForKey: &*key]
    };
    match value.map(|v| v.to_string()).as_deref() {
        Some("Minimize") => TitleAction::Minimize,
        Some("None") => TitleAction::Nothing,
        _ => TitleAction::ToggleMaximize,
    }
}
