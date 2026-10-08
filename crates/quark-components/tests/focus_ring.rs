//! Focus rings keep clear of what they ring: a control without a boundary
//! of its own (a radio row, a checkbox, a switch) reaches its bounds with
//! its marks and label, so a ring drawn against those bounds touched the
//! label and cut into the radio circle and switch track.

use quark::reactive::SignalStore;
use quark::{Color, Rect};
use quark_components::{RadioOption, checkbox, radio_group, switch};
use quark_render::{BorderPrimitive, Primitive, Scene};
use quark_ui::accessibility::AccessibilityFrame;
use quark_ui::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};

#[derive(Debug, Clone, PartialEq)]
struct Pick(&'static str);

impl From<Pick> for Action {
    fn from(value: Pick) -> Self {
        Action::new(value)
    }
}

const SIZE: (f32, f32) = (320.0, 120.0);

type Control = fn() -> AnyElement;

/// Paint `control` alone, padded, with `focus`; return the scene and each
/// labeled node's focus target.
fn paint(control: Control, focus: Option<FocusId>) -> (Scene, Vec<(String, FocusId)>) {
    let theme = Theme::default_light();
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let mut cx =
        ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store).with_focus(focus);
    cx.accessibility = AccessibilityFrame::new(SIZE.0, SIZE.1);
    let mut root = div().p(24.0).child(control()).into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, SIZE.0, SIZE.1);
    let frame = &cx.semantic;
    let targets = (0..frame.nodes().len())
        .filter_map(|i| {
            Some((
                frame.nodes()[i].label.as_deref()?.to_owned(),
                frame.focus_id(i)?,
            ))
        })
        .collect();
    (scene, targets)
}

/// How far `point` is inside the rounded rect `rect` with corner `radius`;
/// negative outside.
fn depth_inside(rect: Rect, radius: f32, point: (f32, f32)) -> f32 {
    let (hw, hh) = (rect.width / 2.0, rect.height / 2.0);
    let qx = (point.0 - (rect.x + hw)).abs() - (hw - radius);
    let qy = (point.1 - (rect.y + hh)).abs() - (hh - radius);
    let outside = qx.max(0.0).hypot(qy.max(0.0));
    -(outside + qx.max(qy).min(0.0) - radius)
}

/// The least clearance between the ring's inner edge and anything else the
/// control paints. The ring's inside is convex, so the closest point of a
/// painted rect is one of its corners.
fn clearance(ring: &BorderPrimitive, content: &[Rect]) -> f32 {
    let w = ring.widths[0];
    let inner = Rect {
        x: ring.rect.x + w,
        y: ring.rect.y + w,
        width: ring.rect.width - 2.0 * w,
        height: ring.rect.height - 2.0 * w,
    };
    let radius = (ring.corner_radii[0] - w).max(0.0);
    content
        .iter()
        .flat_map(|r| {
            [
                (r.x, r.y),
                (r.x + r.width, r.y),
                (r.x, r.y + r.height),
                (r.x + r.width, r.y + r.height),
            ]
        })
        .map(|p| depth_inside(inner, radius, p))
        .fold(f32::INFINITY, f32::min)
}

#[test]
fn focus_rings_keep_clear_of_the_control_they_ring() {
    let ring_color: Color = Theme::default_light().colors.focus_border;
    let cases: [(&str, Control); 3] = [
        ("Remember me", || {
            checkbox(true)
                .label("Remember me")
                .on_toggle(Pick("remember"))
                .into_any()
        }),
        ("Wi-Fi", || {
            switch(true)
                .label("Wi-Fi")
                .on_toggle(Pick("wifi"))
                .into_any()
        }),
        ("Medium", || {
            radio_group(
                "size",
                "Size",
                vec![RadioOption::new("Medium")],
                Some(0),
                |_| Pick("size").into(),
            )
            .into_any()
        }),
    ];
    for (label, control) in cases {
        let (_, targets) = paint(control, None);
        let target = targets
            .iter()
            .find(|(l, _)| l == label)
            .map(|(_, f)| *f)
            .unwrap_or_else(|| panic!("no focus target for {label}"));
        let (scene, _) = paint(control, Some(target));
        // The ring is the outermost border in the ring's color; a checked
        // control's accent border can share that color.
        let mut ring: Option<BorderPrimitive> = None;
        let mut content = Vec::new();
        for primitive in &scene.primitives {
            match primitive {
                Primitive::Border(b) if b.color == ring_color => {
                    let area = |b: &BorderPrimitive| b.rect.width * b.rect.height;
                    match ring.replace(*b) {
                        Some(other) if area(&other) > area(b) => {
                            content.push(b.rect);
                            ring = Some(other);
                        }
                        Some(other) => content.push(other.rect),
                        None => {}
                    }
                }
                Primitive::Border(b) => content.push(b.rect),
                Primitive::Rect(r) => content.push(r.rect),
                Primitive::RoundedRect(r) => content.push(r.rect),
                Primitive::TextRun(t) => content.push(t.rect),
                _ => {}
            }
        }
        let ring = ring.unwrap_or_else(|| panic!("{label}: no ring"));
        let gap = clearance(&ring, &content);
        assert!(gap >= 1.0, "{label}: ring is {gap:.1}pt from the control");
    }
}
