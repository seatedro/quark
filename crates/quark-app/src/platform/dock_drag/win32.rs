//! The Windows window stack.
//!
//! `WindowFromPoint` finds the window a click would reach, skipping hidden
//! and click-through windows as the system does. It cannot look through a
//! window, so when it lands on the window following the pointer, the
//! top-level windows below that one are walked in z-order instead, keeping
//! to the ones a click could reach: shown, not minimized, not cloaked (on
//! another virtual desktop), and not click-through.

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetAsyncKeyState, VK_LBUTTON, VK_RBUTTON};
use windows::Win32::UI::WindowsAndMessaging::{
    GA_ROOT, GW_HWNDNEXT, GWL_EXSTYLE, GetAncestor, GetSystemMetrics, GetWindow, GetWindowLongW,
    GetWindowRect, IsIconic, IsWindowVisible, SM_SWAPBUTTON, WS_EX_TRANSPARENT, WindowFromPoint,
};

use super::{StackHit, WindowStack};
use crate::runner::{DesktopPoint, EventContext, WindowHandle};

pub(super) struct Win32Stack;

/// The top-level window behind one of the app's windows.
fn hwnd(cx: &EventContext, window: WindowHandle) -> Option<HWND> {
    match cx.window_by_handle(window)?.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}

/// Whether a click at a point inside `window` could reach it.
///
/// # Safety
///
/// `window` is a window handle (a stale one fails the queries).
unsafe fn reachable(window: HWND) -> bool {
    // SAFETY: plain queries on a window handle.
    unsafe {
        if !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
            return false;
        }
        if GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TRANSPARENT.0 != 0 {
            return false;
        }
        let mut cloaked = 0u32;
        let cloaked_query = DwmGetWindowAttribute(
            window,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            size_of::<u32>() as u32,
        );
        cloaked_query.is_err() || cloaked == 0
    }
}

/// Whether `point` is in `window`'s visible frame: without the invisible
/// resize borders where DWM reports the frame.
///
/// # Safety
///
/// As [`reachable`].
unsafe fn holds(window: HWND, point: POINT) -> bool {
    let mut rect = RECT::default();
    // SAFETY: plain queries on a window handle, into a local RECT.
    let known = unsafe {
        DwmGetWindowAttribute(
            window,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut rect).cast(),
            size_of::<RECT>() as u32,
        )
        .is_ok()
            || GetWindowRect(window, &mut rect).is_ok()
    };
    known
        && (rect.left..rect.right).contains(&point.x)
        && (rect.top..rect.bottom).contains(&point.y)
}

impl WindowStack for Win32Stack {
    fn hit(&mut self, cx: &EventContext, at: DesktopPoint, skip: Option<WindowHandle>) -> StackHit {
        // Windows desktop units are physical screen pixels, which a
        // per-monitor DPI aware process (winit's default) sees unscaled.
        let point = POINT {
            x: at.0.round() as i32,
            y: at.1.round() as i32,
        };
        let owned: Vec<(HWND, WindowHandle)> = cx
            .windows()
            .into_iter()
            .filter_map(|window| Some((hwnd(cx, window)?, window)))
            .collect();
        let skipped = skip.and_then(|skip| hwnd(cx, skip));
        let classify = |top: HWND| {
            owned
                .iter()
                .find(|(hwnd, _)| *hwnd == top)
                .map_or(StackHit::Other, |&(_, window)| StackHit::Owned(window))
        };
        // SAFETY: plain window queries on the UI thread; handles that go
        // stale meanwhile fail them.
        unsafe {
            let under = WindowFromPoint(point);
            if under.is_invalid() {
                return StackHit::Other;
            }
            let top = GetAncestor(under, GA_ROOT);
            let Some(skipped) = skipped.filter(|&skipped| skipped == top) else {
                return classify(top);
            };
            let mut next = GetWindow(skipped, GW_HWNDNEXT);
            while let Ok(window) = next {
                if reachable(window) && holds(window, point) {
                    return classify(window);
                }
                next = GetWindow(window, GW_HWNDNEXT);
            }
        }
        StackHit::Other
    }

    fn primary_held(&mut self) -> Option<bool> {
        // SAFETY: plain input state queries.
        unsafe {
            // The primary button is the right one when buttons are swapped.
            let primary = if GetSystemMetrics(SM_SWAPBUTTON) != 0 {
                VK_RBUTTON
            } else {
                VK_LBUTTON
            };
            Some(GetAsyncKeyState(i32::from(primary.0)) < 0)
        }
    }
}
