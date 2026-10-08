//! One app in two windows: each window's routing, focus, IME, and
//! accessibility stay its own, shared state repaints every window, and a
//! drag session moves between them.

use std::any::Any;
use std::collections::HashMap;

use accesskit::{Action as AxAction, ActionRequest, Role, TreeId};
use quark::animation::{AnimKey, Curve, Motion, PropId};
use quark::scene::{Primitive, Scene};
use quark_ui::element::{
    AnyElement, DragHandler, DragHandoff, DragOutcome, DragPreview, DragReleaseResult, DragResult,
    DropPolicy, DropTarget, DropTargetHit, DropTargetId, IntoAnyElement, canvas, div, text,
    text_input,
};
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};
use winit::window::Theme as SystemTheme;

use winit::event::{ElementState, MouseButton};
use winit::keyboard::{ModifiersState, NamedKey};

use super::*;
use crate::testing::UiTestHarness;
use crate::{AppEvent, CloseReason, InputEvent, KeyChord, KeyKind, WindowHandle, WindowOptions};

const FIELD: FocusId = FocusId::from_key("field");
/// Both windows' Save button, and the field below it.
const SAVE_AT: (f32, f32) = (10.0, 10.0);
const FIELD_AT: (f32, f32) = (20.0, 50.0);
/// The second window's drop target covers (0, 100)..(400, 300).
const DROP_AT: (f32, f32) = (100.0, 200.0);
/// The second window's corner on the desktop, right of the main one.
const SECOND_AT: (f64, f64) = (500.0, 0.0);
const PREVIEW: quark::Color = quark::Color::rgba(1, 2, 3, 255);
const ANIMATION: AnimKey = AnimKey(1);

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Save(&'static str),
    /// The tab's drag moved; tear it out of its window.
    TabMoved,
    TabCancelled,
    TabDropped(String),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// A tab drag that agrees to become a drag session showing a 40x20
/// preview.
struct TabDrag {
    preview: DragPreview,
}

impl TabDrag {
    fn new() -> Self {
        let preview = DragPreview::new(1, (40.0, 20.0), |_theme, (w, h)| {
            div().w(w).h(h).bg(PREVIEW).into_any()
        });
        Self { preview }
    }
}

impl DragHandler for TabDrag {
    fn on_move(&mut self, _x: f32, _y: f32) -> Vec<Action> {
        vec![Msg::TabMoved.into()]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult::empty()
    }

    fn on_cancel(&mut self) -> Vec<Action> {
        vec![Msg::TabCancelled.into()]
    }

    fn preview(&self) -> Option<&DragPreview> {
        Some(&self.preview)
    }

    fn on_handoff(&mut self) -> Option<DragHandoff> {
        Some(DragHandoff {
            payload: Box::new("tab"),
            actions: Vec::new(),
        })
    }

    fn on_session_drop(&mut self, outcome: &DragOutcome) -> Vec<Action> {
        vec![Msg::TabDropped(format!("{outcome:?}")).into()]
    }
}

/// Takes any drop.
struct Accept;

impl DropPolicy for Accept {
    fn revision(&self, _target: DropTargetId) -> Option<u64> {
        Some(0)
    }

    fn accepts(&self, _hit: &DropTargetHit, _payload: &dyn Any) -> bool {
        true
    }
}

/// The app in its main window and a second one it opens unfocused. Each
/// window shows its name, a Save button, and a text field with the same
/// focus id `FIELD`; the main window has a draggable tab, the second a
/// drop target. Its input hook moves a drag session as a platform
/// transport would, and F6 in the main window moves focus to the second
/// window's field.
#[derive(Default)]
struct Desk {
    main: Option<WindowHandle>,
    second: Option<WindowHandle>,
    fields: HashMap<WindowHandle, TextField>,
    saved: Vec<&'static str>,
    /// The second window animates, from a click on its Save button on.
    animate: bool,
    /// Finish the drag session on a release, as a transport would.
    finish_on_release: bool,
    tab: Vec<Msg>,
    /// How sessions the adapter ended came out.
    ended: Vec<DragResult>,
    closed: Vec<(WindowHandle, CloseReason)>,
}

impl Desk {
    fn harness() -> (UiTestHarness<Self>, WindowHandle, WindowHandle) {
        let app = Desk {
            finish_on_release: true,
            ..Desk::default()
        };
        let ui = UiTestHarness::new(app, (400.0, 300.0), 1.0);
        let (main, second) = (ui.app().main.unwrap(), ui.app().second.unwrap());
        (ui, main, second)
    }

    fn field(&self, window: WindowHandle) -> &TextField {
        &self.fields[&window]
    }

    /// Where `point` in `window` is, as a desktop transport finds it: over
    /// the second window, the main one, or neither. The pressed window gets
    /// all motion until the release, even outside it.
    fn locate(&self, cx: &UiContext, window: WindowHandle, point: (f32, f32)) -> DragLocation {
        let Some(desktop) = cx.window.to_desktop(window, point) else {
            return DragLocation::Unknown;
        };
        for under in [self.second, self.main].into_iter().flatten() {
            if let Some((x, y)) = cx.window.from_desktop(under, desktop)
                && (0.0..400.0).contains(&x)
                && (0.0..300.0).contains(&y)
            {
                return drag_location(under, (x, y));
            }
        }
        DragLocation::Outside
    }

    fn texts(&self) -> [&str; 2] {
        let [main, second] = [self.main, self.second].map(Option::unwrap);
        [self.field(main).text(), self.field(second).text()]
    }
}

impl UiApp for Desk {
    type Action = Msg;
    type Message = ();

    fn init(&mut self, cx: &mut UiContext) {
        let main = cx
            .window_handle()
            .expect("init is bound to the main window");
        let second = cx.window.open_window(WindowOptions {
            title: "Second".into(),
            size: (400.0, 300.0),
            position: Some(SECOND_AT),
            active: false,
            ..WindowOptions::default()
        });
        self.main = Some(main);
        self.second = Some(second);
        self.fields.insert(main, TextField::new(""));
        self.fields.insert(second, TextField::new(""));
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let window = cx.window_handle();
        let is_main = Some(window) == self.main;
        let name = if is_main { "Main" } else { "Second" };
        if !is_main && self.animate {
            let now_ms = cx.frame.elapsed().as_millis() as u64;
            let animations = cx.animations();
            if animations.get(ANIMATION, PropId(0)).is_none() {
                animations.set(ANIMATION, PropId(0), 0.0, now_ms);
                let motion = Motion::tween(500, Curve::Linear);
                animations.animate_to(ANIMATION, PropId(0), 1.0, motion, now_ms);
            }
        }
        let root = div()
            .w(400.0)
            .h(300.0)
            .bg(cx.theme.colors.background)
            .child(
                div()
                    .absolute()
                    .w(80.0)
                    .h(30.0)
                    .accessibility_role(Role::Button)
                    .accessibility_label("Save")
                    .on_click(Msg::Save(name))
                    .child(text(name)),
            )
            .child(
                div().absolute().top(40.0).child(
                    text_input("Note", "")
                        .field(&self.fields[&window])
                        .focus_target(FIELD)
                        .focused(cx.is_focused(FIELD))
                        .w(200.0)
                        .h(40.0),
                ),
            );
        let root = if is_main {
            root.child(
                div()
                    .absolute()
                    .left(300.0)
                    .w(60.0)
                    .h(30.0)
                    .on_drag(|_| Box::new(TabDrag::new())),
            )
        } else {
            root.child(
                div().absolute().top(100.0).child(
                    canvas(|bounds, _scene, cx| {
                        cx.add_drop_target(DropTarget {
                            id: DropTargetId { scope: 1, key: 2 },
                            revision: 0,
                            layout: bounds,
                            slots: &[],
                        })
                    })
                    .w(400.0)
                    .h(200.0),
                ),
            )
        };
        root.into_any()
    }

    fn update(&mut self, action: Msg, cx: &mut UiContext) {
        match action {
            Msg::Save(name) => {
                self.saved.push(name);
                self.animate |= name == "Second";
            }
            Msg::TabMoved if cx.drag_session().is_none() => {
                cx.hand_off_drag().expect("the tab agrees to a session");
            }
            tab => self.tab.push(tab),
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        if *event == InputEvent::KeyPress(f6()) {
            cx.set_focus_in(self.second.unwrap(), Some(FIELD));
            return true;
        }
        if cx.drag_session().is_none() {
            return false;
        }
        let window = cx.window_handle().unwrap();
        match event {
            InputEvent::PointerMoved { x, y } => {
                let location = self.locate(cx, window, (*x, *y));
                cx.update_drag(location, &Accept);
                false
            }
            InputEvent::PointerButton {
                state: ElementState::Released,
                ..
            } if self.finish_on_release => {
                let at = cx.window.pointer_position().unwrap();
                let location = self.locate(cx, window, at);
                cx.finish_drag(Some(location), &Accept);
                true
            }
            _ => false,
        }
    }

    fn window_closed(&mut self, window: WindowHandle, reason: CloseReason, _cx: &mut UiContext) {
        self.closed.push((window, reason));
    }

    fn edit_text_in(
        &mut self,
        window: WindowHandle,
        target: FocusId,
        command: TextEditCommand,
    ) -> TextEditOutcome {
        assert_eq!(target, FIELD);
        self.fields.get_mut(&window).unwrap().apply(command)
    }

    fn set_preedit_in(
        &mut self,
        window: WindowHandle,
        _target: FocusId,
        text: String,
        cursor: Option<(usize, usize)>,
    ) {
        self.fields
            .get_mut(&window)
            .unwrap()
            .set_preedit(&text, cursor);
    }

    fn drag_session_ended(&mut self, end: DragEnd, _cx: &mut UiContext) {
        self.ended.push(end.result);
    }
}

fn f6() -> KeyChord {
    KeyChord {
        logical: KeyKind::Named(NamedKey::F6),
        physical: None,
        modifiers: ModifiersState::empty(),
        repeat: false,
    }
}

/// Where the drag preview's quad is painted in `scene`.
fn preview_at(scene: &Scene) -> Option<(f32, f32)> {
    scene
        .expanded()
        .iter()
        .find_map(|primitive| match primitive {
            Primitive::Rect(p) if p.color == PREVIEW => Some((p.rect.x, p.rect.y)),
            Primitive::RoundedRect(p) if p.color == PREVIEW => Some((p.rect.x, p.rect.y)),
            _ => None,
        })
}

fn background(scene: &Scene) -> Option<quark::Color> {
    scene
        .primitives
        .iter()
        .find_map(|primitive| match primitive {
            Primitive::Rect(p) if p.rect.width == 400.0 => Some(p.color),
            Primitive::RoundedRect(p) if p.rect.width == 400.0 => Some(p.color),
            _ => None,
        })
}

// Catches per-window views built for the wrong window: each window paints
// what the app built for its own handle.
#[test]
fn each_window_paints_the_view_built_for_its_handle() {
    let (mut ui, main, second) = Desk::harness();
    let first_run = [main, second].map(|window| ui.window(window).painted_texts()[0].text.clone());
    assert_eq!(first_run, ["Main", "Second"]);
}

// Regression: the adapter kept one window's hit regions and
// accessibility actions, so after the second window painted, a click or an
// assistive tech action in the main window was routed through the second
// window's frame.
#[test]
fn painting_another_window_keeps_each_windows_targets() {
    let (mut ui, main, second) = Desk::harness();
    ui.window(main).frame();
    ui.window(second).frame();

    ui.window(main).click(SAVE_AT);
    let button = ui
        .window(main)
        .accessibility_update()
        .nodes
        .iter()
        .find(|(_, node)| node.role() == Role::Button)
        .map(|(id, _)| *id)
        .expect("the Save button");
    ui.window(main).accessibility_action(ActionRequest {
        action: AxAction::Click,
        target_tree: TreeId::ROOT,
        target_node: button,
        data: None,
    });
    ui.window(second).click(SAVE_AT);

    assert_eq!(ui.app().saved, ["Main", "Main", "Second"]);
}

// Catches focus or typed text crossing windows: the same focus id in two
// windows names two fields, and each window remembers its own focus.
#[test]
fn focus_and_typing_stay_in_their_window() {
    let (mut ui, main, second) = Desk::harness();
    ui.window(main).click(FIELD_AT);
    ui.window(main).type_text("a");
    ui.window(second).click(FIELD_AT);
    ui.window(second).type_text("b");

    assert_eq!(ui.app().texts(), ["a", "b"]);
    let focus = [main, second].map(|window| ui.window(window).focus_target());
    assert_eq!(focus, [Some(FIELD), Some(FIELD)]);
}

// Regression: a composition kept its owner after focus moved onto an
// element of another window, so the closing preedit and commit the
// platform still delivered to the window the user left landed in its
// field, which may by then stand for an editor moved to the other window.
#[test]
fn a_commit_queued_after_focus_moved_to_another_window_lands_nowhere() {
    fn clicked(window: WindowHandle) -> Vec<(WindowHandle, InputEvent)> {
        let button = |state| InputEvent::PointerButton {
            button: MouseButton::Left,
            state,
        };
        let (x, y) = FIELD_AT;
        vec![
            (window, InputEvent::PointerMoved { x, y }),
            (window, button(ElementState::Pressed)),
            (window, button(ElementState::Released)),
        ]
    }
    type Move = fn(WindowHandle, WindowHandle) -> Vec<(WindowHandle, InputEvent)>;
    let moves: [(&str, Move); 2] = [
        ("clicked into the other window", |_, second| clicked(second)),
        ("moved by the app from the main window", |main, _| {
            vec![(main, InputEvent::KeyPress(f6()))]
        }),
    ];
    for (name, move_focus) in moves {
        let (mut ui, main, second) = Desk::harness();
        ui.window(main).click(FIELD_AT);
        ui.window(main).ime_preedit("にほ", None);

        // The rest of the composition is queued before the frame that
        // resets main's IME.
        let mut events = move_focus(main, second);
        events.push((main, InputEvent::ImePreedit(String::new(), None)));
        events.push((main, InputEvent::ImeCommit("日本".into())));
        ui.send_window_events(events);

        assert_eq!(ui.window(second).focus_target(), Some(FIELD), "{name}");
        assert_eq!(ui.app().texts(), ["", ""], "{name}");
        assert_eq!(ui.app().field(main).preedit(), None, "{name}");
        assert_eq!(ui.window(main).ime().resets, 1, "{name}: main's IME reset");
    }
}

// Regression: closing a window mid-composition dropped its state without
// telling the field, which kept painting a preedit no commit would end
// (wherever the app shows that field next).
#[test]
fn closing_a_window_mid_composition_cancels_its_preedit() {
    let (mut ui, main, second) = Desk::harness();
    ui.window(second).click(FIELD_AT);
    ui.window(second).ime_preedit("にほ", None);

    assert!(ui.window(second).request_close());

    assert_eq!(ui.app().closed, [(second, CloseReason::User)]);
    assert_eq!(ui.app().field(second).preedit(), None);
    assert_eq!(ui.window(main).ime().resets, 0, "main's IME untouched");
}

// Regression: a desktop theme change repainted only the window the event
// was delivered against, leaving the others in the old theme.
#[test]
fn a_theme_change_repaints_every_window() {
    let (mut ui, main, second) = Desk::harness();

    ui.app_event(AppEvent::ThemeChanged(SystemTheme::Light));

    let light = Theme::default_light().colors.background;
    let backgrounds = [main, second].map(|window| background(ui.window(window).scene()));
    assert_eq!(backgrounds, [Some(light), Some(light)]);
}

// Catches redraws or animation frames leaking across windows: input that
// starts an animation in the second window, and the animation's frames,
// draw nothing in the main one.
#[test]
fn an_idle_window_stays_idle_while_another_animates() {
    let (mut ui, main, second) = Desk::harness();
    let before = [main, second].map(|window| ui.window(window).frame_count());

    ui.window(second).click(SAVE_AT);
    ui.advance(300);

    let after = [main, second].map(|window| ui.window(window).frame_count());
    assert_eq!(after[0], before[0], "main drew while idle");
    assert!(
        after[1] > before[1] + 10,
        "second animated: {before:?} -> {after:?}"
    );
}

/// Press the main window's tab and drag it far enough to hand off, then
/// on across the desktop over the second window's drop target. The main
/// window gets all of the motion, as platforms deliver a drag.
fn tear_tab_into_second(ui: &mut UiTestHarness<Desk>) {
    ui.desktop_move((310.0, 10.0));
    ui.desktop_press();
    ui.desktop_move((330.0, 10.0));
    ui.desktop_move((SECOND_AT.0 + f64::from(DROP_AT.0), f64::from(DROP_AT.1)));
}

// Catches a session's preview painted only by its source window (where
// the router used to paint it), or left behind there once the pointer
// moved on.
#[test]
fn a_handed_off_drag_previews_in_the_window_it_is_over() {
    let (mut ui, main, second) = Desk::harness();
    tear_tab_into_second(&mut ui);

    let previews = [main, second].map(|window| preview_at(ui.window(window).scene()));
    assert_eq!(previews, [None, Some(DROP_AT)]);
}

// Catches a drop resolved against another window's targets than the
// window under the release.
#[test]
fn releasing_a_session_drops_on_the_target_of_the_window_under_it() {
    let (mut ui, _, second) = Desk::harness();
    tear_tab_into_second(&mut ui);

    ui.desktop_release();

    let [Msg::TabDropped(outcome)] = &ui.app().tab[..] else {
        panic!("expected one drop, got {:?}", ui.app().tab);
    };
    let window = drag_window_id(second);
    assert!(
        outcome.starts_with(&format!("Target {{ window: {window:?}")),
        "{outcome}"
    );
    assert_eq!(preview_at(ui.window(second).scene()), None);
}

// Regression: a session nobody ended stayed open forever, its preview
// stuck over a window, once the user pressed Escape, closed the window
// the drag started in, or released where the transport finished nothing.
#[test]
fn the_adapter_cancels_a_session_on_escape_source_close_or_unclaimed_release() {
    type End = fn(&mut UiTestHarness<Desk>, WindowHandle, WindowHandle);
    let ends: [(&str, End); 3] = [
        ("Escape", |ui, _, second| ui.window(second).key("escape")),
        ("source closed", |ui, main, _| {
            ui.window(main).request_close();
        }),
        ("unclaimed release", |ui, _, _| {
            ui.app_mut().finish_on_release = false;
            ui.desktop_release();
        }),
    ];
    for (name, end) in ends {
        let (mut ui, main, second) = Desk::harness();
        tear_tab_into_second(&mut ui);

        end(&mut ui, main, second);

        assert_eq!(ui.app().ended, [DragResult::Cancelled], "{name}");
        assert_eq!(ui.app().tab, [Msg::TabCancelled], "{name}");
        assert_eq!(preview_at(ui.window(second).scene()), None, "{name}");
    }
}
