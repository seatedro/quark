//! Lite adapter from quark-ui elements to the runner.
//!
//! Implement [`UiApp`] to describe the window as an element tree each frame
//! and handle your action type; [`run_ui`] (or [`UiAdapter`] with [`run`])
//! does layout, paint, hit testing, focus, text editing, and accessibility
//! publishing.
//!
//! The element tree is laid out in logical points, like the runner's scenes
//! and pointer events; text is shaped at the window's scale factor so it
//! rasterizes at physical size. Accessibility bounds stay logical under a
//! root transform that scales them to the physical pixels accesskit expects.
//!
//! The adapter redraws only for a reason it can name: the
//! hovered elements changed, focus moved, an action was dispatched, a text
//! field changed, or the theme changed. Animations schedule their own
//! frames, and an app that changes state elsewhere asks with
//! `cx.window.request_redraw()`.

use std::any::Any;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use accesskit::{Action as AxAction, ActionData, ActionRequest, Affine, TreeUpdate};
use quark::Rect;
#[cfg(feature = "test-support")]
use quark::SemanticFrame;
use quark::hit::HitId;
use quark::reactive::SignalStore;
use quark::scene::Scene;
use quark_ui::accessibility::{AccessibilityAction, AccessibilityFrame, Announcer, Politeness};
use quark_ui::animation::AnimationTable;
use quark_ui::element::{
    AnyElement, Binding, CursorHint, Delivery, ElementCache, ElementContext, ElementHandle,
    ElementHandles, InputRouter, LayoutSnapshot, Mods, ScrollbarTrack, TextInputHitArea,
    TooltipRegion, WheelEvent, render_element,
};
use quark_ui::key_context::{KeyBindings, context_path};
use quark_ui::text_input::{
    TextEditCommand, TextEditOutcome, TextPointer, TextPointerEvent, command_for_binding,
};
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};
use winit::window::{CursorIcon, Theme as SystemTheme};

use crate::input::{PointerButton, UiInput};
use crate::{
    App, AppEvent, EventContext, FrameContext, InputEvent, RunError, Waker, WindowOptions, run,
};

/// Lines scrolled per accessibility ScrollUp/ScrollDown request.
const ACCESSIBILITY_SCROLL_LINES: i32 = 3;

/// Whether the platform continues trackpad flings with momentum events of
/// its own; elsewhere the adapter adds inertia when the fingers lift.
const PLATFORM_MOMENTUM: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// An app described as quark-ui elements.
///
/// Elements carry actions as type-erased [`Action`]s; the adapter downcasts
/// each to [`UiApp::Action`] and passes it to [`UiApp::update`]. Actions of
/// other types (such as `NoopAction`) are dropped.
///
/// A text field needs no input code: give it a `focus_target` and implement
/// [`UiApp::edit_text`] (and [`UiApp::set_preedit`] for IME). While it has
/// focus, the adapter turns typed text, IME commits, editing keys, paste,
/// and pointer selection into [`TextEditCommand`]s for it.
pub trait UiApp: 'static {
    type Action: Any + Clone;

    /// Values other threads send through a [`UiSender`] (socket reads,
    /// finished jobs). Use `()` when the app takes none.
    type Message: Send + 'static;

    /// Called once after the window and renderer exist.
    fn init(&mut self, _cx: &mut UiContext) {}

    /// Build the element tree for the next frame.
    fn view(&mut self, cx: &mut ViewContext) -> AnyElement;

    /// Handle an action emitted by a click, scroll, drag, key binding, or
    /// assistive tech.
    fn update(&mut self, action: Self::Action, cx: &mut UiContext);

    /// A value sent through a [`UiSender`], delivered on the UI thread in
    /// send order. Redraw with `cx.window.request_redraw()` if it changed
    /// what the view shows.
    fn message(&mut self, _message: Self::Message, _cx: &mut UiContext) {}

    /// Events about the app and its window: theme changes, URLs from later
    /// launches, notification and tray clicks, closed windows and dialogs.
    /// The adapter has already applied a [`AppEvent::ThemeChanged`] to its
    /// theme when this runs.
    fn app_event(&mut self, _event: AppEvent, _cx: &mut UiContext) {}

    /// The user asked to close the window. Return false to keep it open.
    fn close_requested(&mut self, _cx: &mut UiContext) -> bool {
        true
    }

    /// Assistive tech set the value of the text field `target`.
    fn set_text_value(&mut self, _target: FocusId, _value: String, _cx: &mut UiContext) {}

    /// Apply `command` to the model behind the text field `target` and
    /// return what it did; `TextField::apply` and `Editor::apply` do both.
    /// The adapter writes the outcome's `clipboard_write` through
    /// [`UiApp::write_clipboard`] and redraws when the text or selection
    /// changed. Pointer selection steps may arrive while a frame is built,
    /// so this gets no context.
    fn edit_text(&mut self, _target: FocusId, _command: TextEditCommand) -> TextEditOutcome {
        TextEditOutcome::default()
    }

    /// The IME composition in the text field `target` changed
    /// (`TextField::set_preedit`). A composition that ends without a commit
    /// arrives as [`TextEditCommand::CancelPreedit`] instead.
    fn set_preedit(&mut self, _target: FocusId, _text: String, _cursor: Option<(usize, usize)>) {}

    /// Text to paste into a text field. Defaults to the system clipboard.
    fn read_clipboard(&mut self, cx: &mut UiContext) -> Option<String> {
        cx.window.clipboard_text()
    }

    /// Text a copy or cut in a text field produced. Defaults to the system
    /// clipboard.
    fn write_clipboard(&mut self, text: String, cx: &mut UiContext) {
        cx.window.set_clipboard_text(&text);
    }

    /// Sees every input event first. Return `true` to stop the adapter's own
    /// handling (clicks, wheel, keys, text editing, Tab focus traversal).
    fn event(&mut self, _event: &InputEvent, _cx: &mut UiContext) -> bool {
        false
    }

    /// Called after any [`crate::Waker::wake`], once pending messages have
    /// been delivered.
    fn wake(&mut self, _cx: &mut UiContext) {}
}

