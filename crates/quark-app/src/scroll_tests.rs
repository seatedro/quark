//! Scrolling through the whole adapter path: wheel and Shift+wheel, the
//! scrollbar, keys, programmatic and smooth scrolls, and flings, driven by
//! [`UiTestHarness`] with its fake clock.

use quark_ui::FocusId;
use quark_ui::element::{AnyElement, Div, IntoAnyElement, ScrollAlign, ScrollHandle, div};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;
use winit::keyboard::ModifiersState;

use crate::InputEvent;
use crate::testing::UiTestHarness;
use crate::ui::{UiAdapter, UiApp, UiContext, ViewContext};

/// The window is 200x200 points.
const SIZE: (f32, f32) = (200.0, 200.0);

struct Scroller {
    handle: ScrollHandle,
    build: fn(&ScrollHandle) -> Div,
}

impl UiApp for Scroller {
    type Action = ();
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        (self.build)(&self.handle).into_any()
    }

    fn update(&mut self, _action: (), _cx: &mut UiContext) {}
}

/// The window over 1000x1000 points of content, scrolling both ways.
fn grid(handle: &ScrollHandle) -> Div {
    div()
        .size_full()
        .track_scroll(handle)
        .overflow_scroll()
        .focus_ring(FocusId::from_key("grid"))
        .child(div().w(1000.0).h(1000.0).flex_shrink_0())
}

/// The window over 50 keyed rows 40 points tall, scrolling vertically.
fn rows(handle: &ScrollHandle) -> Div {
    div()
        .size_full()
        .flex_col()
        .track_scroll(handle)
        .overflow_y_scroll()
        .children_from((0..50).map(|i| {
            div()
                .key(format!("row-{i}"))
                .w_full()
                .h(40.0)
                .flex_shrink_0()
        }))
}

fn harness(build: fn(&ScrollHandle) -> Div, theme: Theme) -> UiTestHarness<Scroller> {
    let app = Scroller {
        handle: ScrollHandle::new(),
        build,
    };
    let adapter = UiAdapter::new(app, "scroll").with_theme(theme);
    let mut ui = UiTestHarness::with_adapter(adapter, SIZE, 1.0);
    ui.frame();
    ui
}

fn offset(ui: &UiTestHarness<Scroller>) -> (f32, f32) {
    let (x, y) = ui.app().handle.offset();
    // Whole points keep the expectations readable.
    (x.round(), y.round())
}

#[test]
fn shift_wheel_and_sideways_trackpad_motion_scroll_horizontally() {
    let cases = [
        ("wheel", false, (0.0, 60.0), (0.0, 60.0)),
        ("shift+wheel", true, (0.0, 60.0), (60.0, 0.0)),
        ("trackpad sideways", false, (60.0, 0.0), (60.0, 0.0)),
    ];
    for (name, shift, (dx, dy), expected) in cases {
        let mut ui = harness(grid, Theme::default_dark());
        ui.pointer_move((100.0, 100.0));
        if shift {
            ui.send_event(InputEvent::ModifiersChanged(ModifiersState::SHIFT));
        }
        ui.wheel(dx, dy);
        assert_eq!(offset(&ui), expected, "{name}");
    }
}

#[test]
fn dragging_the_thumb_maps_its_travel_onto_the_content() {
    // The vertical track runs y=6..186 (it stops short of the horizontal
    // bar); the thumb is 36 of it, so 144 points of travel cover the 800
    // points of offset. Grab the thumb's middle and move it halfway.
    let mut ui = harness(grid, Theme::default_dark());
    ui.drag((196.0, 24.0), (196.0, 96.0));
    assert_eq!(offset(&ui), (0.0, 400.0));
}

/// A 40 point strip over 400 points of content: its 28 point track is
/// shorter than the minimum thumb, so the thumb fills it.
fn strip(handle: &ScrollHandle) -> Div {
    div()
        .w(200.0)
        .h(40.0)
        .track_scroll(handle)
        .overflow_y_scroll()
        .child(div().w_full().h(400.0).flex_shrink_0())
}

