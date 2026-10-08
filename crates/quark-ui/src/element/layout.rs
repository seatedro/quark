use super::*;
use taffy::{AvailableSpace, TraversePartialTree};

// ---------------------------------------------------------------------------
// MeasureFunc — stored per-node for intrinsic sizing
// ---------------------------------------------------------------------------

pub(super) type MeasureFn = Box<
    dyn Fn(taffy::Size<Option<f32>>, taffy::Size<taffy::AvailableSpace>) -> taffy::Size<f32>
        + Send
        + Sync,
>;

pub(super) enum NodeMeasure {
    /// Leaf with an intrinsic measure function.
    Measure(MeasureFn),
    /// Text that wraps to the width layout gives it.
    Text(TextMeasure),
    /// A cache boundary whose content is laid out in `subtrees[index]`.
    Subtree(usize),
    /// A cache boundary replayed from the cache: it answers the measure
    /// queries recorded with it, `memo[range]`, and flags `slot` stale on
    /// any other query.
    Replay { memo: Range<usize>, slot: u32 },
}

/// What measured text borrows while layout computes: the window's text
/// system and layout cache, so wrapped sizes are shaped (and cached) by the
/// same system paint uses.
pub struct MeasureContext<'a> {
    pub text: &'a mut TextSystem,
    pub layouts: &'a mut LayoutCache,
}

impl ElementContext<'_> {
    /// The context's text state, for computing a layout.
    pub fn measure_context(&mut self) -> MeasureContext<'_> {
        MeasureContext {
            text: self.text,
            layouts: self.layouts,
        }
    }
}

impl MeasureContext<'_> {
    fn layout(&mut self, query: &TextQuery) -> Option<Arc<TextLayout>> {
        self.layouts.layout_query(self.text, query).ok()
    }
}

/// A text leaf that wraps automatically. Its node keeps the descriptor
/// across frames while the unwrapped layout is the same one, so an
/// unchanged frame neither dirties nor measures it.
pub(super) struct TextMeasure {
    /// The text shaped without wrapping, at the frame's scale: its inputs
    /// are what a wrapped probe shapes, its width the max-content width.
    unwrapped: Arc<TextLayout>,
    /// Height of one line; the element is never shorter.
    line_height: f32,
    /// Lines shown; the element is as tall as these and clips the rest.
    max_lines: Option<usize>,
    /// Min-content width, once asked for.
    min_content: Option<f32>,
}

impl TextMeasure {
    pub(super) fn new(unwrapped: Arc<TextLayout>, line_height: f32) -> Self {
        Self {
            unwrapped,
            line_height,
            max_lines: None,
            min_content: None,
        }
    }

    pub(super) fn max_lines(mut self, max_lines: Option<usize>) -> Self {
        self.max_lines = max_lines;
        self
    }

    fn max_content(&self) -> f32 {
        self.unwrapped.size().0.ceil()
    }

    fn min_content(&mut self) -> f32 {
        let max = self.max_content();
        *self
            .min_content
            .get_or_insert_with(|| self.unwrapped.min_content_width().ceil().min(max))
    }

    /// A known width as given, otherwise the width offered up to the
    /// max-content width; the height of the text wrapped at that width.
    ///
    /// An offered width is not raised to the min-content width: a leaf's
    /// final query offers its resolved width as available space without
    /// marking it known, and the height must be the one paint wraps to at
    /// that width. Flex items still stop shrinking at the min-content width
    /// through taffy's automatic minimum size.
    fn measure(
        &mut self,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
        cx: &mut MeasureContext,
    ) -> taffy::Size<f32> {
        let width = known.width.unwrap_or_else(|| match available.width {
            AvailableSpace::MinContent => self.min_content(),
            AvailableSpace::MaxContent => self.max_content(),
            AvailableSpace::Definite(offered) => offered.min(self.max_content()),
        });
        let height = known.height.unwrap_or_else(|| {
            let wrapped = auto_wrap_width(width, self.max_content())
                .and_then(|wrap| cx.layout(&self.unwrapped.query().wrap_width(Some(wrap))));
            let layout = wrapped.as_deref().unwrap_or(&self.unwrapped);
            text_height(layout, self.max_lines, self.line_height)
        });
        taffy::Size { width, height }
    }
}

/// Height of `layout` showing at most `max_lines`, never shorter than one
/// line, in whole pixels.
pub(super) fn text_height(layout: &TextLayout, max_lines: Option<usize>, line_height: f32) -> f32 {
    let height = match max_lines.and_then(|n| layout.line(n)) {
        Some(first_hidden) => first_hidden.top,
        None => layout.size().1,
    };
    height.max(line_height).ceil()
}

