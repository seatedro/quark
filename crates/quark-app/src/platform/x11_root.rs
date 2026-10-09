//! What the X11 root window says about the desktop, read on a connection
//! of our own (winit reads every event on its own): the work area, whether
//! a compositing manager runs, and whether it blurs behind windows. The
//! connection opens on first use and stays for the process.
//!
//! These are plain round trips, made when a window opens or is placed,
//! never per frame; the work area is cached for a second, since a drag
//! asks for placements on every motion.

use std::cell::RefCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use super::placement::PhysicalRect;

/// How long a work area read stays fresh.
const WORK_AREA_TTL: Duration = Duration::from_secs(1);

/// KWin's blur-behind property, set on a window to blur what is behind it
/// and announced as a root window property while the blur effect runs.
const KDE_BLUR: &[u8] = b"_KDE_NET_WM_BLUR_BEHIND_REGION";

static ACTIVE: AtomicBool = AtomicBool::new(false);

thread_local! {
    static ROOT: RefCell<Option<Root>> = const { RefCell::new(None) };
}

struct Root {
    conn: RustConnection,
    screen: usize,
    root: Window,
    work_area: Option<(Instant, Option<PhysicalRect>)>,
}

/// Note that the event loop talks to an X11 server.
pub(crate) fn set_active() {
    ACTIVE.store(true, Ordering::Relaxed);
}

/// Whether the event loop talks to an X11 server.
pub(crate) fn active() -> bool {
    ACTIVE.load(Ordering::Relaxed)
}

fn with_root<T>(f: impl FnOnce(&mut Root) -> Option<T>) -> Option<T> {
    ROOT.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            let (conn, screen) = x11rb::connect(None).ok()?;
            let root = conn.setup().roots[screen].root;
            *slot = Some(Root {
                conn,
                screen,
                root,
                work_area: None,
            });
        }
        f(slot.as_mut()?)
    })
}

impl Root {
    fn atom(&self, name: &[u8]) -> Option<u32> {
        Some(self.conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
    }

    fn cardinals(&self, window: Window, name: &[u8], length: u32) -> Option<Vec<u32>> {
        let atom = self.atom(name)?;
        let reply = self
            .conn
            .get_property(false, window, atom, AtomEnum::CARDINAL, 0, length)
            .ok()?
            .reply()
            .ok()?;
        Some(reply.value32()?.collect())
    }

    fn read_work_area(&self) -> Option<PhysicalRect> {
        let desktop = self
            .cardinals(self.root, b"_NET_CURRENT_DESKTOP", 1)
            .and_then(|values| values.first().copied())
            .unwrap_or(0) as usize;
        // Four cardinals per desktop: x, y, width, height.
        let areas = self.cardinals(self.root, b"_NET_WORKAREA", 4 * 64)?;
        let (areas, _) = areas.as_chunks::<4>();
        let area = areas.get(desktop).or(areas.first())?;
        Some(PhysicalRect {
            x: area[0] as i32,
            y: area[1] as i32,
            width: area[2],
            height: area[3],
        })
    }
}

/// The current desktop's work area across all displays, if the window
/// manager publishes one.
pub(crate) fn work_area() -> Option<PhysicalRect> {
    with_root(|root| {
        let now = Instant::now();
        if let Some((at, area)) = root.work_area
            && now.duration_since(at) < WORK_AREA_TTL
        {
            return Some(area);
        }
        let area = root.read_work_area();
        root.work_area = Some((now, area));
        Some(area)
    })
    .flatten()
}

/// Whether a compositing manager owns the screen's `_NET_WM_CM_S<n>`
/// selection, so a window's transparent pixels show what is behind it.
#[allow(dead_code)] // Used by the material adapter.
pub(crate) fn compositor() -> bool {
    with_root(|root| {
        let name = format!("_NET_WM_CM_S{}", root.screen);
        let atom = root.atom(name.as_bytes())?;
        let owner = root
            .conn
            .get_selection_owner(atom)
            .ok()?
            .reply()
            .ok()?
            .owner;
        Some(owner != x11rb::NONE)
    })
    .unwrap_or(false)
}

/// Whether KWin's blur effect runs: it announces its window property on
/// the root window (as `KWindowEffects::isEffectAvailable` checks).
#[allow(dead_code)] // Used by the material adapter.
pub(crate) fn blur_available() -> bool {
    with_root(|root| {
        let atom = root.atom(KDE_BLUR)?;
        let properties = root.conn.list_properties(root.root).ok()?.reply().ok()?;
        Some(properties.atoms.contains(&atom))
    })
    .unwrap_or(false)
}

/// Ask the compositor to blur behind all of `window` (an empty region
/// means the whole window), or stop. Returns whether the request was sent.
#[allow(dead_code)] // Used by the material adapter.
pub(crate) fn set_blur(window: Window, blur: bool) -> bool {
    with_root(|root| {
        let atom = root.atom(KDE_BLUR)?;
        if blur {
            root.conn
                .change_property32(PropMode::REPLACE, window, atom, AtomEnum::CARDINAL, &[])
                .ok()?;
        } else {
            root.conn.delete_property(window, atom).ok()?;
        }
        root.conn.flush().ok()
    })
    .is_some()
}
