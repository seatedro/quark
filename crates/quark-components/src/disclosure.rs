//! A disclosure: content that expands under a trigger and collapses away.
//!
//! Layout has no height transition, so the expansion is measured: the app
//! keeps a [`DisclosureState`], ticks it in the view with the window's
//! animation table, and renders [`DisclosureState::region`] around the
//! content. The region records the content's height every frame it is
//! painted; while the disclosure moves, it clips the content to that height
//! times the animated progress. At rest it adds nothing to the layout.
//!
//! The content stays mounted while it collapses ([`DisclosurePhase::Closing`])
//! and unmounts once the motion ends. Under reduced motion the disclosure
//! opens and closes at once and schedules no frames.
//!
//! [`DisclosureState::height`] is the height on screen this frame, for a
//! container that anchors scrolling to content around the disclosure.

use std::cell::Cell;
use std::rc::Rc;

use quark::view;
use quark_render::Scene;
use quark_ui::Action;
use quark_ui::FocusId;
use quark_ui::animation::{AnimKey, AnimationTable, Curve, Motion, Prop, PropId};
use quark_ui::element::{
    AnyElement, Bounds, Element, ElementContext, IntoAnyElement, LayoutEngine, LayoutId, div,
    svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::{ElementStyle, Styled};
use quark_ui::theme::Theme;

/// Duration of an expansion or collapse.
pub const DISCLOSURE_MS: u32 = 160;

/// Expansion, 0 (collapsed) to 1 (expanded); present only while moving.
const PROGRESS: PropId = PropId(0);

fn motion() -> Motion {
    Motion::tween(DISCLOSURE_MS, Curve::EaseOutCubic)
}

/// Where a disclosure is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DisclosurePhase {
    Closed,
    Opening,
    Open,
    /// Collapsing: the content is still mounted, clipped shut.
    Closing,
}

/// One disclosure's open state, its collapse in progress, and its content's
/// last measured height.
#[derive(Debug, Clone)]
pub struct DisclosureState {
    id: Rc<str>,
    key: AnimKey,
    focus: FocusId,
    open: bool,
    /// Closed, but the content stays mounted until the collapse ends.
    exiting: bool,
    /// A change made in `update` (which has no animation table), started
    /// at the next tick.
    pending: bool,
    /// The content's natural height, written when the region paints.
    measured: Rc<Cell<f32>>,
}

