//! Keyboard focus: each window's focused element, whether it shows its
//! ring, and the context methods that move it.

use quark_ui::FocusId;

use super::{Redraw, UiAdapter, UiApp, UiContext, redraw};
use crate::{EventContext, WindowHandle};

/// A window's keyboard focus.
#[derive(Debug, Default)]
pub(super) struct FocusState {
    /// The focused element.
    pub(super) current: Option<FocusId>,
    pub(super) modality: Modality,
    /// Focus as the adapter last settled it, to notice focus that moved to
    /// an element of this window.
    settled: Option<FocusId>,
}

/// The kind of input that last could move focus, which decides whether
/// focus shows its ring (`ElementContext::focus_visible`), as CSS
/// `:focus-visible` does. App code moving focus keeps it as is, as
/// programmatic focus in browsers follows the last input.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Modality {
    /// A key or assistive tech: focus shows its ring.
    #[default]
    Keyboard,
    /// A pointer press: a clicked button or dragged divider shows none.
    Pointer,
}

impl FocusState {
    /// Focus `target`. True when focus moved.
    pub(super) fn set(&mut self, target: Option<FocusId>) -> bool {
        let moved = self.current != target;
        self.current = target;
        moved
    }

    /// Whether focus shows its ring.
    pub(super) fn is_visible(&self) -> bool {
        self.modality == Modality::Keyboard
    }

    /// Settle focus where it is now: the element it moved onto since it
    /// last settled, if it moved onto one.
    pub(super) fn settle(&mut self) -> Option<FocusId> {
        if self.current == self.settled {
            return None;
        }
        self.settled = self.current;
        self.current
    }
}

impl UiContext<'_, '_> {
    /// The window's focus.
    pub fn focus(&self) -> Option<FocusId> {
        self.windows.current.focus.current
    }

    /// The focus `window` remembers, whether or not it is the active
    /// window.
    pub fn focus_in(&self, window: WindowHandle) -> Option<FocusId> {
        self.windows.get(window)?.focus.current
    }

    /// Focus `focus` in this window.
    pub fn set_focus(&mut self, focus: Option<FocusId>) {
        if self.windows.current.focus.set(focus) {
            redraw(Redraw::Focus, self.windows.handle, self.window);
        }
    }

    /// Focus `focus` in `window`, which may be another window than the
    /// context's: a panel moved there and keeps its focus. Focus landing on
    /// an element ends IME compositions in every other window, so text
    /// they still deliver cannot reach it. Activating the native window is
    /// up to the caller. Works for a window just opened, before its first
    /// frame; stale handles are ignored.
    pub fn set_focus_in(&mut self, window: WindowHandle, focus: Option<FocusId>) {
        if self.windows.handle == Some(window) {
            self.set_focus(focus);
            return;
        }
        let Some(state) = self.state_in(window) else {
            return;
        };
        if state.focus.set(focus) {
            self.shared.refocused.push(window);
            redraw(Redraw::Focus, Some(window), self.window);
        }
    }
}

impl<U: UiApp> UiAdapter<U> {
    /// Focus `target` in the current window, as input does.
    pub(super) fn focus_on(&mut self, target: FocusId, cx: &mut EventContext) {
        if self.win.focus.set(Some(target)) {
            redraw(Redraw::Focus, self.win_handle, cx);
        }
    }

    /// Make focus show its ring from now on (see [`Modality`]).
    pub(super) fn show_focus(&mut self, cx: &mut EventContext) {
        let focus = &mut self.win.focus;
        if focus.modality == Modality::Pointer {
            focus.modality = Modality::Keyboard;
            if focus.current.is_some() {
                redraw(Redraw::Focus, self.win_handle, cx);
            }
        }
    }
}
