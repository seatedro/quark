//! Controls painted in the high-contrast themes are readable against what
//! is actually behind them: text at 7:1, and icons, borders (against what
//! is outside them), focus rings, and small marks (a switch's thumb, a
//! radio's dot) at 3:1. Contrast is
//! measured on the painted scene, compositing the fills under each mark in
//! paint order, so a control that pairs tokens the themes did not plan for
//! fails here.

use quark::reactive::SignalStore;
use quark::{Color, Rect};
use quark_components::{
    Button, ButtonStyle, DropdownItem, RadioOption, SegmentedControl, SegmentedItem, TabItem,
    badge, checkbox, dropdown, radio_group, slider, switch, tab_bar,
};
use quark_render::{Primitive, Scene};
use quark_ui::accessibility::{AccessibilityFrame, dump_accessibility_states};
use quark_ui::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::{Theme, contrast_ratio};
use quark_ui::{Action, FocusId};

#[derive(Debug, Clone, PartialEq)]
struct Pick(&'static str);

impl From<Pick> for Action {
    fn from(value: Pick) -> Self {
        Action::new(value)
    }
}

const SIZE: (f32, f32) = (480.0, 640.0);
/// Opaque fills no larger than this on either side are marks that carry
/// state on their own (a thumb, a dot, a checked box), held to 3:1.
const MARK: f32 = 24.0;

/// Every enabled control whose colors carry meaning, in its notable states.
fn gallery(theme: &Theme) -> AnyElement {
    div()
        .w(SIZE.0)
        .h(SIZE.1)
        .p(16.0)
        .gap(12.0)
        .flex_col()
        .bg(theme.colors.background)
        .child(
            checkbox(true)
                .label("Remember me")
                .on_toggle(Pick("remember")),
        )
        .child(
            checkbox(false)
                .label("Subscribe")
                .on_toggle(Pick("subscribe")),
        )
        .child(switch(true).label("Wi-Fi").on_toggle(Pick("wifi")))
        .child(
            switch(false)
                .label("Bluetooth")
                .on_toggle(Pick("bluetooth")),
        )
        .child(radio_group(
            "size",
            "Size",
            vec![RadioOption::new("Small"), RadioOption::new("Large")],
            Some(0),
            |_| Pick("size").into(),
        ))
        .child(
            SegmentedControl::new(vec![
                SegmentedItem::new("Day", Pick("day"), true),
                SegmentedItem::new("Week", Pick("week"), false),
            ])
            .id("view"),
        )
        .child(slider("volume", "Volume", 40.0, 0.0, 100.0, |_| {
            Pick("volume").into()
        }))
        .child(
            div()
                .flex_row()
                .gap(8.0)
                .child(
                    Button::new(Pick("save"))
                        .label("Save")
                        .style(ButtonStyle::Filled),
                )
                .child(
                    Button::new(Pick("edit"))
                        .label("Edit")
                        .style(ButtonStyle::Subtle),
                )
                .child(
                    Button::new(Pick("skip"))
                        .label("Skip")
                        .style(ButtonStyle::Ghost),
                )
                .child(badge("New").accent()),
        )
        .child(tab_bar(vec![
            TabItem::new("Files", Pick("files")).active(true),
            TabItem::new("Logs", Pick("logs")),
        ]))
        .child(
            dropdown(
                "Sort",
                vec![
                    DropdownItem::new("Newest", Pick("newest")).selected(true),
                    DropdownItem::new("Oldest", Pick("oldest")),
                ],
            )
            .open(true)
            .on_toggle(Pick("sort")),
        )
        .into_any()
}

struct Painted {
    scene: Scene,
    states: String,
    /// The focus target of each node, by label.
    focus: Vec<(String, FocusId)>,
}

fn paint(theme: &Theme, focus: Option<FocusId>) -> Painted {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let mut cx =
        ElementContext::new(theme, 1.0, &mut text, &mut layouts, None, &store).with_focus(focus);
    cx.accessibility = AccessibilityFrame::new(SIZE.0, SIZE.1);
    let mut scene = Scene::default();
    render_element(&mut gallery(theme), &mut scene, &mut cx, SIZE.0, SIZE.1);
    let frame = &cx.semantic;
    let focus = (0..frame.nodes().len())
        .filter_map(|i| {
            Some((
                frame.nodes()[i].label.as_deref()?.to_owned(),
                frame.focus_id(i)?,
            ))
        })
        .collect();
    Painted {
        scene,
        states: dump_accessibility_states(&cx.accessibility.tree_update("Test", None)),
        focus,
    }
}

/// A primitive at the z and clip the renderer draws it with.
struct Placed<'a> {
    z: i32,
    clip: Rect,
    primitive: &'a Primitive,
}

