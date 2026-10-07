//! Lite adapter from quark-ui elements to the runner.
//!
//! Implement [`UiApp`] to describe the window as an element tree each frame
//! and handle your action type; [`run_ui`] (or [`UiAdapter`] with [`run`])
//! does layout, paint, hit testing, focus, and accessibility publishing.
//!
//! The element tree is laid out in logical points, like the runner's scenes
//! and pointer events; text is shaped at the window's scale factor so it
//! rasterizes at physical size. Accessibility bounds stay logical under a
//! root transform that scales them to the physical pixels accesskit expects.

use std::any::Any;
use std::time::{Duration, Instant};

use accesskit::{Action as AxAction, ActionData, ActionRequest, Affine, TreeUpdate};
use quark::Rect;
use quark::SemanticFrame;
use quark::reactive::SignalStore;
use quark::scene::Scene;
use quark_ui::accessibility::{AccessibilityAction, AccessibilityFrame};
use quark_ui::animation::AnimationTable;
use quark_ui::element::{
    AnyElement, CursorHint, Delivery, ElementContext, InputRouter, TextInputHitArea, render_element,
};
use quark_ui::text_input::{TextEditCommand, TextPointer, TextPointerEvent};
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};
use winit::event::{ElementState, MouseButton, MouseScrollDelta};
use winit::keyboard::NamedKey;
use winit::window::CursorIcon;

use crate::{App, EventContext, FrameContext, InputEvent, RunError, WindowOptions, run};

/// Lines scrolled per accessibility ScrollUp/ScrollDown request.
const ACCESSIBILITY_SCROLL_LINES: i32 = 3;
/// Points per line when a trackpad reports pixel deltas.
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

    /// Apply `command` to the model behind the text field `target`
    /// (`TextField::apply` or `Editor::apply`). The adapter sends pointer
    /// selection (click, drag, double and triple click, Shift-click, and
    /// autoscroll past an edge) and `CancelPreedit` when focus leaves a
    /// field mid-composition. It may be called while a frame is built, so
    /// it gets no context; the adapter redraws afterwards.
    fn edit_text(&mut self, _target: FocusId, _command: TextEditCommand) {}

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
    animations: &'a mut AnimationTable,
}

impl ViewContext<'_, '_> {
    /// The window's animation table, ticked to this frame's clock. Rows
    /// still moving after paint schedule the next frame.
    pub fn animations(&mut self) -> &mut AnimationTable {
        self.animations
    }

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
    /// Text fields of the last frame, for pointer selection and IME.
    text_areas: Vec<TextInputHitArea>,
    text_pointer: TextPointer,
    /// Clock for multi-click and autoscroll timing.
    epoch: Instant,
    /// The focused text field as of the last input, to cancel its
    /// composition once focus leaves it.
    edit_focus: Option<FocusId>,
    /// IME state last sent to the window.
    ime_allowed: bool,
    ime_area: Option<Rect>,
    /// Scale factor of the last painted frame, for accessibility bounds.
    scale_factor: f32,
    animations: AnimationTable,
    #[cfg(feature = "devtools")]
    devtools: quark_ui::inspector::Devtools,
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
            text_areas: Vec::new(),
            text_pointer: TextPointer::default(),
            epoch: Instant::now(),
            edit_focus: None,
            ime_allowed: false,
            ime_area: None,
            scale_factor: 1.0,
            animations: AnimationTable::new(),
            #[cfg(feature = "devtools")]
            devtools: quark_ui::inspector::Devtools::from_env(),
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
            if let Some(event) = action.downcast_ref::<TextPointerEvent>() {
                let extend = cx.modifiers().shift_key();
                let now_ms = self.epoch.elapsed().as_millis() as u64;
                if let Some((target, command)) =
                    self.text_pointer
                        .event(*event, &self.text_areas, now_ms, extend)
                {
                    self.app.edit_text(target, command);
                }
                continue;
            }
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

    /// IME follows focus: allowed while a text field has focus, with the
    /// candidate window at the caret `areas` (this frame's fields) painted.
    fn ime_request(&mut self, areas: &[TextInputHitArea]) -> ImeRequest {
        let target = self
            .focus
            .and_then(|focus| areas.iter().find(|area| area.focus_target == focus));
        let mut request = ImeRequest::default();
        let allowed = target.is_some();
        if allowed != self.ime_allowed {
            request.allowed = Some(allowed);
            self.ime_allowed = allowed;
            self.ime_area = None;
        }
        if let Some(caret) = target.and_then(|area| area.caret)
            && self.ime_area != Some(caret)
        {
            request.cursor_area = Some(caret);
            self.ime_area = Some(caret);
        }
        request
    }

