//! The platform IME: allowed while an IME target has focus, its candidate
//! window at the caret painted last, and each composition kept to the
//! element it began in. See the [module docs](super#windows).

use quark::Rect;
use quark_ui::FocusId;
use quark_ui::element::ImeTarget;
use quark_ui::text_input::{TextEditCommand, TextPointer};

use super::{Redraw, UiAdapter, UiApp, redraw};
use crate::{EventContext, InputEvent};

/// The platform IME's composition, as IME events and focus left it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Composition {
    #[default]
    None,
    /// Composing in this element, focused when its preedit began, until
    /// the commit ends it.
    Owned(FocusId),
    /// Focus left the element composing. Until the next frame resets the
    /// platform IME, events still arriving for that composition are
    /// dropped.
    Orphaned,
}

/// The platform IME as the adapter last set it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
enum PlatformIme {
    #[default]
    Disabled,
    /// Allowed, with its candidate window at this caret once one was sent.
    Enabled(Option<Rect>),
}

/// A window's IME.
#[derive(Debug, Default)]
pub(super) struct ImeState {
    /// The focused text field as of the last input, to cancel its
    /// composition once focus leaves it.
    edit_focus: Option<FocusId>,
    platform: PlatformIme,
    /// The platform IME's composition and the element it belongs to.
    composition: Composition,
}

/// IME changes for the window, applied once the frame is built.
#[derive(Debug, Default, PartialEq)]
pub(super) struct ImeRequest {
    pub(super) allowed: Option<bool>,
    pub(super) cursor_area: Option<Rect>,
    pub(super) reset: bool,
}

impl ImeState {
    /// The changes a frame that painted `targets` asks of the platform IME.
    /// IME follows `focus`: allowed while an IME target (a text field, a
    /// terminal) has focus, with the candidate window at the caret
    /// `targets` painted. Focus that left the element composing resets the
    /// IME, so the rest of that composition cannot land in the element
    /// focused now.
    pub(super) fn request(&mut self, focus: Option<FocusId>, targets: &[ImeTarget]) -> ImeRequest {
        let target = focus.and_then(|focus| targets.iter().find(|t| t.focus_target == focus));
        let mut request = ImeRequest::default();
        let allowed = target.is_some();
        self.orphan_moved(focus);
        let enabled = matches!(self.platform, PlatformIme::Enabled(_));
        if self.composition == Composition::Orphaned {
            self.composition = Composition::None;
            // Turning IME off drops the composition by itself.
            request.reset = enabled && allowed;
        }
        if allowed != enabled || request.reset {
            request.allowed = Some(allowed);
            self.platform = if allowed {
                PlatformIme::Enabled(None)
            } else {
                PlatformIme::Disabled
            };
        }
        if let Some(caret) = target.and_then(|area| area.caret)
            && self.platform != PlatformIme::Enabled(Some(caret))
        {
            request.cursor_area = Some(caret);
            self.platform = PlatformIme::Enabled(Some(caret));
        }
        request
    }

    /// Orphan the composition once focus has left the element composing.
    /// Returns whether it did.
    fn orphan_moved(&mut self, focus: Option<FocusId>) -> bool {
        let moved = matches!(self.composition, Composition::Owned(owner) if Some(owner) != focus);
        if moved {
            self.composition = Composition::Orphaned;
        }
        moved
    }

    /// Orphan the composition an element owns, returning that element.
    fn orphan(&mut self) -> Option<FocusId> {
        let Composition::Owned(target) = self.composition else {
            return None;
        };
        self.composition = Composition::Orphaned;
        Some(target)
    }

    /// End the composition an element owns for good, as its window
    /// closes, returning that element.
    pub(super) fn close(&mut self) -> Option<FocusId> {
        let Composition::Owned(target) = self.composition else {
            return None;
        };
        self.composition = Composition::None;
        Some(target)
    }

    /// Track the composition through `event`, with `focus` on the element
    /// focused now, and whether the event belongs to that element.
    fn event_is_current(&mut self, event: &InputEvent, focus: Option<FocusId>) -> bool {
        match (event, self.composition) {
            (InputEvent::ImePreedit(..) | InputEvent::ImeCommit(_), Composition::Orphaned) => false,
            (InputEvent::ImePreedit(text, _), _) if !text.is_empty() => {
                self.composition = focus.map_or(Composition::None, Composition::Owned);
                true
            }
            // A commit without a preedit, as some IMEs send for plain
            // typing, goes to the focused element.
            (InputEvent::ImeCommit(_), _) => {
                self.composition = Composition::None;
                true
            }
            _ => true,
        }
    }

