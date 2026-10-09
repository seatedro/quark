//! Each display's work area: the part docks, taskbars, and panels leave
//! for windows, which winit does not report. Read from NSScreen's
//! `visibleFrame` on macOS, `GetMonitorInfoW`'s `rcWork` on Windows, and
//! the EWMH `_NET_WORKAREA` of the current desktop, clipped to the display,
//! on X11. Wayland compositors place windows themselves and tell clients
//! nothing about panels, so there is none.
//!
//! The result is in winit's physical monitor coordinates, as
//! [`MonitorInfo::bounds`](super::placement::MonitorInfo::bounds) is.

use winit::monitor::MonitorHandle;

use super::placement::PhysicalRect;

/// `monitor`'s work area, `None` where the platform does not say.
pub(crate) fn work_area(monitor: &MonitorHandle, bounds: PhysicalRect) -> Option<PhysicalRect> {
    imp::work_area(monitor, bounds)
}

/// `bounds` less the given insets, in physical pixels; `None` when nothing
/// is left.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn inset(
    bounds: PhysicalRect,
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
) -> Option<PhysicalRect> {
    let width = f64::from(bounds.width) - left - right;
    let height = f64::from(bounds.height) - top - bottom;
    (width >= 1.0 && height >= 1.0).then(|| PhysicalRect {
        x: bounds.x + left.round() as i32,
        y: bounds.y + top.round() as i32,
        width: width.round() as u32,
        height: height.round() as u32,
    })
}

#[cfg(target_os = "macos")]
mod imp {
    use objc2_app_kit::NSScreen;
    use winit::platform::macos::MonitorHandleExtMacOS;

    use super::*;

    pub(super) fn work_area(monitor: &MonitorHandle, bounds: PhysicalRect) -> Option<PhysicalRect> {
        let screen = monitor.ns_screen()?;
        // SAFETY: winit's pointer to the display's NSScreen, which
        // `+[NSScreen screens]` keeps alive while the display is connected;
        // we are on the main thread, as the event loop is.
        let screen: &NSScreen = unsafe { &*screen.cast::<NSScreen>() };
        let frame = screen.frame();
        let visible = screen.visibleFrame();
        // Cocoa frames grow upward from the primary display's bottom-left;
        // the insets are the same either way up, in points.
        let left = visible.origin.x - frame.origin.x;
        let bottom = visible.origin.y - frame.origin.y;
        let right = frame.origin.x + frame.size.width - (visible.origin.x + visible.size.width);
        let top = frame.origin.y + frame.size.height - (visible.origin.y + visible.size.height);
        let scale = monitor.scale_factor();
        inset(
            bounds,
            left * scale,
            top * scale,
            right * scale,
            bottom * scale,
        )
    }
}

#[cfg(windows)]
mod imp {
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
    use winit::platform::windows::MonitorHandleExtWindows;

    use super::*;

    pub(super) fn work_area(
        monitor: &MonitorHandle,
        _bounds: PhysicalRect,
    ) -> Option<PhysicalRect> {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        // SAFETY: winit's live HMONITOR and a sized MONITORINFO.
        let ok = unsafe { GetMonitorInfoW(HMONITOR(monitor.hmonitor() as _), &mut info) };
        if !ok.as_bool() {
            return None;
        }
        let work = info.rcWork;
        Some(PhysicalRect {
            x: work.left,
            y: work.top,
            width: u32::try_from(work.right - work.left).ok()?,
            height: u32::try_from(work.bottom - work.top).ok()?,
        })
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::*;

    pub(super) fn work_area(
        _monitor: &MonitorHandle,
        bounds: PhysicalRect,
    ) -> Option<PhysicalRect> {
        // A display handle says X11 or Wayland; a monitor handle does not.
        if !super::super::x11_root::active() {
            return None;
        }
        let area = super::super::x11_root::work_area()?;
        intersect(area, bounds)
    }

    /// The overlap of two rectangles. `_NET_WORKAREA` spans every display,
    /// so each display's share of it is the intersection.
    fn intersect(a: PhysicalRect, b: PhysicalRect) -> Option<PhysicalRect> {
        let left = a.x.max(b.x);
        let top = a.y.max(b.y);
        let right = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width));
        let bottom =
            (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height));
        let width = u32::try_from(right - i64::from(left)).ok()?;
        let height = u32::try_from(bottom - i64::from(top)).ok()?;
        (width > 0 && height > 0).then_some(PhysicalRect {
            x: left,
            y: top,
            width,
            height,
        })
    }
}

#[cfg(not(any(target_os = "macos", windows, target_os = "linux")))]
mod imp {
    use super::*;

    pub(super) fn work_area(
        _monitor: &MonitorHandle,
        _bounds: PhysicalRect,
    ) -> Option<PhysicalRect> {
        None
    }
}
