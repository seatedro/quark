//! `UiContext::start_drag_out` from inside a pointer drag. A headless
//! window has no native drag source, so this covers what the app sees when
//! a drag out cannot start: the reason, and its own drag carrying on.

use quark_ui::element::{AnyElement, DragHandler, DragReleaseResult, IntoAnyElement, div};
use quark_ui::style::Styled;

use crate::platform::drag_out::DragOutError;
use crate::testing::UiTestHarness;
use crate::ui::{UiApp, UiContext, ViewContext};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Moved,
    Released,
}

impl From<Msg> for quark_ui::Action {
    fn from(msg: Msg) -> Self {
        quark_ui::Action::new(msg)
    }
}

struct Track;

impl DragHandler for Track {
    fn on_move(&mut self, _x: f32, _y: f32) -> Vec<quark_ui::Action> {
        vec![Msg::Moved.into()]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![Msg::Released.into()],
        }
    }

    fn on_cancel(&mut self) -> Vec<quark_ui::Action> {
        Vec::new()
    }
}

/// A file row that tries to drag `paths` out on its drag's first move.
struct FileRow {
    paths: Vec<&'static str>,
    result: Option<Result<(), DragOutError>>,
    log: Vec<Msg>,
}

impl UiApp for FileRow {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        div()
            .w(100.0)
            .h(40.0)
            .on_drag(|_| Box::new(Track))
            .into_any()
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        if msg == Msg::Moved && self.result.is_none() {
            self.result = Some(cx.start_drag_out(&self.paths));
        }
        self.log.push(msg);
    }
}

#[test]
fn a_drag_out_that_cannot_start_says_why_and_leaves_the_drag_running() {
    let unavailable = if crate::platform::drag_out::supported() {
        DragOutError::NoWindow
    } else {
        DragOutError::Unsupported
    };
    let cases = [
        (vec!["notes.txt", "/tmp/photo.png"], unavailable),
        (vec![], DragOutError::NoPaths),
    ];
    for (paths, expected) in cases {
        let app = FileRow {
            paths: paths.clone(),
            result: None,
            log: Vec::new(),
        };
        let mut ui = UiTestHarness::new(app, (200.0, 100.0), 1.0);
        ui.frame();
        ui.drag((10.0, 10.0), (150.0, 60.0));

        assert_eq!(ui.app().result, Some(Err(expected)), "{paths:?}");
        // Every move still arrives, then the release from the pointer up.
        let log = &ui.app().log;
        assert!(log.len() > 2 && log[..log.len() - 1].iter().all(|m| *m == Msg::Moved));
        assert_eq!(log.last(), Some(&Msg::Released));
    }
}
