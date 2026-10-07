//! Lite adapter from quark-ui elements to the runner.
//!
//! Implement [`UiApp`] to describe the window as an element tree each frame
//! and handle your action type; [`run_ui`] (or [`UiAdapter`] with [`run`])
//! does layout, paint, hit testing, focus, and accessibility publishing.
//!
//! The element tree is laid out in physical pixels, matching the runner's
//! scene coordinates.

use std::any::Any;

use accesskit::{Action as AxAction, ActionData, ActionRequest, TreeUpdate};
use quark::Rect;
use quark::SemanticFrame;
use quark::reactive::SignalStore;
use quark::scene::Scene;
use quark_ui::accessibility::{AccessibilityAction, AccessibilityFrame};
use quark_ui::element::{
    AnyElement, CursorHint, Delivery, ElementContext, InputRouter, render_element,
};
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};
use winit::event::{ElementState, MouseButton, MouseScrollDelta};
use winit::keyboard::NamedKey;
use winit::window::CursorIcon;

use crate::{App, EventContext, FrameContext, InputEvent, RunError, WindowOptions, run};

/// Lines scrolled per accessibility ScrollUp/ScrollDown request.
const ACCESSIBILITY_SCROLL_LINES: i32 = 3;
/// Pixels per line when a trackpad reports pixel deltas.
const PIXELS_PER_LINE: f32 = 20.0;

/// An app described as quark-ui elements.
///
/// Elements carry actions as type-erased [`Action`]s; the adapter downcasts
/// each to [`UiApp::Action`] and passes it to [`UiApp::update`]. Actions of
/// other types (such as `NoopAction`) are dropped.
pub trait UiApp: 'static {
    type Action: Any + Clone;

    /// Called once after the window and renderer exist.
    fn init(&mut self, _cx: &mut UiContext) {}

    /// Build the element tree for the next frame.
    fn view(&mut self, cx: &mut ViewContext) -> AnyElement;

    /// Handle an action emitted by a click, scroll, drag, or assistive tech.
    fn update(&mut self, action: Self::Action, cx: &mut UiContext);

    /// Assistive tech set the value of the text field `target`.
    fn set_text_value(&mut self, _target: FocusId, _value: String, _cx: &mut UiContext) {}

    /// Sees every input event first. Return `true` to stop the adapter's own
    /// handling (clicks, wheel, Tab focus traversal).
    fn event(&mut self, _event: &InputEvent, _cx: &mut UiContext) -> bool {
        false
    }

    /// Called after any [`crate::Waker::wake`].
    fn wake(&mut self, _cx: &mut UiContext) {}
}

/// What [`UiApp::view`] can see while building a frame.
pub struct ViewContext<'a, 'f> {
    pub frame: &'a mut FrameContext<'f>,
    pub theme: &'a Theme,
    focus: Option<FocusId>,
}

impl ViewContext<'_, '_> {
    pub fn focus(&self) -> Option<FocusId> {
        self.focus
    }

    pub fn is_focused(&self, target: FocusId) -> bool {
        self.focus == Some(target)
    }
}

/// An [`EventContext`] plus the adapter's focus.
pub struct UiContext<'a, 'w> {
    pub window: &'a mut EventContext<'w>,
    focus: &'a mut Option<FocusId>,
}

impl UiContext<'_, '_> {
    pub fn focus(&self) -> Option<FocusId> {
        *self.focus
    }

    pub fn set_focus(&mut self, focus: Option<FocusId>) {
        if *self.focus != focus {
            *self.focus = focus;
            self.window.request_redraw();
        }
    }
}

/// Runs a [`UiApp`] as an [`App`].
pub struct UiAdapter<U: UiApp> {
    app: U,
    name: String,
    theme: Theme,
    signals: SignalStore,
    focus: Option<FocusId>,
    pointer: Option<(f32, f32)>,
    /// Routes input through the last painted frame until the next replaces it.
    router: InputRouter,
    accessibility: AccessibilityFrame,
    /// Text fields of the last frame with their caret rects, for IME.
    text_targets: Vec<(FocusId, Option<Rect>)>,
    ime_allowed: bool,
    ime_area: Option<Rect>,
}

/// Open a window titled by `options` and run `app` in it.
pub fn run_ui<U: UiApp>(app: U, options: WindowOptions) -> Result<(), RunError> {
    let adapter = UiAdapter::new(app, options.title.clone());
    run(adapter, options)
}

