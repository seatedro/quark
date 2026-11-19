use super::*;
use taffy::{AvailableSpace, TraversePartialTree};

mod measure;
mod store;
#[cfg(test)]
mod tests;

pub use measure::MeasureContext;
pub(super) use measure::{MeasureMemo, TextMeasure, ceil_to_pixels, round_to_pixels, text_height};
use measure::{NodeMeasure, find_memo};
pub use store::LayoutIntegrityError;
use store::{Claim, KeyedRows, LayoutKey, NodeClaim, NodeStore};

/// Layout passes a frame may run: replayed boundaries that went stale are
/// rebuilt in the next pass, and the last pass replays nothing.
const LAYOUT_PASSES: usize = 3;

/// Evicted subtrees kept, with their engines, for new boundaries.
const SPARE_SUBTREES: usize = 64;

/// Where a frame's layout passes stand. A replayed cache boundary asked a
/// measure query its memo has no answer for (its parent offers a new size)
/// goes stale: its entry is dropped and the pass runs again, rebuilding it.
/// The last pass replays nothing, so a frame always ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LayoutPassState {
    /// The frame's first pass.
    Initial,
    /// Pass `pass`, which rebuilds the boundaries the passes before it
    /// found stale.
    RetryStaleBoundaries { pass: usize },
    /// The last pass, in which every boundary builds.
    FallbackFullBuild,
    /// The frame is laid out.
    Done,
}

impl LayoutPassState {
    /// Whether cache boundaries may replay in this pass.
    pub(super) fn replays(self) -> bool {
        matches!(self, Self::Initial | Self::RetryStaleBoundaries { .. })
    }

    /// The state after this pass, which left boundaries stale or not.
    pub(super) fn next(self, stale: bool) -> Self {
        debug_assert!(
            !stale || self.replays(),
            "a pass that replays nothing went stale"
        );
        let pass = match self {
            _ if !stale => return Self::Done,
            Self::Initial => 1,
            Self::RetryStaleBoundaries { pass } => pass + 1,
            Self::FallbackFullBuild | Self::Done => return Self::Done,
        };
        if pass + 1 < LAYOUT_PASSES {
            Self::RetryStaleBoundaries { pass }
        } else {
            Self::FallbackFullBuild
        }
    }
}

/// Content of one cache boundary, laid out as its own root so the parent
/// sees a single measured leaf.
struct Subtree {
    engine: LayoutEngine,
    /// The boundary's key, which keys its leaf in the parent.
    key: CacheKey,
    /// The block wrapper around the content; the subtree's root.
    root: Option<LayoutId>,
    /// The boundary's leaf in the parent engine.
    host: Option<LayoutId>,
    /// Queries answered this frame, in order; recorded with the entry.
    memo: Vec<MeasureMemo>,
    /// The size the content was last laid out at.
    laid_out_at: Option<taffy::Size<Option<f32>>>,
    /// The root takes the boundary's height when placed; see
    /// [`boundary_root_style`].
    fill_height: bool,
}

impl Subtree {
    fn new() -> Self {
        Self {
            engine: LayoutEngine::new(),
            key: CacheKey(0),
            root: None,
            host: None,
            memo: Vec::new(),
            laid_out_at: None,
            fill_height: false,
        }
    }

    /// Start a pass laying out the boundary keyed `key`.
    fn begin(&mut self, key: CacheKey) {
        self.engine.clear();
        self.key = key;
        self.root = None;
        self.host = None;
        self.memo.clear();
        self.laid_out_at = None;
        self.fill_height = false;
    }

    /// Lay the content out for one parent query. Known dimensions become
    /// definite available space, so the block wrapper takes the known
    /// width and the content's own height.
    fn layout(
        &mut self,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
        cx: &mut MeasureContext,
    ) -> taffy::Size<f32> {
        let Some(root) = self.root else {
            return taffy::Size::ZERO;
        };
        if self.laid_out_at != Some(known) || known.width.is_none() || known.height.is_none() {
            // A root that fills its height sizes at its content's height:
            // its percentage height resolves against definite space only.
            let open_height = if self.fill_height {
                AvailableSpace::MaxContent
            } else {
                available.height
            };
            let available = taffy::Size {
                width: known
                    .width
                    .map_or(available.width, AvailableSpace::Definite),
                height: known.height.map_or(open_height, AvailableSpace::Definite),
            };
            self.engine.compute(root, available, cx);
            self.laid_out_at = Some(known);
        }
        let size = self.engine.tree.unrounded_layout(root).size;
        taffy::Size {
            width: known.width.unwrap_or(size.width),
            height: known.height.unwrap_or(size.height),
        }
    }

