//! The adapter in one window: painting at a scale factor, input reaching
//! fields and controls, focus rings, tooltips, title chrome, and
//! accessibility.

use std::thread::{self, ThreadId};

use accesskit::{Affine, NodeId, Role, TreeUpdate};
use quark::Rect;
use quark::reactive::SignalStore;
use quark::scene::{Scene, ShapedText};
use quark_ui::element::{
    AnyElement, Binding, CursorHint, ElementContext, ElementHandle, InputRouter, IntoAnyElement,
    div, text_input,
};
use quark_ui::style::Styled;
use quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};
use winit::window::{CursorIcon, Theme as SystemTheme};

use super::accessibility::scale_tree;
use super::frame::{Painted, paint};
use super::input::types_text;
use super::{HoveredTooltip, UiApp, UiContext, ViewContext};
use crate::testing::UiTestHarness;
use crate::{AppEvent, InputEvent};

mod ime;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Save,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

const FIELD: FocusId = FocusId::from_key("field");

fn node_with_role(update: &TreeUpdate, role: Role) -> NodeId {
    update
        .nodes
        .iter()
        .find(|(_, node)| node.role() == role)
        .map(|(id, _)| *id)
        .expect("node with role")
}

/// Long-lived text state, so tests can paint twice through one cache.
struct Fixture {
    text: quark_text::TextSystem,
    layouts: quark_text::LayoutCache,
    theme: Theme,
    signals: SignalStore,
}

impl Fixture {
    fn new() -> Self {
        Self {
            text: quark_text::TextSystem::vendored_only(&Default::default()),
            layouts: quark_text::LayoutCache::default(),
            theme: Theme::default_dark(),
            signals: SignalStore::new(),
        }
    }

    /// Paint `root` into a 400x300 point window at `scale`.
    fn paint(&mut self, mut root: AnyElement, scale: f32) -> Painted {
        self.layouts.begin_frame();
        let mut ecx = ElementContext::new(
            &self.theme,
            scale,
            &mut self.text,
            &mut self.layouts,
            None,
            &self.signals,
        );
        paint(&mut root, &mut ecx, Scene::default(), 400.0, 300.0)
    }

    /// Paint `field` as the focused text input `FIELD`.
    fn paint_field(&mut self, field: &TextField) -> Painted {
        self.layouts.begin_frame();
        let mut ecx = ElementContext::new(
            &self.theme,
            1.0,
            &mut self.text,
            &mut self.layouts,
            None,
            &self.signals,
        )
        .with_focus(Some(FIELD));
        let mut root = text_input("Name", "")
            .field(field)
            .focused(true)
            .focus_target(FIELD)
            .w(200.0)
            .h(52.0)
            .into_any();
        paint(&mut root, &mut ecx, Scene::default(), 400.0, 300.0)
    }
}

#[test]
fn option_letters_type_text_only_on_macos() {
    let option_h: Binding = "alt+h".parse().unwrap();
    assert_eq!(types_text(&option_h), cfg!(target_os = "macos"));
    assert!(!types_text(&"ctrl+h".parse().unwrap()));
    assert!(types_text(&"h".parse().unwrap()));
}

const MARK: quark::Color = quark::Color::rgba(1, 2, 3, 255);

/// A 100x50 point box, offset by (`x`, `y`) points, holding "Hi".
fn marked_box(x: f32, y: f32) -> AnyElement {
    div()
        .w(400.0)
        .h(300.0)
        .child(
            div()
                .absolute()
                .left(x)
                .top(y)
                .w(100.0)
                .h(50.0)
                .bg(MARK)
                .accessibility_id("box")
                .accessibility_role(Role::Button)
                .accessibility_label("Box")
                .on_click(Msg::Save)
                .child(quark_ui::element::text("Hi")),
        )
        .into_any()
}

fn marked_quad(scene: &Scene) -> Option<quark::Rect> {
    scene
        .primitives
        .iter()
        .find_map(|primitive| match primitive {
            quark::scene::Primitive::Rect(p) if p.color == MARK => Some(p.rect),
            quark::scene::Primitive::RoundedRect(p) if p.color == MARK => Some(p.rect),
            _ => None,
        })
}