    /// Follow the focused text field to `field` after input. Returns the
    /// field whose composition is stale: the one focus left, or the focused
    /// one when the window lost focus.
    fn leave_field(&mut self, field: Option<FocusId>, window_blurred: bool) -> Option<FocusId> {
        let stale = (self.edit_focus).filter(|previous| window_blurred || Some(*previous) != field);
        self.edit_focus = field;
        stale
    }
}

impl<U: UiApp> UiAdapter<U> {
    /// The IME composition in the focused text field changed; empty text
    /// ends it.
    pub(super) fn preedit(
        &mut self,
        text: String,
        cursor: Option<(usize, usize)>,
        cx: &mut EventContext,
    ) {
        let Some(target) = self.win.focused_field() else {
            return;
        };
        if text.is_empty() {
            let cancel = TextEditCommand::CancelPreedit;
            Self::edit_text_in(&mut self.app, self.win_handle, target, cancel);
        } else if let Some(window) = self.win_handle {
            self.app.set_preedit_in(window, target, text, cursor);
        } else {
            self.app.set_preedit(target, text, cursor);
        }
        redraw(Redraw::TextEdit, self.win_handle, cx);
    }

    /// Cancel the composition of a text field focus left since the last
    /// input, or of the focused one when the window lost focus. winit
    /// reports no `Ime::Disabled` for either, so a preedit would otherwise
    /// linger. Returns whether a field was told.
    pub(super) fn cancel_stale_preedit(&mut self, window_blurred: bool) -> bool {
        let field = self.win.focused_field();
        let stale = self.win.ime.leave_field(field, window_blurred);
        if let Some(target) = stale {
            let cancel = TextEditCommand::CancelPreedit;
            Self::edit_text_in(&mut self.app, self.win_handle, target, cancel);
        }
        stale.is_some()
    }

    /// Bring the current window's IME in line with its focus after input.
    /// When focus landed on one of its elements, compositions in every
    /// other window end.
    pub(super) fn settle_focus(&mut self, window_blurred: bool, cx: &mut EventContext) {
        if self.cancel_stale_preedit(window_blurred) {
            redraw(Redraw::TextEdit, self.win_handle, cx);
        }
        if self.orphan_moved_composition() {
            // The frame resets the platform IME.
            redraw(Redraw::Focus, self.win_handle, cx);
        }
        if self.win.focus.settle().is_some() {
            self.end_other_compositions(cx);
        }
    }

    /// Focus moved onto an element of the current window, so the
    /// compositions of every other window end: their preedits are
    /// cancelled, and the rest the platform delivers for them is dropped
    /// until their next frame resets their IME. Otherwise a commit queued
    /// in a window the user left could land in a field that moved out of
    /// it under the same focus id. Their text selection drags stop too.
    fn end_other_compositions(&mut self, cx: &mut EventContext) {
        for (handle, state) in &mut self.parked {
            let Some(window) = *handle else {
                continue;
            };
            state.text_pointer = TextPointer::default();
            if let Some(target) = state.ime.orphan() {
                let cancel = TextEditCommand::CancelPreedit;
                self.app.edit_text_in(window, target, cancel);
                redraw(Redraw::Focus, Some(window), cx);
            }
        }
    }

    /// Orphan the composition once focus has left the element composing,
    /// right away rather than at the next frame: the platform may still
    /// deliver the rest of it before that frame resets the IME. Returns
    /// whether it did.
    pub(super) fn orphan_moved_composition(&mut self) -> bool {
        let win = &mut *self.win;
        win.ime.orphan_moved(win.focus.current)
    }

    /// Track the composition through an IME event, and whether the event
    /// belongs to the element focused now. Events of an orphaned
    /// composition, its closing empty preedit and its commit included, do
    /// not: they reach neither the app's hook nor the text field.
    ///
    /// The owner is kept through the closing empty preedit and a window
    /// blur, since the platform sends the commit after both: only the
    /// commit or the IME reset ends the composition.
    pub(super) fn ime_event_is_current(&mut self, event: &InputEvent) -> bool {
        let win = &mut *self.win;
        win.ime.event_is_current(event, win.focus.current)
    }
}
