//! Draws the devtools overlay on top of the app's scene: layout outlines,
//! clipped hit regions, the inspector highlight, and the HUD and inspector
//! panels, which are ordinary elements with their own input router.

use quark::hit::{HitTable, UNCLIPPED};
use quark::reactive::SignalStore;
use quark::{Color, Rect, SemanticFrame};
use quark_render::scene::{BorderPrimitive, RoundedRectPrimitive, Scene};
use quark_text::{LayoutCache, TextSystem};

use super::{Devtools, DevtoolsMsg, ElementRecord};
use crate::FocusId;
use crate::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element, text};
use crate::hud::{BUDGET_60_US, BUDGET_120_US, HudState};
use crate::style::Styled;
use crate::theme::Theme;

/// Above any z-index an app uses.
const OVERLAY_Z: i32 = 1 << 30;
const PANEL_WIDTH: f32 = 280.0;
const ROW_HEIGHT: f32 = 16.0;

pub(super) const HIGHLIGHT: Color = Color::rgba(64, 156, 255, 255);
const HIGHLIGHT_FILL: Color = Color::rgba(64, 156, 255, 56);
const PINNED: Color = Color::rgba(255, 64, 160, 255);
const CLIP_OUTLINE: Color = Color::rgba(255, 170, 0, 220);
const HIT_VISIBLE: Color = Color::rgba(39, 174, 96, 60);
const HIT_CLIPPED: Color = Color::rgba(235, 87, 87, 200);
const PANEL_BG: Color = Color::rgba(18, 20, 24, 235);
const PANEL_BORDER: Color = Color::rgba(255, 255, 255, 40);
const BUTTON_BG: Color = Color::rgba(255, 255, 255, 24);
const BUTTON_HOVER: Color = Color::rgba(255, 255, 255, 56);
const ROW_SELECTED: Color = Color::rgba(255, 64, 160, 70);
const TEXT: Color = Color::rgba(230, 232, 236, 255);
const MUTED: Color = Color::rgba(150, 156, 166, 255);

/// What the overlay draws from and with. The text system, layout cache,
/// and theme must be the ones the app's frame used.
pub struct OverlayContext<'a> {
    pub theme: &'a Theme,
    pub scale_factor: f32,
    pub text: &'a mut TextSystem,
    pub layouts: &'a mut LayoutCache,
    pub signals: &'a SignalStore,
    pub width: f32,
    pub height: f32,
    /// The app frame's semantic tree and hit table.
    pub semantic: &'a SemanticFrame,
    pub hits: &'a HitTable,
    pub window: u64,
}

impl Devtools {
    /// Draw the overlay for the frame last passed to [`Self::end_frame`].
    pub fn paint_overlay(&mut self, scene: &mut Scene, ocx: OverlayContext) {
        if !self.is_active() {
            self.panel = Default::default();
            return;
        }
        scene.push_z_index(OVERLAY_Z);
        if self.layout {
            outline_records(scene, self.frame.records());
            clipped_hit_regions(scene, ocx.hits);
        }
        let pinned = self.inspector.then(|| self.pinned()).flatten();
        let hovered = self.inspector.then(|| self.hovered()).flatten();
        if let Some(index) = hovered.filter(|h| Some(*h) != pinned) {
            highlight(scene, &self.frame.records()[index], HIGHLIGHT);
        }
        if let Some(index) = pinned {
            highlight(scene, &self.frame.records()[index], PINNED);
        }

        let mut root = div().w(ocx.width).h(ocx.height);
        if let Some(index) = hovered.or(pinned) {
            root = root.child(tag(&self.frame.records()[index]));
        }
        if self.hud
            && let Some((_, hud)) = self.huds.iter().find(|(w, _)| *w == ocx.window)
        {
            root = root.child(hud_panel(hud));
        }
        if self.inspector {
            root = root.child(self.inspector_panel(pinned.or(hovered), pinned, &ocx));
        }
        let mut root = root.into_any();
        let mut ecx = ElementContext::new(
            ocx.theme,
            ocx.scale_factor,
            ocx.text,
            ocx.layouts,
            self.pointer,
            ocx.signals,
        );
        render_element(&mut root, scene, &mut ecx, ocx.width, ocx.height);
        self.panel.set_frame(ecx.take_input_frame());
        scene.pop_z_index();
    }

