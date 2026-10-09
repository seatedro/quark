//! Native parenting of the GTK window under quark's winit window.
//!
//! GTK and winit hold separate display connections. On X11 a window id is
//! valid on any connection, so GDK wraps the parent's XID as a foreign
//! window and sets it as the transient parent (WM_TRANSIENT_FOR) without
//! ever owning or destroying it. On Wayland a surface from winit's
//! connection means nothing to GTK's, so the parent toplevel is exported
//! through xdg-foreign on winit's connection and GTK imports the handle.
//! Either step can be missing (no exporter global, a GTK without the
//! Wayland call, GTK on another display system); the view then opens
//! unparented and the runner's parent lock is the only modality.

use std::ffi::{CString, c_void};

use gtk::glib::translate::{FromGlibPtrFull, ToGlibPtr};
use gtk::prelude::*;
use raw_window_handle::{RawDisplayHandle, RawWindowHandle};

use super::Display;
use crate::ParentRelationship;
use crate::backend::NativeParent;

/// What keeps a native parent relationship alive; dropped with the host.
pub(super) enum Link {
    /// The parent's XID wrapped by GDK. Foreign windows are never destroyed
    /// through GDK, so dropping this leaves the parent alone.
    X11(gtk::gdk::Window),
    /// The parent's xdg-foreign export, revoked on drop.
    Wayland(#[allow(dead_code)] wayland::Export),
}

impl Link {
    /// Give the keyboard back to the parent after the modal closes.
    pub(super) fn focus_parent(&self) {
        match self {
            // `_NET_ACTIVE_WINDOW` from an application, as winit's own
            // focus_window does.
            Self::X11(parent) => parent.focus(gtk::current_event_time()),
            // Wayland compositors move focus to the parent themselves when
            // a transient child closes; clients cannot request it.
            Self::Wayland(_) => {}
        }
    }
}

/// Make `window` (realized, not yet mapped) a transient child of `parent`.
pub(super) fn attach(
    window: &gtk::Window,
    parent: &NativeParent,
    display: Display,
) -> (ParentRelationship, Option<Link>) {
    let link = match (display, parent.window, parent.display) {
        (Display::X11, RawWindowHandle::Xlib(handle), _) => x11(window, handle.window),
        (Display::X11, RawWindowHandle::Xcb(handle), _) => x11(window, handle.window.get().into()),
        (
            Display::Wayland,
            RawWindowHandle::Wayland(surface),
            RawDisplayHandle::Wayland(wl_display),
        ) => wayland_parent(
            window,
            wl_display.display.as_ptr(),
            surface.surface.as_ptr(),
        ),
        _ => None,
    };
    match link {
        Some(link) => (ParentRelationship::Native, Some(link)),
        None => (ParentRelationship::AppEnforced, None),
    }
}

fn x11(window: &gtk::Window, xid: std::ffi::c_ulong) -> Option<Link> {
    let gdk_window = window.window()?;
    let display = gdk_window.display();
    let display = display.downcast_ref::<gdkx11::X11Display>()?;
    // SAFETY: a live GdkX11Display. GDK traps the X error a stale XID
    // causes and returns null instead, which the check below handles.
    let foreign = unsafe {
        let raw =
            gdkx11::ffi::gdk_x11_window_foreign_new_for_display(display.to_glib_none().0, xid);
        if raw.is_null() {
            return None;
        }
        gtk::gdk::Window::from_glib_full(raw)
    };
    gdk_window.set_transient_for(&foreign);
    gdk_window.set_modal_hint(true);
    Some(Link::X11(foreign))
}

/// Center `window` on the parent's frame where the parent's position is
/// known (X11); Wayland clients do not place windows.
pub(super) fn center_on(window: &gtk::Window, link: Option<&Link>, size: (i32, i32)) {
    let Some(Link::X11(parent)) = link else {
        return;
    };
    let frame = parent.frame_extents();
    let x = frame.x() + (frame.width() - size.0) / 2;
    let y = frame.y() + (frame.height() - size.1) / 2;
    window.move_(x.max(0), y.max(0));
}

fn wayland_parent(
    window: &gtk::Window,
    display: *mut c_void,
    surface: *mut c_void,
) -> Option<Link> {
    let set_transient = set_transient_for_exported()?;
    let export = wayland::Export::new(display, surface)?;
    let gdk_window = window.window()?;
    let handle = CString::new(export.handle()).ok()?;
    // SAFETY: a realized GdkWaylandWindow (GTK runs on Wayland here) and a
    // NUL-terminated handle; GTK copies the string.
    let imported = unsafe { set_transient(gdk_window.to_glib_none().0, handle.as_ptr()) };
    (imported != 0).then_some(Link::Wayland(export))
}

type SetTransientForExported =
    unsafe extern "C" fn(*mut gtk::gdk::ffi::GdkWindow, *const std::ffi::c_char) -> i32;

/// `gdk_wayland_window_set_transient_for_exported`, looked up at run time:
/// GTK 3.22+ built with its Wayland backend has it, and linking it directly
/// would fail on a GTK built without one.
fn set_transient_for_exported() -> Option<SetTransientForExported> {
    // SAFETY: dlsym on the global namespace with a static NUL-terminated
    // name; libgdk-3 is already loaded through gtk.
    let symbol = unsafe {
        libc::dlsym(
            libc::RTLD_DEFAULT,
            c"gdk_wayland_window_set_transient_for_exported".as_ptr(),
        )
    };
    // SAFETY: the symbol has this signature in every GTK 3 since 3.22.
    (!symbol.is_null())
        .then(|| unsafe { std::mem::transmute::<*mut c_void, SetTransientForExported>(symbol) })
}

mod wayland {
    use std::ffi::c_void;

