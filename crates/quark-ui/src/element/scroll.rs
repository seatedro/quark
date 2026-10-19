//! Retained scrolling: a [`ScrollHandle`] owns a container's offset across
//! frames, so the router can move it on wheel, scrollbar, and key input
//! without a round trip through the app, and the app can ask for
//! programmatic scrolls (`set_offset`, `animate_to`, `scroll_to_item`).
//!
//! Attach one with [`Div::track_scroll`] plus `overflow_x_scroll`,
//! `overflow_y_scroll`, or `overflow_scroll`. Each frame the div lays its
//! content out, hands the viewport and content size to the handle, and paints
//! at the offset the handle resolves: pending requests first, then a smooth
//! scroll or an inertial fling in progress, clamped to the content.
//!
//! The app-owned alternative (`scroll_y`/`scroll_x` with `on_scroll`
//! builders) stays: the app keeps the offset and turns line deltas into
//! actions. Both kinds chain per axis and get the same scrollbars.
//!
//! Scrollbars show persistently unless the container asks for
//! `scrollbar_auto_hide`. Then a [`ScrollbarVisibility`] decides when they
//! show: a handle keeps one, and a container whose offset the app owns
//! attaches its own with `scrollbar_visibility`.

use std::cell::{Cell, RefCell};

use super::*;
use crate::design::ScrollbarSz;

/// Distance one arrow key scrolls.
pub const KEY_LINE_PX: f32 = 2.0 * WHEEL_LINE_PX;
/// How long scrollbars of an auto-hiding container stay after it scrolls.
pub const SCROLLBAR_LINGER_MS: u64 = 1000;

/// Smooth scrolls (programmatic and keyboard) take this long, easing out
/// on a cubic.
const SMOOTH_SCROLL_MS: u64 = 220;
/// Time constant of a fling's exponential slowdown. 325 ms is the value
/// iOS-like kinetic scrolling uses: a fling covers `velocity * tau`.
const FLING_TAU_MS: f32 = 325.0;
/// Flings start above this speed and stop below the second, in points/ms.
const FLING_START_SPEED: f32 = 0.25;
const FLING_STOP_SPEED: f32 = 0.02;
/// Wheel samples this recent count toward a fling's velocity.
const VELOCITY_WINDOW_MS: u64 = 100;
/// Fingers that rested longer than this before lifting do not fling.
const LIFT_PAUSE_MS: u64 = 50;

/// One page of a `viewport`-long container: the viewport less two lines of
/// overlap, but at least half of it.
pub fn page_px(viewport: f32) -> f32 {
    (viewport - 2.0 * WHEEL_LINE_PX).max(viewport * 0.5)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    pub(crate) const BOTH: [Axis; 2] = [Axis::X, Axis::Y];

    pub(crate) fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
        }
    }

    /// Start and length of `rect` along this axis.
    fn span(self, rect: Rect) -> (f32, f32) {
        match self {
            Axis::X => (rect.x, rect.width),
            Axis::Y => (rect.y, rect.height),
        }
    }

    fn of(self, (x, y): (f32, f32)) -> f32 {
        match self {
            Axis::X => x,
            Axis::Y => y,
        }
    }
}

/// Which axes a container scrolls.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ScrollAxes {
    pub x: bool,
    pub y: bool,
}

impl ScrollAxes {
    pub fn has(self, axis: Axis) -> bool {
        match axis {
            Axis::X => self.x,
            Axis::Y => self.y,
        }
    }
}

/// Where [`ScrollHandle::scroll_to_item`] puts the item in the viewport.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollAlign {
    Start,
    Center,
    End,
    /// The least movement that shows the item; none when it is visible.
    #[default]
    Nearest,
}

/// How a programmatic scroll moves: at once, or as a smooth scroll (a
/// jump under reduced motion or without an animation table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollBehavior {
    #[default]
    Instant,
    Smooth,
}

/// Where [`ScrollHandle::scroll_to_item_with`] puts an item.
///
/// `inset` is in points per axis (`[x, y]`), and a positive value moves
/// the item toward the end of the axis (down, right):
///
/// - `Start`: the item starts `inset` inside the viewport (an item at
///   y=200 with inset 12 asks for offset 188);
/// - `End`: the item ends `inset` before the viewport end (a positive
///   inset therefore raises it);
/// - `Center`: the item's center sits `inset` past the viewport center;
/// - `Nearest`: the least movement that shows the item inside the
///   viewport shrunk by `inset` at both ends.
///
/// An item longer than the space its alignment gives it starts at the
/// start of that space instead. The offset is clamped to the content once,
/// after the item's geometry of the frame is known.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ScrollIntoView {
    pub align: ScrollAlign,
    pub inset: [f32; 2],
    pub behavior: ScrollBehavior,
}

impl ScrollIntoView {
    pub fn new(align: ScrollAlign) -> Self {
        Self {
            align,
            ..Self::default()
        }
    }

    pub fn inset(mut self, x: f32, y: f32) -> Self {
        self.inset = [x, y];
        self
    }

    pub fn smooth(mut self) -> Self {
        self.behavior = ScrollBehavior::Smooth;
        self
    }
}

/// A scroll a key press asks for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum KeyScroll {
    /// By points; positive is down or right.
    By(Axis, f32),
    /// By pages; `forward` is down or right.
    Page(Axis, bool),
    /// To the start or the end.
    Edge(Axis, bool),
}

impl KeyScroll {
    /// The scroll a key press asks for: arrows, Page Up/Down, Home/End,
    /// and Space (Shift+Space backwards), with no other modifiers.
    pub fn for_binding(pressed: &Binding) -> Option<Self> {
        let m = pressed.mods;
        if m.cmd || m.ctrl || m.alt || (m.shift && pressed.key != "space") {
            return None;
        }
        Some(match pressed.key.as_str() {
            "arrowup" => Self::By(Axis::Y, -KEY_LINE_PX),
            "arrowdown" => Self::By(Axis::Y, KEY_LINE_PX),
            "arrowleft" => Self::By(Axis::X, -KEY_LINE_PX),
            "arrowright" => Self::By(Axis::X, KEY_LINE_PX),
            "pageup" => Self::Page(Axis::Y, false),
            "pagedown" => Self::Page(Axis::Y, true),
            "space" => Self::Page(Axis::Y, !m.shift),
            "home" => Self::Edge(Axis::Y, false),
            "end" => Self::Edge(Axis::Y, true),
            _ => return None,
        })
    }

    pub fn axis(self) -> Axis {
        match self {
            Self::By(axis, _) | Self::Page(axis, _) | Self::Edge(axis, _) => axis,
        }
    }

    /// Whether it moves toward the end of the axis.
    pub fn forward(self) -> bool {
        match self {
            Self::By(_, px) => px > 0.0,
            Self::Page(_, forward) | Self::Edge(_, forward) => forward,
        }
    }

    /// The offset it moves `from` to, in a `viewport`-long container
    /// scrolling up to `max`.
    pub fn target(self, from: f64, viewport: f32, max: f64) -> f64 {
        let to = match self {
            Self::By(_, px) => from + f64::from(px),
            Self::Page(_, true) => from + f64::from(page_px(viewport)),
            Self::Page(_, false) => from - f64::from(page_px(viewport)),
            Self::Edge(_, forward) => {
                if forward {
                    max
                } else {
                    0.0
                }
            }
        };
        to.clamp(0.0, max.max(0.0))
    }
}