/// Sends [`UiApp::Message`]s to the UI thread from any thread. Clone it
/// into each worker. Messages sent before the window opens are delivered
/// right after [`UiApp::init`].
pub struct UiSender<M> {
    sender: Sender<M>,
    waker: Arc<OnceLock<Waker>>,
}

impl<M> Clone for UiSender<M> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            waker: Arc::clone(&self.waker),
        }
    }
}

impl<M: Send> UiSender<M> {
    /// Queue `message` for [`UiApp::message`] and wake the UI thread.
    /// Returns false once the app is gone.
    pub fn send(&self, message: M) -> bool {
        if self.sender.send(message).is_err() {
            return false;
        }
        if let Some(waker) = self.waker.get() {
            waker.wake();
        }
        true
    }
}

/// What [`UiApp::view`] can see while building a frame.
pub struct ViewContext<'a, 'f> {
    pub frame: &'a mut FrameContext<'f>,
    pub theme: &'a Theme,
    focus: Option<FocusId>,
    animations: &'a mut AnimationTable,
    geometry: &'a LayoutSnapshot,
    handles: &'a mut ElementHandles,
}

impl ViewContext<'_, '_> {
    /// Where the elements of the last completed frame landed: the frame
    /// before the one this view builds. Empty before the first frame.
    /// Reading it lays nothing out.
    pub fn geometry(&self) -> &LayoutSnapshot {
        self.geometry
    }

    /// A new handle to name an element with (`Div::element_handle`) and
    /// find it in [`Self::geometry`]. Release it once the element is gone.
    pub fn new_element_handle(&mut self) -> ElementHandle {
        self.handles.allocate()
    }

    pub fn release_element_handle(&mut self, handle: ElementHandle) {
        self.handles.release(handle);
    }

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

/// An [`EventContext`] plus the adapter's focus and message sender.
pub struct UiContext<'a, 'w> {
    pub window: &'a mut EventContext<'w>,
    focus: &'a mut Option<FocusId>,
    key_bindings: &'a mut KeyBindings,
    /// Set when a drag out took the pointer from the adapter.
    pointer_taken: &'a mut bool,
    announcer: &'a mut Announcer,
    theme: &'a mut Theme,
    theme_choice: &'a mut ThemeChoice,
    /// The adapter's `UiSender<U::Message>`.
    sender: &'a dyn Any,
    geometry: &'a LayoutSnapshot,
    handles: &'a mut ElementHandles,
}

impl UiContext<'_, '_> {
    /// Where the elements of the last completed frame landed, the frame
    /// input is routed through. Empty before the first frame. Reading it
    /// lays nothing out, so it does not see changes the app made since.
    pub fn geometry(&self) -> &LayoutSnapshot {
        self.geometry
    }

    /// A new handle to name an element with (`Div::element_handle`) and
    /// find it in [`Self::geometry`]. Release it once the element is gone.
    pub fn new_element_handle(&mut self) -> ElementHandle {
        self.handles.allocate()
    }

    pub fn release_element_handle(&mut self, handle: ElementHandle) {
        self.handles.release(handle);
    }

    pub fn focus(&self) -> Option<FocusId> {
        *self.focus
    }

    /// The window's key bindings, resolved against the key contexts on
    /// the focus path; see [`UiAdapter::with_key_bindings`]. Replace them
    /// when the user edits the keymap.
    pub fn key_bindings_mut(&mut self) -> &mut KeyBindings {
        self.key_bindings
    }

