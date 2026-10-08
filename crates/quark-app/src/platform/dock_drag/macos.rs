//! The macOS window stack: `+[NSWindow windowNumberAtPoint:belowWindowWithWindowNumber:]`
//! names the frontmost window, of any app, that a click at a screen point
//! would hit, and can start below a given window, which looks through the
//! window following the pointer.

use objc2::MainThreadMarker;
use objc2_app_kit::{NSEvent, NSScreen, NSView, NSWindow};
use objc2_foundation::NSPoint;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::{StackHit, WindowStack};
use crate::runner::{DesktopPoint, EventContext, WindowHandle};

pub(super) struct AppKitStack;

/// The window number of one of the app's windows.
fn number(cx: &EventContext, window: WindowHandle) -> Option<isize> {
    let RawWindowHandle::AppKit(handle) =
        cx.window_by_handle(window)?.window_handle().ok()?.as_raw()
    else {
        return None;
    };
    // SAFETY: winit's AppKit handle points at the window's live NSView, and
    // this runs on the main thread, where events are handled.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    Some(view.window()?.windowNumber())
}

impl WindowStack for AppKitStack {
    fn hit(&mut self, cx: &EventContext, at: DesktopPoint, skip: Option<WindowHandle>) -> StackHit {
        let Some(mtm) = MainThreadMarker::new() else {
            return StackHit::Unknown;
        };
        // Desktop points run down from the primary screen's top (winit's
        // flip), Cocoa's screen points up from its bottom.
        let Some(primary) = NSScreen::screens(mtm).firstObject() else {
            return StackHit::Unknown;
        };
        let point = NSPoint::new(at.0, primary.frame().size.height - at.1);
        let owned: Vec<(isize, WindowHandle)> = cx
            .windows()
            .into_iter()
            .filter_map(|window| Some((number(cx, window)?, window)))
            .collect();
        let mut top = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, 0, mtm);
        if top != 0 && skip.and_then(|skip| number(cx, skip)) == Some(top) {
            top = NSWindow::windowNumberAtPoint_belowWindowWithWindowNumber(point, top, mtm);
        }
        owned
            .iter()
            .find(|(number, _)| *number == top)
            .map_or(StackHit::Other, |&(_, window)| StackHit::Owned(window))
    }

    fn primary_held(&mut self) -> Option<bool> {
        Some(NSEvent::pressedMouseButtons() & 1 != 0)
    }
}
