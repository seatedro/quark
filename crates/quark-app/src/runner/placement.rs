//! Where windows sit on the desktop, and what the platform lets an app learn
//! and change about that: [`EventContext::placement`],
//! [`EventContext::capabilities`], desktop pointer coordinates
//! ([`EventContext::to_desktop`]), and moving windows
//! ([`EventContext::set_outer_position`], [`EventContext::align_window`]),
//! for windows that follow the pointer, such as a panel torn off a dock.
//!
//! # Desktop units
//!
//! Every desktop position here is in one space shared by all windows and
//! displays, so positions from different windows compare and subtract
//! directly: Cocoa points on macOS, physical pixels elsewhere (Windows and
//! X11 already lay out displays of different scales in one pixel space).
//! winit's own positions on macOS are Cocoa points times the queried
//! window's scale factor, which differ between displays; the runner
//! converts them. A window's content area spans `size * desktop_scale`
//! desktop units, where [`WindowPlacement::desktop_scale`] is 1 on macOS and
//! the window's scale factor elsewhere.
//!
//! Wayland has no desktop space an app can see: positions are `None`, and
//! [`PlatformCapabilities`] says so.

use winit::dpi::Position;
use winit::monitor::MonitorHandle;

use super::*;

/// A point on the desktop, in desktop units (see the [module docs](self)).
pub type DesktopPoint = (f64, f64);

/// One window's place on the desktop, read when asked for.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowPlacement {
    /// The content area's top-left corner. `None` where the platform does
    /// not tell windows where they are (Wayland).
    pub inner_position: Option<DesktopPoint>,
    /// The window's top-left corner, decorations included. `None` on
    /// Wayland.
    pub outer_position: Option<DesktopPoint>,
    /// The content area in logical points.
    pub size: (f32, f32),
    pub scale_factor: f64,
    /// Desktop units per logical point of this window: 1 on macOS, the
    /// scale factor elsewhere.
    pub desktop_scale: f64,
    /// The display the window is mostly on, if the platform says.
    pub monitor: Option<MonitorInfo>,
    pub maximized: bool,
    /// `None` where the platform cannot tell (Wayland).
    pub minimized: Option<bool>,
    /// Whether the window has keyboard focus: it is the active window.
    pub focused: bool,
}

impl WindowPlacement {
    /// How far the content area's corner sits from the window's corner:
    /// the title bar and left border, in desktop units. `None` on Wayland.
    pub fn client_offset(&self) -> Option<DesktopPoint> {
        let (inner, outer) = (self.inner_position?, self.outer_position?);
        Some((inner.0 - outer.0, inner.1 - outer.1))
    }
}

/// A display as winit reports it: bounds in winit's physical pixels (on
/// macOS, Cocoa points times the display's own scale), and no work area,
/// which winit 0.30 does not report.
pub(super) fn monitor_info(monitor: &MonitorHandle) -> MonitorInfo {
    let position = monitor.position();
    let size = monitor.size();
    MonitorInfo {
        name: monitor.name(),
        bounds: PhysicalRect {
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
        },
        scale_factor: monitor.scale_factor(),
        work_area: None,
    }
}

/// The placement record of a window placed as `placement`: its normal size
/// and position, carried over from `previous` while it is maximized or
/// minimized ([`PlacementRecord::capture`]).
pub(super) fn capture(
    placement: &WindowPlacement,
    previous: Option<&PlacementRecord>,
) -> PlacementRecord {
    // Records hold winit's physical pixels, which on macOS are Cocoa
    // points times the display's scale, as monitor bounds are.
    let pixels = placement
        .monitor
        .as_ref()
        .map_or(1.0, |m| pixels_per_desktop_unit(m.scale_factor));
    let outer = placement
        .outer_position
        .map(|(x, y)| ((x * pixels).round() as i32, (y * pixels).round() as i32));
    let observation = WindowObservation {
        outer_position: outer,
        inner_size: (
            (f64::from(placement.size.0) * placement.scale_factor).round() as u32,
            (f64::from(placement.size.1) * placement.scale_factor).round() as u32,
        ),
        scale_factor: placement.scale_factor,
        maximized: placement.maximized,
        minimized: placement.minimized.unwrap_or(false),
        monitor: placement.monitor.as_ref(),
    };
    PlacementRecord::capture(previous, &observation)
}

