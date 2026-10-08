//! Fine-grained reactive signals.
//!
//! `Signal<T>` is a Copy handle (8 bytes) into a persistent `SignalStore`.
//! Values live in a slot arena. Reads are automatically tracked by the current
//! observer scope; writes mark subscribers for lazy recomputation.
//!
//! Memos carry one of three states: `Clean`, `Check`, `Dirty`. A write marks
//! the signal's direct subscribers `Dirty` and every memo further downstream
//! `Check`; raw signals carry no state, so no read can consume a change
//! before the memos see it. Memos recompute lazily on read. A recomputed
//! value equal to the previous one (`PartialEq`) leaves `Check` dependents to
//! settle back to `Clean` without running.

use std::any::Any;
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

// ---------------------------------------------------------------------------
// SignalId — stable arena index + generation for use-after-free detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SignalId {
    index: u32,
    generation: u32,
}

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
}

impl<T: 'static + PartialEq> Signal<T> {
    /// Shorthand for `store.set_if_changed(self, value)`.
    #[inline]
    pub fn set_if_changed(self, store: &SignalStore, value: T) -> bool {
        store.set_if_changed(self, value)
    }
}

// ---------------------------------------------------------------------------
// Observer — thread-local dependency tracker
// ---------------------------------------------------------------------------

struct TrackingScope {
    dependencies: Vec<SignalId>,
}

thread_local! {
    static OBSERVER: RefCell<Option<TrackingScope>> = const { RefCell::new(None) };
}

fn track_read(id: SignalId) {
    OBSERVER.with(|obs| {
        if let Some(scope) = obs.borrow_mut().as_mut()
            && !scope.dependencies.contains(&id)
        {
            scope.dependencies.push(id);
        }
    });
}

/// Restores the enclosing tracking scope when dropped, so a panic inside a
/// tracked closure does not leave the thread recording into a dead scope.
struct ScopeRestore {
    prev: Option<TrackingScope>,
}

impl Drop for ScopeRestore {
    fn drop(&mut self) {
        let prev = self.prev.take();
        OBSERVER.with(|obs| *obs.borrow_mut() = prev);
    }
}

/// Run `f` with dependency tracking enabled. Returns the result plus the list
/// of signal IDs that were read during `f`.
pub fn with_tracking<R>(f: impl FnOnce() -> R) -> (R, Vec<SignalId>) {
    let prev = OBSERVER.with(|obs| {
        obs.borrow_mut().replace(TrackingScope {
            dependencies: Vec::new(),
        })
    });
    let restore = ScopeRestore { prev };
    let result = f();
    let scope = OBSERVER
        .with(|obs| obs.borrow_mut().take())
        .expect("tracking scope disappeared during with_tracking");
    drop(restore);
    (result, scope.dependencies)
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
    /// Set while the memo's compute runs; a read that reaches it is a cycle.
    computing: bool,
    /// Downstream edges: memos that read this node.
    subscribers: Vec<u32>,
    /// Upstream edges (memos only): nodes this memo read on its last run.
    sources: Vec<u32>,
}

impl Node {
    fn reset(&mut self, value: Option<Box<dyn Any>>, memo: Option<Memo>) {
        debug_assert!(self.subscribers.is_empty() && self.sources.is_empty());
        self.state = if memo.is_some() {
            NodeState::Dirty
        } else {
            NodeState::Clean
        };
        self.value = value;
        self.memo = memo;
        self.computing = false;
    }
}

struct Inner {
    nodes: Vec<Node>,
    free_list: Vec<u32>,
    /// Any signal written since the last `clear_dirty()`. The frame loop
    /// uses it to decide whether to rerender.
    any_dirty: bool,
    /// Reused traversal stack for dirty propagation, so writes do not
    /// allocate once the graph has warmed up.
    scratch: Vec<u32>,
    /// Memos currently computing, outermost first. Used for cycle reports.
    compute_stack: Vec<u32>,
}

