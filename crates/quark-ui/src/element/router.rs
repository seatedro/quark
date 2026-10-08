use std::collections::HashMap;
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::str::FromStr;

use super::*;

// ---------------------------------------------------------------------------
// Input routing: one hit table, handlers keyed by semantic node, and
// capture/bubble dispatch along SemanticFrame routes.
// ---------------------------------------------------------------------------

/// Points of wheel motion per scroll line. Scroll handlers take whole
/// lines; [`InputRouter::wheel`] converts pixel deltas with this.
pub const WHEEL_LINE_PX: f32 = 20.0;

/// Modifier keys of a [`Binding`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Mods {
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// `mod` in binding strings: either Cmd or Ctrl. Only patterns set it;
    /// a pressed key reports the concrete modifier.
    pub primary: bool,
}

/// One key with its modifiers, such as `mod+shift+p` or `escape`. Parse
/// one from the keymap format with [`str::parse`]; it prints back in that
/// format, modifiers first in a fixed order.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Binding {
    pub mods: Mods,
    /// Lowercase key name: a character (`"s"`, `"/"`) or a named key
    /// (`"enter"`, `"arrowup"`).
    pub key: String,
}

impl Binding {
    pub fn new(mods: Mods, key: &str) -> Self {
        Self {
            mods,
            key: normalize_key(key),
        }
    }

    /// Whether some key press satisfies both bindings. For a pattern and a
    /// pressed key this is "the press triggers the pattern"; for two
    /// patterns it is "they conflict".
    pub fn matches(&self, other: &Binding) -> bool {
        self.key == other.key
            && self.mods.alt == other.mods.alt
            && self.mods.shift == other.mods.shift
            && [(false, false), (true, false), (false, true), (true, true)]
                .into_iter()
                .any(|(cmd, ctrl)| self.accepts(cmd, ctrl) && other.accepts(cmd, ctrl))
    }

    /// Whether a press with Cmd and Ctrl held as given satisfies this
    /// binding's Cmd and Ctrl part.
    fn accepts(&self, cmd: bool, ctrl: bool) -> bool {
        let m = self.mods;
        if m.primary {
            (cmd || ctrl) && (cmd || !m.cmd) && (ctrl || !m.ctrl)
        } else {
            cmd == m.cmd && ctrl == m.ctrl
        }
    }
}

/// A binding string that does not parse; see [`Binding`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BindingError(String);

impl fmt::Display for BindingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid key binding {:?}", self.0)
    }
}

impl std::error::Error for BindingError {}

impl FromStr for Binding {
    type Err = BindingError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let error = || BindingError(text.to_owned());
        if text.is_empty() || text.contains(char::is_whitespace) {
            return Err(error());
        }
        // A trailing "+" after a separator names the plus key: "ctrl++".
        let (mods_text, key) = match text.strip_suffix("++") {
            Some(mods) => (mods, "+"),
            None if text == "+" => ("", "+"),
            None => text.rsplit_once('+').unwrap_or(("", text)),
        };
        if key.is_empty() {
            return Err(error());
        }
        let mut mods = Mods::default();
        if !mods_text.is_empty() {
            for part in mods_text.split('+') {
                let flag = match part.to_ascii_lowercase().as_str() {
                    "mod" | "primary" => &mut mods.primary,
                    "cmd" | "command" | "super" | "meta" => &mut mods.cmd,
                    "ctrl" | "control" => &mut mods.ctrl,
                    "alt" | "option" | "opt" => &mut mods.alt,
                    "shift" => &mut mods.shift,
                    _ => return Err(error()),
                };
                *flag = true;
            }
        }
        Ok(Self::new(mods, key))
    }
}

impl fmt::Display for Binding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let m = self.mods;
        for (held, name) in [
            (m.primary, "mod"),
            (m.cmd, "cmd"),
            (m.ctrl, "ctrl"),
            (m.alt, "alt"),
            (m.shift, "shift"),
        ] {
            if held {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&self.key)
    }
}

