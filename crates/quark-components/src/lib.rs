//! Reusable components built on `quark-ui`: buttons, inputs, menus,
//! overlays, and feedback widgets.
//!
//! Components emit app actions as [`quark_ui::Action`] values supplied by the
//! caller; none of them know the hosting app's action or state types.
//!
//! `compare_progress.rs`, `sidebar.rs`, `file_tree.rs`, and `file_icon.rs`
//! are diffy product views carried over with history. They still read
//! diffy state and stay out of the module tree.

pub mod avatar;
pub mod badge;
pub mod behavior;
pub mod breadcrumb;
pub mod button;
pub mod checkbox;
pub mod context_menu;
pub mod dropdown;
pub mod kbd;
pub mod modal;
pub mod picker;
pub mod popover;
pub mod progress;
pub mod search_field;
pub mod segmented;
pub mod sidebar_skeleton;
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
pub use dropdown::*;
pub use kbd::*;
pub use modal::{Modal, ModalAlign};
pub use picker::{PickerItem, PickerLabelStyle, picker_list};
pub use popover::*;
pub use progress::*;
pub use search_field::*;
pub use segmented::{SegmentedControl, SegmentedItem};
pub use sidebar_skeleton::sidebar_skeleton;
pub use stat_summary::*;
pub use tabs::*;
pub use toast::{Toast, ToastKind, ToastLayout, ToastStack};
pub use toolbar::Toolbar;
pub use tooltip::*;
