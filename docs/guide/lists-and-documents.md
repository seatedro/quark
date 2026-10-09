# Lists and documents

Long content is virtualized: the app keeps the whole model, a row table
keeps every row's height, and each frame builds elements only for the rows
in the viewport plus an overscan margin. Selection and find work on the
model by stable key, so they cover rows that were never built.

## Variable-height lists

`quark_ui::virtual_list::VariableList` is a row table (one `RowKey` and one
height per row, with prefix sums in a Fenwick tree) plus the scroll state of
one viewport. Rows start at an estimated height and are measured as they
enter the window. Every mutation returns the scroll delta that keeps the
first visible row where it was on screen, or, while the view is pinned to
the bottom, keeps following the end, which is what a chat transcript needs
while content streams in or history is prepended.

This doctest from [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs)
builds a list and a find over block text:

```rust
use quark::BlockKey;
use quark_ui::document::FindState;
use quark_ui::virtual_list::{RowKey, VariableList};

// 1,000 rows estimated at 20 points in a 100 point viewport. A new list
// is pinned to the bottom, like a chat transcript.
let mut list = VariableList::new(20.0, 100.0);
let keys: Vec<RowKey> = (0..1000).map(RowKey).collect();
list.extend(&keys).unwrap();
let window = list.window(0.0);
assert_eq!(window.range, 995..1000);

let mut find = FindState::new("rust");
find.update([
    (BlockKey(1), 0, "Rust is a language."),
    (BlockKey(2), 0, "Quark is written in rust."),
]);
assert_eq!(find.matches().len(), 2);
assert_eq!(find.status(), "1 of 2");
```

`window(overscan)` returns the index range to build and the spacer heights
above and below it. `scroll_to(key, align)` brings a row into view.

## The block document

`quark_ui::document` builds a virtualized block document on `VariableList`.
Every heading, paragraph, list item, quote, table, code block, and rule of
every row is its own block, so selection runs across rows. Apps draw
per-row chrome (headers, backgrounds, and with `leading_edge` a bar along
the row's leading edge, such as an error row's accent) through a
`RowDecorator`.

- `Document` is the app-owned state: rows, block order, selection, and
  the geometry of the rows on screen. The text stays in the app's model,
  read through `DocumentSource`.
- `MarkdownDocument` wraps it for markdown rows: it parses, converts,
  highlights code blocks on a `quark-syntax` worker (with the `syntax`
  feature and a grammar store; see
  [Syntax highlighting and grammar packs](syntax-packs.md)), loads images on a worker (with `images`), and measures
  off-screen rows on a background thread so their heights become exact
  while the app is idle.
- `IncrementalMarkdown` reparses only the tail of a streaming message: the
  part after the last closed top-level code fence that a blank line
  follows.

[chat_demo.rs](../../crates/quark-app/examples/chat_demo.rs) assembles a
chat on top of it (roles, author lines, jump to latest) with 5,000 messages,
a streaming answer, drag selection, copy, select all, and find.

## Selection

`quark::Selection` endpoints are `(BlockKey, byte)` pairs, and
`quark::BlockOrder` keeps the document order of block keys. A selection
survives its rows scrolling out, history being prepended, and text
streaming into the last block. When a block is removed the selection
shrinks inward (`Selection::after_remove`). This doctest from
[crates/quark/src/lib.rs](../../crates/quark/src/lib.rs) copies across two
blocks:

```rust
use std::collections::HashMap;
use quark::{BlockKey, BlockOrder, Selection, SelectionPoint, copy_text};

let mut order = BlockOrder::new();
order.extend([BlockKey(1), BlockKey(2)]);
let text = HashMap::from([
    (BlockKey(1), "Hello world".to_string()),
    (BlockKey(2), "Second block".to_string()),
]);

let selection = Selection::new(
    SelectionPoint::new(BlockKey(1), 6),
    SelectionPoint::new(BlockKey(2), 6),
);
assert_eq!(copy_text(&selection, &order, &text, "\n"), "world\nSecond");
```

Offsets out of range or inside a multibyte character are clamped when the
text is read, so a selection made against older text stays valid.

## Find

`FindState` matches a query against every block's text in document order,
with a current match that `next_match` and `prev_match` step through. It
rescans only blocks whose revision changed since the last `update`, so
keeping it current while text streams costs one block's scan. Matching is
case-insensitive by Unicode lowercase, with `ß` matching `ss`. `find_bar`
renders the query field, the match count, and previous, next, and close
buttons.

## Trees, tables, and diffs

`quark-components` has three more virtualized views. Each keeps its data in
parallel columns, builds only visible rows, and puts each row (the diff
view: each row's cells) in its own cache boundary inside one boundary for
the whole view, so a frame where
nothing changed replays without building.

| View | State | Example |
|---|---|---|
| `tree_view` | `TreeState`: a flat node table with lazy child loading, drag reordering, multi-select, type-ahead | [tree_table_demo.rs](../../crates/quark-app/examples/tree_table_demo.rs) |
| `table_view` | `TableState` over the app's `TableData`: sorting, resizable and reorderable columns, a sticky header | [tree_table_demo.rs](../../crates/quark-app/examples/tree_table_demo.rs) (100,000 rows) |
| `diff_view` | `DiffViewState` over a `quark_diff::DiffDocument`: unified or side by side, word highlights, collapsed context, selection | [diff_demo.rs](../../crates/quark-app/examples/diff_demo.rs) |

The views emit their own event type through a mapping the caller supplies
(`fn(TreeEvent) -> Action`); the app hands each event back to the state's
`handle` method.
