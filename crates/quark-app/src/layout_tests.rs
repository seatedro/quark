//! Grid placement and sticky section headers through the whole adapter
//! path, read back as node bounds from [`UiTestHarness`].

use quark_ui::element::{AnyElement, Div, IntoAnyElement, ScrollHandle, div, sticky_section};
use quark_ui::style::Styled;
use quark_ui::style::track::{
    auto, fit_content, fr, max_content, min_content, minmax, px, repeat_fill,
};

use crate::testing::{By, UiTestHarness};
use crate::ui::{UiApp, UiContext, ViewContext};

struct View(Box<dyn FnMut() -> AnyElement>);

impl UiApp for View {
    type Action = ();
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        (self.0)()
    }

    fn update(&mut self, _action: (), _cx: &mut UiContext) {}
}

fn harness(size: (f32, f32), view: impl FnMut() -> AnyElement + 'static) -> UiTestHarness<View> {
    let mut ui = UiTestHarness::new(View(Box::new(view)), size, 1.0);
    ui.frame();
    ui
}

/// `"id x,y wxh"` for each test id, joined with `"; "`.
fn placements(ui: &UiTestHarness<View>, ids: &[&str]) -> String {
    ids.iter()
        .map(|id| {
            let b = ui.find(By::test_id(*id)).bounds;
            format!("{id} {},{} {}x{}", b.x, b.y, b.width, b.height)
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// A 20 point tall grid cell named `id`.
fn cell(id: &str) -> Div {
    div().h(20.0).test_id(id)
}

/// A cell whose content is two boxes, 30 and 50 wide, that wrap onto
/// separate lines when narrow: min-content 50, max-content 80.
fn wrapping(id: &str) -> Div {
    div()
        .flex_row()
        .flex_wrap()
        .test_id(id)
        .child(div().w(30.0).h(20.0).flex_shrink_0())
        .child(div().w(50.0).h(20.0).flex_shrink_0())
}

#[test]
fn grid_places_children_by_tracks_spans_lines_and_flow() {
    type Build = fn() -> Div;
    let cases: [(&str, Build, &str); 10] = [
        (
            "fixed and fr columns",
            || {
                div()
                    .grid_cols([px(100.0), fr(1.0)])
                    .child(cell("a"))
                    .child(cell("b"))
            },
            "a 0,0 100x20; b 100,0 200x20",
        ),
        (
            "equal columns wrap into rows with gaps",
            || {
                div()
                    .grid_cols_n(3)
                    .gap(15.0)
                    .children_from(["a", "b", "c", "d"].map(cell))
            },
            "a 0,0 90x20; d 0,35 90x20",
        ),
        (
            "column span",
            || {
                div()
                    .grid_cols_n(3)
                    .child(cell("a").col_span(2))
                    .child(cell("b"))
            },
            "a 0,0 200x20; b 200,0 100x20",
        ),
        (
            "explicit lines, auto items fill around them",
            || {
                div()
                    .grid_cols_n(3)
                    .child(cell("a").col_start(3).row_start(1))
                    .child(cell("b"))
                    .child(cell("c").col_start(1).col_end(-1))
            },
            "a 200,0 100x20; b 0,0 100x20; c 0,20 300x20",
        ),
        (
            "row span",
            || {
                div()
                    .grid_cols_n(2)
                    .grid_auto_rows(px(20.0))
                    .child(div().test_id("a").row_span(2))
                    .children_from(["b", "c"].map(cell))
            },
            "a 0,0 150x40; b 150,0 150x20; c 150,20 150x20",
        ),
        (
            "column flow",
            || {
                div()
                    .grid_rows([px(20.0), px(20.0)])
                    .grid_flow_col()
                    .grid_auto_cols(px(50.0))
                    .children_from(["a", "b", "c"].map(|id| div().test_id(id)))
            },
            "a 0,0 50x20; b 0,20 50x20; c 50,0 50x20",
        ),
        (
            "minmax keeps a floor under fr",
            || {
                div()
                    .grid_cols([minmax(px(250.0), fr(1.0)), fr(1.0)])
                    .child(cell("a"))
                    .child(cell("b"))
            },
            "a 0,0 250x20; b 250,0 50x20",
        ),
        (
            "content keywords",
            || {
                div()
                    .grid_cols([min_content(), max_content(), fit_content(60.0), auto()])
                    .items_start()
                    .child(wrapping("min"))
                    .child(wrapping("max"))
                    .child(wrapping("fit"))
            },
            "min 0,0 50x40; max 50,0 80x20; fit 130,0 60x40",
        ),
        (
            "auto-fill repeats as many tracks as fit",
            || {
                div()
                    .grid_cols([repeat_fill([px(90.0)])])
                    .children_from(["a", "b", "c", "d"].map(cell))
            },
            "c 180,0 90x20; d 0,20 90x20",
        ),
        (
            "aspect ratio sets the free side",
            || {
                div()
                    .grid_cols([px(120.0), fr(1.0)])
                    .items_start()
                    .child(div().test_id("a").aspect_ratio(2.0))
            },
            "a 0,0 120x60",
        ),
    ];
    for (name, build, expected) in cases {
        let ui = harness((300.0, 200.0), move || build().w(300.0).into_any());
        let ids: Vec<&str> = expected
            .split("; ")
            .map(|part| part.split(' ').next().unwrap())
            .collect();
        assert_eq!(placements(&ui, &ids), expected, "{name}");
    }
}

#[test]
fn sticky_header_holds_at_the_viewport_top_until_its_section_ends() {
    // Three 150 point sections (30 header, 120 body) in a 200 point
    // viewport. Section 0 spans 0..150 of the content.
    let cases = [
        ("unscrolled", 0.0, "h0 0,0 200x30; h1 0,150 200x30"),
        ("mid section", 60.0, "h0 0,0 200x30; h1 0,90 200x30"),
        (
            "pushed off by the next",
            140.0,
            "h0 0,-20 200x30; h1 0,10 200x30",
        ),
    ];
    for (name, scroll, expected) in cases {
        let handle = ScrollHandle::new();
        let view_handle = handle.clone();
        let mut ui = harness((200.0, 200.0), move || {
            div()
                .size_full()
                .flex_col()
                .track_scroll(&view_handle)
                .overflow_y_scroll()
                .children_from((0..3).map(|i| {
                    sticky_section(div().h(30.0).test_id(format!("h{i}")), div().h(120.0))
                        .flex_shrink_0()
                }))
                .into_any()
        });
        handle.set_offset(0.0, scroll);
        ui.frame();
        assert_eq!(placements(&ui, &["h0", "h1"]), expected, "{name}");
    }
}
