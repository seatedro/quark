//! Drags that outlive their window's router: a tab torn out of one window
//! and dropped into another, or released over the desktop.
//!
//! A local drag lives in its window's [`InputRouter`] as pointer capture.
//! When the drag must continue where that router cannot follow (a native
//! transport takes the pointer, or the pointer is tracked across windows),
//! the host moves it into a [`DragSession`] with
//! [`InputRouter::hand_off_capture`]. From then on the router delivers
//! nothing to the drag's handler and ordinary window blur no longer
//! cancels it; the session's owner ends it with [`DragSession::finish`] or
//! [`DragSession::cancel`], which reach the handler as
//! [`DragHandler::on_session_drop`] or [`DragHandler::on_cancel`].
//!
//! The session knows nothing about native windows. The host names windows
//! with opaque [`DragWindowId`]s, turns pointer positions into a
//! [`DragLocation`], and passes the [`DropTargets`] of the window under
//! the pointer; [`resolve_drop`] turns that into a [`DragOutcome`].

use std::any::Any;

use super::*;

/// A window as the host names it to drag sessions. quark compares ids
/// only for equality; the host maps them to its own window handles.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DragWindowId(pub u64);

/// Where a session's pointer is, as the host's platform layer found it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragLocation {
    /// Over an owned window, at a point in its logical client coordinates.
    Window {
        window: DragWindowId,
        point: (f32, f32),
    },
    /// Confirmed outside every owned window.
    Outside,
    /// The platform cannot say, such as after the pointer left every window
    /// on a platform without desktop coordinates.
    Unknown,
}

/// Why a drop target under the pointer refuses the drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// [`DropPolicy::accepts`] said no.
    Refused,
    /// The target was painted from another revision of its model than the
    /// current one, or its model is gone. Repaint the window and resolve
    /// again.
    StaleRevision,
    /// At release, the layout under the pointer was no longer the one the
    /// last resolution saw (and the user was shown).
    Moved,
}

/// What a drag would do if released now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragOutcome {
    /// Over a target that takes the drop.
    Target {
        window: DragWindowId,
        hit: DropTargetHit,
    },
    /// Over a target that refuses it. Never a detachment: a release here
    /// drops nothing.
    Rejected {
        window: DragWindowId,
        hit: DropTargetHit,
        reason: RejectReason,
    },
    /// Over an owned window, but not over any of its targets.
    NoTarget { window: DragWindowId },
    /// Outside every owned window: the only outcome that may detach.
    Outside,
    /// The location is unknown.
    Unknown,
}

impl DragOutcome {
    /// The owned window under the pointer, if any.
    pub fn window(&self) -> Option<DragWindowId> {
        match *self {
            Self::Target { window, .. }
            | Self::Rejected { window, .. }
            | Self::NoTarget { window } => Some(window),
            Self::Outside | Self::Unknown => None,
        }
    }

    /// The target under the pointer, accepted or not.
    pub fn hit(&self) -> Option<&DropTargetHit> {
        match self {
            Self::Target { hit, .. } | Self::Rejected { hit, .. } => Some(hit),
            _ => None,
        }
    }

    /// Whether the window under the pointer should be repainted before
    /// resolving again: its targets are older than their models.
    pub fn is_stale(&self) -> bool {
        matches!(
            self,
            Self::Rejected {
                reason: RejectReason::StaleRevision | RejectReason::Moved,
                ..
            }
        )
    }
}

/// The app's rules for a drag's payload, consulted on every resolution.
pub trait DropPolicy {
    /// The current revision of the model `target` was painted from, or
    /// `None` when that model is gone. A hit painted at another revision
    /// is stale.
    fn revision(&self, target: DropTargetId) -> Option<u64>;
    /// Whether the target under the pointer takes `payload`.
    fn accepts(&self, hit: &DropTargetHit, payload: &dyn Any) -> bool;
}

