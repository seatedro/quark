//! Dragging out on Wayland, over winit's `wl_display`.
//!
//! winit keeps its seats and pointers private, so this binds its own on a
//! separate event queue of the same connection (`Backend::from_foreign_display`
//! over the system libwayland winit also uses). The compositor sends pointer
//! events to every `wl_pointer` a client owns, so our pointers see the
//! button press serial that `wl_data_device.start_drag` needs to prove the
//! implicit grab. That is why the pointers are bound when the first window
//! opens rather than when a drag starts: by then the press has already
//! happened. Every seat is bound, as seats come and go, and [`Seats`] picks
//! the one whose press the drag continues.
//!
//! Our queue is read and dispatched by a thread of its own. winit's loop
//! cannot do it: it reads our events off the socket, but sleeps on without
//! an iteration when none were for its own queue. libwayland lets several
//! threads read one connection (`wl_display_prepare_read_queue`). The
//! thread owns all the protocol state; the UI thread sends it requests over
//! a channel and wakes it through a pipe. A drag request waits for the
//! thread's answer, and the thread first makes a round trip, so every
//! pointer event the compositor sent before the request (the press winit
//! just delivered, or its release) is in our state when the seat is chosen.
//! The round trip needs only the thread itself to read, and the UI thread
//! is not inside a read while it waits, so neither queue blocks on the
//! other.
//!
//! Nothing on the thread blocks but its poll, which the wake pipe ends, so
//! a stop request always gets through: the round trip is a `wl_display.sync`
//! whose `done` the loop dispatches like any other event, the dropped files
//! are written to each drop target as its pipe takes them, and requests the
//! socket would not take yet are flushed once it is writable. A drag
//! request touches the window's surface only under a [`ticket`] claim,
//! while the UI still holds the window.
//!
//! Dock drags ([`crate::platform::dock_drag`]) start the same way, with a
//! source offering one MIME type made for that drag alone: the session
//! token. The seat's data device then sees the drag pass over our own
//! windows, and an offer is accepted, and its enter, motion, leave, and
//! drop forwarded to the UI thread, only if it carries the current token
//! and is over one of our windows. Where the compositor has
//! `xdg_toplevel_drag_v1`, the source gets one, and the UI attaches the
//! torn-off window to it itself, so the request reaches the compositor
//! before that window's first frame.

// Dock drags are started only by `dock_drag`, which needs the `ui` feature.
#![cfg_attr(not(feature = "ui"), allow(dead_code))]

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::ptr::NonNull;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use raw_window_handle::{RawWindowHandle, WindowHandle};
use wayland_client::backend::{Backend, ObjectId, WaylandError};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_callback::{self, WlCallback};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child,
};
use wayland_protocols::xdg::shell::client::xdg_toplevel::XdgToplevel;
use wayland_protocols::xdg::toplevel_drag::v1::client::xdg_toplevel_drag_manager_v1::XdgToplevelDragManagerV1;
use wayland_protocols::xdg::toplevel_drag::v1::client::xdg_toplevel_drag_v1::XdgToplevelDragV1;

use super::seat::{Refusal, Seats, Selected, Surface};
use super::ticket::{self, Ticket};
use super::{DragImage, DragOutError};
use crate::runner::Waker;

const URI_LIST: &str = "text/uri-list";

/// How long a drag request waits for the thread to take it on: a round
/// trip with a live compositor takes well under this.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(2);

thread_local! {
    static WAYLAND: RefCell<Option<Wayland>> = const { RefCell::new(None) };
}

/// The UI thread's end.
struct Wayland {
    requests: mpsc::Sender<Request>,
    /// Written to wake the thread for a request.
    wake: OwnedFd,
    thread: JoinHandle<State>,
}

enum Request {
    Window {
        surface: Surface,
        open: bool,
    },
    Start(Start),
    /// Give up the dock drag with this token, if it still runs.
    DockEnd(u64),
    Stop,
}

struct Start {
    drag: Drag,
    ticket: Answerer,
}

/// Where a start's answer goes: what each kind of drag hands back.
enum Answerer {
    Files(Ticket<()>),
    Dock(Ticket<DockStarted>),
}

impl Answerer {
    fn abandoned(&self) -> bool {
        match self {
            Self::Files(ticket) => ticket.abandoned(),
            Self::Dock(ticket) => ticket.abandoned(),
        }
    }
}

/// What a drag starts with.
struct Drag {
    /// The window's `wl_surface`, converted while the window was live. Only
    /// used under a claim on the request's ticket, while the UI keeps the
    /// window open.
    origin: ObjectId,
    /// The same surface as the seats know it, by address: compared, never
    /// dereferenced.
    surface: Surface,
    content: Content,
    seat: Option<String>,
    icon: Option<IconPixels>,
}

/// What a drag offers.
enum Content {
    /// A `text/uri-list` of files.
    Files(Vec<u8>),
    Dock(DockRequest),
}

/// A dock drag, as the UI asks for it.
pub(crate) struct DockRequest {
    /// Tells this drag's events from an earlier one's.
    pub token: u64,
    /// The one MIME type the drag offers, unique to it.
    pub mime: String,
    pub signals: mpsc::Sender<DockSignal>,
    /// Woken after each signal, so the UI reads it.
    pub waker: Waker,
}