/// Where a restored placement puts a window's outer corner, in desktop
/// units, for [`WindowOptions::position`]; `None` leaves it to the platform.
pub fn restored_position(
    restored: &RestoredPlacement,
    monitors: &[MonitorInfo],
) -> Option<DesktopPoint> {
    if cfg!(target_os = "macos") {
        restored.desktop_points(monitors)
    } else {
        restored.position.map(|(x, y)| (f64::from(x), f64::from(y)))
    }
}

/// What the windowing system lets the app do with window positions. The
/// same for every window of a process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlatformCapabilities {
    /// Windows have readable desktop positions
    /// ([`WindowPlacement::outer_position`], [`AppEvent::WindowMoved`]) and
    /// the app can place them ([`WindowOptions::position`],
    /// [`EventContext::set_outer_position`]). False on Wayland, where the
    /// compositor alone places windows.
    pub window_positions: bool,
    /// A pointer position in a window's logical coordinates can be put on
    /// the desktop ([`EventContext::to_desktop`]), so pointer motion in one
    /// window can be located in another. False on Wayland; drags between
    /// windows there need the compositor's drag and drop.
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

/// The native Wayland objects behind a window, for protocols winit does not
/// speak, such as attaching the window to an `xdg_toplevel_drag_v1`. Valid
/// while the window is open.
#[cfg(target_os = "linux")]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WaylandSurface {
    /// The `wl_display`.
    pub display: std::ptr::NonNull<std::ffi::c_void>,
    /// The window's `wl_surface`.
    pub surface: std::ptr::NonNull<std::ffi::c_void>,
    /// The window's `xdg_toplevel`.
    pub xdg_toplevel: std::ptr::NonNull<std::ffi::c_void>,
}

/// winit physical pixels per desktop unit for a window or display at
/// `scale_factor`.
fn pixels_per_desktop_unit(scale_factor: f64) -> f64 {
    if cfg!(target_os = "macos") {
        scale_factor
    } else {
        1.0
    }
}

/// Desktop units per logical point for a window at `scale_factor`.
pub(super) fn desktop_scale(scale_factor: f64) -> f64 {
    scale_factor / pixels_per_desktop_unit(scale_factor)
}

/// A winit position of a window at `scale_factor`, in desktop units.
fn from_winit(position: PhysicalPosition<i32>, scale_factor: f64) -> DesktopPoint {
    let per_pixel = 1.0 / pixels_per_desktop_unit(scale_factor);
    (
        f64::from(position.x) * per_pixel,
        f64::from(position.y) * per_pixel,
    )
}

/// A desktop point as winit takes it: Cocoa points on macOS, which winit
/// passes through unscaled, and physical pixels elsewhere.
pub(super) fn to_winit((x, y): DesktopPoint) -> Position {
    if cfg!(target_os = "macos") {
        Position::Logical(LogicalPosition::new(x, y))
    } else {
        Position::Physical(PhysicalPosition::new(x.round() as i32, y.round() as i32))
    }
}

/// Where a native window is, cached so the desktop conversions a drag makes
/// on every pointer move need no platform round trip. Refreshed when the
/// window opens, moves, resizes, or changes scale.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct CachedPosition {
    inner: Option<DesktopPoint>,
    outer: Option<DesktopPoint>,
}

impl WindowState {
    pub(super) fn refresh_position(&mut self) {
        let scale = self.scale_factor;
        self.position = CachedPosition {
            inner: self
                .window
                .inner_position()
                .ok()
                .map(|p| from_winit(p, scale)),
            outer: self
                .window
                .outer_position()
                .ok()
                .map(|p| from_winit(p, scale)),
        };
    }

    /// The window's outer position in desktop units, from winit's `Moved`.
    pub(super) fn desktop_position(&self, position: PhysicalPosition<i32>) -> DesktopPoint {
        from_winit(position, self.scale_factor)
    }

    pub(super) fn placement(&self) -> WindowPlacement {
        let window = &self.window;
        let scale = self.scale_factor;
        WindowPlacement {
            inner_position: self.position.inner,
            outer_position: self.position.outer,
            size: (
                (f64::from(self.surface_size.width) / scale) as f32,
                (f64::from(self.surface_size.height) / scale) as f32,
            ),
            scale_factor: scale,
            desktop_scale: desktop_scale(scale),
            monitor: window
                .current_monitor()
                .map(|monitor| monitor_info(&monitor)),
            maximized: window.is_maximized(),
            minimized: window.is_minimized(),
            focused: window.has_focus(),
        }
    }

