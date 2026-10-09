use std::collections::HashMap;

use proptest::prelude::*;
use quark::reactive::SignalStore;
use quark::selection::BlockKey;
use quark::{SemanticFrame, scene::Scene};
use quark_text::{LayoutCache, TextSystem};

use super::*;
use crate::accessibility::AccessibilityFrame;
use crate::action::Action;
use crate::element::{ElementContext, InputRouter, IntoAnyElement, div, render_element};
use crate::style::Styled;
use crate::theme::Theme;
use crate::virtual_list::ScrollAlign;

// ---------------------------------------------------------------------------
// A monospace grid measurer: every char is 10px wide, every line 20px tall,
// lines hard-wrap at the column count. Expected positions and heights can be
// worked out by hand.
// ---------------------------------------------------------------------------

const CHAR_W: f32 = 10.0;
const LINE_H: f32 = 20.0;

struct Grid;

#[derive(Debug, Clone)]
struct GridGeometry {
    /// Byte range of each visual line.
    lines: Vec<(usize, usize)>,
}

impl BlockGeometry for GridGeometry {
    fn height(&self) -> f32 {
        self.lines.len() as f32 * LINE_H
    }

    fn hit(&self, x: f32, y: f32) -> usize {
        let line = ((y / LINE_H).floor().max(0.0) as usize).min(self.lines.len() - 1);
        let (start, end) = self.lines[line];
        let col = (x / CHAR_W).round().max(0.0) as usize;
        (start + col).min(end)
    }

    fn range_rects(&self, range: std::ops::Range<usize>, out: &mut Vec<Rect>) {
        for (line, &(start, end)) in self.lines.iter().enumerate() {
            let (a, b) = (range.start.max(start), range.end.min(end));
            if a < b {
                out.push(Rect {
                    x: (a - start) as f32 * CHAR_W,
                    y: line as f32 * LINE_H,
                    width: (b - a) as f32 * CHAR_W,
                    height: LINE_H,
                });
            }
        }
    }
}

impl BlockMeasurer for Grid {
    type Geometry = GridGeometry;

    fn measure(&mut self, block: &Block, width: f32) -> GridGeometry {
        let cols = ((width / CHAR_W).floor() as usize).max(1);
        let mut lines = Vec::new();
        let mut start = 0;
        for line in block.text().split('\n') {
            let mut from = start;
            let end = start + line.len();
            loop {
                let to = (from + cols).min(end);
                lines.push((from, to));
                if to == end {
                    break;
                }
                from = to;
            }
            start = end + 1;
        }
        GridGeometry { lines }
    }
}

fn grid_style() -> DocumentStyle {
    DocumentStyle {
        font_size: 14.0,
        pad_x: 10.0,
        pad_y: 10.0,
        block_gap: 10.0,
        line_scroll: 60.0,
        edge: 32.0,
        overscan: 450.0,
        line_height: LineHeight::PARAGRAPH,
    }
}

/// Message `i`: row key `i`, blocks `10i` ("m{i} first") and `10i + 1`
/// ("m{i} second"). With the grid style each row is 90px tall.
fn message(i: u64) -> DocumentRow {
    message_with(i, &[&format!("m{i} first"), &format!("m{i} second")])
}

fn message_with(i: u64, blocks: &[&str]) -> DocumentRow {
    DocumentRow {
        key: RowKey(i),
        chrome: RowChrome {
            header_height: 20.0,
            label: Some(format!("author {i}").into()),
            kind: (i % 2) as u32,
        },
        blocks: blocks
            .iter()
            .enumerate()
            .map(|(b, text)| Block::plain(BlockKey(i * 10 + b as u64), *text))
            .collect(),
        adornments: Vec::new(),
    }
}

/// A view over a grid-measured document, with a frame clock.
struct Doc {
    messages: HashMap<RowKey, DocumentRow>,
    view: Document<GridGeometry>,
    size: (f32, f32),
    now_ms: u64,
}

impl Doc {
    fn new(messages: impl IntoIterator<Item = DocumentRow>) -> Self {
        let messages: Vec<DocumentRow> = messages.into_iter().collect();
        let mut view = Document::new(grid_style());
        view.extend(&messages).unwrap();
        let mut doc = Self {
            messages: messages.into_iter().map(|m| (m.key, m)).collect(),
            view,
            size: (400.0, 900.0),
            now_ms: 0,
        };
        doc.frame();
        doc
    }

    fn rows(n: u64) -> Self {
        Self::new((0..n).map(message))
    }

    fn frame(&mut self) {
        let (w, h) = self.size;
        self.view
            .prepare(w, h, self.now_ms, &self.messages, &mut Grid);
        self.now_ms += 16;
    }

    fn scroll_to(&mut self, offset: f32) {
        self.view.set_scroll_offset(offset);
        self.frame();
    }

    /// Viewport point over byte `col` of the first line of `block`.
    fn point(&self, block: u64, col: usize) -> (f32, f32) {
        let visible = self
            .view
            .visible_blocks()
            .iter()
            .find(|b| b.key == BlockKey(block))
            .unwrap_or_else(|| panic!("block {block} is not visible"));
        (
            visible.rect.x + col as f32 * CHAR_W,
            visible.rect.y + LINE_H * 0.5,
        )
    }

    fn press(&mut self, (x, y): (f32, f32)) {
        self.view.handle(DocumentEvent::PointerDown { x, y });
    }

    fn drag(&mut self, (x, y): (f32, f32)) {
        self.view.handle(DocumentEvent::PointerDrag { x, y });
    }

    fn release(&mut self) {
        self.view.handle(DocumentEvent::PointerUp);
    }

    fn copy(&self) -> String {
        self.view.selected_text(&self.messages)
    }

    /// Key of the row under the viewport's vertical middle.
    fn middle_row(&self) -> u64 {
        let middle = self.size.1 * 0.5;
        self.view
            .visible_rows()
            .iter()
            .find(|r| r.top <= middle && r.top + r.height > middle)
            .map_or(0, |r| r.key.0)
    }

    /// The row under the viewport's top edge and where its top sits.
    fn anchor(&self) -> String {
        self.view
            .visible_rows()
            .iter()
            .find(|r| r.top <= 0.0 && r.top + r.height > 0.0)
            .map_or("-".to_owned(), |r| format!("{}@{}", r.key.0, r.top))
    }