thread_local! {
    /// Bumped whenever input moves a handle, so the router can tell a
    /// delivery changed what is on screen even when it emitted no action.
    static EPOCH: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn scroll_epoch() -> u64 {
    EPOCH.with(Cell::get)
}

fn bump_epoch() {
    EPOCH.with(|epoch| epoch.set(epoch.get().wrapping_add(1)));
}

#[derive(Debug, Clone, Copy, Default)]
struct Sample {
    delta: [f32; 2],
    at_ms: u64,
}

/// The last few wheel deltas, for a fling's starting velocity.
#[derive(Debug, Default)]
struct Samples {
    ring: [Sample; 8],
    len: usize,
    next: usize,
}

impl Samples {
    fn push(&mut self, sample: Sample) {
        self.ring[self.next] = sample;
        self.next = (self.next + 1) % self.ring.len();
        self.len = (self.len + 1).min(self.ring.len());
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    /// Points per ms over the samples of the last [`VELOCITY_WINDOW_MS`]
    /// before `now_ms`. `None` without two distinct sample times, or when
    /// the newest sample is older than [`LIFT_PAUSE_MS`].
    fn velocity(&self, now_ms: u64) -> Option<[f32; 2]> {
        let recent = || {
            self.ring[..self.len]
                .iter()
                .filter(move |s| s.at_ms + VELOCITY_WINDOW_MS >= now_ms && s.at_ms <= now_ms)
        };
        let newest = recent().map(|s| s.at_ms).max()?;
        let oldest = recent().map(|s| s.at_ms).min()?;
        if newest + LIFT_PAUSE_MS < now_ms || newest == oldest {
            return None;
        }
        // The oldest sample's motion happened before the window opened.
        let mut sum = [0.0; 2];
        for s in recent().filter(|s| s.at_ms > oldest) {
            sum[0] += s.delta[0];
            sum[1] += s.delta[1];
        }
        let span = (newest - oldest) as f32;
        Some([sum[0] / span, sum[1] / span])
    }
}

/// Inertial motion after a trackpad fling. The speed decays exponentially,
/// in closed form, so the path does not depend on when frames land.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Fling {
    from: [f64; 2],
    velocity: [f32; 2],
    start_ms: u64,
}

impl Fling {
    /// Offset and speed (points/ms) at `now_ms`.
    fn at(&self, now_ms: u64) -> ([f64; 2], f32) {
        let t = now_ms.saturating_sub(self.start_ms) as f32;
        let decay = (-t / FLING_TAU_MS).exp();
        let travel = FLING_TAU_MS * (1.0 - decay);
        let offset = [
            self.from[0] + f64::from(self.velocity[0] * travel),
            self.from[1] + f64::from(self.velocity[1] * travel),
        ];
        let speed = self.velocity[0].hypot(self.velocity[1]) * decay;
        (offset, speed)
    }
}

/// A smooth scroll: an ease-out cubic from `from` to `to` over
/// [`SMOOTH_SCROLL_MS`] from `start_ms`, all in `f64`, so a scroll far
/// into long content lands exactly on its target.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Smooth {
    from: [f64; 2],
    to: [f64; 2],
    start_ms: u64,
}

impl Smooth {
    /// Offset at `now_ms`, and whether the scroll is still under way.
    fn at(&self, now_ms: u64) -> ([f64; 2], bool) {
        let t = now_ms.saturating_sub(self.start_ms) as f64 / SMOOTH_SCROLL_MS as f64;
        if t >= 1.0 {
            return (self.to, false);
        }
        let eased = 1.0 - (1.0 - t).powi(3);
        let at = |i: usize| self.from[i] + (self.to[i] - self.from[i]) * eased;
        ([at(0), at(1)], true)
    }
}

/// What [`ScrollHandle::advance`] resolves a frame's offset against.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ScrollStep {
    /// The viewport's size.
    pub size: (f32, f32),
    /// The content's size.
    pub content: (f64, f64),
    pub axes: ScrollAxes,
    pub now_ms: u64,
    /// Smooth scrolls jump and flings stop where they are.
    pub reduced_motion: bool,
    /// Smooth scrolls animate; without it they jump (a div painted without
    /// an animation table).
    pub animate: bool,
    /// An item request waits for the item's bounds in this frame
    /// ([`ScrollHandle::end_frame`]), as a div's does. Otherwise it lands
    /// by the bounds recorded last frame.
    pub defer_items: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Request {
    Offset {
        to: [f64; 2],
        smooth: bool,
    },
    Item {
        key: u64,
        align: ScrollAlign,
        inset: [f32; 2],
        smooth: bool,
    },
}

/// Offsets and extents are `f64`: content hundreds of millions of points
/// long (a long line, millions of rows) needs offsets `f32` cannot hold to
/// the point. They become `f32` only relative to the container's origin
/// (see [`Div::scroll_origin`]), for painting.
#[derive(Debug)]
struct ScrollState {
    offset: [f64; 2],
    max: [f64; 2],
    /// The content position the children are laid out from this frame.
    origin: [f64; 2],
    viewport: Rect,
    /// Keyed descendants painted last frame, relative to `items_origin`,
    /// the origin of that frame.
    items: Vec<(u64, Rect)>,
    items_origin: [f64; 2],
    /// The same, being collected this frame.
    recording: Vec<(u64, Rect)>,
    request: Option<Request>,
    /// An item request taken this frame, with the offset the frame started
    /// at: [`ScrollHandle::end_frame`] resolves it against the item's
    /// bounds in this frame.
    item_request: Option<(Request, [f64; 2])>,
    smooth: Option<Smooth>,
    fling: Option<Fling>,
    samples: Samples,
    /// The axis whose thumb is held.
    dragging: Option<Axis>,
    /// Bumped by every wheel, thumb, key, and requested scroll, even one
    /// clamped to no movement, but not by an owner's own corrections
    /// ([`ScrollHandle::rebase`]): a document tells user intent from
    /// layout by it.
    input: u64,
}

/// Shared, retained scroll state of one container; clones refer to the same
/// state. Keep one per scrolling container in app state and pass it to
/// [`Div::track_scroll`] every frame.
///
/// A [`cached`](super::cached) boundary watches the handles its subtree
/// paints and rebuilds when one moves or has motion pending, so its inputs
/// hash need not cover the offset.
#[derive(Clone)]
pub struct ScrollHandle(Rc<RefCell<ScrollState>>, ScrollbarVisibility);

impl Default for ScrollHandle {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ScrollHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = self.0.borrow();
        f.debug_struct("ScrollHandle")
            .field("offset", &s.offset)
            .field("max", &s.max)
            .finish_non_exhaustive()
    }
}

impl ScrollHandle {
    pub fn new() -> Self {
        let state = Rc::new(RefCell::new(ScrollState {
            offset: [0.0; 2],
            max: [0.0; 2],
            origin: [0.0; 2],
            viewport: Rect::default(),
            items: Vec::new(),
            items_origin: [0.0; 2],
            recording: Vec::new(),
            request: None,
            item_request: None,
            smooth: None,
            fling: None,
            samples: Samples::default(),
            dragging: None,
            input: 0,
        }));
        Self(state, ScrollbarVisibility::new())
    }

    /// When the container's auto-hiding scrollbars show; shared by every
    /// clone of the handle.
    pub(crate) fn scrollbar_visibility(&self) -> &ScrollbarVisibility {
        &self.1
    }

    /// Offset painted last frame (or set since): `(x, y)`, positive when the
    /// content has moved left and up. Rounded to `f32`: past 2^24 points
    /// it steps by whole points or more; [`Self::offset_f64`] is exact.
    pub fn offset(&self) -> (f32, f32) {
        let (x, y) = self.offset_f64();
        (x as f32, y as f32)
    }

    /// [`Self::offset`] as the handle keeps it.
    pub fn offset_f64(&self) -> (f64, f64) {
        let s = self.0.borrow();
        (s.offset[0], s.offset[1])
    }

    /// Largest offset on each axis as of the last frame.
    pub fn max_offset(&self) -> (f32, f32) {
        let (x, y) = self.max_offset_f64();
        (x as f32, y as f32)
    }

    /// [`Self::max_offset`] as the handle keeps it.
    pub fn max_offset_f64(&self) -> (f64, f64) {
        let s = self.0.borrow();
        (s.max[0], s.max[1])
    }