    fn measure(
        &mut self,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
        cx: &mut MeasureContext,
    ) -> taffy::Size<f32> {
        if let Some(hit) = find_memo(&self.memo, known, available) {
            return hit;
        }
        let size = self.layout(known, available, cx);
        // Both dimensions known is the final placement, answered without a
        // memo on replay; only sizing queries need recording.
        if known.width.is_none() || known.height.is_none() {
            self.memo.push(MeasureMemo {
                known,
                available,
                size,
            });
        }
        size
    }
}

/// Taffy node ids are slotmap keys: the low 32 bits are the slot index,
/// dense from zero, so they index a plain vector.
fn slot(id: LayoutId) -> usize {
    (u64::from(id) & u64::from(u32::MAX)) as usize
}

// ---------------------------------------------------------------------------
// LayoutEngine — wraps TaffyTree
// ---------------------------------------------------------------------------

/// Layout of a window, retained across frames. Hosts keep one per window
/// and [`clear`](Self::clear) it before each frame's layout pass.
///
/// Nodes are retained by key. A node's key derives from its parent's and
/// from its own key among its siblings (a div's [`Div::key`], a cache
/// boundary's [`CacheKey`]) or, unkeyed, its place among its unkeyed
/// siblings. A pass takes each key's node from the passes before,
/// restyled or re-parented only where it differs. Taffy keeps each node's
/// cached layout until a change dirties it, so an unchanged frame computes
/// nothing and allocates nothing, a changed one recomputes only the
/// changed nodes' ancestors, and inserting or removing a keyed sibling
/// leaves the other siblings' layouts cached. Nodes no pass has claimed for
/// a while are evicted, and new keys reuse their storage.
pub struct LayoutEngine {
    pub(super) tree: taffy::TaffyTree<NodeMeasure>,
    nodes: NodeStore,
    /// Containers whose children are being requested, innermost last, on
    /// the engine's root.
    scopes: Vec<Scope>,
    /// Absolute origin of every node reachable from the computed root, by
    /// slot; NaN for nodes the last pass did not reach.
    origins: Vec<(f32, f32)>,
    /// Depth-first stack for filling `origins`.
    walk: Vec<(LayoutId, f32, f32)>,
    /// Children ids of the containers being built, as a stack, so a
    /// container passes its children to taffy without a vector of its own.
    child_ids: Vec<LayoutId>,
    /// Cache-boundary subtrees, retained by their boundaries' keys.
    subtree_rows: KeyedRows<CacheKey>,
    subtrees: Vec<Subtree>,
    /// Subtrees of evicted rows, which new boundaries take.
    spare_subtrees: Vec<Subtree>,
    /// Rows of the subtrees this pass claimed.
    live_subtrees: Vec<u32>,
    /// Measure memos of replayed boundaries this pass, and the last pass's.
    replay_memo: Vec<MeasureMemo>,
    last_replay_memo: Vec<MeasureMemo>,
    /// Cache slots whose replayed memo missed a query this frame.
    stale: Vec<u32>,
    /// Device pixels per point of the last pass.
    scale: f32,
}

/// A container whose children are being requested.
struct Scope {
    /// The container's key, which its children's keys derive from.
    key: LayoutKey,
    /// Unkeyed children requested so far.
    unkeyed: u32,
    /// Where its children start on the child stack.
    mark: usize,
}

