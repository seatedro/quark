//! Style transitions for [`Div`]: when a transitioned value of the resolved
//! style changes between frames, paint animates toward it from the value on
//! screen instead of jumping. Rows live in the window's animation table,
//! keyed by the div's stable [`UiKey`].

use super::*;
use crate::animation::{self, AnimKey, Motion, Prop, PropId, PropSet};

/// Per-div transition settings; later entries win for a prop.
#[derive(Clone, Default)]
pub(super) struct Transitions(Vec<(PropSet, Motion)>);

impl Transitions {
    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn push(&mut self, props: PropSet, motion: Motion) {
        self.0.push((props, motion));
    }

    fn motion(&self, prop: Prop) -> Option<Motion> {
        self.0
            .iter()
            .rev()
            .find(|(props, _)| props.contains(prop))
            .map(|(_, motion)| *motion)
    }

    fn key(&self, key: Option<&UiKey>) -> Option<AnimKey> {
        if self.0.is_empty() {
            return None;
        }
        key.map(AnimKey::from)
    }

    /// The transform to paint with this frame.
    pub(super) fn transform(
        &self,
        key: Option<&UiKey>,
        target: PaintTransform,
        cx: &mut ElementContext,
    ) -> PaintTransform {
        let (Some(key), Some(motion)) = (self.key(key), self.motion(Prop::Transform)) else {
            return target;
        };
        let mut value = |prop, target| cx.transition(key, prop, target, motion).0;
        PaintTransform {
            translate: (
                value(animation::TRANSLATE_X, target.translate.0),
                value(animation::TRANSLATE_Y, target.translate.1),
            ),
            rotate: value(animation::ROTATE, target.rotate),
            scale: (
                value(animation::SCALE_X, target.scale.0),
                value(animation::SCALE_Y, target.scale.1),
            ),
        }
    }

    /// Replace transitioned values of `style` (the resolved target) with the
    /// values on screen this frame.
    pub(super) fn apply(
        &self,
        key: Option<&UiKey>,
        style: &mut ElementStyle,
        cx: &mut ElementContext,
    ) {
        let Some(key) = self.key(key) else {
            return;
        };
        if let Some(motion) = self.motion(Prop::Background) {
            style.background = color(key, animation::BACKGROUND, style.background, motion, cx);
        }
        if let Some(motion) = self.motion(Prop::BorderColor) {
            style.border_color =
                color(key, animation::BORDER_COLOR, style.border_color, motion, cx);
        }
        if let Some(motion) = self.motion(Prop::Opacity) {
            let (value, _) = cx.transition(key, animation::OPACITY, style.opacity, motion);
            style.opacity = value.clamp(0.0, 1.0);
        }
    }
}

/// A div's paint-time transform: an offset that layout ignores, then a
/// rotation and scale about the center of the offset bounds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct PaintTransform {
    pub translate: (f32, f32),
    /// Radians, clockwise on screen.
    pub rotate: f32,
    pub scale: (f32, f32),
}

impl PaintTransform {
    pub const IDENTITY: Self = Self {
        translate: (0.0, 0.0),
        rotate: 0.0,
        scale: (1.0, 1.0),
    };

    /// The rotation and scale about the center of `bounds`, or `None` when
    /// the transform only translates.
    pub fn matrix(&self, bounds: Bounds) -> Option<Transform2D> {
        if self.rotate == 0.0 && self.scale == (1.0, 1.0) {
            return None;
        }
        let center = (
            bounds.x + bounds.width * 0.5,
            bounds.y + bounds.height * 0.5,
        );
        Some(
            Transform2D::scale(self.scale.0, self.scale.1)
                .then(Transform2D::rotate(self.rotate))
                .around(center.0, center.1),
        )
    }
}

/// An absent color animates as transparent, so a background can fade in.
fn color(
    key: AnimKey,
    props: [PropId; 4],
    target: Option<Color>,
    motion: Motion,
    cx: &mut ElementContext,
) -> Option<Color> {
    let channels = animation::to_premul_oklab(target.unwrap_or(Color::TRANSPARENT));
    let mut current = [0.0; 4];
    let mut moving = false;
    for i in 0..4 {
        let (value, active) = cx.transition(key, props[i], channels[i], motion);
        current[i] = value;
        moving |= active;
    }
    if !moving {
        // At rest the rows hold the target exactly; paint the style's own
        // color rather than a value that went through f32 conversions.
        return target;
    }
    Some(animation::from_premul_oklab(current)).filter(|c| c.a > 0)
}