    /// Drag `paths` out of the window as files
    /// ([`EventContext::start_drag_out`]). The platform takes the pointer,
    /// so the adapter ends its pointer capture: the drag that called this
    /// gets its release right after.
    pub fn start_drag_out<P: AsRef<std::path::Path>>(
        &mut self,
        paths: impl IntoIterator<Item = P>,
    ) -> Result<(), crate::platform::drag_out::DragOutError> {
        self.window.start_drag_out(paths)?;
        *self.pointer_taken = true;
        Ok(())
    }

    pub fn set_focus(&mut self, focus: Option<FocusId>) {
        if *self.focus != focus {
            *self.focus = focus;
            redraw(Redraw::Focus, self.window);
        }
    }

    /// Have assistive tech speak `text`, for changes nothing on screen
    /// names: a finished response, a background save. Each call is spoken
    /// once, even when it repeats the last text. For text that is on
    /// screen (a toast), make its element a live region instead
    /// (`Div::live`).
    pub fn announce(&mut self, text: impl Into<String>, politeness: Politeness) {
        self.announcer.announce(text, politeness);
        // The announcement is published with the next frame's tree.
        self.window.request_redraw();
    }

    /// The theme the next frame paints with.
    pub fn theme(&self) -> &Theme {
        self.theme
    }

    /// Paint with `theme` from the next frame on, whatever the desktop
    /// prefers. Cached elements rebuild in the new colors; text layouts
    /// are reused unless the theme's font sizes changed.
    pub fn set_theme(&mut self, theme: Theme) {
        *self.theme_choice = ThemeChoice::Fixed;
        self.apply_theme(theme);
    }

    /// Paint with `light` or `dark` as the desktop prefers, from the next
    /// frame on (dark while the preference is unknown).
    pub fn set_themes(&mut self, light: Theme, dark: Theme) {
        let theme = match self.window.theme() {
            Some(SystemTheme::Light) => light.clone(),
            _ => dark.clone(),
        };
        *self.theme_choice = ThemeChoice::System {
            light: Box::new(light),
            dark: Box::new(dark),
        };
        self.apply_theme(theme);
    }

    fn apply_theme(&mut self, theme: Theme) {
        if *self.theme != theme {
            *self.theme = theme;
            redraw(Redraw::Theme, self.window);
        }
    }

    /// Change font families or ligatures live. Text is reshaped once with
    /// the new fonts (layout caches drop the old shapes); editors reshape
    /// on their next flush.
    pub fn set_fonts(&mut self, fonts: &quark_text::FontSettings) {
        let system = &mut self.window.text().system;
        let before = system.generation();
        system.set_font_settings(fonts);
        if system.generation() != before {
            // Every window shares the text system.
            self.window.request_redraw_all();
        }
    }

    /// A sender for this app's messages; `M` must be the app's
    /// [`UiApp::Message`].
    ///
    /// # Panics
    ///
    /// When `M` is another type.
    #[track_caller]
    pub fn sender<M: Send + 'static>(&self) -> UiSender<M> {
        self.sender
            .downcast_ref::<UiSender<M>>()
            .expect("UiContext::sender: M must be the app's UiApp::Message")
            .clone()
    }
}

/// Why the adapter asked for a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Redraw {
    /// The elements under the pointer changed, so hover styles did.
    Hover,
    Focus,
    /// An action reached the app.
    Action,
    /// A text field's text, selection, or composition changed.
    TextEdit,
    /// Input moved a scroll handle.
    Scroll,
    Theme,
}

fn redraw(reason: Redraw, cx: &mut EventContext) {
    tracing::trace!(?reason, "redraw");
    cx.request_redraw();
}

/// Which theme the adapter paints with.
enum ThemeChoice {
    /// One theme whatever the desktop prefers.
    Fixed,
    /// Follow the desktop's light or dark preference.
    System { light: Box<Theme>, dark: Box<Theme> },
}

/// Runs a [`UiApp`] as an [`App`].
pub struct UiAdapter<U: UiApp> {
    pub(crate) app: U,
    name: String,
    theme: Theme,
    theme_choice: ThemeChoice,
    signals: SignalStore,
    focus: Option<FocusId>,
    pointer: Option<(f32, f32)>,
    /// Hit entries under the pointer in the routed frame, topmost first:
    /// what hover styles were painted from.
    hovered: Vec<HitId>,
    /// Routes input through the last painted frame until the next replaces it.
    router: InputRouter,
    /// Handles the app names elements with.
    element_handles: ElementHandles,
    key_bindings: KeyBindings,
    /// A drag out took the pointer during the last dispatch.
    pointer_taken: bool,
    accessibility: AccessibilityFrame,
    announcer: Announcer,
    /// Text fields of the last frame, for pointer selection and IME.
    text_areas: Vec<TextInputHitArea>,
    text_pointer: TextPointer,
    /// The focused text field as of the last input, to cancel its
    /// composition once focus leaves it.
    edit_focus: Option<FocusId>,
    /// IME state last sent to the window.
    ime_allowed: bool,
    ime_area: Option<Rect>,
    /// Scale factor of the last painted frame, for accessibility bounds.
    scale_factor: f32,
    animations: AnimationTable,
    /// Cached subtrees and the layout engine, reused every frame.
    element_cache: ElementCache,
    /// Buffers of the frame before last, reused by the next frame: the
    /// scene the runner handed back, the input frame routing let go of,
    /// the text input areas, and the accessibility frame.
    spare_scene: Scene,
    spare_accessibility: AccessibilityFrame,
    spare_input: quark_ui::element::InputFrame,
    spare_text_areas: Vec<TextInputHitArea>,
    /// Last frame's scrollbar track buffer, reused by the next frame.
    spare_scrollbar_tracks: Vec<ScrollbarTrack>,
    spare_tooltip_regions: Vec<TooltipRegion>,
    sender: UiSender<U::Message>,
    messages: Receiver<U::Message>,
    #[cfg(feature = "devtools")]
    devtools: quark_ui::inspector::Devtools,
}

