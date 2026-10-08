//! Design system (stream B): tokens, the theme adapter, shared style
//! recipes, and live theme reloading. Every surface takes geometry and
//! colors from here rather than spelling values inline.

pub mod recipes;
pub mod reload;
pub mod theme;
pub mod tokens;

pub use theme::{Appearance, dark, light, themes, themes_for};
