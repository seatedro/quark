//! Controls with a preferred width give way inside a narrower container
//! instead of spilling past it: a slider kept its 220pt track and pushed
//! its value out of a 300pt window, and a segmented control never shrank,
//! cutting "Month" off at the window edge.

use quark::Rect;
use quark::reactive::SignalStore;
use quark_components::{SegmentedControl, SegmentedItem, slider};
use quark_render::{Primitive, Scene};
use quark_ui::Action;
use quark_ui::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
struct Pick(&'static str);

impl From<Pick> for Action {
    fn from(value: Pick) -> Self {
        Action::new(value)
    }
}

struct Painted {
    scene: Scene,
    frame: quark::SemanticFrame,
}

/// Paint `control` in a row `width` wide, beside a fixed label as forms
/// lay them out.
fn paint(control: AnyElement, width: f32) -> Painted {
    let theme = Theme::default_light();
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store);
    let mut root = div()
        .w(width)
        .flex_row()
        .gap(8.0)
        .child(div().w(40.0).h(10.0).flex_shrink_0())
        .child(control)
        .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, width, 100.0);
    Painted {
        scene,
        frame: std::mem::take(&mut cx.semantic),
    }
}

fn bounds(painted: &Painted, test_id: &str) -> Vec<Rect> {
    painted
        .frame
        .nodes()
        .iter()
        .filter(|n| n.test_id.as_ref().is_some_and(|t| t.as_str() == test_id))
        .map(|n| n.bounds)
        .collect()
}

fn text_runs(painted: &Painted) -> Vec<Rect> {
    painted
        .scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::TextRun(t) => Some(t.rect),
            _ => None,
        })
        .collect()
}

fn right(r: Rect) -> f32 {
    r.x + r.width
}

#[test]
fn slider_track_gives_way_so_its_value_stays_in_the_row() {
    // (row width, expected track width or None for "whatever fits")
    for (width, track_width) in [(400.0, Some(220.0)), (200.0, None), (140.0, None)] {
        let painted = paint(
            slider("volume", "Volume", 40.0, 0.0, 100.0, |_| Pick("v").into()).into_any(),
            width,
        );
        let track = bounds(&painted, "slider")[0];
        let value = text_runs(&painted)[0];
        assert!(
            right(track) <= value.x && right(value) <= width,
            "{width}: track {track:?}, value {value:?}"
        );
        if let Some(expected) = track_width {
            assert_eq!(track.width, expected, "{width}: preferred width");
        }
    }
}

fn segmented(width: f32) -> (Painted, Rect, Vec<Rect>, Vec<Rect>) {
    let control = SegmentedControl::new(vec![
        SegmentedItem::new("Day", Pick("day"), true),
        SegmentedItem::new("Week", Pick("week"), false),
        SegmentedItem::new("Month", Pick("month"), false),
    ])
    .id("view")
    .into_any();
    let painted = paint(control, width);
    let whole = bounds(&painted, "segmented-control")[0];
    let segments = bounds(&painted, "segmented-item");
    let labels = text_runs(&painted);
    (painted, whole, segments, labels)
}

#[test]
fn segmented_control_fits_its_row_and_keeps_labels_inside_segments() {
    for width in [300.0, 150.0] {
        let (_, whole, segments, labels) = segmented(width);
        assert!(right(whole) <= width, "{width}: control {whole:?}");
        assert_eq!((segments.len(), labels.len()), (3, 3));
        for (segment, label) in segments.iter().zip(&labels) {
            assert!(
                label.x >= segment.x && right(*label) <= right(*segment) + 0.5,
                "{width}: label {label:?} outside {segment:?}"
            );
        }
    }
}

// A row too narrow for the padding but wide enough for the labels takes
// the space from the padding: no label truncates.
#[test]
fn segmented_control_gives_up_padding_before_truncating_labels() {
    let natural: Vec<f32> = segmented(400.0).3.iter().map(|r| r.width).collect();
    let (_, whole, _, labels) = segmented(200.0);
    assert!(right(whole) <= 200.0, "control spilled: {whole:?}");
    let widths: Vec<f32> = labels.iter().map(|r| r.width).collect();
    assert_eq!(widths, natural);
}
