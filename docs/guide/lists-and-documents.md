# Lists and documents

Virtualized lists, the block document, selection, find, trees, tables, and
diffs.

- The app keeps the whole model; a row table keeps every row's height.
- Each frame builds elements only for rows in the viewport plus an overscan
  margin.
- Selection and find work on the model by stable key, so they cover rows
  never built.

## Variable-height lists

`quark_ui::virtual_list::VariableList`: a row table (one `RowKey` and height
per row, prefix sums in a Fenwick tree) plus one viewport's scroll state.

- Rows start at an estimated height and are measured as they enter the
  window.
- Every mutation returns the scroll delta that keeps the first visible row
  in place on screen.
- While pinned to the bottom, it follows the end instead (chat streaming,
  prepended history).
- A new list starts pinned to the bottom.
- `window(overscan)` returns the index range to build and the spacer heights
  above and below.
- `scroll_to(key, align)` brings a row into view.

From [crates/quark-ui/src/lib.rs](../../crates/quark-ui/src/lib.rs):

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

## The block document

`quark_ui::document`: a virtualized block document on `VariableList`.

- Every heading, paragraph, list item, quote, table, code block, and rule is
  its own block, so selection runs across rows.
- `RowDecorator` draws per-row chrome: headers, backgrounds, and with
  `leading_edge` a bar along the row's leading edge (an error row's accent).

| Type | Role |
|---|---|
| `Document` | App-owned state: rows, block order, selection, on-screen row geometry. Text stays in the app's model, read through `DocumentSource` |
| `MarkdownDocument` | Wraps `Document` for markdown rows: parses, converts, highlights code on a worker (feature `syntax` plus a grammar store, see [Syntax highlighting and grammar packs](syntax-packs.md)), loads images on a worker (feature `images`), and measures off-screen rows in the background so heights become exact while idle |
| `IncrementalMarkdown` | Reparses only a streaming message's tail: the part after the last closed top-level code fence followed by a blank line |

- Example: [chat_demo.rs](../../crates/quark-app/examples/chat_demo.rs), a
  chat with roles, author lines, jump to latest, 5,000 messages, a streaming
  answer, drag selection, copy, select all, and find.

## Selection

- `quark::Selection` endpoints are `(BlockKey, byte)` pairs.
- `quark::BlockOrder` keeps the document order of block keys.
- A selection survives rows scrolling out, prepended history, and text
  streaming into the last block.
- Removing a block shrinks the selection inward (`Selection::after_remove`).
- Offsets out of range or inside a multibyte character are clamped on read,
  so a selection made against older text stays valid.

From [crates/quark/src/lib.rs](../../crates/quark/src/lib.rs):

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

## Find

- `FindState` matches a query against every block's text in document order.
- `next_match` and `prev_match` step the current match.
- `update` rescans only blocks whose revision changed, so streaming costs
  one block's scan.
- Case-insensitive by Unicode lowercase; `ß` matches `ss`.
- `find_bar` renders the query field, match count, and previous, next, and
  close buttons.

## Trees, tables, and diffs

From `quark-components`:

| View | State | Example |
|---|---|---|
| `tree_view` | `TreeState`: flat node table, lazy child loading, drag reordering, multi-select, type-ahead | [tree_table_demo.rs](../../crates/quark-app/examples/tree_table_demo.rs) |
| `table_view` | `TableState` over the app's `TableData`: sorting, resizable and reorderable columns, sticky header | [tree_table_demo.rs](../../crates/quark-app/examples/tree_table_demo.rs) (100,000 rows) |
| `diff_view` | `DiffViewState` over a `quark_diff::DiffDocument`: unified or side by side, word highlights, collapsed context, selection | [diff_demo.rs](../../crates/quark-app/examples/diff_demo.rs) |

- Data lives in parallel columns; only visible rows are built.
- Each row (diff view: each row's cells) is a cache boundary inside one
  boundary for the whole view, so an unchanged frame replays without
  building.
- Views emit their own event type through a caller-supplied mapping
  (`fn(TreeEvent) -> Action`); hand each event back to the state's `handle`
  method.