/// Open a window titled by `options` and run `app` in it.
pub fn run_ui<U: UiApp>(app: U, options: WindowOptions) -> Result<(), RunError> {
    let adapter = UiAdapter::new(app, options.title.clone());
    run(adapter, options)
}

impl<U: UiApp> UiAdapter<U> {
    /// `name` labels the accessibility tree's window node. The theme
    /// follows the desktop's light or dark preference, dark until it is
    /// known.
    pub fn new(app: U, name: impl Into<String>) -> Self {
        let (sender, messages) = mpsc::channel();
        Self {
            app,
            name: name.into(),
            theme: Theme::default_dark(),
            theme_choice: ThemeChoice::System {
                light: Box::new(Theme::default_light()),
                dark: Box::new(Theme::default_dark()),
            },
            signals: SignalStore::new(),
            focus: None,
            pointer: None,
            hovered: Vec::new(),
            router: InputRouter::default(),
            element_handles: ElementHandles::default(),
            key_bindings: KeyBindings::new(),
            pointer_taken: false,
            accessibility: AccessibilityFrame::default(),
            announcer: Announcer::default(),
            text_areas: Vec::new(),
            text_pointer: TextPointer::default(),
            edit_focus: None,
            ime_allowed: false,
            ime_area: None,
            scale_factor: 1.0,
            animations: AnimationTable::new(),
            element_cache: ElementCache::new(),
            spare_scene: Scene::default(),
            spare_accessibility: AccessibilityFrame::default(),
            spare_input: Default::default(),
            spare_text_areas: Vec::new(),
            spare_scrollbar_tracks: Vec::new(),
            spare_tooltip_regions: Vec::new(),
            sender: UiSender {
                sender,
                waker: Arc::new(OnceLock::new()),
            },
            messages,
            #[cfg(feature = "devtools")]
            devtools: quark_ui::inspector::Devtools::from_env(),
        }
    }

    /// Paint with `theme` whatever the desktop prefers.
    pub fn with_theme(mut self, theme: Theme) -> Self {
        self.theme = theme;
        self.theme_choice = ThemeChoice::Fixed;
        self
    }

    /// Paint with `light` or `dark` as the desktop prefers.
    pub fn with_themes(mut self, light: Theme, dark: Theme) -> Self {
        self.theme = dark.clone();
        self.theme_choice = ThemeChoice::System {
            light: Box::new(light),
            dark: Box::new(dark),
        };
        self
    }

    /// Resolve key presses against `bindings` as well as the elements'
    /// own `on_key` handlers. A binding's predicate is tested against the
    /// key contexts (`key_context("editor")`) on the focus path; see
    /// [`quark_ui::key_context`]. A binding matched through a context
    /// deeper than the element handling the key wins; otherwise the
    /// element's handler does, and a binding without a predicate only gets
    /// keys no element handles.
    pub fn with_key_bindings(mut self, bindings: KeyBindings) -> Self {
        self.key_bindings = bindings;
        self
    }

    pub fn app(&self) -> &U {
        &self.app
    }

    /// The theme the next frame paints with.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }

    /// A sender for [`UiApp::message`], for threads started before
    /// [`run`]. Inside the app, [`UiContext::sender`] gives the same.
    pub fn sender(&self) -> UiSender<U::Message> {
        self.sender.clone()
    }

    /// Run `f` with the app and a [`UiContext`] over `cx`.
    fn with_app<R>(
        &mut self,
        cx: &mut EventContext,
        f: impl FnOnce(&mut U, &mut UiContext) -> R,
    ) -> R {
        let mut ucx = UiContext {
            window: cx,
            focus: &mut self.focus,
            key_bindings: &mut self.key_bindings,
            pointer_taken: &mut self.pointer_taken,
            announcer: &mut self.announcer,
            theme: &mut self.theme,
            theme_choice: &mut self.theme_choice,
            sender: &self.sender,
            geometry: &self.router.frame().geometry,
            handles: &mut self.element_handles,
        };
        f(&mut self.app, &mut ucx)
    }

