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
fn select_all_copies_every_block_for_either_modifier() {
    let mut doc = Doc::rows(3);

    let copied = ["ctrl", "cmd"].map(|modifier| {
        doc.transcript.set_selection(None);
        if key_command(&format!("{modifier}+a")) == Some(TranscriptCommand::SelectAll) {
            doc.transcript.select_all();
        }
        match key_command(&format!("{modifier}+c")) {
            Some(TranscriptCommand::Copy) => doc.copy(),
            _ => String::new(),
        }
    });

    let all = "m0 first\n\nm0 second\n\nm1 first\n\nm1 second\n\nm2 first\n\nm2 second";
    assert_eq!(copied, [all, all]);
}

// ---------------------------------------------------------------------------
// Autoscroll
// ---------------------------------------------------------------------------

#[test]
fn autoscroll_runs_while_held_past_the_edge_and_stops_on_release() {
    let mut doc = Doc::rows(50);
    doc.scroll_to(2000.0);
    let anchor_row = doc.middle_row();
    doc.press(doc.point(anchor_row * 10, 0));
    doc.drag((50.0, -30.0));
    let focus_row = |doc: &Doc| {
        doc.transcript
            .selection()
            .map_or(0, |s| s.focus.block.0 / 10)
    };
    let first_focus = focus_row(&doc);

    // Content moves down (the view scrolls up) while the pointer is held.
    let mut held = vec![doc.screen_top(anchor_row)];
    for _ in 0..10 {
        doc.frame();
        held.push(doc.screen_top(anchor_row));
    }
    let wanted_frames = doc.transcript.wants_frame();
    let last_focus = focus_row(&doc);
    doc.release();
    let released = doc.screen_top(anchor_row);
    for _ in 0..3 {
        doc.frame();
    }

    // The first held frame only starts the clock.
    assert!(
        held[1..].windows(2).all(|w| w[1] > w[0]),
        "row {anchor_row} should move down every frame: {held:?}"
    );
    assert!(wanted_frames);
    // The selection end follows the rows scrolling under the held pointer.
    assert!(
        last_focus < first_focus && first_focus < anchor_row,
        "focus {first_focus} -> {last_focus}, anchor {anchor_row}"
    );
    assert_eq!(doc.screen_top(anchor_row), released);
    assert!(!doc.transcript.wants_frame());
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
fn streaming_growth_stays_pinned_to_the_bottom() {
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

#[test]
fn visible_rows_stay_bounded_across_5000_messages() {
    // Varied lengths: 1 to 7 lines in the first block.
    let mut doc = Doc::new((0..5000).map(|i| {
        let body = "word ".repeat(1 + (i as usize * 7) % 50);
        message_with(i, &[&body, "tail"])
    }));

    let mut counts = Vec::new();
    for offset in [0.0, 100_000.0, 250_000.0, f32::MAX] {
        doc.scroll_to(offset);
        counts.push(doc.transcript.visible_rows().len());
    }

    // (900 viewport + 2 * 450 overscan) / 90px shortest row, plus partial
    // rows at both ends.
    assert!(counts.iter().all(|&n| (1..=22).contains(&n)), "{counts:?}");
}

// ---------------------------------------------------------------------------
// Invariants under arbitrary document edits
// ---------------------------------------------------------------------------

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
    let messages = real_document(5000);
    let mut transcript = real_transcript(&messages);
    let offset = transcript.list().rows().offset_of(RowKey(2500)).unwrap();
    transcript.set_scroll_offset(offset);
    paint(&mut transcript, &messages, (400.0, 600.0), 0.0);
    // Rows above were measured on the way; settle the anchor at row 2500.
    let offset = transcript.list().rows().offset_of(RowKey(2500)).unwrap();
    transcript.set_scroll_offset(offset);

    let painted = paint(&mut transcript, &messages, (400.0, 600.0), 0.0);

    let update = painted.accessibility.tree_update("Test", None);
    let item = update
        .nodes
        .iter()
        .find(|(_, n)| n.role() == accesskit::Role::ListItem && n.position_in_set() == Some(2501))
        .map(|(_, n)| n)
        .expect("row 2500 is published");
    let texts: Vec<&str> = item
        .children()
        .iter()
        .filter_map(|id| update.nodes.iter().find(|(nid, _)| nid == id))
        .filter_map(|(_, n)| n.value())
        .collect();
    assert_eq!(
        (item.label(), item.size_of_set(), texts),
        (
            Some("author 2500"),
            Some(5000),
            vec!["Message 2500 wraps across a few lines when the column is narrow enough."]
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

    let mut actions = Vec::new();
    painted
        .router
        .pointer_down(from.x - 5.0, top + from.y + 5.0, &mut None);
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
