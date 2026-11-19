//! Signals through the whole adapter path: a write from a callback
//! redraws, and a frame clears what it painted.

use quark::reactive::Signal;
use quark_ui::element::{AnyElement, IntoAnyElement, div, reactive, text};
use quark_ui::style::Styled;

use crate::testing::UiTestHarness;
use crate::ui::{UiApp, UiContext, ViewContext};

/// Two labels, each in a reactive boundary over its own signal. A message
/// renames the first, or with `None` does nothing.
#[derive(Default)]
struct Board {
    labels: Option<[Signal<&'static str>; 2]>,
}

impl UiApp for Board {
    type Action = ();
    type Message = Option<&'static str>;

    fn init(&mut self, cx: &mut UiContext) {
        let signals = cx.signals();
        self.labels = Some([signals.create("First"), signals.create("Second")]);
    }

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        let labels = self.labels.expect("made in init");
        div()
            .flex_col()
            .children(labels.into_iter().enumerate().map(|(i, label)| {
                reactive(i as u64, move |cx| text(cx.read(label)).size(14.0)).into_any()
            }))
            .into_any()
    }

    fn update(&mut self, _action: (), _cx: &mut UiContext) {}

    fn message(&mut self, rename: Option<&'static str>, cx: &mut UiContext) {
        if let (Some(name), Some([first, _])) = (rename, self.labels) {
            first.set(cx.signals(), name);
        }
    }
}

fn board() -> UiTestHarness<Board> {
    UiTestHarness::new(Board::default(), (200.0, 100.0), 1.0)
}

// A message gets no redraw of its own: the write is what asks for it.
#[test]
fn a_signal_a_message_writes_redraws_the_window() {
    let mut ui = board();
    let frames = ui.frame_count();

    ui.send_message(Some("Renamed"));

    assert_eq!(ui.frame_count(), frames + 1);
    assert_eq!(ui.painted_text(), "Renamed\nSecond");
}

// Catches the frame's own publishes (focus, theme, scroll handles)
// leaving the store dirty, which would redraw after every later event.
#[test]
fn a_message_that_writes_no_signal_draws_no_frame() {
    let mut ui = board();
    ui.send_message(Some("Renamed"));
    let frames = ui.frame_count();

    ui.send_message(None);

    assert_eq!(ui.frame_count(), frames);
}
