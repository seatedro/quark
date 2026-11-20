use std::cell::Cell;

use proptest::prelude::*;

use super::*;

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

// ---------------------------------------------------------------------------
// Nested scopes and batches
// ---------------------------------------------------------------------------

fn ids(reads: &[SignalRead]) -> Vec<SignalId> {
    reads.iter().map(|&(id, _)| id).collect()
}

#[test]
fn a_merged_scope_keeps_its_reads_and_hands_them_to_the_enclosing_one() {
    let store = SignalStore::new();
    let outer_only = store.create(1i32);
    let inner_only = store.create(2i32);
    let (mut outer, mut inner) = (Vec::new(), Vec::new());
    track_into(&mut outer, Nesting::Isolate, || {
        store.read(outer_only);
        track_into(&mut inner, Nesting::Merge, || store.read(inner_only));
    });
    assert_eq!(ids(&inner), [inner_only.id]);
    assert_eq!(ids(&outer), [outer_only.id, inner_only.id]);
}

#[test]
fn a_memo_read_inside_a_batch_sees_the_writes_before_it() {
    let store = SignalStore::new();
    let a = store.create(1i32);
    let b = store.create(2i32);
    let sum = store.create_memo(move |s| s.read(a) + s.read(b));
    assert_eq!(store.read(sum), 3);
    let seen = store.batch(|| {
        store.write(a, 10);
        let seen = store.read(sum);
        store.write(b, 20);
        seen
    });
    assert_eq!(seen, 12);
    assert_eq!(store.read(sum), 30);
}

// ---------------------------------------------------------------------------
// Properties over a small graph
// ---------------------------------------------------------------------------

const RAW: usize = 4;
const MEMOS: usize = 3;

/// Four raw signals and three memos over them: `sum = s0 + s1`,
/// `product = sum * s2` (through `sum`), and `pick`, which reads `sum`
/// while `s3 > 0` and `s1` otherwise, so the dependency comes and goes.
struct Graph {
    raw: [Signal<i32>; RAW],
    memos: [Signal<i32>; MEMOS],
}

impl Graph {
    fn new(store: &SignalStore, init: [i32; RAW]) -> Self {
        let raw = init.map(|v| store.create(v));
        let [s0, s1, s2, s3] = raw;
        let sum = store.create_memo(move |s| s.read(s0) + s.read(s1));
        let product = store.create_memo(move |s| s.read(sum) * s.read(s2));
        let pick = store.create_memo(move |s| {
            if s.read(s3) > 0 {
                s.read(sum)
            } else {
                s.read(s1)
            }
        });
        Self {
            raw,
            memos: [sum, product, pick],
        }
    }
}

/// What memo `memo` should hold over raw values `v`.
fn expected(v: [i32; RAW], memo: usize) -> i32 {
    let sum = v[0] + v[1];
    match memo {
        0 => sum,
        1 => sum * v[2],
        _ if v[3] > 0 => sum,
        _ => v[1],
    }
}

#[derive(Debug, Clone)]
enum Op {
    Write(usize, i32),
    SetIfChanged(usize, i32),
    /// Add to the value in place.
    Update(usize, i32),
    ReadMemo(usize),
    Batch(Vec<Op>),
}

fn write_op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0..RAW, -3..3i32).prop_map(|(i, v)| Op::Write(i, v)),
        (0..RAW, -3..3i32).prop_map(|(i, v)| Op::SetIfChanged(i, v)),
        (0..RAW, -2..2i32).prop_map(|(i, v)| Op::Update(i, v)),
    ]
}

fn op() -> impl Strategy<Value = Op> {
    let leaf = prop_oneof![write_op(), (0..MEMOS).prop_map(Op::ReadMemo)];
    leaf.prop_recursive(2, 24, 6, |inner| {
        prop::collection::vec(inner, 0..6).prop_map(Op::Batch)
    })
}

/// Apply `op` to the store and to the model `values`, recording in
/// `written` the raw signals a write changed the version of. Checks every
/// memo read against the model and the store's integrity after each op.
fn apply(
    store: &SignalStore,
    graph: &Graph,
    values: &mut [i32; RAW],
    written: &mut [bool; RAW],
    op: &Op,
) -> Result<(), TestCaseError> {
    match *op {
        Op::Write(i, v) => {
            store.write(graph.raw[i], v);
            values[i] = v;
            written[i] = true;
        }
        Op::SetIfChanged(i, v) => {
            store.set_if_changed(graph.raw[i], v);
            written[i] |= values[i] != v;
            values[i] = v;
        }
        Op::Update(i, d) => {
            store.update(graph.raw[i], |x| *x += d);
            values[i] += d;
            written[i] = true;
        }
        Op::ReadMemo(m) => {
            prop_assert_eq!(store.read(graph.memos[m]), expected(*values, m));
        }
        Op::Batch(ref ops) => {
            store.batch(|| {
                ops.iter()
                    .try_for_each(|op| apply(store, graph, values, written, op))
            })?;
        }
    }
    prop_assert_eq!(store.verify_integrity(), Ok(()));
    Ok(())
}

proptest! {
    #![proptest_config(crate::test_support::proptest_config(256))]

    // Glitch freedom under any mix of writes, batches, and reads: a memo
    // never returns a value its sources no longer give.
    #[test]
    fn memos_read_what_their_sources_give(
        init in [-3..3i32, -3..3i32, -3..3i32, -3..3i32],
        ops in prop::collection::vec(op(), 0..24),
    ) {
        let store = SignalStore::new();
        let graph = Graph::new(&store, init);
        let (mut values, mut written) = (init, [false; RAW]);
        for op in &ops {
            apply(&store, &graph, &mut values, &mut written, op)?;
        }
    }

    // A snapshot goes stale exactly when a signal it read changed: a raw
    // signal written (not by `set_if_changed` with an equal value), or a
    // memo whose value is now different.
    #[test]
    fn a_snapshot_is_stale_exactly_when_a_read_value_changed(
        init in [-3..3i32, -3..3i32, -3..3i32, -3..3i32],
        reads in prop::collection::vec(0..RAW + MEMOS, 0..RAW + MEMOS),
        writes in prop::collection::vec(write_op(), 0..12),
        batched in any::<bool>(),
    ) {
        let store = SignalStore::new();
        let graph = Graph::new(&store, init);
        let mut snapshot = Vec::new();
        track_into(&mut snapshot, Nesting::Isolate, || {
            for &r in &reads {
                match r.checked_sub(RAW) {
                    None => store.read(graph.raw[r]),
                    Some(m) => store.read(graph.memos[m]),
                };
            }
        });
        let (mut values, mut written) = (init, [false; RAW]);
        let writes = if batched { vec![Op::Batch(writes)] } else { writes };
        for op in &writes {
            apply(&store, &graph, &mut values, &mut written, op)?;
        }
        let stale = reads.iter().any(|&r| match r.checked_sub(RAW) {
            None => written[r],
            Some(m) => expected(values, m) != expected(init, m),
        });
        prop_assert_eq!(store.any_changed(&snapshot), stale);
    }
}
