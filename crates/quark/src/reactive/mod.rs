//! Fine-grained reactive signals.
//!
//! `Signal<T>` is a Copy handle (8 bytes) into a persistent `SignalStore`.
//! Values live in a slot arena. Reads are automatically tracked by the
//! current observer scope ([`with_tracking`], [`track_into`]); writes mark
//! subscribers for lazy recomputation.
//!
//! Memos carry one of three states: `Clean`, `Check`, `Dirty`. A write marks
//! the signal's direct subscribers `Dirty` and every memo further downstream
//! `Check`; raw signals carry no state, so no read can consume a change
//! before the memos see it. Memos recompute lazily on read. A recomputed
//! value equal to the previous one (`PartialEq`) leaves `Check` dependents to
//! settle back to `Clean` without running.
//!
//! Every slot also carries a version, stamped from the store's clock
//! whenever its value changes: a write, or a memo recompute whose value
//! differs. An observer that is not a memo (a cache boundary) keeps the
//! `(SignalId, version)` pairs it read and asks
//! [`SignalStore::any_changed`] before reusing its output.

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};

mod integrity;
#[cfg(test)]
mod tests;
mod tracking;

pub use integrity::ReactiveIntegrityError;
pub use tracking::{
    Nesting, SignalRead, is_tracking, track_into, track_read_at, track_reads, track_unpublished,
    with_tracking,
};

// ---------------------------------------------------------------------------
// SignalId — stable arena index + generation for use-after-free detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignalId {
    index: u32,
    generation: u32,
}

impl SignalId {
    /// No signal's id: a read of it is always out of date.
    const UNPUBLISHED: Self = Self {
        index: u32::MAX,
        generation: u32::MAX,
    };
}

/// Identity of a [`SignalStore`], so a holder of signal ids can tell
/// whether they belong to the store it is handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StoreId(u64);

// ---------------------------------------------------------------------------
// Signal<T> — Copy handle into the store
// ---------------------------------------------------------------------------

pub struct Signal<T> {
    pub(crate) id: SignalId,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for Signal<T> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<T> Copy for Signal<T> {}

impl<T> PartialEq for Signal<T> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}
impl<T> Eq for Signal<T> {}

impl<T> std::fmt::Debug for Signal<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Signal")
            .field("index", &self.id.index)
            .field("generation", &self.id.generation)
            .finish()
    }
}

impl<T> Signal<T> {
    /// The untyped id, for snapshots of reads.
    pub fn id(self) -> SignalId {
        self.id
    }
}

// ---------------------------------------------------------------------------
// Signal<T> ergonomic methods — shorter left-to-right calls.
// `sig.get(&store)` instead of `store.read(sig)`.
// ---------------------------------------------------------------------------

impl<T: 'static + Clone> Signal<T> {
    /// Shorthand for `store.read(self)`. Tracked by the current observer.
    #[inline]
    pub fn get(self, store: &SignalStore) -> T {
        store.read(self)
    }

    /// Shorthand for `store.read_untracked(self)`.
    #[inline]
    pub fn get_untracked(self, store: &SignalStore) -> T {
        store.read_untracked(self)
    }
}

impl<T: 'static> Signal<T> {
    /// Shorthand for `store.write(self, value)`.
    #[inline]
    pub fn set(self, store: &SignalStore, value: T) {
        store.write(self, value);
    }

    /// Shorthand for `store.update(self, f)`.
    #[inline]
    pub fn update(self, store: &SignalStore, f: impl FnOnce(&mut T)) {
        store.update(self, f);
    }

    /// Shorthand for `store.with(self, f)`.
    #[inline]
    pub fn with<R>(self, store: &SignalStore, f: impl FnOnce(&T) -> R) -> R {
        store.with(self, f)
    }

    /// Shorthand for `store.track(self)`.
    #[inline]
    pub fn track(self, store: &SignalStore) {
        store.track(self);
    }
}

impl<T: 'static + PartialEq> Signal<T> {
    /// Shorthand for `store.set_if_changed(self, value)`.
    #[inline]
    pub fn set_if_changed(self, store: &SignalStore, value: T) -> bool {
        store.set_if_changed(self, value)
    }
}

// ---------------------------------------------------------------------------
// Internal state
// ---------------------------------------------------------------------------

