//! Enter and exit transitions for keyed divs ([`Div::presence`]).
//!
//! The window's [`PresenceTable`] keeps a row, in columns, for every keyed
//! div that asked for a [`Transition`]. A row is
//!
//! - [`PresencePhase::Entering`]: mounted after the table's first frame
//!   and playing its enter transition, or back from an exit and returning
//!   from wherever the exit had got to;
//! - [`PresencePhase::Present`]: in the tree and at rest;
//! - [`PresencePhase::Exiting`]: gone from the tree. The table keeps
//!   painting the div's last frame where it was, through the exit's
//!   opacity, offset, and scale, until the exit settles. It takes no input.
//!
//! A div with a row paints its subtree into the row's [`SceneChunk`] every
//! frame and draws the chunk through the row's layer
//! ([`Scene::chunk_with_layer`]), so the frame an exit keeps is exactly the
//! last one on screen and is never painted again. The layer's values are
//! the [`LAYER_PROPS`](crate::animation::LAYER_PROPS) rows of the div's key
//! in the window's animation table: a div that comes back mid-exit
//! retargets from where its layer is, velocity included.
//!
//! Exits paint after the frame's root, above it at the z-index and inside
//! the clip and transform the div last painted under. Nothing animates on
//! the table's first frame, so what a window opens with is simply there,
//! and under reduced motion every transition is a short fade.

use std::collections::HashMap;

use quark::scene::{LayerTransform, SceneChunk};

use super::*;
use crate::animation::{AnimKey, AnimPhase, AnimationTable, LAYER_PROPS};

/// How a keyed div enters and leaves; see [`Div::presence`]. Build one
/// from [`Self::fade`], [`Self::slide`], and [`Self::scale`] and combine
/// them with [`Self::and`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transition {
    /// The layer the div enters from.
    enter: LayerTransform,
    /// The layer the div leaves to.
    exit: LayerTransform,
    motion: Motion,
}

impl Transition {
    fn symmetric(layer: LayerTransform) -> Self {
        Self {
            enter: layer,
            exit: layer,
            motion: Motion::smooth(),
        }
    }

    pub fn fade() -> Self {
        Self::symmetric(LayerTransform {
            opacity: 0.0,
            ..LayerTransform::IDENTITY
        })
    }

    /// Enter from, and leave toward, `(dx, dy)` points away. Combine with
    /// [`Self::fade`] unless the div should stay opaque until its exit
    /// settles.
    pub fn slide(dx: f32, dy: f32) -> Self {
        Self::symmetric(LayerTransform {
            offset_x: dx,
            offset_y: dy,
            ..LayerTransform::IDENTITY
        })
    }

    /// Grow from, and shrink to, `scale` about the div's center.
    pub fn scale(scale: f32) -> Self {
        Self::symmetric(LayerTransform {
            scale_x: scale,
            scale_y: scale,
            ..LayerTransform::IDENTITY
        })
    }

    /// Both at once: opacities and scales multiply and offsets add. The
    /// motion stays `self`'s.
    pub fn and(self, other: Self) -> Self {
        Self {
            enter: combine(self.enter, other.enter),
            exit: combine(self.exit, other.exit),
            motion: self.motion,
        }
    }

    /// Enter as `enter` does and leave as `exit` does, with `enter`'s
    /// motion.
    pub fn asymmetric(enter: Self, exit: Self) -> Self {
        Self {
            enter: enter.enter,
            exit: exit.exit,
            motion: enter.motion,
        }
    }

    /// Move with `motion` instead of [`Motion::smooth`].
    pub fn with_motion(mut self, motion: Motion) -> Self {
        self.motion = motion;
        self
    }

    /// The fade that stands in under reduced motion: no travel or growth.
    fn reduced(self) -> Self {
        let fade = |layer: LayerTransform| {
            if layer.is_identity() {
                layer
            } else {
                LayerTransform {
                    opacity: 0.0,
                    ..LayerTransform::IDENTITY
                }
            }
        };
        Self {
            enter: fade(self.enter),
            exit: fade(self.exit),
            motion: Motion::reduced_fade(),
        }
    }
}

