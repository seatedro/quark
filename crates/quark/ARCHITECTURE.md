# quark

View macro + reactive signal store for building UIs.

## Reactive

`quark::reactive` is a fine-grained reactive system modeled on SolidJS.
`Signal<T>` is an 8-byte `Copy` handle into a `SignalStore` arena; values
live in slots indexed by `SignalId { index, generation }`. Reads that
happen inside an observer scope are automatically tracked; writes mark
dependents for lazy recomputation.

### Three-state propagation

Every memo is `Clean`, `Check`, or `Dirty`. Raw signals carry no state,
so reading a written signal cannot consume the change before its memos
see it.

- **Clean** — value is up-to-date.
- **Check** — a transitive source changed; value may or may not still
  be correct. On read, the memo brings its sources up to date first and
  recomputes only if one of them marked it `Dirty`; otherwise it returns
  to `Clean`.
- **Dirty** — a direct source changed. A write marks the signal's direct
  subscribers `Dirty` and everything further downstream `Check`; a memo
  whose recomputed value changed does the same to its own subscribers.

A memo that reads itself, directly or through other memos, panics with
the cycle path (`memo cycle: memo #1 ... (#1 -> #2 -> #1)`). A compute
that panics leaves the memo `Dirty` with its old value, so the next read
retries.

`any_dirty()` returns true if any slot has been written since the last
`clear_dirty()`. Typical frame loop: read `any_dirty()` to decide
whether to redraw, render, then call `clear_dirty()` so the next frame
starts Clean.

### Versions and snapshots

Every slot carries a version stamped from the store's clock whenever its
value changes: a write, or a memo recompute whose value differs. An
observer that is not a memo, such as a cache boundary, keeps the
`(SignalId, version)` pairs it read (`track_into`) and asks
`any_changed(snapshot)` before reusing its output. The check walks the
snapshot in read order, brings memos up to date on the way, stops at the
first change, and allocates nothing when no memo recomputes.

### Tracking scopes

Scopes nest on one thread-local stack and a read lands in the innermost.
`Nesting::Isolate` keeps a scope's reads from the enclosing one: a memo's
compute, so its reader depends on the memo and the equality check can
stop a change. `Nesting::Merge` hands them up as well: a cache boundary
nested in another, whose output the outer recording contains. A replayed
boundary reports its recorded reads with `track_reads` in place of
reading them. A source that publishes into a signal and remembers the
version can report a read without the store (`track_read_at`); before
its first publish it reports `track_unpublished`, which is always stale.

### Batches

`batch(f)` updates values and versions at once but marks subscribers
once, when the outermost batch ends. A memo read inside the batch applies
the held-back writes first, so it never sees half a batch.

### Integrity

`verify_integrity()` checks the free list, edge symmetry, that a memo
whose source may be stale is not `Clean`, versions against the clock, and
the batch's held-back writes. Mutations run it through `debug_assert!`:
the whole store with the `integrity-checks` feature, the touched slot
otherwise.

### Memo equality short-circuit

`create_memo` takes `Fn(&SignalStore) -> T` for `T: Clone + PartialEq`.
If a recomputed memo value equals its previous value, dependents stay
`Clean` and don't re-read it. This is the primary perf lever: upstream
writes cascade down the graph only as far as they change anything.

### Store derive

`#[derive(Store)]` on a struct with scalar fields generates a
`FooStore` parallel struct where every field becomes `Signal<T>`, plus
`FooStore::new(&store, Foo { ... })`. Struct-level `#[store(default)]`
adds `FooStore::new_default(&store)` for `Default` structs.
`#[store(flatten)]` on a nested struct field inlines its fields into the
parent's store. Use this to split big `State` structs into
fine-grained signals without hand-writing every field.

## Sharp edges

- **Re-entrant writes panic.** Writing to a signal while already inside
  a mutable borrow of the store (e.g. inside a `.update()` closure that
  reaches back into the store) triggers `RefCell::borrow_mut().expect(...)`.
  We don't have a reactive effect system today; when we add one, effects
  that write during their own execution will need to defer via a
  pending-writes queue.
- **`set_if_changed` requires `PartialEq`.** Self-documenting compile
  error if someone derives `Store` on a type missing `PartialEq` and a
  caller reaches for the dedupe-on-write path.
- **Memos with unstable outputs.** `create_memo` requires `PartialEq`
  precisely because the equality short-circuit is load-bearing. A memo
  whose compute fn returns `Vec<T>` etc. that ~always differs will
  invalidate every frame — prefer memoizing scalar or small-hashable
  derived values.

## View macro

`view!` lowers HTML-like markup to builder calls at compile time; the
syntax reference is [docs/guide/writing-views.md](../../docs/guide/writing-views.md).

### `{@sig}` reactive attribute

`name={@sig}` expands to `.name(cx.read(sig))`. Requires an
`ElementContext` (or any type with a `read<T>(Signal<T>) -> T` method)
named `cx` in scope at the call site.