/// The first text run's origin and shaped layout.
fn text_run(scene: &Scene) -> (quark::Rect, ShapedText) {
    scene
        .primitives
        .iter()
        .find_map(|primitive| match primitive {
            quark::scene::Primitive::TextRun(p) => Some((p.rect, p.layout.clone())),
            _ => None,
        })
        .expect("a text run")
}

fn layout_of(shaped: &ShapedText) -> &quark_text::TextLayout {
    shaped.downcast_ref().expect("a quark-text layout")
}

// Regression: elements laid out in physical pixels, so UI did not scale
// on HiDPI screens, and text was shaped at logical size and blurred.
#[test]
fn logical_element_at_scale_two_paints_physical_quad_and_text() {
    let mut fixture = Fixture::new();
    let mut scene = fixture.paint(marked_box(10.0, 20.0), 2.0).scene;
    crate::scene_to_physical(&mut scene, 2.0);

    let quad = marked_quad(&scene).expect("the box's background");
    assert_eq!(
        (quad.x, quad.y, quad.width, quad.height),
        (20.0, 40.0, 200.0, 100.0)
    );
    let (origin, shaped) = text_run(&scene);
    let layout = layout_of(&shaped);
    assert!(origin.x >= 20.0 && origin.y >= 40.0, "{origin:?}");
    let logical_size = layout.style().font_size;
    assert_eq!(
        layout.glyph(0).expect("glyph").font_size,
        logical_size * 2.0
    );
}

// Regression: the pointer stayed in physical pixels while hit regions
// were logical, so clicks on HiDPI screens landed at twice the offset.
#[test]
fn physical_pointer_at_scale_two_hits_element_at_logical_point() {
    let mut input = crate::input::InputNormalizer::new(2.0);
    let moved = input.normalize(winit::event::WindowEvent::CursorMoved {
        device_id: winit::event::DeviceId::dummy(),
        position: winit::dpi::PhysicalPosition::new(200.0, 100.0),
    });
    let [InputEvent::PointerMoved { x, y }] = moved[..] else {
        panic!("expected one pointer move, got {moved:?}");
    };

    // The box covers points 99..199 x 49..99; (100, 50) is just inside
    // and the unconverted (200, 100) is outside.
    let mut fixture = Fixture::new();
    let mut router = InputRouter::default();
    router.set_frame(fixture.paint(marked_box(99.0, 49.0), 2.0).input);
    let delivery = router.pointer_down(x, y, &mut None);
    let actions: Vec<_> = delivery
        .actions
        .iter()
        .filter_map(|action| action.downcast_ref::<Msg>().cloned())
        .collect();
    assert_eq!(actions, vec![Msg::Save], "pointer at ({x}, {y})");
}

// Regression: a layout shaped for the old scale was served from the
// cache after the window moved to a display with another scale.
#[test]
fn scale_change_reshapes_cached_text() {
    let mut fixture = Fixture::new();
    let (_, before) = text_run(&fixture.paint(marked_box(0.0, 0.0), 1.0).scene);
    let (_, after) = text_run(&fixture.paint(marked_box(0.0, 0.0), 2.0).scene);
    let (before, after) = (layout_of(&before), layout_of(&after));
    assert_eq!((before.scale_factor(), after.scale_factor()), (1.0, 2.0));
    assert_eq!(
        after.glyph(0).expect("glyph").font_size,
        before.glyph(0).expect("glyph").font_size * 2.0
    );
}

// accesskit expects physical pixels after transforms; the tree is built
// in points, so the root must carry the scale.
#[test]
fn accessibility_bounds_reach_accesskit_in_physical_pixels() {
    let mut fixture = Fixture::new();
    let painted = fixture.paint(marked_box(10.0, 20.0), 2.0);
    let mut update = painted.accessibility.tree_update("Test", None);
    scale_tree(&mut update, 2.0);

    let root = update.tree.as_ref().expect("tree").root;
    let transform = update
        .nodes
        .iter()
        .find(|(id, _)| *id == root)
        .and_then(|(_, node)| node.transform().copied())
        .unwrap_or(Affine::IDENTITY);
    let button = node_with_role(&update, Role::Button);
    let bounds = update
        .nodes
        .iter()
        .find(|(id, _)| *id == button)
        .and_then(|(_, node)| node.bounds())
        .expect("button bounds");
    let physical = transform.transform_rect_bbox(bounds);
    assert_eq!(
        (physical.x0, physical.y0, physical.x1, physical.y1),
        (20.0, 40.0, 220.0, 140.0)
    );
}