    /// `"<block>:<lo>..<hi>"` for every materialized block with a highlight.
    fn highlights(&self) -> String {
        self.view
            .visible_blocks()
            .iter()
            .filter_map(|b| {
                let (lo, hi) = self.view.block_selection(b.key, b.text_len)?;
                Some(format!("{}:{lo}..{hi}", b.key.0))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Replaces the text of `block` in message `row` and reports the change.
    fn stream(&mut self, row: u64, block: usize, text: &str) {
        let message = self.messages.get_mut(&RowKey(row)).unwrap();
        let key = message.blocks[block].key;
        message.blocks[block] = Block::plain(key, text);
        self.view.update(message).unwrap();
    }

    /// Where row `key`'s top sits relative to the viewport top, whether or
    /// not it is materialized.
    fn screen_top(&self, key: u64) -> f32 {
        let rows = self.view.list().rows();
        rows.offset_of(RowKey(key)).unwrap() - self.view.scroll_offset()
    }

    fn last_row_bottom(&self) -> f32 {
        self.view
            .visible_rows()
            .last()
            .map_or(0.0, |r| r.top + r.height)
    }
}

// ---------------------------------------------------------------------------
// Selection
// ---------------------------------------------------------------------------

#[test]
fn drag_from_message_3_to_message_7_copies_exactly_that_text() {
    let mut doc = Doc::rows(20);
    doc.scroll_to(0.0);

    doc.press(doc.point(30, 3));
    doc.drag(doc.point(51, 4));
    doc.drag(doc.point(71, 2));
    doc.release();

    assert_eq!(
        doc.copy(),
        "first\n\nm3 second\n\nm4 first\n\nm4 second\n\nm5 first\n\nm5 second\n\n\
         m6 first\n\nm6 second\n\nm7 first\n\nm7"
    );
}

#[test]
fn selection_survives_its_rows_scrolling_out_and_back() {
    let mut doc = Doc::rows(50);
    doc.scroll_to(0.0);
    doc.press(doc.point(30, 0));
    doc.drag(doc.point(41, 9));
    doc.release();
    let highlights = doc.highlights();

    doc.scroll_to(3000.0);
    let materialized = |doc: &Doc| {
        let rows = doc.view.visible_rows();
        rows.iter().any(|r| r.key == RowKey(3))
    };
    let away = (materialized(&doc), doc.copy());
    doc.scroll_to(0.0);

    assert_eq!(
        away,
        (
            false,
            "m3 first\n\nm3 second\n\nm4 first\n\nm4 second".to_owned()
        )
    );
    assert_eq!(doc.highlights(), highlights);
}

#[test]
fn prepending_history_keeps_the_selection_and_the_row_on_screen() {
    let mut doc = Doc::new((1000..1050).map(message));
    let row = doc.view.list().rows().offset_of(RowKey(1010)).unwrap();
    doc.scroll_to(row - 5.0);
    doc.press(doc.point(10_100, 0));
    doc.drag(doc.point(10_110, 2));
    doc.release();
    let before = (doc.anchor(), doc.copy());

    let history: Vec<DocumentRow> = (0..100).map(message).collect();
    doc.view.prepend(&history).unwrap();
    doc.messages.extend(history.into_iter().map(|m| (m.key, m)));
    doc.frame();

    assert_ne!(before.0, "-");
    assert_eq!((doc.anchor(), doc.copy()), before);
}

#[test]
fn key_command_maps_cmd_and_ctrl_bindings() {
    let cases = [
        ("ctrl+c", Some(DocumentCommand::Copy)),
        ("Cmd+C", Some(DocumentCommand::Copy)),
        ("ctrl+a", Some(DocumentCommand::SelectAll)),
        ("cmd+a", Some(DocumentCommand::SelectAll)),
        ("ctrl+x", None),
        ("ctrl+shift+c", None),
    ];
    for (binding, expected) in cases {
        let pressed: Binding = binding.parse().unwrap();
        assert_eq!(key_command(&pressed), expected, "{binding}");
    }
}

#[test]
fn select_all_copies_every_block() {
    let mut doc = Doc::rows(3);

    doc.view.select_all();

    assert_eq!(
        doc.copy(),
        "m0 first\n\nm0 second\n\nm1 first\n\nm1 second\n\nm2 first\n\nm2 second"
    );
}

// Catches a selection end left in a block an update removed: it moves to
// the end of the previous surviving block, so copy stops there.
#[test]
fn update_dropping_the_block_a_selection_ends_in_copies_up_to_the_previous_block() {
    let mut doc = Doc::new([message_with(0, &["alpha", "beta", "gamma"])]);
    doc.press(doc.point(0, 2));
    doc.drag(doc.point(2, 3));
    doc.release();

    let shrunk = message_with(0, &["alpha", "beta"]);
    doc.view.update(&shrunk).unwrap();
    doc.messages.insert(shrunk.key, shrunk);

    assert_eq!(doc.copy(), "pha\n\nbeta");
}

// Catches a block whose key another message already owns being drawn and
// hit-tested: a press on it stored a point outside the document.
#[test]
fn block_with_a_key_another_message_owns_cannot_be_selected() {
    let mut doc = Doc::new([message_with(0, &["owner"])]);
    let mut duplicate = message_with(1, &["second"]);
    duplicate.blocks[0].key = BlockKey(0);
    doc.view.push(&duplicate).unwrap();
    doc.messages.insert(duplicate.key, duplicate);
    doc.frame();

    // Below both messages: nearest to where the duplicate would be drawn.
    doc.press((50.0, 160.0));

    let drawn: Vec<(u64, u64)> = doc
        .view
        .visible_blocks()
        .iter()
        .map(|b| (b.row.0, b.key.0))
        .collect();
    assert_eq!(drawn, [(0, 0)]);
    // The end of "owner", not of "second".
    assert_eq!(
        doc.view.selection(),
        Some(Selection::collapsed(SelectionPoint::new(BlockKey(0), 5)))
    );
}

// Catches a press between an edit and the next prepare landing in a block
// the edit took out of the document.
#[test]
fn press_after_an_update_removed_the_block_skips_it() {
    let mut doc = Doc::new([message_with(0, &["keep", "drop"])]);
    let dropped = doc.point(1, 1);
    let shrunk = message_with(0, &["keep"]);
    doc.view.update(&shrunk).unwrap();
    doc.messages.insert(shrunk.key, shrunk);

    doc.press(dropped);
    doc.view.push(&message(1)).unwrap();

    assert_eq!(
        doc.view.selection().map(|s| s.focus.block),
        Some(BlockKey(0))
    );
}

// ---------------------------------------------------------------------------
// Autoscroll
// ---------------------------------------------------------------------------

/// Scrolled to the middle of 50 rows, pressed on the row in the middle of
/// the viewport and dragged above the top edge. Returns the pressed row.
fn hold_above_the_top_edge(doc: &mut Doc) -> u64 {
    doc.scroll_to(2000.0);
    let row = doc.middle_row();
    doc.press(doc.point(row * 10, 0));
    doc.drag((50.0, -30.0));
    row
}

#[test]
fn autoscroll_moves_while_held_past_the_edge_and_stops_on_release() {
    let mut doc = Doc::rows(50);
    let row = hold_above_the_top_edge(&mut doc);

    // Content moves down (the view scrolls up) while the pointer is held.
    let mut held = vec![doc.screen_top(row)];
    for _ in 0..10 {
        doc.frame();
        held.push(doc.screen_top(row));
    }
    let wanted_frames = doc.view.wants_frame();
    doc.release();
    let released = doc.screen_top(row);
    for _ in 0..3 {
        doc.frame();
    }

    // The first held frame only starts the clock.
    assert!(
        held[1..].windows(2).all(|w| w[1] > w[0]),
        "row {row} should move down every frame: {held:?}"
    );
    assert!(wanted_frames);
    assert_eq!(doc.screen_top(row), released);
    assert!(!doc.view.wants_frame());
}

#[test]
fn selection_focus_follows_rows_autoscrolling_under_the_pointer() {
    let mut doc = Doc::rows(50);
    let row = hold_above_the_top_edge(&mut doc);
    let focus_row = |doc: &Doc| doc.view.selection().map_or(0, |s| s.focus.block.0 / 10);
    let first = focus_row(&doc);

    for _ in 0..10 {
        doc.frame();
    }

    let last = focus_row(&doc);
    assert!(
        last < first && first < row,
        "focus {first} -> {last}, anchor {row}"
    );
}

#[test]
fn autoscroll_is_faster_farther_past_the_edge() {
    let travel = |pointer_y: f32| {
        let mut doc = Doc::rows(50);
        doc.scroll_to(2000.0);
        let row = doc.middle_row();
        doc.press(doc.point(row * 10, 0));
        doc.drag((50.0, pointer_y));
        let start = doc.screen_top(row);
        for _ in 0..5 {
            doc.frame();
        }
        doc.screen_top(row) - start
    };

    let [near, far] = [20.0, -60.0].map(travel);

    assert!(0.0 < near && near < far, "near {near}, far {far}");
}

// ---------------------------------------------------------------------------
// Scroll model
// ---------------------------------------------------------------------------

#[test]
fn update_then_prepare_keeps_pin() {
    let mut doc = Doc::rows(50);
    let mut text = String::from("m49 second");

    let mut bottoms = Vec::new();
    for _ in 0..8 {
        text.push_str(" streaming words");
        doc.stream(49, 1, &text);
        doc.frame();
        bottoms.push(doc.last_row_bottom());
    }

    assert!(doc.view.is_stuck_to_bottom());
    assert_eq!(bottoms, [900.0; 8]);
    assert!(doc.view.visible_rows().last().unwrap().height > 90.0);
}

#[test]
fn new_content_while_scrolled_up_is_flagged_until_scrolled_to_bottom() {
    let mut doc = Doc::rows(50);
    doc.scroll_to(0.0);

    doc.stream(49, 1, "m49 second, and more");
    doc.frame();
    let scrolled_up = (doc.view.has_content_below(), doc.view.scroll_offset());
    doc.view.scroll_to_bottom();
    doc.frame();

    assert_eq!(scrolled_up, (true, 0.0));
    assert_eq!(
        (doc.view.has_content_below(), doc.last_row_bottom()),
        (false, 900.0)
    );
}

// Catches edits to older rows (a highlight arriving) flagging content
// below when nothing new arrived at the bottom.
#[test]
fn editing_an_older_row_while_scrolled_up_flags_no_content_below() {
    let mut doc = Doc::rows(50);
    doc.scroll_to(0.0);

    doc.stream(10, 1, "m10 second, edited");
    doc.frame();

    assert!(!doc.view.has_content_below());
}

#[test]
fn narrowing_the_viewport_rewraps_rows() {
    // 76 chars: 2 lines at 38 columns (400px), 5 lines at 18 (200px).
    let long = "x".repeat(76);
    let mut doc = Doc::new([message_with(0, &[&long])]);
    let wide = doc.view.visible_rows()[0].height;

    doc.size.0 = 200.0;
    doc.frame();

    assert_eq!((wide, doc.view.visible_rows()[0].height), (80.0, 140.0));
}

// ---------------------------------------------------------------------------
// Invariants under arbitrary document edits
// ---------------------------------------------------------------------------

// Catches debug integrity checks that walk the whole document per block or
// per message, which made loading a long view quadratic in debug
// builds. Counted in check steps, not timed.
#[test]
fn extending_with_30k_blocks_checks_in_linear_steps() {
    let messages: Vec<DocumentRow> = (0..10_000)
        .map(|i| message_with(i, &["a", "b", "c"]))
        .collect();
    let mut view: Document<GridGeometry> = Document::new(grid_style());
    view.push(&message_with(1_000_000, &["first"])).unwrap();

    let before = quark::selection::integrity_steps();
    view.extend(&messages).unwrap();
    let steps = quark::selection::integrity_steps() - before;

    assert_eq!(view.len(), 10_001);
    assert!(steps <= 5 * 30_000, "{steps} check steps for 30k blocks");
}

#[derive(Debug, Clone)]
enum Op {
    Push(u8),
    Prepend(u8),
    /// Message (by rank among current rows), new block count.
    Update(u8, u8),
    Remove(u8),
    SelectAll,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (1u8..4).prop_map(Op::Push),
        (1u8..4).prop_map(Op::Prepend),
        (any::<u8>(), 0u8..4).prop_map(|(r, n)| Op::Update(r, n)),
        any::<u8>().prop_map(Op::Remove),
        Just(Op::SelectAll),
    ]
}

fn blocks_message(key: u64, count: u8) -> DocumentRow {
    let texts: Vec<String> = (0..count).map(|b| format!("{key}.{b}")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    message_with(key, &refs)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Every edit keeps rows, block order, and selection consistent; the
    /// integrity check runs after each mutation in debug builds, and once
    /// more here.
    #[test]
    fn edits_keep_rows_blocks_and_selection_consistent(ops in prop::collection::vec(op(), 1..40)) {
        let mut view: Document<GridGeometry> = Document::new(grid_style());
        let (mut next, mut first) = (1_000u64, 1_000u64);
        for op in ops {
            match op {
                Op::Push(n) => {
                    for _ in 0..n {
                        view.push(&blocks_message(next, 2)).unwrap();
                        next += 1;
                    }
                }
                Op::Prepend(n) => {
                    let history: Vec<_> =
                        (0..n as u64).map(|i| blocks_message(first - n as u64 + i, 2)).collect();
                    first -= n as u64;
                    view.prepend(&history).unwrap();
                }
                Op::Update(..) | Op::Remove(..) if view.is_empty() => {}
                Op::Update(rank, count) => {
                    let keys = view.list().rows().keys();
                    let key = keys[rank as usize % keys.len()];
                    view.update(&blocks_message(key.0, count)).unwrap();
                }
                Op::Remove(rank) => {
                    let keys = view.list().rows().keys();
                    let key = keys[rank as usize % keys.len()];
                    view.remove(key).unwrap();
                }
                Op::SelectAll => view.select_all(),
            }
            prop_assert_eq!(view.verify_integrity(), Ok(()));
        }
    }
}

// ---------------------------------------------------------------------------
// The element, with real text layout
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Ev(DocumentEvent);

impl From<Ev> for Action {
    fn from(ev: Ev) -> Self {
        Action::new(ev)
    }
}

struct Painted {
    scene: Scene,
    accessibility: AccessibilityFrame,
    regions: Vec<crate::element::SelectableTextRegion>,
    router: InputRouter,
}

/// Prepares `view` with real layouts and paints its element `top`
/// pixels below the window's top edge.
fn paint(
    view: &mut Document,
    messages: &HashMap<RowKey, DocumentRow>,
    size: (f32, f32),
    top: f32,
) -> Painted {
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    let font_size = view.style().font_size;
    view.prepare(
        size.0,
        size.1,
        0,
        messages,
        &mut TextMeasurer::new(&mut text, &mut layouts, font_size, 1.0),
    );
    let theme = Theme::default_dark();
    let element = view.element(messages, &theme, |ev| Ev(ev).into());
    let (w, h) = (size.0, size.1 + top);
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    cx.accessibility = AccessibilityFrame::new(w, h);
    cx.semantic = SemanticFrame::new(w, h);
    let mut root = div()
        .w(w)
        .h(h)
        .flex_col()
        .child(div().w(w).h(top).flex_shrink_0())
        .child(element)
        .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, w, h);
    let regions = std::mem::take(&mut cx.selectable_text_runs);
    let accessibility = std::mem::take(&mut cx.accessibility);
    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    Painted {
        scene,
        accessibility,
        regions,
        router,
    }
}

fn real_document(n: u64) -> HashMap<RowKey, DocumentRow> {
    (0..n)
        .map(|i| {
            let mut m = message_with(
                i,
                &[&format!(
                    "Message {i} wraps across a few lines when the column is narrow enough."
                )],
            );
            if i.is_multiple_of(3) {
                m.blocks.push(Block::code(
                    BlockKey(i * 10 + 5),
                    ["fn main() {", "    run();", "}"]
                        .iter()
                        .map(|line| vec![crate::element::StyledSpan::plain(*line)])
                        .collect(),
                ));
            }
            (m.key, m)
        })
        .collect()
}

fn real_view(messages: &HashMap<RowKey, DocumentRow>) -> Document {
    let mut keys: Vec<&RowKey> = messages.keys().collect();
    keys.sort();
    let mut view = Document::new(DocumentStyle::for_font_size(14.0));
    view.extend(keys.into_iter().map(|k| &messages[k])).unwrap();
    view
}

#[test]
fn measured_blocks_match_the_painted_text_elements() {
    let messages = real_document(12);
    let mut view = real_view(&messages);
    view.set_scroll_offset(0.0);

    let painted = paint(&mut view, &messages, (260.0, 600.0), 0.0);

    let fmt = |key: u64, x: f32, y: f32, h: f32, ox: f32, oy: f32| {
        format!("{key} {x:.0},{y:.0} h{h:.0} text@{ox:.0},{oy:.0}")
    };
    let expected: Vec<String> = view
        .visible_blocks()
        .iter()
        .filter(|b| b.rect.y < 600.0 && b.rect.y + b.rect.height > 0.0)
        .map(|b| {
            let r = b.rect;
            let (ox, oy) = b.geometry.text_origin;
            fmt(b.key.0, r.x, r.y, r.height, r.x + ox, r.y + oy)
        })
        .collect();
    let painted: Vec<String> = painted
        .regions
        .iter()
        .filter(|r| r.bounds.y < 600.0 && r.bounds.y + r.bounds.height > 0.0)
        .map(|r| {
            let b = r.bounds;
            fmt(
                r.source_key,
                b.x,
                b.y,
                b.height,
                r.text_origin.0,
                r.text_origin.1,
            )
        })
        .collect();
    assert!(
        expected.iter().any(|line| line.starts_with("5 ")),
        "{expected:?}"
    );
    assert_eq!(painted, expected);
}

#[test]
fn rows_publish_their_position_among_all_rows_and_their_block_text() {
    let messages = real_document(50);
    let mut view = real_view(&messages);
    let offset = view.list().rows().offset_of(RowKey(25)).unwrap();
    view.set_scroll_offset(offset);
    paint(&mut view, &messages, (400.0, 600.0), 0.0);
    // Rows above were measured on the way; settle the anchor at row 25.
    let offset = view.list().rows().offset_of(RowKey(25)).unwrap();
    view.set_scroll_offset(offset);

    let painted = paint(&mut view, &messages, (400.0, 600.0), 0.0);

    let update = painted.accessibility.tree_update("Test", None);
    let item = update
        .nodes
        .iter()
        // AccessKit's index is 0-based; screen readers announce 26.
        .find(|(_, n)| n.role() == accesskit::Role::ListItem && n.position_in_set() == Some(25))
        .map(|(_, n)| n)
        .expect("row 25 is published");
    let texts: Vec<&str> = item
        .children()
        .iter()
        .filter_map(|id| update.nodes.iter().find(|(nid, _)| nid == id))
        .filter_map(|(_, n)| n.value())
        .collect();
    // AT-SPI reads the set size from the list, not the items.
    let list_size = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == accesskit::Role::List)
        .and_then(|(_, n)| n.size_of_set());
    assert_eq!(
        (item.label(), item.size_of_set(), list_size, texts),
        (
            Some("author 25"),
            Some(50),
            Some(50),
            vec!["Message 25 wraps across a few lines when the column is narrow enough."]
        )
    );
}

#[test]
fn routed_drag_selects_from_one_message_into_another() {
    let messages = real_document(6);
    let mut view = real_view(&messages);
    view.set_scroll_offset(0.0);
    // The view sits 50px down, so window and local coordinates differ.
    let top = 50.0;
    let mut painted = paint(&mut view, &messages, (400.0, 600.0), top);
    let block = |key: u64| {
        view.visible_blocks()
            .iter()
            .find(|b| b.key == BlockKey(key))
            .map(|b| b.rect)
            .unwrap()
    };
    // Left of message 1's text, then past the end of message 2's last line.
    let (from, to) = (block(10), block(20));

    let mut actions = painted
        .router
        .pointer_down(from.x - 5.0, top + from.y + 5.0, &mut None)
        .actions;
    actions.extend(
        painted
            .router
            .pointer_move(to.x + to.width + 40.0, top + to.y + to.height - 2.0)
            .actions,
    );
    actions.extend(painted.router.pointer_up().actions);
    for action in actions {
        if let Some(Ev(event)) = action.downcast_ref::<Ev>() {
            view.handle(*event);
        }
    }

    assert_eq!(
        view.selected_text(&messages),
        "Message 1 wraps across a few lines when the column is narrow enough.\n\n\
         Message 2 wraps across a few lines when the column is narrow enough."
    );
}

// ---------------------------------------------------------------------------
// Markdown messages
// ---------------------------------------------------------------------------

/// Converts markdown with a fresh block key allocator, so keys count from 0.
fn markdown_message(
    row: u64,
    markdown: &mut MarkdownBlocks,
    source: &str,
    syntax: &mut SyntaxHighlighter,
) -> DocumentRow {
    markdown_message_with(row, markdown, source, syntax, &mut BlockKeys::new())
}

fn markdown_message_with(
    row: u64,
    markdown: &mut MarkdownBlocks,
    source: &str,
    syntax: &mut SyntaxHighlighter,
    keys: &mut BlockKeys,
) -> DocumentRow {
    DocumentRow {
        key: RowKey(row),
        chrome: RowChrome::default(),
        blocks: markdown.blocks(
            &crate::markdown::MarkdownDoc::parse(source),
            syntax,
            &mut ImageStore::new(),
            keys,
        ),
        adornments: Vec::new(),
    }
}

/// One line per block: key, content kind and label, list and quote depth,
/// gutter marker, copy prefix, then the selectable text.
fn dump_blocks(message: &DocumentRow) -> String {
    message
        .blocks
        .iter()
        .map(|block| {
            let kind = match &block.content {
                BlockContent::Prose(_) => "prose".to_owned(),
                BlockContent::Code { label, .. } => {
                    format!("code({})", label.as_deref().unwrap_or(""))
                }
                BlockContent::Rule => "rule".to_owned(),
                BlockContent::Table(t) => format!("table({}x{})", t.rows, t.columns),
                BlockContent::Image { src, state } => format!("image({src}, {state:?})"),
            };
            let s = &block.style;
            format!(
                "{} {kind} l{} q{} [{}] {:?} {:?}",
                block.key.0,
                s.list_depth,
                s.quote_depth,
                s.marker.as_deref().unwrap_or(""),
                &*s.copy_prefix,
                block.text(),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Texts of the content spans that take a syntax color.
fn highlighted_runs(block: &Block) -> Vec<String> {
    let (Some(spans), Some(tones)) = (block.content.spans(), block.tones()) else {
        return Vec::new();
    };
    spans
        .iter()
        .zip(tones)
        .filter(|(_, tone)| matches!(tone, SpanTone::Syntax(_)))
        .map(|(span, _)| span.text.clone())
        .collect()
}

#[test]
fn markdown_blocks_become_separate_keyed_blocks_with_markers_and_prefixes() {
    let source = "## Setup\n\n- one\n  - nested\n1. first\n\n> quoted\n\n\
                  | a | bb |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```\n\n---";

    let message = markdown_message(
        7,
        &mut MarkdownBlocks::new(),
        source,
        &mut SyntaxHighlighter::new(),
    );

    assert_eq!(
        dump_blocks(&message),
        [
            r###"0 prose l0 q0 [] "## " "Setup""###,
            r###"1 prose l1 q0 [•] "- " "one""###,
            r###"2 prose l2 q0 [•] "  - " "nested""###,
            r###"3 prose l1 q0 [1.] "1. " "first""###,
            r###"4 prose l0 q1 [] "> " "quoted""###,
            r###"5 table(2x2) l0 q0 [] "" "| a | bb |\n| --- | --- |\n| 1 | 2 |""###,
            r###"6 code(rust) l0 q0 [] "" "fn main() {}""###,
            r###"7 rule l0 q0 [] "" "---""###,
        ]
        .join("\n")
    );
}

#[test]
fn drag_from_a_heading_across_a_list_into_code_copies_markers_and_source() {
    let mut syntax = SyntaxHighlighter::new();
    let source = "## Setup\n\n- one\n- two\n\n```rust\nfn main() {\n    run();\n}\n```";
    let message = markdown_message(0, &mut MarkdownBlocks::new(), source, &mut syntax);
    let (heading, code) = (message.blocks[0].key.0, message.blocks[3].key.0);
    let mut doc = Doc::new([message]);

    doc.press(doc.point(heading, 0));
    doc.drag(doc.point(code, 5));
    doc.release();

    assert_eq!(doc.copy(), "## Setup\n\n- one\n- two\n\nfn ma");
}

#[test]
fn streaming_markdown_reshapes_only_the_last_block() {
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    let mut syntax = SyntaxHighlighter::new();
    let mut keys = BlockKeys::new();
    let mut markdown = MarkdownBlocks::new();
    let mut source = String::from("# Title\n\nFirst paragraph.\n\n- a\n- b\n\nStreaming");
    let mut messages = HashMap::new();
    let mut view = Document::new(DocumentStyle::for_font_size(14.0));
    let message = markdown_message_with(0, &mut markdown, &source, &mut syntax, &mut keys);
    view.push(&message).unwrap();
    messages.insert(message.key, message);
    let mut frame = |view: &mut Document, messages: &HashMap<_, _>| {
        let mut measurer = TextMeasurer::new(&mut text, &mut layouts, 14.0, 1.0);
        view.prepare(400.0, 600.0, 0, messages, &mut measurer);
        view.visible_blocks()
            .iter()
            .map(|b| b.geometry.layout.clone().unwrap())
            .collect::<Vec<_>>()
    };
    let mut before = frame(&mut view, &messages);

    let mut reshaped = Vec::new();
    for word in [" more", " words", " arrive"] {
        source.push_str(word);
        let message = markdown_message_with(0, &mut markdown, &source, &mut syntax, &mut keys);
        view.update(&message).unwrap();
        messages.insert(message.key, message);
        let after = frame(&mut view, &messages);
        let changed: Vec<usize> = (0..after.len())
            .filter(|&i| !before.get(i).is_some_and(|b| Arc::ptr_eq(b, &after[i])))
            .collect();
        reshaped.push(changed);
        before = after;
    }

    assert_eq!(reshaped, [[4], [4], [4]]);
}

#[test]
fn unknown_fence_language_renders_plain_with_its_label() {
    let mut syntax = SyntaxHighlighter::new();
    let mut markdown = MarkdownBlocks::new();
    let source = "```klingon\nqapla' \"batlh\" fn\n```";
    markdown_message(0, &mut markdown, source, &mut syntax);
    syntax.finish_pending();

    let message = markdown_message(0, &mut markdown, source, &mut syntax);

    let block = &message.blocks[0];
    let label = match &block.content {
        BlockContent::Code { label, .. } => label.as_deref().map(str::to_owned),
        _ => None,
    };
    assert_eq!(
        (label, highlighted_runs(block)),
        (Some("klingon".to_owned()), Vec::<String>::new())
    );
}

/// A highlighter with the Rust pack `syntax-pack build rust` writes, or
/// `None` (the test skips) when it has not been built.
#[cfg(feature = "syntax")]
fn rust_highlighter() -> Option<SyntaxHighlighter> {
    let mut syntax = SyntaxHighlighter::new();
    syntax.set_grammar_store(quark_syntax::testing::store_with("rust")?);
    Some(syntax)
}

#[cfg(feature = "syntax")]
#[test]
fn rust_code_block_is_colored_once_its_highlight_arrives() {
    let Some(mut syntax) = rust_highlighter() else {
        return;
    };
    let mut markdown = MarkdownBlocks::new();
    let source = "```rust\nfn main() { let s = \"hi\"; }\n```";
    let first = markdown_message(0, &mut markdown, source, &mut syntax);

    syntax.finish_pending();
    let second = markdown_message(0, &mut markdown, source, &mut syntax);

    assert_eq!(
        (
            highlighted_runs(&first.blocks[0]),
            highlighted_runs(&second.blocks[0])
        ),
        (
            vec![],
            vec!["fn".into(), "main".into(), "let".into(), "\"hi\"".into()]
        )
    );
}

#[cfg(feature = "syntax")]
#[test]
fn highlight_arrival_reports_the_block_it_is_for() {
    let mut syntax = SyntaxHighlighter::new();
    let message = markdown_message(
        0,
        &mut MarkdownBlocks::new(),
        "```rust\nfn main() {}\n```",
        &mut syntax,
    );

    assert_eq!(syntax.finish_pending(), vec![message.blocks[0].key]);
}

/// A markdown view holding message 0 with `source`.
fn markdown_document(source: &str) -> MarkdownDocument {
    let mut md = MarkdownDocument::new(DocumentStyle::for_font_size(14.0));
    md.push(MarkdownEntry {
        row: RowKey(0),
        chrome: RowChrome::default(),
        markdown: source.to_owned(),
    })
    .unwrap();
    md
}

// Catches highlighter state outliving its blocks: removing a message must
// drop the highlights of its code blocks.
#[cfg(feature = "syntax")]
#[test]
fn removing_a_message_forgets_its_highlights() {
    let mut md = markdown_document("```rust\nfn a() {}\n```\n\n```rust\nfn b() {}\n```");
    md.finish_highlights();
    let kept = md.highlighter().len();

    md.remove(RowKey(0)).unwrap();

    assert_eq!((kept, md.highlighter().len()), (2, 0));
}

// Catches truncation keeping the highlights of blocks a message lost.
#[cfg(feature = "syntax")]
#[test]
fn shrinking_a_message_forgets_the_highlights_of_dropped_blocks() {
    let mut md = markdown_document("```rust\nfn a() {}\n```\n\n```rust\nfn b() {}\n```");
    md.finish_highlights();

    md.set_markdown(RowKey(0), "```rust\nfn a() {}\n```")
        .unwrap();

    assert_eq!(md.highlighter().len(), 1);
}

// Catches a dead worker leaving every later block plain, or hanging the
// caller waiting for results that never come.
#[cfg(feature = "syntax")]
#[test]
fn dead_highlight_worker_is_replaced_and_does_not_hang() {
    let Some(mut syntax) = rust_highlighter() else {
        return;
    };
    let mut markdown = MarkdownBlocks::new();
    let source = "```rust\nfn main() {}\n```";
    markdown_message(0, &mut markdown, source, &mut syntax);
    syntax.kill_worker();

    syntax.finish_pending();
    let message = markdown_message(0, &mut markdown, source, &mut syntax);

    assert_eq!(
        highlighted_runs(&message.blocks[0]),
        vec!["fn".to_owned(), "main".to_owned()]
    );
}

// Catches a batch with a repeated row key being half adopted, or the
// error naming another key: history loads must be all or nothing.
#[test]
fn a_batch_repeating_a_row_key_is_rejected_whole() {
    let entry = |row: u64| MarkdownEntry {
        row: RowKey(row),
        chrome: RowChrome::default(),
        markdown: format!("row {row}"),
    };
    // (batch, error) against a document already holding row 0.
    let cases: [(&[u64], u64); 2] = [(&[1, 2, 3, 2, 4], 2), (&[5, 0, 6], 0)];
    for (batch, repeated) in cases {
        let mut md = markdown_document("row 0");

        let result = md.extend(batch.iter().map(|&row| entry(row)));

        assert_eq!(
            (result, md.len()),
            (Err(RowError::DuplicateKey(RowKey(repeated))), 1),
            "{batch:?}"
        );
    }
}

#[test]
fn facade_streams_markdown_into_selectable_blocks() {
    let mut md = markdown_document("# Answer\n\nfirst");

    md.set_markdown(RowKey(0), "# Answer\n\nfirst words\n\n- item")
        .unwrap();
    md.document_mut().select_all();

    assert_eq!(md.selected_text(), "# Answer\n\nfirst words\n\n- item");
}

/// Paints `md` with `theme` and returns its text regions and scene.
fn paint_markdown(
    md: &mut MarkdownDocument,
    theme: &Theme,
) -> (Vec<crate::element::SelectableTextRegion>, Scene) {
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    md.prepare(
        400.0,
        300.0,
        0,
        &mut TextMeasurer::new(&mut text, &mut layouts, 14.0, 1.0),
    );
    let element = md.element(theme, |ev| Ev(ev).into());
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(theme, 1.0, &mut text, &mut layouts, None, &signals);
    cx.semantic = SemanticFrame::new(400.0, 300.0);
    let mut scene = Scene::default();
    render_element(&mut element.into_any(), &mut scene, &mut cx, 400.0, 300.0);
    (std::mem::take(&mut cx.selectable_text_runs), scene)
}

// Catches theme colors baked into blocks, which needed every message
// rebuilt on a theme change.
#[test]
fn theme_change_recolors_blocks_without_rebuilding_them() {
    let mut md = markdown_document("see ![a chart](c.png)");
    let (dark, light) = (Theme::default_dark(), Theme::default_light());

    let colors = |(_, scene): (_, Scene)| {
        scene
            .primitives
            .iter()
            .find_map(|p| match p {
                quark_render::Primitive::RichTextRun(text) => Some(text.span_colors.to_vec()),
                _ => None,
            })
            .unwrap_or_default()
    };
    let on_dark = colors(paint_markdown(&mut md, &dark));
    let on_light = colors(paint_markdown(&mut md, &light));

    assert_eq!(on_dark[1], dark.colors.text_muted);
    assert_eq!(on_light[1], light.colors.text_muted);
}

// Regression: span weights override SelectableText's base weight, so
// headings rendered at normal weight.
#[test]
fn heading_weights_reach_the_glyphs() {
    let mut md = markdown_document("# Title\n\n### Small\n\nplain");

    let (regions, _) = paint_markdown(&mut md, &Theme::default_dark());

    let weights: Vec<(String, u16)> = regions
        .iter()
        .map(|r| (r.text.to_string(), r.layout.glyphs().font_weight[0].0))
        .collect();
    let expected = [("Title", 700), ("Small", 600), ("plain", 450)];
    let expected: Vec<(String, u16)> = expected.iter().map(|(t, w)| (t.to_string(), *w)).collect();
    assert_eq!(weights, expected);
}

#[test]
fn link_click_emits_the_document_link_action() {
    #[derive(Debug, Clone, PartialEq)]
    struct Open(Arc<str>);
    let mut md = markdown_document("see [docs](https://example.com/docs) now");
    let theme = Theme::default_dark();
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    md.prepare(
        400.0,
        300.0,
        0,
        &mut TextMeasurer::new(&mut text, &mut layouts, 14.0, 1.0),
    );
    let element = md
        .element(&theme, |ev| Ev(ev).into())
        .on_link(|url| Action::new(Open(url.clone())));
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    cx.semantic = SemanticFrame::new(400.0, 300.0);
    render_element(
        &mut element.into_any(),
        &mut Scene::default(),
        &mut cx,
        400.0,
        300.0,
    );
    let region = cx.selectable_text_runs[0].clone();
    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    let at = region.text.find("docs").unwrap_or(0);
    let rect = region
        .layout
        .selection_rects(at..at + 4)
        .next()
        .expect("glyph rect");
    let (x, y) = (
        region.text_origin.0 + rect.x + rect.width / 2.0,
        region.text_origin.1 + rect.y + rect.height / 2.0,
    );

    let actions = router.pointer_down(x, y, &mut None).actions;

    let opened: Vec<&Open> = actions
        .iter()
        .filter_map(|a| a.downcast_ref::<Open>())
        .collect();
    assert_eq!(opened, [&Open(Arc::from("https://example.com/docs"))]);
}

// Catches document drag selection mapped through untransformed
// coordinates: under a parent scaled about its center, dragging across
// "brave" where it is painted selects "brave".
#[test]
fn drag_selection_follows_a_scaled_document() {
    let mut md = markdown_document("hello brave world");
    let theme = Theme::default_dark();
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    md.prepare(
        400.0,
        300.0,
        0,
        &mut TextMeasurer::new(&mut text, &mut layouts, 14.0, 1.0),
    );
    let element = md.element(&theme, |ev| Ev(ev).into());
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    cx.semantic = SemanticFrame::new(400.0, 300.0);
    let mut root = div().w(400.0).h(300.0).scale(1.5).child(element).into_any();
    render_element(&mut root, &mut Scene::default(), &mut cx, 400.0, 300.0);
    let region = cx.selectable_text_runs[0].clone();
    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    // Window points on the edges of "brave", halfway down its line, scaled
    // about the window's center.
    let scaled = quark::Transform2D::scale(1.5, 1.5).around(200.0, 150.0);
    let at = |offset: usize| {
        let caret = region.layout.caret(offset);
        scaled.apply(
            region.text_origin.0 + caret.x,
            region.text_origin.1 + caret.y + caret.height / 2.0,
        )
    };
    let ((x0, y0), (x1, y1)) = (at(6), at(11));

    let mut actions = router.pointer_down(x0, y0, &mut None).actions;
    actions.extend(router.pointer_move(x1, y1).actions);
    actions.extend(router.pointer_up().actions);
    for action in &actions {
        if let Some(Ev(event)) = action.downcast_ref::<Ev>() {
            md.handle(*event);
        }
    }

    assert_eq!(md.selected_text(), "brave");
}

// Catches markdown span flags not reaching the painted decorations.
#[test]
fn strikethrough_markdown_paints_a_line_through_its_run() {
    let mut md = markdown_document("keep ~~gone~~ keep");

    let (regions, scene) = paint_markdown(&mut md, &Theme::default_dark());

    let region = &regions[0];
    let at = region.text.find("gone").unwrap_or(0);
    let run = region
        .layout
        .selection_rects(at..at + 4)
        .next()
        .expect("glyph rect");
    let (x0, x1) = (
        region.text_origin.0 + run.x,
        region.text_origin.0 + run.right(),
    );
    let mid_y = region.text_origin.1 + run.y + run.height / 2.0;
    let lines: Vec<quark_render::Rect> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            quark_render::Primitive::Rect(r) => Some(r.rect),
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    let line = lines[0];
    assert!(
        line.x >= x0 - 0.01 && line.right() <= x1 + 0.01,
        "{line:?} vs {x0}..{x1}"
    );
    assert!(
        (line.y - mid_y).abs() < 7.0,
        "{line:?} not across the text at {mid_y}"
    );
}

// ---------------------------------------------------------------------------
// Cached rows
// ---------------------------------------------------------------------------

/// Paints documents frame after frame through one element cache, as a
/// window does, and dumps what a frame published.
struct CachedPainter {
    text: TextSystem,
    layouts: LayoutCache,
    cache: crate::element::ElementCache,
}

impl CachedPainter {
    fn new() -> Self {
        Self {
            text: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
            cache: crate::element::ElementCache::new(),
        }
    }

    /// The text regions (`key "text" @x,y`) and the accessibility states
    /// of one frame, painted with the cache or without it.
    fn frame(
        &mut self,
        view: &mut Document,
        messages: &HashMap<RowKey, DocumentRow>,
        size: (f32, f32),
        cached: bool,
    ) -> String {
        let font_size = view.style().font_size;
        view.prepare(
            size.0,
            size.1,
            0,
            messages,
            &mut TextMeasurer::new(&mut self.text, &mut self.layouts, font_size, 1.0),
        );
        let theme = Theme::default_dark();
        let element = view.element(messages, &theme, |ev| Ev(ev).into());
        let signals = SignalStore::new();
        let mut cx = ElementContext::new(
            &theme,
            1.0,
            &mut self.text,
            &mut self.layouts,
            None,
            &signals,
        );
        if cached {
            cx = cx.with_element_cache(&mut self.cache);
        }
        cx.accessibility = AccessibilityFrame::new(size.0, size.1);
        cx.semantic = SemanticFrame::new(size.0, size.1);
        let mut scene = Scene::default();
        render_element(&mut element.into_any(), &mut scene, &mut cx, size.0, size.1);
        let mut out = String::new();
        // Filled rects: row backgrounds, selection, and find highlights.
        for p in &scene.expanded() {
            let (r, c) = match p {
                quark_render::Primitive::Rect(p) => (p.rect, p.color),
                quark_render::Primitive::RoundedRect(p) => (p.rect, p.color),
                _ => continue,
            };
            out.push_str(&format!(
                "rect {:.0},{:.0} {:.0}x{:.0} {:?}\n",
                r.x, r.y, r.width, r.height, c
            ));
        }
        for r in &cx.selectable_text_runs {
            out.push_str(&format!(
                "{} {:?} @{:.0},{:.0}\n",
                r.source_key, r.text, r.bounds.x, r.bounds.y
            ));
        }
        // Author ids of some text nodes embed the position they were first
        // painted at, which a replayed node keeps; compare the rest.
        let update = cx.accessibility.tree_update("Test", None);
        for line in crate::accessibility::dump_accessibility_states(&update).lines() {
            out.push_str(line.split_once(" | ").map_or(line, |(_, rest)| rest));
            out.push('\n');
        }
        out
    }
}

// Catches a row replayed from the cache after something it shows changed:
// each edit is followed by a cached frame that must match an uncached one.
#[test]
fn cached_rows_paint_exactly_what_an_uncached_frame_paints_after_each_edit() {
    let mut messages = real_document(30);
    let mut view = real_view(&messages);
    view.set_scroll_offset(0.0);
    let mut painter = CachedPainter::new();
    let mut size = (400.0, 500.0);
    painter.frame(&mut view, &messages, size, true);

    type Edit = fn(&mut Document, &mut HashMap<RowKey, DocumentRow>, &mut (f32, f32));
    let edits: &[(&str, Edit)] = &[
        ("nothing", |_, _, _| {}),
        ("stream into row 1", |t, m, _| {
            let message = m.get_mut(&RowKey(1)).unwrap();
            message.blocks[0] = Block::plain(BlockKey(10), "Streamed text for row one.");
            t.update(message).unwrap();
        }),
        ("select inside row 2", |t, _, _| {
            t.set_selection(Some(Selection::new(
                SelectionPoint::new(BlockKey(20), 3),
                SelectionPoint::new(BlockKey(20), 12),
            )));
        }),
        ("scroll", |t, _, _| {
            t.scroll_by(70.0);
        }),
        ("find", |t, m, _| t.set_find_query("lines", m)),
        ("narrow", |_, _, size| size.0 = 300.0),
        ("append a row", |t, m, _| {
            let message = message_with(30, &["A new message at the end."]);
            t.push(&message).unwrap();
            m.insert(message.key, message);
        }),
    ];
    for (name, edit) in edits {
        edit(&mut view, &mut messages, &mut size);
        let cached = painter.frame(&mut view, &messages, size, true);
        let mut fresh = view.without_element_memory();
        let uncached = painter.frame(&mut fresh, &messages, size, false);
        assert_eq!(cached, uncached, "after {name}");
    }
}

// ---------------------------------------------------------------------------
// Find
// ---------------------------------------------------------------------------

impl Doc {
    /// `"<block>:<start>..<end>"` per match, the current one starred.
    fn find_dump(&self) -> String {
        let Some(find) = self.view.find() else {
            return "closed".to_owned();
        };
        find.matches()
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let star = if find.current_index() == Some(i) {
                    "*"
                } else {
                    ""
                };
                format!("{star}{}:{}..{}", m.block.0, m.range.start, m.range.end)
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn find(&mut self, query: &str) {
        self.view.set_find_query(query, &self.messages);
    }

    /// Where the current match's block sits in the viewport, if it is
    /// materialized.
    fn current_match_top(&self) -> Option<f32> {
        let m = self.view.find()?.current()?.clone();
        let block = self
            .view
            .visible_blocks()
            .iter()
            .find(|b| b.key == m.block)?;
        Some(block.rect.y + (m.range.start.get() / 40) as f32 * LINE_H)
    }
}

#[test]
fn find_matches_every_block_in_document_order_ignoring_case() {
    let mut doc = Doc::new([
        message_with(0, &["Second thoughts", "no"]),
        message_with(1, &["a SECOND and a second"]),
    ]);

    doc.find("second");

    assert_eq!(doc.find_dump(), "*0:0..6 10:2..8 10:15..21");
}

#[test]
fn find_picks_up_matches_streaming_in_and_keeps_the_current_one() {
    let mut doc = Doc::new([
        message_with(0, &["one word"]),
        message_with(1, &["growing"]),
    ]);
    doc.find("word");

    doc.stream(1, 0, "growing word by word");
    doc.frame();

    assert_eq!(doc.find_dump(), "*0:4..8 10:8..12 10:16..20");
}

#[test]
fn find_drops_matches_of_removed_blocks() {
    let mut doc = Doc::new([
        message_with(0, &["word"]),
        message_with(1, &["word", "word"]),
    ]);
    doc.find("word");
    doc.view.find_next(ScrollAlign::Center);
    doc.view.find_next(ScrollAlign::Center);

    let message = message_with(1, &["word"]);
    doc.view.update(&message).unwrap();
    doc.messages.insert(message.key, message);
    doc.frame();

    // The current match left with its block; the next one after it wraps.
    assert_eq!(doc.find_dump(), "*0:0..4 10:0..4");
}

#[test]
fn next_and_prev_wrap_and_scroll_the_match_to_the_viewport_center() {
    let mut doc = Doc::rows(40);
    doc.scroll_to(0.0);
    doc.find("first");

    // From the first match, previous wraps to the last row's block.
    let prev = doc.view.find_prev(ScrollAlign::Center).unwrap();
    doc.frame();
    let at_last = doc.current_match_top();
    let next = doc.view.find_next(ScrollAlign::Center).unwrap();
    doc.frame();

    assert_eq!(
        (prev.block.0, next.block.0, doc.view.scroll_offset()),
        (390, 0, 0.0)
    );
    // The last row cannot scroll to the center; it sits fully in view.
    let top = at_last.unwrap();
    assert!(top >= 0.0 && top + LINE_H <= doc.size.1, "{top}");
}

#[test]
fn next_centers_a_match_in_the_middle_of_the_document() {
    let mut doc = Doc::rows(40);
    doc.scroll_to(0.0);
    doc.find("m20 second");

    doc.view.find_next(ScrollAlign::Center).unwrap();
    doc.frame();

    let top = doc.current_match_top().unwrap();
    assert_eq!(top + LINE_H * 0.5, doc.size.1 * 0.5);
}

#[test]
fn find_paints_a_highlight_per_match_and_marks_the_current_one() {
    let messages = real_document(4);
    let mut view = real_view(&messages);
    view.set_scroll_offset(0.0);
    view.set_find_query("Message", &messages);
    let theme = Theme::default_dark();
    let (plain, current) = (
        format!("{:?}", theme.colors.search_match_bg),
        format!("{:?}", theme.colors.search_match_active_bg),
    );

    let frame = CachedPainter::new().frame(&mut view, &messages, (400.0, 600.0), true);

    let count = |color: &str| frame.lines().filter(|l| l.ends_with(color)).count();
    assert_eq!((count(&current), count(&plain)), (1, 3));
}

// ---------------------------------------------------------------------------
// Images
// ---------------------------------------------------------------------------

/// A loader that decodes `"<w>x<h>.png"` into a gray image of that size
/// and fails on anything else.
fn sized_loader() -> ImageLoader {
    Arc::new(|src: &str| {
        let (w, h) = src.strip_suffix(".png")?.split_once('x')?;
        let (width, height) = (w.parse().ok()?, h.parse().ok()?);
        Some(LoadedImage::Rgba {
            width,
            height,
            pixels: vec![128; width as usize * height as usize * 4],
        })
    })
}

/// Prepares `md` at 400x300 with real text layouts.
fn prepare_markdown(md: &mut MarkdownDocument, text: &mut TextSystem, layouts: &mut LayoutCache) {
    md.prepare(
        400.0,
        300.0,
        0,
        &mut TextMeasurer::new(text, layouts, 14.0, 1.0),
    );
}

/// Height of the first image block on screen, and whether its pixels
/// arrived.
fn image_block(md: &MarkdownDocument) -> (f32, bool) {
    let t = md.document();
    let visible = t
        .visible_blocks()
        .iter()
        .find(|b| md.rows()[&b.row].blocks[b.index].content_kind() == "image")
        .expect("an image block is on screen");
    let block = &md.rows()[&visible.row].blocks[visible.index];
    let ready = matches!(
        block.content,
        BlockContent::Image {
            state: ImageState::Ready(_),
            ..
        }
    );
    (visible.rect.height, ready)
}

impl Block {
    fn content_kind(&self) -> &'static str {
        match self.content {
            BlockContent::Image { .. } => "image",
            _ => "text",
        }
    }
}

// Each case: the image's size, the size the app hinted, and the block's
// height before and after the pixels arrive (the column is 371 points
// wide, so wider images scale down). Pixels twice the hint are a 2x image
// shown at the hinted size; other mismatches show at their pixels.
#[test]
fn image_blocks_reserve_their_height_and_scale_to_the_column() {
    let placeholder = (14.0 * IMAGE_PLACEHOLDER_HEIGHT).ceil();
    let cases = [
        ("200x100.png", Some((200u32, 100u32)), 100.0f32, 100.0f32),
        ("742x100.png", Some((742, 100)), 50.0, 50.0),
        ("200x100.png", None, placeholder, 100.0),
        ("400x200.png", Some((200, 100)), 100.0, 100.0),
        ("300x150.png", Some((200, 100)), 100.0, 150.0),
    ];
    for (src, hint, before, after) in cases {
        let mut md = markdown_document(&format!("![chart]({src})"));
        let (mut text, mut layouts) = (
            TextSystem::vendored_only(&Default::default()),
            LayoutCache::default(),
        );
        if let Some((w, h)) = hint {
            md.hint_image_size(src, w, h);
        }
        md.set_image_loader(sized_loader());
        prepare_markdown(&mut md, &mut text, &mut layouts);
        let loading = image_block(&md);
        md.finish_images();
        prepare_markdown(&mut md, &mut text, &mut layouts);
        let loaded = image_block(&md);

        assert_eq!(
            (loading, loaded),
            ((before, false), (after, true)),
            "{src} hinted {hint:?}"
        );
    }
}

#[test]
fn an_image_resolving_above_the_view_does_not_move_the_rows_on_screen() {
    let mut md = MarkdownDocument::new(DocumentStyle::for_font_size(14.0));
    md.set_image_loader(sized_loader());
    md.extend((0..40).map(|i| MarkdownEntry {
        row: RowKey(i),
        chrome: RowChrome::default(),
        markdown: if i == 10 {
            "![tall](100x600.png)".to_owned()
        } else {
            format!("Message {i} with a line of text.")
        },
    }))
    .unwrap();
    let (mut text, mut layouts) = (
        TextSystem::vendored_only(&Default::default()),
        LayoutCache::default(),
    );
    prepare_markdown(&mut md, &mut text, &mut layouts);
    md.finish_measures();
    // Put row 12 at the top: the image row sits in the overscan above.
    let top = |md: &MarkdownDocument, row: u64| {
        let t = md.document();
        t.list().rows().offset_of(RowKey(row)).unwrap() - t.scroll_offset()
    };
    let offset = md.document().list().rows().offset_of(RowKey(12)).unwrap() + 5.0;
    md.document_mut().set_scroll_offset(offset);
    prepare_markdown(&mut md, &mut text, &mut layouts);
    let image_row = md.document().list().rows().height_of(RowKey(10)).unwrap();
    let before = top(&md, 12);

    md.finish_images();
    prepare_markdown(&mut md, &mut text, &mut layouts);

    let grown = md.document().list().rows().height_of(RowKey(10)).unwrap() - image_row;
    assert_eq!((top(&md, 12), grown > 400.0), (before, true));
}

#[test]
fn an_image_without_pixels_shows_its_alt_text_and_names_its_node() {
    let mut no_loader = markdown_document("![a sales chart](chart.png)");
    let mut loaded = markdown_document("![a sales chart](30x20.png)");
    loaded.set_image_loader(sized_loader());
    loaded.finish_images();
    let theme = Theme::default_dark();

    let (regions, _) = paint_markdown(&mut no_loader, &theme);
    let alt_text: Vec<&str> = regions.iter().map(|r| &*r.text).collect();
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    prepare_markdown(&mut loaded, &mut text, &mut layouts);
    let element = loaded.element(&theme, |ev| Ev(ev).into());
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
    cx.semantic = SemanticFrame::new(400.0, 300.0);
    let mut scene = Scene::default();
    render_element(&mut element.into_any(), &mut scene, &mut cx, 400.0, 300.0);
    let update = cx.accessibility.tree_update("Test", None);
    let image_name = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == accesskit::Role::Image)
        .and_then(|(_, n)| n.label().map(str::to_owned));
    let pixels = scene.primitives.iter().find_map(|p| match p {
        quark_render::Primitive::Image(image) => Some((image.width, image.height)),
        _ => None,
    });

    assert_eq!(
        (alt_text, image_name.as_deref(), pixels),
        (vec!["a sales chart"], Some("a sales chart"), Some((30, 20)))
    );
}

// ---------------------------------------------------------------------------
// Wide blocks
// ---------------------------------------------------------------------------

#[test]
fn a_wide_code_block_scrolls_sideways_and_pointer_hits_follow_it() {
    let long = format!("let row = \"{}\";", "x".repeat(150));
    let mut message = message_with(0, &["Above the code."]);
    message.blocks.push(Block::code(
        BlockKey(5),
        vec![vec![crate::element::StyledSpan::plain(long.as_str())]],
    ));
    let messages: HashMap<RowKey, DocumentRow> = [(message.key, message)].into();
    let mut view = real_view(&messages);
    let mut painter = CachedPainter::new();
    let size = (300.0, 400.0);
    painter.frame(&mut view, &messages, size, true);
    let hit = |t: &Document| {
        let code = t
            .visible_blocks()
            .iter()
            .find(|b| b.key == BlockKey(5))
            .unwrap();
        let (x, y) = (code.rect.x + 60.0, code.rect.y + code.rect.height * 0.5);
        t.point_at(x, y).unwrap().byte
    };
    let before = hit(&view);

    view.scroll_handles[&BlockKey(5)].set_offset(200.0, 0.0);
    painter.frame(&mut view, &messages, size, true);
    let cached = painter.frame(&mut view, &messages, size, true);
    let mut fresh = view.without_element_memory();
    let uncached = painter.frame(&mut fresh, &messages, size, false);
    let after = hit(&view);

    assert_eq!(view.scroll_handles[&BlockKey(5)].offset().0, 200.0);
    assert_eq!(cached, uncached);
    // 200 points of monospace at this size is well over 15 characters.
    assert!(after > before + 15, "{before} -> {after}");
}

// ---------------------------------------------------------------------------
// Row chrome
// ---------------------------------------------------------------------------

const KIND_1_BACKGROUND: Color = Color::rgba(200, 30, 60, 255);

/// Chrome from the row's kind: a "header {kind}" line on every row, and a
/// background behind kind 1 rows only.
struct KindChrome;

impl RowDecorator for KindChrome {
    fn background(&self, chrome: &RowChrome, _theme: &Theme) -> Option<Color> {
        (chrome.kind == 1).then_some(KIND_1_BACKGROUND)
    }

    fn header(&self, chrome: &RowChrome, _width: f32, _theme: &Theme) -> Option<AnyElement> {
        Some(crate::element::text(format!("header {}", chrome.kind)).into_any())
    }
}

/// Rows 0..n with a 20px header band, kind `i % 2`, and no label, so the
/// header text stays in the accessibility tree.
fn chrome_view(n: u64) -> (HashMap<RowKey, DocumentRow>, Document) {
    let mut rows = real_document(n);
    for row in rows.values_mut() {
        row.chrome.label = None;
    }
    let mut view = real_view(&rows);
    view.set_decorator(KindChrome);
    view.set_scroll_offset(0.0);
    (rows, view)
}

// Catches the app's header not being drawn, or drawn outside the band the
// document reserved for it above the row's blocks.
#[test]
fn decorator_header_is_drawn_in_the_band_above_each_rows_blocks() {
    let (rows, mut view) = chrome_view(4);

    let painted = paint(&mut view, &rows, (400.0, 600.0), 0.0);

    let update = painted.accessibility.tree_update("Test", None);
    let mut headers: Vec<String> = update
        .nodes
        .iter()
        .filter(|(_, n)| n.label().is_some_and(|l| l.starts_with("header ")))
        .map(|(_, n)| {
            let b = n.bounds().expect("header has bounds");
            format!("{} y{:.0}", n.label().unwrap(), b.y0)
        })
        .collect();
    headers.sort_by_key(|h| h.split(" y").nth(1).unwrap().parse::<i32>().unwrap());
    let pad_y = view.style().pad_y;
    let expected: Vec<String> = view
        .visible_rows()
        .iter()
        .map(|r| format!("header {} y{:.0}", r.key.0 % 2, r.top + pad_y))
        .collect();
    let first_blocks_below_band = view.visible_rows().iter().all(|r| {
        view.visible_blocks()[r.blocks.clone()]
            .first()
            .is_some_and(|b| b.rect.y >= r.top + pad_y + 20.0)
    });
    assert_eq!((headers, first_blocks_below_band), (expected, true));
}

// Catches a row keeping the chrome it was pushed with: chat_demo's answer
// still read "Assistant (streaming)" after it finished.
#[test]
fn set_chrome_redraws_the_rows_chrome() {
    let mut md = markdown_document("done");
    md.document_mut().set_decorator(KindChrome);
    let theme = Theme::default_dark();
    let tinted = |scene: &Scene| {
        scene.primitives.iter().any(|p| {
            matches!(p, quark_render::Primitive::RoundedRect(rr) if rr.color == KIND_1_BACKGROUND)
        })
    };
    let before = tinted(&paint_markdown(&mut md, &theme).1);

    let chrome = RowChrome {
        kind: 1,
        ..RowChrome::default()
    };
    md.set_chrome(RowKey(0), chrome).unwrap();

    assert_eq!(
        (before, tinted(&paint_markdown(&mut md, &theme).1)),
        (false, true)
    );
}

// Catches the background hook painting the wrong rows, or not spanning the
// whole row.
#[test]
fn decorator_background_fills_exactly_the_rows_it_returns_a_color_for() {
    let (rows, mut view) = chrome_view(4);

    let painted = paint(&mut view, &rows, (400.0, 600.0), 0.0);

    let fmt = |r: &Rect| format!("{:.0},{:.0} {:.0}x{:.0}", r.x, r.y, r.width, r.height);
    let painted: Vec<String> = painted
        .scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            quark_render::Primitive::RoundedRect(rr) if rr.color == KIND_1_BACKGROUND => {
                Some(fmt(&rr.rect))
            }
            _ => None,
        })
        .collect();
    let expected: Vec<String> = view
        .visible_rows()
        .iter()
        .filter(|r| r.key.0 % 2 == 1)
        .map(|r| {
            fmt(&Rect {
                x: 0.0,
                y: r.top,
                width: 400.0,
                height: r.height,
            })
        })
        .collect();
    assert_eq!(painted, expected);
}

// ---------------------------------------------------------------------------
// Row adornments
// ---------------------------------------------------------------------------

/// An adornment whose band holds nothing.
fn blank_adornment(key: u64, slot: AdornmentSlot, height: f32) -> RowAdornment {
    RowAdornment::new(AdornmentKey(key), slot, height, 0, |_| div().into_any())
}

/// What an adornment's builder shows: "row {key}" text and a "Retry {key}"
/// button that emits `Retry(row)`.
#[derive(Debug, Clone, PartialEq)]
struct Retry(u64);

impl From<Retry> for Action {
    fn from(retry: Retry) -> Self {
        Action::new(retry)
    }
}

fn retry_bar(revision: u64) -> RowAdornment {
    RowAdornment::new(
        AdornmentKey(1),
        AdornmentSlot::Start,
        30.0,
        revision,
        |cx| {
            let row = cx.row.0;
            div()
                .w(cx.width)
                .h(cx.height)
                .flex_row()
                .child(crate::element::text(format!("row {row}")))
                .child(
                    div()
                        .w(80.0)
                        .h(cx.height)
                        .on_click(Retry(row))
                        .accessibility_label(format!("Retry {row}")),
                )
                .into_any()
        },
    )
}

/// `Role name` of every published node, without author ids.
fn accessible_names(painted: &Painted) -> Vec<String> {
    let update = painted.accessibility.tree_update("Test", None);
    crate::accessibility::dump_accessibility_states(&update)
        .lines()
        .map(|line| {
            let mut parts = line.split(" | ").skip(1);
            format!(
                "{} {}",
                parts.next().unwrap_or(""),
                parts.next().unwrap_or("")
            )
        })
        .collect()
}

// Catches adornments not taking their band in the row's flow: blocks
// below them would overlap them, and the row would be too short.
#[test]
fn adornments_take_their_slot_in_the_rows_flow() {
    use AdornmentSlot::{Before, End, Start};
    // Row 0 holds blocks 0 and 1, one 20px line each, below a 20px header.
    let cases: [(&[(AdornmentSlot, f32)], &str); 4] = [
        (&[(Start, 30.0)], "a0@30 b0@70 b1@100 =130"),
        (&[(Before(BlockKey(1)), 16.0)], "b0@30 a0@60 b1@86 =116"),
        // Zero height takes no space; a missing block sends it to the end.
        (&[(Start, 0.0), (End, 16.0)], "b0@30 b1@60 a1@90 =116"),
        (&[(Before(BlockKey(99)), 16.0)], "b0@30 b1@60 a0@90 =116"),
    ];
    for (adornments, expected) in cases {
        let row = message(0).with_adornments(
            adornments
                .iter()
                .enumerate()
                .map(|(i, &(slot, height))| blank_adornment(i as u64, slot, height))
                .collect(),
        );
        let doc = Doc::new([row]);

        let row = &doc.view.visible_rows()[0];
        let mut items: Vec<(f32, String)> = doc.view.visible_blocks()[row.blocks.clone()]
            .iter()
            .map(|b| (b.offset_in_row, format!("b{}@{}", b.key.0, b.offset_in_row)))
            .chain(
                doc.view.visible_adornments()[row.adornments.clone()]
                    .iter()
                    .map(|a| (a.offset_in_row, format!("a{}@{}", a.key.0, a.offset_in_row))),
            )
            .collect();
        items.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut dump: Vec<String> = items.into_iter().map(|(_, s)| s).collect();
        dump.push(format!("={}", row.height));
        assert_eq!(dump.join(" "), expected, "{adornments:?}");
    }
}

// Catches a labelled row hiding its adornment's controls along with the
// header text, and the policy not hiding the text it names.
#[test]
fn adornment_controls_stay_accessible_in_a_labelled_row() {
    let cases = [
        (AdornmentAccessibility::Exposed, true),
        (AdornmentAccessibility::ControlsOnly, false),
    ];
    for (policy, text_published) in cases {
        let mut rows = real_document(1);
        let row = rows.get_mut(&RowKey(0)).unwrap();
        row.adornments = vec![retry_bar(0).accessibility(policy)];
        let mut view = real_view(&rows);

        let names = accessible_names(&paint(&mut view, &rows, (400.0, 300.0), 0.0));

        let has = |name: &str| names.iter().any(|n| n == name);
        assert_eq!(
            (
                has("ListItem author 0"),
                has("Label row 0"),
                has("Button Retry 0")
            ),
            (true, text_published, true),
            "{policy:?}: {names:#?}"
        );
    }
}

// Catches a press on an adornment's button starting a text selection
// instead of reaching the button, which sits over the document's drag
// surface.
#[test]
fn clicking_an_adornment_button_emits_its_action_and_selects_nothing() {
    let mut rows = real_document(3);
    rows.get_mut(&RowKey(1)).unwrap().adornments = vec![retry_bar(0)];
    let mut view = real_view(&rows);
    let mut painted = paint(&mut view, &rows, (400.0, 600.0), 0.0);
    let update = painted.accessibility.tree_update("Test", None);
    let button = update
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some("Retry 1"))
        .and_then(|(_, n)| n.bounds())
        .expect("the button is published");
    let (x, y) = (
        ((button.x0 + button.x1) * 0.5) as f32,
        ((button.y0 + button.y1) * 0.5) as f32,
    );

    let mut actions = painted.router.pointer_down(x, y, &mut None).actions;
    actions.extend(painted.router.pointer_up().actions);
    for action in &actions {
        if let Some(Ev(event)) = action.downcast_ref::<Ev>() {
            view.handle(*event);
        }
    }

    let retries: Vec<&Retry> = actions.iter().filter_map(|a| a.downcast_ref()).collect();
    assert_eq!(
        (retries, view.selected_text(&rows)),
        (vec![&Retry(1)], String::new())
    );
}

// Catches a cached row replaying an adornment whose revision changed: a
// tool card would keep showing "Running" after it finished.
#[test]
fn a_new_adornment_revision_rebuilds_the_cached_row() {
    let label = Rc::new(std::cell::Cell::new("Running"));
    let bar = |revision: u64| {
        let label = label.clone();
        RowAdornment::new(
            AdornmentKey(1),
            AdornmentSlot::Start,
            30.0,
            revision,
            move |_| crate::element::text(label.get()).into_any(),
        )
    };
    let mut rows = real_document(2);
    rows.get_mut(&RowKey(0)).unwrap().adornments = vec![bar(1)];
    let mut view = real_view(&rows);
    let mut painter = CachedPainter::new();
    let size = (400.0, 300.0);
    painter.frame(&mut view, &rows, size, true);

    label.set("Done");
    let row = rows.get_mut(&RowKey(0)).unwrap();
    row.adornments = vec![bar(2)];
    view.update(&rows[&RowKey(0)]).unwrap();
    let frame = painter.frame(&mut view, &rows, size, true);

    assert!(
        frame.contains("Label | Done") && !frame.contains("Running"),
        "{frame}"
    );
}

// Catches a focused adornment control leaving the tree when its row
// scrolls out of the window, which would drop keyboard focus.
#[test]
fn a_kept_row_stays_in_the_tree_while_scrolled_away() {
    for kept in [false, true] {
        let mut rows = real_document(40);
        rows.get_mut(&RowKey(0)).unwrap().adornments = vec![retry_bar(0)];
        let mut view = real_view(&rows);
        view.keep_materialized(kept.then_some(RowKey(0)));
        view.scroll_to_bottom();
        paint(&mut view, &rows, (400.0, 300.0), 0.0);

        let names = accessible_names(&paint(&mut view, &rows, (400.0, 300.0), 0.0));

        let row_0_visible = view
            .visible_rows()
            .iter()
            .any(|r| r.key == RowKey(0) && r.top + r.height > 0.0);
        assert_eq!(
            (row_0_visible, names.iter().any(|n| n == "Button Retry 0")),
            (false, kept),
            "kept {kept}"
        );
    }
}

// ---------------------------------------------------------------------------
// Code toolbar
// ---------------------------------------------------------------------------

const WIDE_LINE: &str =
    "let banner = render(\"a line long enough to run well past the column\", tail_identifier);";

/// Row 0: a paragraph, then code block 5 with a short line and a line far
/// wider than a 300px column.
fn wide_code_rows() -> HashMap<RowKey, DocumentRow> {
    let mut message = message_with(0, &["Above the code."]);
    message.blocks.push(
        Block::code(
            BlockKey(5),
            ["fn main() {", WIDE_LINE, "}"]
                .iter()
                .map(|line| vec![crate::element::StyledSpan::plain(*line)])
                .collect(),
        )
        .with_label(Some("rust".into())),
    );
    [(message.key, message)].into()
}

/// The published node named `name`: its states after the role and name,
/// and the center of its bounds.
fn published(painted: &Painted, name: &str) -> (String, (f32, f32)) {
    let update = painted.accessibility.tree_update("Test", None);
    let (_, node) = update
        .nodes
        .iter()
        .find(|(_, n)| n.label() == Some(name))
        .unwrap_or_else(|| panic!("{name} is not published"));
    let b = node.bounds().expect("bounds");
    let dump = crate::accessibility::dump_accessibility_states(&update);
    let states = dump
        .lines()
        .map(|line| line.splitn(4, " | ").collect::<Vec<_>>())
        .find(|parts| parts.get(2) == Some(&name))
        .and_then(|parts| parts.get(3).map(|s| (*s).to_owned()))
        .unwrap_or_default();
    (
        states,
        (((b.x0 + b.x1) * 0.5) as f32, ((b.y0 + b.y1) * 0.5) as f32),
    )
}

fn click(painted: &mut Painted, (x, y): (f32, f32)) -> Vec<Action> {
    let mut actions = painted.router.pointer_down(x, y, &mut None).actions;
    actions.extend(painted.router.pointer_up().actions);
    actions
}

// Catches Copy reading the text on screen instead of the block's source:
// columns scrolled out of view would be lost.
#[test]
fn copy_button_emits_the_whole_source_of_a_scrolled_code_block() {
    let rows = wide_code_rows();
    let mut view = real_view(&rows);
    view.set_code_toolbar(true);
    paint(&mut view, &rows, (300.0, 400.0), 0.0);
    view.scroll_handles[&BlockKey(5)].set_offset(120.0, 0.0);
    let mut painted = paint(&mut view, &rows, (300.0, 400.0), 0.0);

    let (_, copy) = published(&painted, "Copy code");
    let actions = click(&mut painted, copy);

    let copied: Vec<&CopyCode> = actions.iter().filter_map(|a| a.downcast_ref()).collect();
    let source = format!("fn main() {{\n{WIDE_LINE}\n}}");
    assert_eq!(
        copied,
        vec![&CopyCode {
            block: BlockKey(5),
            text: source.into()
        }]
    );
}

// Catches the wrap toggle not reaching the document, or wrapped code
// still running past the column: the end of the wide line must land
// inside it, and the toggle must read as checked.
#[test]
fn wrap_toggle_brings_the_end_of_a_wide_line_into_the_column() {
    let rows = wide_code_rows();
    let mut view = real_view(&rows);
    view.set_code_toolbar(true);
    let mut painted = paint(&mut view, &rows, (300.0, 400.0), 0.0);
    let (unwrapped, toggle) = published(&painted, "Wrap lines");

    for action in click(&mut painted, toggle) {
        if let Some(Ev(event)) = action.downcast_ref::<Ev>() {
            view.handle(*event);
        }
    }
    let painted = paint(&mut view, &rows, (300.0, 400.0), 0.0);

    let code = view
        .visible_blocks()
        .iter()
        .find(|b| b.key == BlockKey(5))
        .unwrap();
    let end = code.text_len;
    let mut rects = Vec::new();
    code.geometry.range_rects(end - 1..end, &mut rects);
    let last_char_right = rects.iter().map(|r| r.x + r.width).fold(0.0, f32::max);
    let (wrapped, _) = published(&painted, "Wrap lines");
    assert_eq!(
        (
            unwrapped.as_str(),
            wrapped.as_str(),
            last_char_right <= code.rect.width
        ),
        ("unchecked", "checked", true),
        "last char ends at {last_char_right} of {}",
        code.rect.width
    );
}

// Catches the measurer and the painted code block disagreeing about the
// toolbar row or the wrap width: hits and highlights would land on the
// wrong glyphs.
#[test]
fn code_text_is_painted_where_it_was_measured_with_toolbar_and_wrap() {
    for (toolbar, wrap) in [(false, true), (true, false), (true, true)] {
        let rows = wide_code_rows();
        let mut view = real_view(&rows);
        view.set_code_toolbar(toolbar);
        view.set_code_wrap(BlockKey(5), wrap);

        let painted = paint(&mut view, &rows, (300.0, 400.0), 0.0);

        let code = view
            .visible_blocks()
            .iter()
            .find(|b| b.key == BlockKey(5))
            .unwrap();
        let region = painted
            .regions
            .iter()
            .find(|r| r.source_key == 5)
            .expect("code is painted");
        let starts = |layout: &quark_text::TextLayout| -> Vec<usize> {
            layout.lines().map(|line| line.byte_range.start).collect()
        };
        let (ox, oy) = code.geometry.text_origin;
        let measured = format!(
            "h{:.0} text@{:.0},{:.0} lines at {:?}",
            code.rect.height,
            code.rect.x + ox,
            code.rect.y + oy,
            starts(code.geometry.layout.as_ref().unwrap())
        );
        let shown = format!(
            "h{:.0} text@{:.0},{:.0} lines at {:?}",
            region.bounds.height,
            region.text_origin.0,
            region.text_origin.1,
            starts(&region.layout)
        );
        assert_eq!(shown, measured, "toolbar {toolbar} wrap {wrap}");
    }
}

// ---------------------------------------------------------------------------
// Tables
// ---------------------------------------------------------------------------

const TABLE: &str = "| 名前 | n |\n|---|---|\n| 日本語テキスト | 1 |\n| a \\| b | 22 |";

/// Cell texts of the first table block in `markdown`, row by row.
fn parsed_cells(markdown: &str) -> Vec<Vec<String>> {
    let doc = crate::markdown::MarkdownDoc::parse(markdown);
    let block = (0..doc.len())
        .find(|&b| doc.kind(b) == crate::markdown::BlockKind::Table)
        .unwrap_or_else(|| panic!("no table in {markdown:?}"));
    let mut rows: Vec<Vec<String>> = Vec::new();
    for cell in doc.cells(block) {
        let (row, _) = doc.cell_position(cell);
        if rows.len() <= row {
            rows.resize_with(row + 1, Vec::new);
        }
        rows[row].push(doc.cell_text(cell).to_owned());
    }
    rows
}

/// A real-text document holding row 0 with `TABLE` as its markdown.
fn table_view() -> (HashMap<RowKey, DocumentRow>, Document, BlockKey) {
    let message = markdown_message(
        0,
        &mut MarkdownBlocks::new(),
        TABLE,
        &mut SyntaxHighlighter::new(),
    );
    let key = message.blocks[0].key;
    let rows: HashMap<RowKey, DocumentRow> = [(message.key, message)].into();
    let mut view = real_view(&rows);
    view.set_scroll_offset(0.0);
    (rows, view, key)
}

// Catches a copied table that no longer parses as the same table: a pipe
// inside a cell must stay escaped.
#[test]
fn a_copied_table_parses_back_to_the_same_cells() {
    let mut md = markdown_document(TABLE);
    md.document_mut().select_all();

    let copied = md.selected_text();

    assert_eq!(parsed_cells(&copied), parsed_cells(TABLE), "{copied}");
}

// Catches columns sized by char count or per row: a wide CJK cell must
// push its whole column right, and every row's cells must line up.
#[test]
fn table_cells_line_up_in_columns_as_wide_as_their_widest_cell() {
    let (rows, mut view, key) = table_view();

    let painted = paint(&mut view, &rows, (500.0, 300.0), 0.0);

    // Text regions of the table's cells, row-major.
    let cells: Vec<&crate::element::SelectableTextRegion> = painted
        .regions
        .iter()
        .filter(|r| r.source_key == key.0)
        .collect();
    let lefts: Vec<Vec<f32>> = cells
        .chunks(2)
        .map(|row| row.iter().map(|r| r.text_origin.0).collect())
        .collect();
    // A cell narrower than its text would wrap it onto more lines.
    let wrapped: Vec<String> = cells
        .iter()
        .filter(|r| r.layout.line_count() > 1)
        .map(|r| r.layout.source().to_string())
        .collect();
    assert_eq!(
        (lefts[1].clone(), lefts[2].clone(), wrapped),
        (lefts[0].clone(), lefts[0].clone(), Vec::<String>::new()),
        "{lefts:?}"
    );
}

// Catches a press in a cell landing in another cell, or on the pipes and
// rule between them: dragging across one cell copies exactly its text.
#[test]
fn dragging_across_a_cell_copies_exactly_its_text() {
    let (rows, mut view, key) = table_view();
    paint(&mut view, &rows, (500.0, 300.0), 0.0);
    let block = view.visible_blocks().iter().find(|b| b.key == key).unwrap();
    let metrics = block.geometry.table_metrics().unwrap().clone();
    let cell = metrics.cell(2, 1).offset(block.rect.x, block.rect.y);
    let mid = cell.y + cell.height * 0.5;

    view.handle(DocumentEvent::PointerDown {
        x: cell.x + 1.0,
        y: mid,
    });
    view.handle(DocumentEvent::PointerDrag {
        x: cell.x + cell.width - 1.0,
        y: mid,
    });
    view.handle(DocumentEvent::PointerUp);

    assert_eq!(view.selected_text(&rows), "22");
}

// Catches tables published as loose text: assistive tech needs the
// header cells, the data cells, and their row and column.
#[test]
fn a_table_publishes_header_and_data_cells_with_their_text() {
    let (rows, mut view, _) = table_view();

    let painted = paint(&mut view, &rows, (500.0, 300.0), 0.0);

    let update = painted.accessibility.tree_update("Test", None);
    let node = |id: &accesskit::NodeId| update.nodes.iter().find(|(n, _)| n == id).map(|(_, n)| n);
    let cells: Vec<String> = update
        .nodes
        .iter()
        .filter(|(_, n)| {
            matches!(
                n.role(),
                accesskit::Role::ColumnHeader | accesskit::Role::Cell
            )
        })
        .map(|(_, n)| {
            let text: String = n
                .children()
                .iter()
                .filter_map(|c| node(c).and_then(|c| c.value().or(c.label())))
                .collect();
            format!(
                "{:?} {},{} {text}",
                n.role(),
                n.row_index().unwrap_or(99),
                n.column_index().unwrap_or(99)
            )
        })
        .collect();
    let table = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == accesskit::Role::Table)
        .map(|(_, n)| (n.row_count(), n.column_count()));
    assert_eq!(
        (table, cells),
        (
            Some((Some(3), Some(2))),
            vec![
                "ColumnHeader 0,0 名前".to_owned(),
                "ColumnHeader 0,1 n".to_owned(),
                "Cell 1,0 日本語テキスト".to_owned(),
                "Cell 1,1 1".to_owned(),
                "Cell 2,0 a \\| b".to_owned(),
                "Cell 2,1 22".to_owned(),
            ]
        )
    );
}

