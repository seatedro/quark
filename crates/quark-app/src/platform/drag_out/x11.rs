//! Dragging out on X11: the source side of XDND (version 5).
//!
//! winit reads every event on its own connection, so the drag runs on a
//! connection of ours, from a thread, with an unmapped window of ours as
//! the XDND source and `XdndSelection` owner (targets answer that window,
//! and selection requests go to the client that owns the selection).
//!
//! The press that started the drag left the pointer grabbed by winit's
//! client, and X refuses a grab while another client holds one. So the drag
//! first releases winit's grab through winit's own connection (the only
//! client allowed to), then grabs the pointer for itself until the release.
//!
//! The drag image is an override-redirect window of ours that follows the
//! pointer. Its input region is empty, so the pointer is never "in" it and
//! XDND target lookup, which asks the server which child holds the pointer,
//! looks straight through it. Without a compositor its alpha cannot blend,
//! so it is flattened onto an opaque background.

use std::os::fd::AsRawFd;
use std::time::{Duration, Instant};

use x11rb::connection::{Connection, RequestConnection as _};
use x11rb::protocol::Event;
use x11rb::protocol::shape::{self, ConnectionExt as _};
use x11rb::protocol::xinput::ConnectionExt as _;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ClipOrdering, ColormapAlloc,
    ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, GrabMode,
    GrabStatus, ImageFormat, ImageOrder, PropMode, SELECTION_NOTIFY_EVENT, Screen,
    SelectionNotifyEvent, SelectionRequestEvent, StackMode, VisualClass, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::xcb_ffi::XCBConnection;
use x11rb::{CURRENT_TIME, NONE};

use super::{DragImage, DragOutError};

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        XdndAware,
        XdndProxy,
        XdndSelection,
        XdndEnter,
        XdndPosition,
        XdndStatus,
        XdndLeave,
        XdndDrop,
        XdndFinished,
        XdndActionCopy,
        TARGETS,
        TEXT_URI_LIST: b"text/uri-list",
        _NET_WM_WINDOW_TYPE,
        _NET_WM_WINDOW_TYPE_DND,
    }
}

/// The XDND version this source speaks.
const VERSION: u32 = 5;

/// How long a released drag waits on the target: for its status, then for
/// it to fetch the files and finish.
const DROP_TIMEOUT: Duration = Duration::from_secs(10);

/// `XK_Escape`.
const ESCAPE: u32 = 0xff1b;

/// What the drag image is flattened onto without a compositor: the light
/// gray of a file manager's background.
const BACKGROUND: [u8; 3] = [0xf0, 0xf0, 0xf0];

pub(super) fn start(
    display: *mut std::ffi::c_void,
    uris: Vec<u8>,
    image: &DragImage,
) -> Result<(), DragOutError> {
    let session = Session::open(display, uris, image).map_err(DragOutError::Platform)?;
    std::thread::Builder::new()
        .name("quark-drag-out".into())
        .spawn(move || {
            if let Err(error) = session.run() {
                tracing::warn!("drag out: {error}");
            }
        })
        .map_err(|error| DragOutError::Platform(error.to_string()))?;
    Ok(())
}

/// End the implicit grab winit's client got from the press, on winit's
/// connection, and wait until the server has done it.
fn release_winit_grab(display: *mut std::ffi::c_void) -> Result<(), String> {
    let xlib_xcb = x11_dl::xlib_xcb::Xlib_xcb::open().map_err(|error| error.to_string())?;
    // SAFETY: `display` is winit's live Xlib display, and the XCB connection
    // under it lives as long. Only requests go out on it: winit stays the
    // one reading its events.
    let conn = unsafe {
        let raw = (xlib_xcb.XGetXCBConnection)(display.cast());
        XCBConnection::from_raw_xcb_connection(raw, false)
    }
    .map_err(|error| error.to_string())?;
    // winit selects XInput 2 button events, so the grab is an XI2 one,
    // which the core UngrabPointer leaves alone.
    let pointer = conn
        .xinput_xi_get_client_pointer(NONE)
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?;
    conn.xinput_xi_ungrab_device(CURRENT_TIME, pointer.deviceid)
        .map_err(|error| error.to_string())?;
    conn.ungrab_pointer(CURRENT_TIME)
        .map_err(|error| error.to_string())?;
    // A round trip, so our grab on the other connection comes after.
    conn.get_input_focus()
        .map_err(|error| error.to_string())?
        .reply()
        .map_err(|error| error.to_string())?;
    Ok(())
}

struct Session {
    conn: RustConnection,
    atoms: Atoms,
    root: Window,
    source: Window,
    escape: Option<u8>,
    uris: Vec<u8>,
    image: Option<ImageWindow>,
}