/// A save button above a text field, recording what reaches it.
struct ComposerApp {
    field: TextField,
    saved: usize,
    events: Vec<String>,
    messages: Vec<(String, ThreadId)>,
}

impl UiApp for ComposerApp {
    type Action = Msg;
    type Message = String;

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .bg(cx.theme.colors.background)
            .child(
                div()
                    .w(80.0)
                    .h(30.0)
                    .accessibility_role(Role::Button)
                    .accessibility_label("Save")
                    .on_click(Msg::Save),
            )
            .child(
                text_input("Message", "")
                    .field(&self.field)
                    .focus_target(FIELD)
                    .focused(cx.is_focused(FIELD))
                    .w(200.0)
                    .h(40.0),
            )
            .into_any()
    }

    fn update(&mut self, Msg::Save: Msg, _cx: &mut UiContext) {
        self.saved += 1;
    }

    fn message(&mut self, message: String, _cx: &mut UiContext) {
        self.messages.push((message, thread::current().id()));
    }

    fn app_event(&mut self, event: AppEvent, _cx: &mut UiContext) {
        self.events.push(format!("{event:?}"));
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        assert_eq!(target, FIELD);
        self.field.apply(command)
    }
}

/// A composer in a 400x300 point window, first frame painted.
fn composer(text: &str) -> UiTestHarness<ComposerApp> {
    let app = ComposerApp {
        field: TextField::new(text),
        saved: 0,
        events: Vec::new(),
        messages: Vec::new(),
    };
    UiTestHarness::new(app, (400.0, 300.0), 1.0)
}

/// The color of the window-sized background quad.
fn background(scene: &Scene) -> Option<quark::Color> {
    scene
        .primitives
        .iter()
        .find_map(|primitive| match primitive {
            quark::scene::Primitive::Rect(p) if p.rect.width == 400.0 => Some(p.color),
            quark::scene::Primitive::RoundedRect(p) if p.rect.width == 400.0 => Some(p.color),
            _ => None,
        })
}

// Regression: UiAdapter left App::app_event at its no-op default, so a
// run_ui app never saw theme changes, URLs, or notification clicks.
#[test]
fn app_event_reaches_the_app_and_the_theme_follows() {
    let mut ui = composer("");
    // Opening the window was one too.
    ui.app_mut().events.clear();

    ui.app_event(AppEvent::ThemeChanged(SystemTheme::Light));

    assert_eq!(ui.app().events, ["ThemeChanged(Light)"]);
    assert_eq!(
        background(ui.scene()),
        Some(Theme::default_light().colors.background)
    );
}

// Regression: worker threads could only wake the app, not hand it the
// value they had, so apps shared state behind locks to pass data in.
#[test]
fn messages_from_a_worker_arrive_in_order_on_the_ui_thread() {
    let mut ui = composer("");
    let sender = ui.sender();

    thread::spawn(move || {
        sender.send("connected".to_owned());
        sender.send("line 1".to_owned());
    })
    .join()
    .unwrap();
    ui.run_until_idle();

    let ui_thread = thread::current().id();
    assert_eq!(
        ui.app().messages,
        [
            ("connected".to_owned(), ui_thread),
            ("line 1".to_owned(), ui_thread)
        ]
    );
}

// Regression: every pointer move redrew the window, even within one
// element where no hover style can change.
#[test]
fn pointer_moves_redraw_only_when_the_hovered_elements_change() {
    let mut ui = composer("");

    let moves = [(10.0, 10.0), (12.0, 14.0), (10.0, 200.0), (30.0, 250.0)];
    let redraws = moves.map(|at| {
        let before = ui.frame_count();
        ui.pointer_move(at);
        ui.frame_count() > before
    });

    assert_eq!(redraws, [true, false, true, false]);
}

