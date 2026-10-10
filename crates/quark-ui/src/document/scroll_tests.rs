//! Pixel scrolling of a document: wheel and fling input routed to its
//! handle, smooth scrolls, and the rows each frame prepares and paints.

use std::collections::HashMap;

use quark::reactive::SignalStore;
use quark::selection::BlockKey;
use quark::{SemanticFrame, scene::Scene};
use quark_text::{LayoutCache, TextSystem};

use super::*;
use crate::element::{
    AnyElement, ElementContext, InputRouter, IntoAnyElement, WheelEvent, div, render_element,
};
use crate::style::Styled;
use crate::theme::Theme;

/// Window size; the document fills it unless a test wraps it.
const W: f32 = 400.0;
const H: f32 = 300.0;

/// Every block is one 20-point line, so with [`style`] every text row is
/// 32 points tall (6 padding, 20, 6), its block starting 6 below its top,
/// and a row's estimate before measuring is the same 32.
struct Lines;

#[derive(Debug, Clone)]
struct Line;

impl BlockGeometry for Line {
    fn height(&self) -> f32 {
        20.0
    }

    fn hit(&self, _x: f32, _y: f32) -> usize {
        0
    }
}

impl BlockMeasurer for Lines {
    type Geometry = Line;

    fn measure(&mut self, _block: &Block, _width: f32) -> Line {
        Line
    }
}

fn style() -> DocumentStyle {
    DocumentStyle::for_font_size(10.0)
}

/// Row `i`: one block "row {i}", keyed `i`.
fn text_row(i: u64) -> DocumentRow {
    DocumentRow::new(
        RowKey(i),
        RowChrome::default(),
        vec![Block::plain(BlockKey(i), format!("row {i}"))],
    )
}

/// A [`Lines`] document of `rows`, painted at the window's top left.
struct View {
    rows: HashMap<RowKey, DocumentRow>,
    doc: Document<Line>,
    text: TextSystem,
    layouts: LayoutCache,
    router: InputRouter,
    reduced_motion: bool,
}

impl View {
    fn new(rows: impl IntoIterator<Item = DocumentRow>) -> Self {
        let rows: Vec<DocumentRow> = rows.into_iter().collect();
        let mut doc = Document::new(style());
        doc.extend(&rows).unwrap();
        Self {
            rows: rows.into_iter().map(|r| (r.key, r)).collect(),
            doc,
            text: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
            router: InputRouter::default(),
            reduced_motion: false,
        }
    }

    /// Prepares and paints a frame at clock `now_ms`; returns each painted
    /// block's text and top, top to bottom.
    fn frame(&mut self, now_ms: u64) -> Vec<(String, f32)> {
        let (doc, rows) = (&mut self.doc, &self.rows);
        doc.prepare(W, H, now_ms, self.reduced_motion, rows, &mut Lines);
        let theme = Theme::default_dark();
        let root = doc.element(rows, &theme, |_| crate::element::NoopAction.into());
        let (router, painted) = paint(root.into_any(), (&mut self.text, &mut self.layouts), now_ms);
        self.router = router;
        painted
    }

    fn wheel(&mut self, dy: f32, now_ms: u64) {
        let event = WheelEvent {
            dx: 0.0,
            dy,
            now_ms,
        };
        self.router.scroll_wheel(W / 2.0, H / 2.0, event);
    }
}

/// Paints `root` in a `W`x`H` window at clock `now_ms`; returns the router
/// over the frame and each painted text's content and top.
fn paint(
    root: AnyElement,
    (text, layouts): (&mut TextSystem, &mut LayoutCache),
    now_ms: u64,
) -> (InputRouter, Vec<(String, f32)>) {
    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, text, layouts, None, &signals).with_clock(now_ms);
    cx.semantic = SemanticFrame::new(W, H);
    let mut root = root;
    render_element(&mut root, &mut Scene::default(), &mut cx, W, H);
    let painted = cx
        .selectable_text_runs
        .iter()
        .map(|r| (r.text.as_str().to_owned(), r.bounds.y))
        .collect();
    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    (router, painted)
}

/// The topmost painted text reaching into the viewport, as `"text@y"`.
fn top(painted: &[(String, f32)]) -> String {
    painted
        .iter()
        .find(|(_, y)| y + 20.0 > 0.0)
        .map_or("-".to_owned(), |(text, y)| format!("{text}@{y}"))
}