/// The scene's primitives in the order the renderer draws them: by z, then
/// as pushed.
fn draw_order(scene: &Scene) -> Vec<Placed<'_>> {
    let everywhere = Rect {
        x: f32::MIN / 2.0,
        y: f32::MIN / 2.0,
        width: f32::MAX,
        height: f32::MAX,
    };
    let (mut z, mut clips, mut placed) = (vec![0], vec![everywhere], Vec::new());
    for primitive in &scene.primitives {
        match primitive {
            Primitive::ZIndexPush(level) => z.push(*level),
            Primitive::ZIndexPop => {
                z.pop();
            }
            Primitive::ClipStart(clip) => {
                let outer = *clips.last().unwrap_or(&everywhere);
                clips.push(intersect(outer, clip.rect));
            }
            Primitive::ClipEnd => {
                clips.pop();
            }
            _ => placed.push(Placed {
                z: *z.last().unwrap_or(&0),
                clip: *clips.last().unwrap_or(&everywhere),
                primitive,
            }),
        }
    }
    placed.sort_by_key(|p| p.z);
    placed
}

fn intersect(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.max(b.x), a.y.max(b.y));
    let right = (a.x + a.width).min(b.x + b.width);
    let bottom = (a.y + a.height).min(b.y + b.height);
    Rect {
        x,
        y,
        width: (right - x).max(0.0),
        height: (bottom - y).max(0.0),
    }
}

fn contains(rect: Rect, (x, y): (f32, f32)) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

fn center(rect: Rect) -> (f32, f32) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

fn fill(primitive: &Primitive) -> Option<(Rect, Color)> {
    match primitive {
        Primitive::Rect(r) => Some((r.rect, r.color)),
        Primitive::RoundedRect(r) => Some((r.rect, r.color)),
        _ => None,
    }
}

/// The opaque color at `point` once the fills drawn before `placed[upto]`
/// are composited over the window's clear color.
fn backdrop(placed: &[Placed], upto: usize, point: (f32, f32), clear: Color) -> Color {
    placed[..upto]
        .iter()
        .filter(|p| contains(p.clip, point))
        .filter_map(|p| fill(p.primitive))
        .filter(|(rect, _)| contains(*rect, point))
        .fold(clear, |under, (_, over)| composite(over, under))
}

fn composite(over: Color, under: Color) -> Color {
    let a = f32::from(over.a) / 255.0;
    let mix = |o: u8, u: u8| (f32::from(o) * a + f32::from(u) * (1.0 - a)).round() as u8;
    Color::rgba(
        mix(over.r, under.r),
        mix(over.g, under.g),
        mix(over.b, under.b),
        255,
    )
}

/// An icon's color: its most opaque pixel, unpremultiplied.
fn icon_color(rgba: &[u8]) -> Option<Color> {
    let px = rgba.as_chunks::<4>().0.iter().max_by_key(|px| px[3])?;
    let un = |c: u8| (u16::from(c) * 255 / u16::from(px[3].max(1))).min(255) as u8;
    Some(Color::rgba(un(px[0]), un(px[1]), un(px[2]), 255))
}