impl Inner {
    fn live(&self, id: SignalId) -> bool {
        self.nodes.get(id.index as usize).is_some_and(|n| {
            n.generation == id.generation && (n.value.is_some() || n.memo.is_some())
        })
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

// ---------------------------------------------------------------------------
// SignalStore
// ---------------------------------------------------------------------------

/// Persistent store for signal values. Lives in the app, survives across frames.
pub struct SignalStore {
    inner: RefCell<Inner>,
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
        Self {
            inner: RefCell::new(Inner {
                nodes: Vec::new(),
                free_list: Vec::new(),
                any_dirty: false,
                scratch: Vec::new(),
                compute_stack: Vec::new(),
            }),
        }
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
        if let Some(index) = inner.free_list.pop() {
            let node = &mut inner.nodes[index as usize];
            node.reset(value, memo);
            return SignalId {
                index,
                generation: node.generation,
            };
        }
        let index = inner.nodes.len() as u32;
        let mut node = Node {
            value: None,
            generation: 0,
            memo: None,
            state: NodeState::Clean,
            computing: false,
            subscribers: Vec::new(),
            sources: Vec::new(),
        };
        node.reset(value, memo);
        inner.nodes.push(node);
        SignalId {
            index,
            generation: 0,
        }
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
        track_read(signal.id);
        self.with_untracked(signal, f)
    }

    fn with_untracked<T: 'static, R>(&self, signal: Signal<T>, f: impl FnOnce(&T) -> R) -> R {
        {
            let inner = self.inner_ref();
            assert!(
                inner.live(signal.id),
                "stale signal handle (generation mismatch)"
            );
            let node = &inner.nodes[signal.id.index as usize];
            if node.computing {
                panic!("{}", inner.cycle_message(signal.id.index));
            }
        }
        self.realize(signal.id.index as usize);
        let inner = self.inner_ref();
        let value = inner.nodes[signal.id.index as usize]
            .value
            .as_ref()
            .expect("signal slot is empty")
            .downcast_ref::<T>()
            .expect("signal type mismatch");
        f(value)
    }