    /// The container's bounds last frame, in window points.
    pub fn viewport(&self) -> Rect {
        self.0.borrow().viewport
    }

    /// Jump to `(x, y)`, clamped to the content on the next frame.
    pub fn set_offset(&self, x: impl Into<f64>, y: impl Into<f64>) {
        self.request(Request::Offset {
            to: [x.into(), y.into()],
            smooth: false,
        });
    }

    /// Scroll smoothly to `(x, y)`; a jump under reduced motion or without
    /// an animation table.
    pub fn animate_to(&self, x: impl Into<f64>, y: impl Into<f64>) {
        self.request(Request::Offset {
            to: [x.into(), y.into()],
            smooth: true,
        });
    }

    /// Jump so the descendant with sibling key `key` (see [`Div::key`]) sits
    /// at `align` on each scrolling axis; [`Self::scroll_to_item_with`]
    /// with no inset.
    pub fn scroll_to_item(&self, key: &str, align: ScrollAlign) {
        self.scroll_to_item_with(key, ScrollIntoView::new(align));
    }

    /// [`Self::scroll_to_item`], scrolling smoothly.
    pub fn animate_to_item(&self, key: &str, align: ScrollAlign) {
        self.scroll_to_item_with(key, ScrollIntoView::new(align).smooth());
    }

    /// Scroll so the descendant with sibling key `key` (see [`Div::key`])
    /// sits where `options` say, on each scrolling axis. The item's bounds
    /// in the frame being painted decide the offset, so the first frame
    /// that paints the item already shows it in place. A request for a key
    /// no frame paints is dropped.
    pub fn scroll_to_item_with(&self, key: &str, options: ScrollIntoView) {
        self.request(Request::Item {
            key: quark::stable_hash(key),
            align: options.align,
            inset: options.inset,
            smooth: options.behavior == ScrollBehavior::Smooth,
        });
    }

    /// Whether the offset is final until new input: no request waits for
    /// the next frame and no smooth scroll or fling is moving the content.
    pub fn is_settled(&self) -> bool {
        let s = self.0.borrow();
        s.request.is_none() && s.smooth.is_none() && s.fling.is_none()
    }

    /// Whether a smooth scroll or fling is moving the content.
    pub fn is_moving(&self) -> bool {
        let s = self.0.borrow();
        s.smooth.is_some() || s.fling.is_some()
    }

    fn request(&self, request: Request) {
        let mut s = self.0.borrow_mut();
        s.request = Some(request);
        s.fling = None;
        s.input += 1;
    }

    /// Changes with every wheel, thumb, key, and requested scroll since the
    /// handle was made, moved or not, and with nothing else.
    pub(crate) fn input_generation(&self) -> u64 {
        self.0.borrow().input
    }

    /// Drop a pending request and stop a smooth scroll or fling where it
    /// is: the owner moved the offset itself.
    pub(crate) fn stop(&self) {
        let mut s = self.0.borrow_mut();
        s.request = None;
        s.smooth = None;
        s.fling = None;
    }

    /// Follow content that moved under the viewport along `axis`: the
    /// offset becomes `to` (the owner's anchoring correction), a smooth
    /// scroll or fling in progress moves with it, keeping its velocity and
    /// timing, and the offset is clamped to the new `max`. With
    /// `follows_end` the move only follows the end of the content (a view
    /// pinned to the bottom): a smooth scroll keeps its target, unless it
    /// was headed for the end.
    pub(crate) fn rebase(&self, axis: Axis, to: f64, max: f64, follows_end: bool) {
        let mut s = self.0.borrow_mut();
        let i = axis.index();
        let delta = to - s.offset[i];
        let old_max = s.max[i];
        if let Some(smooth) = &mut s.smooth {
            smooth.from[i] += delta;
            if !follows_end || smooth.to[i] >= old_max {
                smooth.to[i] += delta;
            }
        }
        if let Some(fling) = &mut s.fling {
            fling.from[i] += delta;
        }
        s.max[i] = max.max(0.0);
        s.offset[i] = to.clamp(0.0, s.max[i]);
    }

    /// Whether the content can move along `axis` toward its end
    /// (`forward`) or its start.
    pub(crate) fn can_scroll(&self, axis: Axis, forward: bool) -> bool {
        let s = self.0.borrow();
        let i = axis.index();
        if forward {
            s.offset[i] < s.max[i]
        } else {
            s.offset[i] > 0.0
        }
    }

    /// Move by `delta` points on `axis` now, as wheel input does: it ends
    /// any smooth scroll or fling and is clamped to the content. Records the
    /// motion at `now_ms` for a later [`Self::fling`].
    pub(crate) fn scroll_by(&self, axis: Axis, delta: f32, now_ms: u64) -> bool {
        let mut s = self.0.borrow_mut();
        s.smooth = None;
        s.fling = None;
        s.request = None;
        s.input += 1;
        let mut sample = Sample {
            delta: [0.0; 2],
            at_ms: now_ms,
        };
        sample.delta[axis.index()] = delta;
        s.samples.push(sample);
        let i = axis.index();
        let to = (s.offset[i] + f64::from(delta)).clamp(0.0, s.max[i]);
        let moved = to != s.offset[i];
        s.offset[i] = to;
        if moved {
            bump_epoch();
        }
        moved
    }

    /// Start inertia from the recent wheel velocity, as when fingers lift
    /// off a trackpad that does not send momentum itself. Returns whether
    /// a fling started.
    pub(crate) fn fling(&self, now_ms: u64) -> bool {
        let mut s = self.0.borrow_mut();
        let velocity = s.samples.velocity(now_ms);
        s.samples.clear();
        let Some(velocity) = velocity else {
            return false;
        };
        if velocity[0].hypot(velocity[1]) < FLING_START_SPEED {
            return false;
        }
        s.fling = Some(Fling {
            from: s.offset,
            velocity,
            start_ms: now_ms,
        });
        s.input += 1;
        bump_epoch();
        true
    }

    pub(crate) fn stop_fling(&self) {
        self.0.borrow_mut().fling = None;
    }

    /// Smoothly apply a key press's scroll, from where a smooth scroll in
    /// progress is headed so repeated presses add up. Returns whether it
    /// moves anything.
    pub(crate) fn key_scroll(&self, scroll: KeyScroll) -> bool {
        let mut s = self.0.borrow_mut();
        s.input += 1;
        let axis = scroll.axis();
        let i = axis.index();
        let mut to = s.smooth.map_or(s.offset, |smooth| smooth.to);
        if let Some(Request::Offset { to: pending, .. }) = s.request {
            to = pending;
        }
        let from = to[i];
        to[i] = scroll.target(from, axis.span(s.viewport).1, s.max[i]);
        if to[i] == from {
            return false;
        }
        s.request = Some(Request::Offset { to, smooth: true });
        s.fling = None;
        bump_epoch();
        true
    }

    /// Jump `axis` to `offset` now (scrollbar input).
    pub(crate) fn set_axis(&self, axis: Axis, offset: f64) {
        let mut s = self.0.borrow_mut();
        let i = axis.index();
        let to = offset.clamp(0.0, s.max[i]);
        s.smooth = None;
        s.fling = None;
        s.request = None;
        s.input += 1;
        if to != s.offset[i] {
            s.offset[i] = to;
            bump_epoch();
        }
    }

    /// Jump one page along `axis` now, toward the end when `forward` (a
    /// press on a scrollbar's track).
    pub(crate) fn page(&self, axis: Axis, forward: bool) {
        let to = {
            let s = self.0.borrow();
            let i = axis.index();
            KeyScroll::Page(axis, forward).target(s.offset[i], axis.span(s.viewport).1, s.max[i])
        };
        self.set_axis(axis, to);
    }

    pub(crate) fn set_dragging(&self, axis: Option<Axis>) {
        let mut s = self.0.borrow_mut();
        if s.dragging != axis {
            s.dragging = axis;
            bump_epoch();
        }
    }