// The composer needs no input code of its own: typed text, editing
// keys, copy, and paste reach the focused field through edit_text and
// the clipboard, and clicking Save keeps the field focused.
#[test]
fn focused_field_gets_text_keys_and_clipboard_without_app_code() {
    let mut ui = composer("");

    ui.click((10.0, 50.0));
    ui.type_text("hi");
    ui.key("mod+a");
    ui.key("mod+c");
    let copied = ui.clipboard_text();
    ui.set_clipboard_text("pasted");
    ui.key("mod+v");
    ui.click((10.0, 10.0));

    assert_eq!(copied.as_deref(), Some("hi"));
    assert_eq!(ui.app().field.text(), "pasted");
    assert_eq!(ui.app().saved, 1);
    assert_eq!(ui.focus(), Some(FIELD), "Save kept the field focused");
}

const NOTES: FocusId = FocusId::from_key("notes");

/// An editor in a div that binds Enter to Save if `bind_enter`.
struct NotesApp {
    editor: quark_ui::text_input::Editor,
    bind_enter: bool,
    saved: usize,
}

impl UiApp for NotesApp {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let scroll = quark_ui::element::ScrollActionBuilder::new(|_| Msg::Save.into());
        let editor = quark_ui::text_input::text_editor_element(NOTES, scroll)
            .editor_snapshot(&self.editor)
            .focused(cx.is_focused(NOTES))
            .w(300.0)
            .h(100.0);
        let root = div().w(400.0).h(300.0);
        let root = if self.bind_enter {
            root.on_key("enter", Msg::Save)
        } else {
            root
        };
        root.child(editor).into_any()
    }

    fn update(&mut self, Msg::Save: Msg, _cx: &mut UiContext) {
        self.saved += 1;
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        assert_eq!(target, NOTES);
        self.editor.apply(command)
    }

    fn set_preedit(&mut self, _target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        self.editor.set_preedit(text, cursor);
    }
}

fn notes(bind_enter: bool) -> UiTestHarness<NotesApp> {
    let app = NotesApp {
        editor: quark_ui::text_input::Editor::default(),
        bind_enter,
        saved: 0,
    };
    let mut ui = UiTestHarness::new(app, (400.0, 300.0), 1.0);
    ui.click((10.0, 10.0));
    ui
}

#[test]
fn return_in_an_editor_inserts_a_newline_that_undoes() {
    let mut ui = notes(false);
    ui.type_text("ab");
    ui.key("enter");
    ui.type_text("cd");
    ui.key("shift+enter");
    assert_eq!(ui.app().editor.text(), "ab\ncd\n");
    ui.key("mod+z");
    assert!(ui.app().editor.text().len() < 6, "undo removes the newline");
    assert_eq!(ui.app().saved, 0);
}

#[test]
fn an_enter_binding_wins_over_the_editor_newline() {
    let mut ui = notes(true);
    ui.type_text("ab");
    ui.key("enter");
    ui.key("shift+enter");
    assert_eq!(ui.app().saved, 1);
    assert_eq!(ui.app().editor.text(), "ab\n");
}

// Return during composition belongs to the IME.
#[test]
fn return_mid_composition_leaves_the_editor_alone() {
    let mut ui = notes(false);
    ui.type_text("ab");
    ui.ime_preedit("に", None);
    ui.key("enter");
    assert_eq!(ui.app().editor.text(), "ab");
    let preedit = ui.app().editor.preedit().map(|p| p.text.as_str());
    assert_eq!(preedit, Some("に"));
}

#[test]
fn return_in_a_single_line_field_inserts_nothing() {
    let mut ui = composer("hi");
    ui.click((10.0, 50.0));
    ui.key("enter");
    assert_eq!(ui.app().field.text(), "hi");
}