/// Resolve `location` to an outcome. `targets` are the drop targets of the
/// last completed frame of the location's window, `None` when it has not
/// painted (which finds no target). A target counts only at the revision
/// `policy` reports for it, so geometry painted before a model change is
/// refused rather than trusted.
pub fn resolve_drop(
    location: DragLocation,
    targets: Option<&DropTargets>,
    payload: &dyn Any,
    policy: &(impl DropPolicy + ?Sized),
) -> DragOutcome {
    let (window, (x, y)) = match location {
        DragLocation::Window { window, point } => (window, point),
        DragLocation::Outside => return DragOutcome::Outside,
        DragLocation::Unknown => return DragOutcome::Unknown,
    };
    let Some(hit) = targets.and_then(|targets| targets.at(x, y)) else {
        return DragOutcome::NoTarget { window };
    };
    let reason = if policy.revision(hit.id) != Some(hit.revision) {
        RejectReason::StaleRevision
    } else if !policy.accepts(&hit, payload) {
        RejectReason::Refused
    } else {
        return DragOutcome::Target { window, hit };
    };
    DragOutcome::Rejected {
        window,
        hit,
        reason,
    }
}

/// What a [`DragHandler`] hands to a session from
/// [`DragHandler::on_handoff`].
pub struct DragHandoff {
    /// What the drag carries, for the session's owner to downcast.
    pub payload: Box<dyn Any>,
    /// Delivered with the handoff, from the handler's node.
    pub actions: Vec<Action>,
}

/// Why [`InputRouter::hand_off_capture`] handed nothing off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffError {
    /// No drag holds the pointer.
    NoCapture,
    /// The drag holding the pointer stays local
    /// ([`DragHandler::on_handoff`] returned `None`); it keeps the pointer.
    Refused,
}

/// How a session ended.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DragResult {
    /// Released, resolved to this outcome. Only [`DragOutcome::Target`]
    /// should be committed, and only [`DragOutcome::Outside`] may detach.
    Dropped(DragOutcome),
    /// Cancelled: nothing is committed.
    Cancelled,
}

/// A finished session: its result, its payload back, and what the handler
/// it was handed off from returned.
pub struct DragEnd {
    pub result: DragResult,
    pub payload: Box<dyn Any>,
    pub actions: Vec<Action>,
}

/// A drag owned by the app rather than a window's router; see the module
/// docs. It must end with [`Self::finish`] or [`Self::cancel`], which end
/// its handler; dropping it ends nothing.
pub struct DragSession {
    source: DragWindowId,
    payload: Box<dyn Any>,
    hotspot: (f32, f32),
    location: DragLocation,
    outcome: DragOutcome,
    preview: Option<DragPreview>,
    handler: Option<Box<dyn DragHandler>>,
}

impl DragSession {
    /// A session with no handler behind it (a keyboard move, a drag from
    /// a native transport), starting at `location` with no outcome yet.
    pub fn new(source: DragWindowId, payload: Box<dyn Any>, location: DragLocation) -> Self {
        Self {
            source,
            payload,
            hotspot: (0.0, 0.0),
            location,
            outcome: DragOutcome::Unknown,
            preview: None,
            handler: None,
        }
    }

    /// The session [`InputRouter::hand_off_capture`] makes: the handler's
    /// payload and a copy of its preview, at the pointer in `source`.
    pub(crate) fn handed_off(
        source: DragWindowId,
        payload: Box<dyn Any>,
        pointer: (f32, f32),
        handler: Box<dyn DragHandler>,
    ) -> Self {
        let mut session = Self::new(
            source,
            payload,
            DragLocation::Window {
                window: source,
                point: pointer,
            },
        );
        if let Some(preview) = handler.preview() {
            session = session.with_preview(preview.clone());
        }
        session.handler = Some(handler);
        session
    }

    /// Show `preview` under the pointer while the session lasts; the
    /// session's hotspot becomes the preview's.
    pub fn with_preview(mut self, preview: DragPreview) -> Self {
        self.hotspot = preview.hotspot;
        self.preview = Some(preview);
        self
    }