fn combine(a: LayerTransform, b: LayerTransform) -> LayerTransform {
    LayerTransform {
        opacity: a.opacity * b.opacity,
        offset_x: a.offset_x + b.offset_x,
        offset_y: a.offset_y + b.offset_y,
        scale_x: a.scale_x * b.scale_x,
        scale_y: a.scale_y * b.scale_y,
        origin: [0.0; 2],
    }
}

/// Where a [`PresenceTable`] row is in its life; the `phases` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresencePhase {
    Entering,
    Present,
    Exiting,
}

/// What a div painted under, so its exit paints there too.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Place {
    /// The div's bounds in the coordinates it painted in.
    rect: Rect,
    /// Those coordinates to the window's.
    transform: Transform2D,
    /// The clips around it, in window coordinates.
    clip: Rect,
    z: i32,
}

/// A row's last frame, and a second chunk to record into while a scene
/// still holds the first.
#[derive(Debug, Default)]
struct Frames {
    current: Option<Arc<SceneChunk>>,
    spare: Option<Arc<SceneChunk>>,
}

impl Frames {
    /// The chunk to record into: one no scene holds.
    fn writable(&mut self) -> &mut SceneChunk {
        let unique = |chunk: &mut Arc<SceneChunk>| Arc::get_mut(chunk).is_some();
        if !self.current.as_mut().is_some_and(unique) {
            let fresh = self
                .spare
                .take()
                .and_then(|mut spare| unique(&mut spare).then_some(spare))
                .unwrap_or_default();
            self.spare = self.current.replace(fresh);
        }
        let chunk = self.current.as_mut().expect("recorded chunk");
        Arc::get_mut(chunk).expect("unshared chunk")
    }
}

/// The window's presence rows, one per keyed div with a [`Transition`],
/// stored column-wise. Attach it with [`ElementContext::with_presence`]
/// next to the animation table; see the [module docs](self).
#[derive(Debug, Default)]
pub struct PresenceTable {
    keys: Vec<AnimKey>,
    phases: Vec<PresencePhase>,
    transitions: Vec<Transition>,
    places: Vec<Place>,
    frames: Vec<Frames>,
    /// The frame that last painted the row's div (or replayed it).
    seen: Vec<u64>,
    index: HashMap<AnimKey, u32>,
    /// Frames begun so far.
    frame: u64,
    /// Whether a frame has finished; before that, nothing enters.
    started: bool,
    /// The last frame whose exits were painted; each paints once a frame.
    exits_painted: u64,
    /// `(key, rect)` of the rows painted this frame, in paint order, for
    /// the cache boundaries recording them.
    painted: Vec<(AnimKey, Rect)>,
}

/// A row claimed by a div this frame, from its prepaint to its paint.
#[derive(Debug, Clone, Copy)]
pub(super) struct PresenceSlot {
    row: u32,
    layer: LayerTransform,
}

impl PresenceSlot {
    /// The map from the div's content to where its layer draws it, when
    /// the layer moves or scales it.
    pub(super) fn transform(&self, bounds: Bounds) -> Option<Transform2D> {
        layer_matrix(self.layer, bounds)
    }
}

/// The map from content at `bounds` to where `layer` draws it, when the
/// layer moves or scales it; what hit testing and geometry go through.
pub(super) fn layer_matrix(layer: LayerTransform, bounds: Bounds) -> Option<Transform2D> {
    let moves = layer.offset_x != 0.0 || layer.offset_y != 0.0;
    let scales = layer.scale_x != 1.0 || layer.scale_y != 1.0;
    (moves || scales).then(|| layer.transform_at([bounds.x, bounds.y]))
}

/// Where a div's paint output starts in the scene.
pub(super) struct PresenceMark {
    slot: PresenceSlot,
    scene: usize,
    bounds: Bounds,
}

impl PresenceTable {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.keys.len()
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    pub fn phase(&self, key: AnimKey) -> Option<PresencePhase> {
        self.row(key).map(|row| self.phases[row])
    }