    fn inspector_panel(
        &self,
        target: Option<usize>,
        pinned: Option<usize>,
        ocx: &OverlayContext,
    ) -> AnyElement {
        let mut panel = div()
            .absolute()
            .right(0.0)
            .top(0.0)
            .w(PANEL_WIDTH)
            .h(ocx.height)
            .p(10.0)
            .gap(6.0)
            .flex_col()
            .overflow_hidden()
            .bg(PANEL_BG)
            .border_l(PANEL_BORDER)
            .block_mouse()
            .child(
                div()
                    .flex_row()
                    .justify_between()
                    .child(label("Inspector", TEXT))
                    .child(label("click pins, esc unpins", MUTED)),
            );
        match target {
            Some(index) => {
                let record = &self.frame.records()[index];
                panel = panel.child(details(record, ocx.semantic));
                if pinned == Some(index) {
                    panel = panel.child(self.editor(record));
                }
            }
            None => panel = panel.child(label("Hover an element.", MUTED)),
        }
        let layout_label = if self.layout {
            "layout debug: on"
        } else {
            "layout debug: off"
        };
        panel = panel.child(
            div()
                .flex_row()
                .gap(4.0)
                .child(button(layout_label, DevtoolsMsg::ToggleLayout))
                .child(button(
                    &format!("clear all overrides ({})", self.overrides.len()),
                    DevtoolsMsg::ClearAll,
                )),
        );
        let pinned_node = pinned.and_then(|i| self.frame.records()[i].semantic);
        panel
            .child(label("Semantic tree", TEXT))
            .child(semantic_tree(ocx.semantic, pinned_node))
            .into_any()
    }

    fn editor(&self, record: &ElementRecord) -> AnyElement {
        let (Some(key), Some(style)) = (&record.key, record.style) else {
            return label("Add .key() or .id() to edit this element.", MUTED);
        };
        let edited = self.overrides.get(key).is_some();
        div()
            .flex_col()
            .gap(3.0)
            .child(label(
                &format!("Edit {key}{}", if edited { " (overridden)" } else { "" }),
                TEXT,
            ))
            .child(stepper(
                "padding",
                style.padding[0],
                DevtoolsMsg::Padding(-2.0),
                DevtoolsMsg::Padding(2.0),
            ))
            .child(stepper(
                "gap",
                style.gap[0],
                DevtoolsMsg::Gap(-2.0),
                DevtoolsMsg::Gap(2.0),
            ))
            .child(stepper(
                "radius",
                style.corner_radii[0],
                DevtoolsMsg::Radius(-2.0),
                DevtoolsMsg::Radius(2.0),
            ))
            .child(
                div()
                    .flex_row()
                    .gap(4.0)
                    .child(button("bg", DevtoolsMsg::CycleBackground))
                    .child(button("border", DevtoolsMsg::CycleBorder))
                    .child(button("reset", DevtoolsMsg::ClearPinned))
                    .child(button("unpin", DevtoolsMsg::Unpin)),
            )
            .into_any()
    }
}

fn label(content: &str, color: Color) -> AnyElement {
    text(content.to_owned())
        .text_xs()
        .color(color)
        .truncate()
        .into_any()
}

fn mono(content: String) -> AnyElement {
    text(content)
        .text_xs()
        .mono()
        .color(TEXT)
        .truncate()
        .into_any()
}

fn button(content: &str, msg: DevtoolsMsg) -> AnyElement {
    div()
        .px(6.0)
        .h(18.0)
        .items_center()
        .rounded(4.0)
        .bg(BUTTON_BG)
        .hover_bg(BUTTON_HOVER)
        .on_click(msg)
        .child(label(content, TEXT))
        .into_any()
}