    pub(crate) fn dragging(&self) -> Option<Axis> {
        self.0.borrow().dragging
    }

    /// The controller step: take the viewport and content sizes, resolve a
    /// pending request, and move a smooth scroll or fling to `now_ms`,
    /// clamped to the content. Returns whether motion goes on, so the
    /// caller asks for the next frame. Changing to reduced motion while
    /// moving lands a smooth scroll on its target and stops a fling where
    /// it is.
    pub(crate) fn advance(&self, step: ScrollStep) -> bool {
        let now = step.now_ms;
        let mut s = self.0.borrow_mut();
        let s = &mut *s;
        for axis in Axis::BOTH {
            let i = axis.index();
            let (content, view) = match axis {
                Axis::X => (step.content.0, step.size.0),
                Axis::Y => (step.content.1, step.size.1),
            };
            s.max[i] = if step.axes.has(axis) {
                (content - f64::from(view)).max(0.0)
            } else {
                0.0
            };
        }
        let clamp =
            |to: [f64; 2], max: [f64; 2]| [to[0].clamp(0.0, max[0]), to[1].clamp(0.0, max[1])];
        let animate = step.animate && !step.reduced_motion;

        s.item_request = None;
        match s.request.take() {
            Some(Request::Offset { to, smooth }) => {
                s.fling = None;
                let to = clamp(to, s.max);
                s.start_scroll(to, smooth && animate, now);
            }
            Some(request @ Request::Item { smooth, .. }) => {
                s.fling = None;
                let from = s.offset;
                let viewport = Rect {
                    width: step.size.0,
                    height: step.size.1,
                    ..s.viewport
                };
                let found =
                    request.item_offset(&s.items, s.items_origin, from, viewport, step.axes);
                if step.defer_items {
                    // Jump to where the item was last frame now, so a still
                    // item costs no second prepaint; `end_frame` corrects
                    // the offset when this frame moved it.
                    if !smooth && let Some(to) = found {
                        s.start_scroll(clamp(to, s.max), false, now);
                    }
                    s.item_request = Some((request, from));
                } else if let Some(to) = found {
                    s.start_scroll(clamp(to, s.max), smooth && animate, now);
                }
            }
            None => {}
        }

        if step.reduced_motion {
            if let Some(smooth) = s.smooth.take() {
                s.offset = smooth.to;
            }
            if let Some(fling) = s.fling.take() {
                s.offset = fling.at(now).0;
            }
        }

        let mut moving = false;
        if let Some(smooth) = s.smooth {
            let (at, more) = smooth.at(now);
            s.offset = at;
            if more {
                moving = true;
            } else {
                s.smooth = None;
            }
        }

        if let Some(fling) = s.fling {
            let (to, speed) = fling.at(now);
            let clamped = clamp(to, s.max);
            s.offset = clamped;
            // A fling ends when it slows down or runs into an edge on every
            // axis it moves along.
            let blocked = (0..2).all(|i| fling.velocity[i] == 0.0 || clamped[i] != to[i]);
            if speed < FLING_STOP_SPEED || blocked {
                s.fling = None;
            } else {
                moving = true;
            }
        }

        s.offset = clamp(s.offset, s.max);
        moving
    }

    /// Start a frame: take the container's `viewport`, its `content` size,
    /// and the content position its children are laid out from, resolve
    /// requests, advance a smooth scroll or fling ([`Self::advance`]), and
    /// return the offset to paint children at: the scroll offset less
    /// `origin`. Keyed descendants painted before [`Self::end_frame`] are
    /// recorded for `scroll_to_item`.
    pub(crate) fn begin_frame(
        &self,
        viewport: Rect,
        content: (f64, f64),
        origin: (f64, f64),
        axes: ScrollAxes,
        cx: &mut ElementContext,
    ) -> (f32, f32) {
        {
            let mut s = self.0.borrow_mut();
            s.viewport = viewport;
            s.origin = [origin.0, origin.1];
        }
        let now = cx.clock_ms;
        let moving = self.advance(ScrollStep {
            size: (viewport.width, viewport.height),
            content,
            axes,
            now_ms: now,
            reduced_motion: cx.theme.reduced_motion,
            animate: cx.animations().is_some(),
            defer_items: true,
        });
        if moving {
            // Also marks the output as clock-dependent for cache
            // boundaries.
            cx.request_frame_at_ms(now);
        }
        self.watch(viewport, cx)
    }

    /// Start a frame whose offset [`Self::advance`] resolved already, as a
    /// document does while preparing its rows: take the container's
    /// `viewport` (window points), lay the children out at the offset
    /// itself, and ask for the next frame while motion goes on. Keyed
    /// descendants painted before [`Self::end_frame`] are recorded for the
    /// next `advance`.
    pub(crate) fn begin_resolved_frame(&self, viewport: Rect, cx: &mut ElementContext) {
        let moving = {
            let mut s = self.0.borrow_mut();
            s.viewport = viewport;
            s.origin = s.offset;
            s.smooth.is_some() || s.fling.is_some()
        };
        if moving {
            cx.request_frame_at_ms(cx.clock_ms);
        }
        self.watch(viewport, cx);
    }

    /// Start collecting keyed descendants and note the handle for an
    /// enclosing cache boundary. Returns the offset to paint children at.
    fn watch(&self, viewport: Rect, cx: &mut ElementContext) -> (f32, f32) {
        let mut s = self.0.borrow_mut();
        s.recording.clear();
        cx.watch_scroll(|| ScrollWatch {
            handle: self.clone(),
            offset: s.offset,
            dragging: s.dragging,
            viewport,
        });
        s.paint_offset()
    }

    /// Record a keyed descendant painted at `bounds` (window points) this
    /// frame.
    pub(crate) fn record_item(&self, key: u64, bounds: Rect) {
        let mut s = self.0.borrow_mut();
        let (dx, dy) = s.paint_offset();
        let rect = Rect {
            x: bounds.x - s.viewport.x + dx,
            y: bounds.y - s.viewport.y + dy,
            ..bounds
        };
        s.recording.push((key, rect));
    }

    /// End the frame [`Self::begin_frame`] started, resolving an item
    /// request against the item's bounds in this frame (a request for an
    /// item the frame did not paint is dropped). Returns the offset to
    /// prepaint the children at again when a jump has to land elsewhere
    /// than where they were just prepainted; the caller prepaints them
    /// again between [`Self::restart_items`] and another `end_frame`.
    pub(crate) fn end_frame(
        &self,
        axes: ScrollAxes,
        cx: &mut ElementContext,
    ) -> Option<(f32, f32)> {
        let animate = !cx.theme.reduced_motion && cx.animations().is_some();
        let now = cx.clock_ms;
        let mut s = self.0.borrow_mut();
        let s = &mut *s;
        std::mem::swap(&mut s.items, &mut s.recording);
        s.items_origin = s.origin;
        let (request, from) = s.item_request.take()?;
        let Request::Item { smooth, .. } = request else {
            return None;
        };
        let to = request.item_offset(&s.items, s.origin, from, s.viewport, axes)?;
        let to = [to[0].clamp(0.0, s.max[0]), to[1].clamp(0.0, s.max[1])];
        if smooth && animate {
            // This frame paints the smooth scroll's first step, the offset
            // it starts from, wherever the item turned out to be.
            s.start_scroll(to, true, now);
            cx.request_frame_at_ms(now);
            return None;
        }
        if to == s.offset {
            return None;
        }
        s.offset = to;
        s.smooth = None;
        Some(s.paint_offset())
    }

    /// Collect keyed descendants afresh, for prepainting the children
    /// again after [`Self::end_frame`] moved the offset.
    pub(crate) fn restart_items(&self) {
        self.0.borrow_mut().recording.clear();
    }
}