// Regression: a selection drag held past a field's edge kept
// autoscrolling after the window lost focus, since no release came.
#[test]
fn window_blur_stops_selection_autoscroll() {
    let text = "a long line of text that runs well past the edge of the field";
    let cases = [
        ("button still held", false, true),
        ("window blurred", true, false),
    ];
    for (name, blur, grows) in cases {
        let mut ui = composer(text);
        ui.click((5.0, 50.0));
        // Long enough that the next press is not a double click.
        ui.advance(1_000);
        ui.pointer_down((5.0, 50.0));
        ui.pointer_move((260.0, 50.0));
        if blur {
            ui.focus_loss();
        }
        let before = ui.app().field.cursor();
        ui.advance(400);
        let after = ui.app().field.cursor();
        assert_eq!(after > before, grows, "{name}: cursor {before} -> {after}");
    }
}

const ROW: quark_ui::element::GroupId = quark_ui::element::GroupId::new("row");

/// A row whose button only shows while the row is hovered.
struct RevealApp;

impl UiApp for RevealApp {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        div()
            .w(400.0)
            .h(40.0)
            .flex_row()
            .interaction_group(ROW)
            .child(div().w(300.0).h(40.0))
            .child(
                div()
                    .w(40.0)
                    .h(40.0)
                    .cursor(CursorHint::Pointer)
                    .on_click(Msg::Save)
                    .show_when(quark_ui::element::GroupCondition::HoveredOrFocusWithin(ROW)),
            )
            .into_any()
    }

    fn update(&mut self, Msg::Save: Msg, _cx: &mut UiContext) {}
}

// The hover that reveals the button paints the frame the button is in,
// so no later move changes what is hovered; the cursor must follow that
// frame.
#[test]
fn pointer_entering_a_revealed_action_shows_its_cursor() {
    let mut ui = UiTestHarness::new(RevealApp, (400.0, 300.0), 1.0);
    let main = ui.main_window();

    ui.pointer_move((320.0, 20.0));
    ui.pointer_move((321.0, 20.0));

    assert_eq!(ui.window(main).cursor(), CursorIcon::Pointer);
    ui.pointer_move((100.0, 200.0));
    assert_eq!(ui.window(main).cursor(), CursorIcon::Default);
}

/// A 40 point title bar that moves the window, with a Save button at
/// its start.
struct TitleBarApp {
    saved: u32,
}

impl UiApp for TitleBarApp {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        div()
            .w(400.0)
            .h(40.0)
            .flex_row()
            .window_drag_region()
            .child(
                div()
                    .w(80.0)
                    .h(40.0)
                    .accessibility_role(Role::Button)
                    .accessibility_label("Save")
                    .on_click(Msg::Save),
            )
            .into_any()
    }

    fn update(&mut self, Msg::Save: Msg, _cx: &mut UiContext) {
        self.saved += 1;
    }
}

// Catches app-drawn title chrome that does not move the window, a
// button in it that moves the window instead of clicking, and a
// double-click that only starts another drag.
#[test]
fn title_chrome_moves_the_window_and_double_clicks_but_its_buttons_click() {
    let mut ui = UiTestHarness::new(TitleBarApp { saved: 0 }, (400.0, 300.0), 1.0);
    let main = ui.main_window();
    let counts = |ui: &mut UiTestHarness<TitleBarApp>| {
        let window = ui.window(main);
        (window.window_drags(), window.title_double_clicks())
    };

    // Pressed and moved, as a drag is on every platform.
    let drag = |ui: &mut UiTestHarness<TitleBarApp>, at: (f32, f32)| {
        ui.pointer_down(at);
        ui.pointer_move((at.0 + 30.0, at.1 + 10.0));
        ui.pointer_up((at.0 + 30.0, at.1 + 10.0));
        ui.advance(1_000);
    };

    drag(&mut ui, (200.0, 20.0));
    assert_eq!(counts(&mut ui), (1, 0), "blank chrome moves the window");

    ui.click((40.0, 20.0));
    drag(&mut ui, (40.0, 20.0));
    assert_eq!(ui.app().saved, 2, "the button clicks");
    assert_eq!(
        counts(&mut ui),
        (1, 0),
        "the button does not move the window"
    );

    ui.click((200.0, 20.0));
    ui.advance(100);
    ui.click((201.0, 20.0));
    assert_eq!(counts(&mut ui).1, 1, "the second press is the title action");
    let drags = counts(&mut ui).0;

    ui.advance(1_000);
    drag(&mut ui, (200.0, 200.0));
    assert_eq!(
        counts(&mut ui),
        (drags, 1),
        "content below the chrome does nothing"
    );
}