    /// Replace a signal's value and propagate dirtiness to subscribers.
    pub fn write<T: 'static>(&self, signal: Signal<T>, value: T) {
        self.inner_mut().node_checked(signal.id).value = Some(Box::new(value));
        self.notify(signal.id.index as usize);
    }

    /// Write only if the new value differs from the current one (`PartialEq`).
    /// Returns `true` if the write happened. Use when pushing values that may
    /// be equal frame-to-frame so stable values don't re-dirty subscribers.
    pub fn set_if_changed<T: 'static + PartialEq>(&self, signal: Signal<T>, value: T) -> bool {
        {
            let mut inner = self.inner_mut();
            let node = inner.node_checked(signal.id);
            if let Some(cur) = node.value.as_ref().and_then(|b| b.downcast_ref::<T>())
                && *cur == value
            {
                return false;
            }
            node.value = Some(Box::new(value));
        }
        self.notify(signal.id.index as usize);
        true
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

    /// Dispose a signal, freeing its slot for reuse. Costs O(own edges):
    /// the node detaches from its sources and subscribers only.
    pub fn dispose<T>(&self, signal: Signal<T>) {
        let mut inner = self.inner_mut();
        if !inner.live(signal.id) {
            return;
        }
        let idx = signal.id.index;
        let node = &mut inner.nodes[idx as usize];
        let subscribers = std::mem::take(&mut node.subscribers);
        let sources = std::mem::take(&mut node.sources);
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
        inner.free_list.push(idx);
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
        let inner = self.inner_ref();
        inner.live(signal_id) && inner.nodes[signal_id.index as usize].state != NodeState::Clean
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

    // -----------------------------------------------------------------------
    // Internal: dirty propagation & lazy realization
    // -----------------------------------------------------------------------

    fn notify(&self, idx: usize) {
        let mut inner = self.inner_mut();
        inner.any_dirty = true;
        inner.mark_subscribers(idx);
    }

    /// Bring a memo up to date; raw signals are always current. Walks `Check`
    /// sources with an explicit stack so deep memo chains do not recurse.
    /// A memo whose value changes marks its subscribers `Dirty`, which is
    /// what tells a `Check` parent further up the stack to recompute.
    fn realize(&self, idx: usize) {
        if self.inner_ref().nodes[idx].state == NodeState::Clean {
            return;
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
            inner.mark_subscribers(idx);
        }
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    /// A memo over `f` that counts how often it runs.
    fn counted<T: 'static + Clone + PartialEq>(
        store: &SignalStore,
        f: impl Fn(&SignalStore) -> T + 'static,
    ) -> (Signal<T>, Rc<Cell<u32>>) {
        let runs = Rc::new(Cell::new(0));
        let r = Rc::clone(&runs);
        let memo = store.create_memo(move |s| {
            r.set(r.get() + 1);
            f(s)
        });
        (memo, runs)
    }

    #[test]
    fn dispose_and_reuse_slot() {
        let store = SignalStore::new();
        let sig1 = store.create(100i32);
        let old_index = sig1.id.index;
        let old_gen = sig1.id.generation;
        store.dispose(sig1);
        let sig2 = store.create(200i32);
        assert_eq!(sig2.id.index, old_index);
        assert_ne!(sig2.id.generation, old_gen);
        assert_eq!(store.read(sig2), 200);
    }

    #[test]
    #[should_panic(expected = "stale signal handle")]
    fn stale_handle_panics_on_read() {
        let store = SignalStore::new();
        let sig = store.create(1i32);
        store.dispose(sig);
        let _new = store.create(2i32);
        store.read(sig);
    }

    #[test]
    fn with_tracking_captures_reads() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let b = store.create(2i32);
        let c = store.create(3i32);
        let (sum, deps) = with_tracking(|| store.read(a) + store.read(b));
        assert_eq!(sum, 3);
        assert_eq!(deps.len(), 2);
        assert!(deps.contains(&a.id));
        assert!(deps.contains(&b.id));
        assert!(!deps.contains(&c.id));
    }

    #[test]
    fn nested_tracking_scopes_independent() {
        let store = SignalStore::new();
        let a = store.create(10i32);
        let b = store.create(20i32);
        let (_, outer_deps) = with_tracking(|| {
            store.read(a);
            let (_, inner_deps) = with_tracking(|| {
                store.read(b);
            });
            assert_eq!(inner_deps.len(), 1);
            assert!(inner_deps.contains(&b.id));
        });
        assert_eq!(outer_deps.len(), 1);
        assert!(outer_deps.contains(&a.id));
        assert!(!outer_deps.contains(&b.id));
    }

    #[test]
    fn read_untracked_not_captured() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let b = store.create(2i32);
        let (_, deps) = with_tracking(|| {
            store.read(a);
            store.read_untracked(b);
        });
        assert_eq!(deps.len(), 1);
        assert!(deps.contains(&a.id));
        assert!(!deps.contains(&b.id));
    }

    #[test]
    fn duplicate_reads_deduped() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let (_, deps) = with_tracking(|| {
            store.read(a);
            store.read(a);
            store.read(a);
        });
        assert_eq!(deps.len(), 1);
        assert!(deps.contains(&a.id));
    }

    #[test]
    fn any_dirty_set_by_write_and_reset_by_clear() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        assert!(!store.any_dirty());
        store.write(a, 2);
        assert!(store.any_dirty());
        store.clear_dirty();
        assert!(!store.any_dirty());
    }

    #[test]
    fn memo_recomputes_when_dependency_changes() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let b = store.create(2i32);
        let sum = store.create_memo(move |s| s.read(a) + s.read(b));
        assert_eq!(store.read(sum), 3);
        store.write(a, 10);
        assert_eq!(store.read(sum), 12);
    }

    #[test]
    fn memo_tracks_new_dependencies() {
        let store = SignalStore::new();
        let flag = store.create(true);
        let a = store.create(100i32);
        let b = store.create(200i32);
        let val = store.create_memo(move |s| if s.read(flag) { s.read(a) } else { s.read(b) });
        assert_eq!(store.read(val), 100);
        store.write(flag, false);
        assert_eq!(store.read(val), 200);
        store.write(b, 999);
        assert_eq!(store.read(val), 999);
    }

    #[test]
    fn chained_memos() {
        let store = SignalStore::new();
        let base = store.create(2i32);
        let doubled = store.create_memo(move |s| s.read(base) * 2);
        let quadrupled = store.create_memo(move |s| s.read(doubled) * 2);
        assert_eq!(store.read(quadrupled), 8);
        store.write(base, 3);
        assert_eq!(store.read(doubled), 6);
        assert_eq!(store.read(quadrupled), 12);
    }

    /// Regression: reading the written signal, or an intermediate memo, before
    /// the downstream memo used to consume the change and leave it stale.
    #[test]
    fn earlier_reads_do_not_hide_a_change_from_memos() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let m = store.create_memo(move |s| s.read(a) + 1);
        let m2 = store.create_memo(move |s| s.read(m) * 10);
        assert_eq!(store.read(m2), 20);

        store.write(a, 2);
        assert_eq!(store.read(a), 2);
        assert_eq!(store.read(m), 3);
        assert_eq!(store.read(m2), 30);

        store.write(a, 5);
        assert_eq!(store.read(m), 6);
        assert_eq!(store.read(m2), 60);
    }

    #[test]
    fn diamond_recomputes_the_join_once() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let left = store.create_memo(move |s| s.read(a) + 1);
        let right = store.create_memo(move |s| s.read(a) * 2);
        let (join, runs) = counted(&store, move |s| s.read(left) + s.read(right));
        assert_eq!(store.read(join), 4);
        store.write(a, 3);
        assert_eq!(store.read(join), 10);
        assert_eq!(runs.get(), 2);
    }

    #[test]
    fn memo_does_not_recompute_when_source_returns_same_value() {
        let store = SignalStore::new();
        let a = store.create(5i32);
        let sign = store.create_memo(move |s| s.read(a).signum());
        let (dependent, runs) = counted(&store, move |s| s.read(sign) * 10);
        assert_eq!(store.read(dependent), 10);

        store.write(a, 7);
        assert_eq!(store.read(dependent), 10);
        assert_eq!(
            runs.get(),
            1,
            "dependent re-ran though its source stayed equal"
        );

        store.write(a, -3);
        assert_eq!(store.read(dependent), -10);
        assert_eq!(runs.get(), 2);
    }

    #[test]
    fn dispose_memo_does_not_leak() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let m = store.create_memo(move |s| s.read(a) + 1);
        assert_eq!(store.read(m), 2);
        store.dispose(m);
        store.write(a, 10); // must not panic
    }

    /// Disposing a source detaches its memos: a new signal that reuses the
    /// slot must not dirty them into recomputing against the dead handle.
    #[test]
    fn disposed_source_does_not_wake_its_memo_through_slot_reuse() {
        let store = SignalStore::new();
        let a = store.create(1i32);
        let m = store.create_memo(move |s| s.read(a) + 1);
        assert_eq!(store.read(m), 2);
        store.dispose(a);
        let b = store.create(7i32);
        assert_eq!(b.id.index, a.id.index);
        store.write(b, 8);
        assert_eq!(store.read(m), 2);
    }

    #[test]
    #[should_panic(expected = "memo cycle: memo #1 read itself while computing (#1 -> #2 -> #1)")]
    fn memo_cycle_reports_the_path() {
        let store = SignalStore::new();
        let slot: Rc<Cell<Option<Signal<i32>>>> = Rc::new(Cell::new(None));
        let _pad = store.create(0i32);
        let s2 = Rc::clone(&slot);
        let a = store.create_memo(move |s| s.read(s2.get().unwrap()) + 1);
        let b = store.create_memo(move |s| s.read(a) + 1);
        slot.set(Some(b));
        store.read(a);
    }

    #[test]
    fn memo_recovers_after_its_compute_panics() {
        let store = SignalStore::new();
        let fail = store.create(true);
        let m = store.create_memo(move |s| {
            assert!(!s.read(fail), "compute failed");
            1i32
        });
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| store.read(m)));
        assert!(r.is_err());
        store.write(fail, false);
        let (v, deps) = with_tracking(|| store.read(m));
        assert_eq!(v, 1);
        assert_eq!(deps, vec![m.id], "tracking scope leaked from the panic");
    }
}