    pub fn source(&self) -> DragWindowId {
        self.source
    }

    pub fn payload(&self) -> &dyn Any {
        &*self.payload
    }

    /// The point of the dragged item held under the pointer, from its top
    /// left, in points: where a window made from the drop goes relative to
    /// the pointer.
    pub fn hotspot(&self) -> (f32, f32) {
        self.hotspot
    }

    /// Set the hotspot, and the preview's with it.
    pub fn set_hotspot(&mut self, x: f32, y: f32) {
        self.hotspot = (x, y);
        if let Some(preview) = &mut self.preview {
            preview.set_hotspot(x, y);
        }
    }

    pub fn location(&self) -> DragLocation {
        self.location
    }

    /// The outcome of the last [`Self::update`]; [`DragOutcome::Unknown`]
    /// before the first.
    pub fn outcome(&self) -> &DragOutcome {
        &self.outcome
    }

    /// The preview and the point its top left goes at in `window`, while
    /// the pointer is over that window. Paint it with a
    /// [`DragPreviewLayer`]: it stays paint only, and it shows whether or
    /// not the element that started the drag is still painted.
    pub fn preview_in(&self, window: DragWindowId) -> Option<(&DragPreview, (f32, f32))> {
        let preview = self.preview.as_ref()?;
        match self.location {
            DragLocation::Window { window: w, point } if w == window => {
                Some((preview, preview.origin_at(point)))
            }
            _ => None,
        }
    }

    /// The pointer moved to `location`: resolve it (see [`resolve_drop`])
    /// and remember the result as what the user is shown.
    pub fn update(
        &mut self,
        location: DragLocation,
        targets: Option<&DropTargets>,
        policy: &(impl DropPolicy + ?Sized),
    ) -> &DragOutcome {
        self.location = location;
        self.outcome = resolve_drop(location, targets, &*self.payload, policy);
        &self.outcome
    }

    /// Release at `release`, or where the last update left the pointer.
    /// The location is resolved again against `targets` (its window's
    /// current ones), never taken from memory. At the last update's
    /// location, a target that differs from the one that update found
    /// (the layout moved under a still pointer) is refused as
    /// [`RejectReason::Moved`]. The handler gets
    /// [`DragHandler::on_session_drop`] with the outcome.
    pub fn finish(
        mut self,
        release: Option<DragLocation>,
        targets: Option<&DropTargets>,
        policy: &(impl DropPolicy + ?Sized),
    ) -> DragEnd {
        let location = release.unwrap_or(self.location);
        let mut outcome = resolve_drop(location, targets, &*self.payload, policy);
        if location == self.location
            && let Some(window) = outcome.window()
        {
            let place = |o: &DragOutcome| o.hit().map(|h| (h.id, h.revision, h.slot));
            if place(&outcome) != place(&self.outcome)
                && let Some(hit) = outcome.hit().or(self.outcome.hit()).copied()
            {
                outcome = DragOutcome::Rejected {
                    window,
                    hit,
                    reason: RejectReason::Moved,
                };
            }
        }
        let actions = match &mut self.handler {
            Some(handler) => handler.on_session_drop(&outcome),
            None => Vec::new(),
        };
        DragEnd {
            result: DragResult::Dropped(outcome),
            payload: self.payload,
            actions,
        }
    }

    /// End without a drop (Escape, the source went away, the transport
    /// gave up). The handler gets [`DragHandler::on_cancel`], as a local
    /// drag would.
    pub fn cancel(mut self) -> DragEnd {
        let actions = match &mut self.handler {
            Some(handler) => handler.on_cancel(),
            None => Vec::new(),
        };
        DragEnd {
            result: DragResult::Cancelled,
            payload: self.payload,
            actions,
        }
    }
}