/// A sidebar of a given width that asks for the sidebar material.
struct Panes {
    sidebar: f32,
}

impl UiApp for Panes {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        div()
            .w(400.0)
            .h(300.0)
            .flex_row()
            .child(
                div()
                    .w(self.sidebar)
                    .h(300.0)
                    .material(quark::scene::MaterialKind::Sidebar),
            )
            .into_any()
    }

    fn update(&mut self, _msg: Msg, _cx: &mut UiContext) {}
}

// Catches Div::material regions that never reach the window's native
// views, or keep their old rectangle after the layout moves.
#[test]
fn material_regions_reach_the_window_and_follow_the_layout() {
    let mut ui = UiTestHarness::new(Panes { sidebar: 200.0 }, (400.0, 300.0), 1.0);
    let main = ui.main_window();
    let regions = |ui: &mut UiTestHarness<Panes>| {
        ui.window(main)
            .material_regions()
            .iter()
            .map(|r| (r.rect, r.kind))
            .collect::<Vec<_>>()
    };
    let sidebar = |width| {
        vec![(
            Rect {
                x: 0.0,
                y: 0.0,
                width,
                height: 300.0,
            },
            quark::scene::MaterialKind::Sidebar,
        )]
    };

    ui.frame();
    assert_eq!(regions(&mut ui), sidebar(200.0));

    ui.app_mut().sidebar = 240.0;
    ui.frame();
    assert_eq!(regions(&mut ui), sidebar(240.0));
}

/// The published text field as assistive tech sees it, without its id.
fn field_state(ui: &UiTestHarness<ComposerApp>) -> String {
    quark_ui::accessibility::dump_accessibility_states(&ui.accessibility_update())
        .lines()
        .find(|line| line.contains("| TextInput |"))
        .and_then(|line| line.split_once(" | "))
        .map(|(_, rest)| rest.to_owned())
        .expect("text field in the tree")
}

#[test]
fn edits_publish_the_fields_text_caret_and_selection() {
    let mut ui = composer("");

    ui.click((10.0, 50.0));
    ui.type_text("hello");
    let typed = field_state(&ui);
    ui.key("mod+a");

    assert_eq!(typed, r#"TextInput | Message | text="hello" | caret=5"#);
    assert_eq!(
        field_state(&ui),
        r#"TextInput | Message | text="hello" | caret=5 | sel=0..5"#
    );
}

// AT-SPI's set_selection and set_caret_offset arrive as
// SetTextSelection; ReplaceSelectedText replaces only the selection.
#[test]
fn text_actions_from_assistive_tech_reach_the_field() {
    use accesskit::{
        Action as AxAction, ActionData, ActionRequest, TextPosition, TextSelection, TreeId,
    };
    let mut ui = composer("hello world");
    let update = ui.accessibility_update();
    let with_role = |role| {
        update
            .nodes
            .iter()
            .find(|(_, node)| node.role() == role)
            .map(|(id, _)| *id)
            .expect("node with role")
    };
    let (field, run) = (with_role(Role::TextInput), with_role(Role::TextRun));
    let request = |action, data| ActionRequest {
        action,
        target_tree: TreeId::ROOT,
        target_node: field,
        data: Some(data),
    };
    let at = |character_index| TextPosition {
        node: run,
        character_index,
    };

    ui.accessibility_action(request(
        AxAction::SetTextSelection,
        ActionData::SetTextSelection(TextSelection {
            anchor: at(6),
            focus: at(11),
        }),
    ));
    let field_text = ui.app().field.text().to_owned();
    let range = ui.app().field.selection_range().expect("selection");
    assert_eq!(&field_text[range.start.get()..range.end.get()], "world");

    ui.accessibility_action(request(
        AxAction::ReplaceSelectedText,
        ActionData::Value("there".into()),
    ));
    assert_eq!(ui.app().field.text(), "hello there");
}

/// Send an assistive tech Click to the first node with `role`.
fn accessibility_click<A: UiApp>(ui: &mut UiTestHarness<A>, role: Role) {
    use accesskit::{Action as AxAction, ActionRequest, TreeId};
    let node = ui
        .accessibility_update()
        .nodes
        .iter()
        .find(|(_, node)| node.role() == role)
        .map(|(id, _)| *id)
        .expect("node with role");
    ui.accessibility_action(ActionRequest {
        action: AxAction::Click,
        target_tree: TreeId::ROOT,
        target_node: node,
        data: None,
    });
}

// AT-SPI offers only Click as an action, so a tool driving the field
// through it must be able to focus the field that way.
#[test]
fn a_click_from_assistive_tech_focuses_the_field() {
    let mut ui = composer("");
    accessibility_click(&mut ui, Role::TextInput);
    ui.type_text("hi");
    assert_eq!(ui.app().field.text(), "hi");
}

/// One focusable Save button.
struct SaveApp {
    saved: usize,
}

const SAVE: FocusId = FocusId::from_key("save");

impl UiApp for SaveApp {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        div()
            .w(80.0)
            .h(30.0)
            .track_focus(SAVE)
            .accessibility_role(Role::Button)
            .accessibility_label("Save")
            .on_click(Msg::Save)
            .into_any()
    }

    fn update(&mut self, Msg::Save: Msg, _cx: &mut UiContext) {
        self.saved += 1;
    }
}