#[cfg(test)]
mod tests {
    use quark_render::Primitive;

    use super::*;
    use crate::animation::{AnimationTable, Curve};
    use crate::theme::Theme;

    const IDLE: Color = Color::rgba(20, 40, 200, 255);
    const HOVER: Color = Color::rgba(240, 180, 20, 255);
    const POINTER_IN: Option<(f32, f32)> = Some((50.0, 20.0));

    /// One painted frame of a test tree.
    struct Frame {
        /// Background colors in paint order.
        backgrounds: Vec<Color>,
        next_frame_ms: Option<u64>,
    }

    /// Paints `root` at 200x100 at `clock_ms` against the persistent `table`.
    fn frame(
        root: impl IntoAnyElement,
        table: &mut AnimationTable,
        pointer: Option<(f32, f32)>,
        clock_ms: u64,
    ) -> Frame {
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let store = SignalStore::new();
        let theme = Theme::default_dark();
        table.tick(clock_ms);
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, pointer, &store)
            .with_clock(clock_ms)
            .with_animations(table);
        let mut scene = Scene::default();
        render_element(&mut root.into_any(), &mut scene, &mut cx, 200.0, 100.0);
        cx.finish_frame();
        let backgrounds = scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::RoundedRect(rr) => Some(rr.color),
                _ => None,
            })
            .collect();
        Frame {
            backgrounds,
            next_frame_ms: cx.next_frame_ms(),
        }
    }

    fn distance(a: Color, b: Color) -> f32 {
        let d = |x: u8, y: u8| (f32::from(x) - f32::from(y)).powi(2);
        (d(a.r, b.r) + d(a.g, b.g) + d(a.b, b.b)).sqrt()
    }

    fn button() -> Div {
        div()
            .key("button")
            .w(100.0)
            .h(40.0)
            .bg(IDLE)
            .hover_bg(HOVER)
            .transition(Prop::Background, Motion::tween(100, Curve::Linear))
    }

    // Catches a transition that jumps straight to the hover color, never
    // finishes, or keeps the window repainting once it has settled.
    #[test]
    fn hover_bg_passes_through_intermediate_colors_then_settles_and_idles() {
        let mut table = AnimationTable::new();
        let idle = frame(button(), &mut table, None, 0);
        assert_eq!((idle.backgrounds, idle.next_frame_ms), (vec![IDLE], None));

        let entered = frame(button(), &mut table, POINTER_IN, 10);
        assert_eq!(entered.backgrounds, vec![IDLE]);
        assert_eq!(entered.next_frame_ms, Some(10), "a frame is due right away");

        let mid = frame(button(), &mut table, POINTER_IN, 60);
        let color = mid.backgrounds[0];
        assert!(color != IDLE && color != HOVER, "{color:?}");
        assert!(color.r > IDLE.r && color.r < HOVER.r, "{color:?}");
        assert!(color.b < IDLE.b && color.b > HOVER.b, "{color:?}");
        assert_eq!(mid.next_frame_ms, Some(60));

        let settled = frame(button(), &mut table, POINTER_IN, 110);
        assert_eq!(
            (settled.backgrounds, settled.next_frame_ms),
            (vec![HOVER], None)
        );
        let later = frame(button(), &mut table, POINTER_IN, 500);
        assert_eq!(
            (later.backgrounds, later.next_frame_ms),
            (vec![HOVER], None)
        );
    }

    // Catches a retarget that restarts from the old resting color, which
    // flashes when the pointer leaves mid-transition.
    #[test]
    fn hover_bg_leaving_midway_reverses_from_the_color_on_screen() {
        let mut table = AnimationTable::new();
        frame(button(), &mut table, None, 0);
        frame(button(), &mut table, POINTER_IN, 0);
        let on_screen = frame(button(), &mut table, POINTER_IN, 50).backgrounds[0];

        let left = frame(button(), &mut table, None, 50).backgrounds[0];
        assert_eq!(left, on_screen);
        let reversing = frame(button(), &mut table, None, 60).backgrounds[0];
        // A tenth of the way back: closer to IDLE than before, still far
        // nearer the color on screen than to either endpoint.
        let gap = distance(on_screen, IDLE);
        assert!(
            distance(reversing, IDLE) < gap,
            "{on_screen:?} -> {reversing:?}"
        );
        assert!(
            distance(reversing, on_screen) < gap * 0.3,
            "{on_screen:?} -> {reversing:?}"
        );
        assert_eq!(
            frame(button(), &mut table, None, 150).backgrounds,
            vec![IDLE]
        );
    }

    // Catches transitions leaking rows for elements no longer in the tree.
    #[test]
    fn transition_rows_of_unpainted_elements_are_dropped() {
        let mut table = AnimationTable::new();
        frame(button(), &mut table, POINTER_IN, 0);
        assert!(!table.is_empty());
        frame(div().w(10.0).h(10.0), &mut table, POINTER_IN, 10);
        assert!(table.is_empty());
    }

    // Catches a translate transition applied to paint but not to children
    // or hit testing: the child's background must move with the parent.
    #[test]
    fn translate_spring_moves_children_and_settles_on_target() {
        let panel = |x: f32| {
            div()
                .key("panel")
                .w(50.0)
                .h(50.0)
                .translate(x, 0.0)
                .transition(Prop::Transform, Motion::spring(300.0, 30.0, 1.0))
                .child(div().w(10.0).h(10.0).bg(IDLE).hover_bg(HOVER))
        };
        let mut table = AnimationTable::new();
        // The child sits at x 0..10; at translate 100 it is at 100..110.
        let at = |x| Some((x, 5.0));
        frame(panel(0.0), &mut table, None, 0);
        let start = frame(panel(100.0), &mut table, at(5.0), 0);
        assert_eq!(start.backgrounds, vec![HOVER], "not moved yet");
        let mid = frame(panel(100.0), &mut table, at(5.0), 50);
        assert_eq!(mid.backgrounds, vec![IDLE], "moved away from the pointer");
        let mut now = 50;
        while frame(panel(100.0), &mut table, None, now)
            .next_frame_ms
            .is_some()
        {
            now += 16;
            assert!(now < 5_000, "spring never settled");
        }
        assert_eq!(
            frame(panel(100.0), &mut table, at(105.0), now).backgrounds,
            vec![HOVER]
        );
    }

    // Catches hit testing that ignores rotation: a square turned 45
    // degrees is hovered at its new tip and not at its old corner.
    #[test]
    fn rotated_div_is_hovered_where_it_is_drawn() {
        let tile = || {
            div().p(30.0).child(
                div()
                    .w(40.0)
                    .h(40.0)
                    .bg(IDLE)
                    .hover_bg(HOVER)
                    .rotate(std::f32::consts::FRAC_PI_4),
            )
        };
        let mut table = AnimationTable::new();
        // The square spans 30..70; its center is (50, 50).
        let tip = frame(tile(), &mut table, Some((76.0, 50.0)), 0);
        assert_eq!(tip.backgrounds, vec![HOVER], "tip not hovered");
        let corner = frame(tile(), &mut table, Some((31.0, 31.0)), 0);
        assert_eq!(corner.backgrounds, vec![IDLE], "old corner hovered");
    }

    /// Opacity of each layer the frame opens, in paint order.
    fn layer_opacities(
        root: impl IntoAnyElement,
        table: &mut AnimationTable,
        now: u64,
    ) -> Vec<f32> {
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let store = SignalStore::new();
        let theme = Theme::default_dark();
        table.tick(now);
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store)
            .with_clock(now)
            .with_animations(table);
        let mut scene = Scene::default();
        render_element(&mut root.into_any(), &mut scene, &mut cx, 200.0, 100.0);
        cx.finish_frame();
        scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::LayerStart(layer) => Some(layer.opacity),
                _ => None,
            })
            .collect()
    }

    // Catches a fade that jumps, or a settled div that keeps paying for an
    // offscreen layer: opacity passes through the middle as one group, and
    // at rest a fully opaque div opens no layer at all.
    #[test]
    fn opacity_transition_fades_as_a_layer_and_drops_it_at_rest() {
        let card = |opacity: f32| {
            div()
                .key("card")
                .w(50.0)
                .h(50.0)
                .bg(IDLE)
                .opacity(opacity)
                .transition(Prop::Opacity, Motion::tween(100, Curve::Linear))
                .child(div().w(10.0).h(10.0).bg(HOVER))
        };
        let mut table = AnimationTable::new();
        assert_eq!(layer_opacities(card(1.0), &mut table, 0), Vec::<f32>::new());
        layer_opacities(card(0.0), &mut table, 0);
        let mid = layer_opacities(card(0.0), &mut table, 50);
        assert_eq!(mid.len(), 1);
        assert!((mid[0] - 0.5).abs() < 0.05, "{mid:?}");
        layer_opacities(card(1.0), &mut table, 150);
        assert_eq!(
            layer_opacities(card(1.0), &mut table, 300),
            Vec::<f32>::new()
        );
    }
}
