//! One window's share of the UI adapter: everything input routing, focus,
//! IME, accessibility, and painting remember between that window's frames.
//! The app, its messages, signals, theme, and key bindings are the
//! adapter's and shared by every window.

use std::time::Duration;

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
    /// Whether focus shows its ring (`ElementContext::focus_visible`):
    /// the last input that could move focus was a key or assistive tech,
    /// not a pointer press. App code moving focus keeps it as is, as
    /// programmatic focus in browsers follows the last input.
    pub(super) focus_visible: bool,
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
    /// The last press on window drag chrome, to tell a double-click.
    pub(super) title_press: TitlePress,
    /// The material regions last handed to the window.
    pub(super) material_regions: Vec<crate::platform::material::MaterialRect>,
    #[cfg(feature = "devtools")]
    pub(super) devtools: quark_ui::inspector::Devtools,
}

/// Presses on app-drawn title chrome, told apart into drags and the
/// double-clicks that zoom or minimize the window.
#[derive(Debug, Default)]
pub(super) struct TitlePress {
    last: Option<(Duration, (f32, f32))>,
    /// A press held on the chrome whose window move waits for the pointer
    /// to move ([`DEFER_CHROME_DRAG`]).
    held: Option<(f32, f32)>,
}

/// Whether a press on title chrome moves the window only once the pointer
/// moves. X11 window managers grab the pointer for a move as soon as asked
/// and, when the release beat the request there, keep moving until the
/// next one, swallowing a double-click's second press; GTK waits for the
/// drag threshold on Wayland and X11 alike. AppKit and Windows want the
/// press itself.
pub(super) const DEFER_CHROME_DRAG: bool = cfg!(target_os = "linux");

impl TitlePress {
    /// The longest gap between a double-click's presses: the default on
    /// macOS, Windows, and GNOME.
    const INTERVAL: Duration = Duration::from_millis(500);
    /// How far apart a double-click's presses may be, in points.
    const SLOP: f32 = 4.0;

    /// A press at `at` and time `now`: whether it completes a double-click.
    /// The press after a double-click starts over.
    pub(super) fn press(&mut self, now: Duration, at: (f32, f32)) -> bool {
        let double = self.last.is_some_and(|(then, (x, y))| {
            now.saturating_sub(then) <= Self::INTERVAL
                && (x - at.0).abs() <= Self::SLOP
                && (y - at.1).abs() <= Self::SLOP
        });
        self.last = (!double).then_some((now, at));
        double
    }

    /// A press elsewhere: the next press on the chrome is a first one.
    pub(super) fn reset(&mut self) {
        self.last = None;
        self.held = None;
    }

    /// Wait for the pointer to move before moving the window.
    pub(super) fn hold(&mut self, at: (f32, f32)) {
        self.held = Some(at);
    }

    /// The pointer moved to `at`: whether a held press has now moved far
    /// enough to be a window move.
    pub(super) fn moved(&mut self, at: (f32, f32)) -> bool {
        let far = self
            .held
            .is_some_and(|(x, y)| (x - at.0).abs() > Self::SLOP || (y - at.1).abs() > Self::SLOP);
        if far {
            self.held = None;
        }
        far
    }

    /// The button was released.
    pub(super) fn release(&mut self) {
        self.held = None;
    }
}

impl WindowUiState {
    pub(super) fn new() -> Self {
        Self {
            name: None,
            focus: None,
            focus_visible: true,
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
            title_press: TitlePress::default(),
            material_regions: Vec::new(),
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
