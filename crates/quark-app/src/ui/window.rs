//! One window's share of the UI adapter: everything input routing, focus,
//! IME, accessibility, and painting remember between that window's frames.
//! The app, its messages, signals, theme, and key bindings are the
//! adapter's and shared by every window.

use quark::Rect;
use quark::hit::HitId;
use quark::scene::Scene;
use quark_ui::FocusId;
use quark_ui::accessibility::{AccessibilityFrame, Announcer};
use quark_ui::animation::AnimationTable;
use quark_ui::element::{
    DragPreviewLayer, ElementCache, ElementHandles, ImeTarget, InputFrame, InputRouter,
    TextInputHitArea, TooltipRegion,
};
use quark_ui::text_input::TextPointer;

use crate::WindowHandle;

/// The platform IME's composition, as IME events and focus left it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Composition {
    None,
    /// Composing in this element, focused when its preedit began, until
    /// the commit ends it.
    Owned(FocusId),
    /// Focus left the element composing. Until the next frame resets the
    /// platform IME, events still arriving for that composition are
    /// dropped.
    Orphaned,
}

pub(crate) struct WindowUiState {
    /// Names the accessibility tree's window node; the adapter's name when
    /// unset.
    pub(super) name: Option<String>,
    pub(super) focus: Option<FocusId>,
    /// Focus as the adapter last settled it, to notice focus that moved to
    /// an element of this window.
    pub(super) focus_seen: Option<FocusId>,
    pub(super) pointer: Option<(f32, f32)>,
    /// Hit entries under the pointer in the routed frame, topmost first:
    /// what hover styles were painted from.
    pub(super) hovered: Vec<HitId>,
    /// Routes input through the last painted frame until the next replaces
    /// it.
    pub(super) router: InputRouter,
    /// Handles the app names this window's elements with.
    pub(super) element_handles: ElementHandles,
    /// A drag out took the pointer, or the app cancelled the drag, during
    /// the last call into the app.
    pub(super) pointer_taken: bool,
    /// Paints the preview of the drag holding the pointer, or of the drag
    /// session over this window.
    pub(super) drag_preview: DragPreviewLayer,
    pub(super) accessibility: AccessibilityFrame,
    pub(super) announcer: Announcer,
    /// Text fields of the last frame, for pointer selection and IME.
    pub(super) text_areas: Vec<TextInputHitArea>,
    pub(super) text_pointer: TextPointer,
    /// The focused text field as of the last input, to cancel its
    /// composition once focus leaves it.
    pub(super) edit_focus: Option<FocusId>,
    /// IME state last sent to the window.
    pub(super) ime_allowed: bool,
    pub(super) ime_area: Option<Rect>,
    /// The platform IME's composition and the element it belongs to.
    pub(super) composition: Composition,
    /// Scale factor of the last painted frame, for accessibility bounds.
    pub(super) scale_factor: f32,
    pub(super) animations: AnimationTable,
    /// Cached subtrees and the layout engine, reused every frame.
    pub(super) element_cache: ElementCache,
    /// Buffers of the frame before last, reused by the next frame: the
    /// scene the runner handed back, the input frame routing let go of,
    /// the text input areas, the IME targets, and the accessibility frame.
    pub(super) spare_scene: Scene,
    pub(super) spare_accessibility: AccessibilityFrame,
    pub(super) spare_input: InputFrame,
    pub(super) spare_text_areas: Vec<TextInputHitArea>,
    pub(super) spare_ime_targets: Vec<ImeTarget>,
    /// Last frame's scrollbar track buffer, reused by the next frame.
    pub(super) spare_tooltip_regions: Vec<TooltipRegion>,
    #[cfg(feature = "devtools")]
    pub(super) devtools: quark_ui::inspector::Devtools,
}

impl WindowUiState {
    pub(super) fn new() -> Self {
        Self {
            name: None,
            focus: None,
            focus_seen: None,
            pointer: None,
            hovered: Vec::new(),
            router: InputRouter::default(),
            element_handles: ElementHandles::default(),
            pointer_taken: false,
            drag_preview: DragPreviewLayer::default(),
            accessibility: AccessibilityFrame::default(),
            announcer: Announcer::default(),
            text_areas: Vec::new(),
            text_pointer: TextPointer::default(),
            edit_focus: None,
            ime_allowed: false,
            ime_area: None,
            composition: Composition::None,
            scale_factor: 1.0,
            animations: AnimationTable::new(),
            element_cache: ElementCache::new(),
            spare_scene: Scene::default(),
            spare_accessibility: AccessibilityFrame::default(),
            spare_input: Default::default(),
            spare_text_areas: Vec::new(),
            spare_ime_targets: Vec::new(),
            spare_tooltip_regions: Vec::new(),
            #[cfg(feature = "devtools")]
            devtools: quark_ui::inspector::Devtools::from_env(),
        }
    }

    /// The focused text field, if focus is on one painted last frame.
    pub(super) fn focused_field(&self) -> Option<FocusId> {
        self.focus
            .filter(|focus| self.text_areas.iter().any(|a| a.focus_target == *focus))
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