    use wayland_client::backend::{Backend, ObjectId};
    use wayland_client::globals::{GlobalListContents, registry_queue_init};
    use wayland_client::protocol::wl_registry::{self, WlRegistry};
    use wayland_client::protocol::wl_surface::WlSurface;
    use wayland_client::{Connection, Dispatch, EventQueue, Proxy, QueueHandle};
    use wayland_protocols::xdg::foreign::zv1::client::zxdg_exported_v1::{self, ZxdgExportedV1};
    use wayland_protocols::xdg::foreign::zv1::client::zxdg_exporter_v1::ZxdgExporterV1;
    use wayland_protocols::xdg::foreign::zv2::client::zxdg_exported_v2::{self, ZxdgExportedV2};
    use wayland_protocols::xdg::foreign::zv2::client::zxdg_exporter_v2::ZxdgExporterV2;

    /// An exported parent toplevel on winit's connection, on a queue of its
    /// own. The handle stays valid until this drops.
    pub(in super::super) struct Export {
        conn: Connection,
        _queue: EventQueue<State>,
        exported: Exported,
        handle: String,
    }

    enum Exported {
        V2(ZxdgExportedV2),
        V1(ZxdgExportedV1),
    }

    #[derive(Default)]
    struct State {
        handle: Option<String>,
    }

    impl Export {
        /// Export winit's `surface` on `display`. None when the compositor
        /// has no xdg-foreign exporter or never answers with a handle.
        pub(in super::super) fn new(display: *mut c_void, surface: *mut c_void) -> Option<Self> {
            // SAFETY: winit's live wl_display, connected for the event
            // loop's life; this uses only a queue of its own on it.
            let backend = unsafe { Backend::from_foreign_display(display.cast()) };
            let conn = Connection::from_backend(backend);
            let (globals, mut queue) = registry_queue_init::<State>(&conn).ok()?;
            let qh = queue.handle();
            // SAFETY: winit's window surface, live while its window is,
            // which the runner keeps open longer than this view.
            let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }.ok()?;
            let surface = WlSurface::from_id(&conn, id).ok()?;
            // v2 first: its handle is meant for toplevels. Compositors keep
            // one handle namespace for both, so GTK's importer of either
            // version accepts it.
            let exported =
                if let Ok(exporter) = globals.bind::<ZxdgExporterV2, _, _>(&qh, 1..=1, ()) {
                    let exported = Exported::V2(exporter.export_toplevel(&surface, &qh, ()));
                    exporter.destroy();
                    exported
                } else {
                    let exporter = globals.bind::<ZxdgExporterV1, _, _>(&qh, 1..=1, ()).ok()?;
                    let exported = Exported::V1(exporter.export(&surface, &qh, ()));
                    exporter.destroy();
                    exported
                };
            let mut state = State::default();
            // The handle event follows the export request at once; two
            // round trips bound the wait even for a slow compositor.
            for _ in 0..2 {
                queue.roundtrip(&mut state).ok()?;
                if state.handle.is_some() {
                    break;
                }
            }
            let Some(handle) = state.handle.take() else {
                exported.destroy();
                let _ = conn.flush();
                return None;
            };
            Some(Self {
                conn,
                _queue: queue,
                exported,
                handle,
            })
        }

        pub(in super::super) fn handle(&self) -> &str {
            &self.handle
        }
    }

    impl Exported {
        fn destroy(&self) {
            match self {
                Self::V2(exported) => exported.destroy(),
                Self::V1(exported) => exported.destroy(),
            }
        }
    }

    impl Drop for Export {
        fn drop(&mut self) {
            self.exported.destroy();
            let _ = self.conn.flush();
        }
    }

    impl Dispatch<WlRegistry, GlobalListContents> for State {
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

    impl Dispatch<ZxdgExporterV2, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZxdgExporterV2,
            _: <ZxdgExporterV2 as Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZxdgExporterV1, ()> for State {
        fn event(
            _: &mut Self,
            _: &ZxdgExporterV1,
            _: <ZxdgExporterV1 as Proxy>::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
        }
    }

    impl Dispatch<ZxdgExportedV2, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZxdgExportedV2,
            event: zxdg_exported_v2::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zxdg_exported_v2::Event::Handle { handle } = event {
                state.handle = Some(handle);
            }
        }
    }

    impl Dispatch<ZxdgExportedV1, ()> for State {
        fn event(
            state: &mut Self,
            _: &ZxdgExportedV1,
            event: zxdg_exported_v1::Event,
            _: &(),
            _: &Connection,
            _: &QueueHandle<Self>,
        ) {
            if let zxdg_exported_v1::Event::Handle { handle } = event {
                state.handle = Some(handle);
            }
        }
    }
}
