//! Design system (stream B): tokens, the theme adapter, and shared style
//! recipes. Every surface takes geometry and colors from here rather than
//! spelling values inline.

pub mod reload;
pub mod theme;
pub mod tokens;

pub use theme::{dark, light, themes_for};