impl Scope {
    const ROOT: Self = Self {
        key: LayoutKey::ROOT,
        unkeyed: 0,
        mark: 0,
    };
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutEngine {
    pub fn new() -> Self {
        // `layout_bounds` snaps to device pixels itself.
        let mut tree = taffy::TaffyTree::new();
        tree.disable_rounding();
        Self {
            tree,
            nodes: NodeStore::new(),
            scopes: vec![Scope::ROOT],
            origins: Vec::new(),
            walk: Vec::new(),
            child_ids: Vec::new(),
            subtree_rows: KeyedRows::new(),
            subtrees: Vec::new(),
            spare_subtrees: Vec::new(),
            live_subtrees: Vec::new(),
            replay_memo: Vec::new(),
            last_replay_memo: Vec::new(),
            stale: Vec::new(),
            scale: 1.0,
        }
    }

    /// Create a layout node with the given style and children.
    pub fn request_layout(&mut self, style: taffy::Style, children: &[LayoutId]) -> LayoutId {
        let key = self.next_key(None);
        self.node(key, &style, Children::Slice(children), Context::None)
    }

    /// The key of the next node requested in the innermost open container:
    /// keyed `key` among its siblings, or else by its unkeyed place.
    fn next_key(&mut self, key: Option<u64>) -> LayoutKey {
        let scope = self.scopes.last_mut().expect("the root scope stays open");
        match key {
            Some(key) => scope.key.keyed(key),
            None => {
                scope.unkeyed += 1;
                scope.key.nth(scope.unkeyed - 1)
            }
        }
    }

    /// The node for `key` this pass, made to match `style`, `children`, and
    /// `context`. Only what differs from the node's last pass is set, since
    /// every set dirties the node's cached layout.
    fn node(
        &mut self,
        key: LayoutKey,
        style: &taffy::Style,
        children: Children,
        context: Context,
    ) -> LayoutId {
        let children = match children {
            Children::Slice(ids) => ids,
            Children::Stack(mark) => &self.child_ids[mark..],
        };
        let tree = &mut self.tree;
        let (node, continuous) = match self.nodes.claim(key) {
            NodeClaim::Held { node, continuous } => (node, continuous),
            NodeClaim::New(key) => {
                // `set_children` detaches the children from the parents
                // they had, so no other node still lists them.
                let node = tree.new_leaf(style.clone()).expect("taffy new_leaf failed");
                if !children.is_empty() {
                    tree.set_children(node, children).expect("valid node");
                }
                if let Some(context) = context.into_measure() {
                    tree.set_node_context(node, Some(context))
                        .expect("valid node");
                }
                self.nodes.insert(key, node);
                return node;
            }
        };
        if tree.style(node).expect("valid node") != style {
            tree.set_style(node, style.clone()).expect("valid node");
        }
        let same_children = tree.child_count(node) == children.len()
            && children
                .iter()
                .enumerate()
                .all(|(i, child)| tree.child_at_index(node, i).ok() == Some(*child));
        if !same_children {
            tree.set_children(node, children).expect("valid node");
        }
        match (context, tree.get_node_context_mut(node)) {
            (Context::None, None) => {}
            // A replay answering from the memo it answered from last pass
            // answers as it did; only the memo's place in the buffer moved.
            (
                Context::Replay { memo, slot },
                Some(NodeMeasure::Replay {
                    memo: old,
                    slot: old_slot,
                }),
            ) if continuous
                && self.last_replay_memo.get(old.clone()) == self.replay_memo.get(memo.clone()) =>
            {
                *old = memo;
                *old_slot = slot;
            }
            // The same shaped text measures the same; keeping the node
            // context keeps taffy's cached sizes for it.
            (Context::Measure(NodeMeasure::Text(text)), Some(NodeMeasure::Text(old)))
                if text.measures_as(old) => {}
            (context, _) => {
                tree.set_node_context(node, context.into_measure())
                    .expect("valid node");
            }
        }
        node
    }

    /// Start a container keyed `key` among its siblings, or unkeyed: request
    /// each child and push it with [`Self::push_child`], then build the
    /// container with [`Self::finish_children`].
    pub(super) fn begin_children(&mut self, key: Option<u64>) {
        let key = self.next_key(key);
        self.scopes.push(Scope {
            key,
            unkeyed: 0,
            mark: self.child_ids.len(),
        });
    }

    pub(super) fn push_child(&mut self, id: LayoutId) {
        self.child_ids.push(id);
    }

    /// Create the innermost container begun, from the children pushed since.
    pub(super) fn finish_children(&mut self, style: &taffy::Style) -> LayoutId {
        debug_assert!(self.scopes.len() > 1, "no container begun");
        let scope = self.scopes.pop().expect("no container begun");
        let id = self.node(scope.key, style, Children::Stack(scope.mark), Context::None);
        self.child_ids.truncate(scope.mark);
        id
    }

    /// Create a leaf node that uses a measure function for intrinsic sizing.
    pub fn request_measured_layout(
        &mut self,
        style: taffy::Style,
        measure: impl Fn(
            taffy::Size<Option<f32>>,
            taffy::Size<taffy::AvailableSpace>,
        ) -> taffy::Size<f32>
        + Send
        + Sync
        + 'static,
    ) -> LayoutId {
        let key = self.next_key(None);
        let measure = NodeMeasure::Measure(Box::new(measure));
        self.node(key, &style, Children::Slice(&[]), Context::Measure(measure))
    }

    /// Create a leaf for text that wraps to the width layout gives it; see
    /// [`TextMeasure`].
    pub(super) fn request_text_layout(
        &mut self,
        style: &taffy::Style,
        text: TextMeasure,
    ) -> LayoutId {
        let key = self.next_key(None);
        let measure = NodeMeasure::Text(text);
        self.node(key, style, Children::Slice(&[]), Context::Measure(measure))
    }

    /// Claim the subtree of the cache boundary keyed `key` for its content:
    /// the one the boundary laid out in last time, so its nodes keep their
    /// layouts. Lay the content out in [`Self::subtree_mut`], then close it
    /// with [`Self::finish_subtree`].
    pub(super) fn begin_subtree(&mut self, key: CacheKey) -> usize {
        let row = match self.subtree_rows.claim(key) {
            Claim::Row { row, .. } => row,
            Claim::Vacant(probed) => {
                let sub = self.spare_subtrees.pop().unwrap_or_else(Subtree::new);
                self.subtrees.push(sub);
                self.subtree_rows.push(probed)
            }
        };
        self.subtrees[row].begin(key);
        self.live_subtrees.push(row as u32);
        row
    }

    pub(super) fn subtree_mut(&mut self, index: usize) -> &mut LayoutEngine {
        &mut self.subtrees[index].engine
    }

    pub(super) fn subtree(&self, index: usize) -> &LayoutEngine {
        &self.subtrees[index].engine
    }

    /// Wrap `content` in the subtree's block root and return the leaf the
    /// parent lays out in its place.
    pub(super) fn finish_subtree(
        &mut self,
        index: usize,
        style: taffy::Style,
        content: LayoutId,
        fill_height: bool,
    ) -> LayoutId {
        let sub = &mut self.subtrees[index];
        sub.fill_height = fill_height;
        let root_style = boundary_root_style(fill_height);
        sub.root = Some(sub.engine.request_layout(root_style, &[content]));
        let boundary = sub.key;
        let key = self.next_key(Some(boundary.0));
        // Always set, so the host is dirtied: its content may have changed.
        let host = self.node(key, &style, Children::Slice(&[]), Context::Subtree(index));
        self.subtrees[index].host = Some(host);
        host
    }

    /// Queries the subtree answered this frame, for recording.
    pub(super) fn subtree_memo(&self, index: usize) -> &[MeasureMemo] {
        &self.subtrees[index].memo
    }

    /// A leaf for the boundary keyed `key`, replayed from cache `slot`,
    /// that answers from `memo`.
    pub(super) fn request_replay(
        &mut self,
        style: taffy::Style,
        memo: &[MeasureMemo],
        slot: u32,
        key: CacheKey,
    ) -> LayoutId {
        let start = self.replay_memo.len();
        self.replay_memo.extend_from_slice(memo);
        let memo = start..self.replay_memo.len();
        let key = self.next_key(Some(key.0));
        self.node(
            key,
            &style,
            Children::Slice(&[]),
            Context::Replay { memo, slot },
        )
    }

    /// Lay out a detached boundary root (a rebuilt cached subtree) at its
    /// final size.
    pub(super) fn layout_boundary(
        &mut self,
        content: LayoutId,
        width: f32,
        height: f32,
        fill_height: bool,
        cx: &mut MeasureContext,
    ) {
        let root = self.request_layout(boundary_root_style(fill_height), &[content]);
        self.compute_layout(root, width, height, cx);
    }

    /// Move the cache slots whose replayed measures went stale into `out`,
    /// from this engine and every live subtree.
    pub(super) fn drain_stale(&mut self, out: &mut Vec<u32>) {
        out.append(&mut self.stale);
        for &row in &self.live_subtrees {
            self.subtrees[row as usize].engine.drain_stale(out);
        }
    }

    /// Compute layout for the entire tree rooted at `root`, shaping wrapped
    /// text with `cx`.
    pub fn compute_layout(
        &mut self,
        root: LayoutId,
        width: f32,
        height: f32,
        cx: &mut MeasureContext,
    ) {
        self.compute(
            root,
            taffy::Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(height),
            },
            cx,
        );
        self.finish(root, cx);
    }

