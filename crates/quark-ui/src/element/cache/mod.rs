//! Cached elements: a [`cached`] boundary keeps its subtree's layout and
//! paint output in the window's [`ElementCache`] and replays them, moved to
//! the boundary's new origin, until something they depend on changes.
//!
//! # Contract
//!
//! A boundary replays without calling its build closure when all of these
//! match the frame that recorded it:
//!
//! - its key and inputs hash (the hash must cover every value the closure
//!   reads, including app state like selection);
//! - every measure query its parent makes (the size it is offered: a new
//!   width rebuilds it; the frame runs layout once more);
//! - the scale factor, the theme (the cache compares the theme each
//!   frame and bumps a generation when it changes), and the text system's
//!   font epoch (which system, and its font generation): a replay looks up
//!   no text, so without it a font change, or another text system, would
//!   replay geometry shaped with the old fonts;
//! - the focused element, if the subtree read focus;
//! - inherited paint state: the z layer, the text and icon color pushed by
//!   an ancestor's hover, and whether an ancestor hides text from
//!   accessibility;
//! - with devtools, the inspector's style overrides.
//!
//! Interaction state inside the subtree invalidates it instead of going
//! stale:
//!
//! - **Hover**: a subtree with hit entries rebuilds whenever the pointer is
//!   inside the union of those entries, and an output recorded with the
//!   pointer inside is never replayed. Moving the pointer out rebuilds once.
//! - **Animation and the clock**: a subtree that asks for another frame or
//!   has a transition in motion is rebuilt every frame until it settles.
//!   Transition rows of a replayed subtree are kept alive.
//! - **Text inputs**: a subtree with a text input is never replayed.
//! - **Scroll handles**: the boundary watches every [`ScrollHandle`] its
//!   subtree paints and rebuilds when one has moved, has a request pending,
//!   or is in motion. Replays keep the handles' viewports current.
//!
//! A boundary is a block box: the content is laid out at the width the
//! parent gives the boundary and keeps its own height, unless
//! [`Cached::fill_height`] lays it out in the boundary's full height. Style
//! the boundary itself (grow, fixed size) through [`Styled`] on the
//! [`Cached`].
//!
//! Replay re-registers everything the subtree registered: scene
//! primitives, hit entries (clipped again under the current ancestor
//! clips), semantic nodes and their handlers, accessibility nodes,
//! tooltips, selectable text, and scrollbar tracks. The devtools inspector
//! sees a replayed boundary as one element.
//!
//! Entries live in column tables keyed by [`CacheKey`] and are evicted
//! after [`EVICT_AFTER_PASSES`] layout passes unused. Without a cache
//! attached to the context, a boundary builds every frame.

use std::collections::HashMap;
use std::hash::{BuildHasherDefault, DefaultHasher, Hash, Hasher};

use super::layout::MeasureMemo;
use super::*;
use record::{Inherited, PaintRecord, Recording};

mod record;
#[cfg(test)]
mod tests;

/// Layout passes a frame may run: replayed boundaries that went stale are
/// rebuilt in the next pass, and the last pass replays nothing.
pub(super) const LAYOUT_PASSES: usize = 3;

/// Evicted rows whose buffers are kept for new rows.
const SPARE_ROWS: usize = 64;

/// Layout passes an entry survives unused before eviction.
pub const EVICT_AFTER_PASSES: u64 = 60;

/// Identity of a cache entry: a 64-bit stable hash of a [`UiKey`] or any
/// string, or a number the caller derives itself. Two boundaries with the
/// same key in one frame both build, and only the first records.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CacheKey(pub u64);

impl From<&UiKey> for CacheKey {
    fn from(key: &UiKey) -> Self {
        Self(quark::stable_hash(key.as_str()))
    }
}

impl From<UiKey> for CacheKey {
    fn from(key: UiKey) -> Self {
        Self::from(&key)
    }
}

impl From<&str> for CacheKey {
    fn from(key: &str) -> Self {
        Self(quark::stable_hash(key))
    }
}

impl From<u64> for CacheKey {
    fn from(key: u64) -> Self {
        Self(key)
    }
}

/// Hash of everything a cached subtree's build closure reads.
pub fn inputs_hash<T: Hash + ?Sized>(inputs: &T) -> u64 {
    let mut hasher = DefaultHasher::new();
    inputs.hash(&mut hasher);
    hasher.finish()
}