fn stepper(name: &str, value: f32, less: DevtoolsMsg, more: DevtoolsMsg) -> AnyElement {
    div()
        .flex_row()
        .gap(4.0)
        .items_center()
        .child(div().w(60.0).child(label(name, MUTED)))
        .child(button("-", less))
        .child(div().w(36.0).child(mono(format!("{value:.0}"))))
        .child(button("+", more))
        .into_any()
}

fn details(record: &ElementRecord, semantic: &SemanticFrame) -> AnyElement {
    let node = record.semantic.and_then(|i| semantic.nodes().get(i));
    let name = match (&record.key, node.and_then(|n| n.id.as_ref())) {
        (Some(key), _) => format!("{} #{key}", record.kind),
        (None, Some(id)) => format!("{} #{id}", record.kind),
        (None, None) => record.kind.to_owned(),
    };
    let b = record.bounds;
    let clip = if record.clip == UNCLIPPED {
        "none".to_owned()
    } else {
        rect_text(record.clip)
    };
    let role = node
        .and_then(|n| n.role)
        .map_or("-".to_owned(), |role| format!("{role:?}"));
    let focus = node
        .and_then(|n| n.focus)
        .map_or("-".to_owned(), |FocusId(id)| format!("{id:x}"));
    let mut lines = vec![
        name,
        format!("bounds {}", rect_text(b)),
        format!("clip   {clip}"),
        format!("z {}  role {role}  focus {focus}", record.z),
    ];
    if let Some(label) = node.and_then(|n| n.label.as_ref()) {
        lines.push(format!("label  {label}"));
    }
    if let Some(style) = record.style {
        let [l, r, t, bo] = style.padding;
        lines.push(format!(
            "padding {l} {r} {t} {bo}  gap {} {}",
            style.gap[0], style.gap[1]
        ));
        lines.push(format!(
            "bg {}  radius {}",
            color_text(style.background),
            style.corner_radii[0]
        ));
        lines.push(format!(
            "border {} {}  opacity {}",
            color_text(style.border_color),
            style.border_widths.map(|w| w.to_string()).join(" "),
            style.opacity
        ));
    }
    lines
        .into_iter()
        .fold(div().flex_col().gap(1.0), |column, line| {
            column.child(mono(line))
        })
        .into_any()
}

fn semantic_tree(frame: &SemanticFrame, pinned: Option<usize>) -> AnyElement {
    let rows = frame.nodes().iter().enumerate().map(|(index, node)| {
        let depth = frame.ancestors_inclusive(index).count().saturating_sub(1);
        let role = node
            .role
            .map_or("node".to_owned(), |role| format!("{role:?}"));
        let name = node
            .label
            .as_deref()
            .or(node.id.as_ref().map(|id| id.as_str()))
            .unwrap_or("");
        div()
            .h(ROW_HEIGHT)
            .flex_shrink_0()
            .pl(4.0 + depth as f32 * 10.0)
            .items_center()
            .rounded(3.0)
            .when(pinned == Some(index), |row| row.bg(ROW_SELECTED))
            .hover_bg(BUTTON_HOVER)
            .on_click(DevtoolsMsg::PinNode(index))
            .child(label(&format!("{role} {name}"), TEXT))
            .into_any()
    });
    div()
        .flex_col()
        .flex_1()
        .overflow_hidden()
        .children_from(rows)
        .into_any()
}