    /// The content area's corner and desktop units per point, if known.
    pub(super) fn client_origin(&self) -> Option<(DesktopPoint, f64)> {
        Some((self.position.inner?, desktop_scale(self.scale_factor)))
    }

    /// The outer corner minus the content corner.
    pub(super) fn client_offset(&self) -> Option<DesktopPoint> {
        let (inner, outer) = (self.position.inner?, self.position.outer?);
        Some((inner.0 - outer.0, inner.1 - outer.1))
    }

    /// Move the window's outer corner to `position`. The cache follows
    /// right away, so conversions made before the platform reports the
    /// move already use it.
    pub(super) fn set_outer_position(&mut self, position: DesktopPoint) {
        self.window.set_outer_position(to_winit(position));
        if let Some(offset) = self.client_offset() {
            self.position = CachedPosition {
                inner: Some((position.0 + offset.0, position.1 + offset.1)),
                outer: Some(position),
            };
        }
    }

    #[cfg(target_os = "linux")]
    pub(super) fn wayland_surface(&self) -> Option<WaylandSurface> {
        use raw_window_handle::{
            HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
        };
        use winit::platform::wayland::WindowExtWayland;

        let RawDisplayHandle::Wayland(display) = self.window.display_handle().ok()?.as_raw() else {
            return None;
        };
        let RawWindowHandle::Wayland(surface) = self.window.window_handle().ok()?.as_raw() else {
            return None;
        };
        Some(WaylandSurface {
            display: display.display,
            surface: surface.surface,
            xdg_toplevel: self.window.xdg_toplevel()?,
        })
    }
}

/// Window-local logical `point` on the desktop, for a window whose content
/// corner is `origin` at `desktop_scale` units per point.
pub(super) fn local_to_desktop(
    (origin, desktop_scale): (DesktopPoint, f64),
    (x, y): (f32, f32),
) -> DesktopPoint {
    (
        origin.0 + f64::from(x) * desktop_scale,
        origin.1 + f64::from(y) * desktop_scale,
    )
}

/// The inverse of [`local_to_desktop`].
pub(super) fn desktop_to_local(
    (origin, desktop_scale): (DesktopPoint, f64),
    (x, y): DesktopPoint,
) -> (f32, f32) {
    (
        ((x - origin.0) / desktop_scale) as f32,
        ((y - origin.1) / desktop_scale) as f32,
    )
}