fn normalize_key(key: &str) -> String {
    let key = key.to_lowercase();
    let canonical = match key.as_str() {
        "esc" => "escape",
        "return" => "enter",
        "up" => "arrowup",
        "down" => "arrowdown",
        "left" => "arrowleft",
        "right" => "arrowright",
        "del" => "delete",
        "pgup" => "pageup",
        "pgdn" => "pagedown",
        " " => "space",
        _ => return key,
    };
    canonical.to_owned()
}

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
enum AxisTarget<'a> {
    Handle(&'a ScrollHandle),
    Builder(&'a ScrollTarget),
}

impl AxisTarget<'_> {
    fn can_scroll(self, axis: Axis, forward: bool) -> bool {
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

/// Event handlers of one frame. Each kind is a pair of parallel columns:
/// the owning semantic node index and the handler.
#[derive(Default)]
pub struct InputHandlers {
    click_node: Vec<usize>,
    click: Vec<ClickHandler>,
    middle_click_node: Vec<usize>,
    middle_click: Vec<Action>,
    drag_node: Vec<usize>,
    drag: Vec<DragStart>,
    scroll_node: Vec<usize>,
    scroll: Vec<ScrollTarget>,
    scroll_x_node: Vec<usize>,
    scroll_x: Vec<ScrollTarget>,
    handle_node: Vec<usize>,
    handle: Vec<HandleTarget>,
    scrollbar_node: Vec<usize>,
    scrollbar: Vec<(Scrollbar, ScrollSink)>,
    key_node: Vec<usize>,
    /// Shared so a replayed cache boundary re-registers its bindings
    /// without copying their key strings.
    key_binding: Vec<Rc<Binding>>,
    key_action: Vec<Action>,
}

impl InputHandlers {
    pub fn on_click(&mut self, node: usize, handler: ClickHandler) {
        self.click_node.push(node);
        self.click.push(handler);
    }

    /// A click of the middle (auxiliary) button: a press and release on the
    /// same handler.
    pub fn on_middle_click(&mut self, node: usize, action: Action) {
        self.middle_click_node.push(node);
        self.middle_click.push(action);
    }

    pub fn on_drag(&mut self, node: usize, start: DragStart) {
        self.drag_node.push(node);
        self.drag.push(start);
    }

    /// Vertical wheel input for an app-owned offset.
    pub fn on_scroll(&mut self, node: usize, target: ScrollTarget) {
        self.scroll_node.push(node);
        self.scroll.push(target);
    }

    /// Horizontal wheel input for an app-owned offset; `target.builder`
    /// gets the lines.
    pub fn on_scroll_x(&mut self, node: usize, target: ScrollTarget) {
        self.scroll_x_node.push(node);
        self.scroll_x.push(target);
    }

    /// Wheel and key input on `axes` move `handle`. Takes precedence over
    /// app-owned targets of the same node on those axes.
    pub fn on_scroll_handle(&mut self, node: usize, handle: ScrollHandle, axes: ScrollAxes) {
        self.handle_node.push(node);
        self.handle.push(HandleTarget { handle, axes });
    }

    /// A press on `bar` pages or drags its thumb; kept apart from
    /// [`Self::on_drag`] so a frame registers it without allocating.
    pub(crate) fn on_scrollbar(&mut self, node: usize, bar: Scrollbar, sink: ScrollSink) {
        self.scrollbar_node.push(node);
        self.scrollbar.push((bar, sink));
    }

    /// `binding` uses the keymap format, e.g. `"enter"` or `"mod+s"`. A
    /// binding that does not parse is a programming error: it panics in
    /// debug builds and never fires in release builds.
    pub fn on_key(&mut self, node: usize, binding: impl Into<String>, action: Action) {
        let binding = binding.into();
        match binding.parse::<Binding>() {
            Ok(parsed) => {
                self.key_node.push(node);
                self.key_binding.push(Rc::new(parsed));
                self.key_action.push(action);
            }
            Err(error) => debug_assert!(false, "{error}"),
        }
    }

    pub fn clear(&mut self) {
        self.click_node.clear();
        self.click.clear();
        self.middle_click_node.clear();
        self.middle_click.clear();
        self.drag_node.clear();
        self.drag.clear();
        self.scroll_node.clear();
        self.scroll.clear();
        self.scroll_x_node.clear();
        self.scroll_x.clear();
        self.handle_node.clear();
        self.handle.clear();
        self.scrollbar_node.clear();
        self.scrollbar.clear();
        self.key_node.clear();
        self.key_binding.clear();
        self.key_action.clear();
    }

    pub(super) fn marks(&self) -> HandlerMarks {
        HandlerMarks {
            click: self.click.len(),
            middle_click: self.middle_click.len(),
            drag: self.drag.len(),
            scroll: self.scroll.len(),
            scroll_x: self.scroll_x.len(),
            handle: self.handle.len(),
            scrollbar: self.scrollbar.len(),
            key: self.key_action.len(),
        }
    }

    /// Replace `out` with the handlers registered since `marks`, their nodes
    /// made relative to `node_base`. False when one belongs to a node
    /// before `node_base`, which a relative copy cannot express.
    pub(super) fn copy_since(
        &self,
        marks: HandlerMarks,
        node_base: usize,
        out: &mut InputHandlers,
    ) -> bool {
        fn nodes(from: &[usize], base: usize, out: &mut Vec<usize>) -> bool {
            out.clear();
            out.extend(from.iter().map(|node| node.wrapping_sub(base)));
            from.iter().all(|node| *node >= base)
        }
        let ok = nodes(
            &self.click_node[marks.click..],
            node_base,
            &mut out.click_node,
        ) & nodes(
            &self.middle_click_node[marks.middle_click..],
            node_base,
            &mut out.middle_click_node,
        ) & nodes(&self.drag_node[marks.drag..], node_base, &mut out.drag_node)
            & nodes(
                &self.scroll_node[marks.scroll..],
                node_base,
                &mut out.scroll_node,
            )
            & nodes(
                &self.scroll_x_node[marks.scroll_x..],
                node_base,
                &mut out.scroll_x_node,
            )
            & nodes(
                &self.handle_node[marks.handle..],
                node_base,
                &mut out.handle_node,
            )
            & nodes(
                &self.scrollbar_node[marks.scrollbar..],
                node_base,
                &mut out.scrollbar_node,
            )
            & nodes(&self.key_node[marks.key..], node_base, &mut out.key_node);
        fn copy<T: Clone>(from: &[T], out: &mut Vec<T>) {
            out.clear();
            out.extend_from_slice(from);
        }
        copy(&self.click[marks.click..], &mut out.click);
        copy(
            &self.middle_click[marks.middle_click..],
            &mut out.middle_click,
        );
        copy(&self.drag[marks.drag..], &mut out.drag);
        copy(&self.scroll[marks.scroll..], &mut out.scroll);
        copy(&self.scroll_x[marks.scroll_x..], &mut out.scroll_x);
        copy(&self.handle[marks.handle..], &mut out.handle);
        copy(&self.scrollbar[marks.scrollbar..], &mut out.scrollbar);
        copy(&self.key_binding[marks.key..], &mut out.key_binding);
        copy(&self.key_action[marks.key..], &mut out.key_action);
        ok
    }

    /// Append `from`, whose nodes are relative, with nodes at `node_base`.
    pub(super) fn extend_shifted(&mut self, from: &InputHandlers, node_base: usize) {
        fn shift(nodes: &[usize], base: usize) -> impl Iterator<Item = usize> + '_ {
            nodes.iter().map(move |node| node + base)
        }
        self.click_node.extend(shift(&from.click_node, node_base));
        self.click.extend_from_slice(&from.click);
        self.middle_click_node
            .extend(shift(&from.middle_click_node, node_base));
        self.middle_click.extend_from_slice(&from.middle_click);
        self.drag_node.extend(shift(&from.drag_node, node_base));
        self.drag.extend_from_slice(&from.drag);
        self.scroll_node.extend(shift(&from.scroll_node, node_base));
        self.scroll.extend_from_slice(&from.scroll);
        self.scroll_x_node
            .extend(shift(&from.scroll_x_node, node_base));
        self.scroll_x.extend_from_slice(&from.scroll_x);
        self.handle_node.extend(shift(&from.handle_node, node_base));
        self.handle.extend_from_slice(&from.handle);
        self.scrollbar_node
            .extend(shift(&from.scrollbar_node, node_base));
        self.scrollbar.extend_from_slice(&from.scrollbar);
        self.key_node.extend(shift(&from.key_node, node_base));
        self.key_binding.extend_from_slice(&from.key_binding);
        self.key_action.extend_from_slice(&from.key_action);
    }

    fn click(&self, node: usize) -> Option<&ClickHandler> {
        let i = self.click_node.iter().position(|n| *n == node)?;
        self.click.get(i)
    }

    fn middle_click(&self, node: usize) -> Option<&Action> {
        let i = self.middle_click_node.iter().position(|n| *n == node)?;
        self.middle_click.get(i)
    }

    fn drag(&self, node: usize) -> Option<&DragStart> {
        let i = self.drag_node.iter().position(|n| *n == node)?;
        self.drag.get(i)
    }

    /// The drag a press at `(x, y)` on `node` starts, if it has one.
    fn start_drag(&self, node: usize, x: f32, y: f32) -> Option<Box<dyn DragHandler>> {
        if let Some(start) = self.drag(node) {
            return Some(start.start(ClickEvent { x, y }));
        }
        let i = self.scrollbar_node.iter().position(|n| *n == node)?;
        let (bar, sink) = self.scrollbar.get(i)?;
        Some(Box::new(ScrollbarDrag::new(*bar, sink.clone(), x, y)))
    }

    fn axis_target(&self, node: usize, axis: Axis) -> Option<AxisTarget<'_>> {
        let handle = self
            .handle_node
            .iter()
            .zip(&self.handle)
            .find(|(n, target)| **n == node && target.axes.has(axis));
        if let Some((_, target)) = handle {
            return Some(AxisTarget::Handle(&target.handle));
        }
        let (nodes, targets) = match axis {
            Axis::X => (&self.scroll_x_node, &self.scroll_x),
            Axis::Y => (&self.scroll_node, &self.scroll),
        };
        let i = nodes.iter().position(|n| *n == node)?;
        targets.get(i).map(AxisTarget::Builder)
    }

    fn key(&self, node: usize, pressed: &Binding) -> Option<&Action> {
        let i = (0..self.key_node.len())
            .find(|&i| self.key_node[i] == node && self.key_binding[i].matches(pressed))?;
        self.key_action.get(i)
    }
}

