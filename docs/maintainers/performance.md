# Performance internals

How quark keeps steady frames allocation-free. App-facing advice is in the
[performance guide](../guide/performance.md).

## Recycled frame memory

- **Element storage:** each element type keeps a free list of its boxes, and
  child lists keep their buffers, so a steady frame builds from last frame's
  memory ([element/pool.rs](../../crates/quark-ui/src/element/pool.rs)). The
  builder API stays plain values with no arena lifetimes.
- **Frame buffers:** the adapter keeps and refills the buffers of the frame
  before last: the scene the renderer hands back, the input routing frame,
  text input areas, the accessibility frame, tooltip regions.
- **Text layouts:** `LayoutCache` keeps shaped layouts across frames and
  evicts ones unused for a number of frames.

## Data layout

- State that grows with content lives in parallel columns indexed by row or
  key, not object graphs.
- Applies to: the animation table, the virtual list's row table and Fenwick
  offsets, the element cache, the diff document, the tree and table views,
  the markdown block model.
- Stable keys map to rows through an index; the animation table keeps it in
  sync as it swap-removes rows.
- Each type has `verify_integrity`, called after mutations in debug builds
  ([TEST_BIBLE.md](../../TEST_BIBLE.md)).
- `BlockOrder`, the animation table, the row table, and the block document
  check only touched entries unless the `integrity-checks` feature asks for
  the whole structure.

## Release profile

- Thin LTO, one codegen unit, symbols kept so the panic hook's backtrace is
  readable.
