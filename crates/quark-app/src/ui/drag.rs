//! The adapter's drag session: one drag at a time that belongs to the app
//! rather than to a window, such as a tab torn out of one window and held
//! over another. See [`quark_ui::element::DragSession`].
//!
//! The adapter holds the session, paints its preview in the window it is
//! over, and resolves it against that window's last completed drop
//! targets. Moving it is up to the app (or the platform transport it
//! drives): [`UiContext::update_drag`](super::UiContext::update_drag) with
//! a [`DragLocation`] per pointer move, then
//! [`UiContext::finish_drag`](super::UiContext::finish_drag). The adapter
//! ends it by itself, cancelled, on Escape, when its source window closes,
//! and on a release the app did not finish it on.

use std::any::Any;

use quark_ui::Action;
use quark_ui::element::{
    DragEnd, DragLocation, DragOutcome, DragSession, DragWindowId, DropPolicy, DropTargetHit,
    DropTargetId, DropTargets,
};

use super::window::WindowStates;
use crate::{EventContext, WindowHandle};

/// How drag sessions name `window`.
pub fn drag_window_id(window: WindowHandle) -> DragWindowId {
    DragWindowId(window.scope_id())
}

/// The session location for `point`, in logical points, over `window`.
pub fn drag_location(window: WindowHandle, point: (f32, f32)) -> DragLocation {
    DragLocation::Window {
        window: drag_window_id(window),
        point,
    }
}

pub(super) struct ActiveDrag {
    pub(super) session: DragSession,
    /// The window the drag started in, if it is one of the adapter's.
    pub(super) source: Option<WindowHandle>,
}

/// Resolves locations that need no targets: unknown, outside, or over a
/// window that painted none.
struct NoTargets;

impl DropPolicy for NoTargets {
    fn revision(&self, _target: DropTargetId) -> Option<u64> {
        None
    }

    fn accepts(&self, _hit: &DropTargetHit, _payload: &dyn Any) -> bool {
        false
    }
}

/// The window `id` names, if it has state.
pub(super) fn window_of(windows: &WindowStates, id: DragWindowId) -> Option<WindowHandle> {
    windows
        .handles()
        .find(|&window| drag_window_id(window) == id)
}

fn window_at(windows: &WindowStates, location: DragLocation) -> Option<WindowHandle> {
    match location {
        DragLocation::Window { window, .. } => window_of(windows, window),
        DragLocation::Outside | DragLocation::Unknown => None,
    }
}

/// The last completed drop targets of the window under `location`.
fn targets_at<'s>(windows: &'s WindowStates, location: DragLocation) -> Option<&'s DropTargets> {
    let window = window_at(windows, location)?;
    windows
        .get(window)
        .map(|state| &state.router.frame().drop_targets)
}

fn redraw_at(windows: &WindowStates, location: DragLocation, cx: &mut EventContext) {
    if let Some(window) = window_at(windows, location) {
        cx.request_redraw_window(window);
    }
}

/// Start `session`, from the window `source` if it is one of the
/// adapter's; the session it replaces, if any, ends cancelled.
pub(super) fn start(
    drag: &mut Option<ActiveDrag>,
    windows: &WindowStates,
    cx: &mut EventContext,
    queued: &mut Vec<Action>,
    session: DragSession,
) -> Option<DragEnd> {
    let replaced = cancel(drag, windows, cx, queued);
    let source = window_of(windows, session.source());
    redraw_at(windows, session.location(), cx);
    *drag = Some(ActiveDrag { session, source });
    replaced
}

/// Move the session to `location`, repainting the windows its preview
/// left and entered, and a window whose targets turned out stale.
pub(super) fn update(
    drag: &mut Option<ActiveDrag>,
    windows: &WindowStates,
    cx: &mut EventContext,
    location: DragLocation,
    policy: &(impl DropPolicy + ?Sized),
) -> Option<DragOutcome> {
    let session = &mut drag.as_mut()?.session;
    let before = session.location();
    let targets = targets_at(windows, location);
    let outcome = *session.update(location, targets, policy);
    let has_preview = |location| match location {
        DragLocation::Window { window, .. } => session.preview_in(window).is_some(),
        _ => false,
    };
    if before != location && has_preview(location) {
        redraw_at(windows, before, cx);
    }
    if has_preview(location) || outcome.is_stale() {
        redraw_at(windows, location, cx);
    }
    Some(outcome)
}

/// Release the session at `release` (or where it is) and resolve the drop.
/// The handler's actions go to `queued`.
pub(super) fn finish(
    drag: &mut Option<ActiveDrag>,
    windows: &WindowStates,
    cx: &mut EventContext,
    queued: &mut Vec<Action>,
    release: Option<DragLocation>,
    policy: &(impl DropPolicy + ?Sized),
) -> Option<DragEnd> {
    let ActiveDrag { session, source } = drag.take()?;
    let shown = session.location();
    let targets = targets_at(windows, release.unwrap_or(shown));
    let mut end = session.finish(release, targets, policy);
    queued.append(&mut end.actions);
    ended(windows, cx, shown, source);
    Some(end)
}

/// End the session without a drop. The handler's actions go to `queued`.
pub(super) fn cancel(
    drag: &mut Option<ActiveDrag>,
    windows: &WindowStates,
    cx: &mut EventContext,
    queued: &mut Vec<Action>,
) -> Option<DragEnd> {
    let ActiveDrag { session, source } = drag.take()?;
    let shown = session.location();
    let mut end = session.cancel();
    queued.append(&mut end.actions);
    ended(windows, cx, shown, source);
    Some(end)
}

/// Repaint the window the ended session's preview was over, and its
/// source, whose drag state its handler's actions change.
fn ended(
    windows: &WindowStates,
    cx: &mut EventContext,
    shown: DragLocation,
    source: Option<WindowHandle>,
) {
    redraw_at(windows, shown, cx);
    if let Some(source) = source {
        cx.request_redraw_window(source);
    }
}

/// `window` is closing: a session over it is now nowhere known. Returns
/// whether the session started there, so it must end.
pub(super) fn window_closing(drag: &mut Option<ActiveDrag>, window: WindowHandle) -> bool {
    let Some(active) = drag.as_mut() else {
        return false;
    };
    if active.source == Some(window) {
        return true;
    }
    if matches!(active.session.location(), DragLocation::Window { window: w, .. } if w == drag_window_id(window))
    {
        active
            .session
            .update(DragLocation::Unknown, None, &NoTargets);
    }
    false
}
