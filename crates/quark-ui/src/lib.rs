//! Element tree, layout, styling, theming, and text input for Quark.
//!
//! Modules listed here compile standalone. `element`, `accessibility`,
//! `design`, `theme`, `style`, `harness`, `window_chrome`, and `text_input`
//! are carried over from diffy with history and join the tree as their
//! diffy-specific imports are removed.

pub mod action;
pub mod animation;
pub mod hud;
pub mod icons;
pub mod text_input;
pub mod virtual_list;

pub use action::{Action, ActionPayload, FocusId};
