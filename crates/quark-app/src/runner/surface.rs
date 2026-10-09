//! What a window looks like and how it moves beyond its content: its
//! background, material, and corners ([`crate::platform::material`]),
//! moving it from app-drawn chrome ([`crate::platform::chrome`]), and
//! keeping it inside the work area.

use crate::platform::chrome::{TitleAction, WindowDragError};
use crate::platform::material::{
    Backend, MaterialRect, SurfaceEnvironment, WindowBackground, WindowCorners, WindowSurface,
};
use crate::platform::placement::{DesktopRect, constrain_to_work_area, nearest_area};

use super::*;

/// A window's background and corner requests and what they resolved to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct SurfaceState {
    pub(crate) background: WindowBackground,
    pub(crate) corners: WindowCorners,
    pub(crate) environment: SurfaceEnvironment,
    pub(crate) effective: WindowSurface,
}

impl SurfaceState {
    /// Resolve `background` and `corners` on `environment`, for a surface
    /// that composites alpha when `surface_alpha`.
    pub(crate) fn resolve(
        background: WindowBackground,
        corners: WindowCorners,
        environment: SurfaceEnvironment,
        surface_alpha: bool,
    ) -> Self {
        Self {
            background,
            corners,
            environment,
            effective: WindowSurface {
                background: environment.background(background, surface_alpha),
                corners: environment.corners(corners),
            },
        }
    }

    /// The headless runner's: no desktop shows through.
    pub(crate) fn headless(background: WindowBackground, corners: WindowCorners) -> Self {
        let environment = SurfaceEnvironment {
            backend: Backend::Headless,
            reduced_transparency: false,
            increased_contrast: false,
        };
        Self::resolve(background, corners, environment, false)
    }
}

