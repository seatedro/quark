//! A scroll handle's state as a signal: what cache boundaries that paint
//! or read the handle depend on.

use std::rc::Weak;

use quark::reactive::{StoreId, is_tracking, track_read_at, track_unpublished};

use super::*;

/// What a container paints of its handle. Not the viewport: a replay at
/// another origin moves it without changing what the content shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ScrollStamp {
    offset: [f64; 2],
    max: [f64; 2],
    dragging: Option<Axis>,
    /// No request waits and no smooth scroll or fling moves the content.
    settled: bool,
}

impl ScrollState {
    fn stamp(&self) -> ScrollStamp {
        ScrollStamp {
            offset: self.offset,
            max: self.max,
            dragging: self.dragging,
            settled: self.request.is_none() && self.smooth.is_none() && self.fling.is_none(),
        }
    }
}

/// The signal a handle publishes its [`ScrollStamp`] to.
#[derive(Debug, Clone, Copy)]
pub(super) struct Published {
    store: StoreId,
    signal: Signal<ScrollStamp>,
    /// The signal's version after the last publish: a read is tracked at
    /// it without the store. Older than the signal's current version only
    /// when the store moved on, which a reader sees as a change.
    version: u64,
}

/// How a [`ScrollHandle::publish`] went.
pub(crate) enum Publish {
    Unchanged,
    /// The state changed. `seen`: a tracked reader read the state it
    /// replaced, so what that reader made is out of date.
    Changed {
        seen: bool,
    },
    /// The handle had no signal in this store; the caller owns this one.
    Created {
        signal: Signal<ScrollStamp>,
        seen: bool,
    },
}

/// Publish `state` to its signal in `store`, making one if it has none
/// there.
fn publish(state: &RefCell<ScrollState>, store: &SignalStore) -> Publish {
    let s = state.borrow();
    let stamp = s.stamp();
    let current = s
        .published
        .get()
        .filter(|p| p.store == store.id() && store.version(p.signal.id()).is_some());
    let seen = s.observed.get();
    let (signal, outcome) = match current {
        Some(p) if store.set_if_changed(p.signal, stamp) => (p.signal, Publish::Changed { seen }),
        Some(_) => return Publish::Unchanged,
        None => {
            let signal = store.create(stamp);
            (signal, Publish::Created { signal, seen })
        }
    };
    s.observed.set(false);
    s.published.set(Some(Published {
        store: store.id(),
        signal,
        version: store.version(signal.id()).expect("just published"),
    }));
    outcome
}

impl ScrollHandle {
    /// Publish what the container would paint now to the handle's signal
    /// in `store`.
    pub(crate) fn publish(&self, store: &SignalStore) -> Publish {
        publish(&self.0, store)
    }

    /// Register a read of the published state with the current tracking
    /// scope. Before the handle's first publish the reader stays out of
    /// date, so it reads the signal once there is one.
    pub(super) fn track(&self) {
        if !is_tracking() {
            return;
        }
        let s = self.0.borrow();
        match s.published.get() {
            Some(p) => track_read_at(p.signal.id(), p.version),
            None => track_unpublished(),
        }
        s.observed.set(true);
    }

    /// The cache's hold on the signal it made for this handle.
    pub(crate) fn binding(&self, signal: Signal<ScrollStamp>) -> ScrollBinding {
        ScrollBinding {
            state: Rc::downgrade(&self.0),
            signal,
        }
    }
}

/// A signal an element cache made for a handle. The cache publishes the
/// handle through it at the start of every frame, and disposes the signal
/// once the handle is gone.
pub(crate) struct ScrollBinding {
    state: Weak<RefCell<ScrollState>>,
    signal: Signal<ScrollStamp>,
}

impl ScrollBinding {
    /// Publish the handle. False once the handle is gone or publishes to
    /// another signal; then the caller disposes [`Self::signal`].
    pub(crate) fn publish(&self, store: &SignalStore) -> bool {
        let Some(state) = self.state.upgrade() else {
            return false;
        };
        let ours = state
            .borrow()
            .published
            .get()
            .is_some_and(|p| p.signal == self.signal);
        if ours {
            publish(&state, store);
        }
        ours
    }

    pub(crate) fn signal(&self) -> Signal<ScrollStamp> {
        self.signal
    }
}

/// A handle as a cache boundary's subtree painted it. The boundary depends
/// on the handle's signal; the cache publishes the watched handles before
/// it checks whether the recording may replay, since replay skips the
/// container's [`ScrollHandle::begin_frame`], which resolves requests and
/// motion.
#[derive(Clone)]
pub(crate) struct ScrollWatch {
    handle: ScrollHandle,
    /// The container's bounds, in the coordinates of whoever holds the
    /// watch (window points, or relative to a boundary's origin).
    viewport: Rect,
}

impl ScrollWatch {
    pub(crate) fn new(handle: ScrollHandle, viewport: Rect) -> Self {
        Self { handle, viewport }
    }

    pub(crate) fn handle(&self) -> &ScrollHandle {
        &self.handle
    }

    /// The watch with its viewport moved by `(dx, dy)`.
    pub(crate) fn offset(&self, dx: f32, dy: f32) -> Self {
        Self {
            viewport: self.viewport.offset(dx, dy),
            ..self.clone()
        }
    }

    /// A replay painted the container at the watched viewport: keep the
    /// handle's viewport (page size, item positions) current.
    pub(crate) fn replayed(&self) {
        self.handle.0.borrow_mut().viewport = self.viewport;
    }
}
