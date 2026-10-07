use super::*;

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
}

// ---------------------------------------------------------------------------
// LayoutEngine — wraps TaffyTree
// ---------------------------------------------------------------------------

pub struct LayoutEngine {
    pub(super) tree: taffy::TaffyTree<NodeMeasure>,
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

    /// Compute layout for the entire tree rooted at `root`.
    pub fn compute_layout(&mut self, root: LayoutId, width: f32, height: f32) {
        self.tree
            .compute_layout_with_measure(
                root,
                taffy::Size {
                    width: taffy::AvailableSpace::Definite(width),
                    height: taffy::AvailableSpace::Definite(height),
                },
                |known, available, _node_id, context, _style| {
                    if let Some(NodeMeasure::Measure(f)) = context {
                        f(known, available)
                    } else {
                        taffy::Size::ZERO
                    }
                },
            )
            .expect("taffy compute_layout failed");
    }

    /// Get the resolved bounds for a layout node, in absolute coordinates.
    pub fn layout_bounds(&self, id: LayoutId) -> Bounds {
        let mut x = 0.0_f32;
        let mut y = 0.0_f32;

        // Walk up the tree to accumulate parent offsets.
        let mut current = id;
        loop {
            let layout = self.tree.layout(current).expect("invalid layout id");
            x += layout.location.x;
            y += layout.location.y;
            match self.tree.parent(current) {
                Some(parent) => current = parent,
                None => break,
            }
        }

        let layout = self.tree.layout(id).expect("invalid layout id");
        Bounds {
            x,
            y,
            width: layout.size.width,
            height: layout.size.height,
        }
    }

    /// Clear all nodes for the next frame.
    pub fn clear(&mut self) {
        self.tree.clear();
    }
}