// Regression: a thumb that fills its track asked for offset 0 on any drag,
// so grabbing it jumped the content to the top.
#[test]
fn dragging_a_thumb_that_fills_its_track_keeps_the_offset() {
    let mut ui = harness(strip, Theme::default_dark());
    ui.app().handle.set_offset(0.0, 60.0);
    ui.frame();
    ui.drag((196.0, 20.0), (196.0, 30.0));
    assert_eq!(offset(&ui), (0.0, 60.0));
}

#[test]
fn pressing_the_track_pages_toward_the_press() {
    // A page of the 200 point viewport is 160 points.
    let mut ui = harness(grid, Theme::default_dark());
    let got: Vec<f32> = [150.0, 150.0, 20.0]
        .into_iter()
        .map(|y| {
            ui.click((196.0, y));
            offset(&ui).1
        })
        .collect();
    assert_eq!(got, [160.0, 320.0, 160.0]);
}

#[test]
fn keys_scroll_the_focused_container() {
    let mut ui = harness(grid, Theme::default_dark());
    ui.click((100.0, 100.0));
    let got: Vec<(&str, (f32, f32))> = ["pagedown", "arrowdown", "end", "arrowright", "home"]
        .into_iter()
        .map(|key| {
            ui.key(key);
            ui.advance(300);
            (key, offset(&ui))
        })
        .collect();
    assert_eq!(
        got,
        [
            ("pagedown", (0.0, 160.0)),
            ("arrowdown", (0.0, 200.0)),
            ("end", (0.0, 800.0)),
            ("arrowright", (40.0, 800.0)),
            ("home", (40.0, 0.0)),
        ]
    );
}

#[test]
fn scroll_to_item_puts_the_keyed_row_where_asked() {
    // row-10 spans y=400..440 of the content.
    let cases = [
        (ScrollAlign::Start, 400.0),
        (ScrollAlign::End, 240.0),
        (ScrollAlign::Center, 320.0),
        (ScrollAlign::Nearest, 240.0),
    ];
    for (align, expected) in cases {
        let mut ui = harness(rows, Theme::default_dark());
        ui.app().handle.scroll_to_item("row-10", align);
        ui.frame();
        assert_eq!(offset(&ui).1, expected, "{align:?}");
    }
}

#[test]
fn smooth_scrolls_animate_unless_motion_is_reduced() {
    for reduced_motion in [false, true] {
        let theme = Theme {
            reduced_motion,
            ..Theme::default_dark()
        };
        let mut ui = harness(rows, theme);
        ui.app().handle.animate_to(0.0, 400.0);
        ui.frame();
        ui.advance(100);
        let midway = offset(&ui).1;
        ui.advance(300);
        let landed = offset(&ui).1;
        if reduced_motion {
            assert_eq!(midway, 400.0, "reduced motion jumps");
        } else {
            assert!(midway > 0.0 && midway < 400.0, "midway at {midway}");
        }
        assert_eq!(landed, 400.0);
        assert!(!ui.frame_requested(), "idle once landed");
    }
}

// Platforms with their own momentum send it as further wheel events.
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[test]
fn a_fling_coasts_after_the_fingers_lift_then_slows_to_a_stop() {
    use winit::dpi::PhysicalPosition;
    use winit::event::{MouseScrollDelta, TouchPhase};

    let mut ui = harness(rows, Theme::default_dark());
    ui.pointer_move((100.0, 100.0));
    // Five 20 point moves 10 ms apart: 2 points/ms when the fingers lift.
    for _ in 0..5 {
        ui.wheel(0.0, 20.0);
        ui.advance(10);
    }
    ui.send_event(InputEvent::Wheel {
        delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 0.0)),
        phase: TouchPhase::Ended,
    });
    let lifted = offset(&ui).1;
    ui.advance(100);
    let coasting = offset(&ui).1;
    ui.advance(5_000);
    let stopped = offset(&ui).1;

    assert_eq!(lifted, 100.0);
    assert!(coasting > lifted, "coasts: {coasting}");
    // Exponential decay covers at most velocity * 325 ms.
    assert!(stopped > coasting && stopped <= lifted + 650.0, "{stopped}");
    assert!(!ui.frame_requested(), "no frames once stopped");
}
