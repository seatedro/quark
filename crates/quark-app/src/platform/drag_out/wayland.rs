//! Dragging out on Wayland, over winit's `wl_display`.
//!
//! winit keeps its seat and pointer private, so this binds its own on a
//! separate event queue of the same connection (`Backend::from_foreign_display`
//! over the system libwayland winit also uses). The compositor sends pointer
//! events to every `wl_pointer` a client owns, so our pointer sees the button
//! press serial that `wl_data_device.start_drag` needs to prove the implicit
//! grab. That is why the pointer is bound when the first window opens rather
//! than when a drag starts: by then the press has already happened.
//!
//! Our queue is read and dispatched by a thread of its own. winit's loop
//! cannot do it: it reads our events off the socket, but sleeps on without
//! an iteration when none were for its own queue. libwayland lets several
//! threads read one connection (`wl_display_prepare_read_queue`), and the
//! thread stops, through a pipe, before winit disconnects.

use std::cell::RefCell;
use std::io::Write;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;

use wayland_client::backend::{Backend, ObjectId};
use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::WlDataOffer;
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, event_created_child,
};

use super::DragOutError;

const URI_LIST: &str = "text/uri-list";

/// `Press` with no button held.
const NO_PRESS: u64 = u64::MAX;

thread_local! {
    static WAYLAND: RefCell<Option<Wayland>> = const { RefCell::new(None) };
}

struct Wayland {
    conn: Connection,
    qh: QueueHandle<State>,
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    /// The serial of the press holding the pointer's implicit grab, or
    /// `NO_PRESS`.
    press: Arc<AtomicU64>,
    /// Written to stop the thread.
    stop: OwnedFd,
    thread: JoinHandle<State>,
}

struct State {
    pointer: Option<WlPointer>,
    press: Arc<AtomicU64>,
    /// Offers the compositor hands our data device (the clipboard, drags
    /// passing over our windows). Unused, but each must be destroyed once
    /// replaced.
    selection_offer: Option<WlDataOffer>,
    drag_offer: Option<WlDataOffer>,
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
    let seat: WlSeat = globals
        .bind(&qh, 1..=7, ())
        .map_err(|error| format!("wl_seat: {error}"))?;
    let manager: WlDataDeviceManager = globals
        .bind(&qh, 1..=3, ())
        .map_err(|error| format!("wl_data_device_manager: {error}"))?;
    let device = manager.get_data_device(&seat, &qh, ());
    let press = Arc::new(AtomicU64::new(NO_PRESS));
    let mut state = State {
        pointer: None,
        press: press.clone(),
        selection_offer: None,
        drag_offer: None,
    };
    // The seat's capabilities, and with them our pointer.
    queue
        .roundtrip(&mut state)
        .map_err(|error| error.to_string())?;
    let mut fds = [0; 2];
    // SAFETY: a plain pipe2 into a two-element array.
    if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) } != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    // SAFETY: both ends are fresh descriptors this function now owns.
    let (wake, stop) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
    let thread = std::thread::Builder::new()
        .name("quark-drag-out".into())
        .spawn({
            let conn = conn.clone();
            move || run(conn, queue, state, wake)
        })
        .map_err(|error| error.to_string())?;
    Ok(Wayland {
        conn,
        qh,
        manager,
        device,
        press,
        stop,
        thread,
    })
}

/// Read and dispatch our queue until `wake` is written.
fn run(conn: Connection, mut queue: EventQueue<State>, mut state: State, wake: OwnedFd) -> State {
    loop {
        if let Err(error) = queue.dispatch_pending(&mut state) {
            tracing::warn!("drag out: Wayland dispatch failed: {error}");
            return state;
        }
        let _ = conn.flush();
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
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: two valid pollfds over open descriptors.
        if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0 {
            continue;
        }
        if fds[1].revents != 0 {
            // Dropping the guard cancels the read.
            return state;
        }
        if fds[0].revents != 0
            && let Err(error) = guard.read()
            && !matches!(&error, wayland_client::backend::WaylandError::Io(io) if io.kind() == std::io::ErrorKind::WouldBlock)
        {
            tracing::warn!("drag out: Wayland read failed: {error}");
            return state;
        }
    }
}

