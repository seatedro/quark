//! Pointer presses, the drag that captures the pointer from a press to its
//! release, middle clicks, and context menu clicks.

use super::*;

/// The drag holding the pointer. Frames come and go during a drag, so it
/// remembers its node by identity and finds it again in each new frame.
pub(super) struct Capture {
    identity: u64,
    /// The node in the current frame, if it is still there.
    node: Option<usize>,
    drag: Box<dyn DragHandler>,
    pointer: (f32, f32),
    travel: Travel,
}

impl Capture {
    /// Find the drag's node in a new frame, whose nodes have `identities`.
    pub(super) fn follow(&mut self, identities: &[u64], geometry: &LayoutSnapshot) {
        self.node = identities.iter().position(|id| *id == self.identity);
        self.drag.set_geometry(geometry);
    }

    /// Whether the drag's preview is on screen.
    fn shows_preview(&self) -> bool {
        self.travel == Travel::Dragging && self.drag.preview().is_some()
    }

    /// `actions` the drag emitted, from its node. A preview on screen, or
    /// a scroll handle moved since `epoch`, asks for a repaint.
    fn deliver(&self, actions: Vec<Action>, epoch: u64) -> Delivery {
        Delivery {
            node: self.node,
            actions,
            redraw: scroll_epoch() != epoch || self.shows_preview(),
            window_drag: false,
        }
    }
}

/// How far a captured pointer has gone from its press.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Travel {
    /// Within [`DRAG_PREVIEW_THRESHOLD`] of the press, at this window
    /// point.
    Pressed((f32, f32)),
    /// Past it, for the rest of the drag: its preview shows.
    Dragging,
}

impl Travel {
    /// The pointer moved to window point `(x, y)`.
    fn after_move(self, (x, y): (f32, f32)) -> Self {
        match self {
            Self::Pressed((px, py)) if (x - px).hypot(y - py) < DRAG_PREVIEW_THRESHOLD => self,
            Self::Pressed(_) | Self::Dragging => Self::Dragging,
        }
    }
}

impl InputRouter {
    /// Whether a primary press at `(x, y)` should move the window: it lands
    /// in a [`Div::window_drag_region`] with no control above it taking the
    /// press. Hosts ask after [`Self::pointer_down`] delivered nothing and
    /// no selectable text took the press, then start the platform's native
    /// window move while the press is still current.
    pub fn window_drag_at(&self, x: f32, y: f32) -> bool {
        self.capture.is_none() && self.frame.hits.window_drag_at(x, y)
    }

    pub fn cursor_at(&self, x: f32, y: f32) -> CursorHint {
        if let Some(capture) = &self.capture {
            return capture.drag.cursor();
        }
        self.cursor_over(&self.frame.hits.stack_at(x, y))
    }

    /// [`Self::cursor_at`] for `stack`, this frame's hit entries under the
    /// pointer topmost first, so a caller that keeps the stack allocates
    /// nothing.
    pub fn cursor_over(&self, stack: &[HitId]) -> CursorHint {
        if let Some(capture) = &self.capture {
            return capture.drag.cursor();
        }
        stack
            .iter()
            .filter_map(|&id| self.frame.hits.cursor(id))
            .find(|cursor| *cursor != CursorHint::Default)
            .unwrap_or_default()
    }