/// What the compositor said about a dock drag, forwarded as it came.
/// Positions are in the surface's logical coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum DockSignal {
    /// The drag came over one of our windows, by its `wl_surface` address.
    Enter {
        surface: Surface,
        x: f64,
        y: f64,
    },
    Motion {
        x: f64,
        y: f64,
    },
    /// It left the window it entered.
    Leave,
    /// It was dropped on the window it entered.
    Drop,
    /// The user released the drag, wherever (`dnd_drop_performed`).
    DropPerformed,
    /// It ended without a drop on a window of ours that took it: released
    /// elsewhere, or cancelled.
    Cancelled,
    /// The window it was dropped on is done with it.
    Finished,
}

/// A started dock drag, for the UI thread.
pub(crate) struct DockStarted {
    conn: Connection,
    /// Where the compositor has `xdg_toplevel_drag_v1`.
    toplevel_drag: Option<XdgToplevelDragV1>,
}

impl DockStarted {
    /// Whether a window can be attached to move with the drag.
    pub(crate) fn can_follow(&self) -> bool {
        self.toplevel_drag.is_some()
    }

    /// Move the window whose `xdg_toplevel` this is with the drag, `offset`
    /// (in its logical coordinates) under the pointer. Sent from the UI
    /// thread, which keeps the window open meanwhile, and flushed at once,
    /// so it reaches the compositor before the window is first shown.
    ///
    /// # Safety
    ///
    /// `xdg_toplevel` is the live `xdg_toplevel` proxy of an open window.
    pub(crate) unsafe fn attach(
        &self,
        xdg_toplevel: NonNull<std::ffi::c_void>,
        (x, y): (i32, i32),
    ) -> Result<(), DragOutError> {
        let platform = |error: &dyn std::fmt::Display| DragOutError::Platform(error.to_string());
        let drag = self
            .toplevel_drag
            .as_ref()
            .ok_or(DragOutError::Unsupported)?;
        // SAFETY: the caller's live proxy.
        let id =
            unsafe { ObjectId::from_ptr(XdgToplevel::interface(), xdg_toplevel.as_ptr().cast()) }
                .map_err(|e| platform(&e))?;
        let toplevel = XdgToplevel::from_id(&self.conn, id).map_err(|e| platform(&e))?;
        drag.attach(&toplevel, x, y);
        // A socket that would block takes it on winit's next flush, still
        // before the window's first frame.
        match self.conn.flush() {
            Err(WaylandError::Io(io)) if io.kind() != std::io::ErrorKind::WouldBlock => {
                Err(platform(&io))
            }
            Err(error @ WaylandError::Protocol(_)) => Err(platform(&error)),
            _ => Ok(()),
        }
    }
}

/// The drag image in a shared memory file, ready for a buffer.
struct IconPixels {
    file: OwnedFd,
    width: i32,
    height: i32,
    buffer_scale: i32,
    /// Where the pointer sits in the icon, in surface coordinates.
    hotspot: (i32, i32),
}

/// The thread's protocol state.
struct State {
    conn: Connection,
    registry: WlRegistry,
    manager: Option<WlDataDeviceManager>,
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    toplevel_drags: Option<XdgToplevelDragManagerV1>,
    seats: Seats<SeatObjects>,
    /// The dock drag running, if any.
    dock: Option<ActiveDock>,
    /// Drag requests waiting for their round trip's `done`, by the sync
    /// callback's data.
    starts: BTreeMap<u64, Start>,
    next_start: u64,
    /// Drop targets still reading the dropped files.
    transfers: Vec<Transfer>,
    outgoing: Outgoing,
}

/// The dock drag running on the thread.
struct ActiveDock {
    token: u64,
    mime: String,
    signals: mpsc::Sender<DockSignal>,
    waker: Waker,
    source: WlDataSource,
    /// Destroyed once the drag is released or cancelled, as the protocol
    /// requires.
    toplevel_drag: Option<XdgToplevelDragV1>,
    /// The seat whose data device has the drag over one of our windows.
    entered: Option<u32>,
}

impl ActiveDock {
    fn signal(&self, signal: DockSignal) {
        if self.signals.send(signal).is_ok() {
            self.waker.wake();
        }
    }
}

/// Whether requests are still waiting for the socket to take them.
#[derive(Debug, Default)]
struct Outgoing {
    pending: bool,
}

impl Outgoing {
    fn flush(&mut self, conn: &Connection) -> Result<(), WaylandError> {
        self.flushed(conn.flush())
    }

    /// Note a flush's result. A socket that would block takes the rest
    /// later, so that is no failure: the connection is polled for writing
    /// until a flush gets through.
    fn flushed(&mut self, result: Result<(), WaylandError>) -> Result<(), WaylandError> {
        self.pending = match &result {
            Err(WaylandError::Io(io)) => io.kind() == std::io::ErrorKind::WouldBlock,
            _ => false,
        };
        if self.pending { Ok(()) } else { result }
    }

    /// What to poll the connection for.
    fn poll_events(&self) -> libc::c_short {
        if self.pending {
            libc::POLLIN | libc::POLLOUT
        } else {
            libc::POLLIN
        }
    }
}