// Catches a card's disclosure jumping away under the pointer when the card
// expands while the view follows the bottom: the growth would push the
// card up by the height it gained.
#[test]
fn a_held_row_keeps_its_place_while_it_grows_at_the_bottom() {
    let card = |height: f32| {
        message(8).with_adornments(vec![blank_adornment(1, AdornmentSlot::Start, height)])
    };
    let mut doc = Doc::new((0..8).map(message).chain([card(20.0)]).chain([message(9)]));
    doc.size.1 = 400.0;
    doc.view.scroll_to_bottom();
    doc.frame();
    let before = doc.screen_top(8);

    let expanded = card(200.0);
    doc.view.update(&expanded).unwrap();
    doc.messages.insert(expanded.key, expanded);
    doc.view.hold_in_place(RowKey(8));
    doc.frame();

    assert_eq!(doc.screen_top(8), before);
}

// Catches the document disowning the focus a press on wide code gives its
// scroll area: the app would then send Copy to the last text field.
#[test]
fn a_press_on_wide_code_focuses_a_target_the_document_owns() {
    let rows = wide_code_rows();
    let mut view = real_view(&rows);
    let mut painted = paint(&mut view, &rows, (300.0, 400.0), 0.0);
    let code = view
        .visible_blocks()
        .iter()
        .find(|b| b.key == BlockKey(5))
        .unwrap()
        .rect;
    let mut focus = Some(FocusId::from_key("composer"));

    painted
        .router
        .pointer_down(code.x + 40.0, code.y + code.height - 10.0, &mut focus);

    let focus = focus.expect("the press focuses something");
    assert_eq!(
        (
            view.owns_focus(focus),
            view.owns_focus(FocusId::from_key("composer"))
        ),
        (true, false)
    );
}