/// A handle as a cache boundary's subtree painted it. The boundary's
/// recording may replay only while every handle it watched is
/// [`unchanged`](Self::unchanged): replay skips the container's
/// [`ScrollHandle::begin_frame`], which resolves requests and motion.
#[derive(Clone)]
pub(crate) struct ScrollWatch {
    handle: ScrollHandle,
    offset: [f64; 2],
    dragging: Option<Axis>,
    /// The container's bounds, in the coordinates of whoever holds the
    /// watch (window points, or relative to a boundary's origin).
    viewport: Rect,
}

impl ScrollWatch {
    /// Whether the container would paint as watched: same offset and thumb
    /// drag, nothing pending, nothing moving.
    pub(crate) fn unchanged(&self) -> bool {
        let s = self.handle.0.borrow();
        s.offset == self.offset
            && s.dragging == self.dragging
            && s.request.is_none()
            && s.smooth.is_none()
            && s.fling.is_none()
    }

    /// The watch with its viewport moved by `(dx, dy)`.
    pub(crate) fn offset(&self, dx: f32, dy: f32) -> Self {
        Self {
            viewport: self.viewport.offset(dx, dy),
            ..self.clone()
        }
    }

    /// A replay painted the container at the watched viewport: keep the
    /// handle's viewport (page size, item positions) current.
    pub(crate) fn replayed(&self) {
        self.handle.0.borrow_mut().viewport = self.viewport;
    }
}

impl ScrollState {
    /// The offset to paint the children at: the scroll offset from the
    /// origin they are laid out at, small enough for `f32`.
    fn paint_offset(&self) -> (f32, f32) {
        (
            (self.offset[0] - self.origin[0]) as f32,
            (self.offset[1] - self.origin[1]) as f32,
        )
    }

    /// Move to `to` (already clamped): a smooth scroll from the current
    /// offset starting at `now` when `smooth`, else at once.
    fn start_scroll(&mut self, to: [f64; 2], smooth: bool, now: u64) {
        if smooth && to != self.offset {
            self.smooth = Some(Smooth {
                from: self.offset,
                to,
                start_ms: now,
            });
        } else {
            self.offset = to;
            self.smooth = None;
        }
    }
}

impl Request {
    /// The unclamped offset an item request asks for, from `offset`, on
    /// the scrolling axes; `None` when `items` (relative to `origin`) lack
    /// the item.
    fn item_offset(
        &self,
        items: &[(u64, Rect)],
        origin: [f64; 2],
        offset: [f64; 2],
        viewport: Rect,
        axes: ScrollAxes,
    ) -> Option<[f64; 2]> {
        let Request::Item {
            key, align, inset, ..
        } = *self
        else {
            return None;
        };
        let item = items.iter().find(|(k, _)| *k == key)?.1;
        let mut to = offset;
        for axis in Axis::BOTH {
            if axes.has(axis) {
                let i = axis.index();
                let (start, len) = axis.span(item);
                let view = axis.span(viewport).1;
                // Aligned relative to the origin, where it all fits `f32`.
                let from = (offset[i] - origin[i]) as f32;
                to[i] =
                    origin[i] + f64::from(align_offset(align, start, len, view, inset[i], from));
            }
        }
        Some(to)
    }
}

