//! A slider: a value in a range, set by dragging the thumb, clicking the
//! track, or the keyboard.
//!
//! Keyboard: Right and Up add a step, Left and Down take one away, Page Up
//! and Page Down move by a page (ten steps unless set), Home and End go to
//! the ends. Values snap to the step and stay in range.

use std::cell::Cell;
use std::rc::Rc;

use quark::{Rect, view};
use quark_render::Scene;
use quark_ui::accessibility::NumericValue;
use quark_ui::element::{
    AnyElement, Bounds, ClickEvent, CursorHint, DragHandler, DragReleaseResult, Element,
    ElementContext, IntoAnyElement, LayoutEngine, LayoutId, RenderOnce, div, text,
};
use quark_ui::style::Styled;
use quark_ui::{Action, FocusId};

/// Focus target of the slider `id`.
pub fn slider_focus_id(id: &str) -> FocusId {
    FocusId::from_key(id)
}

/// A slider named `label` showing `value` between `min` and `max`.
/// `on_change(v)` is the app's action for the new value `v`. `id` must be
/// unique in the window.
pub fn slider(
    id: &str,
    label: impl Into<String>,
    value: f32,
    min: f32,
    max: f32,
    on_change: impl Fn(f32) -> Action + 'static,
) -> Slider {
    Slider {
        id: id.to_owned(),
        label: label.into(),
        range: SliderRange {
            min,
            max: max.max(min),
            step: 1.0,
        },
        value,
        page: None,
        on_change: Rc::new(on_change),
        width: None,
        disabled: false,
        show_value: true,
    }
}

pub struct Slider {
    id: String,
    label: String,
    range: SliderRange,
    value: f32,
    page: Option<f32>,
    on_change: Rc<dyn Fn(f32) -> Action>,
    width: Option<f32>,
    disabled: bool,
    show_value: bool,
}

impl Slider {
    /// Values snap to multiples of `step` from `min`. Defaults to 1.
    pub fn step(mut self, step: f32) -> Self {
        if step > 0.0 {
            self.range.step = step;
        }
        self
    }

    /// The Page Up and Page Down move. Defaults to ten steps.
    pub fn page_step(mut self, page: f32) -> Self {
        self.page = Some(page);
        self
    }

    pub fn width(mut self, width: f32) -> Self {
        self.width = Some(width);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// Show the value as text after the track. On by default.
    pub fn show_value(mut self, show: bool) -> Self {
        self.show_value = show;
        self
    }
}

/// A slider's range and step, which snapping and keyboard steps use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SliderRange {
    pub min: f32,
    pub max: f32,
    pub step: f32,
}

/// `10^d` for the decimal places a step can need. A table rather than
/// `powi`, which Kani cannot model; the values are the same, all exact.
const POW10: [f32; 7] = [1.0, 10.0, 100.0, 1e3, 1e4, 1e5, 1e6];

impl SliderRange {
    /// `value` moved to the nearest step and into range.
    pub fn snap(&self, value: f32) -> f32 {
        let steps = ((value - self.min) / self.step).round();
        let snapped = self.min + steps * self.step;
        // Round off float noise below the step's precision (0.1 + 0.2).
        let scale = POW10[self.decimals()];
        ((snapped * scale).round() / scale).clamp(self.min, self.max)
    }

    /// Decimal places the step needs, for display: 0 for 1, 1 for 0.5, 2
    /// for 0.25.
    pub fn decimals(&self) -> usize {
        (0..6)
            .find(|&d| {
                let scaled = self.step * POW10[d];
                (scaled - scaled.round()).abs() < 1e-3
            })
            .unwrap_or(6)
    }

    /// The value at fraction `t` of the range.
    fn at(&self, t: f32) -> f32 {
        self.snap(self.min + t.clamp(0.0, 1.0) * (self.max - self.min))
    }

    fn fraction(&self, value: f32) -> f32 {
        if self.max > self.min {
            ((value - self.min) / (self.max - self.min)).clamp(0.0, 1.0)
        } else {
            0.0
        }
    }
}