// Catches offsets narrowed to f32 anywhere between the wheel and the
// glyphs: 18 million points down, f32 steps by 2, so quarter-point wheel
// deltas would not move the rows at all.
#[test]
fn fractional_wheel_moves_a_document_past_f32_precision() {
    // Three rows six million points tall above twenty text rows.
    let tall = |i: u64| {
        let spacer = RowAdornment::new(
            AdornmentKey(0),
            AdornmentSlot::Start,
            6_000_000.0,
            0,
            |_| div().into_any(),
        );
        DocumentRow::new(RowKey(i), RowChrome::default(), Vec::new()).with_adornments(vec![spacer])
    };
    let mut view = View::new((0..3).map(tall).chain((3..23).map(text_row)));
    // A frame tall enough to measure every row, then the window.
    let (doc, rows) = (&mut view.doc, &view.rows);
    doc.prepare(W, 2.0e7, 0, false, rows, &mut Lines);
    let start = view.doc.list().rows().offset_of(RowKey(5)).unwrap();
    assert!(start > f64::from(1u32 << 24));
    view.doc.scroll_handle().set_offset(0.0, start);
    view.frame(0);

    let mut moved = Vec::new();
    for step in 1..=4 {
        view.wheel(0.25, step);
        let painted = view.frame(step);
        moved.push((view.doc.scroll_offset_f64() - start, top(&painted)));
    }

    assert_eq!(
        moved,
        [
            (0.25, "row 5@5.75".to_owned()),
            (0.5, "row 5@5.5".to_owned()),
            (0.75, "row 5@5.25".to_owned()),
            (1.0, "row 5@5".to_owned()),
        ]
    );
}

