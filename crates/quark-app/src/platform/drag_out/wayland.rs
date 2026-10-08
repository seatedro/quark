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

use std::cell::RefCell;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::WlDataOffer;
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

use super::seat::{Refusal, Seats, Selected, Surface};
use super::{DragImage, DragOutError};

const URI_LIST: &str = "text/uri-list";

/// How long a drag request waits on the thread: a round trip with a live
/// compositor takes well under this.
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
    Window { surface: Surface, open: bool },
    Start(Start),
    Stop,
}

struct Start {
    surface: Surface,
    uris: Vec<u8>,
    seat: Option<String>,
    icon: Option<IconPixels>,
    answer: mpsc::SyncSender<Result<(), DragOutError>>,
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
    seats: Seats<SeatObjects>,
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
        seats: Seats::default(),
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
        let _ = state.conn.flush();
        // None: events arrived for our queue meanwhile; dispatch them.
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [
            libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: woken.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two valid pollfds over open descriptors.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            continue;
        }
        if fds[0].revents != 0 {
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
                Request::Start(start) => {
                    let answer = start.answer.clone();
                    let result = match queue.roundtrip(&mut state) {
                        Ok(_) => state.start(&qh, start),
                        Err(error) => Err(DragOutError::Platform(error.to_string())),
                    };
                    let _ = answer.send(result);
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

pub(super) fn start(
    surface: *mut std::ffi::c_void,
    uris: Vec<u8>,
    seat: Option<String>,
    image: &DragImage,
) -> Result<(), DragOutError> {
    // Before anything about the seat, so the drag starts at once after.
    let icon = match icon_pixels(image) {
        Ok(icon) => Some(icon),
        Err(error) => {
            tracing::warn!("drag out: no drag image: {error}");
            None
        }
    };
    WAYLAND.with(|slot| {
        let slot = slot.borrow();
        let wayland = slot
            .as_ref()
            .ok_or_else(|| DragOutError::Platform("no Wayland seat to drag with".into()))?;
        let (answer, answered) = mpsc::sync_channel(1);
        wayland.send(Request::Start(Start {
            surface: surface as Surface,
            uris,
            seat,
            icon,
            answer,
        }))?;
        answered
            .recv_timeout(ANSWER_TIMEOUT)
            .map_err(|_| DragOutError::Platform("the Wayland drag thread did not answer".into()))?
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
            _ => {}
        }
    }

    fn start(&mut self, qh: &QueueHandle<Self>, start: Start) -> Result<(), DragOutError> {
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
        // SAFETY: winit's window handle is its window's live wl_surface: the
        // UI thread holds the window open while it waits on this answer.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), start.surface as *mut _) }
            .map_err(|_| platform("the window has no wl_surface"))?;
        let origin = WlSurface::from_id(&self.conn, id)
            .map_err(|_| platform("the window has no wl_surface"))?;
        let icon = start
            .icon
            .as_ref()
            .and_then(|pixels| self.icon(qh, pixels).map(|icon| (icon, pixels)));
        let source = manager.create_data_source(
            qh,
            Payload {
                uris: Arc::from(start.uris),
                icon: icon.as_ref().map(|(icon, _)| icon.clone()),
            },
        );
        source.offer(URI_LIST.into());
        if source.version() >= 3 {
            source.set_actions(DndAction::Copy);
        }
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
            "drag out started"
        );
        self.conn
            .flush()
            .map_err(|error| DragOutError::Platform(error.to_string()))
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

/// A data source's `text/uri-list` and the icon it drags with.
struct Payload {
    uris: Arc<[u8]>,
    icon: Option<Icon>,
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
        let Some(objects) = state.seats.get_mut(seat).map(|seat| &mut seat.objects) else {
            return;
        };
        match event {
            wl_data_device::Event::Enter { id, .. } => replace(&mut objects.drag_offer, id),
            wl_data_device::Event::Leave | wl_data_device::Event::Drop => {
                replace(&mut objects.drag_offer, None)
            }
            wl_data_device::Event::Selection { id } => replace(&mut objects.selection_offer, id),
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
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

impl Dispatch<WlDataSource, Payload> for State {
    fn event(
        _: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        payload: &Payload,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } if mime_type == URI_LIST => {
                // On our thread, so a slow reader stalls only it.
                if let Err(error) = std::fs::File::from(fd).write_all(&payload.uris) {
                    tracing::warn!("drag out: writing the dropped files failed: {error}");
                }
            }
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => {
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
    WlDataOffer,
    WlCompositor,
    WlShm,
    WlShmPool,
    WlBuffer,
    WlSurface
);