/// One seat's objects on our queue.
struct SeatObjects {
    seat: WlSeat,
    pointer: Option<WlPointer>,
    device: Option<WlDataDevice>,
    /// Offers the compositor hands the seat's data device (the clipboard,
    /// drags passing over our windows). Unused, but each must be destroyed
    /// once replaced.
    selection_offer: Option<WlDataOffer>,
    drag_offer: Option<WlDataOffer>,
}

impl SeatObjects {
    fn release(self) {
        if let Some(pointer) = &self.pointer
            && pointer.version() >= 3
        {
            pointer.release();
        }
        if let Some(device) = &self.device
            && device.version() >= 2
        {
            device.release();
        }
        for offer in [self.selection_offer, self.drag_offer]
            .into_iter()
            .flatten()
        {
            offer.destroy();
        }
        if self.seat.version() >= 5 {
            self.seat.release();
        }
    }
}

pub(super) fn init(display: *mut std::ffi::c_void) {
    WAYLAND.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        match connect(display) {
            Ok(wayland) => *slot.borrow_mut() = Some(wayland),
            Err(error) => tracing::warn!("drag out unavailable on Wayland: {error}"),
        }
    });
}

fn connect(display: *mut std::ffi::c_void) -> Result<Wayland, String> {
    // SAFETY: winit's display handle is its live wl_display, which stays
    // connected until the event loop exits, and `shutdown` stops all use of
    // it before then.
    let backend = unsafe { Backend::from_foreign_display(display.cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, mut queue) =
        registry_queue_init::<State>(&conn).map_err(|error| error.to_string())?;
    let qh = queue.handle();
    let mut state = State {
        conn: conn.clone(),
        registry: globals.registry().clone(),
        manager: None,
        compositor: None,
        shm: None,
        toplevel_drags: None,
        seats: Seats::default(),
        dock: None,
        starts: BTreeMap::new(),
        next_start: 0,
        transfers: Vec::new(),
        outgoing: Outgoing::default(),
    };
    for global in globals.contents().clone_list() {
        state.global(&qh, global.name, &global.interface, global.version);
    }
    if state.manager.is_none() {
        return Err("no wl_data_device_manager".into());
    }
    // The seats' capabilities and names, and with them our pointers.
    queue
        .roundtrip(&mut state)
        .map_err(|error| error.to_string())?;
    let mut fds = [0; 2];
    // SAFETY: a plain pipe2 into a two-element array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: both ends are fresh descriptors this function now owns.
    let (woken, wake) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let (requests, received) = mpsc::channel();
    let thread = std::thread::Builder::new()
        .name("quark-drag-out".into())
        .spawn(move || run(queue, state, woken, received))
        .map_err(|error| error.to_string())?;
    Ok(Wayland {
        requests,
        wake,
        thread,
    })
}

impl Wayland {
    fn send(&self, request: Request) -> Result<(), DragOutError> {
        self.requests
            .send(request)
            .map_err(|_| DragOutError::Platform("the Wayland drag thread stopped".into()))?;
        // SAFETY: a one-byte write to the pipe's open write end. A full pipe
        // already holds a wake.
        unsafe { libc::write(self.wake.as_raw_fd(), [0u8].as_ptr().cast(), 1) };
        Ok(())
    }
}

/// Read and dispatch our queue, and serve requests, until told to stop.
fn run(
    mut queue: EventQueue<State>,
    mut state: State,
    woken: OwnedFd,
    requests: mpsc::Receiver<Request>,
) -> State {
    let qh = queue.handle();
    loop {
        if let Err(error) = queue.dispatch_pending(&mut state) {
            tracing::warn!("drag out: Wayland dispatch failed: {error}");
            return state;
        }
        if let Err(error) = state.outgoing.flush(&state.conn) {
            tracing::warn!("drag out: Wayland flush failed: {error}");
            return state;
        }
        // None: events arrived for our queue meanwhile; dispatch them.
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let pollfd = |fd: i32, events| libc::pollfd {
            fd,
            events,
            revents: 0,
        };
        let mut fds = vec![
            pollfd(
                guard.connection_fd().as_raw_fd(),
                state.outgoing.poll_events(),
            ),
            pollfd(woken.as_raw_fd(), libc::POLLIN),
        ];
        fds.extend(
            state
                .transfers
                .iter()
                .map(|transfer| pollfd(transfer.file.as_raw_fd(), libc::POLLOUT)),
        );
        // SAFETY: valid pollfds over open descriptors, as many as given.
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) } < 0 {
            continue;
        }
        // Only writable: the next turn flushes.
        if fds[0].revents & !libc::POLLOUT != 0 {
            if let Err(error) = guard.read()
                && !matches!(&error, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
            {
                tracing::warn!("drag out: Wayland read failed: {error}");
                return state;
            }
        } else {
            // Dropping the guard cancels the read.
            drop(guard);
        }
        // One pollfd per transfer, in order.
        let mut ready = fds[2..].iter().map(|fd| fd.revents != 0);
        state
            .transfers
            .retain_mut(|transfer| !ready.next().unwrap_or(false) || !transfer.write());
        if fds[1].revents == 0 {
            continue;
        }
        let mut drained = [0u8; 64];
        // SAFETY: reads into a local buffer from the pipe's nonblocking read
        // end, until it is empty.
        while unsafe { libc::read(woken.as_raw_fd(), drained.as_mut_ptr().cast(), 64) } > 0 {}
        while let Ok(request) = requests.try_recv() {
            match request {
                Request::Stop => return state,
                Request::Window {
                    surface,
                    open: true,
                } => state.seats.window_created(surface),
                Request::Window {
                    surface,
                    open: false,
                } => state.seats.window_destroyed(surface),
                Request::DockEnd(token) => state.end_dock(token),
                Request::Start(start) => {
                    // Starts the UI gave up on, whose syncs never came back.
                    state.starts.retain(|_, start| !start.ticket.abandoned());
                    let key = state.next_start;
                    state.next_start += 1;
                    state.starts.insert(key, start);
                    // Served when this sync is done; see `Dispatch<WlCallback>`.
                    state.conn.display().sync(&qh, key);
                }
            }
        }
    }
}