/// Freshness of a memo. Raw signals are always `Clean`: a write pushes the
/// change into the subscribers' state instead of keeping it on the source,
/// so no read of the source can consume it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NodeState {
    Clean,
    /// A transitive source changed; a direct source may or may not have.
    Check,
    /// A direct source changed (or the memo never ran): must recompute.
    Dirty,
}

type ComputeFn = Rc<dyn Fn(&SignalStore) -> Box<dyn Any>>;

struct Memo {
    compute: ComputeFn,
    /// `PartialEq` on the erased values; equal results stop propagation.
    eq: fn(&dyn Any, &dyn Any) -> bool,
}

struct Node {
    value: Option<Box<dyn Any>>,
    generation: u32,
    memo: Option<Memo>,
    state: NodeState,
    /// The store clock when the value last changed.
    version: u64,
    /// Written inside a batch; its subscribers are marked when the batch
    /// ends (or a memo is read first).
    pending: bool,
    /// Set while the memo's compute runs; a read that reaches it is a cycle.
    computing: bool,
    /// Downstream edges: memos that read this node.
    subscribers: Vec<u32>,
    /// Upstream edges (memos only): nodes this memo read on its last run.
    sources: Vec<u32>,
}

impl Node {
    fn reset(&mut self, value: Option<Box<dyn Any>>, memo: Option<Memo>, version: u64) {
        debug_assert!(self.subscribers.is_empty() && self.sources.is_empty());
        self.state = if memo.is_some() {
            NodeState::Dirty
        } else {
            NodeState::Clean
        };
        self.value = value;
        self.memo = memo;
        self.version = version;
        self.pending = false;
        self.computing = false;
    }

    fn is_live(&self) -> bool {
        self.value.is_some() || self.memo.is_some()
    }
}

struct Inner {
    nodes: Vec<Node>,
    free_list: Vec<u32>,
    /// Any signal written since the last `clear_dirty()`. The frame loop
    /// uses it to decide whether to rerender.
    any_dirty: bool,
    /// Bumped by every value change; stamps the changed slot's version.
    clock: u64,
    /// Open [`SignalStore::batch`] calls.
    batch_depth: u32,
    /// Signals written in the open batch whose subscribers are not marked
    /// yet, each once.
    pending: Vec<u32>,
    /// Reused traversal stack for dirty propagation, so writes do not
    /// allocate once the graph has warmed up.
    scratch: Vec<u32>,
    /// Memos currently computing, outermost first. Used for cycle reports.
    compute_stack: Vec<u32>,
}

impl Inner {
    fn live(&self, id: SignalId) -> bool {
        self.nodes
            .get(id.index as usize)
            .is_some_and(|n| n.generation == id.generation && n.is_live())
    }

    fn node_checked(&mut self, id: SignalId) -> &mut Node {
        assert!(self.live(id), "stale signal handle (generation mismatch)");
        &mut self.nodes[id.index as usize]
    }

    /// Mark `idx`'s direct subscribers `Dirty` and every node further
    /// downstream `Check`. A node that is already `Check` or `Dirty` has all
    /// of its downstream at least `Check`, so the walk stops there; node
    /// state doubles as the visited mark.
    fn mark_subscribers(&mut self, idx: usize) {
        let Inner { nodes, scratch, .. } = self;
        scratch.clear();
        for i in 0..nodes[idx].subscribers.len() {
            let sub = nodes[idx].subscribers[i] as usize;
            let was = std::mem::replace(&mut nodes[sub].state, NodeState::Dirty);
            if was == NodeState::Clean {
                scratch.extend_from_slice(&nodes[sub].subscribers);
            }
        }
        while let Some(n) = scratch.pop() {
            let node = &mut nodes[n as usize];
            if node.state == NodeState::Clean {
                node.state = NodeState::Check;
                scratch.extend_from_slice(&node.subscribers);
            }
        }
    }

    /// Mark the subscribers of every signal the open batch wrote.
    fn flush_pending(&mut self) {
        let mut pending = std::mem::take(&mut self.pending);
        for &idx in &pending {
            self.nodes[idx as usize].pending = false;
            self.mark_subscribers(idx as usize);
        }
        pending.clear();
        self.pending = pending;
    }