impl<U: UiApp> UiAdapter<U> {
    /// `name` labels the accessibility tree's window node.
    pub fn new(app: U, name: impl Into<String>) -> Self {
        Self {
            app,
            name: name.into(),
            theme: Theme::default_dark(),
            signals: SignalStore::new(),
            focus: None,
            pointer: None,
            router: InputRouter::default(),
            accessibility: AccessibilityFrame::default(),
            text_targets: Vec::new(),
            ime_allowed: false,
            ime_area: None,
        }
    }

    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self
    }

    pub fn app(&self) -> &U {
        &self.app
    }

    fn dispatch(&mut self, actions: Vec<Action>, cx: &mut EventContext) {
        for action in actions {
            let Some(action) = action.downcast_ref::<U::Action>().cloned() else {
                tracing::debug!("ignoring action of another type: {action:?}");
                continue;
            };
            let mut ucx = UiContext {
                window: cx,
                focus: &mut self.focus,
            };
            self.app.update(action, &mut ucx);
        }
        cx.request_redraw();
    }

    fn deliver(&mut self, delivery: Delivery, cx: &mut EventContext) {
        if delivery.actions.is_empty() {
            if delivery.node.is_some() {
                cx.request_redraw();
            }
        } else {
            self.dispatch(delivery.actions, cx);
        }
    }

    fn pointer_pressed(&mut self, cx: &mut EventContext) {
        let Some((x, y)) = self.pointer else {
            return;
        };
        let before = self.focus;
        let delivery = self.router.pointer_down(x, y, &mut self.focus);
        if self.focus != before {
            cx.request_redraw();
        }
        self.deliver(delivery, cx);
    }

    fn wheel(&mut self, delta: MouseScrollDelta, cx: &mut EventContext) {
        let Some((x, y)) = self.pointer else {
            return;
        };
        // winit reports positive y for scrolling up; scroll builders take
        // positive lines as moving down.
        let lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => -y.round() as i32,
            MouseScrollDelta::PixelDelta(position) => {
                -(position.y as f32 / PIXELS_PER_LINE).round() as i32
            }
        };
        if lines == 0 {
            return;
        }
        let delivery = self.router.wheel(x, y, lines);
        self.deliver(delivery, cx);
    }

    /// Allow IME only while a text field has focus, and point the
    /// candidate window at its caret. Frames cannot reach the window, so
    /// this runs after events with the last frame's caret.
    fn sync_ime(&mut self, cx: &mut EventContext) {
        let target = self
            .focus
            .and_then(|focus| self.text_targets.iter().find(|(t, _)| *t == focus));
        let allowed = target.is_some();
        if allowed != self.ime_allowed {
            cx.set_ime_allowed(allowed);
            self.ime_allowed = allowed;
            self.ime_area = None;
        }
        if let Some(caret) = target.and_then(|(_, caret)| *caret)
            && self.ime_area != Some(caret)
        {
            cx.set_ime_cursor_area(caret.x, caret.y, caret.width, caret.height);
            self.ime_area = Some(caret);
        }
    }

    fn update_cursor(&self, cx: &mut EventContext) {
        let hint = self
            .pointer
            .map(|(x, y)| self.router.cursor_at(x, y))
            .unwrap_or_default();
        cx.set_cursor(match hint {
            CursorHint::Default => CursorIcon::Default,
            CursorHint::Pointer => CursorIcon::Pointer,
            CursorHint::Text => CursorIcon::Text,
            CursorHint::ResizeCol => CursorIcon::ColResize,
            CursorHint::ResizeRow => CursorIcon::RowResize,
            CursorHint::ResizeNs => CursorIcon::NsResize,
            CursorHint::ResizeEw => CursorIcon::EwResize,
            CursorHint::ResizeNesw => CursorIcon::NeswResize,
            CursorHint::ResizeNwse => CursorIcon::NwseResize,
            CursorHint::Move => CursorIcon::Move,
            CursorHint::Grab => CursorIcon::Grab,
            CursorHint::Grabbing => CursorIcon::Grabbing,
            CursorHint::NotAllowed => CursorIcon::NotAllowed,
            CursorHint::Wait => CursorIcon::Wait,
            CursorHint::Progress => CursorIcon::Progress,
            CursorHint::Crosshair => CursorIcon::Crosshair,
            CursorHint::Help => CursorIcon::Help,
        });
    }
}