impl EventContext<'_> {
    /// What `window`'s background, material, and corners resolved to. `None`
    /// for a window not open yet, or a stale handle.
    pub fn window_surface(&self, window: WindowHandle) -> Option<WindowSurface> {
        match self.windows.get(window)? {
            WindowEntry::Open(state) => Some(state.surface.effective),
            #[cfg(feature = "test-support")]
            WindowEntry::Virtual(virtual_window) => Some(virtual_window.surface.effective),
            WindowEntry::Pending(_) => None,
        }
    }

    /// Change `window`'s background, for example to a new theme's fallback
    /// color, and return what it resolved to. A window opened opaque may be
    /// unable to become transparent (X11 picks its visual at creation); it
    /// then reports the fallback.
    pub fn set_window_background(
        &mut self,
        window: WindowHandle,
        background: WindowBackground,
    ) -> Option<WindowSurface> {
        match self.windows.get_mut(window)? {
            WindowEntry::Open(state) => {
                state.set_background(background);
                Some(state.surface.effective)
            }
            #[cfg(feature = "test-support")]
            WindowEntry::Virtual(virtual_window) => {
                let surface = &mut virtual_window.surface;
                *surface =
                    SurfaceState::resolve(background, surface.corners, surface.environment, false);
                Some(surface.effective)
            }
            WindowEntry::Pending(_) => None,
        }
    }

    /// Show a native material of its own kind in each of `regions`, which
    /// replace the previous set. Only macOS has materials per region
    /// ([`crate::platform::material::MaterialScope::Regions`]); elsewhere the
    /// window's one backdrop shows through transparent pixels and this does
    /// nothing. Cheap to call every frame: unchanged regions keep their
    /// native views.
    pub fn set_material_regions(&mut self, window: WindowHandle, regions: &[MaterialRect]) {
        if let Some(WindowEntry::Open(state)) = self.windows.get_mut(window) {
            state.set_material_regions(regions);
        }
    }

    /// Let the window manager move `window` with the pointer, as from a
    /// native title bar. Call it while handling the primary button press
    /// that starts the move; see [`crate::platform::chrome`].
    pub fn start_window_drag(&mut self, window: WindowHandle) -> Result<(), WindowDragError> {
        match self.windows.get_mut(window) {
            Some(WindowEntry::Open(state)) => {
                state.window.drag_window().map_err(|error| match error {
                    winit::error::ExternalError::NotSupported(_) => WindowDragError::Unsupported,
                    other => WindowDragError::Platform(other.to_string()),
                })
            }
            #[cfg(feature = "test-support")]
            Some(WindowEntry::Virtual(virtual_window)) => {
                virtual_window.window_drags += 1;
                Ok(())
            }
            _ => Err(WindowDragError::NotOpen),
        }
    }

    /// Do what a double-click on `window`'s title bar does on this
    /// platform, for an app-drawn title bar.
    pub fn title_double_click(&mut self, window: WindowHandle) -> TitleAction {
        let Some(WindowEntry::Open(state)) = self.windows.get(window) else {
            return TitleAction::Nothing;
        };
        let action = title_double_click_action();
        match action {
            TitleAction::ToggleMaximize => state.window.set_maximized(!state.window.is_maximized()),
            TitleAction::Minimize => state.window.set_minimized(true),
            TitleAction::Nothing => {}
        }
        action
    }

    /// Move, and shrink if it must, `window` so all of it, decorations
    /// included, lies inside a display's work area: the one containing
    /// `anchor` (a desktop point, such as where a drag was released), else
    /// the one nearest the window's center. Returns whether it changed the
    /// window. Does nothing where windows cannot be placed
    /// ([`PlatformCapabilities::window_positions`]) or no display is known.
    pub fn fit_window_to_work_area(
        &mut self,
        window: WindowHandle,
        anchor: Option<DesktopPoint>,
    ) -> bool {
        if !self.capabilities.window_positions {
            return false;
        }
        let Some(frame) = self.outer_frame(window) else {
            return false;
        };
        let monitors = self.monitors();
        let (bounds, usable): (Vec<DesktopRect>, Vec<DesktopRect>) =
            monitors.iter().map(desktop_areas).unzip();
        let center = (
            frame.outer.x + frame.outer.width / 2.0,
            frame.outer.y + frame.outer.height / 2.0,
        );
        let Some(index) = nearest_area(&bounds, anchor.unwrap_or(center)) else {
            return false;
        };
        let fitted = constrain_to_work_area(frame.outer, usable[index], frame.min_outer);
        if fitted == frame.outer {
            return false;
        }
        if (fitted.width, fitted.height) != (frame.outer.width, frame.outer.height) {
            let content = (
                (fitted.width - frame.decorations.0) / frame.desktop_scale,
                (fitted.height - frame.decorations.1) / frame.desktop_scale,
            );
            self.resize_window_to(window, content);
        }
        self.set_outer_position(window, (fitted.x, fitted.y))
    }

    /// `window`'s outer rectangle in desktop units, and what it takes to
    /// convert between that and its content size.
    fn outer_frame(&self, window: WindowHandle) -> Option<OuterFrame> {
        let placement = self.placement(window)?;
        let (x, y) = placement.outer_position?;
        let desktop_scale = placement.desktop_scale;
        let content = (
            f64::from(placement.size.0) * desktop_scale,
            f64::from(placement.size.1) * desktop_scale,
        );
        let (decorations, min_size) = match self.windows.get(window)? {
            WindowEntry::Open(state) => {
                let outer = state.window.outer_size();
                let per_unit = desktop_scale / state.scale_factor;
                (
                    (
                        (f64::from(outer.width) * per_unit - content.0).max(0.0),
                        (f64::from(outer.height) * per_unit - content.1).max(0.0),
                    ),
                    state.min_size,
                )
            }
            #[cfg(feature = "test-support")]
            WindowEntry::Virtual(virtual_window) => {
                (virtual_window.client_offset, virtual_window.min_size)
            }
            WindowEntry::Pending(_) => return None,
        };
        let min_size = min_size.unwrap_or((0.0, 0.0));
        Some(OuterFrame {
            outer: DesktopRect {
                x,
                y,
                width: content.0 + decorations.0,
                height: content.1 + decorations.1,
            },
            decorations,
            desktop_scale,
            min_outer: (
                min_size.0 * desktop_scale + decorations.0,
                min_size.1 * desktop_scale + decorations.1,
            ),
        })
    }

    /// Ask for `window`'s content to be `size` logical points.
    fn resize_window_to(&mut self, window: WindowHandle, size: (f64, f64)) {
        match self.windows.get_mut(window) {
            Some(WindowEntry::Open(state)) => {
                let _ = state
                    .window
                    .request_inner_size(LogicalSize::new(size.0, size.1));
            }
            #[cfg(feature = "test-support")]
            Some(WindowEntry::Virtual(virtual_window)) => {
                virtual_window.size = (size.0 as f32, size.1 as f32);
                self.flags.redraw.push(window);
            }
            _ => {}
        }
    }
}

/// A window's outer rectangle and how it relates to its content.
struct OuterFrame {
    outer: DesktopRect,
    /// Outer size minus content size, in desktop units.
    decorations: (f64, f64),
    desktop_scale: f64,
    /// The smallest outer size, from the window's minimum content size.
    min_outer: (f64, f64),
}

/// A display's bounds and work area in desktop units. winit reports
/// displays in its physical pixels, which on macOS are points times the
/// display's own scale.
fn desktop_areas(monitor: &MonitorInfo) -> (DesktopRect, DesktopRect) {
    let per_unit = if cfg!(target_os = "macos") {
        monitor.scale_factor
    } else {
        1.0
    };
    let convert = |rect: PhysicalRect| DesktopRect {
        x: f64::from(rect.x) / per_unit,
        y: f64::from(rect.y) / per_unit,
        width: f64::from(rect.width) / per_unit,
        height: f64::from(rect.height) / per_unit,
    };
    (convert(monitor.bounds), convert(monitor.usable()))
}

/// What the platform does on a title bar double-click.
fn title_double_click_action() -> TitleAction {
    TitleAction::ToggleMaximize
}

impl WindowState {
    pub(super) fn set_background(&mut self, background: WindowBackground) {
        let surface = &self.surface;
        self.surface = SurfaceState::resolve(
            background,
            surface.corners,
            surface.environment,
            self.surface_alpha,
        );
    }

    pub(super) fn set_material_regions(&mut self, _regions: &[MaterialRect]) {}
}
