# Performance model

A view rebuilds its element tree every frame, which keeps app code simple.
Three mechanisms keep that cheap: frames are drawn only when something
changed, cache boundaries replay unchanged subtrees without building them,
and per-frame buffers are recycled so a steady frame does not call the
allocator. Tests measure the last property as allocation budgets.

## Frames only when needed

The adapter draws a frame for a reason it can name (hover, focus, an
action, a text edit, a scroll, a theme change) or when an animation or the
app asks for one. An idle window draws nothing. A pointer move that does
not change the set of elements under the pointer does not redraw.
[State, actions, and messages](state-actions-messages.md#when-frames-are-drawn)
lists the triggers.

## Cache boundaries

`quark_ui::element::cached(key, inputs_hash, build)` stores a subtree's
layout and paint output in the window's `ElementCache` and replays it,
moved to the boundary's new position, without calling `build`. This
doctest from [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs)
caches each row of a list:

```rust
use quark::view;
use quark_ui::element::{AnyElement, IntoAnyElement, cached, div, inputs_hash, text};
use quark_ui::style::Styled;

struct Row {
    id: u64,
    revision: u64,
    label: String,
}

fn rows(rows: &[Row], selected: Option<u64>) -> AnyElement {
    view! {
        <div class="flex-col">
            for row in rows {
                {row_view(row, selected == Some(row.id))}
            }
        </div>
    }
}

fn row_view(row: &Row, is_selected: bool) -> AnyElement {
    let label = row.label.clone();
    // The hash covers everything the closure reads.
    let build = move || view! {
        <text @when {is_selected} { class="font-semibold" }>{label}</text>
    };
    view! { <cached(row.id, inputs_hash(&(row.revision, is_selected)), build) /> }
}
```

A boundary replays only while all of these match the frame that recorded
it: the key and inputs hash, the size its parent offers, the scale factor
and theme, the focused element (if the subtree read focus), and inherited
paint state. Hovered subtrees, subtrees with animations in motion, and
subtrees containing a text input rebuild instead of going stale. The hash
must cover every value the closure reads; a value it misses is the one way
a boundary shows stale content. Unused entries are evicted after 60 layout
passes. The full contract is in
[element/cache/mod.rs](../../crates/quark-ui/src/element/cache/mod.rs).

The tree, table, and diff views put each row (the diff view: each row's
cells) in a boundary inside one boundary for the whole view, so a frame
where nothing changed replays one entry, and a scrolled frame builds only
the rows that entered the window. The block document caches each row.

Dock tab strips and split dividers cache themselves too, but an app's own
chrome (a title bar, a toolbar, a sidebar header) rebuilds every frame
unless the app puts it in a boundary. In the Workbench, boundaries around
the dock, splits, toolbar, and sidebar took a repeated frame from 507
allocations to 45.

The `build` closure is `'static`, so it owns what it reads, and cloning
strings into it allocates on every frame, replayed or not. The
Workbench's title bar ([titlebar.rs](../../examples/workbench/src/shell/titlebar.rs))
keeps its text in an `Rc` that the view reuses while the text is
unchanged, moves a clone of the `Rc` into the closure, and hashes the
data with the size it is drawn at:
`inputs_hash(&(&*data, width.to_bits(), height.to_bits()))`.

## Recycled frame memory

- **Element storage.** Each element type keeps a free list of its boxes,
  and child lists keep their buffers, so a steady frame builds its tree
  from the last frame's memory ([element/pool.rs](../../crates/quark-ui/src/element/pool.rs)).
  The builder API stays plain values with no arena lifetimes.
- **Frame buffers.** The adapter keeps the buffers of the frame before
  last (the scene the renderer hands back, the input routing frame, the
  text input areas, the accessibility frame, the tooltip regions) and
  refills them.
- **Text layouts.** The `LayoutCache` keeps shaped layouts across frames
  and evicts the ones unused for a number of frames, so unchanged text is
  not shaped again.

## Data layout

State that grows with content lives in parallel columns indexed by row or
key rather than in a graph of objects: the animation table, the virtual
list's row table and Fenwick offsets, the element cache, the diff
document, the tree and table views, and the markdown block model. Stable
keys map to rows through an index, which the animation table, for one,
keeps in sync as it swap-removes rows. Each of these types has a
`verify_integrity` method that debug builds call after mutations, as
[TEST_BIBLE.md](../../TEST_BIBLE.md) requires. For `BlockOrder`, the
animation table, the row table, and the block document, debug builds check
only the entries a mutation touched unless the `integrity-checks` feature
asks for the whole structure.

## Allocation budgets

`quark_ui::test_alloc::Counting` is a global allocator that counts the
current thread's allocations; `test_alloc::count(f)` returns how many `f`
made, and `test_alloc::profile(f)` groups them by call site.
[frame_budget.rs](../../crates/quark-app/src/frame_budget.rs) runs whole
frames through the adapter and asserts:

| Frame | Budget |
|---|---|
| A cached list repeating the last frame, with or without a screen reader connected | 0 allocations |
| A tracked scroll container repeating the last frame | 0 |
| A list frame where one cached row changed | 40 per changed row |
| A markdown document repeating the last frame | 16 |
| A document frame after text streams into one message | 320 |

Examples with a test module can carry a budget of their own:
`hello_ui` rebuilds without a cache boundary and holds a repeated frame to
30 allocations. Each budget file also has an ignored test that prints the
call sites (`cargo test -p quark-app report_ -- --ignored --nocapture`).

An app measures its own frames the same way: an integration test installs
`Counting` as its global allocator and counts `UiTestHarness::frame`.
[examples/workbench/tests/perf.rs](../../examples/workbench/tests/perf.rs)
budgets the Workbench's whole window at 64 allocations for a repeated
frame and 512 for one streamed chunk of an answer with its frame. It
measures 45 (53 with a screen reader connected) and 337, and asserts
ceilings just above those, so a regression fails while still under
budget. Counts taken too early are noise:

- `Counting` counts only the calling thread. The block document measures
  rows, highlights code, and decodes images on worker threads, which the
  harness's fake clock does not drive, and a frame that picks up their
  results rebuilds rows. Before counting, wait for them with
  `MarkdownDocument::finish_highlights`, `finish_images`, and
  `finish_measures` (in that order: the first two rebuild rows that then
  need measuring), then draw a frame and advance the clock so the
  measured heights settle the scroll anchor.
- Draw a few frames first, and take the least of several counts, so a
  table growing once does not count:

  ```rust
  fn repeated_frame(ui: &mut UiTestHarness<Workbench>) -> u64 {
      (0..3)
          .map(|_| {
              test_alloc::count(|| {
                  ui.frame();
              })
              .1
          })
          .min()
          .unwrap_or(0)
  }
  ```

## Profiling

- The `devtools` feature's HUD (`ctrl+shift+h`) shows per-window frame
  timings, primitive counts, and text layout cache hits and misses.
- `profile-puffin` serves profiler scopes around frame phases, layout,
  paint, and text shaping on `127.0.0.1:8585` for `puffin_viewer`;
  `profile-tracy` sends the same scopes to Tracy. Enable one at a time.
- Release builds use thin LTO and one codegen unit, and keep symbols so
  the panic hook's backtrace is readable.