    /// Stamp `idx` as changed now.
    fn stamp(&mut self, idx: usize) {
        self.clock += 1;
        self.nodes[idx].version = self.clock;
    }

    fn rewire_sources(&mut self, idx: u32, deps: Vec<SignalId>) {
        for src in std::mem::take(&mut self.nodes[idx as usize].sources) {
            self.nodes[src as usize].subscribers.retain(|&s| s != idx);
        }
        let mut sources = Vec::with_capacity(deps.len());
        for dep in deps {
            // A dependency disposed during the compute has no node to subscribe to.
            if !self.live(dep) || sources.contains(&dep.index) {
                continue;
            }
            sources.push(dep.index);
            self.nodes[dep.index as usize].subscribers.push(idx);
        }
        self.nodes[idx as usize].sources = sources;
    }

    fn cycle_message(&self, idx: u32) -> String {
        let start = self
            .compute_stack
            .iter()
            .position(|&i| i == idx)
            .unwrap_or(0);
        let path: Vec<String> = self.compute_stack[start..]
            .iter()
            .chain(std::iter::once(&idx))
            .map(|i| format!("#{i}"))
            .collect();
        format!(
            "memo cycle: memo #{idx} read itself while computing ({})",
            path.join(" -> ")
        )
    }

    /// Check what a mutation of `idx` touched, or the whole store under
    /// the `integrity-checks` feature. Debug builds only (skipped under
    /// Kani, whose `#[kani::unwind(2)]` slot-reuse proof otherwise explodes
    /// on the edge-scanning loops inside `verify_node`).
    #[cfg(not(kani))]
    fn debug_check(&self, idx: usize) {
        if crate::selection::FULL_INTEGRITY_CHECKS {
            debug_assert_eq!(self.verify(), Ok(()));
        } else {
            debug_assert_eq!(self.verify_node(idx), Ok(()));
        }
    }

    #[cfg(kani)]
    fn debug_check(&self, _idx: usize) {}
}

/// Clears the computing mark when a compute finishes or unwinds.
struct ComputeGuard<'a> {
    store: &'a SignalStore,
    id: SignalId,
}

impl Drop for ComputeGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.store.inner.try_borrow_mut() {
            inner.compute_stack.pop();
            if inner.live(self.id) {
                inner.nodes[self.id.index as usize].computing = false;
            }
        }
    }
}

/// Ends a batch when its closure returns or unwinds.
struct BatchGuard<'a> {
    store: &'a SignalStore,
}

impl Drop for BatchGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.store.inner.try_borrow_mut() {
            inner.batch_depth -= 1;
            if inner.batch_depth == 0 {
                inner.flush_pending();
                if crate::selection::FULL_INTEGRITY_CHECKS {
                    debug_assert_eq!(inner.verify(), Ok(()));
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// SignalStore
// ---------------------------------------------------------------------------

/// Persistent store for signal values. Lives in the app, survives across frames.
pub struct SignalStore {
    inner: RefCell<Inner>,
    id: StoreId,
}

impl std::fmt::Debug for SignalStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.inner.try_borrow() {
            Ok(inner) => f
                .debug_struct("SignalStore")
                .field("len", &(inner.nodes.len() - inner.free_list.len()))
                .field("any_dirty", &inner.any_dirty)
                .finish(),
            Err(_) => f.write_str("SignalStore { <borrowed> }"),
        }
    }
}

impl SignalStore {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self {
            inner: RefCell::new(Inner {
                nodes: Vec::new(),
                free_list: Vec::new(),
                any_dirty: false,
                clock: 0,
                batch_depth: 0,
                pending: Vec::new(),
                scratch: Vec::new(),
                compute_stack: Vec::new(),
            }),
            id: StoreId(NEXT_ID.fetch_add(1, Ordering::Relaxed)),
        }
    }

    /// This store's identity, unique in the process.
    pub fn id(&self) -> StoreId {
        self.id
    }

