use super::*;
use taffy::{AvailableSpace, TraversePartialTree};

// ---------------------------------------------------------------------------
// MeasureFunc — stored per-node for intrinsic sizing (text)
// ---------------------------------------------------------------------------

pub(super) type MeasureFn = Box<
    dyn Fn(taffy::Size<Option<f32>>, taffy::Size<taffy::AvailableSpace>) -> taffy::Size<f32>
        + Send
        + Sync,
>;

pub(super) enum NodeMeasure {
    /// Leaf with no measure — sized by Taffy style alone.
    None,
    /// Leaf with an intrinsic measure function (e.g. text).
    Measure(MeasureFn),
    /// A cache boundary whose content is laid out in `subtrees[index]`.
    Subtree(usize),
    /// A cache boundary replayed from the cache: it answers the measure
    /// queries recorded with it, `memo[range]`, and flags `slot` stale on
    /// any other query.
    Replay { memo: Range<usize>, slot: u32 },
}

/// One measure query a cache boundary answered: what the parent offered
/// and the size it got back.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct MeasureMemo {
    known: taffy::Size<Option<f32>>,
    available: taffy::Size<AvailableSpace>,
    size: taffy::Size<f32>,
}

/// Content of one cache boundary, laid out as its own root so the parent
/// sees a single measured leaf.
struct Subtree {
    engine: LayoutEngine,
    /// The block wrapper around the content; the subtree's root.
    root: Option<LayoutId>,
    /// The boundary's leaf in the parent engine.
    host: Option<LayoutId>,
    /// Queries answered this frame, in order; recorded with the entry.
    memo: Vec<MeasureMemo>,
    /// The size the content was last laid out at.
    laid_out_at: Option<taffy::Size<Option<f32>>>,
}

impl Subtree {
    fn new() -> Self {
        Self {
            engine: LayoutEngine::new(),
            root: None,
            host: None,
            memo: Vec::new(),
            laid_out_at: None,
        }
    }

    fn clear(&mut self) {
        self.engine.clear();
        self.root = None;
        self.host = None;
        self.memo.clear();
        self.laid_out_at = None;
    }