pub(super) fn window_created(surface: *mut std::ffi::c_void) {
    send(Request::Window {
        surface: surface as Surface,
        open: true,
    });
}

pub(super) fn window_destroyed(surface: *mut std::ffi::c_void) {
    send(Request::Window {
        surface: surface as Surface,
        open: false,
    });
}

fn send(request: Request) {
    WAYLAND.with(|slot| {
        if let Some(wayland) = slot.borrow().as_ref() {
            let _ = wayland.send(request);
        }
    });
}

pub(super) fn shutdown() {
    WAYLAND.with(|slot| {
        let Some(wayland) = slot.borrow_mut().take() else {
            return;
        };
        let _ = wayland.send(Request::Stop);
        let Ok(state) = wayland.thread.join() else {
            return;
        };
        let conn = state.conn.clone();
        for objects in state.seats.into_objects() {
            objects.release();
        }
        let _ = conn.flush();
    });
}

/// Start a drag from `window`, which this keeps borrowed, and so open,
/// until the thread is done with it.
pub(super) fn start(
    window: WindowHandle<'_>,
    uris: Vec<u8>,
    seat: Option<String>,
    image: &DragImage,
) -> Result<(), DragOutError> {
    request_start(
        window,
        Content::Files(uris),
        seat,
        Some(image),
        Answerer::Files,
    )
}

/// Start a dock drag from `window`, as [`start`] does a drag of files.
pub(super) fn start_dock(
    window: WindowHandle<'_>,
    request: DockRequest,
    seat: Option<String>,
    image: Option<&DragImage>,
) -> Result<DockStarted, DragOutError> {
    request_start(window, Content::Dock(request), seat, image, Answerer::Dock)
}

/// Stop the dock drag with `token` if it still runs.
pub(super) fn end_dock(token: u64) {
    send(Request::DockEnd(token));
}

fn request_start<T>(
    window: WindowHandle<'_>,
    content: Content,
    seat: Option<String>,
    image: Option<&DragImage>,
    answerer: fn(Ticket<T>) -> Answerer,
) -> Result<T, DragOutError> {
    let RawWindowHandle::Wayland(handle) = window.as_raw() else {
        return Err(DragOutError::Unsupported);
    };
    let surface = handle.surface.as_ptr();
    // SAFETY: winit's window handle is its window's wl_surface, live while
    // `window` borrows the window.
    let origin = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }
        .map_err(|_| DragOutError::Platform("the window has no wl_surface".into()))?;
    // Before anything about the seat, so the drag starts at once after.
    let icon = image.and_then(|image| match icon_pixels(image) {
        Ok(icon) => Some(icon),
        Err(error) => {
            tracing::warn!("drag out: no drag image: {error}");
            None
        }
    });
    WAYLAND.with(|slot| {
        let slot = slot.borrow();
        let wayland = slot
            .as_ref()
            .ok_or_else(|| DragOutError::Platform("no Wayland seat to drag with".into()))?;
        let (waiter, ticket) = ticket::ticket();
        wayland.send(Request::Start(Start {
            drag: Drag {
                origin,
                surface: surface as Surface,
                content,
                seat,
                icon,
            },
            ticket: answerer(ticket),
        }))?;
        // Holds `window` until the thread can no longer touch its surface.
        waiter.wait(ANSWER_TIMEOUT)
    })
}

/// `image` as premultiplied `ARGB8888` in a memfd, padded to a multiple of
/// the buffer scale, which `wl_surface` requires of buffer sizes.
fn icon_pixels(image: &DragImage) -> std::io::Result<IconPixels> {
    let scale = (image.scale().round() as u32).clamp(1, 16);
    let pad = |side: u32| side.div_ceil(scale) * scale;
    let (width, height) = (pad(image.width()), pad(image.height()));
    let pixels = image.premultiplied_bgra();
    let row = image.width() as usize * 4;
    let mut padded = vec![0u8; width as usize * height as usize * 4];
    for (y, line) in pixels.chunks_exact(row).enumerate() {
        let at = y * width as usize * 4;
        padded[at..at + row].copy_from_slice(line);
    }
    // SAFETY: memfd_create with a NUL-terminated name; the descriptor it
    // returns is owned here.
    let file = unsafe {
        let fd = libc::memfd_create(c"quark-drag-image".as_ptr(), libc::MFD_CLOEXEC);
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        OwnedFd::from_raw_fd(fd)
    };
    let mut writer = std::fs::File::from(file);
    writer.write_all(&padded)?;
    let (hx, hy) = image.hotspot();
    let logical = |px: u32| (f64::from(px) / f64::from(scale)).round() as i32;
    Ok(IconPixels {
        file: writer.into(),
        width: width as i32,
        height: height as i32,
        buffer_scale: scale as i32,
        hotspot: (logical(hx), logical(hy)),
    })
}