impl Session {
    /// Connect and get everything ready, then take the pointer from winit.
    fn open(
        display: *mut std::ffi::c_void,
        uris: Vec<u8>,
        image: &DragImage,
    ) -> Result<Self, String> {
        let error = |error: &dyn std::fmt::Display| error.to_string();
        let (conn, screen) = x11rb::connect(None).map_err(|e| error(&e))?;
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn)
            .map_err(|e| error(&e))?
            .reply()
            .map_err(|e| error(&e))?;
        let source = conn.generate_id().map_err(|e| error(&e))?;
        conn.create_window(
            0,
            source,
            root,
            -1,
            -1,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )
        .map_err(|e| error(&e))?;
        conn.set_selection_owner(source, atoms.XdndSelection, CURRENT_TIME)
            .map_err(|e| error(&e))?;
        // Ready before the grab, so the drag only has to map it.
        let image = match ImageWindow::create(&conn, screen, &atoms, image) {
            Ok(image) => image,
            Err(e) => {
                tracing::warn!("drag out: no drag image: {e}");
                None
            }
        };
        let cursor = hand_cursor(&conn).unwrap_or(NONE);
        release_winit_grab(display)?;
        let grab = conn
            .grab_pointer(
                false,
                root,
                EventMask::POINTER_MOTION | EventMask::BUTTON_RELEASE,
                GrabMode::ASYNC,
                GrabMode::ASYNC,
                NONE,
                cursor,
                CURRENT_TIME,
            )
            .map_err(|e| error(&e))?
            .reply()
            .map_err(|e| error(&e))?;
        if grab.status != GrabStatus::SUCCESS {
            return Err(format!("could not grab the pointer: {:?}", grab.status));
        }
        // Escape cancels; without the keyboard the drag still ends on release.
        let _ = conn
            .grab_keyboard(false, root, CURRENT_TIME, GrabMode::ASYNC, GrabMode::ASYNC)
            .map(|cookie| cookie.reply());
        let escape = escape_keycode(&conn);
        Ok(Self {
            conn,
            atoms,
            root,
            source,
            escape,
            uris,
            image,
        })
    }

    fn run(self) -> Result<(), String> {
        let error = |error: &dyn std::fmt::Display| error.to_string();
        let mut drag = Drag::default();
        let mut deadline = None;
        let at = self
            .conn
            .query_pointer(self.root)
            .map_err(|e| error(&e))?
            .reply()
            .map_err(|e| error(&e))?;
        if let Some(image) = &self.image {
            image.show(&self.conn, at.root_x, at.root_y);
        }
        let target = find_target(&self.lookup(), self.root);
        self.send(drag.moved(target, at.root_x, at.root_y, CURRENT_TIME))?;
        while drag.phase() != Phase::Over {
            let Some(event) = self.next_event(deadline)? else {
                // The target never answered: give up on it.
                self.send(drag.cancel())?;
                break;
            };
            let sends = match event {
                Event::MotionNotify(event) => {
                    if let Some(image) = &self.image {
                        image.move_to(&self.conn, event.root_x, event.root_y);
                    }
                    let target = find_target(&self.lookup(), self.root);
                    drag.moved(target, event.root_x, event.root_y, event.time)
                }
                Event::ButtonRelease(event) => {
                    self.ungrab();
                    deadline = Some(Instant::now() + DROP_TIMEOUT);
                    drag.released(event.time)
                }
                Event::KeyPress(event) if Some(event.detail) == self.escape => {
                    self.ungrab();
                    drag.cancel()
                }
                Event::ClientMessage(event) if event.type_ == self.atoms.XdndStatus => {
                    let data = event.data.as_data32();
                    drag.status(data[0], data[1] & 1 != 0)
                }
                Event::ClientMessage(event) if event.type_ == self.atoms.XdndFinished => {
                    drag.finished(event.data.as_data32()[0]);
                    Sends::default()
                }
                Event::SelectionRequest(request) => {
                    self.answer(&request)?;
                    Sends::default()
                }
                _ => Sends::default(),
            };
            self.send(sends)?;
        }
        self.ungrab();
        let _ = self.conn.destroy_window(self.source);
        if let Some(image) = &self.image {
            image.destroy(&self.conn);
        }
        let _ = self.conn.flush();
        Ok(())
    }

    /// Let go of the pointer and keyboard, and take the drag image down.
    fn ungrab(&self) {
        let _ = self.conn.ungrab_pointer(CURRENT_TIME);
        let _ = self.conn.ungrab_keyboard(CURRENT_TIME);
        if let Some(image) = &self.image {
            let _ = self.conn.unmap_window(image.window);
        }
        let _ = self.conn.flush();
    }

    fn lookup(&self) -> Lookup<'_> {
        Lookup {
            conn: &self.conn,
            atoms: &self.atoms,
        }
    }

    /// The next event, or `None` once `deadline` passes.
    fn next_event(&self, deadline: Option<Instant>) -> Result<Option<Event>, String> {
        loop {
            if let Some(event) = self
                .conn
                .poll_for_event()
                .map_err(|error| error.to_string())?
            {
                return Ok(Some(event));
            }
            let timeout = match deadline {
                Some(deadline) => {
                    let left = deadline.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return Ok(None);
                    }
                    left.as_millis().min(i32::MAX as u128) as i32
                }
                None => -1,
            };
            let mut fd = libc::pollfd {
                fd: self.conn.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd for the connection's open socket.
            if unsafe { libc::poll(&mut fd, 1, timeout) } < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(error.to_string());
                }
            }
        }
    }

    fn send(&self, sends: Sends) -> Result<(), String> {
        for send in sends {
            let (target, kind, data) = match send {
                Send::Enter(target) => (
                    target,
                    self.atoms.XdndEnter,
                    enter(self.source, target.version, self.atoms.TEXT_URI_LIST),
                ),
                Send::Position { target, x, y, time } => (
                    target,
                    self.atoms.XdndPosition,
                    position(self.source, x, y, time, self.atoms.XdndActionCopy),
                ),
                Send::Leave(target) => (target, self.atoms.XdndLeave, leave(self.source)),
                Send::Drop { target, time } => {
                    (target, self.atoms.XdndDrop, drop(self.source, time))
                }
            };
            let message = ClientMessageEvent::new(32, target.window, kind, data);
            self.conn
                .send_event(false, target.send_to, EventMask::NO_EVENT, message)
                .map_err(|error| error.to_string())?;
        }
        self.conn.flush().map_err(|error| error.to_string())
    }

    /// Hand the files to a target converting `XdndSelection`.
    fn answer(&self, request: &SelectionRequestEvent) -> Result<(), String> {
        let error = |error: &dyn std::fmt::Display| error.to_string();
        // Obsolete clients leave the property unset and mean the target.
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let answered = if request.target == self.atoms.TARGETS {
            self.conn
                .change_property32(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    AtomEnum::ATOM,
                    &[self.atoms.TARGETS, self.atoms.TEXT_URI_LIST],
                )
                .map_err(|e| error(&e))?;
            true
        } else if request.target == self.atoms.TEXT_URI_LIST {
            self.conn
                .change_property8(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    self.atoms.TEXT_URI_LIST,
                    &self.uris,
                )
                .map_err(|e| error(&e))?;
            true
        } else {
            false
        };
        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: request.time,
            requestor: request.requestor,
            selection: request.selection,
            target: request.target,
            property: if answered { property } else { NONE },
        };
        self.conn
            .send_event(false, request.requestor, EventMask::NO_EVENT, notify)
            .map_err(|e| error(&e))?;
        self.conn.flush().map_err(|e| error(&e))?;
        Ok(())
    }
}