    fn deliver_messages(&mut self, cx: &mut EventContext) {
        while let Ok(message) = self.messages.try_recv() {
            self.with_app(cx, |app, ucx| app.message(message, ucx));
        }
    }

    fn dispatch(&mut self, actions: Vec<Action>, cx: &mut EventContext) {
        for action in actions {
            if let Some(event) = action.downcast_ref::<TextPointerEvent>() {
                let extend = cx.modifiers().shift_key();
                let now_ms = cx.elapsed().as_millis() as u64;
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
            self.with_app(cx, |app, ucx| app.update(action, ucx));
        }
        redraw(Redraw::Action, cx);
        // The platform's drag loop gets the release, so the drag the app
        // was tracking ends here instead.
        if std::mem::take(&mut self.pointer_taken) {
            let delivery = self.router.cancel_pointer();
            self.deliver(delivery, cx);
        }
    }

    /// Hand a routed event's actions to the app. A delivery without actions
    /// changed nothing, so it draws nothing.
    fn deliver(&mut self, delivery: Delivery, cx: &mut EventContext) {
        if !delivery.actions.is_empty() {
            self.dispatch(delivery.actions, cx);
        } else if delivery.redraw {
            redraw(Redraw::Scroll, cx);
        }
    }

    /// Track the elements under the pointer; hover styles only change when
    /// they do.
    fn update_hover(&mut self, cx: &mut EventContext) {
        let hovered = self.hovered_at(self.pointer);
        if hovered != self.hovered {
            self.hovered = hovered;
            self.update_cursor(cx);
            redraw(Redraw::Hover, cx);
        }
    }

    fn hovered_at(&self, pointer: Option<(f32, f32)>) -> Vec<HitId> {
        pointer.map_or_else(Vec::new, |(x, y)| self.router.frame().hits.stack_at(x, y))
    }

    fn pointer_pressed(&mut self, cx: &mut EventContext) {
        let Some((x, y)) = self.pointer else {
            return;
        };
        let before = self.focus;
        let delivery = self.router.pointer_down(x, y, &mut self.focus);
        if self.focus != before {
            redraw(Redraw::Focus, cx);
        }
        self.deliver(delivery, cx);
    }

    /// The focused text field, if focus is on one painted last frame.
    fn focused_field(&self) -> Option<FocusId> {
        self.focus
            .filter(|focus| self.text_areas.iter().any(|a| a.focus_target == *focus))
    }

    /// Apply `command` to the text field `target`, passing any copied text
    /// to the clipboard.
    fn edit(&mut self, target: FocusId, command: TextEditCommand, cx: &mut EventContext) {
        let outcome = self.app.edit_text(target, command);
        if let Some(text) = outcome.clipboard_write {
            self.with_app(cx, |app, ucx| app.write_clipboard(text, ucx));
        }
        if outcome.text_changed || outcome.selection_changed {
            redraw(Redraw::TextEdit, cx);
        }
    }

    /// Keys go to the focused text field when they edit text, then to key
    /// bindings on the focus path, then to Tab focus traversal, so an
    /// editor that binds Tab keeps it.
    fn key(&mut self, binding: Binding, cx: &mut EventContext) {
        if let Some(target) = self.focused_field() {
            let paste = Binding::new(
                Mods {
                    primary: true,
                    ..Mods::default()
                },
                "v",
            );
            let command = if paste.matches(&binding) {
                self.with_app(cx, |app, ucx| app.read_clipboard(ucx))
                    .map(TextEditCommand::Paste)
            } else {
                command_for_binding(&binding.to_string())
            };
            if let Some(command) = command {
                self.edit(target, command, cx);
                return;
            }
        }
        let delivery = self.router.key_down(&binding, self.focus);
        if let Some(action) = self.bound_action(&binding, delivery.node) {
            self.dispatch(vec![action], cx);
            return;
        }
        if delivery.node.is_some() {
            self.deliver(delivery, cx);
            return;
        }
        // Enter or Space on a focused clickable that binds neither clicks it,
        // so everything a pointer can press is reachable from the keyboard.
        let delivery = self.router.activate(&binding, self.focus);
        if delivery.node.is_some() {
            self.deliver(delivery, cx);
            return;
        }
        let delivery = self.router.scroll_key(&binding, self.focus);
        if delivery.node.is_some() {
            self.deliver(delivery, cx);
            return;
        }
        let mods = binding.mods;
        if binding.key == "tab" && !(mods.cmd || mods.ctrl || mods.alt) {
            let next = self.router.traverse_focus(self.focus, mods.shift);
            if next != self.focus {
                self.focus = next;
                redraw(Redraw::Focus, cx);
            }
        }
    }

    /// The key binding `pressed` triggers, when it outranks the element
    /// `handled_by` that routing found: it matched through a key context
    /// inside that element, or no element handled the key.
    fn bound_action(&self, pressed: &Binding, handled_by: Option<usize>) -> Option<Action> {
        if self.key_bindings.is_empty() {
            return None;
        }
        let semantic = &self.router.frame().semantic;
        let path = context_path(semantic, self.focus);
        let entries: Vec<_> = path.iter().map(|(_, entry)| entry.clone()).collect();
        let found = self.key_bindings.resolve(pressed, &entries)?;
        let context_node = found.depth.checked_sub(1).map(|i| path[i].0);
        let wins = match (handled_by, context_node) {
            (None, _) => true,
            (Some(element), Some(context)) => {
                context != element && semantic.is_within(context, element)
            }
            (Some(_), None) => false,
        };
        wins.then(|| found.binding.action.clone())
    }

    fn input(&mut self, input: UiInput, cx: &mut EventContext) {
        match input {
            UiInput::PointerMove { x, y } => {
                self.pointer = Some((x, y));
                let delivery = self.router.pointer_move(x, y);
                self.deliver(delivery, cx);
                self.update_hover(cx);
            }
            // Capture survives the pointer leaving: a drag outside the
            // window keeps its moves and its release on every platform.
            UiInput::PointerLeave => {
                self.pointer = None;
                self.update_hover(cx);
            }
            UiInput::PointerDown(PointerButton::Primary) => self.pointer_pressed(cx),
            UiInput::PointerUp(PointerButton::Primary) => {
                let delivery = self.router.pointer_up();
                self.deliver(delivery, cx);
            }
            UiInput::PointerDown(PointerButton::Middle) => {
                if let Some((x, y)) = self.pointer {
                    self.router.middle_down(x, y);
                }
            }
            UiInput::PointerUp(PointerButton::Middle) => {
                if let Some((x, y)) = self.pointer {
                    let delivery = self.router.middle_up(x, y);
                    self.deliver(delivery, cx);
                }
            }
            UiInput::PointerDown(_) | UiInput::PointerUp(_) => {}
            UiInput::Wheel { dx, dy, ended } => {
                let now_ms = cx.elapsed().as_millis() as u64;
                // Shift turns a plain vertical wheel sideways.
                let (dx, dy) = if cx.modifiers().shift_key() && dx == 0.0 {
                    (dy, 0.0)
                } else {
                    (dx, dy)
                };
                if let Some((x, y)) = self.pointer {
                    let event = WheelEvent { dx, dy, now_ms };
                    let delivery = self.router.scroll_wheel(x, y, event);
                    self.deliver(delivery, cx);
                }
                if ended && !PLATFORM_MOMENTUM && self.router.fling(now_ms) {
                    redraw(Redraw::Scroll, cx);
                }
            }
            UiInput::Key(binding) => self.key(binding, cx),
            UiInput::Text(text) => {
                if let Some(target) = self.focused_field() {
                    self.edit(target, TextEditCommand::InsertText(text), cx);
                }
            }
            UiInput::Preedit { text, cursor } => {
                if let Some(target) = self.focused_field() {
                    if text.is_empty() {
                        self.app.edit_text(target, TextEditCommand::CancelPreedit);
                    } else {
                        self.app.set_preedit(target, text, cursor);
                    }
                    redraw(Redraw::TextEdit, cx);
                }
            }
            // The platform sends no release once the window lost focus.
            UiInput::WindowFocus(false) => {
                let delivery = self.router.cancel_pointer();
                self.deliver(delivery, cx);
            }
            UiInput::WindowFocus(true) => {}
        }
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
        let current = self.focused_field();
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
            redraw(Redraw::TextEdit, cx);
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

/// Lay out and paint `root` into a `width` x `height` point viewport, in
/// `scene`'s buffer.
fn paint(
    root: &mut AnyElement,
    ecx: &mut ElementContext,
    mut scene: Scene,
    width: f32,
    height: f32,
) -> Painted {
    scene.primitives.clear();
    ecx.accessibility.reset(width, height);
    ecx.semantic.reset(width, height);
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
    /// A click: focus what the node stands for, as a pointer click does,
    /// then deliver its action.
    Click(Action, Option<FocusId>),
    Focus(FocusId),
    SetValue(FocusId, String),
    /// Select from `anchor` to `focus`, byte offsets into the field's text.
    Select {
        target: FocusId,
        anchor: usize,
        focus: usize,
    },
    Edit(FocusId, TextEditCommand),
}

fn route_accessibility(frame: &AccessibilityFrame, request: &ActionRequest) -> Option<Routed> {
    // Any node standing for a focus target takes focus, action or not.
    if request.action == AxAction::Focus {
        return frame
            .focus_target_of(request.target_node)
            .map(Routed::Focus);
    }
    let target = frame.action_for(request.target_node)?;
    let scroll_lines = match request.action {
        AxAction::ScrollUp => -ACCESSIBILITY_SCROLL_LINES,
        _ => ACCESSIBILITY_SCROLL_LINES,
    };
    match (request.action, target) {
        (AxAction::Click, AccessibilityAction::Click(action)) => Some(Routed::Click(
            action.clone(),
            frame.focus_target_of(request.target_node),
        )),
        (
            AxAction::Click,
            AccessibilityAction::TextValue(focus)
            | AccessibilityAction::EditorViewport { focus, .. },
        ) => Some(Routed::Focus(*focus)),
        (AxAction::SetValue, AccessibilityAction::TextValue(focus)) => match &request.data {
            Some(ActionData::Value(value)) => Some(Routed::SetValue(*focus, value.to_string())),
            _ => None,
        },
        (
            AxAction::ReplaceSelectedText,
            AccessibilityAction::TextValue(focus)
            | AccessibilityAction::EditorViewport { focus, .. },
        ) => match &request.data {
            Some(ActionData::Value(value)) => Some(Routed::Edit(
                *focus,
                TextEditCommand::InsertText(value.to_string()),
            )),
            _ => None,
        },
        (AxAction::SetTextSelection, _) => match &request.data {
            Some(ActionData::SetTextSelection(selection)) => frame
                .text_selection_for(request.target_node, selection)
                .map(|(target, anchor, focus)| Routed::Select {
                    target,
                    anchor,
                    focus,
                }),
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
        let _ = self.sender.waker.set(cx.waker().clone());
        self.with_app(cx, |app, ucx| app.init(ucx));
        self.deliver_messages(cx);
        self.after_input(false, cx);
    }

    fn frame(&mut self, cx: &mut FrameContext) -> Scene {
        let (width, height) = cx.size();
        let scale = cx.scale_factor();
        let clock_ms = cx.elapsed().as_millis() as u64;
        // A pointer held past a text field's edge keeps selecting (and so
        // scrolling) without moving; apply the step before this frame's view.
        if let Some((target, command)) = self.text_pointer.autoscroll(&self.text_areas, clock_ms) {
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
            geometry: &self.router.frame().geometry,
            handles: &mut self.element_handles,
        });
        #[cfg(feature = "devtools")]
        let build_us = view_started.elapsed().as_micros() as u64;

        let accessibility = cx.accessibility_active();
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
        .with_animations(&mut self.animations)
        .with_element_cache(&mut self.element_cache)
        .with_accessibility(accessibility)
        .with_input_frame(std::mem::take(&mut self.spare_input));
        ecx.text_input_hit_areas = std::mem::take(&mut self.spare_text_areas);
        ecx.text_input_hit_areas.clear();
        ecx.accessibility = std::mem::take(&mut self.spare_accessibility);
        ecx.scrollbar_tracks = std::mem::take(&mut self.spare_scrollbar_tracks);
        ecx.scrollbar_tracks.clear();
        ecx.tooltip_regions = std::mem::take(&mut self.spare_tooltip_regions);
        ecx.tooltip_regions.clear();
        #[cfg(feature = "devtools")]
        self.devtools.begin_frame(&mut ecx.devtools);
        let scene = std::mem::take(&mut self.spare_scene);
        let painted = paint(&mut root, &mut ecx, scene, width, height);
        self.spare_scrollbar_tracks = std::mem::take(&mut ecx.scrollbar_tracks);
        self.spare_tooltip_regions = std::mem::take(&mut ecx.tooltip_regions);
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
            cx.request_frame_in(Duration::from_millis(at_ms.saturating_sub(clock_ms)));
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
        let input = UiInput::from_event(&event);
        let window_blurred = input == Some(UiInput::WindowFocus(false));
        self.handle_event(&event, input, cx);
        self.after_input(window_blurred, cx);
    }

    fn wake(&mut self, cx: &mut EventContext) {
        self.deliver_messages(cx);
        self.with_app(cx, |app, ucx| app.wake(ucx));
        self.after_input(false, cx);
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
        if let AppEvent::ThemeChanged(system) = &event
            && let ThemeChoice::System { light, dark } = &self.theme_choice
        {
            self.theme = match system {
                SystemTheme::Light => (**light).clone(),
                SystemTheme::Dark => (**dark).clone(),
            };
            redraw(Redraw::Theme, cx);
        }
        self.with_app(cx, |app, ucx| app.app_event(event, ucx));
        self.after_input(false, cx);
    }

    fn close_requested(&mut self, cx: &mut EventContext) -> bool {
        self.with_app(cx, |app, ucx| app.close_requested(ucx))
    }

    fn recycle_scene(&mut self, scene: Scene) {
        self.spare_scene = scene;
    }

    fn accessibility(&mut self) -> Option<TreeUpdate> {
        // A full tree; accesskit diffs it against the last one.
        let mut update = self.logical_accessibility_tree();
        scale_tree(&mut update, self.scale_factor);
        Some(update)
    }

    fn accessibility_action(&mut self, request: ActionRequest, cx: &mut EventContext) {
        self.handle_accessibility_action(request, cx);
        self.after_input(false, cx);
    }
}

/// What [`crate::testing`] reads from the adapter.
#[cfg(feature = "test-support")]
impl<U: UiApp> UiAdapter<U> {
    pub(crate) fn app_mut(&mut self) -> &mut U {
        &mut self.app
    }

    pub(crate) fn focus(&self) -> Option<FocusId> {
        self.focus
    }

    pub(crate) fn accessibility_frame(&self) -> &AccessibilityFrame {
        &self.accessibility
    }

    pub(crate) fn semantic_frame(&self) -> &SemanticFrame {
        &self.router.frame().semantic
    }

    pub(crate) fn geometry(&self) -> &LayoutSnapshot {
        &self.router.frame().geometry
    }

    /// The semantic node of the topmost hit region at a point that has one.
    pub(crate) fn semantic_node_at(&self, x: f32, y: f32) -> Option<usize> {
        let hits = &self.router.frame().hits;
        hits.stack_at(x, y).into_iter().find_map(|id| hits.node(id))
    }
}

impl<U: UiApp> UiAdapter<U> {
    /// The last painted frame's accessibility tree, in points.
    pub(crate) fn logical_accessibility_tree(&self) -> TreeUpdate {
        let mut update = self.accessibility.tree_update(&self.name, self.focus);
        self.announcer.publish(&mut update);
        update
    }

    /// Keep `painted`'s input state for routing until the next frame, and
    /// return its scene with the IME changes for the caret it painted.
    fn finish_frame(&mut self, painted: Painted) -> (Scene, ImeRequest) {
        self.spare_input = self.router.replace_frame(painted.input);
        // This frame painted hover for the current pointer; later moves
        // compare against it. Into the kept buffer: a pointer resting over
        // the window must not cost an allocation per frame.
        match self.pointer {
            Some((x, y)) => self
                .router
                .frame()
                .hits
                .stack_at_into(x, y, &mut self.hovered),
            None => self.hovered.clear(),
        }
        self.spare_accessibility =
            std::mem::replace(&mut self.accessibility, painted.accessibility);
        let ime = self.ime_request(&painted.text_areas);
        self.spare_text_areas = std::mem::replace(&mut self.text_areas, painted.text_areas);
        (painted.scene, ime)
    }

    fn handle_event(&mut self, event: &InputEvent, input: Option<UiInput>, cx: &mut EventContext) {
        #[cfg(feature = "devtools")]
        if let Some(input) = &input
            && crate::devtools::intercept(&mut self.devtools, input, cx)
        {
            return;
        }
        if self.with_app(cx, |app, ucx| app.event(event, ucx)) {
            return;
        }
        if let Some(input) = input {
            self.input(input, cx);
        }
    }

    fn handle_accessibility_action(&mut self, request: ActionRequest, cx: &mut EventContext) {
        match route_accessibility(&self.accessibility, &request) {
            Some(Routed::Dispatch(action)) => self.dispatch(vec![action], cx),
            Some(Routed::Click(action, focus)) => {
                if let Some(focus) = focus
                    && self.focus != Some(focus)
                {
                    self.focus = Some(focus);
                    redraw(Redraw::Focus, cx);
                }
                self.dispatch(vec![action], cx);
            }
            Some(Routed::Focus(focus)) => {
                if self.focus != Some(focus) {
                    self.focus = Some(focus);
                    redraw(Redraw::Focus, cx);
                }
            }
            Some(Routed::SetValue(target, value)) => {
                self.with_app(cx, |app, ucx| app.set_text_value(target, value, ucx));
                redraw(Redraw::TextEdit, cx);
            }
            Some(Routed::Select {
                target,
                anchor,
                focus,
            }) => {
                self.app
                    .edit_text(target, TextEditCommand::SetTextCursor(anchor));
                self.edit(target, TextEditCommand::ExtendTextSelection(focus), cx);
                // The cursor move alone may have changed the selection.
                redraw(Redraw::TextEdit, cx);
            }
            Some(Routed::Edit(target, command)) => self.edit(target, command, cx),
            None => tracing::debug!("unhandled accessibility request: {request:?}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::thread::{self, ThreadId};

    use accesskit::{NodeId, Role};
    use quark::scene::ShapedText;
    use quark_ui::element::{IntoAnyElement, div, text_input};
    use quark_ui::style::Styled;
    use quark_ui::text_input::TextField;

    use super::*;
    use crate::testing::UiTestHarness;

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

    /// An app with one text field that applies the adapter's edits to it.
    struct FieldApp {
        field: TextField,
    }

    impl UiApp for FieldApp {
        type Action = Msg;
        type Message = ();

        fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
            div().into_any()
        }

        fn update(&mut self, _action: Msg, _cx: &mut UiContext) {}

        fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
            if target == FIELD {
                return self.field.apply(command);
            }
            TextEditOutcome::default()
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
}