impl EventContext<'_> {
    /// Where `window` is on the desktop and how big, read now. `None` for a
    /// window not open yet, or a stale handle.
    pub fn placement(&self, window: WindowHandle) -> Option<WindowPlacement> {
        match self.windows.get(window)? {
            WindowEntry::Open(state) => Some(state.placement()),
            #[cfg(feature = "test-support")]
            WindowEntry::Virtual(virtual_window) => {
                Some(virtual_window.placement(self.capabilities))
            }
            WindowEntry::Pending(_) => None,
        }
    }

    /// The connected displays, the primary one first, as
    /// [`crate::platform::placement::restore`] takes them. Empty before any
    /// window is open. The headless runner reports the displays its windows
    /// were put on.
    pub fn monitors(&self) -> Vec<MonitorInfo> {
        let mut monitors: Vec<MonitorInfo> = Vec::new();
        for (_, entry) in self.windows.iter() {
            match entry {
                WindowEntry::Open(state) => {
                    let window = &state.window;
                    let primary = window.primary_monitor();
                    monitors.extend(primary.iter().map(monitor_info));
                    monitors.extend(
                        window
                            .available_monitors()
                            .filter(|m| Some(m) != primary.as_ref())
                            .map(|m| monitor_info(&m)),
                    );
                    return monitors;
                }
                #[cfg(feature = "test-support")]
                WindowEntry::Virtual(virtual_window) => {
                    if let Some(monitor) = &virtual_window.monitor
                        && !monitors.contains(monitor)
                    {
                        monitors.push(monitor.clone());
                    }
                }
                WindowEntry::Pending(_) => {}
            }
        }
        monitors
    }

    /// `window`'s placement as a record to save and restore it by, with
    /// its normal size and position kept from `previous` while it is
    /// maximized or minimized. `None` for a window not open.
    pub fn capture_placement(
        &self,
        window: WindowHandle,
        previous: Option<&PlacementRecord>,
    ) -> Option<PlacementRecord> {
        Some(capture(&self.placement(window)?, previous))
    }

    /// What the windowing system lets the app do with window positions.
    pub fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities
    }

    /// `point`, in `window`'s logical coordinates, on the desktop. Points
    /// outside the window work too: during a drag, platforms keep sending
    /// the pressed window motion outside it, and this places that motion
    /// on the desktop. `None` without [`PlatformCapabilities::desktop_pointer`]
    /// or for a window not open.
    pub fn to_desktop(&self, window: WindowHandle, point: (f32, f32)) -> Option<DesktopPoint> {
        Some(local_to_desktop(self.client_origin(window)?, point))
    }

    /// A desktop point in `window`'s logical coordinates, which may lie
    /// outside the window. The inverse of [`Self::to_desktop`].
    pub fn from_desktop(&self, window: WindowHandle, point: DesktopPoint) -> Option<(f32, f32)> {
        Some(desktop_to_local(self.client_origin(window)?, point))
    }

    /// The context window's last pointer position on the desktop.
    pub fn desktop_pointer(&self) -> Option<DesktopPoint> {
        self.to_desktop(self.window?, self.pointer_position()?)
    }

    /// Move `window`'s outer corner to `position`. Cheap enough to call on
    /// every pointer move. The platform reports the move with
    /// [`AppEvent::WindowMoved`]. Returns false where windows cannot be
    /// placed ([`PlatformCapabilities::window_positions`]) or the window is
    /// not open.
    pub fn set_outer_position(&mut self, window: WindowHandle, position: DesktopPoint) -> bool {
        if !self.capabilities.window_positions {
            return false;
        }
        match self.windows.get_mut(window) {
            Some(WindowEntry::Open(state)) => {
                state.set_outer_position(position);
                true
            }
            #[cfg(feature = "test-support")]
            Some(WindowEntry::Virtual(virtual_window)) => {
                virtual_window.position = position;
                self.flags.moved.push(window);
                true
            }
            _ => false,
        }
    }

    /// Move `window` so its logical content point `hotspot` lands on the
    /// desktop at `at`, decorations and scale accounted for: keeps a torn-off
    /// panel's grab point under the pointer. Returns false as
    /// [`Self::set_outer_position`] does, or before the window's position is
    /// known.
    pub fn align_window(
        &mut self,
        window: WindowHandle,
        hotspot: (f32, f32),
        at: DesktopPoint,
    ) -> bool {
        let frame = match self.windows.get(window) {
            Some(WindowEntry::Open(state)) => state
                .client_offset()
                .map(|offset| (offset, desktop_scale(state.scale_factor))),
            #[cfg(feature = "test-support")]
            Some(WindowEntry::Virtual(virtual_window)) => {
                Some((virtual_window.client_offset, virtual_window.scale_factor))
            }
            _ => None,
        };
        let Some(((dx, dy), scale)) = frame else {
            return false;
        };
        let outer = (
            at.0 - dx - f64::from(hotspot.0) * scale,
            at.1 - dy - f64::from(hotspot.1) * scale,
        );
        self.set_outer_position(window, outer)
    }

    /// The native Wayland objects behind `window`; `None` on X11 and for a
    /// window not open. For anything else native, see
    /// [`Self::window_by_handle`].
    #[cfg(target_os = "linux")]
    pub fn wayland_surface(&self, window: WindowHandle) -> Option<WaylandSurface> {
        self.windows.get(window)?.open()?.wayland_surface()
    }

    fn client_origin(&self, window: WindowHandle) -> Option<(DesktopPoint, f64)> {
        if !self.capabilities.desktop_pointer {
            return None;
        }
        match self.windows.get(window)? {
            WindowEntry::Open(state) => state.client_origin(),
            #[cfg(feature = "test-support")]
            WindowEntry::Virtual(virtual_window) => Some(virtual_window.client_origin()),
            WindowEntry::Pending(_) => None,
        }
    }
}
