//! Accessibility: each window's tree as accesskit takes it, requests from
//! assistive tech routed to the app, and announcements.

use accesskit::{Action as AxAction, ActionData, ActionRequest, Affine, TreeUpdate};
use quark_ui::accessibility::{AccessibilityAction, AccessibilityFrame, Politeness};
use quark_ui::text_input::TextEditCommand;
use quark_ui::{Action, FocusId};

use super::{Redraw, UiAdapter, UiApp, UiContext, redraw};
use crate::{EventContext, WindowHandle};

/// Lines scrolled per accessibility ScrollUp/ScrollDown request.
const ACCESSIBILITY_SCROLL_LINES: i32 = 3;

impl UiContext<'_, '_> {
    /// Have assistive tech speak `text`, for changes nothing on screen
    /// names: a finished response, a background save. Each call is spoken
    /// once, even when it repeats the last text. For text that is on
    /// screen (a toast), make its element a live region instead
    /// (`Div::live`).
    pub fn announce(&mut self, text: impl Into<String>, politeness: Politeness) {
        self.windows.current.announcer.announce(text, politeness);
        // The announcement is published with the next frame's tree.
        redraw(Redraw::Announce, self.windows.handle, self.window);
    }

    /// [`Self::announce`] through `window`'s tree, as for a panel that
    /// just moved there.
    pub fn announce_in(
        &mut self,
        window: WindowHandle,
        text: impl Into<String>,
        politeness: Politeness,
    ) {
        if let Some(state) = self.state_in(window) {
            state.announcer.announce(text, politeness);
            redraw(Redraw::Announce, Some(window), self.window);
        }
    }

    /// Name the window's accessibility tree, as assistive tech lists it
    /// among the app's windows. Defaults to the adapter's name.
    pub fn set_window_name(&mut self, name: impl Into<String>) {
        self.windows.current.name = Some(name.into());
        redraw(Redraw::Announce, self.windows.handle, self.window);
    }

    /// [`Self::set_window_name`] for `window`, which may be another window
    /// than the context's, or one still opening.
    pub fn set_window_name_in(&mut self, window: WindowHandle, name: impl Into<String>) {
        if let Some(state) = self.state_in(window) {
            state.name = Some(name.into());
            redraw(Redraw::Announce, Some(window), self.window);
        }
    }
}

/// accesskit wants bounds in physical pixels once every transform is
/// applied. The tree is built in points, so one scale on the root converts
/// the whole tree.
pub(super) fn scale_tree(update: &mut TreeUpdate, scale: f32) {
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
        (AxAction::SetValue, AccessibilityAction::Numeric(numeric)) => match &request.data {
            Some(ActionData::NumericValue(value)) => Some(Routed::Dispatch(numeric.set(*value))),
            _ => None,
        },
        (AxAction::Increment, AccessibilityAction::Numeric(numeric)) => {
            numeric.increment().cloned().map(Routed::Dispatch)
        }
        (AxAction::Decrement, AccessibilityAction::Numeric(numeric)) => {
            numeric.decrement().cloned().map(Routed::Dispatch)
        }
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

impl<U: UiApp> UiAdapter<U> {
    /// `window`'s last painted accessibility tree, in points.
    pub(crate) fn logical_accessibility_tree(&self, window: WindowHandle) -> Option<TreeUpdate> {
        let state = self.window_states(window)?;
        let name = state.name.as_deref().unwrap_or(&self.shared.name);
        let mut update = state.accessibility.tree_update(name, state.focus.current);
        state.announcer.publish(&mut update);
        Some(update)
    }

    pub(super) fn handle_accessibility_action(
        &mut self,
        request: ActionRequest,
        cx: &mut EventContext,
    ) {
        // Assistive tech users find their place by the ring, as keyboard
        // users do.
        self.show_focus(cx);
        match route_accessibility(&self.win.accessibility, &request) {
            Some(Routed::Dispatch(action)) => self.dispatch(vec![action], cx),
            Some(Routed::Click(action, focus)) => {
                if let Some(focus) = focus {
                    self.focus_on(focus, cx);
                }
                self.dispatch(vec![action], cx);
            }
            Some(Routed::Focus(focus)) => self.focus_on(focus, cx),
            Some(Routed::SetValue(target, value)) => {
                self.with_app(cx, |app, ucx| app.set_text_value(target, value, ucx));
                redraw(Redraw::TextEdit, self.win_handle, cx);
            }
            Some(Routed::Select {
                target,
                anchor,
                focus,
            }) => {
                let cursor = TextEditCommand::SetTextCursor(anchor);
                Self::edit_text_in(&mut self.app, self.win_handle, target, cursor);
                self.edit(target, TextEditCommand::ExtendTextSelection(focus), cx);
                // The cursor move alone may have changed the selection.
                redraw(Redraw::TextEdit, self.win_handle, cx);
            }
            Some(Routed::Edit(target, command)) => self.edit(target, command, cx),
            None => tracing::debug!("unhandled accessibility request: {request:?}"),
        }
    }
}
