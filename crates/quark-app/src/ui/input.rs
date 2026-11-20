//! Input routing: each event goes to the app's hook first, then to jump
//! mode, the pointer, the focused text field, key bindings, and focus
//! traversal.

use quark_ui::element::{Binding, Mods, TextHitContent};
use quark_ui::key_context::context_path;
use quark_ui::text_input::{TextEditCommand, command_for_binding};
use quark_ui::{Action, FocusId};

use super::{Redraw, UiAdapter, UiApp, redraw};
use crate::input::UiInput;
use crate::{EventContext, InputEvent};

impl<U: UiApp> UiAdapter<U> {
    pub(super) fn handle_event(
        &mut self,
        event: &InputEvent,
        input: Option<UiInput>,
        cx: &mut EventContext,
    ) {
        #[cfg(feature = "devtools")]
        if let Some(input) = &input
            && crate::devtools::intercept(&mut self.win.devtools, input, cx)
        {
            return;
        }
        if self.jump_input(input.as_ref(), cx) {
            return;
        }
        let handled = self.with_app(cx, |app, ucx| app.event(event, ucx));
        self.end_taken_pointer(cx);
        if handled {
            return;
        }
        if let Some(input) = input {
            let mods = crate::input::mods_of(cx.modifiers());
            self.win.router.set_modifiers(mods);
            self.input(input, cx);
        }
    }

    fn input(&mut self, input: UiInput, cx: &mut EventContext) {
        match input {
            UiInput::PointerMove { x, y } => self.pointer_moved(x, y, cx),
            UiInput::PointerLeave => self.pointer_left(cx),
            UiInput::PointerDown(button) => self.button_down(button, cx),
            UiInput::PointerUp(button) => self.button_up(button, cx),
            UiInput::Wheel { dx, dy, ended } => self.wheel(dx, dy, ended, cx),
            UiInput::Key(binding) => self.key(binding, cx),
            UiInput::Text(text) | UiInput::ImeCommit(text) => self.typed(text, cx),
            UiInput::Preedit { text, cursor } => self.preedit(text, cursor, cx),
            // The platform sends no release once the window lost focus. A
            // drag session is not the window's: its transport ends it.
            UiInput::WindowFocus(false) => {
                let delivery = self.win.router.cancel_pointer();
                self.deliver(delivery, cx);
            }
            UiInput::WindowFocus(true) => {}
        }
    }

    /// Typed or committed text, for the focused text field.
    fn typed(&mut self, text: String, cx: &mut EventContext) {
        if let Some(target) = self.win.focused_field() {
            self.edit(target, TextEditCommand::InsertText(text), cx);
        } else {
            // Only the length, since the text may be a password.
            tracing::debug!(
                focus = ?self.win.focus.current,
                "{} chars of text input dropped: focus is not on a text field",
                text.chars().count()
            );
        }
    }

    /// Apply `command` to the text field `target`, passing any copied text
    /// to the clipboard.
    pub(super) fn edit(
        &mut self,
        target: FocusId,
        command: TextEditCommand,
        cx: &mut EventContext,
    ) {
        let outcome = Self::edit_text_in(&mut self.app, self.win_handle, target, command);
        if let Some(text) = outcome.clipboard_write {
            self.with_app(cx, |app, ucx| app.write_clipboard(text, ucx));
        }
        if outcome.text_changed || outcome.selection_changed {
            redraw(Redraw::TextEdit, self.win_handle, cx);
        }
    }

    /// Keys go to the focused text field when they edit text, then to key
    /// bindings on the focus path, then to Tab focus traversal, so an
    /// editor that binds Tab keeps it. Escape first ends a drag session.
    fn key(&mut self, binding: Binding, cx: &mut EventContext) {
        // Any key shows focus from here on, also where it lands through a
        // binding or the app's own `set_focus`.
        self.show_focus(cx);
        if self.shared.drag.is_some() && binding.key == "escape" && binding.mods == Mods::default()
        {
            self.end_session(cx);
            return;
        }
        if let Some(target) = self.win.focused_field() {
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
            if types_text(&binding) {
                return;
            }
        }
        let delivery = self.win.router.key_down(&binding, self.win.focus.current);
        if let Some(action) = self.bound_action(&binding, delivery.node) {
            self.dispatch(vec![action], cx);
            return;
        }
        if delivery.node.is_some() {
            self.deliver(delivery, cx);
            return;
        }
        // After bindings, so an app can bind Enter to send.
        let m = binding.mods;
        if binding.key == "enter"
            && !(m.cmd || m.ctrl || m.alt || m.primary)
            && let Some(target) = self.win.focused_field()
            && (self.win.text_areas.iter()).any(|a| {
                a.focus_target == target && matches!(a.content, TextHitContent::Multiline { .. })
            })
        {
            self.edit(target, TextEditCommand::InsertNewline, cx);
            return;
        }
        let delivery = self
            .win
            .router
            .context_menu_key(&binding, self.win.focus.current);
        if delivery.node.is_some() {
            self.deliver(delivery, cx);
            return;
        }
        // Enter or Space on a focused clickable that binds neither clicks it,
        // so everything a pointer can press is reachable from the keyboard.
        let delivery = self.win.router.activate(&binding, self.win.focus.current);
        if delivery.node.is_some() {
            self.deliver(delivery, cx);
            return;
        }
        let delivery = self.win.router.scroll_key(&binding, self.win.focus.current);
        if delivery.node.is_some() {
            self.deliver(delivery, cx);
            return;
        }
        let mods = binding.mods;
        if binding.key == "tab" && !(mods.cmd || mods.ctrl || mods.alt) {
            let next = self
                .win
                .router
                .traverse_focus(self.win.focus.current, mods.shift);
            if self.win.focus.set(next) {
                redraw(Redraw::Focus, self.win_handle, cx);
            }
        }
    }

    /// The key binding `pressed` triggers, when it outranks the element
    /// `handled_by` that routing found: it matched through a key context
    /// inside that element, or no element handled the key.
    fn bound_action(&self, pressed: &Binding, handled_by: Option<usize>) -> Option<Action> {
        if self.shared.key_bindings.is_empty() {
            return None;
        }
        let semantic = &self.win.router.frame().semantic;
        let path = context_path(semantic, self.win.focus.current);
        let entries: Vec<_> = path.iter().map(|(_, entry)| entry.clone()).collect();
        let found = self.shared.key_bindings.resolve(pressed, &entries)?;
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
}

/// Whether `binding` types a character, which a text field takes as typed
/// text rather than as a key: a character or space with no modifier but
/// Shift, or with Option on macOS, where it composes characters.
pub(super) fn types_text(binding: &Binding) -> bool {
    let m = binding.mods;
    let composes = cfg!(target_os = "macos") && m.alt;
    !(m.cmd || m.ctrl || m.primary || (m.alt && !composes))
        && (binding.key == "space" || binding.key.chars().count() == 1)
}