    /// Shared-borrow helper. Panics with a clear message on re-entrancy
    /// (e.g. attempting a write from inside a read or vice versa).
    fn inner_ref(&self) -> std::cell::Ref<'_, Inner> {
        self.inner
            .try_borrow()
            .expect("signal store re-entrancy: tried to read while a write is in progress")
    }

    /// Exclusive-borrow helper. Panics on re-entrancy.
    fn inner_mut(&self) -> std::cell::RefMut<'_, Inner> {
        self.inner
            .try_borrow_mut()
            .expect("signal store re-entrancy: tried to write while another access is in progress")
    }

    fn alloc(&self, value: Option<Box<dyn Any>>, memo: Option<Memo>) -> SignalId {
        let mut inner = self.inner_mut();
        let version = inner.clock;
        let id = if let Some(index) = inner.free_list.pop() {
            let node = &mut inner.nodes[index as usize];
            node.reset(value, memo, version);
            SignalId {
                index,
                generation: node.generation,
            }
        } else {
            let index = inner.nodes.len() as u32;
            let mut node = Node {
                value: None,
                generation: 0,
                memo: None,
                state: NodeState::Clean,
                version: 0,
                pending: false,
                computing: false,
                subscribers: Vec::new(),
                sources: Vec::new(),
            };
            node.reset(value, memo, version);
            inner.nodes.push(node);
            SignalId {
                index,
                generation: 0,
            }
        };
        inner.debug_check(id.index as usize);
        id
    }

    /// Create a new signal with the given initial value.
    pub fn create<T: 'static>(&self, value: T) -> Signal<T> {
        Signal {
            id: self.alloc(Some(Box::new(value)), None),
            _marker: PhantomData,
        }
    }

    /// Create a derived signal (memo) whose value is computed from other signals.
    /// The compute function runs lazily on first read. Propagation to dependents
    /// is skipped when the recomputed value equals the previous one (`PartialEq`).
    pub fn create_memo<T: 'static + Clone + PartialEq>(
        &self,
        compute: impl Fn(&SignalStore) -> T + 'static,
    ) -> Signal<T> {
        fn eq_erased<T: 'static + PartialEq>(a: &dyn Any, b: &dyn Any) -> bool {
            a.downcast_ref::<T>() == b.downcast_ref::<T>()
        }
        let memo = Memo {
            compute: Rc::new(move |store| Box::new(compute(store)) as Box<dyn Any>),
            eq: eq_erased::<T>,
        };
        Signal {
            id: self.alloc(None, Some(memo)),
            _marker: PhantomData,
        }
    }

    /// Read a signal's value (clones it). Registers this signal with the
    /// current tracking scope, if one exists.
    pub fn read<T: 'static + Clone>(&self, signal: Signal<T>) -> T {
        self.with(signal, Clone::clone)
    }

    /// Read a signal's value without registering a dependency.
    pub fn read_untracked<T: 'static + Clone>(&self, signal: Signal<T>) -> T {
        self.with_untracked(signal, Clone::clone)
    }

    /// Access a signal's value by reference. Registers this signal with the
    /// current tracking scope, if one exists.
    pub fn with<T: 'static, R>(&self, signal: Signal<T>, f: impl FnOnce(&T) -> R) -> R {
        self.with_value(signal, true, f)
    }

    fn with_untracked<T: 'static, R>(&self, signal: Signal<T>, f: impl FnOnce(&T) -> R) -> R {
        self.with_value(signal, false, f)
    }

    /// Register a read of `signal` with the current tracking scope without
    /// reading its value: what a reader does that holds the value already.
    pub fn track<T>(&self, signal: Signal<T>) {
        if !is_tracking() {
            return;
        }
        let version = self.version(signal.id).expect("stale signal handle");
        tracking::track_read(signal.id, version);
    }

    fn with_value<T: 'static, R>(
        &self,
        signal: Signal<T>,
        tracked: bool,
        f: impl FnOnce(&T) -> R,
    ) -> R {
        self.check_readable(signal.id);
        self.realize(signal.id.index as usize);
        let inner = self.inner_ref();
        let node = &inner.nodes[signal.id.index as usize];
        if tracked {
            tracking::track_read(signal.id, node.version);
        }
        let value = node
            .value
            .as_ref()
            .expect("signal slot is empty")
            .downcast_ref::<T>()
            .expect("signal type mismatch");
        f(value)
    }

    /// Panics on a stale handle or a memo reading itself.
    fn check_readable(&self, id: SignalId) {
        let inner = self.inner_ref();
        assert!(inner.live(id), "stale signal handle (generation mismatch)");
        if inner.nodes[id.index as usize].computing {
            panic!("{}", inner.cycle_message(id.index));
        }
    }

    /// Replace a signal's value and propagate dirtiness to subscribers.
    pub fn write<T: 'static>(&self, signal: Signal<T>, value: T) {
        self.inner_mut().node_checked(signal.id).value = Some(Box::new(value));
        self.notify(signal.id.index as usize);
    }

    /// Write only if the new value differs from the current one (`PartialEq`).
    /// Returns `true` if the write happened. Use when pushing values that may
    /// be equal frame-to-frame so stable values don't re-dirty subscribers.
    /// A changed value is assigned in place, so pushing a value every frame
    /// allocates nothing.
    pub fn set_if_changed<T: 'static + PartialEq>(&self, signal: Signal<T>, value: T) -> bool {
        {
            let mut inner = self.inner_mut();
            let node = inner.node_checked(signal.id);
            match node.value.as_mut().and_then(|b| b.downcast_mut::<T>()) {
                Some(cur) if *cur == value => return false,
                Some(cur) => *cur = value,
                None => node.value = Some(Box::new(value)),
            }
        }
        self.notify(signal.id.index as usize);
        true
    }

    /// Whether `signal` still names a value: false once it was disposed,
    /// even if its slot was reused.
    pub fn is_live<T>(&self, signal: Signal<T>) -> bool {
        self.inner_ref().live(signal.id)
    }

    /// Mutate a signal's value in place and propagate dirtiness to subscribers.
    pub fn update<T: 'static>(&self, signal: Signal<T>, f: impl FnOnce(&mut T)) {
        {
            let mut inner = self.inner_mut();
            let value = inner
                .node_checked(signal.id)
                .value
                .as_mut()
                .expect("signal slot is empty")
                .downcast_mut::<T>()
                .expect("signal type mismatch");
            f(value);
        }
        self.notify(signal.id.index as usize);
    }

    /// Run `f` with propagation held back: writes in it update values and
    /// versions at once, and their subscribers are marked once, when the
    /// outermost batch ends. A memo read inside the batch first applies
    /// the writes made so far, so it never sees a half-applied batch.
    pub fn batch<R>(&self, f: impl FnOnce() -> R) -> R {
        self.inner_mut().batch_depth += 1;
        let _end = BatchGuard { store: self };
        f()
    }

    /// Dispose a signal, freeing its slot for reuse. Costs O(own edges):
    /// the node detaches from its sources and subscribers only.
    pub fn dispose<T>(&self, signal: Signal<T>) {
        self.dispose_id(signal.id);
    }

    /// [`Self::dispose`] by id. A stale or unknown id is ignored.
    pub fn dispose_id(&self, id: SignalId) {
        let mut inner = self.inner_mut();
        if !inner.live(id) {
            return;
        }
        let idx = id.index;
        let node = &mut inner.nodes[idx as usize];
        let subscribers = std::mem::take(&mut node.subscribers);
        let sources = std::mem::take(&mut node.sources);
        let pending = std::mem::take(&mut node.pending);
        node.value = None;
        node.memo = None;
        node.state = NodeState::Clean;
        node.computing = false;
        node.generation = node.generation.wrapping_add(1);
        for src in sources {
            inner.nodes[src as usize].subscribers.retain(|&s| s != idx);
        }
        for sub in subscribers {
            inner.nodes[sub as usize].sources.retain(|&s| s != idx);
        }
        if pending {
            inner.pending.retain(|&p| p != idx);
        }
        inner.free_list.push(idx);
        inner.debug_check(idx as usize);
    }

    /// Notify `signal_id`'s subscribers as if it had been written. A stale
    /// or unknown id is ignored.
    pub fn mark_dirty(&self, signal_id: SignalId) {
        if self.inner_ref().live(signal_id) {
            self.notify(signal_id.index as usize);
        }
    }

    /// True when `signal_id` is a memo whose cached value may be stale.
    /// Raw signals are never dirty; a stale or unknown id is not dirty.
    pub fn is_dirty(&self, signal_id: SignalId) -> bool {
        if !self.inner_ref().pending.is_empty() {
            self.inner_mut().flush_pending();
        }
        let inner = self.inner_ref();
        inner.live(signal_id) && inner.nodes[signal_id.index as usize].state != NodeState::Clean
    }

    /// The version of `id`'s value: it changes whenever the value does
    /// and never goes back. Brings a memo up to date first. `None` for a
    /// stale or unknown id.
    pub fn version(&self, id: SignalId) -> Option<u64> {
        if !self.inner_ref().live(id) {
            return None;
        }
        self.realize(id.index as usize);
        Some(self.inner_ref().nodes[id.index as usize].version)
    }

    /// Whether any read in `snapshot` is out of date: its signal changed
    /// since the version recorded, or no longer exists. Checks in order and
    /// stops at the first change, so a memo read only behind a condition
    /// that changed is not brought up to date for nothing. O(reads) and
    /// allocation-free when no memo has to recompute.
    pub fn any_changed(&self, snapshot: &[SignalRead]) -> bool {
        snapshot
            .iter()
            .any(|&(id, version)| self.version(id) != Some(version))
    }

    /// Returns true if any signal has been written since the last `clear_dirty()`.
    /// Used by the frame loop to decide whether to rerender.
    pub fn any_dirty(&self) -> bool {
        self.inner_ref().any_dirty
    }

    pub fn clear_dirty(&self) {
        self.inner_mut().any_dirty = false;
    }

    /// Returns true if the given signal is a live memo.
    pub fn is_memo(&self, signal_id: SignalId) -> bool {
        let inner = self.inner_ref();
        inner.live(signal_id) && inner.nodes[signal_id.index as usize].memo.is_some()
    }

    /// Number of live signals.
    pub fn len(&self) -> usize {
        let inner = self.inner_ref();
        inner.nodes.len() - inner.free_list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Check the store's invariants: the free list and live slots partition
    /// the arena, every edge is recorded at both ends and joins a memo to a
    /// live node, a memo whose source is stale is stale itself, versions
    /// never run ahead of the clock, and only an open batch holds writes
    /// back. Mutations call it through `debug_assert!` (the whole store
    /// under the `integrity-checks` feature, the touched node otherwise),
    /// so release builds skip it. O(slots + edges).
    pub fn verify_integrity(&self) -> Result<(), ReactiveIntegrityError> {
        self.inner_ref().verify()
    }

    // -----------------------------------------------------------------------
    // Internal: dirty propagation & lazy realization
    // -----------------------------------------------------------------------

    fn notify(&self, idx: usize) {
        let mut inner = self.inner_mut();
        inner.any_dirty = true;
        inner.stamp(idx);
        if inner.batch_depth == 0 {
            inner.mark_subscribers(idx);
        } else if !inner.nodes[idx].pending {
            inner.nodes[idx].pending = true;
            inner.pending.push(idx as u32);
        }
        inner.debug_check(idx);
    }

    /// Bring a memo up to date; raw signals are always current. Walks `Check`
    /// sources with an explicit stack so deep memo chains do not recurse.
    /// A memo whose value changes marks its subscribers `Dirty`, which is
    /// what tells a `Check` parent further up the stack to recompute.
    fn realize(&self, idx: usize) {
        // Shared borrows while nothing needs doing: a reader inside a
        // `with` closure may read another current signal.
        let held_back = {
            let inner = self.inner_ref();
            let node = &inner.nodes[idx];
            if node.memo.is_none() {
                return;
            }
            let held_back = !inner.pending.is_empty();
            if !held_back && node.state == NodeState::Clean {
                return;
            }
            held_back
        };
        if held_back {
            // A batch's held-back writes reach the memos before one is read.
            let mut inner = self.inner_mut();
            inner.flush_pending();
            if inner.nodes[idx].state == NodeState::Clean {
                return;
            }
        }
        // (node, index of the next source to check)
        let mut stack: Vec<(usize, usize)> = vec![(idx, 0)];
        while let Some(&(node, pos)) = stack.last() {
            let next = {
                let mut inner = self.inner_mut();
                match inner.nodes[node].state {
                    NodeState::Clean => None,
                    NodeState::Dirty => Some(None),
                    NodeState::Check => match inner.nodes[node].sources.get(pos) {
                        Some(&src) => Some(Some(src as usize)),
                        None => {
                            // No source changed: the cached value stands.
                            inner.nodes[node].state = NodeState::Clean;
                            None
                        }
                    },
                }
            };
            match next {
                None => {
                    stack.pop();
                }
                Some(None) => {
                    stack.pop();
                    self.recompute(node);
                }
                Some(Some(src)) => {
                    stack.last_mut().expect("non-empty").1 += 1;
                    let inner = self.inner_ref();
                    let src_node = &inner.nodes[src];
                    if src_node.computing {
                        panic!("{}", inner.cycle_message(src as u32));
                    }
                    if src_node.state != NodeState::Clean {
                        stack.push((src, 0));
                    }
                }
            }
        }
    }

    /// Run a `Dirty` memo's compute, rewire its sources, and mark its
    /// subscribers `Dirty` when the value changed. If the compute panics the
    /// memo stays `Dirty` with its old value, so the next read retries.
    fn recompute(&self, idx: usize) {
        let (id, compute) = {
            let mut inner = self.inner_mut();
            if inner.nodes[idx].computing {
                let msg = inner.cycle_message(idx as u32);
                drop(inner);
                panic!("{msg}");
            }
            inner.compute_stack.push(idx as u32);
            let node = &mut inner.nodes[idx];
            node.computing = true;
            let compute = Rc::clone(&node.memo.as_ref().expect("recompute on non-memo").compute);
            let id = SignalId {
                index: idx as u32,
                generation: node.generation,
            };
            (id, compute)
        };
        let guard = ComputeGuard { store: self, id };
        let (new_value, deps) = with_tracking(|| compute(self));
        drop(guard);

        let mut inner = self.inner_mut();
        if !inner.live(id) {
            return; // disposed by its own compute
        }
        let node = &mut inner.nodes[idx];
        let eq = node.memo.as_ref().expect("memo").eq;
        let changed = node
            .value
            .as_deref()
            .is_none_or(|old| !eq(old, &*new_value));
        node.value = Some(new_value);
        node.state = NodeState::Clean;
        inner.rewire_sources(id.index, deps);
        if changed {
            inner.stamp(idx);
            inner.mark_subscribers(idx);
        }
        inner.debug_check(idx);
    }
}

