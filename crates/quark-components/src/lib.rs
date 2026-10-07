//! Reusable components built on `quark-ui`: buttons, inputs, menus,
//! overlays, and feedback widgets.
//!
//! Components emit app actions as [`quark_ui::Action`] values supplied by the
//! caller; none of them know the hosting app's action or state types.

pub mod avatar;
pub mod badge;
pub mod behavior;
pub mod breadcrumb;
pub mod button;
pub mod checkbox;
pub mod context_menu;
pub mod dock;
pub mod dropdown;
pub mod hover_card;
pub mod kbd;
pub mod modal;
pub mod palette;
pub mod picker;
pub mod popover;
pub mod progress;
pub mod search_field;
pub mod segmented;
pub mod sidebar_skeleton;
pub mod split;
pub mod stat_summary;
pub mod tabs;
pub mod toast;
pub mod toolbar;
pub mod tooltip;

pub use avatar::*;
pub use badge::*;
pub use behavior::*;
pub use breadcrumb::*;
pub use button::{Button, ButtonSize, ButtonStyle};
pub use checkbox::*;
pub use context_menu::*;
pub use dock::{
    Dock, DockEvent, DockLayout, DockRegion, DockSnapshot, DockSplit, DockState, PanelId,
    RegionSnapshot,
};
pub use dropdown::*;
pub use hover_card::*;
pub use kbd::*;
pub use modal::{Modal, ModalAlign};
pub use palette::{
    CommandPalette, FuzzyMatcher, PALETTE_INPUT, PaletteEvent, PaletteItem, PaletteOutcome,
    PaletteProvider, binding_label,
};
pub use picker::{PickerItem, PickerLabelStyle, picker_list};
pub use popover::*;
pub use progress::*;
pub use search_field::*;
pub use segmented::{SegmentedControl, SegmentedItem};
pub use sidebar_skeleton::sidebar_skeleton;
pub use split::{
    Axis, Pane, PaneSizes, Split, SplitEvent, SplitIntegrityError, SplitSnapshot, SplitState,
};
pub use stat_summary::*;
pub use tabs::*;
pub use toast::{
    Toast, ToastAction, ToastKind, ToastLayout, ToastQueue, ToastStack, animate_toast_fan,
    animate_toast_in, animate_toast_out, animate_toast_progress, retire_toast,
};
pub use toolbar::Toolbar;
pub use tooltip::*;