    /// Cancel the composition of a text field focus left since the last
    /// input, or of the focused one when the window lost focus. winit
    /// reports no `Ime::Disabled` for either, so a preedit would otherwise
    /// linger. Returns whether a field was told.
    fn cancel_stale_preedit(&mut self, window_blurred: bool) -> bool {
        let current = self
            .focus
            .filter(|focus| self.text_areas.iter().any(|a| a.focus_target == *focus));
        let stale = self
            .edit_focus
            .filter(|previous| window_blurred || Some(*previous) != current);
        self.edit_focus = current;
        if let Some(target) = stale {
            self.app.edit_text(target, TextEditCommand::CancelPreedit);
        }
        stale.is_some()
    }

    fn after_input(&mut self, window_blurred: bool, cx: &mut EventContext) {
        if self.cancel_stale_preedit(window_blurred) {
            cx.request_redraw();
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

/// IME changes for the window, applied once the frame is built.
#[derive(Debug, Default, PartialEq)]
struct ImeRequest {
    allowed: Option<bool>,
    cursor_area: Option<Rect>,
}

/// One painted frame, with the scene still in logical points.
struct Painted {
    scene: Scene,
    input: quark_ui::element::InputFrame,
    accessibility: AccessibilityFrame,
    next_frame_ms: Option<u64>,
    text_areas: Vec<TextInputHitArea>,
}

/// Lay out and paint `root` into a `width` x `height` point viewport.
fn paint(root: &mut AnyElement, ecx: &mut ElementContext, width: f32, height: f32) -> Painted {
    let mut scene = Scene::default();
    ecx.accessibility = AccessibilityFrame::new(width, height);
    ecx.semantic = SemanticFrame::new(width, height);
    render_element(root, &mut scene, ecx, width, height);
    ecx.finish_frame();
    Painted {
        scene,
        input: ecx.take_input_frame(),
        accessibility: std::mem::take(&mut ecx.accessibility),
        next_frame_ms: ecx.next_frame_ms(),
        text_areas: std::mem::take(&mut ecx.text_input_hit_areas),
    }
}

/// accesskit wants bounds in physical pixels once every transform is
/// applied. The tree is built in points, so one scale on the root converts
/// the whole tree.
fn scale_tree(update: &mut TreeUpdate, scale: f32) {
    let Some(root) = update.tree.as_ref().map(|tree| tree.root) else {
        return;
    };
    // accesskit asks for no transform rather than an identity one.
    if scale == 1.0 {
        return;
    }
    if let Some((_, node)) = update.nodes.iter_mut().find(|(id, _)| *id == root) {
        node.set_transform(Affine::scale(f64::from(scale)));
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
        self.after_input(false, cx);
    }

    fn frame(&mut self, cx: &mut FrameContext) -> Scene {
        let (width, height) = cx.size();
        let scale = cx.scale_factor();
        let clock_ms = cx.elapsed().as_millis() as u64;
        // A pointer held past a text field's edge keeps selecting (and so
        // scrolling) without moving; apply the step before this frame's view.
        let now_ms = self.epoch.elapsed().as_millis() as u64;
        if let Some((target, command)) = self.text_pointer.autoscroll(&self.text_areas, now_ms) {
            self.app.edit_text(target, command);
        }
        self.animations.tick(clock_ms);
        #[cfg(feature = "devtools")]
        let view_started = std::time::Instant::now();
        let mut root = self.app.view(&mut ViewContext {
            frame: cx,
            theme: &self.theme,
            focus: self.focus,
            animations: &mut self.animations,
        });
        #[cfg(feature = "devtools")]
        let build_us = view_started.elapsed().as_micros() as u64;

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
        .with_clock(clock_ms)
        .with_animations(&mut self.animations);
        #[cfg(feature = "devtools")]
        self.devtools.begin_frame(&mut ecx.devtools);
        let painted = paint(&mut root, &mut ecx, width, height);
        #[cfg(feature = "devtools")]
        let phases = self.devtools.end_frame(&mut ecx.devtools);
        self.scale_factor = scale;
        if let Some(at_ms) = painted.next_frame_ms {
            cx.request_frame_in(Duration::from_millis(at_ms.saturating_sub(clock_ms)));
        }
        let (scene, ime) = self.finish_frame(painted);
        if let Some(allowed) = ime.allowed {
            cx.set_ime_allowed(allowed);
        }
        if let Some(caret) = ime.cursor_area {
            cx.set_ime_cursor_area(caret.x, caret.y, caret.width, caret.height);
        }
        if let Some(at_ms) = self.text_pointer.next_autoscroll_ms(&self.text_areas) {
            cx.request_frame_in(Duration::from_millis(at_ms.saturating_sub(now_ms)));
        }
        #[cfg(feature = "devtools")]
        let frame = crate::devtools::PaintedFrame {
            router: &self.router,
            theme: &self.theme,
            signals: &self.signals,
            build_us,
            phases,
        };
        #[cfg(feature = "devtools")]
        let scene = crate::devtools::finish_frame(&mut self.devtools, scene, cx, frame);
        scene
    }

    fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
        let window_blurred = matches!(event, InputEvent::Focused(false));
        self.handle_event(event, cx);
        self.after_input(window_blurred, cx);
    }

    fn wake(&mut self, cx: &mut EventContext) {
        let mut ucx = UiContext {
            window: cx,
            focus: &mut self.focus,
        };
        self.app.wake(&mut ucx);
        self.after_input(false, cx);
    }

    fn accessibility(&mut self) -> Option<TreeUpdate> {
        // A full tree every frame; accesskit diffs it against the last one.
        let mut update = self.accessibility.tree_update(&self.name, self.focus);
        scale_tree(&mut update, self.scale_factor);
        Some(update)
    }

    fn accessibility_action(&mut self, request: ActionRequest, cx: &mut EventContext) {
        self.handle_accessibility_action(request, cx);
        self.after_input(false, cx);
    }
}

impl<U: UiApp> UiAdapter<U> {
    /// Keep `painted`'s input state for routing until the next frame, and
    /// return its scene with the IME changes for the caret it painted.
    fn finish_frame(&mut self, painted: Painted) -> (Scene, ImeRequest) {
        self.router.set_frame(painted.input);
        self.accessibility = painted.accessibility;
        let ime = self.ime_request(&painted.text_areas);
        self.text_areas = painted.text_areas;
        (painted.scene, ime)
    }

    fn handle_event(&mut self, event: InputEvent, cx: &mut EventContext) {
        #[cfg(feature = "devtools")]
        if crate::devtools::intercept(&mut self.devtools, &event, cx) {
            return;
        }
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
    use quark::scene::ShapedText;
    use quark_ui::element::{IntoAnyElement, ScrollActionBuilder, div, text_input};
    use quark_ui::style::Styled;
    use quark_ui::text_input::TextField;

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
            paint(&mut root, &mut ecx, 400.0, 300.0)
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
            paint(&mut root, &mut ecx, 400.0, 300.0)
        }
    }

    /// An app with one text field that applies the adapter's edits to it.
    struct FieldApp {
        field: TextField,
    }

    impl UiApp for FieldApp {
        type Action = Msg;

        fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
            div().into_any()
        }

        fn update(&mut self, _action: Msg, _cx: &mut UiContext) {}

        fn edit_text(&mut self, target: FocusId, command: TextEditCommand) {
            if target == FIELD {
                self.field.apply(command);
            }
        }
    }

