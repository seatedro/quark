use super::*;

// ---------------------------------------------------------------------------
// Bounds — the resolved rectangle for a laid-out element
// ---------------------------------------------------------------------------

pub type Bounds = Rect;

// ---------------------------------------------------------------------------
// ClickHandler / DragStart — pointer callbacks registered per semantic node
// ---------------------------------------------------------------------------

/// Click callback. Stored as `Rc<Fn>` so it can be cloned and invoked
/// repeatedly; a plain action is kept as itself, so `on_click(action)`
/// registers without allocating a closure every frame.
#[derive(Clone)]
pub struct ClickHandler(Click);

#[derive(Clone)]
enum Click {
    Action(Action),
    Fn(Rc<dyn Fn(ClickEvent) -> Vec<Action>>),
}

impl ClickHandler {
    pub fn new(f: impl Fn(ClickEvent) -> Vec<Action> + 'static) -> Self {
        Self(Click::Fn(Rc::new(f)))
    }

    /// Emit `action` on every click.
    pub fn from_action(action: Action) -> Self {
        Self(Click::Action(action))
    }

    pub fn invoke(&self, event: ClickEvent) -> Vec<Action> {
        match &self.0 {
            Click::Action(action) => vec![action.clone()],
            Click::Fn(f) => f(event),
        }
    }
}

impl std::fmt::Debug for ClickHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClickHandler(..)")
    }
}

/// Starts a drag on pointer down. The returned handler captures the
/// pointer: it receives every move and the release, wherever they land.
#[derive(Clone)]
pub struct DragStart(Rc<dyn Fn(ClickEvent) -> Box<dyn DragHandler>>);

impl DragStart {
    pub fn new(f: impl Fn(ClickEvent) -> Box<dyn DragHandler> + 'static) -> Self {
        Self(Rc::new(f))
    }

    pub fn start(&self, event: ClickEvent) -> Box<dyn DragHandler> {
        (self.0)(event)
    }
}

pub trait DragHandler {
    /// Called once right after the drag starts, before any move. Actions
    /// returned here are delivered with the press, so they act on the frame
    /// the user pressed in rather than whatever is on screen at the first
    /// move.
    fn on_press(&mut self) -> Vec<Action> {
        Vec::new()
    }
    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action>;
    fn on_release(&mut self) -> DragReleaseResult;
    /// The geometry of the frame the drag routes through: before
    /// [`Self::on_press`], and again each time a newly painted frame
    /// replaces it mid-drag. A handler that maps the pointer onto other
    /// elements (a dock's tab groups) looks them up here rather than
    /// trusting positions known when the view was built.
    fn set_geometry(&mut self, _geometry: &LayoutSnapshot) {}
    fn cursor(&self) -> CursorHint {
        CursorHint::Default
    }
}

pub struct DragReleaseResult {
    pub actions: Vec<Action>,
}

impl DragReleaseResult {
    pub fn empty() -> Self {
        Self {
            actions: Vec::new(),
        }
    }
}

pub struct ScrollbarDragHandler {
    action_builder: ScrollActionBuilder,
    track_top: f32,
    track_height: f32,
    thumb_height: f32,
    content_height: f32,
    viewport_height: f32,
    grab_offset: f32,
}

impl ScrollbarDragHandler {
    pub fn new(track: &ScrollbarTrack, click_y: f32) -> Self {
        let on_thumb =
            click_y >= track.thumb_top && click_y <= track.thumb_top + track.thumb_height;
        let grab_offset = if on_thumb {
            click_y - track.thumb_top
        } else {
            track.thumb_height / 2.0
        };
        Self {
            action_builder: track.action_builder.clone(),
            track_top: track.track_rect.y,
            track_height: track.track_rect.height,
            thumb_height: track.thumb_height,
            content_height: track.content_height,
            viewport_height: track.viewport_height,
            grab_offset,
        }
    }

    fn compute_scroll_action(&self, mouse_y: f32) -> Option<Action> {
        let thumb_top = (mouse_y - self.track_top - self.grab_offset)
            .clamp(0.0, self.track_height - self.thumb_height);
        let max_scroll = self.content_height - self.viewport_height;
        let scroll_range = self.track_height - self.thumb_height;
        let fraction = if scroll_range > 0.0 {
            thumb_top / scroll_range
        } else {
            0.0
        };
        let target_px = (fraction * max_scroll) as u32;

        self.action_builder.build_to_px(target_px)
    }
}

impl DragHandler for ScrollbarDragHandler {
    fn on_move(&mut self, _x: f32, y: f32) -> Vec<Action> {
        self.compute_scroll_action(y).into_iter().collect()
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: self.action_builder.on_drag_end.iter().cloned().collect(),
        }
    }
}

/// A scrollbar track region — registered during paint for click-to-scroll and drag.
#[derive(Debug, Clone)]
pub struct ScrollbarTrack {
    pub track_rect: Rect,
    pub thumb_top: f32,
    pub thumb_height: f32,
    pub content_height: f32,
    pub viewport_height: f32,
    pub action_builder: ScrollActionBuilder,
}

/// How to convert scroll input into app actions.
///
/// `by_lines` handles wheel deltas (in lines). `to_px` handles scrollbar
/// drags with an absolute pixel offset; without it, dragging the thumb
/// emits nothing. `on_drag_end` is emitted when a scrollbar drag releases.
#[derive(Clone)]
pub struct ScrollActionBuilder {
    pub by_lines: Rc<dyn Fn(i32) -> Action>,
    pub to_px: Option<Rc<dyn Fn(u32) -> Action>>,
    pub on_drag_end: Option<Action>,
}

impl ScrollActionBuilder {
    pub fn new(by_lines: impl Fn(i32) -> Action + 'static) -> Self {
        Self {
            by_lines: Rc::new(by_lines),
            to_px: None,
            on_drag_end: None,
        }
    }

    pub fn with_to_px(mut self, to_px: impl Fn(u32) -> Action + 'static) -> Self {
        self.to_px = Some(Rc::new(to_px));
        self
    }

    pub fn with_drag_end(mut self, action: impl Into<Action>) -> Self {
        self.on_drag_end = Some(action.into());
        self
    }

    pub fn build(&self, delta: i32) -> Action {
        (self.by_lines)(delta)
    }

    pub fn build_to_px(&self, target_px: u32) -> Option<Action> {
        self.to_px.as_ref().map(|f| f(target_px))
    }
}

impl std::fmt::Debug for ScrollActionBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScrollActionBuilder")
            .field("to_px", &self.to_px.is_some())
            .field("on_drag_end", &self.on_drag_end)
            .finish_non_exhaustive()
    }
}

/// Click payload that swallows a click without doing anything. Elements
/// carrying it get a hitbox but no accessibility click action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoopAction;

impl From<NoopAction> for Action {
    fn from(value: NoopAction) -> Self {
        Action::new(value)
    }
}

pub use quark::hit::TooltipRegion;