/// Column lengths of [`InputHandlers`]: where a cache boundary's
/// registrations start.
#[derive(Debug, Clone, Copy)]
pub(super) struct HandlerMarks {
    click: usize,
    middle_click: usize,
    drag: usize,
    scroll: usize,
    scroll_x: usize,
    handle: usize,
    scrollbar: usize,
    key: usize,
}

/// What one painted frame registered for input.
#[derive(Default)]
pub struct InputFrame {
    pub hits: HitTable,
    pub handlers: InputHandlers,
    pub semantic: SemanticFrame,
    /// Where the frame's identified elements landed.
    pub geometry: LayoutSnapshot,
    /// Where the frame takes drops.
    pub drop_targets: DropTargets,
}

/// The outcome of routing one event: the semantic node that handled it
/// and the actions it emitted.
#[derive(Debug, Default)]
pub struct Delivery {
    pub node: Option<usize>,
    pub actions: Vec<Action>,
    /// The event moved a [`ScrollHandle`] (or its scrollbar state) or a
    /// drag preview, so the window needs a repaint even without actions.
    pub redraw: bool,
}

/// The drag holding the pointer. Frames come and go during a drag, so it
/// remembers its node by identity and finds it again in each new frame.
struct Capture {
    identity: u64,
    /// The node in the current frame, if it is still there.
    node: Option<usize>,
    drag: Box<dyn DragHandler>,
    press: (f32, f32),
    pointer: (f32, f32),
    /// The pointer has gone [`DRAG_PREVIEW_THRESHOLD`] from the press.
    moved: bool,
}

impl Capture {
    /// Whether the drag's preview is on screen.
    fn shows_preview(&self) -> bool {
        self.moved && self.drag.preview().is_some()
    }
}

/// Routes pointer, wheel, and key input through the last painted frame.
#[derive(Default)]
pub struct InputRouter {
    frame: InputFrame,
    focus_tree: FocusTree,
    capture: Option<Capture>,
    /// Wheel motion in lines not yet delivered on each axis, so slow
    /// trackpad motion adds up instead of rounding away.
    wheel_lines: [f32; 2],
    /// The handle wheel input last moved: the one a fling continues.
    wheel_handle: Option<ScrollHandle>,
    /// Identity of the node a middle press landed on, until its release.
    middle_press: Option<u64>,
}

impl InputRouter {
    pub fn set_frame(&mut self, frame: InputFrame) {
        frame.semantic.fill_focus_tree(&mut self.focus_tree);
        self.frame = frame;
        if let Some(capture) = &mut self.capture {
            let identities = node_identities(&self.frame.semantic);
            capture.node = identities.iter().position(|id| *id == capture.identity);
            capture.drag.set_geometry(&self.frame.geometry);
        }
    }

    /// [`Self::set_frame`], returning the previous frame so its buffers
    /// can be reused for the next one (see
    /// [`ElementContext::with_input_frame`]).
    pub fn replace_frame(&mut self, frame: InputFrame) -> InputFrame {
        let previous = std::mem::take(&mut self.frame);
        self.set_frame(frame);
        previous
    }