fn hud_panel(hud: &HudState) -> AnyElement {
    let s = hud.last;
    let ms = |us: u64| us as f32 / 1000.0;
    let lines = [
        format!(
            "{:5.1} fps  cpu {:.2} ms  avg {:.2}",
            hud.fps(),
            ms(s.cpu_us()),
            ms(hud.cpu_ema_us())
        ),
        format!(
            "build {}  layout {}  paint {} us",
            s.build_us, s.layout_us, s.paint_us
        ),
        format!(
            "render {}  acq {}  present {} us",
            s.render_cpu_us, s.acquire_us, s.present_us
        ),
        format!("primitives {}", s.primitive_count),
        format!(
            "text cache {} hit  {} miss  {} held",
            s.text_hits, s.text_misses, s.text_entries
        ),
    ];
    let peak = hud.history_peak_us().max(BUDGET_60_US) as f32;
    let bars = hud.samples().map(|us| {
        let us = u64::from(us);
        let color = if us <= BUDGET_120_US {
            Color::rgba(39, 174, 96, 255)
        } else if us <= BUDGET_60_US {
            Color::rgba(242, 201, 76, 255)
        } else {
            Color::rgba(235, 87, 87, 255)
        };
        div()
            .w(2.0)
            .h((us as f32 / peak * 28.0).max(1.0))
            .bg(color)
            .into_any()
    });
    let column = lines
        .into_iter()
        .fold(div().flex_col().gap(1.0), |column, line| {
            column.child(mono(line))
        });
    div()
        .absolute()
        .left(8.0)
        .top(8.0)
        .w(280.0)
        .p(8.0)
        .gap(4.0)
        .flex_col()
        .rounded(6.0)
        .bg(PANEL_BG)
        .child(column)
        .child(div().flex_row().items_end().h(28.0).children_from(bars))
        .into_any()
}

/// A label above the highlighted element with its kind and size.
fn tag(record: &ElementRecord) -> AnyElement {
    let b = record.bounds;
    div()
        .absolute()
        .left(b.x.max(0.0))
        .top((b.y - ROW_HEIGHT - 2.0).max(0.0))
        .h(ROW_HEIGHT)
        .px(4.0)
        .items_center()
        .rounded(3.0)
        .bg(HIGHLIGHT)
        .child(label(
            &format!("{} {:.0}x{:.0}", record.kind, b.width, b.height),
            Color::rgba(255, 255, 255, 255),
        ))
        .into_any()
}

fn highlight(scene: &mut Scene, record: &ElementRecord, color: Color) {
    let b = record.bounds;
    if color == HIGHLIGHT {
        scene.rounded_rect(RoundedRectPrimitive::uniform(b, 0.0, HIGHLIGHT_FILL));
    }
    scene.border(BorderPrimitive {
        rect: b,
        widths: [2.0; 4],
        corner_radii: [0.0; 4],
        color,
    });
    if record.clip != UNCLIPPED && !contains_rect(record.clip, b) {
        scene.border(BorderPrimitive {
            rect: record.clip,
            widths: [1.0; 4],
            corner_radii: [0.0; 4],
            color: CLIP_OUTLINE,
        });
    }
}

fn outline_records(scene: &mut Scene, records: &[ElementRecord]) {
    for record in records {
        let color = match record.kind {
            "Div" => Color::rgba(80, 200, 255, 140),
            "TextElement" => Color::rgba(255, 220, 80, 140),
            _ => Color::rgba(255, 80, 255, 140),
        };
        scene.border(BorderPrimitive {
            rect: record.bounds,
            widths: [1.0; 4],
            corner_radii: [0.0; 4],
            color,
        });
    }
}

/// Hit regions a clip cuts: the part that still takes input filled, the
/// whole declared region outlined.
fn clipped_hit_regions(scene: &mut Scene, hits: &HitTable) {
    for id in hits.ids() {
        let (Some(bounds), Some(clip)) = (hits.bounds(id), hits.clip(id)) else {
            continue;
        };
        if contains_rect(clip, bounds) {
            continue;
        }
        if let Some(visible) = clip.intersection(bounds) {
            scene.rounded_rect(RoundedRectPrimitive::uniform(visible, 0.0, HIT_VISIBLE));
        }
        scene.border(BorderPrimitive {
            rect: bounds,
            widths: [1.0; 4],
            corner_radii: [0.0; 4],
            color: HIT_CLIPPED,
        });
    }
}

fn contains_rect(outer: Rect, inner: Rect) -> bool {
    outer.x <= inner.x
        && outer.y <= inner.y
        && outer.right() >= inner.right()
        && outer.bottom() >= inner.bottom()
}

fn rect_text(r: Rect) -> String {
    format!("{:.0},{:.0} {:.0}x{:.0}", r.x, r.y, r.width, r.height)
}

fn color_text(color: Option<Color>) -> String {
    color.map_or("-".to_owned(), |c| {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    })
}
