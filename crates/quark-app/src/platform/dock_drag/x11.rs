//! The X11 window stack, read on a connection of our own: winit reads
//! every event on its connection, and these are plain round trips.
//!
//! The root window's children, bottom to top, are the desktop's top-level
//! windows: window manager frames around clients (ours among them), and
//! override-redirect windows such as menus. The topmost mapped one whose
//! rectangle holds the point is on top there. Our windows are found by
//! their frame, the root child they sit in.

use std::collections::HashMap;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, KeyButMask, MapState, Window, WindowClass};
use x11rb::rust_connection::RustConnection;

use super::{StackHit, WindowStack};
use crate::runner::{DesktopPoint, EventContext, WindowHandle};

pub(super) struct X11Stack {
    conn: RustConnection,
    root: Window,
    /// Each client window's root child, as last found.
    frames: HashMap<Window, Window>,
}

impl X11Stack {
    pub(super) fn connect() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|error| error.to_string())?;
        let root = conn.setup().roots[screen].root;
        Ok(Self {
            conn,
            root,
            frames: HashMap::new(),
        })
    }

    /// The root's children, bottom to top.
    fn children(&self) -> Option<Vec<Window>> {
        Some(self.conn.query_tree(self.root).ok()?.reply().ok()?.children)
    }

    /// The root child `client` sits in, among `children`.
    fn frame(&mut self, client: Window, children: &[Window]) -> Option<Window> {
        // The window manager reparents a window some time after it maps (a
        // torn-off window mid-drag), so a frame found before counts only
        // while it is still a root child.
        if let Some(&frame) = self.frames.get(&client)
            && children.contains(&frame)
        {
            return Some(frame);
        }
        let mut window = client;
        loop {
            let parent = self.conn.query_tree(window).ok()?.reply().ok()?.parent;
            if parent == self.root {
                break;
            }
            window = parent;
        }
        self.frames.insert(client, window);
        Some(window)
    }

    /// `children` from top to bottom that are shown and hold `(x, y)`,
    /// with every request sent before the first reply is read.
    fn under(&self, children: &[Window], (x, y): (i16, i16)) -> Option<Vec<Window>> {
        let cookies: Vec<_> = children
            .iter()
            .rev()
            .map(|&window| {
                let attributes = self.conn.get_window_attributes(window).ok()?;
                let geometry = self.conn.get_geometry(window).ok()?;
                Some((window, attributes, geometry))
            })
            .collect::<Option<_>>()?;
        let mut hits = Vec::new();
        for (window, attributes, geometry) in cookies {
            // A window gone meanwhile errors; it holds nothing.
            let (Ok(attributes), Ok(geometry)) = (attributes.reply(), geometry.reply()) else {
                continue;
            };
            // Input-only windows show nothing; some window managers spread
            // them over the screen.
            if attributes.map_state != MapState::VIEWABLE
                || attributes.class == WindowClass::INPUT_ONLY
            {
                continue;
            }
            let border = i32::from(geometry.border_width) * 2;
            let (left, top) = (i32::from(geometry.x), i32::from(geometry.y));
            let right = left + i32::from(geometry.width) + border;
            let bottom = top + i32::from(geometry.height) + border;
            if (left..right).contains(&i32::from(x)) && (top..bottom).contains(&i32::from(y)) {
                hits.push(window);
            }
        }
        Some(hits)
    }
}

/// The X window behind one of the app's windows.
fn client(cx: &EventContext, window: WindowHandle) -> Option<Window> {
    match cx.window_by_handle(window)?.window_handle().ok()?.as_raw() {
        RawWindowHandle::Xlib(handle) => Window::try_from(handle.window).ok(),
        RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
        _ => None,
    }
}

impl WindowStack for X11Stack {
    fn hit(&mut self, cx: &EventContext, at: DesktopPoint, skip: Option<WindowHandle>) -> StackHit {
        // X11 desktop units are root window pixels.
        let clamp = |v: f64| v.round().clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16;
        let Some(children) = self.children() else {
            return StackHit::Unknown;
        };
        let mut owned = Vec::new();
        for window in cx.windows() {
            if let Some(frame) = client(cx, window).and_then(|client| self.frame(client, &children))
            {
                owned.push((frame, window));
            }
        }
        let skipped = skip.and_then(|skip| owned.iter().find(|(_, w)| *w == skip).map(|(f, _)| *f));
        let Some(under) = self.under(&children, (clamp(at.0), clamp(at.1))) else {
            return StackHit::Unknown;
        };
        let Some(top) = under.into_iter().find(|&frame| Some(frame) != skipped) else {
            return StackHit::Other;
        };
        owned
            .iter()
            .find(|(frame, _)| *frame == top)
            .map_or(StackHit::Other, |&(_, window)| StackHit::Owned(window))
    }

    fn primary_held(&mut self) -> Option<bool> {
        let reply = self.conn.query_pointer(self.root).ok()?.reply().ok()?;
        Some(reply.mask.contains(KeyButMask::BUTTON1))
    }
}