    /// Lay the content out for one parent query. Known dimensions become
    /// definite available space, so the block wrapper takes the known
    /// width and the content's own height.
    fn layout(
        &mut self,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
    ) -> taffy::Size<f32> {
        let Some(root) = self.root else {
            return taffy::Size::ZERO;
        };
        if self.laid_out_at != Some(known) || known.width.is_none() || known.height.is_none() {
            let available = taffy::Size {
                width: known
                    .width
                    .map_or(available.width, AvailableSpace::Definite),
                height: known
                    .height
                    .map_or(available.height, AvailableSpace::Definite),
            };
            self.engine.compute(root, available);
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
    ) -> taffy::Size<f32> {
        if let Some(hit) = find_memo(&self.memo, known, available) {
            return hit;
        }
        let size = self.layout(known, available);
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

fn find_memo(
    memo: &[MeasureMemo],
    known: taffy::Size<Option<f32>>,
    available: taffy::Size<AvailableSpace>,
) -> Option<taffy::Size<f32>> {
    if let (Some(width), Some(height)) = (known.width, known.height) {
        return Some(taffy::Size { width, height });
    }
    memo.iter()
        .find(|m| m.known == known && m.available == available)
        .map(|m| m.size)
}

/// Taffy node ids are slotmap keys: the low 32 bits are the slot index,
/// dense from zero, so they index a plain vector.
fn slot(id: LayoutId) -> usize {
    (u64::from(id) & u64::from(u32::MAX)) as usize
}

// ---------------------------------------------------------------------------
// LayoutEngine — wraps TaffyTree
// ---------------------------------------------------------------------------

/// One frame's layout. Hosts keep one per window and [`clear`](Self::clear)
/// it between frames, so node storage, cache-boundary subtrees, and the
/// scratch vectors keep their capacity.
pub struct LayoutEngine {
    pub(super) tree: taffy::TaffyTree<NodeMeasure>,
    /// Absolute origin of every node reachable from the computed root, by
    /// slot; NaN for nodes the last pass did not reach.
    origins: Vec<(f32, f32)>,
    /// Depth-first stack for filling `origins`.
    walk: Vec<(LayoutId, f32, f32)>,
    /// Children ids of the containers being built, as a stack, so a
    /// container passes its children to taffy without a vector of its own.
    child_ids: Vec<LayoutId>,
    /// Cache-boundary subtrees; the first `live_subtrees` belong to this
    /// frame, the rest are pooled.
    subtrees: Vec<Subtree>,
    live_subtrees: usize,
    /// Measure memos of replayed boundaries this frame.
    replay_memo: Vec<MeasureMemo>,
    /// Cache slots whose replayed memo missed a query this frame.
    stale: Vec<u32>,
}

impl Default for LayoutEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl LayoutEngine {
    pub fn new() -> Self {
        Self {
            tree: taffy::TaffyTree::new(),
            origins: Vec::new(),
            walk: Vec::new(),
            child_ids: Vec::new(),
            subtrees: Vec::new(),
            live_subtrees: 0,
            replay_memo: Vec::new(),
            stale: Vec::new(),
        }
    }

    /// Create a layout node with the given style and children.
    pub fn request_layout(&mut self, style: taffy::Style, children: &[LayoutId]) -> LayoutId {
        if children.is_empty() {
            self.tree
                .new_leaf_with_context(style, NodeMeasure::None)
                .expect("taffy new_leaf failed")
        } else {
            self.tree
                .new_with_children(style, children)
                .expect("taffy new_with_children failed")
        }
    }

    /// Start collecting a container's children: push each with
    /// [`Self::push_child`], then build the container with
    /// [`Self::finish_children`] and this mark.
    pub(super) fn begin_children(&self) -> usize {
        self.child_ids.len()
    }

    pub(super) fn push_child(&mut self, id: LayoutId) {
        self.child_ids.push(id);
    }

    /// Create a container from the children pushed since `mark`.
    pub(super) fn finish_children(&mut self, style: taffy::Style, mark: usize) -> LayoutId {
        let id = if self.child_ids.len() == mark {
            self.tree
                .new_leaf_with_context(style, NodeMeasure::None)
                .expect("taffy new_leaf failed")
        } else {
            self.tree
                .new_with_children(style, &self.child_ids[mark..])
                .expect("taffy new_with_children failed")
        };
        self.child_ids.truncate(mark);
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
        self.tree
            .new_leaf_with_context(style, NodeMeasure::Measure(Box::new(measure)))
            .expect("taffy new_leaf_with_context failed")
    }

    /// Claim a cleared subtree for a cache boundary's content. Lay the
    /// content out in [`Self::subtree_mut`], then close it with
    /// [`Self::finish_subtree`].
    pub(super) fn begin_subtree(&mut self) -> usize {
        if self.live_subtrees == self.subtrees.len() {
            self.subtrees.push(Subtree::new());
        }
        self.live_subtrees += 1;
        self.live_subtrees - 1
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
    ) -> LayoutId {
        let sub = &mut self.subtrees[index];
        sub.root = Some(sub.engine.request_layout(boundary_root_style(), &[content]));
        let host = self
            .tree
            .new_leaf_with_context(style, NodeMeasure::Subtree(index))
            .expect("taffy new_leaf_with_context failed");
        self.subtrees[index].host = Some(host);
        host
    }

    /// Queries the subtree answered this frame, for recording.
    pub(super) fn subtree_memo(&self, index: usize) -> &[MeasureMemo] {
        &self.subtrees[index].memo
    }

    /// A leaf for a replayed boundary that answers from `memo`.
    pub(super) fn request_replay(
        &mut self,
        style: taffy::Style,
        memo: &[MeasureMemo],
        slot: u32,
    ) -> LayoutId {
        let start = self.replay_memo.len();
        self.replay_memo.extend_from_slice(memo);
        let memo = start..self.replay_memo.len();
        self.tree
            .new_leaf_with_context(style, NodeMeasure::Replay { memo, slot })
            .expect("taffy new_leaf_with_context failed")
    }

    /// Lay out a detached boundary root (a rebuilt cached subtree) at its
    /// final size.
    pub(super) fn layout_boundary(&mut self, content: LayoutId, width: f32, height: f32) {
        let root = self.request_layout(boundary_root_style(), &[content]);
        self.compute_layout(root, width, height);
    }

    /// Move the cache slots whose replayed measures went stale into `out`,
    /// from this engine and every live subtree.
    pub(super) fn drain_stale(&mut self, out: &mut Vec<u32>) {
        out.append(&mut self.stale);
        for sub in &mut self.subtrees[..self.live_subtrees] {
            sub.engine.drain_stale(out);
        }
    }

    /// Compute layout for the entire tree rooted at `root`.
    pub fn compute_layout(&mut self, root: LayoutId, width: f32, height: f32) {
        self.compute(
            root,
            taffy::Size {
                width: AvailableSpace::Definite(width),
                height: AvailableSpace::Definite(height),
            },
        );
        self.finish(root);
    }

    fn compute(&mut self, root: LayoutId, available: taffy::Size<AvailableSpace>) {
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
                Some(NodeMeasure::Subtree(index)) => subtrees[*index].measure(known, available),
                Some(NodeMeasure::Replay { memo, slot }) => {
                    find_memo(&replay_memo[memo.clone()], known, available).unwrap_or_else(|| {
                        stale.push(*slot);
                        taffy::Size::ZERO
                    })
                }
                Some(NodeMeasure::None) | None => taffy::Size::ZERO,
            },
        )
        .expect("taffy compute_layout failed");
    }

    /// After the pass: lay every subtree out at its host's final size, and
    /// record absolute origins so [`Self::layout_bounds`] is a lookup.
    fn finish(&mut self, root: LayoutId) {
        for sub in &mut self.subtrees[..self.live_subtrees] {
            let (Some(host), Some(sub_root)) = (sub.host, sub.root) else {
                continue;
            };
            let size = self.tree.unrounded_layout(host).size;
            sub.layout(
                taffy::Size {
                    width: Some(size.width),
                    height: Some(size.height),
                },
                taffy::Size {
                    width: AvailableSpace::Definite(size.width),
                    height: AvailableSpace::Definite(size.height),
                },
            );
            sub.engine.finish(sub_root);
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
    }

    /// Get the resolved bounds for a layout node, in absolute coordinates.
    pub fn layout_bounds(&self, id: LayoutId) -> Bounds {
        let layout = self.tree.layout(id).expect("invalid layout id");
        let (x, y) = match self.origins.get(slot(id)) {
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
        };
        Bounds {
            x,
            y,
            width: layout.size.width,
            height: layout.size.height,
        }
    }

    /// Clear all nodes for the next frame, keeping capacity.
    pub fn clear(&mut self) {
        self.tree.clear();
        for sub in &mut self.subtrees[..self.live_subtrees] {
            sub.clear();
        }
        self.live_subtrees = 0;
        self.child_ids.clear();
        self.replay_memo.clear();
        self.stale.clear();
    }

    /// Nodes created this frame, across subtrees: the layout work a frame
    /// asked for.
    #[cfg(test)]
    pub(super) fn node_count(&self) -> usize {
        self.tree.total_node_count()
            + self.subtrees[..self.live_subtrees]
                .iter()
                .map(|sub| sub.engine.node_count())
                .sum::<usize>()
    }
}

/// A cache boundary's content root: a block box, so the content stretches
/// to the width the parent gives the boundary and keeps its own height.
fn boundary_root_style() -> taffy::Style {
    taffy::Style {
        display: taffy::Display::Block,
        ..Default::default()
    }
}