/// A drag's icon surface and its buffer, kept until the drag ends.
#[derive(Clone)]
struct Icon {
    surface: WlSurface,
    buffer: WlBuffer,
    pool: WlShmPool,
}

impl Icon {
    fn destroy(&self) {
        // The surface first, which lets go of the buffer.
        self.surface.destroy();
        self.buffer.destroy();
        self.pool.destroy();
    }
}

impl State {
    /// A global, at startup or added later.
    fn global(&mut self, qh: &QueueHandle<Self>, name: u32, interface: &str, version: u32) {
        match interface {
            "wl_seat" => {
                let seat: WlSeat = self.registry.bind(name, version.min(7), qh, name);
                let device = self
                    .manager
                    .as_ref()
                    .map(|manager| manager.get_data_device(&seat, qh, name));
                self.seats.add(
                    name,
                    SeatObjects {
                        seat,
                        pointer: None,
                        device,
                        selection_offer: None,
                        drag_offer: None,
                    },
                );
            }
            "wl_data_device_manager" if self.manager.is_none() => {
                let manager: WlDataDeviceManager = self.registry.bind(name, version.min(3), qh, ());
                for (key, seat) in self.seats.iter_mut() {
                    seat.objects.device =
                        Some(manager.get_data_device(&seat.objects.seat, qh, key));
                }
                self.manager = Some(manager);
            }
            "wl_compositor" if self.compositor.is_none() => {
                self.compositor = Some(self.registry.bind(name, version.min(6), qh, ()));
            }
            "wl_shm" if self.shm.is_none() => {
                self.shm = Some(self.registry.bind(name, 1, qh, ()));
            }
            "xdg_toplevel_drag_manager_v1" if self.toplevel_drags.is_none() => {
                self.toplevel_drags = Some(self.registry.bind(name, 1, qh, ()));
            }
            _ => {}
        }
    }

    /// A drag request whose round trip is done: start it, unless the UI
    /// stopped waiting, when its window may be gone.
    fn serve(&mut self, qh: &QueueHandle<Self>, Start { drag, ticket }: Start) {
        let unclaimed = || tracing::debug!("drag out: the UI stopped waiting; not started");
        match ticket {
            Answerer::Files(ticket) => match ticket.claim() {
                Some(claim) => claim.answer(self.start(qh, drag).map(drop)),
                None => unclaimed(),
            },
            Answerer::Dock(ticket) => match ticket.claim() {
                Some(claim) => {
                    claim.answer(self.start(qh, drag).map(|toplevel_drag| DockStarted {
                        conn: self.conn.clone(),
                        toplevel_drag,
                    }))
                }
                None => unclaimed(),
            },
        }
    }

    /// Under a claim on the request's ticket. A dock drag answers with its
    /// `xdg_toplevel_drag_v1`, where the compositor has them.
    fn start(
        &mut self,
        qh: &QueueHandle<Self>,
        start: Drag,
    ) -> Result<Option<XdgToplevelDragV1>, DragOutError> {
        let Selected { seat, press } = self
            .seats
            .select(start.surface, start.seat.as_deref())
            .map_err(|refusal| match refusal {
                Refusal::NoPress => DragOutError::NoPointerEvent,
                Refusal::Ambiguous => DragOutError::AmbiguousSeat,
                Refusal::UnknownSeat(name) => DragOutError::UnknownSeat(name),
            })?;
        let platform = |message: &str| DragOutError::Platform(message.into());
        let manager = self.manager.as_ref().ok_or(platform("no data devices"))?;
        let device = self
            .seats
            .get_mut(seat)
            .and_then(|state| state.objects.device.clone())
            .ok_or(platform("the seat has no data device"))?;
        // The claim keeps the UI waiting, and so the window and its surface
        // alive, until this answers.
        let origin = WlSurface::from_id(&self.conn, start.origin)
            .map_err(|_| platform("the window has no wl_surface"))?;
        let icon = start
            .icon
            .as_ref()
            .and_then(|pixels| self.icon(qh, pixels).map(|icon| (icon, pixels)));
        let (offered, mime, action, dock) = match start.content {
            Content::Files(uris) => (
                Offered::Files(Arc::from(uris)),
                URI_LIST.to_owned(),
                DndAction::Copy,
                None,
            ),
            Content::Dock(request) => (
                Offered::Dock(request.token),
                request.mime.clone(),
                DndAction::Move,
                Some(request),
            ),
        };
        let source = manager.create_data_source(
            qh,
            Payload {
                offered,
                icon: icon.as_ref().map(|(icon, _)| icon.clone()),
            },
        );
        source.offer(mime);
        if source.version() >= 3 {
            source.set_actions(action);
        }
        // Must come before the drag starts.
        let toplevel_drag = dock
            .as_ref()
            .and(self.toplevel_drags.as_ref())
            .map(|manager| manager.get_xdg_toplevel_drag(&source, qh, ()));
        device.start_drag(
            Some(&source),
            &origin,
            icon.as_ref().map(|(icon, _)| &icon.surface),
            press.serial,
        );
        // The surface has its drag icon role now; give it the pixels, placed
        // so the hotspot is under the pointer.
        if let Some((icon, pixels)) = &icon {
            show(icon, pixels);
        }
        tracing::debug!(
            seat,
            serial = press.serial,
            sequence = press.sequence,
            dock = dock.is_some(),
            "drag out started"
        );
        if let Some(request) = dock {
            // An earlier dock drag's source, if it lingers, still cleans up
            // after itself; its events no longer match the token.
            self.dock = Some(ActiveDock {
                token: request.token,
                mime: request.mime,
                signals: request.signals,
                waker: request.waker,
                source,
                toplevel_drag: toplevel_drag.clone(),
                entered: None,
            });
        }
        // Queued is started: the loop flushes the rest once the socket
        // takes it.
        self.outgoing
            .flush(&self.conn)
            .map_err(|error| DragOutError::Platform(error.to_string()))?;
        Ok(toplevel_drag)
    }