impl std::fmt::Debug for DragSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DragSession")
            .field("source", &self.source)
            .field("hotspot", &self.hotspot)
            .field("location", &self.location)
            .field("outcome", &self.outcome)
            .field("preview", &self.preview)
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::super::drop_targets::tests::{paint, target};
    use quark_render::scene::Primitive;

    use super::*;
    use crate::accessibility::dump_accessibility;
    use crate::theme::Theme;

    const SOURCE: DragWindowId = DragWindowId(1);
    const OTHER: DragWindowId = DragWindowId(2);

    /// Targets report revision `revision`; target `refuse` takes nothing.
    struct Rules {
        revision: u64,
        refuse: Option<u64>,
    }

    const OPEN: Rules = Rules {
        revision: 0,
        refuse: None,
    };

    impl DropPolicy for Rules {
        fn revision(&self, _target: DropTargetId) -> Option<u64> {
            Some(self.revision)
        }

        fn accepts(&self, hit: &DropTargetHit, payload: &dyn Any) -> bool {
            payload.is::<&str>() && self.refuse != Some(hit.id.key)
        }
    }

    fn at(window: DragWindowId, x: f32, y: f32) -> DragLocation {
        DragLocation::Window {
            window,
            point: (x, y),
        }
    }

    fn describe(outcome: &DragOutcome) -> String {
        match outcome {
            DragOutcome::Target { window, hit } => format!("target {} in {}", hit.id.key, window.0),
            DragOutcome::Rejected { hit, reason, .. } => {
                format!("rejected {} {reason:?}", hit.id.key)
            }
            DragOutcome::NoTarget { window } => format!("none in {}", window.0),
            DragOutcome::Outside => "outside".to_owned(),
            DragOutcome::Unknown => "unknown".to_owned(),
        }
    }

    /// Two 100x100 targets side by side at revision 0; the first `first`
    /// wide, so a smaller one shifts the second under a still pointer.
    fn row(first: f32) -> DropTargets {
        paint(
            div()
                .w(400.0)
                .h(300.0)
                .flex_row()
                .child(
                    div()
                        .w(first)
                        .h(100.0)
                        .child(target(1, 0, &[], (first, 100.0))),
                )
                .child(
                    div()
                        .w(100.0)
                        .h(100.0)
                        .child(target(2, 0, &[], (100.0, 100.0))),
                ),
        )
    }

    // Catches outcomes merged together: a refused or stale target that
    // reads as "no target" (a detach on release) or as accepted, or an
    // unpainted window that finds targets.
    #[test]
    fn resolve_drop_tells_every_outcome_apart() {
        let targets = row(100.0);
        let refuse_2 = Rules {
            revision: 0,
            refuse: Some(2),
        };
        let newer = Rules {
            revision: 1,
            refuse: None,
        };
        let cases = [
            (
                "accepted",
                at(OTHER, 50.0, 50.0),
                Some(&targets),
                &OPEN,
                "target 1 in 2",
            ),
            (
                "refused",
                at(OTHER, 150.0, 50.0),
                Some(&targets),
                &refuse_2,
                "rejected 2 Refused",
            ),
            (
                "stale",
                at(OTHER, 50.0, 50.0),
                Some(&targets),
                &newer,
                "rejected 1 StaleRevision",
            ),
            (
                "between targets",
                at(OTHER, 300.0, 50.0),
                Some(&targets),
                &OPEN,
                "none in 2",
            ),
            (
                "unpainted window",
                at(OTHER, 50.0, 50.0),
                None,
                &OPEN,
                "none in 2",
            ),
            (
                "outside",
                DragLocation::Outside,
                Some(&targets),
                &OPEN,
                "outside",
            ),
            (
                "unknown",
                DragLocation::Unknown,
                Some(&targets),
                &OPEN,
                "unknown",
            ),
        ];
        for (name, location, targets, rules, expected) in cases {
            let outcome = resolve_drop(location, targets, &"tab", rules);
            assert_eq!(describe(&outcome), expected, "{name}");
        }
    }

    // Catches a release committing the target remembered from the last
    // move after the layout moved under the still pointer, which drops
    // the tab somewhere the user was never shown.
    #[test]
    fn finish_refuses_a_target_that_moved_since_it_was_shown() {
        let shown = row(100.0);
        let cases = [
            ("repainted unchanged", row(100.0), None, "target 1 in 2"),
            (
                "shifted under the pointer",
                row(20.0),
                None,
                "rejected 2 Moved",
            ),
            (
                "released elsewhere",
                row(20.0),
                Some(at(OTHER, 100.0, 50.0)),
                "target 2 in 2",
            ),
        ];
        for (name, now, release, expected) in cases {
            let mut session = DragSession::new(SOURCE, Box::new("tab"), at(SOURCE, 0.0, 0.0));
            session.update(at(OTHER, 50.0, 50.0), Some(&shown), &OPEN);
            let end = session.finish(release, Some(&now), &OPEN);
            let DragResult::Dropped(outcome) = end.result else {
                panic!("{name}: cancelled");
            };
            assert_eq!(describe(&outcome), expected, "{name}");
        }
    }

    /// A drag with a 40x20 preview held at (5, 5), handed off on request.
    struct TabDrag(DragPreview);

    impl DragHandler for TabDrag {
        fn on_move(&mut self, _x: f32, _y: f32) -> Vec<Action> {
            Vec::new()
        }

        fn on_release(&mut self) -> DragReleaseResult {
            DragReleaseResult::empty()
        }

        fn on_cancel(&mut self) -> Vec<Action> {
            Vec::new()
        }

        fn preview(&self) -> Option<&DragPreview> {
            Some(&self.0)
        }

        fn on_handoff(&mut self) -> Option<DragHandoff> {
            Some(DragHandoff {
                payload: Box::new("tab"),
                actions: Vec::new(),
            })
        }
    }

    // Catches a session preview that still depends on its source element
    // being painted, follows the source window instead of the pointer, or
    // gains hits or accessibility nodes in the window it is painted in.
    #[test]
    fn a_handed_off_preview_follows_the_pointer_after_its_source_is_gone() {
        let preview = DragPreview::new(1, (40.0, 20.0), |_, (w, h)| {
            div().w(w).h(h).bg(Color::rgba(255, 0, 0, 255)).into_any()
        })
        .hotspot(5.0, 5.0);
        let source = div().w(400.0).h(300.0).child(
            div()
                .w(50.0)
                .h(50.0)
                .test_id("tab")
                .on_drag(move |_| Box::new(TabDrag(preview.clone()))),
        );
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut router = InputRouter::default();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        cx.semantic = SemanticFrame::new(400.0, 300.0);
        render_element(
            &mut source.into_any(),
            &mut Scene::default(),
            &mut cx,
            400.0,
            300.0,
        );
        router.set_frame(cx.take_input_frame());
        router.pointer_down(10.0, 10.0, &mut None);
        router.pointer_move(100.0, 100.0);
        let (mut session, _) = router.hand_off_capture(SOURCE).expect("handed off");
        session.update(at(OTHER, 60.0, 70.0), None, &OPEN);

        // The other window paints a frame the tab was never in.
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        cx.semantic = SemanticFrame::new(400.0, 300.0);
        cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
        let mut scene = Scene::default();
        let root = div().w(400.0).h(300.0).accessibility_label("Other");
        render_element(&mut root.into_any(), &mut scene, &mut cx, 400.0, 300.0);
        let (hits, nodes) = (cx.hit_table.len(), dump_accessibility(&cx.accessibility));
        let mut layer = DragPreviewLayer::default();
        layer.paint(session.preview_in(OTHER), &mut scene, &mut cx);

        let rects: Vec<Rect> = scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                Primitive::Rect(r) => Some(r.rect),
                Primitive::RoundedRect(r) => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(
            rects,
            [Rect {
                x: 55.0,
                y: 65.0,
                width: 40.0,
                height: 20.0
            }]
        );
        assert!(session.preview_in(SOURCE).is_none(), "the pointer left it");
        assert_eq!(cx.hit_table.len(), hits);
        assert_eq!(dump_accessibility(&cx.accessibility), nodes);
    }
}
