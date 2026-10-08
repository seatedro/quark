//! Shared style recipes: the workbench's text roles and the few surfaces
//! more than one panel draws. Each scales the design's points by the
//! theme's zoom once, so callers pass points as the spec states them.
//!
//! Components (buttons, selects, menus, tooltips, toasts, the modal) need
//! no recipe here: the theme adapter sets their sizes on the theme.

use quark::Scene;
use quark_app::quark_ui::Action;
use quark_app::quark_ui::animation::{Curve, Motion, Prop};
use quark_app::quark_ui::element::{
    AnyElement, Bounds, Div, Element, ElementContext, IntoAnyElement, LayoutEngine, LayoutId,
    TextElement, div, text,
};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::{Color, Theme, ThemeMode};
use quark_components::{Button, ButtonStyle};

use super::tokens::*;

/// `points` at the theme's zoom, rounded to whole points.
pub fn pt(theme: &Theme, points: f32) -> f32 {
    (points * theme.metrics.ui_scale()).round()
}

/// The design's text roles (section 3, Type).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextRole {
    /// 11/16: timestamps, counts, badges.
    Meta,
    /// 13/18: navigation and controls.
    Control,
    /// 14/22: transcript prose and the composer.
    Prose,
    /// 13/20, monospace.
    Code,
    /// 16/22, semibold.
    Heading,
    /// 22/28, semibold.
    EmptyHeading,
}

impl TextRole {
    /// Size and line height in points at 100% zoom.
    pub fn metrics(self) -> (f32, f32) {
        match self {
            Self::Meta => TYPE_META,
            Self::Control => TYPE_CONTROL,
            Self::Prose => TYPE_PROSE,
            Self::Code => TYPE_CODE,
            Self::Heading => TYPE_HEADING,
            Self::EmptyHeading => TYPE_EMPTY_HEADING,
        }
    }

    /// Size and line height at the theme's zoom.
    pub fn scaled(self, theme: &Theme) -> (f32, f32) {
        let (size, line) = self.metrics();
        let zoom = theme.metrics.ui_scale();
        (size * zoom, line * zoom)
    }
}

/// `content` in `role`, in body color (strong for headings).
pub fn text_as(role: TextRole, content: impl Into<String>, theme: &Theme) -> TextElement {
    let (size, line) = role.metrics();
    let tc = &theme.colors;
    let t = text(content.into())
        .size(size * theme.metrics.ui_scale())
        .line_height(line / size);
    match role {
        TextRole::Code => t.mono().color(tc.text),
        TextRole::Heading | TextRole::EmptyHeading => t.semibold().color(tc.text_strong),
        TextRole::Meta => t.color(tc.text_muted),
        TextRole::Control | TextRole::Prose => t.color(tc.text),
    }
}

/// A hover or selection fill that eases over 100 ms. Reduced motion keeps
/// it: a color change is not movement.
pub fn hover_motion() -> Motion {
    Motion::tween(MOTION_HOVER_MS, Curve::EaseOutCubic)
}

/// A card: surface fill, 1-point border, 8-point radius. Tool cards, code,
/// and settings groups.
pub fn card(theme: &Theme) -> Div {
    let tc = &theme.colors;
    div()
        .flex_col()
        .bg(tc.surface)
        .border(tc.border_variant)
        .rounded(pt(theme, RADIUS_MENU))
}

/// A 32-point list row with 6-point corners: hover fill, and the selected
/// fill while `selected` (kept when hover leaves). Give it a key for the
/// hover transition.
pub fn row(theme: &Theme, selected: bool) -> Div {
    let tc = &theme.colors;
    let fill = if selected {
        tc.sidebar_row_selected
    } else {
        Color::TRANSPARENT
    };
    div()
        .flex_row()
        .items_center()
        .w_full()
        .h(pt(theme, SIDEBAR_ROW))
        .px(pt(theme, SPACE_8))
        .gap(pt(theme, SPACE_8))
        .rounded(pt(theme, RADIUS_ROW))
        .bg(fill)
        .hover_bg(if selected { fill } else { tc.sidebar_row_hover })
        .transition(Prop::Background, hover_motion())
}

/// A 1-point divider across its container.
pub fn divider(theme: &Theme) -> Div {
    div().w_full().h(DIVIDER).bg(theme.colors.border_variant)
}

/// A 28-point ghost icon button named `label`, which is also its tooltip.
pub fn icon_button(icon: &'static str, label: &str, action: impl Into<Action>) -> Button {
    Button::new(action)
        .icon(icon)
        .tooltip(label.to_owned())
        .fixed_size(ICON_BUTTON)
}

/// The 36-point filled button for a surface's main action.
pub fn primary_button(label: &str, action: impl Into<Action>) -> Button {
    Button::new(action)
        .label(label)
        .style(ButtonStyle::Filled)
        .metrics(quark_app::quark_ui::theme::ControlMetrics {
            height: Some(PRIMARY_CONTROL),
            padding_x: Some(SPACE_16),
            ..Default::default()
        })
}

/// `color` mixed into the opaque `base` by `amount` (0 to 1), in sRGB
/// like CSS `color-mix`. Use this, never a translucent color, for tinted
/// surfaces: the renderer blends in linear light, where a light color at a
/// low alpha over a dark surface comes out far stronger than its alpha.
pub fn tint(base: Color, color: Color, amount: f32) -> Color {
    base.lerp(color.with_alpha(255), amount).with_alpha(255)
}

/// How strongly a status color tints a surface: more in dark mode, where
/// the same mix reads weaker against the dark canvas.
pub fn status_tint(theme: &Theme) -> f32 {
    match theme.mode {
        ThemeMode::Light => 0.06,
        ThemeMode::Dark => 0.10,
    }
}

/// `child` with its text in `color` even where an ancestor sets the text
/// color for everything under it (the transcript paints its row headers
/// muted, and an inherited color wins over a text element's own).
pub fn text_color(color: Color, child: impl IntoAnyElement) -> AnyElement {
    AnyElement::new(TextColor {
        color,
        child: child.into_any(),
    })
}

struct TextColor {
    color: Color,
    child: AnyElement,
}

impl Element for TextColor {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        // The child's own node: this wrapper adds no box of its own.
        (self.child.request_layout(engine, cx), ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        self.child.prepaint(engine, cx);
    }

    fn paint(
        &mut self,
        _bounds: Bounds,
        _layout: &mut (),
        _prepaint: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        cx.push_text_color(self.color);
        self.child.paint(engine, scene, cx);
        cx.pop_text_color();
    }
}
