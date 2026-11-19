//! Wheel and keyboard scrolling: each axis goes to the innermost node
//! scrolling on it, chaining outward from one at its limit.

use super::*;

/// Points of wheel motion per scroll line. Scroll handlers take whole
/// lines; [`InputRouter::wheel`] converts pixel deltas with this.
pub const WHEEL_LINE_PX: f32 = 20.0;

/// A node's wheel handler plus its position in the scroll range, so wheel
/// input can chain to the parent once this node is at its limit.
#[derive(Debug, Clone)]
pub struct ScrollTarget {
    pub builder: ScrollActionBuilder,
    pub offset: f32,
    /// Largest offset, or `None` when the element does not know its
    /// content size (it then never reports a limit when scrolling down).
    pub max: Option<f32>,
}

impl ScrollTarget {
    fn at_limit(&self, lines: i32) -> bool {
        if lines < 0 {
            self.offset <= 0.0
        } else {
            self.max.is_some_and(|max| self.offset >= max)
        }
    }
}

/// A node's retained scroll state: wheel and key input move the handle
/// directly on the axes it scrolls.
#[derive(Debug, Clone)]
pub struct HandleTarget {
    pub handle: ScrollHandle,
    pub axes: ScrollAxes,
}

/// What scrolls a node along one axis.
#[derive(Clone, Copy)]
pub(super) enum AxisTarget<'a> {
    Handle(&'a ScrollHandle),
    Builder(&'a ScrollTarget),
}

impl AxisTarget<'_> {
    pub(super) fn can_scroll(self, axis: Axis, forward: bool) -> bool {
        match self {
            Self::Handle(handle) => handle.can_scroll(axis, forward),
            Self::Builder(target) => !target.at_limit(if forward { 1 } else { -1 }),
        }
    }
}

/// One wheel event for [`InputRouter::scroll_wheel`]: motion in points,
/// positive scrolling content down and right, and when it happened, for
/// fling velocity.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct WheelEvent {
    pub dx: f32,
    pub dy: f32,
    pub now_ms: u64,
}

/// Wheel input carried from one event to the next.
#[derive(Default)]
pub(super) struct Wheel {
    /// Motion in lines not yet delivered on each axis, so slow trackpad
    /// motion adds up instead of rounding away.
    pending: [f32; 2],
    /// The handle wheel input last moved: the one a fling continues.
    handle: Option<ScrollHandle>,
}

impl Wheel {
    /// Add `delta` points of motion on `axis`, taking the whole lines it
    /// completes. A reversal starts over rather than first paying off the
    /// old direction's fraction.
    fn take_lines(&mut self, axis: Axis, delta: f32) -> i32 {
        let pending = &mut self.pending[axis.index()];
        if *pending * delta < 0.0 {
            *pending = 0.0;
        }
        *pending += delta / WHEEL_LINE_PX;
        let lines = pending.trunc() as i32;
        *pending -= lines as f32;
        lines
    }

    pub(super) fn forget_lines(&mut self) {
        self.pending = [0.0; 2];
    }

    pub(super) fn stop_fling(&self) {
        if let Some(handle) = &self.handle {
            handle.stop_fling();
        }
    }
}

impl InputRouter {
    /// Vertical wheel motion; see [`Self::scroll_wheel`].
    pub fn wheel(&mut self, x: f32, y: f32, delta_px: f32) -> Delivery {
        self.scroll_wheel(
            x,
            y,
            WheelEvent {
                dy: delta_px,
                ..WheelEvent::default()
            },
        )
    }

    /// Wheel motion goes, per axis, to the innermost node under the pointer
    /// that scrolls on that axis. A node at its limit in the wheel's
    /// direction passes the input to the next ancestor scrolling on that
    /// axis; when every one is at its limit, the innermost still gets it so
    /// the app can clamp or rubber-band.
    ///
    /// A [`ScrollHandle`] moves by the points given. An app-owned target
    /// gets whole lines of [`WHEEL_LINE_PX`]; the fraction of a line left
    /// over carries into the next call.
    pub fn scroll_wheel(&mut self, x: f32, y: f32, event: WheelEvent) -> Delivery {
        let Some(target) = self.target_at(x, y) else {
            self.wheel.forget_lines();
            return Delivery::default();
        };
        let epoch = scroll_epoch();
        let mut delivery = Delivery::default();
        for (axis, delta) in [(Axis::Y, event.dy), (Axis::X, event.dx)] {
            if delta == 0.0 {
                continue;
            }
            let routed = self.wheel_axis(target, axis, delta, event.now_ms);
            delivery.node = delivery.node.or(routed.node);
            delivery.actions.extend(routed.actions);
        }
        delivery.redraw = scroll_epoch() != epoch;
        delivery
    }