// Catches toolbar buttons that only a pointer can press: assistive tech
// activates them through their click action.
#[test]
fn code_toolbar_buttons_offer_assistive_tech_a_click() {
    let rows = wide_code_rows();
    let mut view = real_view(&rows);
    view.set_code_toolbar(true);

    let painted = paint(&mut view, &rows, (300.0, 400.0), 0.0);

    let update = painted.accessibility.tree_update("Test", None);
    let clickable = |name: &str| {
        update
            .nodes
            .iter()
            .find(|(_, n)| n.label() == Some(name))
            .is_some_and(|(_, n)| n.supports_action(accesskit::Action::Click))
    };
    assert_eq!(
        (clickable("Copy code"), clickable("Wrap lines")),
        (true, true)
    );
}

// Catches an image hinted before any row showed it never loading: the hint
// made the store think the load had started.
#[test]
fn an_image_hinted_before_its_row_arrives_still_loads() {
    let mut md = MarkdownDocument::new(DocumentStyle::for_font_size(14.0));
    md.set_image_loader(sized_loader());
    md.hint_image_size("200x100.png", 200, 100);
    md.push(MarkdownEntry {
        row: RowKey(0),
        chrome: RowChrome::default(),
        markdown: "![chart](200x100.png)".to_owned(),
    })
    .unwrap();
    let (mut text, mut layouts) = (
        TextSystem::vendored_only(&Default::default()),
        LayoutCache::default(),
    );

    md.finish_images();
    prepare_markdown(&mut md, &mut text, &mut layouts);

    assert_eq!(image_block(&md), (100.0, true));
}