    /// Give up the dock drag with `token`: destroying its source ends the
    /// drag. Its `xdg_toplevel_drag_v1` is left alone, since destroying
    /// one before the compositor reports the drag over is a protocol
    /// error, and no report comes for a destroyed source.
    fn end_dock(&mut self, token: u64) {
        if self.dock.as_ref().is_none_or(|dock| dock.token != token) {
            return;
        }
        if let Some(dock) = self.dock.take() {
            if let Some(icon) = dock.source.data::<Payload>().and_then(|p| p.icon.as_ref()) {
                icon.destroy();
            }
            dock.source.destroy();
        }
    }

    /// A roleless surface with a buffer of `pixels`, not yet committed.
    fn icon(&self, qh: &QueueHandle<Self>, pixels: &IconPixels) -> Option<Icon> {
        let (compositor, shm) = (self.compositor.as_ref()?, self.shm.as_ref()?);
        let stride = pixels.width * 4;
        // libwayland duplicates the descriptor, so ours can close after.
        let pool = shm.create_pool(pixels.file.as_fd(), stride * pixels.height, qh, ());
        let buffer = pool.create_buffer(
            0,
            pixels.width,
            pixels.height,
            stride,
            wl_shm::Format::Argb8888,
            qh,
            (),
        );
        let surface = compositor.create_surface(qh, ());
        Some(Icon {
            surface,
            buffer,
            pool,
        })
    }
}

fn show(icon: &Icon, pixels: &IconPixels) {
    let surface = &icon.surface;
    if surface.version() >= 3 {
        surface.set_buffer_scale(pixels.buffer_scale);
    }
    let (dx, dy) = (-pixels.hotspot.0, -pixels.hotspot.1);
    // Since version 5 attach's offset must be zero, and offset moves it.
    if surface.version() >= 5 {
        surface.attach(Some(&icon.buffer), 0, 0);
        surface.offset(dx, dy);
    } else {
        surface.attach(Some(&icon.buffer), dx, dy);
    }
    if surface.version() >= 4 {
        surface.damage_buffer(0, 0, pixels.width, pixels.height);
    } else {
        surface.damage(0, 0, i32::MAX, i32::MAX);
    }
    surface.commit();
}

/// A drop target reading the `text/uri-list`, written as its pipe takes it
/// so that a target that stops reading holds up nothing else. Dropped
/// unfinished (on shutdown), it closes the pipe, cutting the list short.
struct Transfer {
    file: std::fs::File,
    uris: Arc<[u8]>,
    written: usize,
}

impl Transfer {
    fn new(fd: OwnedFd, uris: Arc<[u8]>) -> std::io::Result<Self> {
        // SAFETY: fcntl on a descriptor this owns.
        unsafe {
            let flags = libc::fcntl(fd.as_raw_fd(), libc::F_GETFL);
            if flags < 0 || libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(Self {
            file: fd.into(),
            uris,
            written: 0,
        })
    }

    /// Write what the pipe takes now. True once done: all written, or the
    /// target went away.
    fn write(&mut self) -> bool {
        while self.written < self.uris.len() {
            match self.file.write(&self.uris[self.written..]) {
                Ok(0) => return true,
                Ok(n) => self.written += n,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => return false,
                Err(error) => {
                    tracing::warn!("drag out: writing the dropped files failed: {error}");
                    return true;
                }
            }
        }
        true
    }
}

/// What a data source offers, and the icon it drags with.
struct Payload {
    offered: Offered,
    icon: Option<Icon>,
}

enum Offered {
    /// A `text/uri-list`.
    Files(Arc<[u8]>),
    /// A dock drag, by token; it sends no data.
    Dock(u64),
}

/// What a data offer has said about itself so far.
#[derive(Debug, Default)]
struct OfferState {
    mimes: Mutex<Vec<String>>,
    /// The action the compositor settled on, from `wl_data_offer.action`.
    action: Mutex<Option<DndAction>>,
}

impl OfferState {
    fn offers(offer: &WlDataOffer, mime: &str) -> bool {
        offer
            .data::<OfferState>()
            .is_some_and(|state| lock(&state.mimes).iter().any(|offered| offered == mime))
    }