/// The window tree under the pointer, as XDND target lookup reads it.
trait Tree {
    /// The child of `window` the pointer is in.
    fn child_at_pointer(&self, window: Window) -> Option<Window>;
    /// The XDND version `window` advertises in `XdndAware`.
    fn aware(&self, window: Window) -> Option<u32>;
    /// The window `window`'s `XdndProxy` names.
    fn proxy(&self, window: Window) -> Option<Window>;
}

/// The live window tree, on our connection.
struct Lookup<'a> {
    conn: &'a RustConnection,
    atoms: &'a Atoms,
}

impl Tree for Lookup<'_> {
    fn child_at_pointer(&self, window: Window) -> Option<Window> {
        let reply = self.conn.query_pointer(window).ok()?.reply().ok()?;
        (reply.child != NONE).then_some(reply.child)
    }

    fn aware(&self, window: Window) -> Option<u32> {
        self.property(window, self.atoms.XdndAware, AtomEnum::ATOM)
    }

    fn proxy(&self, window: Window) -> Option<Window> {
        self.property(window, self.atoms.XdndProxy, AtomEnum::WINDOW)
    }
}

impl Lookup<'_> {
    fn property(&self, window: Window, name: u32, kind: AtomEnum) -> Option<u32> {
        let reply = self
            .conn
            .get_property(false, window, name, kind, 0, 1)
            .ok()?
            .reply()
            .ok()?;
        reply.value32()?.next()
    }
}

/// The drag image's window, made but not yet shown.
struct ImageWindow {
    window: Window,
    colormap: u32,
    hotspot: (i16, i16),
}