/// The wrap width automatically wrapped text of max-content width
/// `max_content` is shaped at in a box `width` wide: `None` when it fits
/// unwrapped. Measurement and paint both use it, so the painted lines are
/// the measured ones.
///
/// The width is floored: taffy rounds a box to whole pixels after
/// measuring it, and the rounded width is never below the floor of the
/// measured one, so paint never wraps narrower than measurement did.
pub(super) fn auto_wrap_width(width: f32, max_content: f32) -> Option<f32> {
    (width < max_content).then(|| width.floor().max(1.0))
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
        cx: &mut MeasureContext,
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

/// Layout of a window, retained across frames. Hosts keep one per window
/// and [`clear`](Self::clear) it before each frame's layout pass.
///
/// Nodes are reused by position: the `n`th node requested in a frame is
/// the `n`th node of the last frame, restyled or re-parented only where it
/// differs. Taffy keeps each node's cached layout until a change dirties
/// it, so an unchanged frame computes nothing and allocates nothing, and a
/// changed one recomputes only the changed nodes' ancestors.
pub struct LayoutEngine {
    pub(super) tree: taffy::TaffyTree<NodeMeasure>,
    /// Nodes in request order; this frame has requested `nodes[..cursor]`.
    nodes: Vec<LayoutId>,
    cursor: usize,
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
            nodes: Vec::new(),
            cursor: 0,
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
        self.node(&style, Children::Slice(children), Context::None)
    }

    /// The next node in request order, made to match `style`, `children`,
    /// and `context`. Only what differs from last frame's node at this
    /// position is set, since every set dirties the node's cached layout.
    fn node(&mut self, style: &taffy::Style, children: Children, context: Context) -> LayoutId {
        let children = match children {
            Children::Slice(ids) => ids,
            Children::Stack(mark) => &self.child_ids[mark..],
        };
        let Some(&node) = self.nodes.get(self.cursor) else {
            let node = self
                .tree
                .new_with_children(style.clone(), children)
                .expect("taffy new_with_children failed");
            if let Some(context) = context.into_measure() {
                self.tree
                    .set_node_context(node, Some(context))
                    .expect("valid node");
            }
            self.nodes.push(node);
            self.cursor += 1;
            return node;
        };
        self.cursor += 1;
        let tree = &mut self.tree;
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
            // The same replayed entry answers from the same memo; only its
            // place in this frame's memo buffer moved.
            (
                Context::Replay { memo, slot },
                Some(NodeMeasure::Replay {
                    memo: old,
                    slot: old_slot,
                }),
            ) if *old_slot == slot => *old = memo,
            // The same shaped text measures the same; keeping the node
            // context keeps taffy's cached sizes for it.
            (Context::Measure(NodeMeasure::Text(text)), Some(NodeMeasure::Text(old)))
                if Arc::ptr_eq(&text.unwrapped, &old.unwrapped)
                    && text.line_height == old.line_height
                    && text.max_lines == old.max_lines => {}
            (context, _) => {
                tree.set_node_context(node, context.into_measure())
                    .expect("valid node");
            }
        }
        node
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
    pub(super) fn finish_children(&mut self, style: &taffy::Style, mark: usize) -> LayoutId {
        let id = self.node(style, Children::Stack(mark), Context::None);
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
        let measure = NodeMeasure::Measure(Box::new(measure));
        self.node(&style, Children::Slice(&[]), Context::Measure(measure))
    }

    /// Create a leaf for text that wraps to the width layout gives it; see
    /// [`TextMeasure`].
    pub(super) fn request_text_layout(
        &mut self,
        style: &taffy::Style,
        text: TextMeasure,
    ) -> LayoutId {
        let measure = NodeMeasure::Text(text);
        self.node(style, Children::Slice(&[]), Context::Measure(measure))
    }

    /// Claim a cleared subtree for a cache boundary's content. Lay the
    /// content out in [`Self::subtree_mut`], then close it with
    /// [`Self::finish_subtree`]. Subtrees go to boundaries in build order,
    /// not by key: when the set of rebuilt boundaries shifts, a boundary
    /// can get an engine whose nodes another shape left behind.
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
        // Always set, so the host is dirtied: its content may have changed.
        let host = self.node(&style, Children::Slice(&[]), Context::Subtree(index));
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
        self.node(&style, Children::Slice(&[]), Context::Replay { memo, slot })
    }

    /// Lay out a detached boundary root (a rebuilt cached subtree) at its
    /// final size.
    pub(super) fn layout_boundary(
        &mut self,
        content: LayoutId,
        width: f32,
        height: f32,
        cx: &mut MeasureContext,
    ) {
        let root = self.request_layout(boundary_root_style(), &[content]);
        self.compute_layout(root, width, height, cx);
    }

    /// Move the cache slots whose replayed measures went stale into `out`,
    /// from this engine and every live subtree.
    pub(super) fn drain_stale(&mut self, out: &mut Vec<u32>) {
        out.append(&mut self.stale);
        for sub in &mut self.subtrees[..self.live_subtrees] {
            sub.engine.drain_stale(out);
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
        // Nodes past the cursor belong to a larger earlier frame.
        for node in self.nodes.drain(self.cursor..) {
            self.tree.remove(node).expect("valid node");
        }
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

    /// Width and height of what a scroll container at `id` scrolls over:
    /// the far edges of its laid-out children plus its end padding, at
    /// least its own size.
    pub fn scroll_content_size(&self, id: LayoutId) -> (f32, f32) {
        let layout = self.tree.layout(id).expect("invalid layout id");
        let content = layout.content_size;
        (
            (content.width + layout.padding.right).max(layout.size.width),
            (content.height + layout.padding.bottom).max(layout.size.height),
        )
    }

    /// Start the next layout pass. Nodes stay and are reused in request
    /// order; see the type docs.
    pub fn clear(&mut self) {
        self.cursor = 0;
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
        self.cursor
            + self.subtrees[..self.live_subtrees]
                .iter()
                .map(|sub| sub.engine.node_count())
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
fn boundary_root_style() -> taffy::Style {
    taffy::Style {
        display: taffy::Display::Block,
        ..Default::default()
    }
}