/// What an accessibility request asks the app to do.
#[derive(Debug)]
enum Routed {
    Dispatch(Action),
    Focus(FocusId),
    SetValue(FocusId, String),
}

fn route_accessibility(frame: &AccessibilityFrame, request: &ActionRequest) -> Option<Routed> {
    let target = frame.action_for(request.target_node)?;
    let scroll_lines = match request.action {
        AxAction::ScrollUp => -ACCESSIBILITY_SCROLL_LINES,
        _ => ACCESSIBILITY_SCROLL_LINES,
    };
    match (request.action, target) {
        (AxAction::Click, AccessibilityAction::Click(action)) => {
            Some(Routed::Dispatch(action.clone()))
        }
        (
            AxAction::Focus,
            AccessibilityAction::Focus(focus)
            | AccessibilityAction::TextValue(focus)
            | AccessibilityAction::EditorViewport { focus, .. },
        ) => Some(Routed::Focus(*focus)),
        (
            AxAction::SetValue | AxAction::ReplaceSelectedText,
            AccessibilityAction::TextValue(focus),
        ) => match &request.data {
            Some(ActionData::Value(value)) => Some(Routed::SetValue(*focus, value.to_string())),
            _ => None,
        },
        (
            AxAction::ScrollUp | AxAction::ScrollDown,
            AccessibilityAction::Scroll(scroll)
            | AccessibilityAction::EditorViewport { scroll, .. },
        ) => Some(Routed::Dispatch(scroll.build(scroll_lines))),
        _ => None,
    }
}

impl<U: UiApp> App for UiAdapter<U> {
    fn init(&mut self, cx: &mut EventContext) {
        let mut ucx = UiContext {
            window: cx,
            focus: &mut self.focus,
        };
        self.app.init(&mut ucx);
    }

    fn frame(&mut self, cx: &mut FrameContext) -> Scene {
        let (width, height) = cx.size();
        let scale = cx.scale_factor();
        let clock_ms = cx.elapsed().as_millis() as u64;
        let mut root = self.app.view(&mut ViewContext {
            frame: cx,
            theme: &self.theme,
            focus: self.focus,
        });

        let mut scene = Scene::default();
        let text = cx.text();
        let mut ecx = ElementContext::new(
            &self.theme,
            scale,
            &mut text.system,
            &mut text.layouts,
            self.pointer,
            &self.signals,
        )
        .with_focus(self.focus)
        .with_clock(clock_ms);
        ecx.accessibility = AccessibilityFrame::new(width, height);
        ecx.semantic = SemanticFrame::new(width, height);
        render_element(&mut root, &mut scene, &mut ecx, width, height);

        self.router.set_frame(ecx.take_input_frame());
        self.accessibility = std::mem::take(&mut ecx.accessibility);
        self.text_targets = ecx
            .text_input_hit_areas
            .iter()
            .map(|area| (area.focus_target, area.caret))
            .collect();
        scene
    }

    fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
        self.handle_event(event, cx);
        self.sync_ime(cx);
    }

    fn wake(&mut self, cx: &mut EventContext) {
        let mut ucx = UiContext {
            window: cx,
            focus: &mut self.focus,
        };
        self.app.wake(&mut ucx);
        self.sync_ime(cx);
    }

    fn accessibility(&mut self) -> Option<TreeUpdate> {
        // A full tree every frame; accesskit diffs it against the last one.
        Some(self.accessibility.tree_update(&self.name, self.focus))
    }

    fn accessibility_action(&mut self, request: ActionRequest, cx: &mut EventContext) {
        self.handle_accessibility_action(request, cx);
        self.sync_ime(cx);
    }
}

