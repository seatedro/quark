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

/// Event handlers of one frame. Each kind is a pair of parallel columns:
/// the owning semantic node index and the handler.
#[derive(Default)]
pub struct InputHandlers {
    click_node: Vec<usize>,
    click: Vec<ClickHandler>,
    drag_node: Vec<usize>,
    drag: Vec<DragStart>,
    scroll_node: Vec<usize>,
    scroll: Vec<ScrollTarget>,
    key_node: Vec<usize>,
    key_binding: Vec<Binding>,
    key_action: Vec<Action>,
}

impl InputHandlers {
    pub fn on_click(&mut self, node: usize, handler: ClickHandler) {
        self.click_node.push(node);
        self.click.push(handler);
    }

    pub fn on_drag(&mut self, node: usize, start: DragStart) {
        self.drag_node.push(node);
        self.drag.push(start);
    }

    pub fn on_scroll(&mut self, node: usize, target: ScrollTarget) {
        self.scroll_node.push(node);
        self.scroll.push(target);
    }

    /// `binding` uses the keymap format, e.g. `"enter"` or `"mod+s"`. A
    /// binding that does not parse is a programming error: it panics in
    /// debug builds and never fires in release builds.
    pub fn on_key(&mut self, node: usize, binding: impl Into<String>, action: Action) {
        let binding = binding.into();
        match binding.parse::<Binding>() {
            Ok(parsed) => {
                self.key_node.push(node);
                self.key_binding.push(parsed);
                self.key_action.push(action);
            }
            Err(error) => debug_assert!(false, "{error}"),
        }
    }

    pub fn clear(&mut self) {
        self.click_node.clear();
        self.click.clear();
        self.drag_node.clear();
        self.drag.clear();
        self.scroll_node.clear();
        self.scroll.clear();
        self.key_node.clear();
        self.key_binding.clear();
        self.key_action.clear();
    }

    pub(super) fn marks(&self) -> HandlerMarks {
        HandlerMarks {
            click: self.click.len(),
            drag: self.drag.len(),
            scroll: self.scroll.len(),
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
        ) & nodes(&self.drag_node[marks.drag..], node_base, &mut out.drag_node)
            & nodes(
                &self.scroll_node[marks.scroll..],
                node_base,
                &mut out.scroll_node,
            )
            & nodes(&self.key_node[marks.key..], node_base, &mut out.key_node);
        fn copy<T: Clone>(from: &[T], out: &mut Vec<T>) {
            out.clear();
            out.extend_from_slice(from);
        }
        copy(&self.click[marks.click..], &mut out.click);
        copy(&self.drag[marks.drag..], &mut out.drag);
        copy(&self.scroll[marks.scroll..], &mut out.scroll);
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
        self.drag_node.extend(shift(&from.drag_node, node_base));
        self.drag.extend_from_slice(&from.drag);
        self.scroll_node.extend(shift(&from.scroll_node, node_base));
        self.scroll.extend_from_slice(&from.scroll);
        self.key_node.extend(shift(&from.key_node, node_base));
        self.key_binding.extend_from_slice(&from.key_binding);
        self.key_action.extend_from_slice(&from.key_action);
    }

    fn click(&self, node: usize) -> Option<&ClickHandler> {
        let i = self.click_node.iter().position(|n| *n == node)?;
        self.click.get(i)
    }

    fn drag(&self, node: usize) -> Option<&DragStart> {
        let i = self.drag_node.iter().position(|n| *n == node)?;
        self.drag.get(i)
    }

    fn scroll(&self, node: usize) -> Option<&ScrollTarget> {
        let i = self.scroll_node.iter().position(|n| *n == node)?;
        self.scroll.get(i)
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
    drag: usize,
    scroll: usize,
    key: usize,
}

/// What one painted frame registered for input.
#[derive(Default)]
pub struct InputFrame {
    pub hits: HitTable,
    pub handlers: InputHandlers,
    pub semantic: SemanticFrame,
}

/// The outcome of routing one event: the semantic node that handled it
/// and the actions it emitted.
#[derive(Debug, Default)]
pub struct Delivery {
    pub node: Option<usize>,
    pub actions: Vec<Action>,
}

/// The drag holding the pointer. Frames come and go during a drag, so it
/// remembers its node by identity and finds it again in each new frame.
struct Capture {
    identity: u64,
    /// The node in the current frame, if it is still there.
    node: Option<usize>,
    drag: Box<dyn DragHandler>,
}

/// Routes pointer, wheel, and key input through the last painted frame.
#[derive(Default)]
pub struct InputRouter {
    frame: InputFrame,
    focus_tree: FocusTree,
    capture: Option<Capture>,
    /// Wheel motion in lines not yet delivered, so slow trackpad motion
    /// adds up instead of rounding away.
    wheel_lines: f32,
}

impl InputRouter {
    pub fn set_frame(&mut self, frame: InputFrame) {
        self.focus_tree = frame.semantic.focus_tree();
        self.frame = frame;
        if let Some(capture) = &mut self.capture {
            let identities = node_identities(&self.frame.semantic);
            capture.node = identities.iter().position(|id| *id == capture.identity);
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
    /// on the route.
    pub fn pointer_down(&mut self, x: f32, y: f32, focus: &mut Option<FocusId>) -> Delivery {
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
            (handlers.drag(node).is_some() || handlers.click(node).is_some()).then(Vec::new)
        });
        if let Some(node) = drag.node
            && let Some(start) = handlers.drag(node)
        {
            let mut drag = start.start(ClickEvent { x, y });
            let actions = drag.on_press();
            self.capture = Some(Capture {
                identity: node_identities(&self.frame.semantic)[node],
                node: Some(node),
                drag,
            });
            return Delivery {
                node: Some(node),
                actions,
            };
        }
        self.walk(target, UiEventKind::Click, |node| {
            handlers
                .click(node)
                .map(|handler| handler.invoke(ClickEvent { x, y }))
        })
    }

