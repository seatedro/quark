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
pub mod child;
pub mod combobox;
pub mod context_menu;
pub mod diff_view;
pub mod dock;
pub mod dropdown;
pub mod hover_card;
pub mod kbd;
mod list_nav;
pub mod menu_bar;
pub mod modal;
pub mod palette;
pub mod pane_tree;
pub mod picker;
pub mod popover;
pub mod progress;
pub mod radio;
pub mod search_field;
pub mod segmented;
pub mod select;
pub mod skeleton;
pub mod slider;
pub mod split;
pub mod table;
pub mod tabs;
pub mod toast;
pub mod toolbar;
pub mod tooltip;
pub mod tree;

pub use avatar::*;
pub use badge::*;
pub use behavior::*;
pub use breadcrumb::*;
pub use button::{Button, ButtonBuilder, ButtonSize, ButtonStyle};
pub use checkbox::*;
pub use child::Child;
pub use combobox::*;
pub use context_menu::*;
pub use diff_view::{
    CopySide, DiffEvent, DiffKey, DiffOutcome, DiffStyle, DiffViewState, diff_view,
};
pub use dock::{
    Dock, DockEvent, DockIntegrityError, DockLayout, DockRegion, DockSnapshot, DockSplit,
    DockState, PaneDividerEvent, PanelId, TabPolicy,
};
pub use dropdown::*;
pub use hover_card::*;
pub use kbd::*;
pub use list_nav::TypeAhead;
pub use menu_bar::{MenuBar, MenuBarMenu};
pub use modal::{Modal, ModalAlign};
pub use palette::{
    CommandPalette, FuzzyMatcher, PALETTE_INPUT, PaletteEvent, PaletteItem, PaletteOutcome,
    PaletteProvider, binding_label,
};
pub use pane_tree::{DropZone, PaneDrop, PaneId, PaneNode, PaneSplit, TabGroup};
pub use picker::{PickerItem, PickerLabelStyle, picker_list};
pub use popover::*;
pub use progress::*;
pub use radio::*;
pub use search_field::*;
pub use segmented::{SegmentedControl, SegmentedItem, segmented_focus_id};
pub use select::*;
pub use skeleton::{skeleton, skeleton_lines};
pub use slider::*;
pub use split::{
    Axis, Pane, PaneSizes, Split, SplitEvent, SplitIntegrityError, SplitSnapshot, SplitState,
};
pub use table::{
    CellStyle, TableData, TableEvent, TableIntegrityError, TableKey, TableOutcome, TableState,
    table_view,
};
pub use tabs::*;
pub use toast::{
    Toast, ToastAction, ToastKind, ToastLayout, ToastQueue, ToastStack, animate_toast_fan,
    animate_toast_in, animate_toast_out, animate_toast_progress, retire_toast,
};
pub use toolbar::Toolbar;
pub use tooltip::*;
pub use tree::{
    CollectionEnv, DropPosition, DropTarget, NodeId, SelectMods, SelectionMode, TreeEvent,
    TreeIntegrityError, TreeKey, TreeNav, TreeOutcome, TreeState, tree_view,
};