impl<U: UiApp> UiAdapter<U> {
    fn handle_event(&mut self, event: InputEvent, cx: &mut EventContext) {
        let mut ucx = UiContext {
            window: cx,
            focus: &mut self.focus,
        };
        if self.app.event(&event, &mut ucx) {
            return;
        }
        match event {
            InputEvent::PointerMoved { x, y } => {
                self.pointer = Some((x, y));
                let delivery = self.router.pointer_move(x, y);
                self.deliver(delivery, cx);
                self.update_cursor(cx);
                // Hover styles depend on the pointer.
                cx.request_redraw();
            }
            InputEvent::PointerLeft => {
                self.pointer = None;
                cx.request_redraw();
            }
            InputEvent::PointerButton {
                button: MouseButton::Left,
                state: ElementState::Pressed,
            } => self.pointer_pressed(cx),
            InputEvent::PointerButton {
                button: MouseButton::Left,
                state: ElementState::Released,
            } => {
                let delivery = self.router.pointer_up();
                self.deliver(delivery, cx);
            }
            InputEvent::Wheel { delta, .. } => self.wheel(delta, cx),
            InputEvent::KeyPress(chord) if chord.named() == Some(NamedKey::Tab) => {
                let next = self.router.traverse_focus(self.focus, chord.shift());
                if next != self.focus {
                    self.focus = next;
                    cx.request_redraw();
                }
            }
            InputEvent::KeyPress(chord) => {
                if let Some(binding) = chord.binding_string() {
                    let delivery = self.router.key_down(&binding, self.focus);
                    self.deliver(delivery, cx);
                }
            }
            _ => {}
        }
    }

    fn handle_accessibility_action(&mut self, request: ActionRequest, cx: &mut EventContext) {
        match route_accessibility(&self.accessibility, &request) {
            Some(Routed::Dispatch(action)) => self.dispatch(vec![action], cx),
            Some(Routed::Focus(focus)) => {
                self.focus = Some(focus);
                cx.request_redraw();
            }
            Some(Routed::SetValue(target, value)) => {
                let mut ucx = UiContext {
                    window: cx,
                    focus: &mut self.focus,
                };
                self.app.set_text_value(target, value, &mut ucx);
                cx.request_redraw();
            }
            None => tracing::debug!("unhandled accessibility request: {request:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use accesskit::{NodeId, Role, TreeId};
    use quark_ui::element::{IntoAnyElement, ScrollActionBuilder, div, text_input};
    use quark_ui::style::Styled;

    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Save,
        Scroll(i32),
    }

    impl From<Msg> for Action {
        fn from(msg: Msg) -> Self {
            Action::new(msg)
        }
    }

    const FIELD: FocusId = FocusId::from_key("field");

    fn painted_frame() -> AccessibilityFrame {
        let mut text = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        let mut root = div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .accessibility_id("list")
            .accessibility_role(Role::List)
            .on_scroll(ScrollActionBuilder::new(|lines| Msg::Scroll(lines).into()))
            .child(
                div()
                    .w(80.0)
                    .h(30.0)
                    .accessibility_id("save")
                    .accessibility_role(Role::Button)
                    .accessibility_label("Save")
                    .on_click(Msg::Save),
            )
            .child(text_input("Name", "").focus_target(FIELD).w(200.0).h(40.0))
            .into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, 400.0, 300.0);
        cx.accessibility
    }

    fn node_with_role(update: &TreeUpdate, role: Role) -> NodeId {
        update
            .nodes
            .iter()
            .find(|(_, node)| node.role() == role)
            .map(|(id, _)| *id)
            .expect("node with role")
    }

    #[test]
    fn route_accessibility_maps_requests_to_app_commands() {
        let frame = painted_frame();
        let update = frame.tree_update("Test", None);
        let button = node_with_role(&update, Role::Button);
        let field = node_with_role(&update, Role::TextInput);
        let list = node_with_role(&update, Role::List);
        let value = Some(ActionData::Value("Ada".into()));
        let cases = [
            (
                AxAction::Click,
                button,
                None,
                "Some(Dispatch(Save))".to_owned(),
            ),
            (
                AxAction::Focus,
                field,
                None,
                format!("Some(Focus(FocusId({})))", FIELD.0),
            ),
            (
                AxAction::SetValue,
                field,
                value,
                format!("Some(SetValue(FocusId({}), \"Ada\"))", FIELD.0),
            ),
            (
                AxAction::ScrollDown,
                list,
                None,
                "Some(Dispatch(Scroll(3)))".to_owned(),
            ),
            (
                AxAction::ScrollUp,
                list,
                None,
                "Some(Dispatch(Scroll(-3)))".to_owned(),
            ),
            (AxAction::Click, field, None, "None".to_owned()),
        ];
        for (action, target_node, data, expected) in cases {
            let request = ActionRequest {
                action,
                target_tree: TreeId::ROOT,
                target_node,
                data,
            };
            let routed = format!("{:?}", route_accessibility(&frame, &request));
            assert_eq!(routed, expected, "{action:?}");
        }
    }
}