    fn field_adapter(text: &str) -> UiAdapter<FieldApp> {
        let mut adapter = UiAdapter::new(
            FieldApp {
                field: TextField::new(text),
            },
            "Test",
        );
        adapter.focus = Some(FIELD);
        adapter
    }

    // Regression: the IME candidate window was placed during event
    // handling with the previous frame's caret, so it trailed by a frame.
    #[test]
    fn ime_area_requested_with_a_frame_is_that_frames_caret() {
        let mut fixture = Fixture::new();
        let mut adapter = field_adapter("hello world");

        let frames = [0, 11].map(|cursor| {
            adapter
                .app
                .field
                .apply(TextEditCommand::SetTextCursor(cursor));
            let painted = fixture.paint_field(&adapter.app.field);
            let caret = painted.text_areas[0].caret.expect("focused caret");
            let (_, ime) = adapter.finish_frame(painted);
            (caret, ime.cursor_area)
        });

        assert_ne!(frames[0].0, frames[1].0, "the caret moved");
        for (caret, requested) in frames {
            assert_eq!(requested, Some(caret));
        }
    }

    // Regression: winit sends no Ime::Disabled when focus leaves a field
    // or the window, so the field kept painting a stale composition.
    #[test]
    fn leaving_a_composing_field_cancels_its_preedit() {
        let cases = [
            ("focus stays", Some(FIELD), false, Some("にほ")),
            ("focus moves away", None, false, None),
            ("window loses focus", Some(FIELD), true, None),
        ];
        for (name, focus, window_blurred, expected) in cases {
            let mut fixture = Fixture::new();
            let mut adapter = field_adapter("");
            let painted = fixture.paint_field(&adapter.app.field);
            adapter.finish_frame(painted);
            adapter.cancel_stale_preedit(false);
            adapter.app.field.set_preedit("にほ", None);

            adapter.focus = focus;
            adapter.cancel_stale_preedit(window_blurred);

            let preedit = adapter.app.field.preedit().map(|p| p.text.as_str());
            assert_eq!(preedit, expected, "{name}");
        }
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
        assert_eq!(layout.glyphs().font_size[0], logical_size * 2.0);
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
            after.glyphs().font_size[0],
            before.glyphs().font_size[0] * 2.0
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