pub(super) fn shutdown() {
    WAYLAND.with(|slot| {
        let Some(wayland) = slot.borrow_mut().take() else {
            return;
        };
        // SAFETY: a one-byte write to the pipe's open write end.
        unsafe { libc::write(wayland.stop.as_fd().as_raw_fd(), [0u8].as_ptr().cast(), 1) };
        let Ok(state) = wayland.thread.join() else {
            return;
        };
        if let Some(pointer) = &state.pointer
            && pointer.version() >= 3
        {
            pointer.release();
        }
        if wayland.device.version() >= 2 {
            wayland.device.release();
        }
        let _ = wayland.conn.flush();
    });
}

pub(super) fn start(surface: *mut std::ffi::c_void, uris: Vec<u8>) -> Result<(), DragOutError> {
    WAYLAND.with(|slot| {
        let slot = slot.borrow();
        let wayland = slot
            .as_ref()
            .ok_or_else(|| DragOutError::Platform("no Wayland seat to drag with".into()))?;
        let serial = match wayland.press.load(Ordering::Acquire) {
            NO_PRESS => return Err(DragOutError::NoPointerEvent),
            serial => serial as u32,
        };
        // SAFETY: winit's window handle is its window's live wl_surface.
        let id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface.cast()) }
            .map_err(|_| DragOutError::Platform("the window has no wl_surface".into()))?;
        let origin = WlSurface::from_id(&wayland.conn, id)
            .map_err(|_| DragOutError::Platform("the window has no wl_surface".into()))?;
        let source = wayland
            .manager
            .create_data_source(&wayland.qh, Payload(Arc::from(uris)));
        source.offer(URI_LIST.into());
        if source.version() >= 3 {
            source.set_actions(DndAction::Copy);
        }
        wayland
            .device
            .start_drag(Some(&source), &origin, None, serial);
        wayland
            .conn
            .flush()
            .map_err(|error| DragOutError::Platform(error.to_string()))
    })
}

/// The `text/uri-list` a data source hands out.
struct Payload(Arc<[u8]>);

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(
        state: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities { capabilities } = event else {
            return;
        };
        let has_pointer = matches!(capabilities, WEnum::Value(caps) if caps.contains(wl_seat::Capability::Pointer));
        match (has_pointer, state.pointer.take()) {
            // Asking for a pointer the seat never had is a protocol error,
            // and would end winit's connection too, so only on the event.
            (true, None) => state.pointer = Some(seat.get_pointer(qh, ())),
            (true, pointer) => state.pointer = pointer,
            (false, Some(pointer)) => {
                if pointer.version() >= 3 {
                    pointer.release();
                }
                state.press.store(NO_PRESS, Ordering::Release);
            }
            (false, None) => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Button {
                serial,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } => state.press.store(u64::from(serial), Ordering::Release),
            wl_pointer::Event::Button { .. } => state.press.store(NO_PRESS, Ordering::Release),
            _ => {}
        }
    }
}

impl Dispatch<WlDataDeviceManager, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataDeviceManager,
        _: <WlDataDeviceManager as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Enter { id, .. } => replace(&mut state.drag_offer, id),
            wl_data_device::Event::Leave | wl_data_device::Event::Drop => {
                replace(&mut state.drag_offer, None)
            }
            wl_data_device::Event::Selection { id } => replace(&mut state.selection_offer, id),
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

impl Dispatch<WlDataOffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        _: <WlDataOffer as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
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
                if let Err(error) = std::fs::File::from(fd).write_all(&payload.0) {
                    tracing::warn!("drag out: writing the dropped files failed: {error}");
                }
            }
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => {
                source.destroy();
            }
            _ => {}
        }
    }
}
