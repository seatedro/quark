//! IME targets: what the host needs to turn IME on for the focused element
//! and place the candidate window at its caret.

use crate::action::FocusId;
use quark_render::scene::Rect;

/// An element that takes IME composition while it has keyboard focus,
/// recorded each frame by [`super::ElementContext::register_ime_target`].
/// Text fields register one; so do other editable elements, such as a
/// terminal, that handle composition themselves.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImeTarget {
    pub focus_target: FocusId,
    /// The caret in window coordinates (every ancestor transform applied),
    /// for the candidate window. `None` when there is no usable caret: a
    /// transform flattens it or a clip hides it.
    pub caret: Option<Rect>,
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use quark::reactive::SignalStore;
    use quark_render::Scene;

    use super::*;
    use crate::element::{IntoAnyElement, canvas, div, render_element};
    use crate::style::Styled;
    use crate::theme::Theme;

    const TARGET: FocusId = FocusId::from_key("test.ime");

    /// The IME targets a 200x100 window registers when `wrap` places a
    /// 100x20 canvas whose caret is the 2x10 rect at (10, 5) in it.
    fn targets(
        wrap: impl FnOnce(crate::element::AnyElement) -> crate::element::AnyElement,
    ) -> Vec<ImeTarget> {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut cx = crate::element::ElementContext::new(
            &theme,
            1.0,
            &mut text,
            &mut layouts,
            None,
            &signals,
        );
        let editor = canvas(|bounds, _scene, cx| {
            let caret = Rect {
                x: bounds.x + 10.0,
                y: bounds.y + 5.0,
                width: 2.0,
                height: 10.0,
            };
            cx.register_ime_target(TARGET, Some(caret));
        })
        .w(100.0)
        .h(20.0)
        .into_any();
        let mut root = wrap(editor);
        render_element(&mut root, &mut Scene::default(), &mut cx, 200.0, 100.0);
        std::mem::take(&mut cx.ime_targets)
    }

    fn caret(targets: &[ImeTarget]) -> Option<Rect> {
        assert_eq!(targets.len(), 1, "one target");
        assert_eq!(targets[0].focus_target, TARGET);
        targets[0].caret
    }

    /// A rect rounded to hundredths, so rotated corners compare exactly.
    fn rounded(r: Rect) -> [f32; 4] {
        [r.x, r.y, r.width, r.height].map(|v| (v * 100.0).round() / 100.0)
    }

    #[test]
    fn caret_is_mapped_through_ancestor_transforms_into_the_window() {
        // Turned a quarter clockwise about the canvas center (50, 10), the
        // caret's 2x10 box at (10..12, 5..15) lands at (45..55, -30..-28).
        let rotated = targets(|editor| {
            div()
                .w(100.0)
                .h(20.0)
                .rotate(FRAC_PI_2)
                .child(editor)
                .into_any()
        });
        assert_eq!(caret(&rotated).map(rounded), Some([45.0, -30.0, 10.0, 2.0]));
    }

    #[test]
    fn caret_is_dropped_where_a_transform_flattens_or_a_clip_hides_it() {
        let flat = targets(|editor| {
            div()
                .w(100.0)
                .h(20.0)
                .scale_xy(0.0, 1.0)
                .child(editor)
                .into_any()
        });
        assert_eq!(caret(&flat), None, "flattened");
        // The canvas is scrolled 40pt up inside a 20pt-tall clip, taking
        // the caret (5..15 in the canvas) out of view.
        let hidden = targets(|editor| {
            div()
                .w(100.0)
                .h(20.0)
                .overflow_hidden()
                .child(div().absolute().top(-40.0).child(editor))
                .into_any()
        });
        assert_eq!(caret(&hidden), None, "clipped out");
    }
}