/// Keys are already hashes.
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for byte in bytes {
            self.0 = (self.0 << 8) ^ u64::from(*byte) ^ (self.0 >> 56);
        }
    }

    fn write_u64(&mut self, value: u64) {
        self.0 = value;
    }
}

/// What an entry was recorded under, beyond its key.
#[derive(Debug, Clone, Copy)]
struct EntryInputs {
    hash: u64,
    scale: f32,
    theme: u32,
    /// The fonts the subtree was shaped with.
    font: quark_text::FontEpoch,
    /// Whether accessibility nodes were built.
    accessibility: bool,
    /// The focus and its visibility the output read, when it read any.
    focus: Option<(Option<FocusId>, bool)>,
    /// The entry holds a complete recording that may replay.
    reusable: bool,
}

/// The window's element cache: one row per [`cached`] boundary, stored
/// column-wise, plus the layout engines a window reuses every frame.
/// Attach it with [`ElementContext::with_element_cache`].
#[derive(Default)]
pub struct ElementCache {
    /// Layout passes run so far; stamps rows for duplicates and eviction.
    pass: u64,
    /// Whether this pass may replay (false on the last pass of a frame).
    replay: bool,
    theme: Option<Theme>,
    theme_generation: u32,
    rows: HashMap<CacheKey, u32, BuildHasherDefault<KeyHasher>>,
    key: Vec<CacheKey>,
    last_pass: Vec<u64>,
    inputs: Vec<EntryInputs>,
    inherited: Vec<Inherited>,
    /// Union of the entry's hit bounds, relative to its origin.
    hit_extent: Vec<Option<Rect>>,
    memo: Vec<Vec<MeasureMemo>>,
    paint: Vec<PaintRecord>,
    /// Buffers of evicted rows, reused by new ones.
    spare_paint: Vec<PaintRecord>,
    spare_memo: Vec<Vec<MeasureMemo>>,
    engine: Option<LayoutEngine>,
    /// The context's working buffers between frames.
    pub(super) buffers: super::context::FrameBuffers,
    /// Engines of subtrees rebuilt after layout. Boxed because they move
    /// between this pool and elements; a box moves without reallocating.
    #[expect(clippy::vec_box, reason = "pooled boxes move in and out")]
    spare_engines: Vec<Box<LayoutEngine>>,
}

/// Per-frame state every entry depends on.
#[derive(Clone, Copy)]
struct FrameInputs {
    scale: f32,
    font: quark_text::FontEpoch,
    accessibility: bool,
    focus: (Option<FocusId>, bool),
}

/// How a boundary may use its row this pass.
enum Claim {
    /// No cache, or another boundary claimed the key this pass.
    Uncached,
    /// Build and record into the row.
    Build(u32),
    /// Replay the row.
    Replay(u32),
}