    pub fn frame(&self) -> &InputFrame {
        &self.frame
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Topmost semantic node under the point, honoring clips, z, and
    /// `BLOCKS_MOUSE`.
    pub fn target_at(&self, x: f32, y: f32) -> Option<usize> {
        self.frame
            .hits
            .stack_at(x, y)
            .into_iter()
            .find_map(|id| self.frame.hits.node(id))
    }

    pub fn cursor_at(&self, x: f32, y: f32) -> CursorHint {
        if let Some(capture) = &self.capture {
            return capture.drag.cursor();
        }
        self.frame
            .hits
            .stack_at(x, y)
            .into_iter()
            .filter_map(|id| self.frame.hits.cursor(id))
            .find(|cursor| *cursor != CursorHint::Default)
            .unwrap_or_default()
    }

    /// Press: update `focus`, then start a drag (capturing the pointer and
    /// delivering its press actions) or deliver a click to the first handler
    /// on the route. A press stops any fling.
    pub fn pointer_down(&mut self, x: f32, y: f32, focus: &mut Option<FocusId>) -> Delivery {
        if let Some(handle) = &self.wheel_handle {
            handle.stop_fling();
        }
        let target = self.target_at(x, y);
        if let Some(next) = self.focus_after_press(target) {
            *focus = next;
        }
        let Some(target) = target else {
            return Delivery::default();
        };
        let handlers = &self.frame.handlers;
        // The innermost node with a drag or click handler owns the press, so
        // a button inside a draggable panel still clicks.
        let drag = self.walk(target, UiEventKind::PointerDown, |node| {
            (handlers.drag(node).is_some()
                || handlers.scrollbar_node.contains(&node)
                || handlers.click(node).is_some())
            .then(Vec::new)
        });
        let epoch = scroll_epoch();
        if let Some(node) = drag.node
            && let Some(mut drag) = handlers.start_drag(node, x, y)
        {
            drag.set_geometry(&self.frame.geometry);
            let actions = drag.on_press();
            self.capture = Some(Capture {
                identity: node_identities(&self.frame.semantic)[node],
                node: Some(node),
                drag,
                press: (x, y),
                pointer: (x, y),
                moved: false,
            });
            return Delivery {
                node: Some(node),
                actions,
                redraw: scroll_epoch() != epoch,
            };
        }
        self.walk(target, UiEventKind::Click, |node| {
            handlers
                .click(node)
                .map(|handler| handler.invoke(ClickEvent { x, y }))
        })
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
        Delivery {
            node: Some(node),
            actions: self
                .frame
                .handlers
                .middle_click(node)
                .cloned()
                .into_iter()
                .collect(),
            redraw: false,
        }
    }

    /// Moves go to the capturing drag, wherever the pointer is. A drag
    /// showing a preview redraws on every move, so the preview follows.
    pub fn pointer_move(&mut self, x: f32, y: f32) -> Delivery {
        let epoch = scroll_epoch();
        match &mut self.capture {
            Some(capture) => {
                capture.pointer = (x, y);
                let (dx, dy) = (x - capture.press.0, y - capture.press.1);
                capture.moved |= dx.hypot(dy) >= DRAG_PREVIEW_THRESHOLD;
                Delivery {
                    node: capture.node,
                    actions: capture.drag.on_move(x, y),
                    redraw: scroll_epoch() != epoch || capture.shows_preview(),
                }
            }
            None => Delivery::default(),
        }
    }

    /// Release ends pointer capture.
    pub fn pointer_up(&mut self) -> Delivery {
        let epoch = scroll_epoch();
        match self.capture.take() {
            Some(mut capture) => Delivery {
                node: capture.node,
                actions: capture.drag.on_release().actions,
                redraw: scroll_epoch() != epoch || capture.shows_preview(),
            },
            None => Delivery::default(),
        }
    }

    /// End pointer capture without a release: the platform sends none once
    /// the window has lost focus, the app cancelled the drag, or a native
    /// drag took the pointer. The drag gets [`DragHandler::on_cancel`], never
    /// its release, so cancelling commits no drop.
    pub fn cancel_pointer(&mut self) -> Delivery {
        self.wheel_lines = [0.0; 2];
        let epoch = scroll_epoch();
        match self.capture.take() {
            Some(mut capture) => Delivery {
                node: capture.node,
                actions: capture.drag.on_cancel(),
                redraw: scroll_epoch() != epoch || capture.shows_preview(),
            },
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
        let delivery = Delivery {
            node: capture.node,
            actions: handoff.actions,
            redraw: scroll_epoch() != epoch || capture.shows_preview(),
        };
        let session =
            DragSession::handed_off(source, handoff.payload, capture.pointer, capture.drag);
        Ok((session, delivery))
    }

    /// The drag's preview and the window point its top left goes at,
    /// while one shows: the pointer has moved [`DRAG_PREVIEW_THRESHOLD`]
    /// from the press and the drag's source is still in `next`, the frame
    /// being painted.
    pub fn drag_preview(&self, next: &InputFrame) -> Option<(&DragPreview, (f32, f32))> {
        let capture = self.capture.as_ref().filter(|c| c.moved)?;
        let preview = capture.drag.preview()?;
        node_identities(&next.semantic)
            .contains(&capture.identity)
            .then(|| (preview, preview.origin_at(capture.pointer)))
    }

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
            self.wheel_lines = [0.0; 2];
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
                self.wheel_handle = Some(handle.clone());
                return Delivery {
                    node: Some(node),
                    ..Delivery::default()
                };
            }
            AxisTarget::Builder(target) => target,
        };
        let pending = &mut self.wheel_lines[axis.index()];
        // A reversal starts over rather than first paying off the old
        // direction's fraction.
        if *pending * delta < 0.0 {
            *pending = 0.0;
        }
        *pending += delta / WHEEL_LINE_PX;
        let lines = pending.trunc() as i32;
        if lines == 0 {
            return Delivery::default();
        }
        *pending -= lines as f32;
        Delivery {
            node: Some(node),
            actions: vec![target.builder.build(lines)],
            redraw: false,
        }
    }

    /// Fingers lifted off a trackpad that sends no momentum of its own:
    /// continue the handle wheel input last moved with inertia, from its
    /// recent velocity. Returns whether a fling started.
    pub fn fling(&mut self, now_ms: u64) -> bool {
        self.wheel_handle
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
                node: Some(node),
                actions,
                redraw: scroll_epoch() != epoch,
            };
        }
        Delivery::default()
    }

    /// Key press: the focused node first, then its ancestors. With nothing
    /// focused, the top-level nodes get it, as a page's body would.
    pub fn key_down(&self, pressed: &Binding, focus: Option<FocusId>) -> Delivery {
        let semantic = &self.frame.semantic;
        let handlers = &self.frame.handlers;
        let route = |target| {
            self.walk(target, UiEventKind::KeyDown, |node| {
                handlers
                    .key(node, pressed)
                    .map(|action| vec![action.clone()])
            })
        };
        if let Some(target) = focus.and_then(|focus| semantic.node_for_focus(focus)) {
            return route(target);
        }
        (0..semantic.nodes().len())
            .filter(|&node| semantic.nodes()[node].parent.is_none())
            .map(route)
            .find(|delivery| delivery.node.is_some())
            .unwrap_or_default()
    }

    /// Enter or Space with no modifiers on the focused node clicks it, at
    /// its center, when it has a click handler. Ancestors are not tried: a
    /// key on a focused field must not press the card around it.
    pub fn activate(&self, pressed: &Binding, focus: Option<FocusId>) -> Delivery {
        let m = pressed.mods;
        if !matches!(pressed.key.as_str(), "enter" | "space") || m.cmd || m.ctrl || m.alt || m.shift
        {
            return Delivery::default();
        }
        let semantic = &self.frame.semantic;
        let Some(node) = focus.and_then(|focus| semantic.node_for_focus(focus)) else {
            return Delivery::default();
        };
        let (Some(handler), Some(target)) =
            (self.frame.handlers.click(node), semantic.nodes().get(node))
        else {
            return Delivery::default();
        };
        if target.state.disabled {
            return Delivery::default();
        }
        let b = target.bounds;
        Delivery {
            node: Some(node),
            actions: handler.invoke(ClickEvent {
                x: b.x + b.width / 2.0,
                y: b.y + b.height / 2.0,
            }),
            redraw: false,
        }
    }

    /// Next focus along the tab order, wrapping at either end. Inside a
    /// modal scope, only that scope's targets are in the order.
    pub fn traverse_focus(&self, focus: Option<FocusId>, backwards: bool) -> Option<FocusId> {
        let order: Vec<FocusId> = self
            .focus_tree
            .tab_order(None)
            .into_iter()
            .map(|node| node.id)
            .collect();
        if order.is_empty() {
            return focus;
        }
        let current = focus.and_then(|focus| order.iter().position(|id| *id == focus));
        let next = match (current, backwards) {
            (None, false) => 0,
            (None, true) => order.len() - 1,
            (Some(i), false) => (i + 1) % order.len(),
            (Some(i), true) => (i + order.len() - 1) % order.len(),
        };
        Some(order[next])
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
            actions.click
                || actions.drag
                || handlers.click(i).is_some()
                || handlers.drag(i).is_some()
                || handlers.scrollbar_node.contains(&i)
        });
        (!interactive).then_some(None)
    }

    /// Capture then bubble along `target`'s route. `handle` runs at the
    /// target and bubble steps; the first `Some` handles the event. An event
    /// binding whose default result stops propagation ends the walk at its
    /// node.
    fn walk(
        &self,
        target: usize,
        kind: UiEventKind,
        mut handle: impl FnMut(usize) -> Option<Vec<Action>>,
    ) -> Delivery {
        let Some(steps) = self.frame.semantic.route_indices(target) else {
            return Delivery::default();
        };
        for (node, phase) in steps {
            if phase != UiEventPhase::Capture
                && let Some(actions) = handle(node)
            {
                return Delivery {
                    node: Some(node),
                    actions,
                    redraw: false,
                };
            }
            if self.binding_stops(node, kind, phase) {
                return Delivery {
                    node: Some(node),
                    ..Delivery::default()
                };
            }
        }
        Delivery::default()
    }

    fn binding_stops(&self, node: usize, kind: UiEventKind, phase: UiEventPhase) -> bool {
        let capture = phase == UiEventPhase::Capture;
        self.frame.semantic.nodes()[node]
            .event_bindings
            .iter()
            .any(|binding| {
                binding.kind == kind
                    && (binding.phase == UiEventPhase::Capture) == capture
                    && !binding.default_result.should_continue()
            })
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

/// Each node's identity across frames: its stable id or test id when it
/// has one, else its key, focus target, or position among its parent's
/// children, chained to its parent's identity.
fn node_identities(semantic: &SemanticFrame) -> Vec<u64> {
    let nodes = semantic.nodes();
    let mut identities: Vec<u64> = Vec::with_capacity(nodes.len());
    let mut children: HashMap<Option<usize>, u32> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let ordinal = children.entry(node.parent).or_default();
        let position = *ordinal;
        *ordinal += 1;
        let mut hasher = DefaultHasher::new();
        if let Some(id) = &node.id {
            (0u8, id).hash(&mut hasher);
        } else if let Some(test_id) = &node.test_id {
            (1u8, test_id).hash(&mut hasher);
        } else {
            // Parents precede children in a well-formed frame.
            let parent = node.parent.filter(|p| *p < index).map(|p| identities[p]);
            parent.hash(&mut hasher);
            match (&node.key, node.focus) {
                (Some(key), _) => (2u8, key).hash(&mut hasher),
                (None, Some(focus)) => (3u8, focus).hash(&mut hasher),
                (None, None) => (4u8, position).hash(&mut hasher),
            }
        }
        identities.push(hasher.finish());
    }
    identities
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::Styled;
    use crate::theme::Theme;

    #[derive(Debug, Clone, PartialEq)]
    enum Msg {
        Click(&'static str),
        Scroll(&'static str, i32),
        Move(i32, i32),
        Release,
        Cancel,
    }

    impl From<Msg> for Action {
        fn from(msg: Msg) -> Self {
            Action::new(msg)
        }
    }

    /// Paint `root` into a `w`x`h` window and route input through the result.
    fn routed(root: impl IntoAnyElement, w: f32, h: f32) -> InputRouter {
        // Vendored fonts only, so the tests do not depend on the host's fonts.
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
        cx.semantic = SemanticFrame::new(w, h);
        let mut root = root.into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, w, h);
        let mut router = InputRouter::default();
        router.set_frame(cx.take_input_frame());
        router
    }

    /// `"<node test id> <actions>"`, or `"-"` when nothing handled the event.
    fn dump(router: &InputRouter, delivery: Delivery) -> String {
        let Some(node) = delivery.node else {
            return "-".to_owned();
        };
        let name = router.frame().semantic.nodes()[node]
            .test_id
            .as_ref()
            .map_or("?", |id| id.as_str())
            .to_owned();
        format!("{name} {:?}", delivery.actions)
    }

    fn click(router: &mut InputRouter, x: f32, y: f32) -> String {
        let delivery = router.pointer_down(x, y, &mut None);
        router.pointer_up();
        dump(router, delivery)
    }

    fn button(name: &'static str, w: f32, h: f32) -> Div {
        div().w(w).h(h).test_id(name).on_click(Msg::Click(name))
    }

    #[test]
    fn clipped_out_row_cannot_take_clicks_outside_its_clip() {
        // The list starts at y=40 and is scrolled by 100, so row-1 spans
        // y=-10..40: inside its bounds at y=20, but outside the list's clip.
        let rows = ["row-0", "row-1", "row-2", "row-3", "row-4"]
            .map(|name| button(name, 200.0, 50.0).flex_shrink_0());
        let root = div()
            .w(200.0)
            .h(140.0)
            .flex_col()
            .child(button("header", 200.0, 40.0))
            .child(
                div()
                    .w(200.0)
                    .h(100.0)
                    .flex_col()
                    .scroll_y(100.0)
                    .children_from(rows),
            );
        let mut router = routed(root, 200.0, 140.0);

        let got = [
            click(&mut router, 10.0, 20.0),
            click(&mut router, 10.0, 60.0),
        ];

        assert_eq!(
            got,
            [r#"header [Click("header")]"#, r#"row-2 [Click("row-2")]"#]
        );
    }

    #[test]
    fn higher_z_wins_over_later_paint_order() {
        let root = div()
            .w(100.0)
            .h(100.0)
            .child(
                button("raised", 100.0, 100.0)
                    .absolute()
                    .top(0.0)
                    .left(0.0)
                    .z_index(10),
            )
            .child(button("later", 100.0, 100.0).absolute().top(0.0).left(0.0));
        let mut router = routed(root, 100.0, 100.0);

        assert_eq!(
            click(&mut router, 50.0, 50.0),
            r#"raised [Click("raised")]"#
        );
    }

    #[test]
    fn wheel_chains_to_parent_only_at_the_scroll_limit() {
        // The inner list is scrolled to its end (300 content - 100 viewport).
        let inner = div()
            .w(200.0)
            .h(100.0)
            .test_id("inner")
            .scroll_y(200.0)
            .scroll_total(300.0)
            .on_scroll(ScrollActionBuilder::new(|lines| {
                Msg::Scroll("inner", lines).into()
            }))
            .child(div().w(200.0).h(300.0));
        let outer = div()
            .w(200.0)
            .h(200.0)
            .test_id("outer")
            .flex_col()
            .scroll_y(0.0)
            .scroll_total(1000.0)
            .on_scroll(ScrollActionBuilder::new(|lines| {
                Msg::Scroll("outer", lines).into()
            }))
            .child(inner);
        let mut router = routed(outer, 200.0, 200.0);

        let got = [3.0, -3.0].map(|lines| {
            let delivery = router.wheel(50.0, 50.0, lines * WHEEL_LINE_PX);
            dump(&router, delivery)
        });

        assert_eq!(
            got,
            [
                r#"outer [Scroll("outer", 3)]"#,
                r#"inner [Scroll("inner", -3)]"#
            ]
        );
    }

    // A code block that scrolls sideways inside a page that scrolls both
    // ways: each axis goes to the innermost container scrolling on it that
    // is not at its limit.
    #[test]
    fn wheel_chains_per_axis() {
        let route = |inner_x: f32, dx: f32, dy: f32| {
            let (outer, inner) = (ScrollHandle::new(), ScrollHandle::new());
            inner.set_offset(inner_x, 0.0);
            let code = div()
                .w(200.0)
                .h(100.0)
                .flex_shrink_0()
                .track_scroll(&inner)
                .overflow_x_scroll()
                .child(div().w(600.0).h(100.0).flex_shrink_0());
            let page = div()
                .w(200.0)
                .h(200.0)
                .flex_col()
                .track_scroll(&outer)
                .overflow_scroll()
                .child(code)
                .child(div().w(400.0).h(1000.0).flex_shrink_0());
            let mut router = routed(page, 200.0, 200.0);
            router.scroll_wheel(50.0, 50.0, WheelEvent { dx, dy, now_ms: 0 });
            format!("outer {:?} inner {:?}", outer.offset(), inner.offset())
        };

        let got = [
            route(0.0, 0.0, 40.0),
            route(0.0, 40.0, 0.0),
            // The code block is scrolled to its end (600 - 200).
            route(400.0, 40.0, 0.0),
        ];

        assert_eq!(
            got,
            [
                "outer (0.0, 40.0) inner (0.0, 0.0)",
                "outer (0.0, 0.0) inner (40.0, 0.0)",
                "outer (40.0, 0.0) inner (400.0, 0.0)",
            ]
        );
    }

    struct RecordDrag;

    impl DragHandler for RecordDrag {
        fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
            vec![Msg::Move(x as i32, y as i32).into()]
        }

        fn on_release(&mut self) -> DragReleaseResult {
            DragReleaseResult {
                actions: vec![Msg::Release.into()],
            }
        }

        fn on_cancel(&mut self) -> Vec<Action> {
            vec![Msg::Cancel.into()]
        }
    }

    #[test]
    fn drag_capture_keeps_delivering_after_leaving_bounds() {
        let root = div().w(400.0).h(400.0).child(
            div()
                .w(50.0)
                .h(50.0)
                .test_id("handle")
                .on_drag(|_| Box::new(RecordDrag)),
        );
        let mut router = routed(root, 400.0, 400.0);

        router.pointer_down(10.0, 10.0, &mut None);
        let moved = router.pointer_move(300.0, 300.0);
        let moved = dump(&router, moved);
        let released = router.pointer_up();
        let released = dump(&router, released);

        assert_eq!(
            [moved, released],
            ["handle [Move(300, 300)]", "handle [Release]"]
        );
    }

    // Regression: trackpad pixel deltas were rounded to lines one event at
    // a time, so slow two-finger scrolling never moved anything.
    #[test]
    fn slow_trackpad_deltas_accumulate_into_lines() {
        let list = div()
            .w(200.0)
            .h(200.0)
            .test_id("list")
            .on_scroll(ScrollActionBuilder::new(|lines| {
                Msg::Scroll("list", lines).into()
            }));
        let mut router = routed(list, 200.0, 200.0);

        // 25 events of 2px are 50px: two whole 20px lines, in order.
        let mut delivered = Vec::new();
        for _ in 0..25 {
            let delivery = router.wheel(50.0, 50.0, 2.0);
            if delivery.node.is_some() {
                delivered.push(dump(&router, delivery));
            }
        }

        assert_eq!(
            delivered,
            [r#"list [Scroll("list", 1)]"#, r#"list [Scroll("list", 1)]"#]
        );
    }

    // Regression: a drag interrupted by the window losing focus never got a
    // release, so it kept the pointer captured forever.
    #[test]
    fn cancel_pointer_cancels_the_drag_and_ends_capture() {
        let root = div().w(400.0).h(400.0).child(
            div()
                .w(50.0)
                .h(50.0)
                .test_id("handle")
                .on_drag(|_| Box::new(RecordDrag)),
        );
        let mut router = routed(root, 400.0, 400.0);

        router.pointer_down(10.0, 10.0, &mut None);
        let cancelled = router.cancel_pointer();
        let cancelled = dump(&router, cancelled);
        let moved = router.pointer_move(300.0, 300.0);
        let moved = dump(&router, moved);

        assert_eq!([cancelled, moved], ["handle [Cancel]", "-"]);
    }

    /// [`RecordDrag`] that agrees to move into a session carrying "tab".
    struct TearDrag;

    impl DragHandler for TearDrag {
        fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
            RecordDrag.on_move(x, y)
        }

        fn on_release(&mut self) -> DragReleaseResult {
            RecordDrag.on_release()
        }

        fn on_cancel(&mut self) -> Vec<Action> {
            RecordDrag.on_cancel()
        }

        fn on_handoff(&mut self) -> Option<DragHandoff> {
            Some(DragHandoff {
                payload: Box::new("tab"),
                actions: vec![Msg::Click("handed off").into()],
            })
        }
    }

    fn drag_frame(start: fn(ClickEvent) -> Box<dyn DragHandler>) -> InputRouter {
        let root = div()
            .w(400.0)
            .h(400.0)
            .child(div().w(50.0).h(50.0).test_id("handle").on_drag(start));
        routed(root, 400.0, 400.0)
    }

    // Catches a handed-off drag still driven by its source router: a blur
    // of the source window cancelling a drag now over another window, or
    // a release there committing a local drop. The session ends it, once.
    #[test]
    fn a_handed_off_drag_hears_only_from_its_session() {
        let mut router = drag_frame(|_| Box::new(TearDrag));
        router.pointer_down(10.0, 10.0, &mut None);
        router.pointer_move(30.0, 30.0);

        let (session, handed) = router
            .hand_off_capture(DragWindowId(1))
            .expect("handed off");
        let handed = dump(&router, handed);
        let after = [
            router.pointer_move(300.0, 300.0),
            router.cancel_pointer(),
            router.pointer_up(),
        ]
        .map(|delivery| dump(&router, delivery));
        let ended = session.cancel();

        assert_eq!(handed, r#"handle [Click("handed off")]"#);
        assert_eq!(after, ["-", "-", "-"]);
        assert_eq!(format!("{:?}", ended.actions), "[Cancel]");
        assert_eq!(ended.payload.downcast_ref::<&str>(), Some(&"tab"));
    }

    // Catches a handoff taking drags that never opted in (a scrollbar
    // thumb, a text selection) away from their window.
    #[test]
    fn a_drag_that_refuses_handoff_keeps_the_pointer() {
        let mut router = drag_frame(|_| Box::new(RecordDrag));
        router.pointer_down(10.0, 10.0, &mut None);

        let refused = router.hand_off_capture(DragWindowId(1)).err();
        let moved = router.pointer_move(300.0, 300.0);

        assert_eq!(refused, Some(HandoffError::Refused));
        assert_eq!(dump(&router, moved), "handle [Move(300, 300)]");
    }

    // Regression: capture kept the pressed node's index, so once a new frame
    // inserted a node before it, drag deliveries named some other node.
    #[test]
    fn capture_follows_its_node_into_a_reordered_frame() {
        let frame = |banner: bool| {
            let mut root = div().w(400.0).h(400.0).flex_col();
            if banner {
                root = root.child(button("banner", 400.0, 20.0));
            }
            root.child(
                div()
                    .w(50.0)
                    .h(50.0)
                    .test_id("handle")
                    .on_drag(|_| Box::new(RecordDrag)),
            )
        };
        let mut router = routed(frame(false), 400.0, 400.0);
        router.pointer_down(10.0, 10.0, &mut None);

        let next = routed(frame(true), 400.0, 400.0);
        router.set_frame(next.frame);
        let moved = router.pointer_move(300.0, 300.0);

        assert_eq!(dump(&router, moved), "handle [Move(300, 300)]");
    }

    // A tab strip: tabs close on a middle click, the strip only takes
    // primary clicks.
    #[test]
    fn a_middle_click_needs_press_and_release_on_one_handler() {
        let tab = |name: &'static str| {
            div()
                .w(100.0)
                .h(40.0)
                .test_id(name)
                .on_click(Msg::Click(name))
                .on_middle_click(Msg::Click("close"))
        };
        let strip = div()
            .w(300.0)
            .h(40.0)
            .flex_row()
            .test_id("strip")
            .on_click(Msg::Click("strip"))
            .child(tab("a"))
            .child(tab("b"));
        let mut router = routed(strip, 300.0, 40.0);
        // (press x, release x, delivery)
        let cases = [
            (50.0, 60.0, r#"a [Click("close")]"#),
            (50.0, 150.0, "-"),
            (250.0, 250.0, "-"),
        ];
        for (down, up, expected) in cases {
            router.middle_down(down, 20.0);
            let delivery = router.middle_up(up, 20.0);
            assert_eq!(dump(&router, delivery), expected, "{down} -> {up}");
        }
    }

    // Regression: `.on_key("mod+s")` was compared as text against the
    // pressed "ctrl+s", and keys with nothing focused went nowhere.
    #[test]
    fn mod_binding_fires_for_cmd_or_ctrl_with_nothing_focused() {
        let root = div()
            .w(100.0)
            .h(100.0)
            .test_id("root")
            .on_key("mod+s", Msg::Click("save"));
        let router = routed(root, 100.0, 100.0);

        let cases = [
            ("ctrl+s", r#"root [Click("save")]"#),
            ("cmd+s", r#"root [Click("save")]"#),
            ("s", "-"),
            ("ctrl+shift+s", "-"),
        ];
        for (pressed, expected) in cases {
            let pressed: Binding = pressed.parse().unwrap();
            let delivery = router.key_down(&pressed, None);
            assert_eq!(dump(&router, delivery), expected, "{pressed}");
        }
    }

    #[test]
    fn binding_strings_parse_to_canonical_form() {
        let cases = [
            ("mod+s", Some("mod+s")),
            ("Shift+Ctrl+P", Some("ctrl+shift+p")),
            ("cmd+alt+esc", Some("cmd+alt+escape")),
            ("ctrl++", Some("ctrl++")),
            ("option+up", Some("alt+arrowup")),
            ("hyper+s", None),
            ("ctrl+", None),
            ("g g", None),
        ];
        for (text, expected) in cases {
            let parsed = text.parse::<Binding>().ok().map(|b| b.to_string());
            assert_eq!(parsed.as_deref(), expected, "{text}");
        }
    }

    // Regression: any press outside a text field cleared focus, so clicking
    // Send blurred the composer it was sending from.
    #[test]
    fn press_moves_focus_only_to_focusable_nodes() {
        let field = FocusId::from_key("field");
        let root = div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(
                text_input("Message", "")
                    .focus_target(field)
                    .w(200.0)
                    .h(40.0),
            )
            .child(button("send", 80.0, 30.0))
            .child(focusable("toggle").on_click(Msg::Click("toggle")))
            .child(div().w(400.0).h(100.0));
        let mut router = routed(root, 400.0, 300.0);

        // (press, focus after): the field is focused before each press.
        let cases = [
            ("send button", (10.0, 50.0), Some(field)),
            ("focusable", (10.0, 80.0), Some(FocusId::from_key("toggle"))),
            ("background", (10.0, 200.0), None),
        ];
        for (name, (x, y), expected) in cases {
            let mut focus = Some(field);
            router.pointer_down(x, y, &mut focus);
            router.pointer_up();
            assert_eq!(focus, expected, "{name}");
        }
    }

    mod streaming {
        use std::collections::HashMap;

        use quark::selection::BlockKey;

        use super::*;
        use crate::document::{
            Block, Document, DocumentEvent, DocumentRow, DocumentStyle, RowChrome, TextMeasurer,
        };
        use crate::virtual_list::RowKey;

        #[derive(Debug, Clone, PartialEq)]
        struct Ev(DocumentEvent);

        impl From<Ev> for Action {
            fn from(ev: Ev) -> Self {
                Action::new(ev)
            }
        }

        type Messages = HashMap<RowKey, DocumentRow>;

        fn message(i: u64, text: &str) -> DocumentRow {
            DocumentRow {
                key: RowKey(i),
                chrome: RowChrome {
                    header_height: 22.0,
                    label: Some(format!("author {i}").into()),
                    kind: 0,
                },
                blocks: vec![Block::plain(BlockKey(i), text)],
                adornments: Vec::new(),
            }
        }

        /// A 400x300 document stuck to the bottom, with shared text state
        /// so each frame is painted the way the adapter paints it.
        struct Chat {
            messages: Messages,
            document: Document,
            text: TextSystem,
            layouts: LayoutCache,
            router: InputRouter,
        }

        impl Chat {
            fn new(n: u64) -> Self {
                let messages: Messages = (0..n)
                    .map(|i| (RowKey(i), message(i, &format!("Message {i} text."))))
                    .collect();
                let mut document = Document::new(DocumentStyle::for_font_size(14.0));
                document
                    .extend((0..n).map(|i| &messages[&RowKey(i)]))
                    .unwrap();
                let mut chat = Self {
                    messages,
                    document,
                    text: TextSystem::vendored_only(&Default::default()),
                    layouts: LayoutCache::default(),
                    router: InputRouter::default(),
                };
                chat.frame();
                chat.document.scroll_to_bottom();
                chat.frame();
                chat
            }

            /// Prepare and paint a frame; route input through it from now on.
            fn frame(&mut self) {
                let (w, h) = (400.0, 300.0);
                self.document.prepare(
                    w,
                    h,
                    0,
                    &self.messages,
                    &mut TextMeasurer::new(&mut self.text, &mut self.layouts, 14.0, 1.0),
                );
                let theme = Theme::default_dark();
                let signals = SignalStore::new();
                let mut root = self
                    .document
                    .element(&self.messages, &theme, |ev| Ev(ev).into())
                    .into_any();
                let mut cx = ElementContext::new(
                    &theme,
                    1.0,
                    &mut self.text,
                    &mut self.layouts,
                    None,
                    &signals,
                );
                cx.semantic = SemanticFrame::new(w, h);
                render_element(&mut root, &mut Scene::default(), &mut cx, w, h);
                self.router.set_frame(cx.take_input_frame());
            }

            /// Apply delivered actions the way an app would, right away.
            fn apply(&mut self, delivery: Delivery) {
                for action in delivery.actions {
                    if let Some(Ev(event)) = action.downcast_ref::<Ev>() {
                        self.document.handle(*event);
                    }
                }
            }

            fn block_rect(&self, key: u64) -> Rect {
                self.document
                    .visible_blocks()
                    .iter()
                    .find(|b| b.key == BlockKey(key))
                    .map(|b| b.rect)
                    .expect("block is visible")
            }

            /// Grow the last message, as a streaming reply does.
            fn stream_into_last(&mut self, text: &str) {
                let last = RowKey(self.messages.len() as u64 - 1);
                let message = message(last.0, text);
                self.document.update(&message).unwrap();
                self.messages.insert(last, message);
            }
        }

        // Regression: a drag reported its press only with the first move, so
        // text streaming in between shifted the anchor onto other text.
        #[test]
        fn press_then_stream_keeps_the_anchor_at_the_pressed_text() {
            let mut chat = Chat::new(8);
            let from = chat.block_rect(5);

            let pressed = chat
                .router
                .pointer_down(from.x - 5.0, from.y + 5.0, &mut None);
            chat.apply(pressed);
            chat.stream_into_last(&"streamed words ".repeat(15));
            chat.frame();
            let to = chat.block_rect(5);
            let moved = chat
                .router
                .pointer_move(to.x + to.width + 40.0, to.y + to.height - 2.0);
            chat.apply(moved);
            let released = chat.router.pointer_up();
            chat.apply(released);

            assert!(to.y < from.y, "streaming moved the pressed block up");
            assert_eq!(
                chat.document.selected_text(&chat.messages),
                "Message 5 text."
            );
        }
    }

    fn focusable(name: &'static str) -> Div {
        div()
            .w(50.0)
            .h(20.0)
            .test_id(name)
            .focus_ring(FocusId::from_key(name))
    }

    #[test]
    fn tab_cycles_only_inside_a_modal_scope() {
        let root = div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(focusable("outside"))
            .child(
                div()
                    .flex_col()
                    .focus_scope("dialog")
                    .trap_focus(true)
                    .child(focusable("ok"))
                    .child(focusable("cancel")),
            );
        let router = routed(root, 400.0, 300.0);

        let mut focus = None;
        let visited: Vec<&str> = (0..3)
            .map(|_| {
                focus = router.traverse_focus(focus, false);
                ["outside", "ok", "cancel"]
                    .into_iter()
                    .find(|name| focus == Some(FocusId::from_key(name)))
                    .unwrap_or("?")
            })
            .collect();

        assert_eq!(visited, ["ok", "cancel", "ok"]);
    }

    #[test]
    fn click_outside_a_modal_scope_keeps_focus() {
        let inside = FocusId::from_key("inside");
        let root = div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(
                text_input("Outside", "")
                    .focus_target(FocusId::from_key("outside"))
                    .w(200.0)
                    .h(40.0),
            )
            .child(
                div().focus_scope("dialog").trap_focus(true).child(
                    text_input("Inside", "")
                        .focus_target(inside)
                        .w(200.0)
                        .h(40.0),
                ),
            );
        let mut router = routed(root, 400.0, 300.0);

        let mut focus = Some(inside);
        router.pointer_down(10.0, 10.0, &mut focus);

        assert_eq!(focus, Some(inside));
    }

    // Keyboard parity: a focused button that binds no keys was unreachable
    // by keyboard; Enter or Space now clicks it, and only it.
    #[test]
    fn enter_and_space_click_the_focused_node_only() {
        let root = div()
            .w(200.0)
            .h(100.0)
            .test_id("card")
            .on_click(Msg::Click("card"))
            .child(button("save", 50.0, 20.0))
            .child(
                button("off", 50.0, 20.0)
                    .accessibility_role(accesskit::Role::Button)
                    .accessibility_disabled(true),
            )
            .child(
                div()
                    .w(50.0)
                    .h(20.0)
                    .test_id("label")
                    .focus_ring(FocusId::from_key("label")),
            );
        let router = routed(root, 200.0, 100.0);

        let save = Some(FocusId::from_key("save"));
        let cases = [
            ("enter", save, r#"save [Click("save")]"#),
            ("space", save, r#"save [Click("save")]"#),
            ("shift+enter", save, "-"),
            ("a", save, "-"),
            ("enter", Some(FocusId::from_key("off")), "-"),
            // Focus inside the card on a node without a click handler.
            ("enter", Some(FocusId::from_key("label")), "-"),
            ("enter", None, "-"),
        ];
        for (pressed, focus, expected) in cases {
            let pressed: Binding = pressed.parse().unwrap();
            let delivery = router.activate(&pressed, focus);
            assert_eq!(dump(&router, delivery), expected, "{pressed} on {focus:?}");
        }
    }
}
