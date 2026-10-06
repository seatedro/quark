//! Element tree, layout, styling, theming, and text input for Quark.
//!
//! `harness` and `window_chrome` are carried over from diffy with history and
//! join the tree as their diffy-specific imports are removed.

pub mod accessibility;
pub mod action;
pub mod animation;
pub mod design;
pub mod element;
pub mod hud;
pub mod icons;
pub mod palette;
pub mod style;
pub mod text_input;
pub mod theme;
pub mod virtual_list;

pub use action::{Action, ActionPayload, FocusId};
