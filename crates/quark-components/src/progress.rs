use quark::{Props, view};

use quark_ui::accessibility::NumericValue;
use quark_ui::design::{Sp, Sz};
use quark_ui::element::{AnyElement, ElementContext, IntoAnyElement, RenderOnce, div, text};
use quark_ui::style::Styled;
use quark_ui::theme::Color;

/// A horizontal bar filled to `value` (0 to 1): `progress_bar(0.4)`, or in
/// `view!` `<ProgressBar value={0.4} show_label />`.
#[derive(Props)]
pub struct ProgressBar {
    /// Clamped to 0..=1 when drawn.
    value: f32,
    #[prop(optional)]
    color: Option<Color>,
    #[prop(optional)]
    track_color: Option<Color>,
    #[prop(default = Sz::PROGRESS_H)]
    height: f32,
    #[prop(default)]
    show_label: bool,
}

pub fn progress_bar(value: f32) -> ProgressBar {
    ProgressBar::builder().value(value).build()
}

impl ProgressBar {
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }

    pub fn track_color(mut self, c: Color) -> Self {
        self.track_color = Some(c);
        self
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn show_label(mut self) -> Self {
        self.show_label = true;
        self
    }
}

impl RenderOnce for ProgressBar {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let fill_color = self.color.unwrap_or(tc.accent);
        let bg_color = self.track_color.unwrap_or(tc.element_background);
        let h = self.height;
        let v = self.value.clamp(0.0, 1.0);

        let track = view! {
            <div
                class="w-full"
                h={h}
                class="flex-row"
                bg={bg_color}
                rounded={h / 2.0}
                class="overflow-hidden"
                accessibility_role={accesskit::Role::ProgressIndicator}
                accessibility_numeric={NumericValue {
                    value: f64::from((v * 100.0).round()),
                    min: 0.0,
                    max: 100.0,
                    step: None,
                }}
            >
                <div
                    class="h-full"
                    bg={fill_color}
                    rounded={h / 2.0}
                    flex_grow_val={if v > 0.001 { v } else { 0.001 }}
                />
                <div class="h-full" flex_grow_val={(1.0 - v).max(0.001)} />
            </div>
        };

        if self.show_label {
            let pct = (v * 100.0).round() as u32;
            view! {
                <div class="flex-row items-center w-full" gap={Sp::SM}>
                    <div class="flex-1">{track}</div>
                    <text class="text-xs" color={tc.text_muted}>{format!("{pct}%")}</text>
                </div>
            }
        } else {
            track
        }
    }
}

/// A bar split into colored segments in proportion to their weights, as
/// a diff's added and deleted lines or used space by kind. Segments of
/// zero weight are left out; an empty bar shows only its track.
pub struct SegmentBar {
    segments: Vec<(f32, Color)>,
    width: Option<f32>,
    height: f32,
    track_color: Option<Color>,
}

pub fn segment_bar(segments: impl IntoIterator<Item = (f32, Color)>) -> SegmentBar {
    SegmentBar {
        segments: segments
            .into_iter()
            .filter(|(weight, _)| *weight > 0.0)
            .collect(),
        width: None,
        height: Sz::PROGRESS_H,
        track_color: None,
    }
}

impl SegmentBar {
    /// A fixed width; by default the bar fills its parent.
    pub fn width(mut self, w: f32) -> Self {
        self.width = Some(w);
        self
    }

    pub fn height(mut self, h: f32) -> Self {
        self.height = h;
        self
    }

    pub fn track_color(mut self, c: Color) -> Self {
        self.track_color = Some(c);
        self
    }
}

impl RenderOnce for SegmentBar {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let h = self.height;
        view! {
            <div
                class="flex-row"
                h={h}
                gap={Sz::SEPARATOR_W}
                class="overflow-hidden"
                rounded={h / 2.0}
                bg={self
                    .track_color
                    .unwrap_or(cx.theme.colors.element_background)}
                w={if let Some(w) = self.width {
                    w
                }}
                @when {self.width.is_none()} { class="w-full" }
            >
                // Flex grow splits the width left after the gaps by weight.
                for (weight, color) in self.segments {
                    <div class="h-full" bg={color} flex_grow_val={weight} />
                }
            </div>
        }
    }
}

#[cfg(test)]
mod tests {
    use quark::reactive::SignalStore;
    use quark_render::{Primitive, Scene};
    use quark_ui::element::render_element;
    use quark_ui::theme::Theme;

    use super::*;

    const RED: Color = Color::rgba(255, 0, 0, 255);
    const GREEN: Color = Color::rgba(0, 255, 0, 255);
    const BLUE: Color = Color::rgba(0, 0, 255, 255);

    /// `(color, width)` of each rect painted in one of the test colors.
    fn painted_segments(bar: SegmentBar) -> Vec<(&'static str, f32)> {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let store = SignalStore::new();
        let theme = Theme::default_dark();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store);
        let mut scene = Scene::default();
        render_element(&mut bar.into_any(), &mut scene, &mut cx, 400.0, 100.0);
        scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::RoundedRect(rr) => [("red", RED), ("green", GREEN), ("blue", BLUE)]
                    .into_iter()
                    .find(|(_, c)| *c == rr.color)
                    .map(|(name, _)| (name, rr.rect.width)),
                _ => None,
            })
            .collect()
    }

    // Catches segments not splitting the width left after the 1px gaps by
    // weight, or a zero weight segment still taking a sliver and a gap.
    #[test]
    fn segments_split_the_bar_by_weight_and_zero_weights_take_no_room() {
        let bar = segment_bar([(3.0, RED), (0.0, GREEN), (1.0, BLUE)]).width(101.0);

        assert_eq!(painted_segments(bar), [("red", 75.0), ("blue", 25.0)]);
    }
}
