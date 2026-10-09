//! The WebKitGTK backend.
//!
//! Each view is a decorated GTK top-level window holding a wry-built
//! WebKitGTK view, on quark's UI thread next to winit. GTK never runs its own
//! main loop: the runner calls [`Backend::service`], which dispatches a
//! bounded number of GLib main-context iterations without blocking.
//!
//! Wry builds the view and keeps its window plumbing; quark owns every
//! policy and lifecycle signal itself (`decide-policy`, `load-changed`,
//! `load-failed`, process termination), and wry's portable navigation, load,
//! and new-window handlers stay unset so nothing decides before quark does.

mod eval;
mod host;
mod parent;
mod profiles;

use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gtk::prelude::*;
use raw_window_handle::RawDisplayHandle;

use super::{Backend, ClearRequest, EvalDispatch, OpenRequest, Serviced};
use crate::{EvaluationId, OpenError, PlatformError, WebViewHandle};

pub(super) fn backend() -> Option<Box<dyn Backend>> {
    Some(Box::new(Linux::default()))
}

/// The display system GTK was started on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Display {
    X11,
    Wayland,
    /// GTK came up on something else (Broadway, or not the parent's
    /// system), so no native parent relationship is possible.
    Other,
}

enum Gtk {
    NotStarted,
    Ready(Display),
    /// `gtk_init` failed; it cannot be retried in this process.
    Failed(String),
}

struct Linux {
    gtk: Gtk,
    hosts: HashMap<WebViewHandle, host::Host>,
    profiles: profiles::Profiles,
    /// Native work still owed after a host or clear is gone: destroyed
    /// reports and profile clears waiting on GLib. Servicing continues
    /// until it drops to zero.
    owed: Rc<Cell<usize>>,
}

impl Default for Linux {
    fn default() -> Self {
        Self {
            gtk: Gtk::NotStarted,
            hosts: HashMap::new(),
            profiles: profiles::Profiles::default(),
            owed: Rc::new(Cell::new(0)),
        }
    }
}

impl Linux {
    /// Start GTK on this thread, on the same display system as `parent`.
    fn start_gtk(&mut self, parent: &RawDisplayHandle) -> Result<Display, OpenError> {
        match &self.gtk {
            Gtk::Ready(display) => return Ok(*display),
            Gtk::Failed(error) => {
                return Err(OpenError::Platform(PlatformError::new(
                    "start GTK",
                    error.clone(),
                )));
            }
            Gtk::NotStarted => {}
        }
        // GTK would otherwise pick Wayland whenever WAYLAND_DISPLAY is set,
        // even under an X11 winit; matching the parent keeps transient
        // parenting possible. This is GDK state, not the environment.
        match parent {
            RawDisplayHandle::Xlib(_) | RawDisplayHandle::Xcb(_) => {
                gtk::gdk::set_allowed_backends("x11")
            }
            RawDisplayHandle::Wayland(_) => gtk::gdk::set_allowed_backends("wayland"),
            _ => {}
        }
        if let Err(error) = gtk::init() {
            let error = error.to_string();
            self.gtk = Gtk::Failed(error.clone());
            return Err(OpenError::Platform(PlatformError::new("start GTK", error)));
        }
        let display = match gtk::gdk::Display::default().map(|display| display.type_().name()) {
            Some("GdkX11Display") => Display::X11,
            Some("GdkWaylandDisplay") => Display::Wayland,
            _ => Display::Other,
        };
        self.gtk = Gtk::Ready(display);
        Ok(display)
    }
}

impl Backend for Linux {
    fn open(&mut self, request: OpenRequest) -> Result<(), OpenError> {
        let display = self.start_gtk(&request.parent.display)?;
        let view = request.view;
        let host = host::Host::open(request, display, &mut self.profiles)?;
        self.hosts.insert(view, host);
        Ok(())
    }

    fn evaluate(&mut self, view: WebViewHandle, dispatch: EvalDispatch) {
        // The session only dispatches to views it has not closed.
        if let Some(host) = self.hosts.get(&view) {
            host.evaluate(dispatch);
        }
    }

    fn cancel(&mut self, view: WebViewHandle, evaluation: EvaluationId) {
        if let Some(host) = self.hosts.get(&view) {
            host.cancel(evaluation);
        }
    }

    fn close(&mut self, view: WebViewHandle) {
        if let Some(host) = self.hosts.remove(&view) {
            host.destroy(&self.owed);
        }
    }

    fn focus(&mut self, view: WebViewHandle) {
        if let Some(host) = self.hosts.get(&view) {
            host.present();
        }
    }

    fn clear_profile(&mut self, request: ClearRequest) {
        self.profiles.clear(request, &self.owed);
    }

    fn service(&mut self, iterations: u32) -> Serviced {
        if !matches!(self.gtk, Gtk::Ready(_)) {
            return Serviced::Idle;
        }
        let context = gtk::glib::MainContext::default();
        for _ in 0..iterations {
            if !context.iteration(false) {
                return Serviced::Idle;
            }
        }
        if context.pending() {
            Serviced::Exhausted
        } else {
            Serviced::Idle
        }
    }

    fn needs_service(&self) -> bool {
        matches!(self.gtk, Gtk::Ready(_)) && (!self.hosts.is_empty() || self.owed.get() > 0)
    }

    fn shutdown(&mut self) {
        for (_, host) in self.hosts.drain() {
            host.destroy(&self.owed);
        }
        // Let the destroyed reports and WebKit's own teardown messages run
        // while the context is still being serviced.
        if matches!(self.gtk, Gtk::Ready(_)) {
            let context = gtk::glib::MainContext::default();
            let mut budget = super::SERVICE_ITERATIONS;
            while budget > 0 && context.iteration(false) {
                budget -= 1;
            }
        }
    }
}