impl DisclosureState {
    /// `id` must be unique in the window; it keys the animation, the
    /// trigger's focus, and accessibility ids.
    pub fn new(id: &str, open: bool) -> Self {
        Self {
            id: Rc::from(id),
            key: AnimKey::from_str_key(&format!("quark.disclosure.{id}")),
            focus: FocusId::from_key(&format!("disclosure:{id}")),
            open,
            exiting: false,
            pending: false,
            measured: Rc::new(Cell::new(0.0)),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// The trigger's focus target, to restore focus after a collapse.
    pub fn focus_id(&self) -> FocusId {
        self.focus
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Whether to build the content: open, or still collapsing.
    pub fn is_mounted(&self) -> bool {
        self.open || self.exiting
    }

    /// Open or close; the motion starts at the next [`Self::tick`].
    pub fn set_open(&mut self, open: bool) {
        if self.open != open {
            self.open = open;
            self.exiting = !open;
            self.pending = true;
        }
    }

    pub fn toggle(&mut self) {
        self.set_open(!self.open);
    }

    /// Start a pending motion and finish a settled one. Call in the view,
    /// before rendering, with the window's animation table (already ticked
    /// to `now_ms`) and the theme's `reduced_motion`. A moving row keeps
    /// the window drawing frames until it settles.
    pub fn tick(&mut self, table: &mut AnimationTable, now_ms: u64, reduced_motion: bool) {
        if reduced_motion {
            // Jump to the end, including out of a motion already running.
            self.pending = false;
            table.remove(self.key, PROGRESS);
            self.exiting = false;
            return;
        }
        if std::mem::take(&mut self.pending) {
            let target = if self.open { 1.0 } else { 0.0 };
            if table.get(self.key, PROGRESS).is_none() {
                table.set(self.key, PROGRESS, 1.0 - target, now_ms);
            }
            table.animate_to(self.key, PROGRESS, target, motion(), now_ms);
        }
        if table.get(self.key, PROGRESS).is_some() && !table.is_animating(self.key, PROGRESS) {
            table.remove(self.key, PROGRESS);
            self.exiting = false;
        }
    }

    pub fn phase(&self, table: &AnimationTable) -> DisclosurePhase {
        let moving = table.is_animating(self.key, PROGRESS);
        match (self.open, self.exiting) {
            (true, _) if moving => DisclosurePhase::Opening,
            (true, _) => DisclosurePhase::Open,
            (false, true) => DisclosurePhase::Closing,
            (false, false) => DisclosurePhase::Closed,
        }
    }

    /// The content's natural height when it last painted, in points.
    pub fn measured_height(&self) -> f32 {
        self.measured.get()
    }

    /// The region's height on screen this frame, in points.
    pub fn height(&self, table: &AnimationTable) -> f32 {
        match table.get(self.key, PROGRESS) {
            Some(progress) => progress.clamp(0.0, 1.0) * self.measured.get(),
            None if self.open => self.measured.get(),
            None => 0.0,
        }
    }

    /// The region holding `content`, or `None` when it is unmounted (call
    /// this only then to skip building the content).
    pub fn region(
        &self,
        table: &AnimationTable,
        content: impl IntoAnyElement,
    ) -> Option<AnyElement> {
        if !self.is_mounted() {
            return None;
        }
        let measured = Measured {
            child: content.into_any(),
            height: self.measured.clone(),
            out_of_flow: false,
        };
        let region = div()
            .w_full()
            .flex_col()
            .test_id("disclosure-region")
            .accessibility_id(format!("{}-region", self.id));
        Some(match table.get(self.key, PROGRESS) {
            None => region.child(measured).into_any(),
            // Out of flow, the content keeps its natural height for the
            // next frame while the region clips it to the animated one.
            Some(_) => region
                .h(self.height(table))
                .overflow_hidden()
                .child(Measured {
                    out_of_flow: true,
                    ..measured
                })
                .into_any(),
        })
    }

    /// A full-width trigger row: a chevron that turns as the disclosure
    /// opens, then `label`. Click, Enter, or Space emits `on_toggle`.
    pub fn trigger(&self, label: &str, on_toggle: impl Into<Action>, theme: &Theme) -> AnyElement {
        let tc = &theme.colors;
        let m = &theme.metrics;
        let scale = m.ui_scale();
        let on_toggle = on_toggle.into();
        let angle = if self.open {
            std::f32::consts::FRAC_PI_2
        } else {
            0.0
        };
        let chevron = div()
            .key(format!("quark.disclosure.{}.chevron", self.id))
            .rotate(angle)
            .transition(Prop::Transform, motion())
            .child(svg_icon(lucide::CHEVRON_RIGHT, (16.0 * scale).round()).color(tc.text_muted));
        view! {
            <div class="flex-row items-center w-full cursor-pointer"
                 gap={m.spacing_sm} px={m.spacing_sm} py={m.spacing_xs}
                 rounded={m.control_radius}
                 hover_bg={tc.ghost_element_hover}
                 role="button" aria-label={label.to_owned()} aria-expanded={self.open}
                 accessibility_id={format!("{}-trigger", self.id)} test_id="disclosure-trigger"
                 focus_ring={self.focus}
                 on:click={on_toggle.clone()}
                 on_key={("enter", on_toggle.clone())}
                 on_key={("space", on_toggle)}>
                {chevron}
                <text class="text-sm font-medium" color={tc.text}>{label.to_owned()}</text>
            </div>
        }
    }
}

/// [`Styled`] over a bare style, to build a layout node's taffy style.
struct LayoutStyle(ElementStyle);

impl Styled for LayoutStyle {
    fn element_style_mut(&mut self) -> &mut ElementStyle {
        &mut self.0
    }
}

/// Lays out its child at full width and records the height it got.
struct Measured {
    child: AnyElement,
    height: Rc<Cell<f32>>,
    out_of_flow: bool,
}

impl Element for Measured {
    type LayoutState = LayoutId;
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, LayoutId) {
        let child = self.child.request_layout(engine, cx);
        let style = LayoutStyle(ElementStyle::default()).flex_col().w_full();
        let style = match self.out_of_flow {
            true => style.absolute().left(0.0).top(0.0),
            false => style,
        };
        (engine.request_layout(style.0.layout, &[child]), child)
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _child: &mut LayoutId,
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        self.height.set(bounds.height);
        self.child.prepaint(engine, cx);
    }

    fn paint(
        &mut self,
        _bounds: Bounds,
        _child: &mut LayoutId,
        _prepaint: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        self.child.paint(engine, scene, cx);
    }
}

impl IntoAnyElement for Measured {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