    fn row(&self, key: AnimKey) -> Option<usize> {
        self.index.get(&key).map(|row| *row as usize)
    }

    pub(super) fn begin_frame(&mut self) {
        self.frame += 1;
        self.painted.clear();
    }

    pub(super) fn painted_len(&self) -> usize {
        self.painted.len()
    }

    pub(super) fn painted_since(&self, start: usize) -> &[(AnimKey, Rect)] {
        &self.painted[start..]
    }

    fn push_row(&mut self, key: AnimKey, phase: PresencePhase, transition: Transition) -> usize {
        let row = self.keys.len();
        self.keys.push(key);
        self.phases.push(phase);
        self.transitions.push(transition);
        self.places.push(Place {
            rect: Rect::default(),
            transform: Transform2D::IDENTITY,
            clip: quark::hit::UNCLIPPED,
            z: 0,
        });
        self.frames.push(Frames::default());
        self.seen.push(self.frame);
        self.index.insert(key, row as u32);
        row
    }

    fn swap_remove_row(&mut self, row: usize) {
        self.index.remove(&self.keys[row]);
        self.keys.swap_remove(row);
        self.phases.swap_remove(row);
        self.transitions.swap_remove(row);
        self.places.swap_remove(row);
        self.frames.swap_remove(row);
        self.seen.swap_remove(row);
        if let Some(moved) = self.keys.get(row) {
            self.index.insert(*moved, row as u32);
        }
    }

    /// Claim `key`'s row for a div painting this frame: mount it, or bring
    /// it back from an exit. Claiming it again in the same frame (a div
    /// prepainted twice, or two divs under one key) changes nothing more.
    fn claim(
        &mut self,
        key: AnimKey,
        transition: Transition,
        animations: &mut AnimationTable,
        now_ms: u64,
    ) -> usize {
        let Some(row) = self.row(key) else {
            if !self.started {
                return self.push_row(key, PresencePhase::Present, transition);
            }
            animations.set_layer(key, transition.enter, now_ms);
            animations.animate_layer(key, LayerTransform::IDENTITY, transition.motion, now_ms);
            return self.push_row(key, PresencePhase::Entering, transition);
        };
        let claimed = self.seen[row] == self.frame && self.phases[row] != PresencePhase::Exiting;
        if !claimed {
            self.seen[row] = self.frame;
            self.transitions[row] = transition;
            if self.phases[row] == PresencePhase::Exiting {
                animations.animate_layer(key, LayerTransform::IDENTITY, transition.motion, now_ms);
                self.phases[row] = PresencePhase::Entering;
            }
        }
        row
    }

    /// Start the exits of the rows no div painted this frame.
    fn start_exits(&mut self, animations: &mut AnimationTable, now_ms: u64) {
        for row in 0..self.keys.len() {
            if self.seen[row] == self.frame || self.phases[row] == PresencePhase::Exiting {
                continue;
            }
            let key = self.keys[row];
            if animations.layer(key).is_none() {
                animations.set_layer(key, LayerTransform::IDENTITY, now_ms);
            }
            let transition = self.transitions[row];
            animations.animate_layer(key, transition.exit, transition.motion, now_ms);
            self.phases[row] = PresencePhase::Exiting;
        }
    }

    /// Start exits, then paint every exit once a frame.
    fn end_render(&mut self, scene: &mut Scene, animations: &mut AnimationTable, now_ms: u64) {
        self.start_exits(animations, now_ms);
        if self.exits_painted == self.frame {
            return;
        }
        self.exits_painted = self.frame;
        for row in 0..self.keys.len() {
            if self.phases[row] != PresencePhase::Exiting {
                continue;
            }
            let Some(chunk) = self.frames[row].current.as_ref().filter(|c| !c.is_empty()) else {
                continue;
            };
            let place = self.places[row];
            let layer = layer_at(animations, self.keys[row], place.rect);
            if place.z != 0 {
                scene.push_z_index(place.z);
            }
            let clipped = place.clip != quark::hit::UNCLIPPED;
            if clipped {
                scene.clip(place.clip);
            }
            let transformed = !place.transform.is_identity();
            if transformed {
                scene.push_layer(1.0, place.transform);
            }
            scene.chunk_with_layer(chunk, [place.rect.x, place.rect.y], layer);
            if transformed {
                scene.pop_layer();
            }
            if clipped {
                scene.pop_clip();
            }
            if place.z != 0 {
                scene.pop_z_index();
            }
        }
    }