impl ElementCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Rows held.
    pub fn len(&self) -> usize {
        self.key.len()
    }

    pub fn is_empty(&self) -> bool {
        self.key.is_empty()
    }

    /// Drop every entry; the next frame rebuilds every boundary.
    pub fn clear(&mut self) {
        while !self.key.is_empty() {
            self.remove_row(self.key.len() - 1);
        }
    }

    fn claim(&mut self, key: CacheKey, hash: u64, frame: FrameInputs) -> Claim {
        let FrameInputs {
            scale,
            font,
            accessibility,
            focus,
        } = frame;
        let Some(&row) = self.rows.get(&key) else {
            let row = self.key.len() as u32;
            self.rows.insert(key, row);
            self.key.push(key);
            self.last_pass.push(self.pass);
            self.inputs.push(EntryInputs {
                hash,
                scale,
                theme: self.theme_generation,
                font,
                accessibility,
                focus: None,
                reusable: false,
            });
            self.inherited.push(Inherited::default());
            self.hit_extent.push(None);
            self.memo.push(self.spare_memo.pop().unwrap_or_default());
            self.paint.push(self.spare_paint.pop().unwrap_or_default());
            debug_assert_eq!(self.verify_integrity(), Ok(()));
            return Claim::Build(row);
        };
        let r = row as usize;
        if self.last_pass[r] == self.pass {
            return Claim::Uncached;
        }
        self.last_pass[r] = self.pass;
        let scroll_unchanged = self.paint[r].scroll_unchanged();
        let inputs = &mut self.inputs[r];
        let replay = self.replay
            && scroll_unchanged
            && inputs.reusable
            && inputs.hash == hash
            && inputs.scale == scale
            && inputs.theme == self.theme_generation
            && inputs.font == font
            && inputs.accessibility == accessibility
            && inputs.focus.is_none_or(|read| read == focus);
        if replay {
            Claim::Replay(row)
        } else {
            inputs.reusable = false;
            Claim::Build(row)
        }
    }

    fn remove_row(&mut self, row: usize) {
        self.rows.remove(&self.key[row]);
        self.key.swap_remove(row);
        self.last_pass.swap_remove(row);
        self.inputs.swap_remove(row);
        self.inherited.swap_remove(row);
        self.hit_extent.swap_remove(row);
        let mut memo = self.memo.swap_remove(row);
        if self.spare_memo.len() < SPARE_ROWS {
            memo.clear();
            self.spare_memo.push(memo);
        }
        let mut paint = self.paint.swap_remove(row);
        if self.spare_paint.len() < SPARE_ROWS {
            paint.clear();
            self.spare_paint.push(paint);
        }
        if let Some(moved) = self.key.get(row) {
            self.rows.insert(*moved, row as u32);
        }
    }

    fn evict(&mut self) {
        for row in (0..self.key.len()).rev() {
            if self.last_pass[row] + EVICT_AFTER_PASSES < self.pass {
                self.remove_row(row);
            }
        }
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    fn take_engine(&mut self) -> Box<LayoutEngine> {
        let mut engine = self.spare_engines.pop().unwrap_or_default();
        engine.clear();
        engine
    }

    /// Every column has one entry per row, and the key map points at them.
    pub fn verify_integrity(&self) -> Result<(), CacheIntegrityError> {
        let rows = self.key.len();
        let columns = [
            ("last_pass", self.last_pass.len()),
            ("inputs", self.inputs.len()),
            ("inherited", self.inherited.len()),
            ("hit_extent", self.hit_extent.len()),
            ("memo", self.memo.len()),
            ("paint", self.paint.len()),
        ];
        for (column, len) in columns {
            if len != rows {
                return Err(CacheIntegrityError::ColumnLength { column, len, rows });
            }
        }
        if self.rows.len() != rows {
            return Err(CacheIntegrityError::KeyMap);
        }
        for (row, key) in self.key.iter().enumerate() {
            if self.rows.get(key) != Some(&(row as u32)) {
                return Err(CacheIntegrityError::KeyMap);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheIntegrityError {
    ColumnLength {
        column: &'static str,
        len: usize,
        rows: usize,
    },
    /// The key map and the key column disagree.
    KeyMap,
}

// ---------------------------------------------------------------------------
// Frame hooks, called by `render_element`
// ---------------------------------------------------------------------------

/// The window's layout engine, cleared, and the cache's frame state.
pub(super) fn begin_frame(cx: &mut ElementContext) -> LayoutEngine {
    cx.load_buffers();
    // Each render resolves its own groups; an earlier root's are painted.
    cx.interaction.clear();
    let theme = cx.theme;
    let Some(cache) = cx.cache.as_deref_mut() else {
        return LayoutEngine::new();
    };
    if cache.theme.as_ref() != Some(theme) {
        cache.theme = Some(theme.clone());
        cache.theme_generation = cache.theme_generation.wrapping_add(1);
    }
    let mut engine = cache.engine.take().unwrap_or_default();
    engine.clear();
    engine
}

pub(super) fn begin_layout_pass(cx: &mut ElementContext, pass: usize) {
    if let Some(cache) = cx.cache.as_deref_mut() {
        cache.pass += 1;
        cache.replay = pass + 1 < LAYOUT_PASSES;
    }
}

/// Drop the recordings of `rows`; their boundaries rebuild next pass.
pub(super) fn invalidate(cx: &mut ElementContext, rows: &[u32]) {
    if let Some(cache) = cx.cache.as_deref_mut() {
        for &row in rows {
            cache.inputs[row as usize].reusable = false;
        }
    }
}

pub(super) fn end_frame(cx: &mut ElementContext, engine: LayoutEngine) {
    cx.store_buffers();
    if let Some(cache) = cx.cache.as_deref_mut() {
        cache.engine = Some(engine);
        cache.evict();
    }
}

// ---------------------------------------------------------------------------
// Cached — the boundary element
// ---------------------------------------------------------------------------

/// A subtree built by `build` and cached under `key` while `inputs_hash`
/// and the rest of the [contract](self) hold. The closure runs only when
/// the subtree has to be rebuilt.
///
/// ```ignore
/// cached(&row.key, inputs_hash(&(row.revision, selected)), move || row_view(row))
/// ```
pub fn cached<E: IntoAnyElement>(
    key: impl Into<CacheKey>,
    inputs_hash: u64,
    build: impl FnOnce() -> E + 'static,
) -> Cached<impl FnOnce() -> AnyElement + 'static> {
    Cached {
        key: key.into(),
        hash: inputs_hash,
        style: ElementStyle::default(),
        fill_height: false,
        build: Some(move || build().into_any()),
        state: State::Idle,
    }
}

/// Built by [`cached`].
pub struct Cached<F> {
    key: CacheKey,
    hash: u64,
    /// The boundary's own box in its parent; only `layout` is used.
    style: ElementStyle,
    fill_height: bool,
    build: Option<F>,
    state: State,
}

#[expect(
    clippy::large_enum_variant,
    reason = "inline so a rebuild does not allocate; the element's pooled box holds it"
)]
enum State {
    Idle,
    /// Replaying `row`.
    Replay {
        row: u32,
    },
    Live(Live),
}

/// A built subtree, laid out in a subtree of the parent's engine or, when
/// rebuilt after layout, in an engine of its own.
struct Live {
    child: AnyElement,
    layout: LiveLayout,
    /// The row to record into with the inputs hash, and the recording
    /// once begun.
    entry: Option<(u32, u64)>,
    recording: Option<Recording>,
}

enum LiveLayout {
    Subtree(usize),
    Own(Box<LayoutEngine>),
    /// Painted; an own engine went back to the cache.
    Done,
}

impl<F> Styled for Cached<F> {
    fn element_style_mut(&mut self) -> &mut ElementStyle {
        &mut self.style
    }
}

impl<F> Cached<F> {
    /// Lay the content out in the boundary's full height, so content sized
    /// with `h_full` spans a boundary whose height its parent sets (a
    /// divider stretched across a row). The content's own height is still
    /// what the boundary measures at. A replay at another height rebuilds,
    /// so the inputs hash need not cover it.
    pub fn fill_height(mut self) -> Self {
        self.fill_height = true;
        self
    }

    /// The inputs hash, mixed with what every entry depends on that the
    /// caller does not hash: the inspector's style overrides, which apply
    /// to elements inside the subtree while it builds.
    fn frame_hash(&self, cx: &ElementContext) -> u64 {
        #[cfg(feature = "devtools")]
        {
            let revision = cx.devtools.overrides.revision();
            self.hash ^ revision.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        }
        #[cfg(not(feature = "devtools"))]
        {
            let _ = cx;
            self.hash
        }
    }
}

impl<F: FnOnce() -> AnyElement + 'static> Cached<F> {
    fn build(&mut self) -> AnyElement {
        (self.build.take().expect("cached subtree built twice"))()
    }

    /// Build after layout, at the size layout already gave the boundary.
    fn rebuild(&mut self, row: u32, bounds: Bounds, cx: &mut ElementContext) -> Live {
        let mut engine = match cx.cache.as_deref_mut() {
            Some(cache) => cache.take_engine(),
            None => Box::default(),
        };
        let mut child = self.build();
        let content = child.request_layout(&mut engine, cx);
        engine.layout_boundary(
            content,
            bounds.width,
            bounds.height,
            self.fill_height,
            &mut cx.measure_context(),
        );
        Live {
            child,
            layout: LiveLayout::Own(engine),
            entry: Some((row, self.frame_hash(cx))),
            recording: None,
        }
    }
}

