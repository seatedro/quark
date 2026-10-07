use std::sync::Arc;

use quark::SemanticFrame;
use quark::reactive::SignalStore;
use quark_render::{Primitive, Rect, Scene};
use quark_text::{LayoutCache, TextSystem};

use super::*;
use crate::element::{
    CursorHint, ElementContext, InputRouter, IntoAnyElement, LinkClicked, SelectableTextRegion,
    render_element,
};
use crate::theme::Theme;

#[test]
fn blocks_dump_matches_commonmark_structure() {
    let cases = [
        (
            "Hello *world* and **bold _both_**",
            "p: Hello [i:world] and [b:bold ][b,i:both]",
        ),
        ("# One\n## Two\n###### Six", "h1: One\nh2: Two\nh6: Six"),
        (
            "- a\n- b\n  - c\n\n1. x\n2. y",
            "p -: a\np -: b\n  p -: c\np 1.: x\np 2.: y",
        ),
        ("3. three\n4. four", "p 3.: three\np 4.: four"),
        ("- item\n\n  second para", "p -: item\np: second para"),
        ("- [ ] todo\n- [x] done", "p [ ]: todo\np [x]: done"),
        ("> quote\n>> nested", ">p: quote\n>>p: nested"),
        ("> - in quote", ">p -: in quote"),
        (
            "```rust title\nfn main() {}\n\n```",
            "code(rust): [c:fn main() {}\\n]",
        ),
        ("    indented", "code(): [c:indented]"),
        (
            "a ~~gone~~ `code` [link **b**](https://x.y) ![alt](i.png) ![](j.png)",
            "p: a [s:gone] [c:code] [l=https://x.y:link ][b,l=https://x.y:b] [img:[alt]] [img:[image]]",
        ),
        (
            "line one\nline two  \nline three",
            "p: line one\\nline two\\nline three",
        ),
        ("a\n\n---\n\nb", "p: a\nhr: \np: b"),
        (
            "| a | b |\n|---|:-:|\n| 1 | **2** |\n| | x |",
            "table2: r0c0:a r0c1:b r1c0:1 r1c1:[b:2] r2c0: r2c1:x",
        ),
        ("-\n- b", "p -: \np -: b"),
    ];
    for (input, expected) in cases {
        let doc = MarkdownDoc::parse(input);
        assert_eq!(doc.verify_integrity(), Ok(()), "{input:?}");
        assert_eq!(doc.dump(), expected, "{input:?}");
    }
}

#[test]
fn plain_text_keeps_source_markers() {
    let cases = [
        ("## Two *x*", "## Two x"),
        ("- [x] done", "- [x] done"),
        ("7. seventh", "7. seventh"),
        ("> > deep\n> > two", "> > deep\n> > two"),
        ("```\nfn a() {}\nfn b() {}\n```", "fn a() {}\nfn b() {}"),
        (
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "| a | b |\n| --- | --- |\n| 1 | 2 |",
        ),
    ];
    for (input, expected) in cases {
        let doc = MarkdownDoc::parse(input);
        assert_eq!(doc.plain_text(0), expected, "{input:?}");
    }
}

#[test]
fn fit_columns_shrinks_only_columns_wider_than_their_share() {
    let cases: [(&[f32], f32, &[f32]); 3] = [
        (&[40.0, 60.0], 200.0, &[40.0, 60.0]),
        (&[40.0, 300.0, 500.0], 300.0, &[40.0, 130.0, 130.0]),
        (&[100.0, 100.0], 50.0, &[25.0, 25.0]),
    ];
    for (natural, avail, expected) in cases {
        assert_eq!(
            fit_columns(natural, avail),
            expected,
            "{natural:?} in {avail}"
        );
    }
}

use super::view::fit_columns;

/// Renders one frame and keeps what input and assertions need.
struct Frame {
    scene: Scene,
    regions: Vec<SelectableTextRegion>,
    router: InputRouter,
}

struct Harness {
    text: TextSystem,
    layouts: LayoutCache,
    theme: Theme,
    signals: SignalStore,
}

impl Harness {
    fn new() -> Self {
        Self {
            text: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
            theme: Theme::default_dark(),
            signals: SignalStore::new(),
        }
    }

    fn frame(&mut self, view: MarkdownView, mouse: Option<(f32, f32)>) -> Frame {
        let (w, h) = (600.0, 2000.0);
        self.layouts.begin_frame();
        let mut cx = ElementContext::new(
            &self.theme,
            1.0,
            &mut self.text,
            &mut self.layouts,
            mouse,
            &self.signals,
        );
        cx.semantic = SemanticFrame::new(w, h);
        let mut scene = Scene::default();
        let mut root = view.into_any();
        render_element(&mut root, &mut scene, &mut cx, w, h);
        let regions = std::mem::take(&mut cx.selectable_text_runs);
        let mut router = InputRouter::default();
        router.set_frame(cx.take_input_frame());
        Frame {
            scene,
            regions,
            router,
        }
    }
}