    fn compute(
        &mut self,
        root: LayoutId,
        available: taffy::Size<AvailableSpace>,
        cx: &mut MeasureContext,
    ) {
        let Self {
            tree,
            subtrees,
            replay_memo,
            stale,
            ..
        } = self;
        tree.compute_layout_with_measure(
            root,
            available,
            |known, available, _node_id, context, _style| match context {
                Some(NodeMeasure::Measure(f)) => f(known, available),
                Some(NodeMeasure::Text(text)) => text.measure(known, available, cx),
                Some(NodeMeasure::Subtree(index)) => subtrees[*index].measure(known, available, cx),
                Some(NodeMeasure::Replay { memo, slot }) => {
                    find_memo(&replay_memo[memo.clone()], known, available).unwrap_or_else(|| {
                        stale.push(*slot);
                        taffy::Size::ZERO
                    })
                }
                None => taffy::Size::ZERO,
            },
        )
        .expect("taffy compute_layout failed");
    }

    /// After the pass: lay every subtree out at its host's final size, and
    /// record absolute origins so [`Self::layout_bounds`] is a lookup.
    fn finish(&mut self, root: LayoutId, cx: &mut MeasureContext) {
        self.scale = cx.scale;
        for &row in &self.live_subtrees {
            let sub = &mut self.subtrees[row as usize];
            let (Some(host), Some(sub_root)) = (sub.host, sub.root) else {
                continue;
            };
            let size = self.tree.layout(host).expect("invalid layout id").size;
            sub.layout(
                taffy::Size {
                    width: Some(size.width),
                    height: Some(size.height),
                },
                taffy::Size {
                    width: AvailableSpace::Definite(size.width),
                    height: AvailableSpace::Definite(size.height),
                },
                cx,
            );
            sub.engine.finish(sub_root, cx);
        }

        self.origins.fill((f32::NAN, f32::NAN));
        self.walk.push((root, 0.0, 0.0));
        while let Some((node, parent_x, parent_y)) = self.walk.pop() {
            let location = self.tree.layout(node).expect("invalid layout id").location;
            let origin = (parent_x + location.x, parent_y + location.y);
            let index = slot(node);
            if index >= self.origins.len() {
                self.origins.resize(index + 1, (f32::NAN, f32::NAN));
            }
            self.origins[index] = origin;
            for child in self.tree.child_ids(node) {
                self.walk.push((child, origin.0, origin.1));
            }
        }
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// Get the resolved bounds for a layout node, in absolute coordinates.
    /// Edges snap to device pixels, so boxes that touch share an edge.
    pub fn layout_bounds(&self, id: LayoutId) -> Bounds {
        let size = self.tree.layout(id).expect("invalid layout id").size;
        let (x, y) = self.origin(id);
        let left = round_to_pixels(x, self.scale);
        let top = round_to_pixels(y, self.scale);
        Bounds {
            x: left,
            y: top,
            width: round_to_pixels(x + size.width, self.scale) - left,
            height: round_to_pixels(y + size.height, self.scale) - top,
        }
    }

    /// `length` from absolute `start`, snapped as [`Self::layout_bounds`]
    /// snaps a box's edges.
    fn snapped_span(&self, start: f32, length: f32) -> f32 {
        round_to_pixels(start + length, self.scale) - round_to_pixels(start, self.scale)
    }

    /// The exact absolute origin of `id`, before snapping.
    fn origin(&self, id: LayoutId) -> (f32, f32) {
        match self.origins.get(slot(id)) {
            Some(&(x, y)) if !x.is_nan() => (x, y),
            // Not reached from the computed root: walk up the parents.
            _ => {
                let (mut x, mut y) = (0.0_f32, 0.0_f32);
                let mut current = Some(id);
                while let Some(node) = current {
                    let location = self.tree.layout(node).expect("invalid layout id").location;
                    x += location.x;
                    y += location.y;
                    current = self.tree.parent(node);
                }
                (x, y)
            }
        }
    }

    /// The width automatically wrapping text at `id` is shaped at, `None`
    /// when it fits unwrapped or `id` is not such text: the decision its
    /// measure made at the width layout resolved. Paint shapes the text
    /// here, so the painted lines are the measured ones.
    ///
    /// It reads the exact width. The snapped box can be up to a device
    /// pixel wider or narrower than the width measured, which can cross a
    /// line break: rewrapping at it would paint more or fewer lines than
    /// the height layout reserved.
    pub(super) fn auto_wrap_width(&self, id: LayoutId) -> Option<f32> {
        let Some(NodeMeasure::Text(text)) = self.tree.get_node_context(id) else {
            return None;
        };
        text.wrap_width(self.tree.unrounded_layout(id).size.width)
    }

    /// Width and height of what a scroll container at `id` scrolls over:
    /// the far edges of its laid-out children plus its end padding, at
    /// least its own size.
    /// Snapped like the container's bounds, so content that fits does not
    /// scroll.
    pub fn scroll_content_size(&self, id: LayoutId) -> (f32, f32) {
        let layout = self.tree.layout(id).expect("invalid layout id");
        let content = layout.content_size;
        let (x, y) = self.origin(id);
        (
            self.snapped_span(
                x,
                (content.width + layout.padding.right).max(layout.size.width),
            ),
            self.snapped_span(
                y,
                (content.height + layout.padding.bottom).max(layout.size.height),
            ),
        )
    }

    /// Start the next layout pass. Nodes and subtrees stay, found again by
    /// key; see the type docs.
    pub fn clear(&mut self) {
        self.nodes.next_pass(&mut self.tree);
        let Self {
            subtree_rows,
            subtrees,
            spare_subtrees,
            ..
        } = self;
        subtree_rows.next_pass(|row| {
            let sub = subtrees.swap_remove(row);
            if spare_subtrees.len() < SPARE_SUBTREES {
                spare_subtrees.push(sub);
            }
        });
        self.live_subtrees.clear();
        self.scopes.clear();
        self.scopes.push(Scope::ROOT);
        self.child_ids.clear();
        std::mem::swap(&mut self.replay_memo, &mut self.last_replay_memo);
        self.replay_memo.clear();
        self.stale.clear();
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    /// The node store holds every taffy node once under the key map, the
    /// pass stamps match the claims, and no container is open. O(nodes +
    /// subtrees); passes call it through `debug_assert!` when they start
    /// and finish.
    pub fn verify_integrity(&self) -> Result<(), LayoutIntegrityError> {
        self.nodes.verify_integrity(&self.tree)?;
        self.subtree_rows.verify_integrity("subtrees")?;
        if self.subtrees.len() != self.subtree_rows.len() {
            return Err(LayoutIntegrityError::ColumnLength {
                table: "subtrees",
                column: "subtree",
                len: self.subtrees.len(),
                rows: self.subtree_rows.len(),
            });
        }
        let live = |&row: &u32| self.subtree_rows.is_claimed(row as usize);
        if self.live_subtrees.len() != self.subtree_rows.claimed()
            || !self.live_subtrees.iter().all(live)
        {
            return Err(LayoutIntegrityError::PassAccounting { table: "subtrees" });
        }
        if self.scopes.len() != 1 || !self.child_ids.is_empty() {
            return Err(LayoutIntegrityError::OpenScope);
        }
        Ok(())
    }

    /// Nodes claimed this pass, across subtrees: the layout work a frame
    /// asked for.
    #[cfg(test)]
    pub(super) fn node_count(&self) -> usize {
        self.nodes.claimed()
            + self
                .live_subtrees
                .iter()
                .map(|&row| self.subtrees[row as usize].engine.node_count())
                .sum::<usize>()
    }
}

/// Children of a requested node: a slice, or the engine's child stack
/// from a mark.
enum Children<'a> {
    Slice(&'a [LayoutId]),
    Stack(usize),
}

/// What a requested node measures with.
enum Context {
    None,
    Measure(NodeMeasure),
    Subtree(usize),
    Replay { memo: Range<usize>, slot: u32 },
}

impl Context {
    fn into_measure(self) -> Option<NodeMeasure> {
        match self {
            Self::None => None,
            Self::Measure(measure) => Some(measure),
            Self::Subtree(index) => Some(NodeMeasure::Subtree(index)),
            Self::Replay { memo, slot } => Some(NodeMeasure::Replay { memo, slot }),
        }
    }
}

/// A cache boundary's content root: a block box, so the content stretches
/// to the width the parent gives the boundary and keeps its own height.
/// With `fill_height` the placed root is as tall as the boundary, so a
/// percentage height inside it (`h_full`) resolves against the boundary;
/// sizing queries still see the content's own height.
fn boundary_root_style(fill_height: bool) -> taffy::Style {
    taffy::Style {
        display: taffy::Display::Block,
        size: taffy::Size {
            width: taffy::Dimension::auto(),
            height: if fill_height {
                taffy::Dimension::percent(1.0)
            } else {
                taffy::Dimension::auto()
            },
        },
        ..Default::default()
    }
}