    /// Moves go to the capturing drag, wherever the pointer is.
    pub fn pointer_move(&mut self, x: f32, y: f32) -> Delivery {
        match &mut self.capture {
            Some(capture) => Delivery {
                node: capture.node,
                actions: capture.drag.on_move(x, y),
            },
            None => Delivery::default(),
        }
    }

    /// Release ends pointer capture.
    pub fn pointer_up(&mut self) -> Delivery {
        match self.capture.take() {
            Some(mut capture) => Delivery {
                node: capture.node,
                actions: capture.drag.on_release().actions,
            },
            None => Delivery::default(),
        }
    }

    /// End pointer capture without a release from the platform, which will
    /// not send one once the window has lost focus. The drag still gets its
    /// release, so handlers that act while the button is held (selection
    /// autoscroll) stop.
    pub fn cancel_pointer(&mut self) -> Delivery {
        self.wheel_lines = 0.0;
        self.pointer_up()
    }

    /// Wheel goes to the innermost scrollable under the pointer, in whole
    /// lines of [`WHEEL_LINE_PX`]; positive `delta_px` scrolls down. The
    /// fraction of a line left over carries into the next call. A node at
    /// its limit in the wheel's direction passes the input to the next
    /// scrollable ancestor; when every one is at its limit, the innermost
    /// still gets it so the app can clamp or rubber-band.
    pub fn wheel(&mut self, x: f32, y: f32, delta_px: f32) -> Delivery {
        let Some(target) = self.target_at(x, y) else {
            self.wheel_lines = 0.0;
            return Delivery::default();
        };
        // A reversal starts over rather than first paying off the old
        // direction's fraction.
        if self.wheel_lines * delta_px < 0.0 {
            self.wheel_lines = 0.0;
        }
        self.wheel_lines += delta_px / WHEEL_LINE_PX;
        let lines = self.wheel_lines.trunc() as i32;
        if lines == 0 {
            return Delivery::default();
        }
        self.wheel_lines -= lines as f32;
        let handlers = &self.frame.handlers;
        let mut innermost = None;
        let delivery = self.walk(target, UiEventKind::Wheel, |node| {
            let scroll = handlers.scroll(node)?;
            innermost.get_or_insert(node);
            (!scroll.at_limit(lines)).then(|| vec![scroll.builder.build(lines)])
        });
        if delivery.node.is_some() {
            return delivery;
        }
        match innermost.and_then(|node| Some((node, handlers.scroll(node)?))) {
            Some((node, scroll)) => Delivery {
                node: Some(node),
                actions: vec![scroll.builder.build(lines)],
            },
            None => delivery,
        }
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
                };
            }
            if self.binding_stops(node, kind, phase) {
                return Delivery {
                    node: Some(node),
                    actions: Vec::new(),
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
    fn cancel_pointer_releases_the_drag_and_ends_capture() {
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

        assert_eq!([cancelled, moved], ["handle [Release]", "-"]);
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
        use crate::transcript::{
            TextMeasurer, Transcript, TranscriptBlock, TranscriptEvent, TranscriptMessage,
            TranscriptRole, TranscriptStyle,
        };
        use crate::virtual_list::RowKey;

        #[derive(Debug, Clone, PartialEq)]
        struct Ev(TranscriptEvent);

        impl From<Ev> for Action {
            fn from(ev: Ev) -> Self {
                Action::new(ev)
            }
        }

        type Messages = HashMap<RowKey, TranscriptMessage>;

        fn message(i: u64, text: &str) -> TranscriptMessage {
            TranscriptMessage {
                key: RowKey(i),
                role: TranscriptRole::Assistant,
                author: format!("author {i}").into(),
                blocks: vec![TranscriptBlock::plain(BlockKey(i), text)],
            }
        }

        /// A 400x300 transcript stuck to the bottom, with shared text state
        /// so each frame is painted the way the adapter paints it.
        struct Chat {
            messages: Messages,
            transcript: Transcript,
            text: TextSystem,
            layouts: LayoutCache,
            router: InputRouter,
        }

        impl Chat {
            fn new(n: u64) -> Self {
                let messages: Messages = (0..n)
                    .map(|i| (RowKey(i), message(i, &format!("Message {i} text."))))
                    .collect();
                let mut transcript = Transcript::new(TranscriptStyle::for_font_size(14.0));
                transcript
                    .extend((0..n).map(|i| &messages[&RowKey(i)]))
                    .unwrap();
                let mut chat = Self {
                    messages,
                    transcript,
                    text: TextSystem::vendored_only(&Default::default()),
                    layouts: LayoutCache::default(),
                    router: InputRouter::default(),
                };
                chat.frame();
                chat.transcript.jump_to_latest();
                chat.frame();
                chat
            }

            /// Prepare and paint a frame; route input through it from now on.
            fn frame(&mut self) {
                let (w, h) = (400.0, 300.0);
                self.transcript.prepare(
                    w,
                    h,
                    0,
                    &self.messages,
                    &mut TextMeasurer::new(&mut self.text, &mut self.layouts, 14.0, 1.0),
                );
                let theme = Theme::default_dark();
                let signals = SignalStore::new();
                let mut root = self
                    .transcript
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
                        self.transcript.handle(*event);
                    }
                }
            }

            fn block_rect(&self, key: u64) -> Rect {
                self.transcript
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
                self.transcript.update(&message).unwrap();
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
                chat.transcript.selected_text(&chat.messages),
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