impl ImageWindow {
    /// A window showing `image`, or `None` where it would get in the way:
    /// without SHAPE 1.1 its input region cannot be emptied, and it would
    /// hide every target under it.
    fn create(
        conn: &RustConnection,
        screen: usize,
        atoms: &Atoms,
        image: &DragImage,
    ) -> Result<Option<Self>, String> {
        let error = |error: &dyn std::fmt::Display| error.to_string();
        if conn
            .extension_information(shape::X11_EXTENSION_NAME)
            .map_err(|e| error(&e))?
            .is_none()
        {
            return Ok(None);
        }
        let version = conn
            .shape_query_version()
            .map_err(|e| error(&e))?
            .reply()
            .map_err(|e| error(&e))?;
        if (version.major_version, version.minor_version) < (1, 1) {
            return Ok(None);
        }
        let setup = conn.setup();
        let info = &setup.roots[screen];
        let Some((depth, visual, pixels)) = pixels_for(conn, screen, info, image)? else {
            return Ok(None);
        };
        // ZPixmap rows of whole 32-bit pixels, low byte first, as converted.
        let packs = setup
            .pixmap_formats
            .iter()
            .any(|format| format.depth == depth && format.bits_per_pixel == 32);
        if setup.image_byte_order != ImageOrder::LSB_FIRST || !packs {
            return Ok(None);
        }
        let (width, height) = (image.width() as u16, image.height() as u16);
        let colormap = conn.generate_id().map_err(|e| error(&e))?;
        conn.create_colormap(ColormapAlloc::NONE, colormap, info.root, visual)
            .map_err(|e| error(&e))?;
        let window = conn.generate_id().map_err(|e| error(&e))?;
        conn.create_window(
            depth,
            window,
            info.root,
            0,
            0,
            width,
            height,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .override_redirect(1)
                .save_under(1)
                .colormap(colormap),
        )
        .map_err(|e| error(&e))?;
        // The pixels as the window's background, which the server repaints
        // on its own as the window moves.
        let pixmap = conn.generate_id().map_err(|e| error(&e))?;
        conn.create_pixmap(depth, pixmap, window, width, height)
            .map_err(|e| error(&e))?;
        let gc = conn.generate_id().map_err(|e| error(&e))?;
        conn.create_gc(gc, pixmap, &CreateGCAux::new())
            .map_err(|e| error(&e))?;
        let row = usize::from(width) * 4;
        // In strips that fit a request, past its 24 byte header.
        let rows = ((conn.maximum_request_bytes() - 24) / row).max(1);
        for (strip, chunk) in pixels.chunks(rows * row).enumerate() {
            conn.put_image(
                ImageFormat::Z_PIXMAP,
                pixmap,
                gc,
                width,
                (chunk.len() / row) as u16,
                0,
                (strip * rows) as i16,
                0,
                depth,
                chunk,
            )
            .map_err(|e| error(&e))?;
        }
        conn.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().background_pixmap(pixmap),
        )
        .map_err(|e| error(&e))?;
        let _ = conn.free_gc(gc);
        let _ = conn.free_pixmap(pixmap);
        conn.shape_rectangles(
            shape::SO::SET,
            shape::SK::INPUT,
            ClipOrdering::UNSORTED,
            window,
            0,
            0,
            &[],
        )
        .map_err(|e| error(&e))?;
        conn.change_property32(
            PropMode::REPLACE,
            window,
            atoms._NET_WM_WINDOW_TYPE,
            AtomEnum::ATOM,
            &[atoms._NET_WM_WINDOW_TYPE_DND],
        )
        .map_err(|e| error(&e))?;
        let (hx, hy) = image.hotspot();
        Ok(Some(Self {
            window,
            colormap,
            hotspot: (hx as i16, hy as i16),
        }))
    }

    /// Put the hotspot at (`x`, `y`) on the root, above everything.
    fn move_to(&self, conn: &RustConnection, x: i16, y: i16) {
        let _ = conn.configure_window(
            self.window,
            &ConfigureWindowAux::new()
                .x(i32::from(x) - i32::from(self.hotspot.0))
                .y(i32::from(y) - i32::from(self.hotspot.1))
                .stack_mode(StackMode::ABOVE),
        );
    }

    fn show(&self, conn: &RustConnection, x: i16, y: i16) {
        self.move_to(conn, x, y);
        let _ = conn.map_window(self.window);
        let _ = conn.flush();
    }

    fn destroy(&self, conn: &RustConnection) {
        let _ = conn.destroy_window(self.window);
        let _ = conn.free_colormap(self.colormap);
    }
}

/// The depth, visual, and pixels to show `image` with: premultiplied on a
/// 32-bit visual when a compositor will blend it, else flattened onto
/// `BACKGROUND` on the root's visual. `None` if neither visual is the
/// 8-bit-a-channel TrueColor the pixels are converted for.
fn pixels_for(
    conn: &RustConnection,
    screen: usize,
    info: &Screen,
    image: &DragImage,
) -> Result<Option<(u8, u32, Vec<u8>)>, String> {
    let error = |error: &dyn std::fmt::Display| error.to_string();
    let true_color = |depth: u8, id: Option<u32>| {
        info.allowed_depths
            .iter()
            .filter(|d| d.depth == depth)
            .flat_map(|d| &d.visuals)
            .find(|v| {
                id.is_none_or(|id| v.visual_id == id)
                    && v.class == VisualClass::TRUE_COLOR
                    && (v.red_mask, v.green_mask, v.blue_mask) == (0xff_0000, 0xff00, 0xff)
            })
            .map(|v| v.visual_id)
    };
    let manager = conn
        .intern_atom(false, format!("_NET_WM_CM_S{screen}").as_bytes())
        .map_err(|e| error(&e))?
        .reply()
        .map_err(|e| error(&e))?
        .atom;
    let composited = conn
        .get_selection_owner(manager)
        .map_err(|e| error(&e))?
        .reply()
        .map_err(|e| error(&e))?
        .owner
        != NONE;
    if composited && let Some(visual) = true_color(32, None) {
        return Ok(Some((32, visual, image.premultiplied_bgra())));
    }
    Ok(true_color(info.root_depth, Some(info.root_visual))
        .map(|visual| (info.root_depth, visual, image.opaque_bgrx(BACKGROUND))))
}

/// Where XDND messages for the window under the pointer go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Target {
    /// The window the drop lands on, named in every message.
    window: Window,
    /// Where the messages are sent: `window`, or the proxy it names.
    send_to: Window,
    /// The version both sides speak.
    version: u32,
}

/// The XDND version to speak with a target advertising `theirs`. Versions
/// before 3 differ in message layout and are long gone.
fn negotiate(theirs: u32) -> Option<u32> {
    (theirs >= 3).then_some(theirs.min(VERSION))
}