    /// Press: update `focus`, then start a drag (capturing the pointer and
    /// delivering its press actions) or deliver a click to the first handler
    /// on the route. A press stops any fling.
    pub fn pointer_down(&mut self, x: f32, y: f32, focus: &mut Option<FocusId>) -> Delivery {
        self.wheel.stop_fling();
        let target = self.target_at(x, y);
        if let Some(next) = self.focus_after_press(target) {
            *focus = next;
        }
        let Some(target) = target else {
            return Delivery {
                window_drag: self.frame.hits.window_drag_at(x, y),
                ..Delivery::default()
            };
        };
        let handlers = &self.frame.handlers;
        // The innermost node with a drag or click handler owns the press, so
        // a button inside a draggable panel still clicks.
        let drag = self.walk(target, UiEventKind::PointerDown, |node| {
            handlers.takes_press(node).then(Vec::new)
        });
        let epoch = scroll_epoch();
        let press = ClickEvent {
            x,
            y,
            modifiers: self.modifiers,
        };
        if let Some(node) = drag.node
            && let Some(mut drag) = handlers.start_drag(node, press)
        {
            drag.set_geometry(&self.frame.geometry);
            let actions = drag.on_press();
            self.capture = Some(Capture {
                identity: node_identities(&self.frame.semantic)[node],
                node: Some(node),
                drag,
                pointer: (x, y),
                travel: Travel::Pressed((x, y)),
            });
            return Delivery {
                redraw: scroll_epoch() != epoch,
                ..Delivery::handled(node, actions)
            };
        }
        let mut delivery = self.walk(target, UiEventKind::Click, |node| {
            handlers.click(node).map(|handler| handler.invoke(press))
        });
        delivery.window_drag = delivery.node.is_none() && self.frame.hits.window_drag_at(x, y);
        delivery
    }

    /// The innermost node on the route at `(x, y)` with a middle click
    /// handler.
    fn middle_target(&self, x: f32, y: f32) -> Option<usize> {
        let target = self.target_at(x, y)?;
        let handlers = &self.frame.handlers;
        self.walk(target, UiEventKind::Click, |node| {
            handlers.middle_click(node).map(|_| Vec::new())
        })
        .node
        .filter(|&node| handlers.middle_click(node).is_some())
    }

    /// Middle button press: remember which handler it landed on.
    pub fn middle_down(&mut self, x: f32, y: f32) {
        self.middle_press = self
            .middle_target(x, y)
            .map(|node| node_identities(&self.frame.semantic)[node]);
    }

    /// Middle button release: a click when it lands on the handler the
    /// press did, as in browsers' `auxclick`, so a press dragged off a tab
    /// does not close it.
    pub fn middle_up(&mut self, x: f32, y: f32) -> Delivery {
        let Some(pressed) = self.middle_press.take() else {
            return Delivery::default();
        };
        let Some(node) = self.middle_target(x, y) else {
            return Delivery::default();
        };
        if node_identities(&self.frame.semantic)[node] != pressed {
            return Delivery::default();
        }
        let actions = self.frame.handlers.middle_click(node).cloned();
        Delivery::handled(node, actions.into_iter().collect())
    }

    /// A context menu click at `(x, y)`: the innermost handler on the
    /// route gets the pointer position. Focus does not move.
    pub fn context_menu_at(&self, x: f32, y: f32) -> Delivery {
        let Some(target) = self.target_at(x, y) else {
            return Delivery::default();
        };
        let handlers = &self.frame.handlers;
        let event = ContextMenuEvent {
            x,
            y,
            keyboard: false,
        };
        self.walk(target, UiEventKind::ContextMenu, |node| {
            handlers
                .context_menu(node)
                .map(|handler| vec![handler.invoke(event)])
        })
    }

    /// Moves go to the capturing drag, wherever the pointer is. A drag
    /// showing a preview redraws on every move, so the preview follows.
    pub fn pointer_move(&mut self, x: f32, y: f32) -> Delivery {
        let epoch = scroll_epoch();
        match &mut self.capture {
            Some(capture) => {
                capture.pointer = (x, y);
                capture.travel = capture.travel.after_move((x, y));
                capture.drag.set_modifiers(self.modifiers);
                let actions = capture.drag.on_move(x, y);
                capture.deliver(actions, epoch)
            }
            None => Delivery::default(),
        }
    }

