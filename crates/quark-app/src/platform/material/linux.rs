//! Linux: KWin's blur behind windows, on X11 through its window property
//! and on Wayland through `org_kde_kwin_blur_manager` (which winit binds
//! for `Window::set_blur`). Other compositors have no blur a client can
//! ask for, so materials fall back there.

use std::sync::OnceLock;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::window::Window;

use super::{EffectiveBackground, MaterialRect, WindowSurface};

#[derive(Default)]
pub(crate) struct NativeMaterial;

impl NativeMaterial {
    /// Ask the compositor to blur behind `window` when `surface` resolved
    /// to a material. Returns false when the request could not be sent.
    pub(crate) fn apply(&mut self, window: &Window, surface: &WindowSurface) -> bool {
        let blur = matches!(surface.background, EffectiveBackground::Material { .. });
        match window.window_handle().map(|handle| handle.as_raw()) {
            Ok(RawWindowHandle::Xlib(handle)) => {
                crate::platform::x11_root::set_blur(handle.window as u32, blur)
            }
            Ok(RawWindowHandle::Xcb(handle)) => {
                crate::platform::x11_root::set_blur(handle.window.get(), blur)
            }
            Ok(RawWindowHandle::Wayland(_)) => {
                window.set_blur(blur);
                true
            }
            _ => !blur,
        }
    }

    /// One blur serves the whole window here.
    pub(crate) fn set_regions(&mut self, _window: &Window, _regions: &[MaterialRect]) {}
}

/// Whether the Wayland compositor behind `display` has KWin's blur
/// manager, read once per process from a registry of our own.
pub(crate) fn wayland_blur(display: std::ptr::NonNull<std::ffi::c_void>) -> bool {
    static BLUR: OnceLock<bool> = OnceLock::new();
    *BLUR.get_or_init(|| {
        use wayland_client::globals::{GlobalListContents, registry_queue_init};
        use wayland_client::protocol::wl_registry::{self, WlRegistry};
        use wayland_client::{Connection, Dispatch, QueueHandle};

        struct Globals;
        impl Dispatch<WlRegistry, GlobalListContents> for Globals {
            fn event(
                _: &mut Self,
                _: &WlRegistry,
                _: wl_registry::Event,
                _: &GlobalListContents,
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }

        // SAFETY: winit's live wl_display, connected for the event loop's
        // life; this only reads the global list on a queue of its own.
        let backend = unsafe {
            wayland_client::backend::Backend::from_foreign_display(display.as_ptr().cast())
        };
        let conn = Connection::from_backend(backend);
        let Ok((globals, _queue)) = registry_queue_init::<Globals>(&conn) else {
            return false;
        };
        globals.contents().with_list(|list| {
            list.iter()
                .any(|g| g.interface == "org_kde_kwin_blur_manager")
        })
    })
}