impl RenderOnce for Slider {
    fn render(self, cx: &ElementContext) -> AnyElement {
        let tc = &cx.theme.colors;
        let m = &cx.theme.metrics;
        let scale = m.ui_scale();
        let range = self.range;
        let value = range.snap(self.value);
        let t = range.fraction(value);
        let thumb = (m.ui_font_size * 1.125).round();
        let bar_h = (4.0 * scale).round();
        let width = self.width.unwrap_or((220.0 * scale).round());
        let value_text = format!("{value:.*}", range.decimals());
        let (fill, rest, knob) = if self.disabled {
            (tc.text_muted, tc.element_background, tc.text_muted)
        } else {
            (tc.accent, tc.element_background, tc.text_strong)
        };

        let probe = Rc::new(Cell::new(Rect::default()));
        let page = self.page.unwrap_or(range.step * 10.0);
        let change = &self.on_change;
        let track = view! {
            <div class="flex-row items-center" w={width} h={thumb + 4.0 * scale}
                 rounded={thumb / 2.0} accessibility_id={self.id.clone()} test_id="slider"
                 accessibility_role={accesskit::Role::Slider} aria-label={self.label.clone()}
                 aria-valuetext={value_text.clone()}
                 accessibility_numeric={NumericValue {
                     value: f64::from(value),
                     min: f64::from(range.min),
                     max: f64::from(range.max),
                     step: Some(f64::from(range.step)),
                 }}
                 aria-disabled={self.disabled}
                 @when {!self.disabled} {
                     on_key={("arrowright", change(range.snap(value + range.step)))}
                     on_key={("arrowup", change(range.snap(value + range.step)))}
                     on_key={("arrowleft", change(range.snap(value - range.step)))}
                     on_key={("arrowdown", change(range.snap(value - range.step)))}
                     on_key={("pageup", change(range.snap(value + page)))}
                     on_key={("pagedown", change(range.snap(value - page)))}
                     on_key={("home", change(range.snap(range.min)))}
                     on_key={("end", change(range.snap(range.max)))}
                     focus_ring={slider_focus_id(&self.id)} class="cursor-pointer"
                     on:drag={{
                         let on_change = self.on_change.clone();
                         let bounds = probe.clone();
                         move |press: ClickEvent| {
                             Box::new(SliderDrag {
                                 track: bounds.get(),
                                 thumb,
                                 range,
                                 on_change: on_change.clone(),
                                 last: None,
                                 press_x: press.x,
                             }) as Box<dyn DragHandler>
                         }
                     }}
                 }>
                // The thumb sits between the filled and empty parts of the
                // bar, which split the width less the thumb by `t`; a pointer
                // at x maps back through the same span (`SliderDrag::value_at`).
                <div class="flex-1" flex_grow_val={t} h={bar_h} rounded={bar_h / 2.0} bg={fill} />
                <div class="shrink-0" w={thumb} h={thumb} rounded={thumb / 2.0} bg={knob}
                     border={fill} />
                <div class="flex-1" flex_grow_val={1.0 - t} h={bar_h} rounded={bar_h / 2.0}
                     bg={rest} />
            </div>
        };
        view! {
            <div class="flex-row items-center" gap={m.spacing_sm}>
                {BoundsProbe {
                    child: track,
                    bounds: probe,
                }}
                if self.show_value {
                    <text class="text-sm" color={if self.disabled { tc.text_muted } else { tc.text }}>
                        {value_text}
                    </text>
                }
            </div>
        }
    }
}

/// A drag on a slider's track: the press jumps to the pointer, and moves
/// follow it.
struct SliderDrag {
    /// The track's bounds in the frame the press landed in.
    track: Rect,
    thumb: f32,
    range: SliderRange,
    on_change: Rc<dyn Fn(f32) -> Action>,
    /// The last value emitted, so moves within one step emit nothing.
    last: Option<f32>,
    press_x: f32,
}

impl SliderDrag {
    fn value_at(&self, x: f32) -> f32 {
        let span = (self.track.width - self.thumb).max(1.0);
        self.range.at((x - self.track.x - self.thumb / 2.0) / span)
    }

    fn emit(&mut self, x: f32) -> Vec<Action> {
        let value = self.value_at(x);
        if self.last == Some(value) {
            return Vec::new();
        }
        self.last = Some(value);
        vec![(self.on_change)(value)]
    }
}

impl DragHandler for SliderDrag {
    fn on_press(&mut self) -> Vec<Action> {
        self.emit(self.press_x)
    }

    fn on_move(&mut self, x: f32, _y: f32) -> Vec<Action> {
        self.emit(x)
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult::empty()
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Grabbing
    }
}

/// Records its child's painted bounds, for handlers that run after the
/// frame (a drag maps pointer positions through the track's bounds).
struct BoundsProbe {
    child: AnyElement,
    bounds: Rc<Cell<Rect>>,
}

impl Element for BoundsProbe {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        // The child's node is this element's node: the probe adds no box.
        (self.child.request_layout(engine, cx), ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        self.bounds.set(bounds);
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
        self.child.paint(engine, scene, cx);
    }
}

impl IntoAnyElement for BoundsProbe {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    /// For any range and step in quarter points (exact in `f32`, as are
    /// the snapped values), a snapped value is in range, on a step from
    /// `min` unless it is `max`, and the nearest such value to the input.
    #[kani::proof]
    #[kani::unwind(8)]
    fn snap_lands_on_the_nearest_step_in_range() {
        let quarters = |q: i16| f32::from(q) / 4.0;
        let min = quarters(kani::any::<i8>().into());
        let max = min + quarters((kani::any::<u8>() % 128).into());
        let step = quarters((kani::any::<u8>() % 32 + 1).into());
        let range = SliderRange { min, max, step };
        let value = quarters(kani::any::<i16>() % 1024);

        let snapped = range.snap(value);

        assert!(min <= snapped && snapped <= max);
        let steps = (snapped - min) / step;
        assert!(snapped == max || steps == steps.round());
        assert!((snapped - value.clamp(min, max)).abs() <= step / 2.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snap_rounds_to_the_step_clamps_and_drops_float_noise() {
        let tenths = SliderRange {
            min: 0.0,
            max: 1.0,
            step: 0.1,
        };
        let fives = SliderRange {
            min: -10.0,
            max: 10.0,
            step: 5.0,
        };
        let cases = [
            (tenths, 0.1 + 0.2, 0.3),
            (tenths, 0.94, 0.9),
            (tenths, 1.7, 1.0),
            (fives, 2.4, 0.0),
            (fives, 2.6, 5.0),
            (fives, -12.0, -10.0),
        ];
        for (range, input, expected) in cases {
            assert_eq!(range.snap(input), expected, "{input} in {range:?}");
        }
    }
}
