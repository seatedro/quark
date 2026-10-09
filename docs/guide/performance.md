# Performance model

How to keep a view that rebuilds every frame cheap.

- A view rebuilds its element tree every frame.
- Frames are drawn only when something changed.
- Cache boundaries replay unchanged subtrees without building them.
- Steady frames reuse last frame's memory instead of allocating.

## Frames only when needed

- An idle window draws nothing.
- A pointer move that does not change the elements under the pointer does
  not redraw.
- Full trigger list:
[State, actions, and messages](state-actions-messages.md#when-frames-are-drawn).

## Cache boundaries

`quark_ui::element::cached(key, inputs_hash, build)` stores a subtree's
layout and paint in the window's `ElementCache` and replays it at the
boundary's new position without calling `build`.

From [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs):

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

- Replays only while these match the recording frame: key and inputs hash,
  the size the parent offers, scale factor and theme, the focused element
  (if the subtree read focus), inherited paint state.
- Hovered subtrees, subtrees with running animations, and subtrees
  containing a text input rebuild instead.
- The hash must cover every value the closure reads. A missed value is the
  one way a boundary shows stale content.
- Unused entries are evicted after 60 layout passes.
- Full contract:
  [element/cache/mod.rs](../../crates/quark-ui/src/element/cache/mod.rs).

### What caches itself

- Tree, table, and diff views: each row (diff view: each row's cells) in a
  boundary inside one for the whole view. An unchanged frame replays one
  entry; a scrolled frame builds only rows that entered.
- The block document: each row.
- Dock tab strips and split dividers.
- Not the app's own chrome (title bar, toolbar, sidebar header): it rebuilds
  every frame unless wrapped. In the Workbench, wrapping the dock, splits,
  toolbar, and sidebar cut a repeated frame from 507 allocations to 45.

### Closure gotcha

- `build` is `'static`, so it owns what it reads. Cloning strings into it
  allocates every frame, replayed or not.
- Keep such data in an `Rc` reused while unchanged, move an `Rc` clone into
  the closure, and hash the data with its drawn size.
- Example: the Workbench title bar
  ([titlebar.rs](../../examples/workbench/src/shell/titlebar.rs)) hashes
  `inputs_hash(&(&*data, width.to_bits(), height.to_bits()))`.

## Measuring

- Count a frame's allocations in a test:
  [Testing](testing.md#allocation-counts).
- The `devtools` HUD (`ctrl+shift+h`) shows per-window frame timings,
  primitive counts, and text layout cache hits and misses.
- `profile-puffin` serves profiler scopes (frame phases, layout, paint, text
  shaping) on `127.0.0.1:8585` for `puffin_viewer`.
- `profile-tracy` sends the same scopes to Tracy. Enable one of the two at a
  time.

How quark reuses frame memory internally:
[docs/maintainers/performance.md](../maintainers/performance.md).