/// Names a box with a handle taken in `init`, and notes in each view
/// where the last completed frame put it.
struct GeometryApp {
    handle: Option<ElementHandle>,
    show: bool,
    seen: Vec<Option<Rect>>,
}

impl GeometryApp {
    fn harness() -> UiTestHarness<Self> {
        let app = Self {
            handle: None,
            show: true,
            seen: Vec::new(),
        };
        UiTestHarness::new(app, (200.0, 100.0), 1.0)
    }
}

impl UiApp for GeometryApp {
    type Action = Msg;
    type Message = ();

    fn init(&mut self, cx: &mut UiContext) {
        self.handle = Some(cx.new_element_handle());
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let handle = self.handle.expect("init ran");
        let seen = cx.geometry().by_handle(handle).ok().map(|g| g.bounds);
        self.seen.push(seen);
        let column = div().flex_col().child(div().h(40.0));
        let column = match self.show {
            true => column.child(div().element_handle(handle).w(50.0).h(20.0)),
            false => column,
        };
        column.into_any()
    }

    fn update(&mut self, _: Msg, _: &mut UiContext) {}
}

// Catches a view reading geometry from the frame it is still building,
// or from no frame: the first view finds nothing, the second finds
// where the first frame put the box.
#[test]
fn view_sees_where_the_last_completed_frame_put_an_element() {
    let mut ui = GeometryApp::harness();
    ui.frame();
    let placed = Rect {
        x: 0.0,
        y: 40.0,
        width: 50.0,
        height: 20.0,
    };
    assert_eq!(ui.app().seen, [None, Some(placed)]);
}

// Catches geometry that outlives the element it names.
#[test]
fn a_removed_element_has_no_geometry() {
    let mut ui = GeometryApp::harness();
    ui.app_mut().show = false;
    ui.frame();
    let handle = ui.app().handle.expect("init ran");
    assert_eq!(
        ui.geometry().by_handle(handle),
        Err(quark_ui::element::LookupError::Missing)
    );
}

// A click from assistive tech moves focus with it, as a pointer click
// does, so keys that follow reach the control (a radio's arrows).
#[test]
fn a_click_from_assistive_tech_focuses_its_control() {
    let mut ui = UiTestHarness::new(SaveApp { saved: 0 }, (200.0, 100.0), 1.0);
    accessibility_click(&mut ui, Role::Button);
    assert_eq!(ui.app().saved, 1);
    assert_eq!(ui.focus(), Some(SAVE));
}