/// The boundary's position in its engine, without element offsets: what a
/// child laid out at the subtree origin must be offset by.
fn layout_offset(bounds: Bounds, cx: &ElementContext) -> (f32, f32) {
    let (x, y) = cx.current_element_offset();
    (bounds.x - x, bounds.y - y)
}

impl LiveLayout {
    fn engine<'e>(&'e self, parent: &'e LayoutEngine) -> &'e LayoutEngine {
        match self {
            LiveLayout::Subtree(index) => parent.subtree(*index),
            LiveLayout::Own(engine) => engine,
            LiveLayout::Done => unreachable!("cached subtree painted twice"),
        }
    }
}

impl Live {
    fn prepaint(&mut self, bounds: Bounds, parent: &LayoutEngine, cx: &mut ElementContext) {
        let (dx, dy) = layout_offset(bounds, cx);
        self.recording = self
            .entry
            .map(|(row, hash)| Recording::begin_prepaint(row, hash, bounds, cx));
        self.child
            .prepaint_with_offset(self.layout.engine(parent), cx, dx, dy);
        if let Some(recording) = &mut self.recording {
            recording.end_prepaint(cx);
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        parent: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let (dx, dy) = layout_offset(bounds, cx);
        if let Some(recording) = &mut self.recording {
            recording.begin_paint(scene, cx);
        }
        self.child
            .paint_with_offset(self.layout.engine(parent), scene, cx, dx, dy);
        if let Some(recording) = self.recording.take() {
            // A subtree rebuilt after layout keeps the measures recorded
            // with the entry: layout replayed them this frame.
            let memo = match &self.layout {
                LiveLayout::Subtree(index) => Some(parent.subtree_memo(*index)),
                LiveLayout::Own(_) | LiveLayout::Done => None,
            };
            recording.commit(scene, memo, cx);
        }
        if let LiveLayout::Own(engine) = std::mem::replace(&mut self.layout, LiveLayout::Done)
            && let Some(cache) = cx.cache.as_deref_mut()
        {
            cache.spare_engines.push(engine);
        }
    }
}

impl<F: FnOnce() -> AnyElement + 'static> Element for Cached<F> {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let frame = FrameInputs {
            scale: cx.scale_factor,
            font: cx.text.font_epoch(),
            accessibility: cx.accessibility_enabled(),
            focus: (cx.focus, cx.focus_visible),
        };
        let hash = self.frame_hash(cx);
        let claim = match cx.cache.as_deref_mut() {
            Some(cache) => cache.claim(self.key, hash, frame),
            None => Claim::Uncached,
        };
        let entry = match claim {
            // A subtree built in an earlier pass of this frame is laid out
            // again rather than replayed.
            Claim::Replay(row) if !matches!(self.state, State::Live(_)) => {
                let cache = cx.cache.as_deref().expect("replay claims need a cache");
                let id = engine.request_replay(
                    self.style.layout.clone(),
                    &cache.memo[row as usize],
                    row,
                );
                self.state = State::Replay { row };
                return (id, ());
            }
            Claim::Replay(row) | Claim::Build(row) => Some((row, hash)),
            Claim::Uncached => None,
        };
        let mut child = match std::mem::replace(&mut self.state, State::Idle) {
            State::Live(live) => live.child,
            State::Idle | State::Replay { .. } => self.build(),
        };
        let index = engine.begin_subtree();
        let content = child.request_layout(engine.subtree_mut(index), cx);
        let id = engine.finish_subtree(index, self.style.layout.clone(), content, self.fill_height);
        self.state = State::Live(Live {
            child,
            layout: LiveLayout::Subtree(index),
            entry,
            recording: None,
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        if let State::Replay { row } = self.state {
            if record::replay_prepaint(row, bounds, cx) {
                return;
            }
            self.state = State::Live(self.rebuild(row, bounds, cx));
        }
        if let State::Live(live) = &mut self.state {
            live.prepaint(bounds, engine, cx);
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        if let State::Replay { row } = self.state {
            if record::replay_paint(row, bounds, scene, cx) {
                return;
            }
            // Inherited paint state changed after prepaint: build now. The
            // replayed hits stay in the table unbound to any node, so they
            // route nothing; the rebuilt ones follow them.
            let mut live = self.rebuild(row, bounds, cx);
            live.prepaint(bounds, engine, cx);
            self.state = State::Live(live);
        }
        if let State::Live(live) = &mut self.state {
            live.paint(bounds, engine, scene, cx);
        }
    }
}

impl<F: FnOnce() -> AnyElement + 'static> IntoAnyElement for Cached<F> {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