/// Scene-space center of the glyphs covering `needle` in the region whose
/// text contains it, plus the run's horizontal extent.
fn locate(frame: &Frame, needle: &str) -> ((f32, f32), (f32, f32)) {
    let region = frame
        .regions
        .iter()
        .find(|r| r.text.contains(needle))
        .expect("region with needle");
    let start = region.text.find(needle).unwrap_or(0);
    let rect = region
        .layout
        .selection_rects(start..start + needle.len())
        .next()
        .expect("glyph rect");
    let (ox, oy) = region.text_origin;
    (
        (
            ox + rect.x + rect.width / 2.0,
            oy + rect.y + rect.height / 2.0,
        ),
        (ox + rect.x, ox + rect.right()),
    )
}

fn decoration_quads(scene: &Scene) -> Vec<Rect> {
    scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Rect(r) => Some(r.rect),
            _ => None,
        })
        .collect()
}

const LINKED: &str = "see [docs](https://example.com/docs) now";

#[test]
fn link_click_routes_an_action_carrying_the_url() {
    let mut h = Harness::new();
    let probe = h.frame(markdown_view(LINKED).width(400.0), None);
    let ((x, y), _) = locate(&probe, "docs");

    let mut frame = h.frame(markdown_view(LINKED).width(400.0), None);
    let delivery = frame.router.pointer_down(x, y, &mut None);
    let cursor = frame.router.cursor_at(x, y);

    let expected = LinkClicked {
        url: Arc::from("https://example.com/docs"),
    };
    assert_eq!(delivery.actions, vec![expected.into()]);
    assert_eq!(cursor, CursorHint::Pointer);
}

#[test]
fn hovered_link_gets_an_underline_inside_its_glyphs() {
    let mut h = Harness::new();
    let away = h.frame(markdown_view(LINKED).width(400.0), Some((590.0, 1990.0)));
    let ((x, y), (x0, x1)) = locate(&away, "docs");
    let hovered = h.frame(markdown_view(LINKED).width(400.0), Some((x, y)));

    let quads = decoration_quads(&hovered.scene);
    assert!(decoration_quads(&away.scene).is_empty());
    assert_eq!(quads.len(), 1);
    assert!(quads[0].x >= x0 - 0.01 && quads[0].right() <= x1 + 0.01);
}

#[test]
fn strikethrough_span_paints_a_quad_over_its_run() {
    let mut h = Harness::new();
    let frame = h.frame(markdown_view("keep ~~gone~~ keep").width(400.0), None);
    let ((_, y), (x0, x1)) = locate(&frame, "gone");

    let quads = decoration_quads(&frame.scene);
    assert_eq!(quads.len(), 1);
    let q = quads[0];
    assert!(
        q.x >= x0 - 0.01 && q.right() <= x1 + 0.01,
        "{q:?} vs {x0}..{x1}"
    );
    // Through the glyphs: above the line's vertical center plus a margin.
    assert!(
        (q.y - y).abs() < 14.0 * 0.5,
        "{q:?} not across the text at {y}"
    );
}

#[test]
fn streaming_append_reshapes_only_the_last_block() {
    let mut h = Harness::new();
    let base = "# Title\n\nFirst paragraph with `code`.\n\n- one\n- two\n\nStreaming tail";
    let first = h.frame(markdown_view(base).width(400.0), None);
    let before = h.layouts.stats().misses;

    let grown = format!("{base} grows");
    let second = h.frame(markdown_view(&grown).width(400.0), None);

    assert_eq!(h.layouts.stats().misses - before, 1);
    let reused: Vec<bool> = first
        .regions
        .iter()
        .zip(&second.regions)
        .map(|(a, b)| Arc::ptr_eq(&a.layout, &b.layout))
        .collect();
    assert_eq!(reused, [true, true, true, true, false]);
}

#[test]
fn table_cells_wrap_within_the_view_width() {
    let mut h = Harness::new();
    let long = "word ".repeat(30);
    let md = format!("| key | value |\n|---|---|\n| k | {long} |\n| k2 | {long} |");
    let width = 300.0;
    let frame = h.frame(markdown_view(&md).width(width), None);

    let cells: Vec<&SelectableTextRegion> = frame.regions.iter().collect();
    assert_eq!(cells.len(), 6);
    for cell in &cells {
        assert!(
            cell.bounds.x + cell.bounds.width <= width + 0.01,
            "cell {:?} overflows",
            cell.text
        );
    }
    let wrapped = cells.iter().filter(|c| c.layout.line_count() > 1).count();
    assert_eq!(wrapped, 2, "both long cells wrap, short ones do not");
}

// Regression: span weights override SelectableText's base weight, so
// headings and table headers rendered at normal weight.
#[test]
fn heading_and_header_weights_reach_the_glyphs() {
    let mut h = Harness::new();
    let md = "# Title\n\n| head |\n|---|\n| body |\n\nplain";
    let frame = h.frame(markdown_view(md).width(400.0), None);

    let weights: Vec<(String, u16)> = frame
        .regions
        .iter()
        .map(|r| (r.text.to_string(), r.layout.glyphs().font_weight[0].0))
        .collect();
    let expected = [("Title", 700), ("head", 600), ("body", 450), ("plain", 450)];
    let expected: Vec<(String, u16)> = expected.iter().map(|(t, w)| (t.to_string(), *w)).collect();
    assert_eq!(weights, expected);
}