    /// Settle the rows whose layer came to rest: an entered row is present
    /// and an exited one goes, each with its layer rows. An exit that faded
    /// out shows nothing more and goes while the rest of it settles. Once
    /// one frame is done, later mounts enter.
    fn finish_frame(&mut self, animations: &mut AnimationTable, now_ms: u64) {
        self.start_exits(animations, now_ms);
        let mut row = 0;
        while row < self.keys.len() {
            let key = self.keys[row];
            let phase = self.phases[row];
            let [opacity, ..] = LAYER_PROPS;
            let faded_out = phase == PresencePhase::Exiting
                && animations.phase(key, opacity) == Some(AnimPhase::Settled)
                && animations.get(key, opacity) == Some(0.0);
            let rested = faded_out || !animations.any_animating(key, &LAYER_PROPS);
            if phase == PresencePhase::Present || !rested {
                row += 1;
                continue;
            }
            for prop in LAYER_PROPS {
                animations.remove(key, prop);
            }
            if self.phases[row] == PresencePhase::Exiting {
                self.swap_remove_row(row);
            } else {
                self.phases[row] = PresencePhase::Present;
                row += 1;
            }
        }
        self.started = true;
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Every column has one entry per row, the key map points at them, and
    /// a row no div painted this frame is exiting. Called through
    /// `debug_assert!` at the end of every frame.
    pub fn verify_integrity(&self) -> Result<(), PresenceIntegrityError> {
        let rows = self.keys.len();
        let columns = [
            ("phases", self.phases.len()),
            ("transitions", self.transitions.len()),
            ("places", self.places.len()),
            ("frames", self.frames.len()),
            ("seen", self.seen.len()),
        ];
        for (column, len) in columns {
            if len != rows {
                return Err(PresenceIntegrityError::ColumnLength { column, len, rows });
            }
        }
        if self.index.len() != rows {
            return Err(PresenceIntegrityError::KeyMap);
        }
        for (row, key) in self.keys.iter().enumerate() {
            if self.index.get(key) != Some(&(row as u32)) {
                return Err(PresenceIntegrityError::KeyMap);
            }
            if self.seen[row] != self.frame && self.phases[row] != PresencePhase::Exiting {
                return Err(PresenceIntegrityError::UnseenNotExiting { row });
            }
        }
        Ok(())
    }
}

/// A broken [`PresenceTable`] invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PresenceIntegrityError {
    ColumnLength {
        column: &'static str,
        len: usize,
        rows: usize,
    },
    /// The key map and the key column disagree.
    KeyMap,
    /// A row no div painted this frame that is not exiting.
    UnseenNotExiting { row: usize },
}

/// `key`'s layer, scaled about the center of `rect` (a chunk placed at its
/// origin).
fn layer_at(animations: &AnimationTable, key: AnimKey, rect: Rect) -> LayerTransform {
    LayerTransform {
        origin: [rect.width * 0.5, rect.height * 0.5],
        ..animations.layer(key).unwrap_or(LayerTransform::IDENTITY)
    }
}

