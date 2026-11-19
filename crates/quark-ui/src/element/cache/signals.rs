//! The cache's share of the signal store: the signals a window publishes
//! its frame state through, the reads each row's recording depends on, and
//! the scroll handles it publishes.
//!
//! Every boundary tracks what its subtree reads while it builds, lays out,
//! prepaints, and paints, in a scope of its own that merges into the
//! enclosing boundary's (see [`quark::reactive::Nesting::Merge`]): the
//! outer recording contains the inner one, so it depends on what the inner
//! one read. A replayed boundary reports its recorded reads to the
//! enclosing scope in place of reading them. Reads outside any boundary go
//! to the enclosing boundary as they are.

use quark::reactive::{Nesting, SignalRead, StoreId, track_into};

use super::*;
use crate::element::scroll::Publish;

/// The signals a window's [`ElementCache`] publishes its frame state
/// through, synced from the [`ElementContext`] before each frame lays out.
/// A [`cached`] or [`reactive`] boundary whose subtree reads them, through
/// [`ElementContext::focused`], [`ElementContext::is_focused`],
/// [`ElementContext::is_focus_visible`], or the store, rebuilds when they
/// change. Every boundary depends on the theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowSignals {
    pub focus: Signal<Option<FocusId>>,
    pub focus_visible: Signal<bool>,
    /// Bumped whenever the theme the window paints with changes.
    pub theme_generation: Signal<u64>,
}

/// The frame state [`WindowSignals`] publish.
#[derive(Debug, Clone, Copy)]
pub struct WindowFrameState<'t> {
    pub focus: Option<FocusId>,
    pub focus_visible: bool,
    pub theme: &'t Theme,
}

/// The store a cache's signals and snapshots live in.
pub(super) struct Bound {
    store: StoreId,
    window: WindowSignals,
}

impl ElementCache {
    /// Publish `state` to the window's signals in `store` and return them,
    /// making them on first use. Also publishes every scroll handle the
    /// cache made a signal for, and lets go of those whose handle is gone.
    /// A frame does this as it starts; a host calls it earlier to read
    /// the signals while building the frame's elements. Cheap when nothing
    /// changed.
    ///
    /// A cache handed another store than the one it last saw drops every
    /// row, since their snapshots name the other store's signals.
    pub fn sync_signals(&mut self, store: &SignalStore, state: WindowFrameState) -> WindowSignals {
        let window = self.bind(store);
        if self.theme.as_ref() != Some(state.theme) {
            self.theme = Some(state.theme.clone());
            self.theme_generation += 1;
        }
        store.batch(|| {
            store.set_if_changed(window.focus, state.focus);
            store.set_if_changed(window.focus_visible, state.focus_visible);
            store.set_if_changed(window.theme_generation, self.theme_generation);
        });
        let mut at = 0;
        while at < self.scroll.len() {
            if self.scroll[at].publish(store) {
                at += 1;
            } else {
                store.dispose(self.scroll.swap_remove(at).signal());
            }
        }
        window
    }

    /// The window's signals in `store`, made on first use.
    fn bind(&mut self, store: &SignalStore) -> WindowSignals {
        if let Some(bound) = &self.bound
            && bound.store == store.id()
        {
            return bound.window;
        }
        if self.bound.is_some() {
            // The signals stay in the store they were made in.
            self.clear();
            self.scroll.clear();
        }
        let window = WindowSignals {
            focus: store.create(None),
            focus_visible: store.create(true),
            theme_generation: store.create(self.theme_generation),
        };
        self.bound = Some(Bound {
            store: store.id(),
            window,
        });
        window
    }

    /// Dispose the signals the cache made in `store`: the window's and
    /// the scroll handles'. Call it when the window closes; a later frame
    /// makes them again.
    pub fn release_signals(&mut self, store: &SignalStore) {
        let Some(bound) = self.bound.take() else {
            return;
        };
        if bound.store != store.id() {
            return;
        }
        let WindowSignals {
            focus,
            focus_visible,
            theme_generation,
        } = bound.window;
        store.dispose(focus);
        store.dispose(focus_visible);
        store.dispose(theme_generation);
        for binding in self.scroll.drain(..) {
            store.dispose(binding.signal());
        }
        self.clear();
    }

    /// Publish the handles row `row`'s recording painted, so the check of
    /// its reads sees where they are now.
    pub(super) fn publish_watched(&mut self, row: usize, store: &SignalStore) {
        for at in 0..self.paint[row].scroll().len() {
            let handle = self.paint[row].scroll()[at].handle().clone();
            self.publish_during_frame(&handle, store);
        }
    }

    /// Publish `handle` in the middle of a frame. A change that a reader
    /// already read past shows only in the next frame, which the frame
    /// asks for as it ends.
    fn publish_during_frame(&mut self, handle: &ScrollHandle, store: &SignalStore) {
        let seen = match handle.publish(store) {
            Publish::Unchanged => false,
            Publish::Changed { seen } => seen,
            Publish::Created { signal, seen } => {
                self.scroll.push(handle.binding(signal));
                seen
            }
        };
        self.frame_due |= seen;
    }
}

/// Run one phase of a boundary's subtree with its reads tracked into row
/// `row`'s snapshot, and seen by the enclosing boundary too. `fresh` starts
/// the snapshot over for a new build, which depends on the theme before
/// anything else: the row cannot replay until its recording is complete
/// again. Without a row, the reads go to the enclosing scope as they are.
pub(super) fn tracked<R>(
    cx: &mut ElementContext,
    row: Option<u32>,
    fresh: bool,
    f: impl FnOnce(&mut ElementContext) -> R,
) -> R {
    let Some((row, cache)) = row.zip(cx.cache.as_deref_mut()) else {
        return f(cx);
    };
    let r = row as usize;
    let mut reads = std::mem::take(&mut cache.signals[r]);
    if fresh {
        reads.clear();
        cache.inputs[r].reusable = false;
    }
    let result = track_into(&mut reads, Nesting::Merge, || {
        if fresh && let Some(window) = cx.window_signals {
            window.theme_generation.track(cx.signal_store);
        }
        f(cx)
    });
    if let Some(cache) = cx.cache.as_deref_mut() {
        cache.signals[r] = reads;
    }
    result
}

/// What row `row`'s recording read, to report to the enclosing scope when
/// it replays.
pub(super) fn row_reads<'c>(cx: &'c ElementContext, row: u32) -> &'c [SignalRead] {
    cx.cache
        .as_deref()
        .map_or(&[], |cache| &cache.signals[row as usize])
}

impl ElementContext<'_> {
    /// Publish `handle` for the cache's boundaries and the readers of its
    /// signal, once its container resolved this frame's offset.
    pub(in crate::element) fn publish_scroll(&mut self, handle: &ScrollHandle) {
        if let Some(cache) = self.cache.as_deref_mut() {
            cache.publish_during_frame(handle, self.signal_store);
        }
    }
}
