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

use std::cell::{Cell, RefCell};
use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::animation::{AnimKey, Curve, PropId};
use crate::design::ScrollbarSz;

/// Distance one arrow key scrolls.
pub const KEY_LINE_PX: f32 = 2.0 * WHEEL_LINE_PX;
/// How long scrollbars of an auto-hiding container stay after it scrolls.
pub const SCROLLBAR_LINGER_MS: u64 = 1000;

/// Smooth scrolls (programmatic and keyboard) take this long.
const SMOOTH_SCROLL: Motion = Motion::Tween {
    duration_ms: 220,
    delay_ms: 0,
    curve: Curve::EaseOutCubic,
};
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
/// Animation table props of a handle's smooth scroll; below the transition
/// range so the per-frame transition sweep leaves them alone.
const PROP_X: PropId = PropId(0xE000);
const PROP_Y: PropId = PropId(0xE001);

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
    pub fn target(self, from: f32, viewport: f32, max: f32) -> f32 {
        let to = match self {
            Self::By(_, px) => from + px,
            Self::Page(_, true) => from + page_px(viewport),
            Self::Page(_, false) => from - page_px(viewport),
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
    from: [f32; 2],
    velocity: [f32; 2],
    start_ms: u64,
}

impl Fling {
    /// Offset and speed (points/ms) at `now_ms`.
    fn at(&self, now_ms: u64) -> ([f32; 2], f32) {
        let t = now_ms.saturating_sub(self.start_ms) as f32;
        let decay = (-t / FLING_TAU_MS).exp();
        let travel = FLING_TAU_MS * (1.0 - decay);
        let offset = [
            self.from[0] + self.velocity[0] * travel,
            self.from[1] + self.velocity[1] * travel,
        ];
        let speed = self.velocity[0].hypot(self.velocity[1]) * decay;
        (offset, speed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Request {
    Offset {
        to: [f32; 2],
        smooth: bool,
    },
    Item {
        key: u64,
        align: ScrollAlign,
        smooth: bool,
    },
}

#[derive(Debug)]
struct ScrollState {
    offset: [f32; 2],
    max: [f32; 2],
    viewport: Rect,
    /// Keyed descendants painted last frame, relative to the content origin.
    items: Vec<(u64, Rect)>,
    /// The same, being collected this frame.
    recording: Vec<(u64, Rect)>,
    request: Option<Request>,
    /// Target of the smooth scroll in progress.
    smooth: Option<[f32; 2]>,
    fling: Option<Fling>,
    samples: Samples,
    /// Offset painted last frame, and the clock when it last changed.
    painted: [f32; 2],
    changed_ms: Option<u64>,
    /// The axis whose thumb is held.
    dragging: Option<Axis>,
    key: AnimKey,
}

/// Shared, retained scroll state of one container; clones refer to the same
/// state. Keep one per scrolling container in app state and pass it to
/// [`Div::track_scroll`] every frame.
///
/// A handle read inside a [`cached`](super::cached) boundary goes stale on
/// replay: include [`Self::offset`] in that boundary's inputs hash.
#[derive(Clone)]
pub struct ScrollHandle(Rc<RefCell<ScrollState>>);

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
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(Rc::new(RefCell::new(ScrollState {
            offset: [0.0; 2],
            max: [0.0; 2],
            viewport: Rect::default(),
            items: Vec::new(),
            recording: Vec::new(),
            request: None,
            smooth: None,
            fling: None,
            samples: Samples::default(),
            painted: [0.0; 2],
            changed_ms: None,
            dragging: None,
            // Hash the counter so handle keys spread over the key space
            // instead of sitting next to small app-chosen keys.
            key: AnimKey(
                quark::stable_hash("scroll-handle") ^ id.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            ),
        })))
    }

    /// Offset painted last frame (or set since): `(x, y)`, positive when the
    /// content has moved left and up.
    pub fn offset(&self) -> (f32, f32) {
        let s = self.0.borrow();
        (s.offset[0], s.offset[1])
    }

    /// Largest offset on each axis as of the last frame.
    pub fn max_offset(&self) -> (f32, f32) {
        let s = self.0.borrow();
        (s.max[0], s.max[1])
    }

    /// The container's bounds last frame, in window points.
    pub fn viewport(&self) -> Rect {
        self.0.borrow().viewport
    }

    /// Jump to `(x, y)`, clamped to the content on the next frame.
    pub fn set_offset(&self, x: f32, y: f32) {
        self.request(Request::Offset {
            to: [x, y],
            smooth: false,
        });
    }

    /// Scroll smoothly to `(x, y)`; a jump under reduced motion or without
    /// an animation table.
    pub fn animate_to(&self, x: f32, y: f32) {
        self.request(Request::Offset {
            to: [x, y],
            smooth: true,
        });
    }

    /// Jump so the descendant with sibling key `key` (see [`Div::key`]) sits
    /// at `align` on each scrolling axis. Items are found by the bounds they
    /// had in the last frame (or in the next one, at the cost of a frame).
    pub fn scroll_to_item(&self, key: &str, align: ScrollAlign) {
        self.request(Request::Item {
            key: quark::stable_hash(key),
            align,
            smooth: false,
        });
    }

    /// [`Self::scroll_to_item`], scrolling smoothly.
    pub fn animate_to_item(&self, key: &str, align: ScrollAlign) {
        self.request(Request::Item {
            key: quark::stable_hash(key),
            align,
            smooth: true,
        });
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
        let mut sample = Sample {
            delta: [0.0; 2],
            at_ms: now_ms,
        };
        sample.delta[axis.index()] = delta;
        s.samples.push(sample);
        let i = axis.index();
        let to = (s.offset[i] + delta).clamp(0.0, s.max[i]);
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
        let axis = scroll.axis();
        let i = axis.index();
        let mut to = s.smooth.unwrap_or(s.offset);
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
    pub(crate) fn set_axis(&self, axis: Axis, offset: f32) {
        let mut s = self.0.borrow_mut();
        let i = axis.index();
        let to = offset.clamp(0.0, s.max[i]);
        s.smooth = None;
        s.fling = None;
        s.request = None;
        if to != s.offset[i] {
            s.offset[i] = to;
            bump_epoch();
        }
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

    /// Whether the scrollbars of an auto-hiding container should show at
    /// `now_ms` because it scrolled recently; asks for the frame that hides
    /// them.
    pub(crate) fn recently_scrolled(&self, cx: &mut ElementContext) -> bool {
        let Some(changed) = self.0.borrow().changed_ms else {
            return false;
        };
        let until = changed + SCROLLBAR_LINGER_MS;
        if cx.clock_ms < until {
            cx.request_frame_at_ms(until);
            true
        } else {
            false
        }
    }

    /// Start a frame: take the container's `viewport` and `content` size,
    /// resolve requests, advance a smooth scroll or fling, and return the
    /// offset to paint children at. Keyed descendants painted before
    /// [`Self::end_frame`] are recorded for `scroll_to_item`.
    pub(crate) fn begin_frame(
        &self,
        viewport: Rect,
        content: (f32, f32),
        axes: ScrollAxes,
        cx: &mut ElementContext,
    ) -> (f32, f32) {
        let now = cx.clock_ms;
        let reduced_motion = cx.theme.reduced_motion;
        let mut s = self.0.borrow_mut();
        let s = &mut *s;
        s.viewport = viewport;
        for axis in Axis::BOTH {
            let i = axis.index();
            s.max[i] = if axes.has(axis) {
                (axis.of(content) - axis.span(viewport).1).max(0.0)
            } else {
                0.0
            };
        }
        let clamp =
            |to: [f32; 2], max: [f32; 2]| [to[0].clamp(0.0, max[0]), to[1].clamp(0.0, max[1])];

        if let Some(request) = s.request {
            let resolved = match request {
                Request::Offset { to, smooth } => Some((to, smooth)),
                Request::Item { key, align, smooth } => {
                    item_offset(&s.items, key, align, s.offset, viewport, axes)
                        .map(|to| (to, smooth))
                }
            };
            if let Some((to, smooth)) = resolved {
                s.request = None;
                s.fling = None;
                let to = clamp(to, s.max);
                match cx.animations_mut() {
                    Some(table) if smooth && !reduced_motion && to != s.offset => {
                        for (prop, i) in [(PROP_X, 0), (PROP_Y, 1)] {
                            table.set(s.key, prop, s.offset[i], now);
                            table.animate_to(s.key, prop, to[i], SMOOTH_SCROLL, now);
                        }
                        s.smooth = Some(to);
                    }
                    _ => {
                        s.offset = to;
                        s.smooth = None;
                    }
                }
            }
        }

        if let Some(target) = s.smooth {
            let key = s.key;
            let moving = match cx.animations_mut() {
                Some(table) => {
                    s.offset = [
                        table.get(key, PROP_X).unwrap_or(target[0]),
                        table.get(key, PROP_Y).unwrap_or(target[1]),
                    ];
                    let moving = table.is_animating(key, PROP_X) || table.is_animating(key, PROP_Y);
                    if !moving {
                        table.remove(key, PROP_X);
                        table.remove(key, PROP_Y);
                    }
                    moving
                }
                None => false,
            };
            if moving {
                // The table schedules the frame; this marks the output as
                // clock-dependent for cache boundaries.
                cx.request_frame_at_ms(now);
            } else {
                s.offset = target;
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
                cx.request_frame_at_ms(now);
            }
        }

        s.offset = clamp(s.offset, s.max);
        if s.offset != s.painted {
            s.painted = s.offset;
            s.changed_ms = Some(now);
        }
        s.recording.clear();
        (s.offset[0], s.offset[1])
    }

    /// Record a keyed descendant painted at `bounds` (window points) this
    /// frame.
    pub(crate) fn record_item(&self, key: u64, bounds: Rect) {
        let mut s = self.0.borrow_mut();
        let rect = Rect {
            x: bounds.x - s.viewport.x + s.offset[0],
            y: bounds.y - s.viewport.y + s.offset[1],
            ..bounds
        };
        s.recording.push((key, rect));
    }

    /// End the frame [`Self::begin_frame`] started. An item request that
    /// only this frame's items can satisfy is kept for the next frame,
    /// which it asks for; one no frame painted the item for is dropped.
    pub(crate) fn end_frame(&self, cx: &mut ElementContext) {
        let mut s = self.0.borrow_mut();
        let s = &mut *s;
        std::mem::swap(&mut s.items, &mut s.recording);
        if let Some(Request::Item { key, .. }) = s.request {
            if s.items.iter().any(|(k, _)| *k == key) {
                cx.request_frame_at_ms(cx.clock_ms);
            } else {
                s.request = None;
            }
        }
    }
}

/// The offset that puts item `key` at `align`, on the scrolling axes.
fn item_offset(
    items: &[(u64, Rect)],
    key: u64,
    align: ScrollAlign,
    offset: [f32; 2],
    viewport: Rect,
    axes: ScrollAxes,
) -> Option<[f32; 2]> {
    let item = items.iter().find(|(k, _)| *k == key)?.1;
    let mut to = offset;
    for axis in Axis::BOTH {
        if !axes.has(axis) {
            continue;
        }
        let i = axis.index();
        let (start, len) = axis.span(item);
        let view = axis.span(viewport).1;
        to[i] = match align {
            ScrollAlign::Start => start,
            ScrollAlign::End => start + len - view,
            ScrollAlign::Center => start + len / 2.0 - view / 2.0,
            ScrollAlign::Nearest if start < offset[i] || len > view => start,
            ScrollAlign::Nearest if start + len > offset[i] + view => start + len - view,
            ScrollAlign::Nearest => offset[i],
        };
    }
    Some(to)
}

// ---------------------------------------------------------------------------
// Scrollbars
// ---------------------------------------------------------------------------

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
pub(crate) fn scrollbars(
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
    /// start) under the pointer at `pointer` along the axis.
    pub fn offset_for_pointer(&self, pointer: f32, grab: f32) -> f32 {
        let (start, len) = self.axis.span(self.track);
        let range = len - self.axis.span(self.thumb).1;
        if range <= 0.0 {
            return 0.0;
        }
        (pointer - start - grab).clamp(0.0, range) / range * self.max
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
            if let ScrollSink::Handle(handle) = &self.sink {
                handle.set_dragging(Some(bar.axis));
            }
            return Vec::new();
        }
        let (thumb_start, _) = bar.axis.span(bar.thumb);
        let forward = self.press > thumb_start;
        let page = KeyScroll::Page(bar.axis, forward);
        match &self.sink {
            ScrollSink::Handle(handle) => {
                handle.set_axis(bar.axis, page.target(bar.offset, bar.viewport, bar.max));
                Vec::new()
            }
            ScrollSink::Builder(builder) => {
                let lines = (page_px(bar.viewport) / WHEEL_LINE_PX).round() as i32;
                vec![builder.build(if forward { lines } else { -lines })]
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
                handle.set_axis(self.bar.axis, to);
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
            ScrollSink::Builder(builder) => DragReleaseResult {
                actions: builder.on_drag_end.iter().cloned().collect(),
            },
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
}