impl Default for SignalStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    const OPS: usize = 3;

    /// Every handle made so far, and whether it is still live.
    struct Model {
        made: [Option<Signal<u8>>; OPS],
        alive: [bool; OPS],
    }

    /// Step `step`: create a signal, or dispose any handle made so far,
    /// live or already disposed.
    fn any_op(store: &SignalStore, model: &mut Model, step: usize) {
        if kani::any() {
            model.made[step] = Some(store.create(step as u8));
            model.alive[step] = true;
        } else {
            let j: usize = kani::any();
            kani::assume(j < step);
            if let Some(signal) = model.made[j] {
                store.dispose(signal);
                model.alive[j] = false;
            }
        }
    }

    /// Handle `j` resolves exactly while it is live. Values are not read:
    /// with the downcasts as well the proof ran the runner out of memory,
    /// and a slot handed to two handles already shows up as a stale
    /// handle resolving or as a wrong count.
    fn check(store: &SignalStore, model: &Model, j: usize) -> usize {
        let Some(signal) = model.made[j] else {
            return 0;
        };
        assert!(store.inner_ref().live(signal.id) == model.alive[j]);
        usize::from(model.alive[j])
    }

    /// Any three creates and disposes: a live handle resolves, a disposed
    /// one never resolves again, even once a later signal reuses its slot,
    /// and the store counts exactly the live handles. Three steps reach a
    /// double dispose and a create into a disposed slot; at four the CI
    /// runner ran out of memory. Generation wraparound after 2^32 reuses
    /// of one slot is beyond this bound. The steps are unrolled by hand so
    /// the unwind bound stays at 2: at 7 the symbolic execution alone took
    /// 25 minutes.
    #[kani::proof]
    #[kani::unwind(2)]
    #[kani::solver(kissat)]
    fn reused_slots_never_alias_a_handle() {
        let store = SignalStore::new();
        let mut model = Model {
            made: [None; OPS],
            alive: [false; OPS],
        };
        any_op(&store, &mut model, 0);
        any_op(&store, &mut model, 1);
        any_op(&store, &mut model, 2);
        let live = check(&store, &model, 0) + check(&store, &model, 1) + check(&store, &model, 2);
        assert!(store.len() == live);
        // Skip the drop glue of the boxed values; it only adds to the
        // formula.
        std::mem::forget(store);
    }
}