impl ElementContext<'_> {
    /// Claim the row of the div keyed `key` for this frame: what its layer
    /// draws it through. `None` without a presence table and an animation
    /// table.
    pub(super) fn presence_prepaint(
        &mut self,
        key: &UiKey,
        transition: Transition,
        bounds: Bounds,
    ) -> Option<PresenceSlot> {
        let (Some(table), Some(animations)) =
            (self.presence.as_deref_mut(), self.animations.as_deref_mut())
        else {
            return None;
        };
        let key = AnimKey::from(key);
        let transition = if self.theme.reduced_motion {
            transition.reduced()
        } else {
            transition
        };
        let row = table.claim(key, transition, animations, self.clock_ms);
        if animations.any_animating(key, &LAYER_PROPS) {
            // A layer in motion is no output a cache boundary may keep.
            self.mark_volatile();
        }
        let animations = self.animations.as_deref().expect("checked above");
        Some(PresenceSlot {
            row: row as u32,
            layer: layer_at(animations, key, bounds),
        })
    }

    /// `key`'s animated layer for content at `bounds`, scaled about its
    /// center; `None` when it has none or it changes nothing. A layer in
    /// motion makes the output volatile for the cache boundaries around.
    pub(super) fn animated_layer(
        &mut self,
        key: AnimKey,
        bounds: Bounds,
    ) -> Option<LayerTransform> {
        let animations = self.animations.as_deref()?;
        let layer = layer_at(animations, key, bounds);
        if animations.any_animating(key, &LAYER_PROPS) {
            self.mark_volatile();
        }
        (!layer.is_identity()).then_some(layer)
    }

    /// Start painting a claimed div: what it paints from here goes into its
    /// row's chunk at [`Self::end_presence`].
    pub(super) fn begin_presence(
        &mut self,
        slot: PresenceSlot,
        bounds: Bounds,
        scene: &Scene,
    ) -> PresenceMark {
        PresenceMark {
            slot,
            scene: scene.len(),
            bounds,
        }
    }

    /// Move what the div painted into its row's chunk and draw the chunk
    /// through the row's layer in its place.
    pub(super) fn end_presence(&mut self, mark: PresenceMark, scene: &mut Scene) {
        let place = Place {
            rect: mark.bounds,
            transform: self.current_transform(),
            clip: self.current_window_clip(),
            z: self.current_z_index(),
        };
        let Some(table) = self.presence.as_deref_mut() else {
            return;
        };
        let row = mark.slot.row as usize;
        let (ox, oy) = (place.rect.x, place.rect.y);
        table.places[row] = place;
        table.painted.push((table.keys[row], place.rect));
        table.frames[row]
            .writable()
            .replace(scene.primitives.drain(mark.scene..).map(|mut p| {
                p.offset(-ox, -oy);
                p
            }));
        let chunk = table.frames[row].current.as_ref().expect("just recorded");
        scene.chunk_with_layer(chunk, [ox, oy], mark.slot.layer);
    }

    /// Keep the rows a replayed cache boundary painted, at its origin now.
    pub(super) fn replay_presence(&mut self, rows: &[(AnimKey, Rect)], (ox, oy): (f32, f32)) {
        let Some(table) = self.presence.as_deref_mut() else {
            return;
        };
        for &(key, rect) in rows {
            let Some(row) = table.row(key) else {
                continue;
            };
            let rect = rect.offset(ox, oy);
            table.seen[row] = table.frame;
            table.places[row].rect = rect;
            table.painted.push((key, rect));
        }
    }

    /// How many rows were painted so far this frame.
    pub(super) fn presence_painted_len(&self) -> usize {
        self.presence
            .as_deref()
            .map_or(0, PresenceTable::painted_len)
    }

    /// The rows painted since `start`, for a recording cache boundary.
    pub(super) fn presence_painted_since(&self, start: usize) -> &[(AnimKey, Rect)] {
        self.presence
            .as_deref()
            .map_or(&[], |table| table.painted_since(start))
    }

    /// Start exits and paint them over `scene`; the end of every render.
    pub(super) fn paint_exits(&mut self, scene: &mut Scene) {
        if let (Some(table), Some(animations)) =
            (self.presence.as_deref_mut(), self.animations.as_deref_mut())
        {
            table.end_render(scene, animations, self.clock_ms);
        }
    }

    /// Drop settled exits; the end of the frame.
    pub(super) fn finish_presence(&mut self) {
        if let (Some(table), Some(animations)) =
            (self.presence.as_deref_mut(), self.animations.as_deref_mut())
        {
            table.finish_frame(animations, self.clock_ms);
        }
    }
}

#[cfg(test)]
mod tests;
