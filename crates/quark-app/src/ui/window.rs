//! One window's share of the UI adapter: everything input routing, focus,
//! IME, accessibility, and painting remember between that window's frames.
//! The app, its messages, signals, theme, and key bindings are the
//! adapter's and shared by every window.

use quark::scene::Scene;
use quark_ui::FocusId;
use quark_ui::accessibility::{AccessibilityFrame, Announcer};
use quark_ui::animation::{AnimationTable, PresenceTable};
use quark_ui::element::{
    DragPreviewLayer, ElementCache, ElementHandles, ImeTarget, InputFrame, InputRouter,
    TextInputHitArea, TooltipRegion,
};
use quark_ui::text_input::TextPointer;

use super::focus::FocusState;
use super::ime::ImeState;
use super::jump::JumpState;
use super::pointer::PointerState;
use crate::WindowHandle;
use crate::platform::material::MaterialRect;

pub(crate) struct WindowUiState {
    /// Names the accessibility tree's window node; the adapter's name when
    /// unset.
    pub(super) name: Option<String>,
    pub(super) focus: FocusState,
    pub(super) pointer: PointerState,
    /// Routes input through the last painted frame until the next replaces
    /// it.
    pub(super) router: InputRouter,
    /// Handles the app names this window's elements with.
    pub(super) element_handles: ElementHandles,
    /// Paints the preview of the drag holding the pointer, or of the drag
    /// session over this window.
    pub(super) drag_preview: DragPreviewLayer,
    pub(super) jump: JumpState,
    pub(super) accessibility: AccessibilityFrame,
    pub(super) announcer: Announcer,
    /// Text fields of the last frame, for pointer selection and IME.
    pub(super) text_areas: Vec<TextInputHitArea>,
    pub(super) text_pointer: TextPointer,
    pub(super) ime: ImeState,
    /// Scale factor of the last painted frame, for accessibility bounds.
    pub(super) scale_factor: f32,
    pub(super) animations: AnimationTable,
    /// Keyed divs entering and leaving, and the frames exits still paint.
    pub(super) presence: PresenceTable,
    /// Cached subtrees and the layout engine, reused every frame.
    pub(super) element_cache: ElementCache,
    pub(super) spare: Spares,
    /// The material regions last handed to the window.
    pub(super) material_regions: Vec<MaterialRect>,
    #[cfg(feature = "devtools")]
    pub(super) devtools: quark_ui::inspector::Devtools,
}

/// Buffers of the frame before last, reused by the next frame: the scene
/// the runner handed back, the input frame routing let go of, the text
/// input areas, the IME targets, and the accessibility frame.
#[derive(Default)]
pub(super) struct Spares {
    pub(super) scene: Scene,
    pub(super) accessibility: AccessibilityFrame,
    pub(super) input: InputFrame,
    pub(super) text_areas: Vec<TextInputHitArea>,
    pub(super) ime_targets: Vec<ImeTarget>,
    /// Last frame's tooltip regions, reused by the next frame.
    pub(super) tooltip_regions: Vec<TooltipRegion>,
}

impl WindowUiState {
    pub(super) fn new() -> Self {
        Self {
            name: None,
            focus: FocusState::default(),
            pointer: PointerState::default(),
            router: InputRouter::default(),
            element_handles: ElementHandles::default(),
            drag_preview: DragPreviewLayer::default(),
            jump: JumpState::default(),
            accessibility: AccessibilityFrame::default(),
            announcer: Announcer::default(),
            text_areas: Vec::new(),
            text_pointer: TextPointer::default(),
            ime: ImeState::default(),
            scale_factor: 1.0,
            animations: AnimationTable::new(),
            presence: PresenceTable::new(),
            element_cache: ElementCache::new(),
            spare: Spares::default(),
            material_regions: Vec::new(),
            #[cfg(feature = "devtools")]
            devtools: quark_ui::inspector::Devtools::from_env(),
        }
    }

    /// The focused text field, if focus is on one painted last frame.
    pub(super) fn focused_field(&self) -> Option<FocusId> {
        (self.focus.current)
            .filter(|focus| self.text_areas.iter().any(|a| a.focus_target == *focus))
    }
}

/// What [`crate::testing`] reads of a window.
#[cfg(feature = "test-support")]
impl WindowUiState {
    pub(crate) fn accessibility_frame(&self) -> &AccessibilityFrame {
        &self.accessibility
    }

    pub(crate) fn semantic_frame(&self) -> &quark::SemanticFrame {
        &self.router.frame().semantic
    }

    pub(crate) fn geometry(&self) -> &quark_ui::element::LayoutSnapshot {
        &self.router.frame().geometry
    }

    pub(crate) fn focus(&self) -> Option<FocusId> {
        self.focus.current
    }

    /// The semantic node of the topmost hit region at a point that has one.
    pub(crate) fn semantic_node_at(&self, x: f32, y: f32) -> Option<usize> {
        let hits = &self.router.frame().hits;
        hits.stack_at(x, y).into_iter().find_map(|id| hits.node(id))
    }
}

/// The states of windows other than the one the adapter is acting for,
/// keyed by window; `None` is the state of callbacks bound to no window.
pub(super) type Parked = Vec<(Option<WindowHandle>, Box<WindowUiState>)>;

/// Every window's state: the one the adapter is acting for, and the rest.
pub(super) struct WindowStates<'a> {
    pub(super) handle: Option<WindowHandle>,
    pub(super) current: &'a mut WindowUiState,
    pub(super) parked: &'a mut Parked,
}

impl WindowStates<'_> {
    pub(super) fn get(&self, window: WindowHandle) -> Option<&WindowUiState> {
        if self.handle == Some(window) {
            return Some(self.current);
        }
        self.parked
            .iter()
            .find(|(handle, _)| *handle == Some(window))
            .map(|(_, state)| &**state)
    }

    pub(super) fn get_mut(&mut self, window: WindowHandle) -> Option<&mut WindowUiState> {
        if self.handle == Some(window) {
            return Some(self.current);
        }
        self.parked
            .iter_mut()
            .find(|(handle, _)| *handle == Some(window))
            .map(|(_, state)| &mut **state)
    }

    /// Every window with state, the current one first.
    pub(super) fn handles(&self) -> impl Iterator<Item = WindowHandle> + '_ {
        self.handle
            .into_iter()
            .chain(self.parked.iter().filter_map(|(handle, _)| *handle))
    }
}