    fn wheel_axis(&mut self, target: usize, axis: Axis, delta: f32, now_ms: u64) -> Delivery {
        let handlers = &self.frame.handlers;
        let forward = delta > 0.0;
        let mut innermost = None;
        let routed = self.walk(target, UiEventKind::Wheel, |node| {
            let scroll = handlers.axis_target(node, axis)?;
            innermost.get_or_insert(node);
            scroll.can_scroll(axis, forward).then(Vec::new)
        });
        let Some(node) = routed.node.or(innermost) else {
            return routed;
        };
        // An event binding that stops the wheel ends the walk at a node
        // that may not scroll.
        let Some(scroll) = handlers.axis_target(node, axis) else {
            return routed;
        };
        let target = match scroll {
            AxisTarget::Handle(handle) => {
                handle.scroll_by(axis, delta, now_ms);
                self.wheel.handle = Some(handle.clone());
                return Delivery::handled(node, Vec::new());
            }
            AxisTarget::Builder(target) => target,
        };
        match self.wheel.take_lines(axis, delta) {
            0 => Delivery::default(),
            lines => Delivery::handled(node, vec![target.builder.build(lines)]),
        }
    }

    /// Fingers lifted off a trackpad that sends no momentum of its own:
    /// continue the handle wheel input last moved with inertia, from its
    /// recent velocity. Returns whether a fling started.
    pub fn fling(&mut self, now_ms: u64) -> bool {
        self.wheel
            .handle
            .as_ref()
            .is_some_and(|handle| handle.fling(now_ms))
    }

    /// Keyboard scrolling: arrows, Page Up/Down, Space, Home, and End move
    /// the nearest node on the focus path (focused node first) that scrolls
    /// on the key's axis and is not at its limit in that direction. A
    /// [`ScrollHandle`] scrolls smoothly; an app-owned target gets lines,
    /// or `to_px` for Home and End when it has one.
    pub fn scroll_key(&self, pressed: &Binding, focus: Option<FocusId>) -> Delivery {
        let Some(scroll) = KeyScroll::for_binding(pressed) else {
            return Delivery::default();
        };
        let semantic = &self.frame.semantic;
        let Some(start) = focus.and_then(|focus| semantic.node_for_focus(focus)) else {
            return Delivery::default();
        };
        let handlers = &self.frame.handlers;
        let (axis, forward) = (scroll.axis(), scroll.forward());
        let epoch = scroll_epoch();
        for node in semantic.ancestors_inclusive(start) {
            let Some(target) = handlers.axis_target(node, axis) else {
                continue;
            };
            if !target.can_scroll(axis, forward) {
                continue;
            }
            let actions = match target {
                AxisTarget::Handle(handle) => {
                    handle.key_scroll(scroll);
                    Vec::new()
                }
                AxisTarget::Builder(target) => {
                    let bounds = semantic.nodes()[node].bounds;
                    let viewport = if axis == Axis::X {
                        bounds.width
                    } else {
                        bounds.height
                    };
                    key_scroll_actions(target, scroll, viewport)
                }
            };
            return Delivery {
                redraw: scroll_epoch() != epoch,
                ..Delivery::handled(node, actions)
            };
        }
        Delivery::default()
    }
}

/// Actions an app-owned target gets for a key scroll: lines, or `to_px`
/// for Home and End when the builder has one.
fn key_scroll_actions(target: &ScrollTarget, scroll: KeyScroll, viewport: f32) -> Vec<Action> {
    let lines = |px: f32| (px / WHEEL_LINE_PX).round() as i32;
    let action = match scroll {
        KeyScroll::By(_, px) => Some(target.builder.build(lines(px))),
        KeyScroll::Page(_, forward) => {
            let page = lines(page_px(viewport));
            Some(target.builder.build(if forward { page } else { -page }))
        }
        KeyScroll::Edge(_, forward) => {
            let to = if forward { target.max } else { Some(0.0) };
            to.and_then(|to| {
                target
                    .builder
                    .build_to_px(to as u32)
                    .or_else(|| Some(target.builder.build(lines(to - target.offset))))
            })
        }
    };
    action.into_iter().collect()
}
