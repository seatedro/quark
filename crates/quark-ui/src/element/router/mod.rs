//! Input routing: one hit table, handlers keyed by semantic node, and
//! capture/bubble dispatch along [`SemanticFrame`] routes.

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use super::*;

mod binding;
mod handlers;
mod keys;
mod pointer;
mod scroll;
#[cfg(test)]
mod tests;

pub use binding::{Binding, BindingError};
pub(in crate::element) use handlers::HandlerMarks;
pub use handlers::InputHandlers;
use pointer::Capture;
pub use quark::hit::Mods;
use scroll::Wheel;
pub use scroll::{HandleTarget, ScrollTarget, WHEEL_LINE_PX, WheelEvent};

/// What one painted frame registered for input.
#[derive(Default)]
pub struct InputFrame {
    pub hits: HitTable,
    pub handlers: InputHandlers,
    pub semantic: SemanticFrame,
    /// Where the frame's identified elements landed.
    pub geometry: LayoutSnapshot,
    /// Where the frame takes drops.
    pub drop_targets: DropTargets,
    /// Native material regions the frame asks for, in paint order.
    pub material_regions: Vec<MaterialRegionRequest>,
}

/// The outcome of routing one event: the semantic node that handled it
/// and the actions it emitted.
#[derive(Debug, Default)]
pub struct Delivery {
    pub node: Option<usize>,
    pub actions: Vec<Action>,
    /// The event moved a [`ScrollHandle`] (or its scrollbar state) or a
    /// drag preview, so the window needs a repaint even without actions.
    pub redraw: bool,
    /// A press no handler took landed on blank [`Div::window_drag_region`]
    /// chrome (see [`InputRouter::window_drag_at`]): the host starts the
    /// native window move now, while the press is current, unless
    /// selectable text under the pointer takes the press.
    pub window_drag: bool,
}

impl Delivery {
    /// `node` handled the event, emitting `actions`.
    fn handled(node: usize, actions: Vec<Action>) -> Self {
        Self {
            node: Some(node),
            actions,
            ..Self::default()
        }
    }
}

/// Routes pointer, wheel, and key input through the last painted frame.
#[derive(Default)]
pub struct InputRouter {
    frame: InputFrame,
    focus_tree: FocusTree,
    /// The drag holding the pointer, from its press to its release.
    capture: Option<Capture>,
    wheel: Wheel,
    /// Identity of the node a middle press landed on, until its release.
    middle_press: Option<u64>,
    /// The modifier keys held now, for clicks and drags.
    modifiers: Mods,
}

impl InputRouter {
    /// The modifier keys held now, for clicks and drags.
    pub fn set_modifiers(&mut self, modifiers: Mods) {
        self.modifiers = modifiers;
    }

    pub fn set_frame(&mut self, frame: InputFrame) {
        frame.semantic.fill_focus_tree(&mut self.focus_tree);
        self.frame = frame;
        if let Some(capture) = &mut self.capture {
            capture.follow(&node_identities(&self.frame.semantic), &self.frame.geometry);
        }
    }

    /// [`Self::set_frame`], returning the previous frame so its buffers
    /// can be reused for the next one (see
    /// [`ElementContext::with_input_frame`]).
    pub fn replace_frame(&mut self, frame: InputFrame) -> InputFrame {
        let previous = std::mem::take(&mut self.frame);
        self.set_frame(frame);
        previous
    }

    pub fn frame(&self) -> &InputFrame {
        &self.frame
    }

    pub fn is_capturing(&self) -> bool {
        self.capture.is_some()
    }

    /// Topmost semantic node under the point, honoring clips, z, and
    /// `BLOCKS_MOUSE`.
    pub fn target_at(&self, x: f32, y: f32) -> Option<usize> {
        self.frame
            .hits
            .stack_at(x, y)
            .into_iter()
            .find_map(|id| self.frame.hits.node(id))
    }

    /// Capture then bubble along `target`'s route. `handle` runs at the
    /// target and bubble steps; the first `Some` handles the event. An event
    /// binding whose default result stops propagation ends the walk at its
    /// node.
    fn walk(
        &self,
        target: usize,
        kind: UiEventKind,
        mut handle: impl FnMut(usize) -> Option<Vec<Action>>,
    ) -> Delivery {
        let Some(steps) = self.frame.semantic.route_indices(target) else {
            return Delivery::default();
        };
        for (node, phase) in steps {
            if phase != UiEventPhase::Capture
                && let Some(actions) = handle(node)
            {
                return Delivery::handled(node, actions);
            }
            if self.binding_stops(node, kind, phase) {
                return Delivery::handled(node, Vec::new());
            }
        }
        Delivery::default()
    }

    fn binding_stops(&self, node: usize, kind: UiEventKind, phase: UiEventPhase) -> bool {
        let capture = phase == UiEventPhase::Capture;
        self.frame.semantic.nodes()[node]
            .event_bindings
            .iter()
            .any(|binding| {
                binding.kind == kind
                    && (binding.phase == UiEventPhase::Capture) == capture
                    && !binding.default_result.should_continue()
            })
    }
}

/// Each node's identity across frames: its stable id or test id when it
/// has one, else its key, focus target, or position among its parent's
/// children, chained to its parent's identity.
fn node_identities(semantic: &SemanticFrame) -> Vec<u64> {
    let nodes = semantic.nodes();
    let mut identities: Vec<u64> = Vec::with_capacity(nodes.len());
    let mut children: HashMap<Option<usize>, u32> = HashMap::new();
    for (index, node) in nodes.iter().enumerate() {
        let ordinal = children.entry(node.parent).or_default();
        let position = *ordinal;
        *ordinal += 1;
        let mut hasher = DefaultHasher::new();
        if let Some(id) = &node.id {
            (0u8, id).hash(&mut hasher);
        } else if let Some(test_id) = &node.test_id {
            (1u8, test_id).hash(&mut hasher);
        } else {
            // Parents precede children in a well-formed frame.
            let parent = node.parent.filter(|p| *p < index).map(|p| identities[p]);
            parent.hash(&mut hasher);
            match (&node.key, node.focus) {
                (Some(key), _) => (2u8, key).hash(&mut hasher),
                (None, Some(focus)) => (3u8, focus).hash(&mut hasher),
                (None, None) => (4u8, position).hash(&mut hasher),
            }
        }
        identities.push(hasher.finish());
    }
    identities
}