/// Two buttons side by side, each ringing its own focus.
struct Pair;

const LEFT: FocusId = FocusId::from_key("left");
const RIGHT: FocusId = FocusId::from_key("right");

impl UiApp for Pair {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
        let button = |focus| div().w(60.0).h(30.0).focus_ring(focus).on_click(Msg::Save);
        div()
            .flex_row()
            .gap(20.0)
            .p(10.0)
            .child(button(LEFT))
            .child(button(RIGHT))
            .into_any()
    }

    fn update(&mut self, _: Msg, _: &mut UiContext) {}
}

/// Focus rings painted in the window's last frame: the app draws no
/// other border.
fn rings<A: UiApp>(ui: &UiTestHarness<A>) -> usize {
    ui.scene()
        .primitives
        .iter()
        .filter(|p| matches!(p, quark::scene::Primitive::Border(_)))
        .count()
}

// Regression: a clicked control (or a dragged split divider) kept a
// ring, which `:focus-visible` shows only for keyboard focus.
#[test]
fn a_click_focuses_without_a_ring() {
    let mut ui = UiTestHarness::new(Pair, (200.0, 60.0), 1.0);
    ui.click((40.0, 25.0));
    ui.frame();
    assert_eq!((ui.focus(), rings(&ui)), (Some(LEFT), 0));
}

// Tab after a click shows focus again where it lands.
#[test]
fn tab_after_a_click_rings_the_next_control() {
    let mut ui = UiTestHarness::new(Pair, (200.0, 60.0), 1.0);
    ui.click((40.0, 25.0));
    ui.key("tab");
    ui.frame();
    assert_eq!((ui.focus(), rings(&ui)), (Some(RIGHT), 1));
}

/// Two 50-point rows with tooltips "a" and "b", below `scrolled` points
/// of space without one.
struct TooltipApp {
    scrolled: f32,
    seen: Option<HoveredTooltip>,
}

impl UiApp for TooltipApp {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        self.seen = cx.hovered_tooltip().cloned();
        div()
            .flex_col()
            .child(div().w(100.0).h(self.scrolled))
            .child(div().w(100.0).h(50.0).tooltip("a"))
            .child(div().w(100.0).h(50.0).tooltip("b"))
            .into_any()
    }

    fn update(&mut self, _action: Msg, _cx: &mut UiContext) {}
}

#[test]
fn views_see_the_hovered_tooltip_and_redraw_only_when_it_changes() {
    let app = TooltipApp {
        scrolled: 0.0,
        seen: None,
    };
    let mut ui = UiTestHarness::new(app, (400.0, 300.0), 1.0);
    ui.frame();
    let seen = |ui: &UiTestHarness<TooltipApp>| ui.app().seen.as_ref().map(|t| t.text.to_string());

    ui.advance(100);
    ui.pointer_move((10.0, 10.0));
    let hovered = ui.app().seen.clone().expect("tooltip a");
    assert_eq!((&*hovered.text, hovered.pointer), ("a", (10.0, 10.0)));
    assert_eq!(hovered.since, ui.now());

    let frames = ui.frame_count();
    ui.pointer_move((20.0, 20.0));
    assert_eq!(
        ui.frame_count(),
        frames,
        "moving within a region redraws nothing"
    );

    ui.pointer_move((10.0, 60.0));
    assert_eq!(seen(&ui).as_deref(), Some("b"));

    ui.pointer_down((10.0, 60.0));
    ui.pointer_up((10.0, 60.0));
    ui.pointer_move((12.0, 62.0));
    assert_eq!(seen(&ui), None, "a press hides it until the pointer leaves");

    ui.pointer_move((10.0, 10.0));
    assert_eq!(seen(&ui).as_deref(), Some("a"));

    ui.pointer_leave();
    assert_eq!(seen(&ui), None);

    // Scrolling moves "a" out from under a still pointer.
    ui.pointer_move((10.0, 10.0));
    ui.app_mut().scrolled = 50.0;
    ui.frame();
    ui.advance(16);
    assert_eq!(seen(&ui), None);
}
