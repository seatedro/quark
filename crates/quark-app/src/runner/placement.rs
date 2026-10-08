//! Where windows sit on the desktop, and what the platform lets an app learn
//! and change about that: [`EventContext::placement`] and
//! [`EventContext::capabilities`].
//!
//! Positions are physical desktop pixels, as winit reports them; sizes are
//! logical points. On macOS winit derives desktop pixels from Cocoa points
//! with each window's own scale factor, so positions of windows on displays
//! with different scales are not in one shared pixel space: divide each by
//! its window's scale factor before comparing them there.

use winit::monitor::MonitorHandle;

use super::*;

/// One window's place on the desktop, read when asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowPlacement {
    /// The content area's top-left corner on the desktop. `None` where the
    /// platform does not tell windows where they are (Wayland).
    pub inner_position: Option<(i32, i32)>,
    /// The window's top-left corner, decorations included. `None` on
    /// Wayland.
    pub outer_position: Option<(i32, i32)>,
    /// The content area in logical points.
    pub size: (f32, f32),
    pub scale_factor: f64,
    /// The display the window is mostly on, if the platform says.
    pub monitor: Option<MonitorInfo>,
    pub maximized: bool,
    /// `None` where the platform cannot tell (Wayland).
    pub minimized: Option<bool>,
    /// Whether the window has keyboard focus: it is the active window.
    pub focused: bool,
}

/// A display, as [`WindowPlacement::monitor`] reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorInfo {
    /// A human-readable name; not guaranteed unique or stable across
    /// reconnects, so treat it as a hint.
    pub name: Option<String>,
    /// Top-left corner in physical desktop pixels.
    pub position: (i32, i32),
    /// In physical pixels.
    pub size: (u32, u32),
    pub scale_factor: f64,
}

impl MonitorInfo {
    pub(super) fn from_handle(monitor: &MonitorHandle) -> Self {
        let position = monitor.position();
        let size = monitor.size();
        Self {
            name: monitor.name(),
            position: (position.x, position.y),
            size: (size.width, size.height),
            scale_factor: monitor.scale_factor(),
        }
    }
}

/// What the windowing system lets the app do with window positions. The
/// same for every window of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformCapabilities {
    /// Windows have readable desktop positions
    /// ([`WindowPlacement::outer_position`], [`AppEvent::WindowMoved`]) and
    /// [`WindowOptions::position`] places them. False on Wayland, where the
    /// compositor alone places windows.
    pub window_positions: bool,
    /// A pointer position in a window's logical coordinates can be put on
    /// the desktop through [`WindowPlacement::inner_position`], so pointer
    /// motion in one window can be located in another. False on Wayland;
    /// drags between windows there need the compositor's drag and drop.
    pub desktop_pointer: bool,
}

impl PlatformCapabilities {
    /// macOS, Windows, and X11.
    pub const DESKTOP: Self = Self {
        window_positions: true,
        desktop_pointer: true,
    };
    /// Wayland: no window positions, no desktop pointer coordinates.
    pub const WAYLAND: Self = Self {
        window_positions: false,
        desktop_pointer: false,
    };

    /// The capabilities of the display server `event_loop` talks to.
    pub(super) fn detect(event_loop: &ActiveEventLoop) -> Self {
        use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};

        match event_loop.display_handle().map(|handle| handle.as_raw()) {
            Ok(RawDisplayHandle::Wayland(_)) => Self::WAYLAND,
            _ => Self::DESKTOP,
        }
    }
}

impl Default for PlatformCapabilities {
    fn default() -> Self {
        Self::DESKTOP
    }
}

impl WindowState {
    pub(super) fn placement(&self) -> WindowPlacement {
        let window = &self.window;
        let position = |position: winit::dpi::PhysicalPosition<i32>| (position.x, position.y);
        let scale = self.scale_factor;
        WindowPlacement {
            inner_position: window.inner_position().ok().map(position),
            outer_position: window.outer_position().ok().map(position),
            size: (
                (f64::from(self.surface_size.width) / scale) as f32,
                (f64::from(self.surface_size.height) / scale) as f32,
            ),
            scale_factor: scale,
            monitor: window
                .current_monitor()
                .map(|monitor| MonitorInfo::from_handle(&monitor)),
            maximized: window.is_maximized(),
            minimized: window.is_minimized(),
            focused: window.has_focus(),
        }
    }
}