/// The innermost XDND aware window under the pointer, walking down from
/// `root`. A window whose `XdndProxy` names a window that names itself has
/// its messages sent there instead.
fn find_target(tree: &impl Tree, root: Window) -> Option<Target> {
    let mut window = root;
    loop {
        if let Some(proxy) = tree.proxy(window)
            && tree.proxy(proxy) == Some(proxy)
        {
            let version = negotiate(tree.aware(proxy)?)?;
            return Some(Target {
                window,
                send_to: proxy,
                version,
            });
        }
        if let Some(theirs) = tree.aware(window) {
            return Some(Target {
                window,
                send_to: window,
                version: negotiate(theirs)?,
            });
        }
        window = tree.child_at_pointer(window)?;
    }
}

fn enter(source: Window, version: u32, offered: u32) -> [u32; 5] {
    [source, version << 24, offered, NONE, NONE]
}

fn position(source: Window, x: i16, y: i16, time: u32, action: u32) -> [u32; 5] {
    [
        source,
        0,
        ((x as u16 as u32) << 16) | y as u16 as u32,
        time,
        action,
    ]
}

fn leave(source: Window) -> [u32; 5] {
    [source, 0, 0, 0, 0]
}

fn drop(source: Window, time: u32) -> [u32; 5] {
    [source, 0, time, 0, 0]
}

/// A message to a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Send {
    Enter(Target),
    Position {
        target: Target,
        x: i16,
        y: i16,
        time: u32,
    },
    Leave(Target),
    Drop {
        target: Target,
        time: u32,
    },
}

/// What one event sends: at most a Leave, an Enter, and a Position. Kept
/// inline rather than in a `Vec`, which also made the Kani proof below run
/// out of memory.
#[derive(Debug, Default)]
struct Sends {
    items: [Option<Send>; 3],
    len: usize,
}

impl Sends {
    fn push(&mut self, send: Send) {
        self.items[self.len] = Some(send);
        self.len += 1;
    }
}

impl Extend<Send> for Sends {
    fn extend<I: IntoIterator<Item = Send>>(&mut self, sends: I) {
        for send in sends {
            self.push(send);
        }
    }
}

impl FromIterator<Send> for Sends {
    fn from_iter<I: IntoIterator<Item = Send>>(sends: I) -> Self {
        let mut out = Self::default();
        out.extend(sends);
        out
    }
}

impl IntoIterator for Sends {
    type Item = Send;
    type IntoIter = std::iter::Flatten<std::array::IntoIter<Option<Send>, 3>>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter().flatten()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The button is held.
    Dragging,
    /// Released while the target had a position unanswered.
    Releasing,
    /// Dropped on the window; it fetches the files, then finishes.
    Dropped(Window),
    Over,
}

/// The source's half of XDND, apart from the X connection: which target
/// the pointer is over, and what to tell it. One position is in flight at a
/// time; moves meanwhile collapse into the latest.
#[derive(Debug)]
struct Drag {
    phase: Phase,
    target: Option<Target>,
    /// Sent a position, no status back yet.
    waiting: bool,
    /// The latest move made while waiting.
    queued: Option<(i16, i16, u32)>,
    /// The target's last status accepted a drop.
    accepted: bool,
    release_time: u32,
}

impl Default for Drag {
    fn default() -> Self {
        Self {
            phase: Phase::Dragging,
            target: None,
            waiting: false,
            queued: None,
            accepted: false,
            release_time: 0,
        }
    }
}

impl Drag {
    fn phase(&self) -> Phase {
        self.phase
    }

    fn moved(&mut self, target: Option<Target>, x: i16, y: i16, time: u32) -> Sends {
        if self.phase != Phase::Dragging {
            return Sends::default();
        }
        let mut sends = Sends::default();
        if target.map(|t| t.window) != self.target.map(|t| t.window) {
            sends.extend(self.target.take().map(Send::Leave));
            self.target = target;
            self.waiting = false;
            self.queued = None;
            self.accepted = false;
            sends.extend(target.map(Send::Enter));
        }
        if let Some(target) = self.target {
            if self.waiting {
                self.queued = Some((x, y, time));
            } else {
                self.waiting = true;
                sends.push(Send::Position { target, x, y, time });
            }
        }
        sends
    }

    fn status(&mut self, from: Window, accept: bool) -> Sends {
        let Some(target) = self.target else {
            return Sends::default();
        };
        if from != target.window || !self.waiting {
            return Sends::default();
        }
        self.waiting = false;
        self.accepted = accept;
        match self.phase {
            Phase::Releasing => self.drop_or_leave(),
            _ => match self.queued.take() {
                Some((x, y, time)) => {
                    self.waiting = true;
                    [Send::Position { target, x, y, time }]
                        .into_iter()
                        .collect()
                }
                None => Sends::default(),
            },
        }
    }

    fn released(&mut self, time: u32) -> Sends {
        if self.phase != Phase::Dragging {
            return Sends::default();
        }
        self.release_time = time;
        if self.waiting {
            // The answer to the last position decides.
            self.phase = Phase::Releasing;
            return Sends::default();
        }
        self.drop_or_leave()
    }

    fn drop_or_leave(&mut self) -> Sends {
        match self.target.take() {
            Some(target) if self.accepted => {
                self.phase = Phase::Dropped(target.window);
                [Send::Drop {
                    target,
                    time: self.release_time,
                }]
                .into_iter()
                .collect()
            }
            target => {
                self.phase = Phase::Over;
                target.map(Send::Leave).into_iter().collect()
            }
        }
    }