/// The point just outside a border's first drawn side, at its middle. A
/// border has to stand out from what holds it; on its inner side it often
/// frames a fill of its own color (a checked box) or a fill that carries
/// its own contrast (a slider's knob).
fn outside_border(rect: Rect, widths: [f32; 4]) -> Option<(f32, f32)> {
    let (cx, cy) = center(rect);
    let [top, right, bottom, left] = widths;
    if top > 0.0 {
        Some((cx, rect.y - 0.5))
    } else if right > 0.0 {
        Some((rect.x + rect.width + 0.5, cy))
    } else if bottom > 0.0 {
        Some((cx, rect.y + rect.height + 0.5))
    } else if left > 0.0 {
        Some((rect.x - 0.5, cy))
    } else {
        None
    }
}

/// Every painted mark below its target, as `what color on backdrop: ratio`.
fn shortfalls(theme: &Theme, scene: &Scene) -> Vec<String> {
    let placed = draw_order(scene);
    let clear = theme.colors.background;
    let mut failures = Vec::new();
    let mut check = |what: String, fg: Color, bg: Color, min: f32| {
        let ratio = contrast_ratio(fg, bg);
        if ratio < min {
            failures.push(format!("{what} {fg:?} on {bg:?}: {ratio:.2} < {min}"));
        }
    };
    for (i, p) in placed.iter().enumerate() {
        match p.primitive {
            Primitive::TextRun(run) => {
                let bg = backdrop(&placed, i, center(run.rect), clear);
                check(format!("text at {:?}", run.rect), run.color, bg, 7.0);
            }
            Primitive::Image(image) => {
                let Some(color) = icon_color(&image.rgba) else {
                    continue;
                };
                let bg = backdrop(&placed, i, center(image.rect), clear);
                check(format!("icon at {:?}", image.rect), color, bg, 3.0);
            }
            Primitive::Border(border) => {
                if let Some(point) = outside_border(border.rect, border.widths) {
                    let bg = backdrop(&placed, i, point, clear);
                    check(
                        format!("border at {:?}", border.rect),
                        border.color,
                        bg,
                        3.0,
                    );
                }
            }
            primitive => {
                let Some((rect, color)) = fill(primitive) else {
                    continue;
                };
                if color.a == 255 && rect.width <= MARK && rect.height <= MARK {
                    let bg = backdrop(&placed, i, center(rect), clear);
                    check(format!("mark at {rect:?}"), color, bg, 3.0);
                }
            }
        }
    }
    failures
}

fn high_contrast_themes() -> [Theme; 2] {
    [Theme::high_contrast_dark(), Theme::high_contrast_light()]
}

// Catches a control drawing a fixed color or an unplanned token pair over
// the accent or a selected row: a white check over a light accent, or a
// filled button's label in text_strong over it.
#[test]
fn high_contrast_controls_paint_readable_text_icons_borders_and_marks() {
    for theme in high_contrast_themes() {
        let failures = shortfalls(&theme, &paint(&theme, None).scene);
        assert!(failures.is_empty(), "{:?}: {failures:#?}", theme.mode);
    }
}

// Catches a focus ring that disappears against what holds its control.
// Rings sit outside a control: over the page for a checkbox, over the
// segmented control's track for a segment.
#[test]
fn high_contrast_focus_rings_stand_out_from_what_holds_them() {
    for theme in high_contrast_themes() {
        let focus = paint(&theme, None).focus;
        for label in ["Remember me", "Day", "Volume"] {
            let target = focus
                .iter()
                .find(|(l, _)| l == label)
                .map(|(_, f)| *f)
                .unwrap_or_else(|| panic!("no focus target for {label}"));
            let scene = paint(&theme, Some(target)).scene;
            let ring = scene
                .primitives
                .iter()
                .any(|p| matches!(p, Primitive::Border(b) if b.color == theme.colors.focus_border));
            assert!(ring, "{:?}: {label} painted no ring", theme.mode);
            let failures = shortfalls(&theme, &scene);
            assert!(
                failures.is_empty(),
                "{:?} {label}: {failures:#?}",
                theme.mode
            );
        }
    }
}

// Catches a theme changing what controls publish: contrast changes colors
// only, never roles, names, or states.
#[test]
fn high_contrast_keeps_roles_and_states() {
    let standard = paint(&Theme::default_dark(), None).states;
    for theme in high_contrast_themes() {
        assert_eq!(paint(&theme, None).states, standard, "{:?}", theme.mode);
    }
}