    /// Release ends pointer capture.
    pub fn pointer_up(&mut self) -> Delivery {
        let epoch = scroll_epoch();
        match self.capture.take() {
            Some(mut capture) => {
                capture.drag.set_modifiers(self.modifiers);
                let actions = capture.drag.on_release().actions;
                capture.deliver(actions, epoch)
            }
            None => Delivery::default(),
        }
    }

    /// End pointer capture without a release: the platform sends none once
    /// the window has lost focus, the app cancelled the drag, or a native
    /// drag took the pointer. The drag gets [`DragHandler::on_cancel`], never
    /// its release, so cancelling commits no drop.
    pub fn cancel_pointer(&mut self) -> Delivery {
        self.wheel.forget_lines();
        let epoch = scroll_epoch();
        match self.capture.take() {
            Some(mut capture) => {
                let actions = capture.drag.on_cancel();
                capture.deliver(actions, epoch)
            }
            None => Delivery::default(),
        }
    }

    /// Move the drag holding the pointer into a [`DragSession`] started in
    /// window `source`, when its handler agrees
    /// ([`DragHandler::on_handoff`]). The router lets go of the pointer:
    /// later moves, releases, and [`Self::cancel_pointer`] reach the
    /// handler no more, and its preview leaves this router for the
    /// session. The delivery carries the handler's handoff actions and
    /// asks for a repaint when a preview was showing.
    pub fn hand_off_capture(
        &mut self,
        source: DragWindowId,
    ) -> Result<(DragSession, Delivery), HandoffError> {
        let capture = self.capture.as_mut().ok_or(HandoffError::NoCapture)?;
        let epoch = scroll_epoch();
        let handoff = capture.drag.on_handoff().ok_or(HandoffError::Refused)?;
        let Some(capture) = self.capture.take() else {
            return Err(HandoffError::NoCapture);
        };
        let delivery = capture.deliver(handoff.actions, epoch);
        let session =
            DragSession::handed_off(source, handoff.payload, capture.pointer, capture.drag);
        Ok((session, delivery))
    }

    /// The drag's preview and the window point its top left goes at,
    /// while one shows: the pointer has moved [`DRAG_PREVIEW_THRESHOLD`]
    /// from the press and the drag's source is still in `next`, the frame
    /// being painted.
    pub fn drag_preview(&self, next: &InputFrame) -> Option<(&DragPreview, (f32, f32))> {
        let capture = self
            .capture
            .as_ref()
            .filter(|c| c.travel == Travel::Dragging)?;
        let preview = capture.drag.preview()?;
        node_identities(&next.semantic)
            .contains(&capture.identity)
            .then(|| (preview, preview.origin_at(capture.pointer)))
    }

    /// Focus after a press on `target`: the nearest node on its path that
    /// asks for focus (a text field, a tab stop, or an explicit focus
    /// target). Every clickable node is focusable for assistive tech, but a
    /// press on one that does not ask (a Send button) leaves focus where it
    /// is, so a composer stays focused; a press on inert background clears
    /// it. `None` means focus stays, which is also the case for presses
    /// outside an open modal.
    fn focus_after_press(&self, target: Option<usize>) -> Option<Option<FocusId>> {
        let semantic = &self.frame.semantic;
        if let Some(modal) = semantic.modal_root()
            && !target.is_some_and(|node| semantic.is_within(node, modal))
        {
            return None;
        }
        let Some(target) = target else {
            return Some(None);
        };
        let nodes = semantic.nodes();
        if let Some(focus) = semantic
            .ancestors_inclusive(target)
            .filter(|&i| {
                let node = &nodes[i];
                node.focus.is_some() || node.actions.text_value || node.tab_stop.is_some()
            })
            .find_map(|i| semantic.focus_id(i))
        {
            return Some(Some(focus));
        }
        let handlers = &self.frame.handlers;
        let interactive = semantic.ancestors_inclusive(target).any(|i| {
            let actions = &nodes[i].actions;
            actions.click || actions.drag || handlers.takes_press(i)
        });
        (!interactive).then_some(None)
    }
}