    fn finished(&mut self, from: Window) {
        if self.phase == Phase::Dropped(from) {
            self.phase = Phase::Over;
        }
    }

    /// Escape, or a target that stopped answering.
    fn cancel(&mut self) -> Sends {
        let sends = match self.phase {
            Phase::Dragging | Phase::Releasing => self.target.take().map(Send::Leave),
            Phase::Dropped(_) | Phase::Over => None,
        };
        self.phase = Phase::Over;
        sends.into_iter().collect()
    }
}

/// The cursor font's hand, shown while dragging.
fn hand_cursor(conn: &RustConnection) -> Option<u32> {
    const XC_HAND2: u16 = 60;
    let font = conn.generate_id().ok()?;
    conn.open_font(font, b"cursor").ok()?;
    let cursor = conn.generate_id().ok()?;
    conn.create_glyph_cursor(
        cursor,
        font,
        font,
        XC_HAND2,
        XC_HAND2 + 1,
        0,
        0,
        0,
        0xffff,
        0xffff,
        0xffff,
    )
    .ok()?;
    conn.close_font(font).ok()?;
    Some(cursor)
}

fn escape_keycode(conn: &RustConnection) -> Option<u8> {
    let setup = conn.setup();
    let (min, max) = (setup.min_keycode, setup.max_keycode);
    let mapping = conn
        .get_keyboard_mapping(min, max - min + 1)
        .ok()?
        .reply()
        .ok()?;
    let per = usize::from(mapping.keysyms_per_keycode).max(1);
    let index = mapping.keysyms.iter().position(|&sym| sym == ESCAPE)?;
    u8::try_from(index / per).ok().map(|offset| min + offset)
}

#[cfg(kani)]
mod verification {
    use super::*;

    /// What a target has been told, checked against XDND's rules as each
    /// message goes out.
    #[derive(Default)]
    struct Monitor {
        /// The target between our Enter and our Leave or Drop.
        entered: Option<Window>,
        /// A Position the entered target has not answered.
        in_flight: bool,
        /// The entered target's latest answer accepted a drop.
        accepted: bool,
        /// Nothing may follow a Drop.
        dropped: bool,
    }

    impl Monitor {
        fn sent(&mut self, send: Send, release_time: u32) {
            assert!(!self.dropped);
            match send {
                Send::Enter(t) => {
                    assert!(self.entered.is_none());
                    *self = Self {
                        entered: Some(t.window),
                        ..Self::default()
                    };
                }
                Send::Position { target, .. } => {
                    assert!(self.entered == Some(target.window));
                    // One position at a time.
                    assert!(!self.in_flight);
                    self.in_flight = true;
                }
                Send::Leave(t) => {
                    assert!(self.entered == Some(t.window));
                    self.entered = None;
                }
                Send::Drop { target, time } => {
                    assert!(self.entered == Some(target.window));
                    // Only on an answered position that accepted.
                    assert!(!self.in_flight && self.accepted);
                    assert!(time == release_time);
                    self.entered = None;
                    self.dropped = true;
                }
            }
        }

        fn answered(&mut self, from: Window, accept: bool) {
            if self.entered == Some(from) && self.in_flight {
                self.in_flight = false;
                self.accepted = accept;
            }
        }
    }

    fn any_window() -> Window {
        if kani::any() { 5 } else { 6 }
    }

    /// One arbitrary event: a pointer move, a target's answer (from either
    /// window, due or not), a release, escape, or a finish.
    fn any_event(drag: &mut Drag, monitor: &mut Monitor, release_time: &mut Option<u32>) {
        let window = any_window();
        let sends = match kani::any::<u8>() % 5 {
            0 => {
                let target = kani::any::<bool>().then_some(Target {
                    window,
                    send_to: window,
                    version: VERSION,
                });
                drag.moved(target, kani::any(), kani::any(), kani::any())
            }
            1 => {
                let accept = kani::any();
                monitor.answered(window, accept);
                drag.status(window, accept)
            }
            2 => {
                let time = kani::any();
                if drag.phase() == Phase::Dragging {
                    *release_time = Some(time);
                }
                drag.released(time)
            }
            3 => drag.cancel(),
            _ => {
                drag.finished(window);
                Sends::default()
            }
        };
        for send in sends {
            monitor.sent(send, release_time.unwrap_or(0));
        }
        match drag.phase() {
            Phase::Dragging => {}
            Phase::Releasing => assert!(monitor.in_flight),
            Phase::Dropped(_) | Phase::Over => assert!(monitor.entered.is_none()),
        }
    }

