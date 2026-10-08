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
            let wrapped = self
                .wrap_width(width)
                .and_then(|wrap| cx.layout(&self.unwrapped.query().wrap_width(Some(wrap))));
            let layout = wrapped.as_deref().unwrap_or(&self.unwrapped);
            text_height(layout, self.max_lines, self.line_height)
        });
        taffy::Size { width, height }
    }

    /// The width the text is shaped at in a box `width` wide: `None` when
    /// it fits unwrapped.
    ///
    /// The width is floored: taffy rounds a box to whole pixels after
    /// measuring it, and the rounded width is never below the floor of the
    /// measured one, so the wrapped lines fit the painted box.
    fn wrap_width(&self, width: f32) -> Option<f32> {
        (width < self.max_content()).then(|| width.floor().max(1.0))
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
    /// Whether a query at a definite width is answered by measuring the
    /// root instead of laying the content out; see [`Self::size`]. Set
    /// for content taffy can measure without laying any of it out, until
    /// a query lays it out.
    measures: bool,
}

impl Subtree {
    fn new() -> Self {
        Self {
            engine: LayoutEngine::new(),
            root: None,
            host: None,
            memo: Vec::new(),
            laid_out_at: None,
            measures: false,
        }
    }

    fn clear(&mut self) {
        self.engine.clear();
        self.root = None;
        self.host = None;
        self.memo.clear();
        self.laid_out_at = None;
        self.measures = false;
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

    /// The size the content takes for one parent query, as
    /// [`Self::layout`] would lay it out.
    ///
    /// At a definite width the root is only measured: the content is laid
    /// out once, at the boundary's final size, after the parent's pass.
    /// The block root takes that width whatever height is offered, so the
    /// content is measured with the inputs it is laid out with, and taffy
    /// keeps the result among the content's sizes, not as its layout.
    ///
    /// A query at a min- or max-content width lays the content out, and so
    /// does every later query this frame. Taffy answers a query from a
    /// layout it kept when the layout's size is the one asked for, and the
    /// root sizes the content at the width its intrinsic layout found:
    /// the answer is that layout, where measuring at that width computes
    /// afresh and can differ, as for a row whose percentage-wide items
    /// take no space when it is sized by its content.
    fn size(
        &mut self,
        known: taffy::Size<Option<f32>>,
        available: taffy::Size<AvailableSpace>,
        cx: &mut MeasureContext,
    ) -> taffy::Size<f32> {
        let width = known
            .width
            .map_or(available.width, AvailableSpace::Definite);
        match self.root {
            Some(root) if self.measures && width.is_definite() => {
                let available = taffy::Size {
                    width,
                    height: known
                        .height
                        .map_or(available.height, AvailableSpace::Definite),
                };
                let size = self.engine.compute_size(root, available, cx);
                taffy::Size {
                    width: known.width.unwrap_or(size.width),
                    height: known.height.unwrap_or(size.height),
                }
            }
            _ => {
                self.measures = false;
                self.layout(known, available, cx)
            }
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
        let size = self.size(known, available, cx);
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
    /// The rest are spares a larger earlier frame left: detached, without
    /// measure contexts, kept for a later frame to reuse with their storage.
    nodes: Vec<LayoutId>,
    cursor: usize,
    /// The cursor when the last pass finished: nodes past it are spares.
    finished: usize,
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
    /// Whether a node requested this frame makes taffy lay out its
    /// children while only measuring it; see [`lays_out_when_measured`].
    lays_out_when_measured: bool,
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
            finished: 0,
            origins: Vec::new(),
            walk: Vec::new(),
            child_ids: Vec::new(),
            subtrees: Vec::new(),
            live_subtrees: 0,
            replay_memo: Vec::new(),
            stale: Vec::new(),
            lays_out_when_measured: false,
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
        self.lays_out_when_measured |= lays_out_when_measured(style, children);
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
        let tree = &sub.engine.tree;
        // Measuring stands in for layout only where taffy measures the
        // content without laying any of it out: the block root measures a
        // flex or grid container, and nothing inside lays its children out
        // to measure itself. A node laid out while measuring keeps that
        // layout when the final pass is answered from the cache.
        sub.measures = matches!(
            tree.style(content).map(|style| style.display),
            Ok(taffy::Display::Flex | taffy::Display::Grid)
        ) && tree.child_count(content) > 0
            && !sub.engine.lays_out_when_measured
            && !layout_by_layout();
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
        tree.compute_layout_with_measure(root, available, |known, available, _, context, _| {
            measure_node(context, known, available, subtrees, replay_memo, stale, cx)
        })
        .expect("taffy compute_layout failed");
    }

    /// The size [`Self::compute`] would give `root`, without laying out
    /// anything; unrounded.
    fn compute_size(
        &mut self,
        root: LayoutId,
        available: taffy::Size<AvailableSpace>,
        cx: &mut MeasureContext,
    ) -> taffy::Size<f32> {
        let Self {
            tree,
            subtrees,
            replay_memo,
            stale,
            ..
        } = self;
        tree.compute_size_with_measure(root, available, |known, available, _, context, _| {
            measure_node(context, known, available, subtrees, replay_memo, stale, cx)
        })
        .expect("taffy compute_size failed")
    }

    /// After the pass: lay every subtree out at its host's final size, and
    /// record absolute origins so [`Self::layout_bounds`] is a lookup.
    fn finish(&mut self, root: LayoutId, cx: &mut MeasureContext) {
        // Nodes past the cursor belong to a larger earlier frame. They stay,
        // so a frame that needs them again reuses them (and their child
        // lists) instead of creating nodes: no node of this frame has them
        // as children, and dropping their contexts releases the text and
        // closures those hold.
        for &node in self
            .nodes
            .get(self.cursor..self.finished)
            .unwrap_or_default()
        {
            if self.tree.get_node_context(node).is_some() {
                self.tree.set_node_context(node, None).expect("valid node");
            }
        }
        self.finished = self.cursor;
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

    /// The width automatically wrapping text at `id` is shaped at, `None`
    /// when it fits unwrapped or `id` is not such text: the decision its
    /// measure made at the width layout resolved. Paint shapes the text
    /// here, so the painted lines are the measured ones.
    ///
    /// It reads the unrounded width. The rounded box can be up to a pixel
    /// wider or narrower than the width measured, which can cross a line
    /// break: rewrapping at it would paint more or fewer lines than the
    /// height layout reserved.
    pub(super) fn auto_wrap_width(&self, id: LayoutId) -> Option<f32> {
        let Some(NodeMeasure::Text(text)) = self.tree.get_node_context(id) else {
            return None;
        };
        text.wrap_width(self.tree.unrounded_layout(id).size.width)
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
        self.lays_out_when_measured = false;
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

/// Size of a node with `context` for a taffy query.
fn measure_node(
    context: Option<&mut NodeMeasure>,
    known: taffy::Size<Option<f32>>,
    available: taffy::Size<AvailableSpace>,
    subtrees: &mut [Subtree],
    replay_memo: &[MeasureMemo],
    stale: &mut Vec<u32>,
    cx: &mut MeasureContext,
) -> taffy::Size<f32> {
    match context {
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
    }
}

/// Whether taffy lays out the children of a node with `style` while it
/// only measures the node: a block container lays out the blocks and
/// leaves in it, and a flex or grid item aligned on its baseline is laid
/// out to find the baseline.
fn lays_out_when_measured(style: &taffy::Style, children: &[LayoutId]) -> bool {
    (style.display == taffy::Display::Block && !children.is_empty())
        || style.align_items == Some(taffy::AlignItems::Baseline)
        || style.align_self == Some(taffy::AlignSelf::Baseline)
}

#[cfg(test)]
std::thread_local! {
    /// Whether subtrees answer every query by laying their content out,
    /// as before they could measure it: the differential tests' reference.
    static LAYOUT_BY_LAYOUT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(test)]
fn layout_by_layout() -> bool {
    LAYOUT_BY_LAYOUT.with(std::cell::Cell::get)
}

#[cfg(not(test))]
fn layout_by_layout() -> bool {
    false
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_alloc;
    use quark_text::TextQuery;
    use taffy::{Dimension, LengthPercentage, LengthPercentageAuto};

    fn sized(width: f32, height: f32) -> taffy::Style {
        taffy::Style {
            size: taffy::Size {
                width: Dimension::length(width),
                height: Dimension::length(height),
            },
            ..Default::default()
        }
    }

    fn flex(direction: taffy::FlexDirection) -> taffy::Style {
        taffy::Style {
            display: taffy::Display::Flex,
            flex_direction: direction,
            ..Default::default()
        }
    }

    /// One frame of a window of nested flex rows and columns, a block, a
    /// wrapping row, and an absolute overlay: `width` sizes one deep leaf,
    /// and `popup` adds a container with two children.
    fn frame(engine: &mut LayoutEngine, cx: &mut MeasureContext, width: f32, popup: bool) {
        engine.clear();
        let title = engine.request_layout(sized(120.0, 20.0), &[]);
        let spacer = engine.request_layout(
            taffy::Style {
                flex_grow: 1.0,
                ..Default::default()
            },
            &[],
        );
        let button = engine.request_layout(sized(30.0, 20.0), &[]);
        let header = engine.request_layout(
            taffy::Style {
                gap: taffy::Size {
                    width: LengthPercentage::length(8.0),
                    height: LengthPercentage::length(0.0),
                },
                ..flex(taffy::FlexDirection::Row)
            },
            &[title, spacer, button],
        );
        let items = [
            engine.request_layout(sized(30.0, 30.0), &[]),
            engine.request_layout(sized(30.0, 30.0), &[]),
        ];
        let sidebar = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: Dimension::length(80.0),
                    height: Dimension::auto(),
                },
                ..flex(taffy::FlexDirection::Column)
            },
            &items,
        );
        let tiles: [LayoutId; 6] =
            std::array::from_fn(|_| engine.request_layout(sized(50.0, 24.0), &[]));
        let grid = engine.request_layout(
            taffy::Style {
                flex_wrap: taffy::FlexWrap::Wrap,
                ..flex(taffy::FlexDirection::Row)
            },
            &tiles,
        );
        let deep = engine.request_layout(sized(width, 10.0), &[]);
        let column = engine.request_layout(flex(taffy::FlexDirection::Column), &[deep]);
        let content = engine.request_layout(
            taffy::Style {
                display: taffy::Display::Block,
                flex_grow: 1.0,
                ..Default::default()
            },
            &[grid, column],
        );
        let body = engine.request_layout(
            taffy::Style {
                flex_grow: 1.0,
                ..flex(taffy::FlexDirection::Row)
            },
            &[sidebar, content],
        );
        let mark = engine.begin_children();
        engine.push_child(header);
        engine.push_child(body);
        if popup {
            let lines = [
                engine.request_layout(sized(60.0, 12.0), &[]),
                engine.request_layout(sized(40.0, 12.0), &[]),
            ];
            let popup = engine.request_layout(
                taffy::Style {
                    position: taffy::Position::Absolute,
                    inset: taffy::Rect {
                        left: LengthPercentageAuto::percent(0.5),
                        top: LengthPercentageAuto::length(40.0),
                        right: LengthPercentageAuto::auto(),
                        bottom: LengthPercentageAuto::auto(),
                    },
                    ..flex(taffy::FlexDirection::Column)
                },
                &lines,
            );
            engine.push_child(popup);
        }
        let root = engine.finish_children(
            &taffy::Style {
                padding: taffy::Rect::length(4.0_f32),
                ..flex(taffy::FlexDirection::Column)
            },
            mark,
        );
        engine.compute_layout(root, 400.0, 300.0, cx);
    }

    /// Allocations of the last of `frames`, each `(width, popup)`.
    fn last_frame_allocations(frames: &[(f32, bool)]) -> u64 {
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let mut cx = MeasureContext {
            text: &mut text,
            layouts: &mut layouts,
        };
        let mut engine = LayoutEngine::new();
        let (last, warmup) = frames.split_last().unwrap();
        for &(width, popup) in warmup {
            frame(&mut engine, &mut cx, width, popup);
        }
        test_alloc::count(|| frame(&mut engine, &mut cx, last.0, last.1)).1
    }

    #[test]
    fn warmed_relayout_allocates_nothing() {
        let frames = [(10.0, false), (20.0, false), (10.0, false), (20.0, false)];
        assert_eq!(last_frame_allocations(&frames), 0);
    }

    #[test]
    fn warmed_frame_that_adds_nodes_allocates_nothing() {
        let frames = [(10.0, false), (10.0, true), (10.0, false), (10.0, true)];
        assert_eq!(last_frame_allocations(&frames), 0);
    }

    // -----------------------------------------------------------------------
    // Differential: subtrees that measure against subtrees that lay out
    // -----------------------------------------------------------------------

    fn pct(value: f32) -> Dimension {
        Dimension::percent(value)
    }

    fn column() -> taffy::Style {
        flex(taffy::FlexDirection::Column)
    }

    fn row() -> taffy::Style {
        flex(taffy::FlexDirection::Row)
    }

    fn margins(left: f32, right: f32, top: f32, bottom: f32) -> taffy::Rect<LengthPercentageAuto> {
        taffy::Rect {
            left: LengthPercentageAuto::length(left),
            right: LengthPercentageAuto::length(right),
            top: LengthPercentageAuto::length(top),
            bottom: LengthPercentageAuto::length(bottom),
        }
    }

    /// Text that wraps to the width layout gives it, as `text()` lays out.
    fn text(engine: &mut LayoutEngine, cx: &mut MeasureContext, content: &str) -> LayoutId {
        let style = quark_text::TextStyle::new(14.0).line_height(18.0);
        let query = TextQuery::new(content, style);
        let unwrapped = cx.layout(&query).expect("vendored fonts shape");
        engine.request_text_layout(&taffy::Style::default(), TextMeasure::new(unwrapped, 18.0))
    }

    /// A cache boundary with `style` around the content `build` makes.
    fn boundary(
        engine: &mut LayoutEngine,
        cx: &mut MeasureContext,
        style: taffy::Style,
        build: impl FnOnce(&mut LayoutEngine, &mut MeasureContext) -> LayoutId,
    ) -> LayoutId {
        let index = engine.begin_subtree();
        let content = build(engine.subtree_mut(index), cx);
        engine.finish_subtree(index, style, content)
    }

    /// A message row as a transcript caches it: a column the boundary's
    /// width, an author line, and wrapped body text.
    fn message(engine: &mut LayoutEngine, cx: &mut MeasureContext, body: &str) -> LayoutId {
        boundary(engine, cx, taffy::Style::default(), |engine, cx| {
            let author = text(engine, cx, "Author");
            let body = text(engine, cx, body);
            let style = taffy::Style {
                size: taffy::Size {
                    width: pct(1.0),
                    height: Dimension::auto(),
                },
                padding: taffy::Rect::length(4.0_f32),
                ..column()
            };
            engine.request_layout(style, &[author, body])
        })
    }

    const BODY: &str = "The quick brown fox jumps over the lazy dog while the band plays on";

    /// Messages in a scrolling column, one of them edited every frame.
    fn transcript(engine: &mut LayoutEngine, cx: &mut MeasureContext, frame: usize) -> LayoutId {
        let mark = engine.begin_children();
        for i in 0..6 {
            let body = if i == 3 {
                &BODY[..20 + 7 * (frame % 6)]
            } else {
                &BODY[..10 * (i + 1)]
            };
            let id = message(engine, cx, body);
            engine.push_child(id);
        }
        let style = taffy::Style {
            size: taffy::Size {
                width: pct(1.0),
                height: pct(1.0),
            },
            overflow: taffy::Point {
                x: taffy::Overflow::Visible,
                y: taffy::Overflow::Scroll,
            },
            ..column()
        };
        engine.finish_children(&style, mark)
    }

    /// Boundaries growing and shrinking in a row: the row asks each for its
    /// min-content and max-content width as well as definite ones.
    fn grow_and_shrink(
        engine: &mut LayoutEngine,
        cx: &mut MeasureContext,
        frame: usize,
    ) -> LayoutId {
        let items = [
            (1.0, 1.0, &BODY[..30]),
            (2.0, 3.0, BODY),
            (0.0, 1.0, &BODY[..12 + frame % 3]),
        ];
        let mark = engine.begin_children();
        for (grow, shrink, body) in items {
            let style = taffy::Style {
                flex_grow: grow,
                flex_shrink: shrink,
                min_size: taffy::Size {
                    width: Dimension::length(40.0),
                    height: Dimension::auto(),
                },
                ..Default::default()
            };
            let id = boundary(engine, cx, style, |engine, cx| {
                let words = text(engine, cx, body);
                let icon = engine.request_layout(sized(16.0, 16.0), &[]);
                let style = taffy::Style {
                    gap: taffy::Size::length(4.0_f32),
                    margin: margins(2.0, 3.0, 1.0, 0.0),
                    ..row()
                };
                engine.request_layout(style, &[icon, words])
            });
            engine.push_child(id);
        }
        engine.finish_children(
            &taffy::Style {
                gap: taffy::Size::length(6.0_f32),
                ..row()
            },
            mark,
        )
    }

    /// Columns that wrap under a max height, in boundaries that grow in a
    /// column whose height is the window's.
    fn wrapping_columns(
        engine: &mut LayoutEngine,
        cx: &mut MeasureContext,
        frame: usize,
    ) -> LayoutId {
        let mark = engine.begin_children();
        for i in 0..3 {
            let style = taffy::Style {
                flex_grow: i as f32,
                ..Default::default()
            };
            let id = boundary(engine, cx, style, |engine, cx| {
                let mark = engine.begin_children();
                for j in 0..4 + (frame + i) % 3 {
                    let id = text(engine, cx, &BODY[..8 + 6 * j]);
                    engine.push_child(id);
                }
                let style = taffy::Style {
                    flex_wrap: taffy::FlexWrap::Wrap,
                    max_size: taffy::Size {
                        width: Dimension::auto(),
                        height: Dimension::length(60.0),
                    },
                    gap: taffy::Size::length(2.0_f32),
                    ..column()
                };
                engine.finish_children(&style, mark)
            });
            engine.push_child(id);
        }
        let style = taffy::Style {
            size: taffy::Size {
                width: pct(1.0),
                height: pct(1.0),
            },
            ..column()
        };
        engine.finish_children(&style, mark)
    }

    /// A boundary inside a boundary, an absolute overlay, and sizes,
    /// padding, and margins in percentages.
    fn nested_and_percentages(
        engine: &mut LayoutEngine,
        cx: &mut MeasureContext,
        frame: usize,
    ) -> LayoutId {
        let half = taffy::Style {
            size: taffy::Size {
                width: pct(0.5),
                height: Dimension::auto(),
            },
            ..Default::default()
        };
        let outer = boundary(engine, cx, half, |engine, cx| {
            let inner = boundary(engine, cx, taffy::Style::default(), |engine, cx| {
                let words = text(engine, cx, &BODY[..25 + 5 * (frame % 4)]);
                let bar = engine.request_layout(
                    taffy::Style {
                        size: taffy::Size {
                            width: pct(0.3),
                            height: Dimension::length(6.0),
                        },
                        flex_shrink: 0.0,
                        ..Default::default()
                    },
                    &[],
                );
                engine.request_layout(row(), &[words, bar])
            });
            let beside = boundary(engine, cx, taffy::Style::default(), |engine, cx| {
                let words = text(engine, cx, &BODY[..14 + 3 * (frame % 3)]);
                engine.request_layout(column(), &[words])
            });
            let note = text(engine, cx, &BODY[30..]);
            let line = engine.request_layout(row(), &[beside, note]);
            let label = text(engine, cx, &BODY[..18]);
            let badge = engine.request_layout(
                taffy::Style {
                    position: taffy::Position::Absolute,
                    inset: taffy::Rect {
                        left: LengthPercentageAuto::auto(),
                        right: LengthPercentageAuto::length(0.0),
                        top: LengthPercentageAuto::percent(0.1),
                        bottom: LengthPercentageAuto::auto(),
                    },
                    ..sized(10.0, 10.0)
                },
                &[],
            );
            let style = taffy::Style {
                padding: taffy::Rect {
                    left: LengthPercentage::percent(0.05),
                    right: LengthPercentage::length(3.0),
                    top: LengthPercentage::length(2.0),
                    bottom: LengthPercentage::percent(0.02),
                },
                margin: taffy::Rect {
                    left: LengthPercentageAuto::percent(0.1),
                    right: LengthPercentageAuto::length(0.0),
                    top: LengthPercentageAuto::length(4.0),
                    bottom: LengthPercentageAuto::length(4.0),
                },
                ..column()
            };
            engine.request_layout(style, &[label, inner, line, badge])
        });
        let fit = boundary(engine, cx, taffy::Style::default(), |engine, cx| {
            let words = text(engine, cx, &BODY[..40]);
            let style = taffy::Style {
                max_size: taffy::Size {
                    width: pct(0.8),
                    height: Dimension::auto(),
                },
                ..column()
            };
            engine.request_layout(style, &[words])
        });
        let fitted = engine.request_layout(
            taffy::Style {
                align_items: Some(taffy::AlignItems::FlexStart),
                ..column()
            },
            &[fit],
        );
        let side = text(engine, cx, BODY);
        engine.request_layout(column(), &[outer, fitted, side])
    }

    /// Content that measuring cannot stand in for: text as the boundary's
    /// whole content, and a row aligned on baselines.
    fn laid_out_while_measured(
        engine: &mut LayoutEngine,
        cx: &mut MeasureContext,
        frame: usize,
    ) -> LayoutId {
        let leaf = boundary(engine, cx, taffy::Style::default(), |engine, cx| {
            text(engine, cx, &BODY[..30 + frame % 5])
        });
        let baselines = boundary(engine, cx, taffy::Style::default(), |engine, cx| {
            let a = text(engine, cx, &BODY[..20]);
            let b = text(engine, cx, &BODY[20..]);
            let style = taffy::Style {
                align_items: Some(taffy::AlignItems::Baseline),
                ..row()
            };
            engine.request_layout(style, &[a, b])
        });
        engine.request_layout(column(), &[leaf, baselines])
    }

    /// Every node's rounded bounds and unrounded box, then each subtree's
    /// with the measures it answered, depth first.
    fn geometry(engine: &LayoutEngine, out: &mut Vec<String>) {
        for &node in &engine.nodes[..engine.cursor] {
            let exact = engine.tree.unrounded_layout(node);
            out.push(format!(
                "{:?} {:?} {:?}",
                engine.layout_bounds(node),
                exact.location,
                exact.size
            ));
        }
        for (i, sub) in engine.subtrees[..engine.live_subtrees].iter().enumerate() {
            out.push(format!("subtree {i}: {:?}", sub.memo));
            geometry(&sub.engine, out);
        }
    }

    type Scene = fn(&mut LayoutEngine, &mut MeasureContext, usize) -> LayoutId;

    /// Lays each scene out over frames of changing window sizes and
    /// content, once with subtrees that measure and once with subtrees that
    /// lay out for every query, and compares the geometry of every frame.
    #[test]
    fn measured_subtrees_lay_out_as_laid_out_ones() {
        let scenes: [(&str, Scene); 5] = [
            ("transcript", transcript),
            ("grow and shrink", grow_and_shrink),
            ("wrapping columns", wrapping_columns),
            ("nested and percentages", nested_and_percentages),
            ("laid out while measured", laid_out_while_measured),
        ];
        let windows = [
            (480.0, 400.0),
            (300.0, 400.0),
            (300.0, 250.0),
            (300.0, 250.0),
            (480.0, 400.0),
            (137.5, 400.0),
            (90.0, 120.0),
            (800.0, 600.0),
        ];
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        let mut cx = MeasureContext {
            text: &mut text,
            layouts: &mut layouts,
        };
        for (name, scene) in scenes {
            let mut engines = [LayoutEngine::new(), LayoutEngine::new()];
            for (frame, &(width, height)) in windows.iter().enumerate() {
                let [expected, actual] = [true, false].map(|by_layout| {
                    LAYOUT_BY_LAYOUT.with(|flag| flag.set(by_layout));
                    let engine = &mut engines[usize::from(!by_layout)];
                    engine.clear();
                    let root = scene(engine, &mut cx, frame);
                    engine.compute_layout(root, width, height, &mut cx);
                    let mut out = Vec::new();
                    geometry(engine, &mut out);
                    out
                });
                LAYOUT_BY_LAYOUT.with(|flag| flag.set(false));
                assert_eq!(actual.len(), expected.len(), "{name}, frame {frame}");
                for (i, (actual, expected)) in actual.iter().zip(&expected).enumerate() {
                    assert_eq!(actual, expected, "{name}, frame {frame}, entry {i}");
                }
            }
        }
    }
}
