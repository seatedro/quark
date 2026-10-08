//! Dock drags on the desktop platforms, driven through the headless runner:
//! desktop pointer motion under the source window's grab, window
//! placements with decorations and scale, and a scripted window stack.

use std::collections::HashMap;

use quark::scene::Scene;
use winit::event::{ElementState, MouseButton};
use winit::keyboard::{ModifiersState, NamedKey};

use super::*;
use crate::input::KeyChord;
use crate::runner::{App, AppEvent, CloseReason, FrameContext, HeadlessRunner, WindowOptions};

/// An app that runs one dock drag, logging what it reports, and follows
/// the pointer with the next window that opens when asked.
#[derive(Default)]
struct Docking {
    drag: Option<DockDrag>,
    stack: ScriptedStack,
    names: HashMap<WindowHandle, &'static str>,
    log: Vec<String>,
    /// The hotspot to follow the next opened window by.
    follow_next: Option<(f32, f32)>,
}

impl Docking {
    fn location(&self, cx: &EventContext, location: DragLocation) -> String {
        match location {
            DragLocation::Window { window, point } => {
                let name = window_of(cx, window).map_or("?", |window| self.names[&window]);
                format!("{name} {},{}", point.0, point.1)
            }
            other => format!("{other:?}"),
        }
    }

    fn report(&mut self, cx: &EventContext, event: Option<DockDragEvent>) {
        let line = match event {
            None => return,
            Some(DockDragEvent::Moved(at)) => format!("moved {}", self.location(cx, at)),
            Some(DockDragEvent::Released(at)) => format!("released {}", self.location(cx, at)),
            Some(DockDragEvent::Cancelled(reason)) => format!("cancelled {reason:?}"),
        };
        self.log.push(line);
    }
}

impl App for Docking {
    fn frame(&mut self, _: &mut FrameContext) -> Scene {
        Scene::default()
    }

    fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
        let reported = self
            .drag
            .as_mut()
            .and_then(|drag| drag.handle_input(cx, &event));
        self.report(cx, reported);
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
        let Some(drag) = self.drag.as_mut() else {
            return;
        };
        match event {
            AppEvent::WindowOpened(window) => {
                if let Some(hotspot) = self.follow_next.take() {
                    drag.follow(cx, window, hotspot).unwrap();
                }
            }
            AppEvent::WindowClosed { window, .. } => {
                let reported = drag.window_closed(window);
                self.report(cx, reported);
            }
            _ => {}
        }
    }
}

/// A 400x300 main window at the desktop's corner, a 200x100 "tools"
/// window at `tools`, both raised in that order, and the first frames
/// drawn.
fn desktop(tools: DesktopPoint) -> (HeadlessRunner, Docking, WindowHandle, WindowHandle) {
    let mut runner = HeadlessRunner::new((400.0, 300.0), 1.0);
    let mut app = Docking::default();
    let main = runner.main_window();
    app.names.insert(main, "main");
    let tools = open(&mut runner, &mut app, "tools", tools, (200.0, 100.0));
    app.stack.raise(main).raise(tools);
    runner.run_until_idle(&mut app);
    (runner, app, main, tools)
}

fn open(
    runner: &mut HeadlessRunner,
    app: &mut Docking,
    name: &'static str,
    position: DesktopPoint,
    size: (f64, f64),
) -> WindowHandle {
    runner.callback(app, |app, cx| {
        let window = cx.open_window(WindowOptions {
            title: name.to_owned(),
            size,
            position: Some(position),
            active: false,
            ..WindowOptions::default()
        });
        app.names.insert(window, name);
        window
    })
}

/// Press the primary button at `at` over `source` and start a drag there,
/// which `source`'s grab then feeds.
fn press_and_start(
    runner: &mut HeadlessRunner,
    app: &mut Docking,
    source: WindowHandle,
    at: DesktopPoint,
) {
    runner.desktop_pointer_move(app, at);
    runner.desktop_button(app, MouseButton::Left, ElementState::Pressed);
    runner.callback_in(source, app, |app, cx| {
        let pointer = cx.from_desktop(source, at).unwrap();
        let start = DockDragStart::new(source, pointer, "panel");
        let stack = Box::new(app.stack.clone());
        app.drag = Some(DockDrag::start_with_stack(cx, start, stack).unwrap());
    });
    app.log.clear();
}

fn escape() -> InputEvent {
    InputEvent::KeyPress(KeyChord {
        logical: KeyKind::Named(NamedKey::Escape),
        physical: None,
        modifiers: ModifiersState::empty(),
        repeat: false,
    })
}

// The drop location is the window actually on top under the pointer: an
// app's own window over another of its windows takes the drop, and another
// app's window over one of ours blocks it (outside, never a drop into the
// hidden window).
#[test]
fn the_window_on_top_under_the_pointer_is_the_drop_location() {
    // tools overlaps main's lower right corner; another app's window
    // covers main from x 100 to 200.
    let (mut runner, mut app, main, _) = desktop((300.0, 200.0));
    app.stack.raise_other((100.0, 0.0), (100.0, 100.0));
    press_and_start(&mut runner, &mut app, main, (50.0, 50.0));
    let cases = [
        ((60.0, 50.0), "moved main 60,50"),
        ((150.0, 50.0), "moved Outside"),
        ((350.0, 250.0), "moved tools 50,50"),
        ((450.0, 250.0), "moved tools 150,50"),
        ((250.0, 250.0), "moved main 250,250"),
        ((700.0, 700.0), "moved Outside"),
    ];
    for (at, expected) in cases {
        app.log.clear();
        runner.desktop_pointer_move(&mut app, at);
        assert_eq!(app.log, [expected], "at {at:?}");
    }

    app.stack.break_queries();
    app.log.clear();
    runner.desktop_pointer_move(&mut app, (60.0, 50.0));
    assert_eq!(
        app.log,
        ["moved Unknown"],
        "the window system stopped answering"
    );
}