/// The offset that puts an item spanning `start..start + len` of the
/// content at `align` in a `view`-long viewport scrolled to `offset`, with
/// `inset` as [`ScrollIntoView`] defines it. An item longer than the space
/// its alignment gives it starts at the start of that space.
fn align_offset(
    align: ScrollAlign,
    start: f32,
    len: f32,
    view: f32,
    inset: f32,
    offset: f32,
) -> f32 {
    match align {
        ScrollAlign::Start => start - inset,
        ScrollAlign::End if len > view - inset => start,
        ScrollAlign::End => start + len - view + inset,
        ScrollAlign::Center if len > view => start,
        ScrollAlign::Center => start + len / 2.0 - view / 2.0 - inset,
        ScrollAlign::Nearest => {
            let (low, high) = (offset + inset, offset + view - inset);
            if start < low || len > high - low {
                start - inset
            } else if start + len > high {
                start + len - view + inset
            } else {
                offset
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Scrollbar visibility
// ---------------------------------------------------------------------------

/// When the auto-hiding scrollbars of one container show, kept across
/// frames; clones refer to the same state. Besides while the pointer is
/// over the container or a thumb is held, the bars show for
/// [`SCROLLBAR_LINGER_MS`] after the painted offset moves (wheel, keys, a
/// drag, or the app setting it) or the container gains keyboard focus.
///
/// A [`ScrollHandle`] keeps one. A container whose offset the app owns
/// (`scroll_y` with `on_scroll`) keeps one in app state, next to the
/// offset, and attaches it each frame:
/// `.scrollbar_visibility(&state).scrollbar_auto_hide()`. Without one, such
/// a container's bars show only on hover and while held.
///
/// The bars are observed while the container prepaints, against the
/// frame's clock, and a lingering bar asks for the frame that hides it.
/// That request also keeps an enclosing [`cached`](super::cached) boundary
/// from replaying the shown bars past the deadline.
#[derive(Clone, Default)]
pub struct ScrollbarVisibility(Rc<Cell<Visibility>>);

#[derive(Debug, Clone, Copy, Default)]
struct Visibility {
    /// Offset and focus seen by the last frame; `None` before the first.
    seen: Option<([f64; 2], bool)>,
    /// When the offset last moved or focus last arrived.
    revealed_ms: Option<u64>,
}

impl std::fmt::Debug for ScrollbarVisibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let v = self.0.get();
        f.debug_struct("ScrollbarVisibility")
            .field("revealed_ms", &v.revealed_ms)
            .finish_non_exhaustive()
    }
}

impl ScrollbarVisibility {
    pub fn new() -> Self {
        Self::default()
    }

    /// Note the container's offset and focus at `now_ms`. A change from
    /// the last frame starts the linger; the first frame only sets the
    /// baseline, so a view does not flash its bars when it appears.
    pub(crate) fn observe(&self, offset: [f64; 2], focused: bool, now_ms: u64) {
        let mut v = self.0.get();
        if let Some((was_offset, was_focused)) = v.seen
            && (offset != was_offset || (focused && !was_focused))
        {
            v.revealed_ms = Some(now_ms);
        }
        v.seen = Some((offset, focused));
        self.0.set(v);
    }

    /// Whether the linger still runs at the frame's clock; asks for the
    /// frame that ends it.
    pub(crate) fn lingering(&self, cx: &mut ElementContext) -> bool {
        let Some(revealed) = self.0.get().revealed_ms else {
            return false;
        };
        let until = revealed + SCROLLBAR_LINGER_MS;
        if cx.clock_ms < until {
            cx.request_frame_at_ms(until);
            true
        } else {
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Scrollbars
// ---------------------------------------------------------------------------

thread_local! {
    /// The app-owned scrollbar whose thumb is held, by axis and track. An
    /// app-owned offset has no retained state of its own, and the track
    /// stays put through a drag, so it identifies the bar across frames.
    static HELD_THUMB: Cell<Option<(Axis, Rect)>> = const { Cell::new(None) };
}

/// Scrollbars of one scrolling container for a frame: geometry, hit
/// entries, and where their input goes. Every scrolling view uses this, so
/// thumbs drag, tracks page, and bars hover, hide, and size alike whether
/// the offset lives in a [`ScrollHandle`] or in app state.
#[derive(Default)]
pub(crate) struct Scrollbars([Option<BarSlot>; 2]);

struct BarSlot {
    bar: Scrollbar,
    /// Where input goes; a bar without one only shows the offset.
    sink: Option<ScrollSink>,
    hit: Option<HitId>,
}

impl BarSlot {
    fn held(&self) -> bool {
        match &self.sink {
            Some(ScrollSink::Handle(handle)) => handle.dragging() == Some(self.bar.axis),
            Some(ScrollSink::Builder(_)) => {
                HELD_THUMB.with(Cell::get) == Some((self.bar.axis, self.bar.track))
            }
            None => false,
        }
    }
}

/// What a scrolling container shows scrollbars for.
pub(crate) struct ScrollbarInput {
    /// The container, in window points.
    pub bounds: Rect,
    /// Content size and scroll offset, in content coordinates.
    pub content: (f64, f64),
    pub offset: (f64, f64),
    pub axes: ScrollAxes,
    /// Input target of each axis's bar: `[x, y]`.
    pub sinks: [Option<ScrollSink>; 2],
    /// Show the bars only while the pointer is over the container, a thumb
    /// is held, or `visibility` lingers.
    pub auto_hide: bool,
    /// What decides the linger of auto-hiding bars: a handle's, or one the
    /// view attached.
    pub visibility: Option<ScrollbarVisibility>,
    /// The container holds keyboard focus.
    pub focused: bool,
}

impl Scrollbars {
    /// Lay out the bars and give the visible ones with a sink hit entries.
    /// Call after the content's prepaint, under the container's clip, so
    /// the bars sit above the content.
    pub(crate) fn prepaint(input: ScrollbarInput, cx: &mut ElementContext) -> Self {
        let ScrollbarInput {
            bounds,
            content,
            offset,
            axes,
            mut sinks,
            auto_hide,
            visibility,
            focused,
        } = input;
        let visibility = visibility.filter(|_| auto_hide);
        // Observed before the overflow check, so content that starts to
        // overflow does not count an offset from frames ago as a scroll.
        if let Some(visibility) = &visibility {
            visibility.observe([offset.0, offset.1], focused, cx.clock_ms);
        }
        // The bars are a few hundred points long: f32 places the thumb.
        let bars = scrollbars(
            bounds,
            (content.0 as f32, content.1 as f32),
            (offset.0 as f32, offset.1 as f32),
            axes,
        );
        if bars.iter().all(Option::is_none) {
            return Self::default();
        }
        let mut slots = bars.map(|bar| {
            bar.map(|bar| BarSlot {
                sink: sinks[bar.axis.index()].take(),
                bar,
                hit: None,
            })
        });
        if auto_hide {
            let pointer_inside = cx
                .mouse_position
                .is_some_and(|(x, y)| bounds.contains(x, y) && cx.current_clip().contains(x, y));
            let visible = pointer_inside
                || slots.iter().flatten().any(BarSlot::held)
                || visibility.is_some_and(|v| v.lingering(cx));
            if !visible {
                return Self::default();
            }
        }
        for slot in slots.iter_mut().flatten() {
            if slot.sink.is_some() {
                slot.hit = Some(cx.insert_hit(
                    slot.bar.hit,
                    HitFlags::DRAG | HitFlags::HOVER,
                    CursorHint::Default,
                ));
            }
        }
        Self(slots)
    }

    /// Give each interactive bar a draggable semantic node under `parent`
    /// and route its presses. Call before painting the content, so a bar's
    /// node keeps its position (and its identity for drag capture) as the
    /// content changes.
    pub(crate) fn register(&self, parent: usize, cx: &mut ElementContext) {
        for slot in self.0.iter().flatten() {
            if let (Some(hit), Some(sink)) = (slot.hit, &slot.sink) {
                let mut node = SemanticNode::new(slot.bar.hit);
                node.parent = Some(parent);
                node.actions = SemanticActions::default().draggable();
                let node = cx.semantic.push(node);
                cx.bind_hit(hit, node);
                cx.handlers.on_scrollbar(node, slot.bar, sink.clone());
            }
        }
    }

    /// Paint the bars, brighter while hovered and brightest while held.
    pub(crate) fn paint(&self, scene: &mut Scene, cx: &ElementContext) {
        for slot in self.0.iter().flatten() {
            let hovered = slot.hit.is_some_and(|hit| cx.is_hovered(hit));
            slot.bar.paint(scene, cx.theme, hovered, slot.held());
        }
    }
}

/// One scrollbar's geometry for a frame, in window points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Scrollbar {
    pub axis: Axis,
    pub track: Rect,
    pub thumb: Rect,
    /// Where the pointer can grab it: the track, widened inward.
    pub hit: Rect,
    pub offset: f32,
    pub max: f32,
    pub viewport: f32,
}

/// Scrollbars of a container at `bounds`, on each axis in `axes` whose
/// `content` overflows. A vertical bar leaves room for a horizontal one.
fn scrollbars(
    bounds: Rect,
    content: (f32, f32),
    offset: (f32, f32),
    axes: ScrollAxes,
) -> [Option<Scrollbar>; 2] {
    let w = ScrollbarSz::WIDTH;
    let inset = ScrollbarSz::INSET;
    let overflows = |axis: Axis| axes.has(axis) && axis.of(content) > axis.span(bounds).1 + 0.5;
    let (has_x, has_y) = (overflows(Axis::X), overflows(Axis::Y));
    let bar = |axis: Axis, track: Rect, hit: Rect| {
        let viewport = axis.span(bounds).1;
        let content_len = axis.of(content);
        let max = (content_len - viewport).max(0.0);
        let offset = axis.of(offset).clamp(0.0, max);
        let (start, len) = axis.span(track);
        let thumb_len = (len * viewport / content_len)
            .max(ScrollbarSz::MIN_THUMB)
            .min(len);
        let thumb_start = start
            + if max > 0.0 {
                offset / max * (len - thumb_len)
            } else {
                0.0
            };
        let thumb = match axis {
            Axis::X => Rect {
                x: thumb_start,
                width: thumb_len,
                ..track
            },
            Axis::Y => Rect {
                y: thumb_start,
                height: thumb_len,
                ..track
            },
        };
        Scrollbar {
            axis,
            track,
            thumb,
            hit,
            offset,
            max,
            viewport,
        }
    };
    let corner = |other: bool| if other { w + inset } else { inset };
    let y = has_y.then(|| {
        let track = Rect {
            x: bounds.right() - w,
            y: bounds.y + inset,
            width: w,
            height: (bounds.height - inset - corner(has_x)).max(0.0),
        };
        let hit = Rect {
            x: track.x - ScrollbarSz::HIT_PAD,
            y: bounds.y,
            width: w + ScrollbarSz::HIT_PAD,
            height: (bounds.height - if has_x { w } else { 0.0 }).max(0.0),
        };
        bar(Axis::Y, track, hit)
    });
    let x = has_x.then(|| {
        let track = Rect {
            x: bounds.x + inset,
            y: bounds.bottom() - w,
            width: (bounds.width - inset - corner(has_y)).max(0.0),
            height: w,
        };
        let hit = Rect {
            x: bounds.x,
            y: track.y - ScrollbarSz::HIT_PAD,
            width: (bounds.width - if has_y { w } else { 0.0 }).max(0.0),
            height: w + ScrollbarSz::HIT_PAD,
        };
        bar(Axis::X, track, hit)
    });
    [x, y]
}

impl Scrollbar {
    /// The offset that puts the thumb's grab point (`grab` points from its
    /// start) under the pointer at `pointer` along the axis. A thumb at the
    /// end of its track asks for `f32::MAX`, which sinks clamp to their
    /// current end: content that grew since the press (a streaming
    /// transcript) is still reached, so a drag to the bottom stays there.
    /// A thumb that fills its track has no travel and keeps the offset.
    pub fn offset_for_pointer(&self, pointer: f32, grab: f32) -> f32 {
        let (start, len) = self.axis.span(self.track);
        let range = len - self.axis.span(self.thumb).1;
        if range <= 0.0 {
            return self.offset;
        }
        let travel = (pointer - start - grab).clamp(0.0, range);
        if travel >= range {
            return f32::MAX;
        }
        travel / range * self.max
    }

    /// Paint the track and thumb; `hovered` and `active` brighten the thumb.
    pub fn paint(&self, scene: &mut Scene, theme: &Theme, hovered: bool, active: bool) {
        let w = ScrollbarSz::WIDTH;
        scene.rounded_rect(RoundedRectPrimitive::uniform(
            self.track,
            w / 2.0,
            Color::rgba(128, 128, 128, 10),
        ));
        let base = theme.colors.scrollbar_thumb;
        let boost = if active {
            2.0
        } else if hovered {
            1.5
        } else {
            1.0
        };
        let color = base.with_alpha((f32::from(base.a) * boost).min(255.0) as u8);
        let t = self.thumb;
        scene.rounded_rect(RoundedRectPrimitive::uniform(
            Rect {
                x: t.x + 1.0,
                y: t.y + 1.0,
                width: (t.width - 2.0).max(0.0),
                height: (t.height - 2.0).max(0.0),
            },
            w / 2.0 - 1.0,
            color,
        ));
    }
}

/// Where a scrollbar sends its input.
#[derive(Clone)]
pub(crate) enum ScrollSink {
    Handle(ScrollHandle),
    /// App-owned offset: thumb drags emit `to_px`, track presses lines.
    Builder(ScrollActionBuilder),
}

/// A press on a scrollbar. On the thumb it drags: the content follows the
/// pointer. On the track it pages once toward the press.
pub(crate) struct ScrollbarDrag {
    bar: Scrollbar,
    sink: ScrollSink,
    press: f32,
    /// Distance from the thumb's start to the press, when on the thumb.
    grab: Option<f32>,
}

impl ScrollbarDrag {
    /// The drag a press at `(x, y)` on `bar` starts.
    pub fn new(bar: Scrollbar, sink: ScrollSink, x: f32, y: f32) -> Self {
        let press = bar.axis.of((x, y));
        let (thumb_start, thumb_len) = bar.axis.span(bar.thumb);
        let on_thumb = press >= thumb_start && press <= thumb_start + thumb_len;
        Self {
            bar,
            sink,
            press,
            grab: on_thumb.then_some(press - thumb_start),
        }
    }
}

impl DragHandler for ScrollbarDrag {
    fn on_press(&mut self) -> Vec<Action> {
        let bar = &self.bar;
        if self.grab.is_some() {
            match &self.sink {
                ScrollSink::Handle(handle) => handle.set_dragging(Some(bar.axis)),
                ScrollSink::Builder(_) => {
                    HELD_THUMB.with(|held| held.set(Some((bar.axis, bar.track))));
                    // Redraw for the held thumb's styling.
                    bump_epoch();
                }
            }
            return Vec::new();
        }
        let (thumb_start, _) = bar.axis.span(bar.thumb);
        let forward = self.press > thumb_start;
        match &self.sink {
            ScrollSink::Handle(handle) => {
                // From the handle's exact offset, not the bar's rounding.
                handle.page(bar.axis, forward);
                Vec::new()
            }
            // An absolute offset when the app takes one: its wheel lines
            // need not be `WHEEL_LINE_PX` long.
            ScrollSink::Builder(builder) => {
                let page = KeyScroll::Page(bar.axis, forward);
                let to = page.target(f64::from(bar.offset), bar.viewport, f64::from(bar.max));
                match builder.build_to_px(to as u32) {
                    Some(action) => vec![action],
                    None => {
                        let lines = (page_px(bar.viewport) / WHEEL_LINE_PX).round() as i32;
                        vec![builder.build(if forward { lines } else { -lines })]
                    }
                }
            }
        }
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        let Some(grab) = self.grab else {
            return Vec::new();
        };
        let to = self.bar.offset_for_pointer(self.bar.axis.of((x, y)), grab);
        match &self.sink {
            ScrollSink::Handle(handle) => {
                handle.set_axis(self.bar.axis, f64::from(to));
                Vec::new()
            }
            ScrollSink::Builder(builder) => builder.build_to_px(to as u32).into_iter().collect(),
        }
    }

    fn on_release(&mut self) -> DragReleaseResult {
        match &self.sink {
            ScrollSink::Handle(handle) => {
                handle.set_dragging(None);
                DragReleaseResult::empty()
            }
            ScrollSink::Builder(builder) => {
                if self.grab.is_some() {
                    HELD_THUMB.with(|held| held.set(None));
                    bump_epoch();
                }
                DragReleaseResult {
                    actions: builder.on_drag_end.iter().cloned().collect(),
                }
            }
        }
    }

    /// The offset dragged to stays, as it does with every live drag; the
    /// release only lets go of the thumb and reports the drag's end.
    fn on_cancel(&mut self) -> Vec<Action> {
        self.on_release().actions
    }
}

#[cfg(kani)]
mod verification {
    use super::*;

    /// Any vertical bar (track, thumb length, offset, and max in whole
    /// points), held anywhere on its thumb, with the pointer anywhere: the
    /// offset asked for is 0 with the thumb at or before the start of its
    /// track, the end once the thumb reaches the end, and in range between.
    /// A thumb that fills its track keeps the offset. One pointer per run:
    /// comparing two (monotonicity) doubles the float divisions, and that
    /// proof ran past the 30 minute job. The bar comes from arbitrary
    /// values rather than `scrollbars` for the same reason.
    #[kani::proof]
    #[kani::solver(kissat)]
    fn a_dragged_thumb_asks_for_an_offset_in_range() {
        let len = f32::from(kani::any::<u8>());
        let thumb_len = f32::from(kani::any::<u8>());
        let max = f32::from(kani::any::<u16>());
        let offset = f32::from(kani::any::<u16>());
        let grab = f32::from(kani::any::<u8>());
        kani::assume(thumb_len <= len && 0.0 < max && offset <= max && grab <= thumb_len);
        let track = Rect {
            x: 0.0,
            y: 6.0,
            width: ScrollbarSz::WIDTH,
            height: len,
        };
        let bar = Scrollbar {
            axis: Axis::Y,
            track,
            thumb: Rect {
                height: thumb_len,
                ..track
            },
            hit: track,
            offset,
            max,
            viewport: 0.0,
        };
        let pointer = f32::from(kani::any::<i16>() % 512);

        let asked = bar.offset_for_pointer(pointer, grab);

        let travel = pointer - track.y - grab;
        if len - thumb_len <= 0.0 {
            // A thumb that fills its track has nowhere to go.
            assert!(asked == offset);
        } else if travel >= len - thumb_len {
            assert!(asked == f32::MAX);
        } else if travel <= 0.0 {
            assert!(asked == 0.0);
        } else {
            assert!(0.0 < asked && asked <= max);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn velocity_needs_recent_motion() {
        let mut samples = Samples::default();
        for (i, dy) in [10.0, 10.0, 10.0, 10.0].into_iter().enumerate() {
            samples.push(Sample {
                delta: [0.0, dy],
                at_ms: 1_000 + i as u64 * 10,
            });
        }
        let cases = [
            // Lifted right after the last sample: 30 points over 30 ms.
            (1_035, Some([0.0, 1.0])),
            // Rested 60 ms before lifting.
            (1_090, None),
        ];
        for (now, expected) in cases {
            assert_eq!(samples.velocity(now), expected, "lift at {now}");
        }
    }

    mod items {
        use super::super::*;
        use crate::animation::AnimationTable;
        use crate::style::Styled;
        use crate::theme::Theme;

        /// What [`list`] holds.
        #[derive(Clone, Copy)]
        struct Rows {
            /// Unnamed 40-point rows before `row-0`.
            before: usize,
            /// The last `row-{i}`.
            last: usize,
            /// A 150-point `tall` row after the others.
            tall: bool,
        }

        const TEN: Rows = Rows {
            before: 0,
            last: 9,
            tall: false,
        };

        /// A 200x100 list scrolling both ways over 400x40 rows `row-0`
        /// ..= `row-{last}` (content y = 40 * i without rows before them);
        /// `row-3` holds a 50-point `cell` at x=250.
        fn list(handle: &ScrollHandle, rows: Rows) -> Div {
            let row = |name: &str| div().w(400.0).h(40.0).flex_shrink_0().flex_row().key(name);
            let named = |i: usize| {
                let name = format!("row-{i}");
                row(&name)
                    .test_id(name.as_str())
                    .on_click(NoopAction)
                    .when(i == 3, |row| {
                        row.child(div().w(250.0).h(40.0))
                            .child(div().w(50.0).h(40.0).key("cell").test_id("cell"))
                    })
            };
            let before = (0..rows.before).map(|i| row(&format!("before-{i}")).into_any());
            let named = (0..=rows.last).map(|i| named(i).into_any());
            div()
                .w(200.0)
                .h(100.0)
                .flex_col()
                .track_scroll(handle)
                .overflow_scroll()
                .children(before.chain(named))
                .when(rows.tall, |list| {
                    list.child(
                        div()
                            .w(400.0)
                            .h(150.0)
                            .flex_shrink_0()
                            .key("tall")
                            .test_id("tall"),
                    )
                })
        }

        struct Harness {
            text: TextSystem,
            layouts: LayoutCache,
            theme: Theme,
            signals: SignalStore,
            animations: AnimationTable,
            handle: ScrollHandle,
        }

        impl Harness {
            fn new() -> Self {
                Self {
                    text: TextSystem::vendored_only(&Default::default()),
                    layouts: LayoutCache::default(),
                    theme: Theme::default_dark(),
                    signals: SignalStore::new(),
                    animations: AnimationTable::new(),
                    handle: ScrollHandle::new(),
                }
            }

            /// Paint [`list`] of `rows` at clock `now_ms`.
            fn frame(&mut self, rows: Rows, now_ms: u64) -> InputRouter {
                let root = list(&self.handle, rows);
                self.paint(root, now_ms)
            }

            /// Paint `root` in a 200x100 window at clock `now_ms`.
            fn paint(&mut self, root: Div, now_ms: u64) -> InputRouter {
                self.animations.tick(now_ms);
                let mut cx = ElementContext::new(
                    &self.theme,
                    1.0,
                    &mut self.text,
                    &mut self.layouts,
                    None,
                    &self.signals,
                )
                .with_clock(now_ms)
                .with_animations(&mut self.animations);
                cx.semantic = SemanticFrame::new(200.0, 100.0);
                let mut root = root.into_any();
                render_element(&mut root, &mut Scene::default(), &mut cx, 200.0, 100.0);
                cx.finish_frame();
                let mut router = InputRouter::default();
                router.set_frame(cx.take_input_frame());
                router
            }
        }

        /// `(x, y)` of `test_id` as painted, in the viewport.
        fn at(router: &InputRouter, test_id: &str) -> (f32, f32) {
            let rect = router.frame().geometry.by_test_id(test_id).unwrap().bounds;
            (rect.x, rect.y)
        }

        fn start(inset: f32) -> ScrollIntoView {
            ScrollIntoView::new(ScrollAlign::Start).inset(0.0, inset)
        }

        // The item is where the previous frame painted it; one frame after
        // the request it sits where the request puts it.
        #[test]
        fn item_lands_at_its_alignment_and_inset() {
            let with = |align, x, y| ScrollIntoView::new(align).inset(x, y);
            let tall = Rows { tall: true, ..TEN };
            // Without `tall` the content is 400 high; offsets reach 300.
            let cases = [
                ("row-5", start(12.0), TEN, (0.0, 12.0)),
                ("row-5", with(ScrollAlign::End, 0.0, 12.0), TEN, (0.0, 48.0)),
                (
                    "row-5",
                    with(ScrollAlign::Center, 0.0, 10.0),
                    TEN,
                    (0.0, 40.0),
                ),
                (
                    "row-1",
                    with(ScrollAlign::Nearest, 0.0, 12.0),
                    TEN,
                    (0.0, 40.0),
                ),
                (
                    "row-2",
                    with(ScrollAlign::Nearest, 0.0, 12.0),
                    TEN,
                    (0.0, 48.0),
                ),
                ("row-0", start(12.0), TEN, (0.0, 0.0)),
                ("row-9", start(12.0), TEN, (0.0, 60.0)),
                (
                    "cell",
                    with(ScrollAlign::Start, 8.0, 12.0),
                    TEN,
                    (50.0, 12.0),
                ),
                ("tall", with(ScrollAlign::End, 0.0, 0.0), tall, (0.0, 0.0)),
            ];
            for (key, options, rows, expected) in cases {
                let mut h = Harness::new();
                h.frame(rows, 0);
                h.handle.scroll_to_item_with(key, options);
                let router = h.frame(rows, 0);
                assert_eq!(at(&router, key), expected, "{key} {options:?}");
            }
        }

        // A prompt appended in the same update that asks to scroll to it:
        // the first frame painting it shows it at the inset.
        #[test]
        fn new_item_reaches_its_inset_in_the_frame_that_adds_it() {
            let mut h = Harness::new();
            h.frame(Rows { last: 5, ..TEN }, 0);
            h.handle.scroll_to_item_with("row-6", start(12.0));
            let router = h.frame(TEN, 0);

            assert_eq!(at(&router, "row-6"), (0.0, 12.0));
        }

        // Rows inserted above move the item in the frame that resolves the
        // request: it lands by its new position, and the pointer finds it
        // where it is painted.
        #[test]
        fn item_moved_by_this_frame_lands_by_its_new_position() {
            let mut h = Harness::new();
            h.frame(TEN, 0);
            h.handle.scroll_to_item_with("row-5", start(0.0));
            let router = h.frame(Rows { before: 2, ..TEN }, 0);

            assert_eq!(at(&router, "row-5"), (0.0, 0.0));
            let node = router.target_at(100.0, 20.0).unwrap();
            let name = router.frame().semantic.nodes()[node].test_id.clone();
            assert_eq!(name.as_ref().map(TestId::as_str), Some("row-5"));
        }

        // Catches offsets kept in f32, which steps by 64 points at 1e9:
        // content a billion points wide (a 64 MiB line is half that) must
        // still scroll by half a point and paint where it scrolled to.
        #[test]
        fn content_a_billion_points_wide_scrolls_and_paints_exactly() {
            const FAR: f64 = 1e9;
            let mut h = Harness::new();
            // Children laid out from just before `FAR`; the mark sits at
            // content x = FAR + 37.
            let line = |handle: &ScrollHandle| {
                div()
                    .w(200.0)
                    .h(100.0)
                    .track_scroll(handle)
                    .overflow_x_scroll()
                    .scroll_total_x(2.0 * FAR)
                    .scroll_origin(FAR - 100.0, 0.0)
                    .child(div().w(1000.0).h(40.0).flex_row().children([
                        div().w(137.0).h(40.0).flex_shrink_0().into_any(),
                        div().w(10.0).h(40.0).test_id("mark").into_any(),
                    ]))
            };
            h.paint(line(&h.handle.clone()), 0);
            h.handle.set_offset(FAR + 0.5, 0.0);
            let router = h.paint(line(&h.handle.clone()), 0);

            assert_eq!(h.handle.offset_f64(), (FAR + 0.5, 0.0));
            assert_eq!(at(&router, "mark"), (36.5, 0.0));

            h.handle.scroll_by(Axis::X, 0.25, 16);
            let router = h.paint(line(&h.handle.clone()), 16);
            assert_eq!(at(&router, "mark"), (36.25, 0.0));
        }

        #[test]
        fn smooth_item_scroll_ends_at_the_inset() {
            let mut h = Harness::new();
            h.frame(TEN, 0);
            h.handle.scroll_to_item_with("row-5", start(12.0).smooth());
            let first = h.frame(TEN, 0);
            let midway = h.frame(TEN, 100);
            let landed = h.frame(TEN, 400);

            assert_eq!(at(&first, "row-5"), (0.0, 200.0), "starts where it was");
            let y = at(&midway, "row-5").1;
            assert!(y < 200.0 && y > 12.0, "midway at {y}");
            assert_eq!(at(&landed, "row-5"), (0.0, 12.0));
        }
    }
}