    /// Any five events in any order send only messages XDND allows, and an
    /// ended drag leaves no target entered.
    #[kani::proof]
    // Calls send at most three messages; the steps are unrolled by hand so
    // this bound stays small.
    #[kani::unwind(4)]
    fn drag_messages_follow_xdnd_for_any_event_order() {
        let mut drag = Drag::default();
        let mut monitor = Monitor::default();
        let mut release_time = None;
        any_event(&mut drag, &mut monitor, &mut release_time);
        any_event(&mut drag, &mut monitor, &mut release_time);
        any_event(&mut drag, &mut monitor, &mut release_time);
        any_event(&mut drag, &mut monitor, &mut release_time);
        any_event(&mut drag, &mut monitor, &mut release_time);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    /// A window tree: each window's child under the pointer and its
    /// `XdndAware` and `XdndProxy` properties.
    #[derive(Default)]
    struct Windows {
        child: HashMap<Window, Window>,
        aware: HashMap<Window, u32>,
        proxy: HashMap<Window, Window>,
    }

    impl Tree for Windows {
        fn child_at_pointer(&self, window: Window) -> Option<Window> {
            self.child.get(&window).copied()
        }
        fn aware(&self, window: Window) -> Option<u32> {
            self.aware.get(&window).copied()
        }
        fn proxy(&self, window: Window) -> Option<Window> {
            self.proxy.get(&window).copied()
        }
    }

    const ROOT: Window = 1;

    #[test]
    fn target_lookup_finds_the_aware_window_under_the_pointer_and_honors_proxies() {
        let target = |window, send_to, version| {
            Some(Target {
                window,
                send_to,
                version,
            })
        };
        // Root, then a window manager frame (10), then the client (11).
        let cases: [(&str, Windows, Option<Target>); 6] = [
            (
                "aware client inside a frame",
                Windows {
                    child: [(ROOT, 10), (10, 11)].into(),
                    aware: [(11, 5)].into(),
                    ..Default::default()
                },
                target(11, 11, 5),
            ),
            (
                "newer target speaks our version",
                Windows {
                    child: [(ROOT, 10)].into(),
                    aware: [(10, 9)].into(),
                    ..Default::default()
                },
                target(10, 10, 5),
            ),
            (
                "too old to talk to",
                Windows {
                    child: [(ROOT, 10), (10, 11)].into(),
                    aware: [(10, 2), (11, 5)].into(),
                    ..Default::default()
                },
                None,
            ),
            (
                "nothing aware",
                Windows {
                    child: [(ROOT, 10), (10, 11)].into(),
                    ..Default::default()
                },
                None,
            ),
            (
                "a desktop proxies the root",
                Windows {
                    child: [(ROOT, 10)].into(),
                    aware: [(20, 4)].into(),
                    proxy: [(ROOT, 20), (20, 20)].into(),
                },
                target(ROOT, 20, 4),
            ),
            (
                "a proxy that does not name itself is stale",
                Windows {
                    child: [(ROOT, 10)].into(),
                    aware: [(10, 5), (20, 5)].into(),
                    proxy: [(ROOT, 20)].into(),
                },
                target(10, 10, 5),
            ),
        ];
        for (name, windows, expected) in cases {
            assert_eq!(find_target(&windows, ROOT), expected, "{name}");
        }
    }

    /// A private Xvfb server, killed on drop; `None` where Xvfb is not
    /// installed, which skips the tests that need a real X server.
    struct Xvfb {
        child: std::process::Child,
        display: String,
    }

    impl Xvfb {
        fn start() -> Option<Self> {
            use std::io::BufRead;
            use std::os::fd::{FromRawFd, OwnedFd};
            let mut fds = [0; 2];
            // SAFETY: a plain pipe into a two-element array; only the write
            // end is left inheritable, for Xvfb to report its display on.
            let (read, write) = unsafe {
                assert_eq!(libc::pipe(fds.as_mut_ptr()), 0);
                libc::fcntl(fds[0], libc::F_SETFD, libc::FD_CLOEXEC);
                (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1]))
            };
            let child = std::process::Command::new("Xvfb")
                .args(["-displayfd", &fds[1].to_string(), "-nolisten", "tcp"])
                .args(["-screen", "0", "320x240x24"])
                .stdin(std::process::Stdio::null())
                // A system Xvfb, not linked against a dev shell's libraries.
                .env_remove("LD_LIBRARY_PATH")
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
            std::mem::drop(write);
            let Ok(child) = child else {
                eprintln!("skipped: no Xvfb");
                return None;
            };
            let mut line = String::new();
            std::io::BufReader::new(std::fs::File::from(read))
                .read_line(&mut line)
                .unwrap();
            assert!(!line.trim().is_empty(), "Xvfb did not start");
            Some(Self {
                child,
                display: format!(":{}", line.trim()),
            })
        }
    }

    impl Drop for Xvfb {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    /// On a fresh server, an XDND aware window at the top left and the
    /// drag image of `rgba` (8 by 8 pixels, hotspot at its corner) shown
    /// with the pointer at (50, 50).
    fn drag_image_over_a_target(
        server: &Xvfb,
        rgba: [u8; 4],
    ) -> (RustConnection, Atoms, Window, Window) {
        let (conn, screen) = x11rb::connect(Some(&server.display)).unwrap();
        let root = conn.setup().roots[screen].root;
        let atoms = Atoms::new(&conn).unwrap().reply().unwrap();
        let target = conn.generate_id().unwrap();
        conn.create_window(
            0,
            target,
            root,
            0,
            0,
            200,
            200,
            0,
            WindowClass::INPUT_OUTPUT,
            0,
            &CreateWindowAux::new().background_pixel(0),
        )
        .unwrap();
        conn.change_property32(
            PropMode::REPLACE,
            target,
            atoms.XdndAware,
            AtomEnum::ATOM,
            &[5],
        )
        .unwrap();
        conn.map_window(target).unwrap();
        conn.warp_pointer(NONE, root, 0, 0, 0, 0, 50, 50).unwrap();
        let image = DragImage::from_rgba(8, 8, rgba.repeat(64))
            .and_then(|image| image.with_hotspot(0, 0))
            .unwrap();
        let window = ImageWindow::create(&conn, screen, &atoms, &image)
            .unwrap()
            .expect("Xvfb has SHAPE and a TrueColor root");
        window.show(&conn, 50, 50);
        conn.get_input_focus().unwrap().reply().unwrap();
        (conn, atoms, root, target)
    }

    #[test]
    fn target_lookup_sees_through_the_drag_image_under_the_pointer() {
        let Some(server) = Xvfb::start() else { return };
        let (conn, atoms, root, target) = drag_image_over_a_target(&server, [0, 0, 255, 255]);
        let lookup = Lookup {
            conn: &conn,
            atoms: &atoms,
        };
        assert_eq!(find_target(&lookup, root).map(|t| t.window), Some(target));
    }

    #[test]
    fn without_a_compositor_the_drag_image_is_flattened_onto_light_gray() {
        let Some(server) = Xvfb::start() else { return };
        // Half transparent red.
        let (conn, _, root, _) = drag_image_over_a_target(&server, [255, 0, 0, 128]);
        let shown = conn
            .get_image(ImageFormat::Z_PIXMAP, root, 52, 52, 1, 1, !0)
            .unwrap()
            .reply()
            .unwrap();
        // BGRX.
        assert_eq!(shown.data[..3], [120, 120, 248]);
    }

    #[test]
    fn client_messages_pack_version_and_position_as_xdnd_specifies() {
        assert_eq!(enter(7, 5, 300), [7, 0x0500_0000, 300, 0, 0]);
        assert_eq!(position(7, 640, 480, 99, 301), [7, 0, 0x0280_01e0, 99, 301]);
        // Negative coordinates (left of a monitor at the origin) keep their
        // 16 bits rather than spilling into the other half.
        assert_eq!(position(7, -1, 2, 0, 0)[2], 0xffff_0002);
        assert_eq!(drop(7, 1234), [7, 0, 1234, 0, 0]);
    }

    /// Run `script` through a drag, one line per message it sends.
    fn run(script: &[Step]) -> String {
        let target = |window| Target {
            window,
            send_to: window,
            version: 5,
        };
        let mut drag = Drag::default();
        let mut log = String::new();
        for step in script {
            let sends = match *step {
                Step::Move(window, x) => drag.moved(window.map(target), x, 0, 0),
                Step::Status(window, accept) => drag.status(window, accept),
                Step::Release => drag.released(77),
                Step::Escape => drag.cancel(),
                Step::Finished(window) => {
                    drag.finished(window);
                    Sends::default()
                }
            };
            for send in sends {
                log += &match send {
                    Send::Enter(t) => format!("enter {}\n", t.window),
                    Send::Position { target, x, .. } => {
                        format!("position {} x={x}\n", target.window)
                    }
                    Send::Leave(t) => format!("leave {}\n", t.window),
                    Send::Drop { target, time } => format!("drop {} t={time}\n", target.window),
                };
            }
        }
        log + &format!("{:?}", drag.phase())
    }

    #[derive(Clone, Copy)]
    enum Step {
        Move(Option<Window>, i16),
        Status(Window, bool),
        Release,
        Escape,
        Finished(Window),
    }

    #[test]
    fn drag_talks_to_targets_one_position_at_a_time_and_drops_only_when_accepted() {
        use Step::*;
        let cases: [(&str, &[Step], &str); 6] = [
            (
                "moving between targets leaves one before entering the next",
                &[
                    Move(Some(5), 1),
                    Status(5, false),
                    Move(Some(6), 2),
                    Move(None, 3),
                ],
                "enter 5\nposition 5 x=1\nleave 5\nenter 6\nposition 6 x=2\nleave 6\nDragging",
            ),
            (
                "moves while a status is due collapse into the latest",
                &[
                    Move(Some(5), 1),
                    Move(Some(5), 2),
                    Move(Some(5), 3),
                    Status(5, true),
                ],
                "enter 5\nposition 5 x=1\nposition 5 x=3\nDragging",
            ),
            (
                "an accepted drop waits for the target to finish",
                &[Move(Some(5), 1), Status(5, true), Release, Finished(5)],
                "enter 5\nposition 5 x=1\ndrop 5 t=77\nOver",
            ),
            (
                "a release with a status due waits for it, then drops",
                &[Move(Some(5), 1), Release, Status(5, true)],
                "enter 5\nposition 5 x=1\ndrop 5 t=77\nDropped(5)",
            ),
            (
                "a refused drop leaves",
                &[Move(Some(5), 1), Release, Status(5, false)],
                "enter 5\nposition 5 x=1\nleave 5\nOver",
            ),
            (
                "escape leaves the target",
                &[Move(Some(5), 1), Status(5, true), Escape, Release],
                "enter 5\nposition 5 x=1\nleave 5\nOver",
            ),
        ];
        for (name, script, expected) in cases {
            assert_eq!(run(script), expected, "{name}");
        }
    }
}
