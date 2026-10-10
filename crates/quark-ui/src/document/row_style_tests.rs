//! Rows narrowed, aligned, padded, and rounded by their [`RowStyle`].

use quark::SemanticFrame;
use quark::reactive::SignalStore;
use quark::scene::Scene;
use quark_text::{LayoutCache, TextSystem};

use super::tests::{Ev, paint, real_document, real_view};
use super::*;
use crate::element::{ElementContext, IntoAnyElement, render_element};
use crate::theme::Theme;
use crate::virtual_list::ScrollAlign;

/// A row box 240 points wide at `align`, with `padding`.
fn bubble(align: RowAlign, padding: Option<[f32; 4]>) -> RowStyle {
    RowStyle {
        max_width: Some(240.0),
        align,
        padding,
        ..RowStyle::default()
    }
}

// Catches a narrowed row measured, painted, and hit tested at different
// places: its row x counted twice or not at all, or its padding left out
// of one of them. Dragging from the glyphs of the styled row 1 into row 2
// copies exactly the text between them, and the text is painted where it
// was measured. A document column narrower than the viewport moves every
// row into it the same way.
#[test]
fn a_narrow_end_aligned_row_selects_the_text_it_paints() {
    let padded = Some([4.0, 24.0, 8.0, 12.0]);
    // A 400-point viewport, the document's pad_x 17 and pad_y 8, and a
    // 20-point header band; a 300-point column sits 50 points in.
    let cases = [
        (RowAlign::Start, None, None, "box 0+240 block 17,28"),
        (RowAlign::End, None, None, "box 160+240 block 177,28"),
        (RowAlign::Center, None, None, "box 80+240 block 97,28"),
        (RowAlign::End, padded, None, "box 160+240 block 172,24"),
        (RowAlign::Start, None, Some(300.0), "box 50+240 block 67,28"),
        (RowAlign::End, None, Some(300.0), "box 110+240 block 127,28"),
    ];
    for (align, padding, column, expected) in cases {
        let mut rows = real_document(3);
        rows.get_mut(&RowKey(1)).unwrap().chrome.style = bubble(align, padding);
        let mut view = real_view(&rows);
        view.set_style(DocumentStyle {
            max_column: column,
            ..*view.style()
        });
        view.set_scroll_offset(0.0);
        let mut painted = paint(&mut view, &rows, (400.0, 600.0), 0.0);
        let row = view.visible_rows()[1].clone();
        let block = view.visible_blocks()[row.blocks.clone()][0].clone();
        let region = |key: u64| {
            painted
                .regions
                .iter()
                .find(|r| r.source_key == key)
                .unwrap()
                .clone()
        };
        let (styled, below) = (region(10), region(20));
        let painted_where_measured = styled.bounds == block.rect
            && styled.text_origin
                == (
                    block.rect.x + block.geometry.text_origin.0,
                    block.rect.y + block.geometry.text_origin.1,
                );
        // From the start of "wraps" in row 1 to the end of "Message 2".
        let at = |region: &crate::element::SelectableTextRegion, offset: usize| {
            let caret = region.layout.caret(offset);
            (
                region.text_origin.0 + caret.x,
                region.text_origin.1 + caret.y + caret.height * 0.5,
            )
        };
        let ((x0, y0), (x1, y1)) = (at(&styled, 10), at(&below, 9));

        let mut actions = painted.router.pointer_down(x0, y0, &mut None).actions;
        actions.extend(painted.router.pointer_move(x1, y1).actions);
        actions.extend(painted.router.pointer_up().actions);
        for action in &actions {
            if let Some(Ev(event)) = action.downcast_ref::<Ev>() {
                view.handle(*event);
            }
        }

        let geometry = format!(
            "box {}+{} block {},{}",
            row.rect.x, row.rect.width, block.rect.x, block.offset_in_row
        );
        assert_eq!(
            (
                geometry.as_str(),
                painted_where_measured,
                view.selected_text(&rows)
            ),
            (
                expected,
                true,
                "wraps across a few lines when the column is narrow enough.\n\nMessage 2"
                    .to_owned()
            ),
            "{align:?} {padding:?} {column:?}"
        );
    }
}

