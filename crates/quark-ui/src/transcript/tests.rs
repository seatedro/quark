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
}

impl BlockMeasurer for Grid {
    type Geometry = GridGeometry;

    fn measure(&mut self, block: &TranscriptBlock, width: f32) -> GridGeometry {
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

fn grid_style() -> TranscriptStyle {
    TranscriptStyle {
        font_size: 14.0,
        pad_x: 10.0,
        pad_y: 10.0,
        header_height: 20.0,
        block_gap: 10.0,
        line_scroll: 60.0,
        edge: 32.0,
        overscan: 450.0,
    }
}

/// Message `i`: row key `i`, blocks `10i` ("m{i} first") and `10i + 1`
/// ("m{i} second"). With the grid style each row is 90px tall.
fn message(i: u64) -> TranscriptMessage {
    message_with(i, &[&format!("m{i} first"), &format!("m{i} second")])
}

fn message_with(i: u64, blocks: &[&str]) -> TranscriptMessage {
    TranscriptMessage {
        key: RowKey(i),
        role: if i.is_multiple_of(2) {
            TranscriptRole::User
        } else {
            TranscriptRole::Assistant
        },
        author: format!("author {i}").into(),
        blocks: blocks
            .iter()
            .enumerate()
            .map(|(b, text)| TranscriptBlock::plain(BlockKey(i * 10 + b as u64), *text))
            .collect(),
    }
}

/// A transcript over a grid-measured document, with a frame clock.
struct Doc {
    messages: HashMap<RowKey, TranscriptMessage>,
    transcript: Transcript<GridGeometry>,
    size: (f32, f32),
    now_ms: u64,
}

impl Doc {
    fn new(messages: impl IntoIterator<Item = TranscriptMessage>) -> Self {
        let messages: Vec<TranscriptMessage> = messages.into_iter().collect();
        let mut transcript = Transcript::new(grid_style());
        transcript.extend(&messages).unwrap();
        let mut doc = Self {
            messages: messages.into_iter().map(|m| (m.key, m)).collect(),
            transcript,
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
        self.transcript
            .prepare(w, h, self.now_ms, &self.messages, &mut Grid);
        self.now_ms += 16;
    }

    fn scroll_to(&mut self, offset: f32) {
        self.transcript.set_scroll_offset(offset);
        self.frame();
    }

    /// Viewport point over byte `col` of the first line of `block`.
    fn point(&self, block: u64, col: usize) -> (f32, f32) {
        let visible = self
            .transcript
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
        self.transcript
            .handle(TranscriptEvent::PointerDown { x, y });
    }

    fn drag(&mut self, (x, y): (f32, f32)) {
        self.transcript
            .handle(TranscriptEvent::PointerDrag { x, y });
    }

    fn release(&mut self) {
        self.transcript.handle(TranscriptEvent::PointerUp);
    }

    fn copy(&self) -> String {
        self.transcript.selected_text(&self.messages)
    }

    /// Key of the row under the viewport's vertical middle.
    fn middle_row(&self) -> u64 {
        let middle = self.size.1 * 0.5;
        self.transcript
            .visible_rows()
            .iter()
            .find(|r| r.top <= middle && r.top + r.height > middle)
            .map_or(0, |r| r.key.0)
    }

    /// The row under the viewport's top edge and where its top sits.
    fn anchor(&self) -> String {
        self.transcript
            .visible_rows()
            .iter()
            .find(|r| r.top <= 0.0 && r.top + r.height > 0.0)
            .map_or("-".to_owned(), |r| format!("{}@{}", r.key.0, r.top))
    }

    /// `"<block>:<lo>..<hi>"` for every materialized block with a highlight.
    fn highlights(&self) -> String {
        self.transcript
            .visible_blocks()
            .iter()
            .filter_map(|b| {
                let (lo, hi) = self.transcript.block_selection(b.key, b.text_len)?;
                Some(format!("{}:{lo}..{hi}", b.key.0))
            })
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Replaces the text of `block` in message `row` and reports the change.
    fn stream(&mut self, row: u64, block: usize, text: &str) {
        let message = self.messages.get_mut(&RowKey(row)).unwrap();
        let key = message.blocks[block].key;
        message.blocks[block] = TranscriptBlock::plain(key, text);
        self.transcript.update(message).unwrap();
    }

    /// Where row `key`'s top sits relative to the viewport top, whether or
    /// not it is materialized.
    fn screen_top(&self, key: u64) -> f32 {
        let rows = self.transcript.list().rows();
        rows.offset_of(RowKey(key)).unwrap() - self.transcript.scroll_offset()
    }

    fn last_row_bottom(&self) -> f32 {
        self.transcript
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
        let rows = doc.transcript.visible_rows();
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
    doc.scroll_to(905.0);
    doc.press(doc.point(10_100, 0));
    doc.drag(doc.point(10_110, 2));
    doc.release();
    let before = (doc.anchor(), doc.copy());

    let history: Vec<TranscriptMessage> = (0..100).map(message).collect();
    doc.transcript.prepend(&history).unwrap();
    doc.messages.extend(history.into_iter().map(|m| (m.key, m)));
    doc.frame();

    assert_ne!(before.0, "-");
    assert_eq!((doc.anchor(), doc.copy()), before);
}

#[test]
fn key_command_maps_cmd_and_ctrl_bindings() {
    let cases = [
        ("ctrl+c", Some(TranscriptCommand::Copy)),
        ("Cmd+C", Some(TranscriptCommand::Copy)),
        ("ctrl+a", Some(TranscriptCommand::SelectAll)),
        ("cmd+a", Some(TranscriptCommand::SelectAll)),
        ("ctrl+x", None),
    ];
    for (binding, expected) in cases {
        assert_eq!(key_command(binding), expected, "{binding}");
    }
}

#[test]
fn select_all_copies_every_block() {
    let mut doc = Doc::rows(3);

    doc.transcript.select_all();

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
    doc.transcript.update(&shrunk).unwrap();
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
    doc.transcript.push(&duplicate).unwrap();
    doc.messages.insert(duplicate.key, duplicate);
    doc.frame();

    // Below both messages: nearest to where the duplicate would be drawn.
    doc.press((50.0, 160.0));

    let drawn: Vec<(u64, u64)> = doc
        .transcript
        .visible_blocks()
        .iter()
        .map(|b| (b.row.0, b.key.0))
        .collect();
    assert_eq!(drawn, [(0, 0)]);
    // The end of "owner", not of "second".
    assert_eq!(
        doc.transcript.selection(),
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
    doc.transcript.update(&shrunk).unwrap();
    doc.messages.insert(shrunk.key, shrunk);

    doc.press(dropped);
    doc.transcript.push(&message(1)).unwrap();

    assert_eq!(
        doc.transcript.selection().map(|s| s.focus.block),
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
    let wanted_frames = doc.transcript.wants_frame();
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
    assert!(!doc.transcript.wants_frame());
}

#[test]
fn selection_focus_follows_rows_autoscrolling_under_the_pointer() {
    let mut doc = Doc::rows(50);
    let row = hold_above_the_top_edge(&mut doc);
    let focus_row = |doc: &Doc| {
        doc.transcript
            .selection()
            .map_or(0, |s| s.focus.block.0 / 10)
    };
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

    assert!(doc.transcript.is_stuck_to_bottom());
    assert_eq!(bottoms, [900.0; 8]);
    assert!(doc.transcript.visible_rows().last().unwrap().height > 90.0);
}

#[test]
fn new_content_while_scrolled_up_offers_jump_to_latest() {
    let mut doc = Doc::rows(50);
    doc.scroll_to(0.0);

    doc.stream(49, 1, "m49 second, and more");
    doc.frame();
    let scrolled_up = (doc.transcript.has_unseen(), doc.transcript.scroll_offset());
    doc.transcript.handle(TranscriptEvent::JumpToLatest);
    doc.frame();

    assert_eq!(scrolled_up, (true, 0.0));
    assert_eq!(
        (doc.transcript.has_unseen(), doc.last_row_bottom()),
        (false, 900.0)
    );
}

// Catches edits to older messages (a highlight arriving) offering "Jump to
// latest" when nothing new arrived at the bottom.
#[test]
fn editing_an_older_message_while_scrolled_up_offers_no_jump() {
    let mut doc = Doc::rows(50);
    doc.scroll_to(0.0);

    doc.stream(10, 1, "m10 second, edited");
    doc.frame();

    assert!(!doc.transcript.has_unseen());
}

#[test]
fn narrowing_the_viewport_rewraps_rows() {
    // 76 chars: 2 lines at 38 columns (400px), 5 lines at 18 (200px).
    let long = "x".repeat(76);
    let mut doc = Doc::new([message_with(0, &[&long])]);
    let wide = doc.transcript.visible_rows()[0].height;

    doc.size.0 = 200.0;
    doc.frame();

    assert_eq!(
        (wide, doc.transcript.visible_rows()[0].height),
        (80.0, 140.0)
    );
}

// ---------------------------------------------------------------------------
// Invariants under arbitrary document edits
// ---------------------------------------------------------------------------

// Catches debug integrity checks that walk the whole document per block or
// per message, which made loading a long transcript quadratic in debug
// builds. Counted in check steps, not timed.
#[test]
fn extending_with_30k_blocks_checks_in_linear_steps() {
    let messages: Vec<TranscriptMessage> = (0..10_000)
        .map(|i| message_with(i, &["a", "b", "c"]))
        .collect();
    let mut transcript: Transcript<GridGeometry> = Transcript::new(grid_style());
    transcript
        .push(&message_with(1_000_000, &["first"]))
        .unwrap();

    let before = quark::selection::integrity_steps();
    transcript.extend(&messages).unwrap();
    let steps = quark::selection::integrity_steps() - before;

    assert_eq!(transcript.len(), 10_001);
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

fn blocks_message(key: u64, count: u8) -> TranscriptMessage {
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
        let mut transcript: Transcript<GridGeometry> = Transcript::new(grid_style());
        let (mut next, mut first) = (1_000u64, 1_000u64);
        for op in ops {
            match op {
                Op::Push(n) => {
                    for _ in 0..n {
                        transcript.push(&blocks_message(next, 2)).unwrap();
                        next += 1;
                    }
                }
                Op::Prepend(n) => {
                    let history: Vec<_> =
                        (0..n as u64).map(|i| blocks_message(first - n as u64 + i, 2)).collect();
                    first -= n as u64;
                    transcript.prepend(&history).unwrap();
                }
                Op::Update(..) | Op::Remove(..) if transcript.is_empty() => {}
                Op::Update(rank, count) => {
                    let keys = transcript.list().rows().keys();
                    let key = keys[rank as usize % keys.len()];
                    transcript.update(&blocks_message(key.0, count)).unwrap();
                }
                Op::Remove(rank) => {
                    let keys = transcript.list().rows().keys();
                    let key = keys[rank as usize % keys.len()];
                    transcript.remove(key).unwrap();
                }
                Op::SelectAll => transcript.select_all(),
            }
            prop_assert_eq!(transcript.verify_integrity(), Ok(()));
        }
    }
}

// ---------------------------------------------------------------------------
// The element, with real text layout
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Ev(TranscriptEvent);

impl From<Ev> for Action {
    fn from(ev: Ev) -> Self {
        Action::new(ev)
    }
}

struct Painted {
    accessibility: AccessibilityFrame,
    regions: Vec<crate::element::SelectableTextRegion>,
    router: InputRouter,
}

/// Prepares `transcript` with real layouts and paints its element `top`
/// pixels below the window's top edge.
fn paint(
    transcript: &mut Transcript,
    messages: &HashMap<RowKey, TranscriptMessage>,
    size: (f32, f32),
    top: f32,
) -> Painted {
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    let font_size = transcript.style().font_size;
    transcript.prepare(
        size.0,
        size.1,
        0,
        messages,
        &mut TextMeasurer::new(&mut text, &mut layouts, font_size, 1.0),
    );
    let theme = Theme::default_dark();
    let element = transcript.element(messages, &theme, |ev| Ev(ev).into());
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
    render_element(&mut root, &mut Scene::default(), &mut cx, w, h);
    let regions = std::mem::take(&mut cx.selectable_text_runs);
    let accessibility = std::mem::take(&mut cx.accessibility);
    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    Painted {
        accessibility,
        regions,
        router,
    }
}

fn real_document(n: u64) -> HashMap<RowKey, TranscriptMessage> {
    (0..n)
        .map(|i| {
            let mut m = message_with(
                i,
                &[&format!(
                    "Message {i} wraps across a few lines when the column is narrow enough."
                )],
            );
            if i.is_multiple_of(3) {
                m.blocks.push(TranscriptBlock::code(
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

fn real_transcript(messages: &HashMap<RowKey, TranscriptMessage>) -> Transcript {
    let mut keys: Vec<&RowKey> = messages.keys().collect();
    keys.sort();
    let mut transcript = Transcript::new(TranscriptStyle::for_font_size(14.0));
    transcript
        .extend(keys.into_iter().map(|k| &messages[k]))
        .unwrap();
    transcript
}

#[test]
fn measured_blocks_match_the_painted_text_elements() {
    let messages = real_document(12);
    let mut transcript = real_transcript(&messages);
    transcript.set_scroll_offset(0.0);

    let painted = paint(&mut transcript, &messages, (260.0, 600.0), 0.0);

    let fmt = |key: u64, x: f32, y: f32, h: f32, ox: f32, oy: f32| {
        format!("{key} {x:.0},{y:.0} h{h:.0} text@{ox:.0},{oy:.0}")
    };
    let expected: Vec<String> = transcript
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
    let mut transcript = real_transcript(&messages);
    let offset = transcript.list().rows().offset_of(RowKey(25)).unwrap();
    transcript.set_scroll_offset(offset);
    paint(&mut transcript, &messages, (400.0, 600.0), 0.0);
    // Rows above were measured on the way; settle the anchor at row 25.
    let offset = transcript.list().rows().offset_of(RowKey(25)).unwrap();
    transcript.set_scroll_offset(offset);

    let painted = paint(&mut transcript, &messages, (400.0, 600.0), 0.0);

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
    let mut transcript = real_transcript(&messages);
    transcript.set_scroll_offset(0.0);
    // The transcript sits 50px down, so window and local coordinates differ.
    let top = 50.0;
    let mut painted = paint(&mut transcript, &messages, (400.0, 600.0), top);
    let block = |key: u64| {
        transcript
            .visible_blocks()
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
            transcript.handle(*event);
        }
    }

    assert_eq!(
        transcript.selected_text(&messages),
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
    markdown: &mut MarkdownMessage,
    source: &str,
    syntax: &mut SyntaxHighlighter,
) -> TranscriptMessage {
    markdown_message_with(row, markdown, source, syntax, &mut BlockKeys::new())
}

fn markdown_message_with(
    row: u64,
    markdown: &mut MarkdownMessage,
    source: &str,
    syntax: &mut SyntaxHighlighter,
    keys: &mut BlockKeys,
) -> TranscriptMessage {
    TranscriptMessage {
        key: RowKey(row),
        role: TranscriptRole::Assistant,
        author: "assistant".into(),
        blocks: markdown.blocks(&crate::markdown::MarkdownDoc::parse(source), syntax, keys),
    }
}

/// One line per block: key, content kind and label, list and quote depth,
/// gutter marker, copy prefix, then the selectable text.
fn dump_blocks(message: &TranscriptMessage) -> String {
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
fn highlighted_runs(block: &TranscriptBlock) -> Vec<String> {
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
        &mut MarkdownMessage::new(),
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
            r###"5 code() l0 q0 [] "" "| a   | bb  |\n| --- | --- |\n| 1   | 2   |""###,
            r###"6 code(rust) l0 q0 [] "" "fn main() {}""###,
            r###"7 rule l0 q0 [] "" "---""###,
        ]
        .join("\n")
    );
}

// Catches columns padded by char count, which misaligns wide (CJK) text in
// the monospace grid.
#[test]
fn table_columns_align_by_display_width() {
    let source = "| 名前 | n |\n|---|---|\n| 日本語テキスト | 1 |\n| abc | 22 |";

    let message = markdown_message(
        0,
        &mut MarkdownMessage::new(),
        source,
        &mut SyntaxHighlighter::new(),
    );

    assert_eq!(
        message.blocks[0].text(),
        "| 名前           | n   |\n\
         | -------------- | --- |\n\
         | 日本語テキスト | 1   |\n\
         | abc            | 22  |"
    );
}

#[test]
fn drag_from_a_heading_across_a_list_into_code_copies_markers_and_source() {
    let mut syntax = SyntaxHighlighter::new();
    let source = "## Setup\n\n- one\n- two\n\n```rust\nfn main() {\n    run();\n}\n```";
    let message = markdown_message(0, &mut MarkdownMessage::new(), source, &mut syntax);
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
    let mut markdown = MarkdownMessage::new();
    let mut source = String::from("# Title\n\nFirst paragraph.\n\n- a\n- b\n\nStreaming");
    let mut messages = HashMap::new();
    let mut transcript = Transcript::new(TranscriptStyle::for_font_size(14.0));
    let message = markdown_message_with(0, &mut markdown, &source, &mut syntax, &mut keys);
    transcript.push(&message).unwrap();
    messages.insert(message.key, message);
    let mut frame = |transcript: &mut Transcript, messages: &HashMap<_, _>| {
        let mut measurer = TextMeasurer::new(&mut text, &mut layouts, 14.0, 1.0);
        transcript.prepare(400.0, 600.0, 0, messages, &mut measurer);
        transcript
            .visible_blocks()
            .iter()
            .map(|b| b.geometry.layout.clone().unwrap())
            .collect::<Vec<_>>()
    };
    let mut before = frame(&mut transcript, &messages);

    let mut reshaped = Vec::new();
    for word in [" more", " words", " arrive"] {
        source.push_str(word);
        let message = markdown_message_with(0, &mut markdown, &source, &mut syntax, &mut keys);
        transcript.update(&message).unwrap();
        messages.insert(message.key, message);
        let after = frame(&mut transcript, &messages);
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
    let mut markdown = MarkdownMessage::new();
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

#[cfg(feature = "syntax")]
#[test]
fn rust_code_block_is_colored_once_its_highlight_arrives() {
    let mut syntax = SyntaxHighlighter::new();
    let mut markdown = MarkdownMessage::new();
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
        &mut MarkdownMessage::new(),
        "```rust\nfn main() {}\n```",
        &mut syntax,
    );

    assert_eq!(syntax.finish_pending(), vec![message.blocks[0].key]);
}

/// A markdown transcript holding message 0 with `source`.
fn markdown_transcript(source: &str) -> MarkdownTranscript {
    let mut md = MarkdownTranscript::new(TranscriptStyle::for_font_size(14.0));
    md.push(MarkdownEntry {
        row: RowKey(0),
        role: TranscriptRole::Assistant,
        author: "assistant".into(),
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
    let mut md = markdown_transcript("```rust\nfn a() {}\n```\n\n```rust\nfn b() {}\n```");
    md.finish_highlights();
    let kept = md.highlighter().len();

    md.remove(RowKey(0)).unwrap();

    assert_eq!((kept, md.highlighter().len()), (2, 0));
}

// Catches truncation keeping the highlights of blocks a message lost.
#[cfg(feature = "syntax")]
#[test]
fn shrinking_a_message_forgets_the_highlights_of_dropped_blocks() {
    let mut md = markdown_transcript("```rust\nfn a() {}\n```\n\n```rust\nfn b() {}\n```");
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
    let mut syntax = SyntaxHighlighter::new();
    let mut markdown = MarkdownMessage::new();
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

#[test]
fn facade_streams_markdown_into_selectable_blocks() {
    let mut md = markdown_transcript("# Answer\n\nfirst");

    md.set_markdown(RowKey(0), "# Answer\n\nfirst words\n\n- item")
        .unwrap();
    md.transcript_mut().select_all();

    assert_eq!(md.selected_text(), "# Answer\n\nfirst words\n\n- item");
}

/// Paints `md` with `theme` and returns its text regions and scene.
fn paint_markdown(
    md: &mut MarkdownTranscript,
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
    let mut md = markdown_transcript("see ![a chart](c.png)");
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
    let mut md = markdown_transcript("# Title\n\n### Small\n\nplain");

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
fn link_click_emits_the_transcript_link_action() {
    #[derive(Debug, Clone, PartialEq)]
    struct Open(Arc<str>);
    let mut md = markdown_transcript("see [docs](https://example.com/docs) now");
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

// Catches markdown span flags not reaching the painted decorations.
#[test]
fn strikethrough_markdown_paints_a_line_through_its_run() {
    let mut md = markdown_transcript("keep ~~gone~~ keep");

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