// Catches motion advanced anywhere but before the rows are prepared: a
// frame would paint rows laid out for another offset, leaving blank
// strips. Each frame's painted rows sit where the frame's offset puts
// them (row k's block at 32k + 6 in the content).
#[test]
fn document_motion_prepares_the_rows_it_paints() {
    type Start = fn(&mut View);
    let smooth: Start = |view| view.doc.scroll_handle().animate_to(0.0, 640.0);
    let fling: Start = |view| {
        // Five 20-point moves 10 ms apart: 2 points/ms when fingers lift.
        for i in 0..5 {
            view.wheel(20.0, 960 + i * 10);
        }
        assert!(view.router.fling(1000));
    };
    // Name, reduced motion, how motion starts at t=1000, a 10-point wheel
    // at t=1120, and the offsets of the frames at the times below. Halfway
    // through its 220 ms, an ease-out cubic has covered 7/8 of the way.
    let frames = [1000, 1110, 1220, 1300, 1600, 3000, 9000];
    type Case = (&'static str, bool, Start, bool, Option<[f64; 7]>);
    let cases: [Case; 4] = [
        (
            "smooth",
            false,
            smooth,
            false,
            Some([0.0, 560.0, 640.0, 640.0, 640.0, 640.0, 640.0]),
        ),
        (
            "smooth, reduced motion",
            true,
            smooth,
            false,
            Some([640.0; 7]),
        ),
        (
            "smooth, then a wheel",
            false,
            smooth,
            true,
            Some([0.0, 560.0, 570.0, 570.0, 570.0, 570.0, 570.0]),
        ),
        ("fling", false, fling, false, None),
    ];
    for (name, reduced_motion, start, interrupt, expected) in cases {
        let mut view = View::new((0..100).map(text_row));
        view.reduced_motion = reduced_motion;
        view.doc.set_scroll_offset(0.0);
        view.frame(900);
        start(&mut view);

        let mut offsets = Vec::new();
        for now in frames {
            if interrupt && now == 1220 {
                view.wheel(10.0, 1120);
            }
            let painted = view.frame(now);
            let offset = view.doc.scroll_offset_f64();
            for (text, y) in &painted {
                let k: f64 = text.trim_start_matches("row ").parse().unwrap();
                let at = 32.0 * k + 6.0 - offset;
                assert!(
                    (f64::from(*y) - at).abs() < 1e-3,
                    "{name} at {now}: {text}@{y}"
                );
            }
            offsets.push(offset);
        }

        assert!(!view.doc.wants_frame(), "{name} settles");
        match expected {
            Some(expected) => assert_eq!(offsets, expected, "{name}"),
            None => {
                // Coasts on past the last wheel, slowing, and stops within
                // velocity * 325 ms of it.
                assert!(offsets.windows(2).all(|w| w[1] >= w[0]), "{offsets:?}");
                assert!(offsets[1] > offsets[0], "{offsets:?}");
                assert!(offsets[6] <= offsets[0] + 650.0, "{offsets:?}");
            }
        }
    }
}

// Catches per-axis chaining lost when the document took a handle: over
// wide code in a document in a scrolling parent, each axis moves the
// innermost content that can move that way.
#[test]
fn document_scroll_axes_chain_at_their_limits() {
    const WIDE: &str = "let wide = \"a line of code far wider than any column it could sit in, \
                        so it scrolls sideways under its own scrollbar\";";
    let rows: HashMap<RowKey, DocumentRow> = (0..14)
        .map(|i| {
            let mut row = DocumentRow::new(
                RowKey(i),
                RowChrome::default(),
                vec![Block::plain(BlockKey(i * 10), format!("Row {i} text."))],
            );
            if i == 6 {
                let line = vec![crate::element::StyledSpan::plain(WIDE)];
                row.blocks
                    .push(Block::code(BlockKey(i * 10 + 1), vec![line]));
            }
            (row.key, row)
        })
        .collect();
    let code = BlockKey(61);
    // Document top or bottom, parent offset, wheel; then how far the code
    // moved sideways, the document down, and where the parent ends up.
    type Case = (&'static str, bool, (f32, f32), (f32, f32));
    type Moved = (f32, f64, (f32, f32));
    let cases: [(Case, Moved); 3] = [
        (
            ("both inner axes free", false, (0.0, 0.0), (30.0, 30.0)),
            (30.0, 30.0, (0.0, 0.0)),
        ),
        (
            ("document at its bottom", true, (0.0, 0.0), (30.0, 30.0)),
            (30.0, 0.0, (0.0, 30.0)),
        ),
        (
            ("code at its start", false, (40.0, 0.0), (-30.0, 30.0)),
            (0.0, 30.0, (10.0, 0.0)),
        ),
    ];
    for ((name, bottom, parent_at, (dx, dy)), expected) in cases {
        let mut keys: Vec<RowKey> = rows.keys().copied().collect();
        keys.sort();
        let mut doc = Document::new(DocumentStyle::for_font_size(14.0));
        doc.extend(keys.iter().map(|k| &rows[k])).unwrap();
        if !bottom {
            doc.set_scroll_offset(0.0);
        }
        let parent = crate::element::ScrollHandle::new();
        parent.set_offset(parent_at.0, parent_at.1);
        let (mut text, mut layouts) = (
            TextSystem::vendored_only(&Default::default()),
            LayoutCache::default(),
        );
        // The document is 400x400 inside a 300x300 parent, so the parent
        // scrolls 100 points each way.
        doc.prepare(
            400.0,
            400.0,
            0,
            false,
            &rows,
            &mut TextMeasurer::new(&mut text, &mut layouts, 14.0, 1.0),
        );
        let element = doc.element(&rows, &Theme::default_dark(), |_| {
            crate::element::NoopAction.into()
        });
        let root = div()
            .w(300.0)
            .h(300.0)
            .track_scroll(&parent)
            .overflow_scroll()
            .child(element)
            .into_any();
        let (mut router, _) = paint(root, (&mut text, &mut layouts), 0);
        let rect = doc
            .visible_blocks()
            .iter()
            .find(|b| b.key == code)
            .unwrap()
            .rect;
        let before = doc.scroll_handle().offset_f64().1;

        let (x, y) = (rect.x + 40.0 - parent_at.0, rect.y + 10.0 - parent_at.1);
        assert!(
            x < 300.0 && (0.0..300.0).contains(&y),
            "{name}: code in view"
        );
        router.scroll_wheel(x, y, WheelEvent { dx, dy, now_ms: 0 });

        let moved = (
            doc.scroll_x(code),
            doc.scroll_handle().offset_f64().1 - before,
            parent.offset(),
        );
        assert_eq!(moved, expected, "{name}");
    }
}

// Catches a correction that moves the view without its motion: a row
// above the viewport measuring taller mid-scroll shifts the offset, and the
// smooth scroll must move with it, so the rows on screen follow the same
// path as when nothing changed.
#[test]
fn remeasuring_above_a_moving_document_preserves_its_path() {
    let mut views = [(); 2].map(|_| View::new((0..100).map(text_row)));
    for view in &mut views {
        view.doc.set_scroll_offset(320.0);
        view.frame(0);
        view.doc.scroll_handle().animate_to(0.0, 960.0);
        view.frame(1000);
        view.frame(1055);
    }
    // Row 25 is on screen as it grows, and by the next frame above the
    // viewport inside the measured overscan: a second block makes it 26
    // points taller.
    let [edited, unchanged] = &mut views;
    let grown = DocumentRow::new(
        RowKey(25),
        RowChrome::default(),
        vec![
            Block::plain(BlockKey(25), "row 25"),
            Block::plain(BlockKey(1025), "row 25, more"),
        ],
    );
    edited.doc.update(&grown).unwrap();
    edited.rows.insert(grown.key, grown);

    let on_screen = |painted: Vec<(String, f32)>| -> Vec<(String, f32)> {
        painted
            .into_iter()
            .filter(|(_, y)| (0.0..H).contains(y))
            .collect()
    };
    for now in [1110, 1165, 1220, 1500] {
        let seen = on_screen(edited.frame(now));
        assert_eq!(seen, on_screen(unchanged.frame(now)), "at {now}");
        assert!(!seen.is_empty());
    }
    assert_eq!(
        edited.doc.scroll_offset_f64() - unchanged.doc.scroll_offset_f64(),
        26.0
    );
}