// Catches find placing highlights or the reveal by a full-width layout in
// a narrowed row: the match in an offscreen bubble scrolls into view, and
// its highlight covers exactly the glyphs painted for it.
#[test]
fn find_reveals_and_highlights_text_in_a_narrow_row() {
    let mut rows = real_document(40);
    rows.get_mut(&RowKey(30)).unwrap().chrome.style = RowStyle {
        max_width: Some(200.0),
        align: RowAlign::End,
        padding: Some([6.0, 10.0, 6.0, 30.0]),
        ..RowStyle::default()
    };
    let mut view = real_view(&rows);
    view.set_scroll_offset(0.0);
    let size = (400.0, 300.0);
    paint(&mut view, &rows, size, 0.0);
    let shown_before = view.visible_rows().iter().any(|r| r.key == RowKey(30));
    let query = "Message 30 wraps";
    view.set_find_query(query, &rows);
    view.find_next(ScrollAlign::Center);

    let painted = paint(&mut view, &rows, size, 0.0);

    let current = Theme::default_dark().colors.search_match_active_bg;
    let fmt = |r: &Rect| format!("{:.1},{:.1} {:.1}x{:.1}", r.x, r.y, r.width, r.height);
    let highlights: Vec<Rect> = painted
        .scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            quark_render::Primitive::RoundedRect(rr) if rr.color == current => Some(rr.rect),
            _ => None,
        })
        .collect();
    let region = painted
        .regions
        .iter()
        .find(|r| r.source_key == 300)
        .expect("row 30 is painted");
    let start = region.layout.text().find(query).unwrap();
    let glyphs: Vec<String> = region
        .layout
        .selection_rects(start..start + query.len())
        .map(|r| {
            fmt(&Rect {
                x: region.text_origin.0 + r.x,
                y: region.text_origin.1 + r.y,
                width: r.width,
                height: r.height,
            })
        })
        .collect();
    // Inside the viewport, and inside the row's box at its right end.
    let in_view = highlights
        .iter()
        .all(|r| r.y >= 0.0 && r.y + r.height <= size.1 && r.x >= 200.0 && r.x + r.width <= size.0);
    assert_eq!(
        (
            shown_before,
            highlights.iter().map(fmt).collect::<Vec<_>>(),
            in_view
        ),
        (false, glyphs, true)
    );
}

// Catches the fill, its corners, or the row's content escaping the row
// box: the fill covers the box's padding, the rounded corner shows the
// window behind it, and the margin the row leaves is not filled. Runs
// only on a host with a GPU.
#[test]
fn a_rounded_row_paints_inside_its_box() {
    const FILL: Color = Color::rgba(220, 40, 40, 255);
    let (width, height) = (400u32, 200u32);
    let chrome = RowChrome {
        style: RowStyle {
            max_width: Some(200.0),
            align: RowAlign::End,
            background: Some(FILL),
            corner_radius: 24.0,
            padding: Some([60.0, 20.0, 60.0, 20.0]),
        },
        ..RowChrome::default()
    };
    let row = DocumentRow::new(RowKey(0), chrome, vec![Block::plain(BlockKey(0), "hi")]);
    let rows: HashMap<RowKey, DocumentRow> = [(row.key, row)].into();
    let mut view = real_view(&rows);
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    let (w, h) = (width as f32, height as f32);
    view.prepare(
        w,
        h,
        0,
        false,
        &rows,
        &mut TextMeasurer::new(&mut text, &mut layouts, 14.0, 1.0),
    );
    let theme = Theme::default_dark();
    let element = view.element(&rows, &theme, |ev| Ev(ev).into());
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    cx.semantic = SemanticFrame::new(w, h);
    let mut scene = Scene::default();
    render_element(&mut element.into_any(), &mut scene, &mut cx, w, h);
    drop(cx);
    let mut renderer = match quark_render::Renderer::new_headless(width, height, 1.0) {
        Ok(renderer) => renderer,
        Err(quark_render::RenderError::NoAdapter) => {
            assert!(
                std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
            );
            eprintln!("skipping: no wgpu adapter available");
            return;
        }
        Err(error) => panic!("headless render failed: {error}"),
    };

    let pixels = renderer
        .render_to_rgba(&scene, &mut text, width, height)
        .expect("render");

    let filled = |x: u32, y: u32| {
        let i = ((y * width + x) * 4) as usize;
        let fill = [FILL.r, FILL.g, FILL.b, FILL.a];
        (0..4).all(|c| pixels[i + c].abs_diff(fill[c]) <= 2)
    };
    // In the left padding, at the box's top left corner, in the margin.
    assert_eq!(
        [filled(210, 70), filled(201, 1), filled(100, 70)],
        [true, false, false]
    );
}