    fn action(offer: &WlDataOffer) -> Option<DndAction> {
        *lock(&offer.data::<OfferState>()?.action)
    }
}

/// A plain lock: every change under these is a single assignment or push,
/// so a panic while holding one leaves its value whole.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poison| poison.into_inner())
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        state: &mut Self,
        _: &WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => state.global(qh, name, &interface, version),
            wl_registry::Event::GlobalRemove { name } => {
                if let Some(objects) = state.seats.remove(name) {
                    objects.release();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, u32> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        &key: &u32,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_seat::Event::Capabilities { capabilities } => {
                let has_pointer = matches!(capabilities, WEnum::Value(caps) if caps.contains(wl_seat::Capability::Pointer));
                let Some(objects) = state.seats.get_mut(key).map(|seat| &mut seat.objects) else {
                    return;
                };
                match (has_pointer, objects.pointer.take()) {
                    // Asking for a pointer the seat never had is a protocol
                    // error, and would end winit's connection too, so only
                    // on the event.
                    (true, None) => objects.pointer = Some(seat.get_pointer(qh, key)),
                    (true, pointer) => objects.pointer = pointer,
                    (false, Some(pointer)) => {
                        if pointer.version() >= 3 {
                            pointer.release();
                        }
                    }
                    (false, None) => {}
                }
                state.seats.pointer(key, has_pointer);
            }
            wl_seat::Event::Name { name } => state.seats.named(key, name),
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        &seat: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter { surface, .. } => {
                // Null once the surface is gone.
                let surface = Some(surface.id().as_ptr() as Surface).filter(|&ptr| ptr != 0);
                state.seats.enter(seat, surface);
            }
            wl_pointer::Event::Leave { .. } => state.seats.leave(seat),
            wl_pointer::Event::Button {
                serial,
                button,
                state: pressed,
                ..
            } => {
                let pressed = matches!(pressed, WEnum::Value(wl_pointer::ButtonState::Pressed));
                state.seats.button(seat, serial, button, pressed);
            }
            _ => {}
        }
    }
}

impl Dispatch<WlDataDevice, u32> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        &seat: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // Known before borrowing the seat: whether it is one of ours.
        let owned = match &event {
            wl_data_device::Event::Enter { surface, .. } => {
                let surface = surface.id().as_ptr() as Surface;
                (surface != 0 && state.seats.is_window(surface)).then_some(surface)
            }
            _ => None,
        };
        let Some(objects) = state.seats.get_mut(seat).map(|seat| &mut seat.objects) else {
            return;
        };
        // Only the current dock drag's events over our windows go on.
        let dock = state.dock.as_mut();
        match event {
            wl_data_device::Event::Enter {
                serial, x, y, id, ..
            } => {
                replace(&mut objects.drag_offer, id);
                let (Some(dock), Some(offer)) = (dock, &objects.drag_offer) else {
                    return;
                };
                if !OfferState::offers(offer, &dock.mime) {
                    return;
                }
                if dock.entered == Some(seat) {
                    dock.entered = None;
                }
                match owned {
                    Some(surface) => {
                        offer.accept(serial, Some(dock.mime.clone()));
                        if offer.version() >= 3 {
                            offer.set_actions(DndAction::Move, DndAction::Move);
                        }
                        dock.entered = Some(seat);
                        dock.signal(DockSignal::Enter { surface, x, y });
                    }
                    None => offer.accept(serial, None),
                }
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                if let Some(dock) = dock.filter(|dock| dock.entered == Some(seat)) {
                    dock.signal(DockSignal::Motion { x, y });
                }
            }
            wl_data_device::Event::Leave => {
                if let Some(dock) = dock.filter(|dock| dock.entered == Some(seat)) {
                    dock.entered = None;
                    dock.signal(DockSignal::Leave);
                }
                replace(&mut objects.drag_offer, None);
            }
            wl_data_device::Event::Drop => {
                if let Some(dock) = dock.filter(|dock| dock.entered == Some(seat)) {
                    dock.entered = None;
                    // Finishing an offer without a settled action is a
                    // protocol error, fatal to winit's connection too.
                    if let Some(offer) = &objects.drag_offer
                        && offer.version() >= 3
                        && OfferState::action(offer) == Some(DndAction::Move)
                    {
                        offer.finish();
                    }
                    dock.signal(DockSignal::Drop);
                }
                replace(&mut objects.drag_offer, None);
            }
            wl_data_device::Event::Selection { id } => replace(&mut objects.selection_offer, id),
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, OfferState::default()),
    ]);
}

/// Hold `offer` in `slot`, destroying the offer it replaces.
fn replace(slot: &mut Option<WlDataOffer>, offer: Option<WlDataOffer>) {
    if let Some(old) = std::mem::replace(slot, offer)
        && slot.as_ref() != Some(&old)
    {
        old.destroy();
    }
}

impl Dispatch<WlCallback, u64> for State {
    fn event(
        state: &mut Self,
        _: &WlCallback,
        event: wl_callback::Event,
        key: &u64,
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_callback::Event::Done { .. } = event
            && let Some(start) = state.starts.remove(key)
        {
            state.serve(qh, start);
        }
    }
}

impl Dispatch<WlDataOffer, OfferState> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        offer: &OfferState,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_offer::Event::Offer { mime_type } => lock(&offer.mimes).push(mime_type),
            wl_data_offer::Event::Action {
                dnd_action: WEnum::Value(action),
            } => *lock(&offer.action) = Some(action),
            _ => {}
        }
    }
}