// A torn-off window follows the pointer by its grab point, whatever its
// decorations and scale, and is never itself the drop location: the
// window under it is.
#[test]
fn a_following_window_keeps_its_hotspot_under_the_pointer_and_is_looked_through() {
    let (mut runner, mut app, main, _) = desktop((500.0, 0.0));
    press_and_start(&mut runner, &mut app, main, (50.0, 50.0));
    app.follow_next = Some((10.0, 5.0));
    let float = open(&mut runner, &mut app, "float", (40.0, 45.0), (120.0, 80.0));
    runner.set_client_offset(float, (4.0, 20.0));
    runner.set_scale_factor(&mut app, float, 2.0);
    app.stack.raise(float);

    let mut seen = Vec::new();
    for at in [(600.0, 300.0), (100.0, 100.0)] {
        app.log.clear();
        runner.desktop_pointer_move(&mut app, at);
        let (outer, hotspot) = runner.callback(&mut app, |_, cx| {
            let placement = cx.placement(float).unwrap();
            (placement.outer_position, cx.to_desktop(float, (10.0, 5.0)))
        });
        seen.push(format!("{:?} outer {outer:?} hotspot {hotspot:?}", app.log));
    }

    assert_eq!(
        seen,
        [
            r#"["moved Outside"] outer Some((576.0, 270.0)) hotspot Some((600.0, 300.0))"#,
            r#"["moved main 100,100"] outer Some((76.0, 70.0)) hotspot Some((100.0, 100.0))"#,
        ]
    );
}

// X11 reports motion over the torn-off window against that window, whose
// position lags the moves asked of it; converting it through that position
// moved the window by twice the pointer's motion. The window system's own
// pointer position wins, and without one the motion is skipped.
#[test]
fn motion_reported_by_the_following_window_is_placed_by_the_window_system() {
    let (mut runner, mut app, main, _) = desktop((500.0, 0.0));
    press_and_start(&mut runner, &mut app, main, (50.0, 50.0));
    app.follow_next = Some((10.0, 5.0));
    let float = open(&mut runner, &mut app, "float", (40.0, 45.0), (120.0, 80.0));
    // The window system has the pointer at 300,200; the window's stale
    // position would put this motion at 40+70, 45+60.
    let moved = InputEvent::PointerMoved { x: 70.0, y: 60.0 };
    let outer = |runner: &mut HeadlessRunner, app: &mut Docking| {
        runner.callback(app, |_, cx| cx.placement(float).unwrap().outer_position)
    };

    runner.input(&mut app, float, moved.clone());
    assert_eq!(outer(&mut runner, &mut app), Some((40.0, 45.0)), "skipped");

    app.stack.set_pointer(Some((300.0, 200.0)));
    runner.input(&mut app, float, moved);
    assert_eq!(outer(&mut runner, &mut app), Some((290.0, 195.0)));
}

// Every way a drag ends must end it exactly once with its reason, and
// focus loss alone must not: only a focus loss with the button already up
// means another client took the pointer.
#[test]
fn a_drag_ends_on_release_escape_lost_capture_or_a_closed_window() {
    type End = fn(&mut HeadlessRunner, &mut Docking, WindowHandle);
    let cases: [(&str, End, &[&str]); 5] = [
        (
            "released over tools",
            |runner, app, _| {
                runner.desktop_pointer_move(app, (550.0, 50.0));
                runner.desktop_button(app, MouseButton::Left, ElementState::Released);
            },
            &["moved tools 50,50", "released tools 50,50"],
        ),
        (
            "escape",
            |runner, app, main| runner.input(app, main, escape()),
            &["cancelled Escape"],
        ),
        (
            "focus lost with the button up",
            |runner, app, main| {
                app.stack.set_primary_held(Some(false));
                runner.set_focus(app, main, false);
            },
            &["cancelled CaptureLost"],
        ),
        (
            "focus lost with the button held",
            |runner, app, main| {
                app.stack.set_primary_held(Some(true));
                runner.set_focus(app, main, false);
            },
            // Still running: the Escape after it cancels it.
            &["cancelled Escape"],
        ),
        (
            "source window closed",
            |runner, app, main| {
                runner.request_close(app, main, CloseReason::User);
            },
            &["cancelled WindowClosed"],
        ),
    ];
    for (name, end, expected) in cases {
        let (mut runner, mut app, main, _) = desktop((500.0, 0.0));
        press_and_start(&mut runner, &mut app, main, (50.0, 50.0));

        end(&mut runner, &mut app, main);
        // Nothing after the end.
        runner.input(&mut app, main, escape());

        assert_eq!(app.log, expected, "{name}");
    }
}