impl Dispatch<WlDataSource, Payload> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        payload: &Payload,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        // This source's dock drag, if it is the one running.
        let token = match payload.offered {
            Offered::Dock(token) => Some(token),
            Offered::Files(_) => None,
        };
        let dock = state.dock.as_mut().filter(|dock| Some(dock.token) == token);
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let Offered::Files(uris) = &payload.offered else {
                    // A dock drag carries nothing; closing the pipe ends it.
                    return;
                };
                if mime_type != URI_LIST {
                    return;
                }
                match Transfer::new(fd, Arc::clone(uris)) {
                    Ok(mut transfer) => {
                        if !transfer.write() {
                            state.transfers.push(transfer);
                        }
                    }
                    Err(error) => {
                        tracing::warn!("drag out: writing the dropped files failed: {error}")
                    }
                }
            }
            wl_data_source::Event::DndDropPerformed => {
                if let Some(dock) = dock {
                    if let Some(drag) = dock.toplevel_drag.take() {
                        drag.destroy();
                    }
                    dock.signal(DockSignal::DropPerformed);
                }
            }
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => {
                if dock.is_some()
                    && let Some(dock) = state.dock.take()
                {
                    if let Some(drag) = &dock.toplevel_drag {
                        drag.destroy();
                    }
                    dock.signal(match event {
                        wl_data_source::Event::Cancelled => DockSignal::Cancelled,
                        _ => DockSignal::Finished,
                    });
                }
                source.destroy();
                if let Some(icon) = &payload.icon {
                    icon.destroy();
                }
            }
            _ => {}
        }
    }
}

/// Objects whose events this has no use for.
macro_rules! ignore_events {
    ($($proxy:ty),*) => {$(
        impl Dispatch<$proxy, ()> for State {
            fn event(
                _: &mut Self,
                _: &$proxy,
                _: <$proxy as Proxy>::Event,
                _: &(),
                _: &Connection,
                _: &QueueHandle<Self>,
            ) {
            }
        }
    )*};
}

ignore_events!(
    WlDataDeviceManager,
    XdgToplevelDragManagerV1,
    XdgToplevelDragV1,
    WlCompositor,
    WlShm,
    WlShmPool,
    WlBuffer,
    WlSurface
);

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::sync::Arc;

    use super::{Outgoing, Transfer};
    use wayland_client::backend::WaylandError;

    /// A blocking pipe, as a drop target hands over: its read end, and the
    /// write end for the transfer.
    fn pipe() -> (std::fs::File, OwnedFd) {
        let mut fds = [0; 2];
        // SAFETY: a plain pipe2 into a two-element array.
        assert_eq!(unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) }, 0);
        // SAFETY: both ends are fresh descriptors owned from here on.
        unsafe {
            (
                std::fs::File::from_raw_fd(fds[0]),
                OwnedFd::from_raw_fd(fds[1]),
            )
        }
    }

    /// A list far larger than a pipe holds.
    fn long_list() -> Arc<[u8]> {
        (0..1 << 20).map(|i| b'a' + (i % 26) as u8).collect()
    }

    #[test]
    fn a_drop_target_that_stops_reading_holds_up_nothing_and_is_cut_off_on_shutdown() {
        let (mut reader, write_end) = pipe();
        let uris = long_list();
        let mut transfer = Transfer::new(write_end, Arc::clone(&uris)).unwrap();
        assert!(!transfer.write(), "a full pipe leaves the rest for later");
        drop(transfer);
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        assert!(got.len() < uris.len());
        assert_eq!(got[..], uris[..got.len()]);
    }

    #[test]
    fn a_drop_target_that_keeps_reading_gets_the_whole_list() {
        let (mut reader, write_end) = pipe();
        let uris = long_list();
        let reading = std::thread::spawn(move || {
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            got
        });
        let mut transfer = Transfer::new(write_end, Arc::clone(&uris)).unwrap();
        while !transfer.write() {
            let mut fd = libc::pollfd {
                fd: std::os::fd::AsRawFd::as_raw_fd(&transfer.file),
                events: libc::POLLOUT,
                revents: 0,
            };
            // SAFETY: one valid pollfd over an open descriptor.
            unsafe { libc::poll(&mut fd, 1, -1) };
        }
        drop(transfer);
        assert!(reading.join().unwrap()[..] == uris[..]);
    }

    // Regression: a flush the socket would not take was ignored, so the
    // queued requests (a drag's start among them) waited for an unrelated
    // event, and a drag reported failure though it was queued.
    #[test]
    fn requests_the_socket_would_not_take_are_queued_and_polled_for_writing() {
        use libc::{POLLIN, POLLOUT};
        use std::io::ErrorKind::{BrokenPipe, WouldBlock};
        let io = |kind: std::io::ErrorKind| Err(WaylandError::Io(kind.into()));
        let mut outgoing = Outgoing::default();
        let flushes = [
            ("would block", io(WouldBlock), true, POLLIN | POLLOUT),
            ("would block again", io(WouldBlock), true, POLLIN | POLLOUT),
            ("got through", Ok::<(), WaylandError>(()), true, POLLIN),
            ("connection broke", io(BrokenPipe), false, POLLIN),
        ];
        for (name, result, accepted, events) in flushes {
            let got = outgoing.flushed(result);
            assert_eq!(
                (got.is_ok(), outgoing.poll_events()),
                (accepted, events),
                "{name}"
            );
        }
    }
}
