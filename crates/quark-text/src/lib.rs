//! One text layout shared by measurement, hit-testing, selection, and
//! painting.
//!
//! Integration note (follow-up task): once quark-render and quark-ui use this
//! crate, delete from quark-render `fonts.rs`'s font bytes, `new_font_system*`,
//! `configure_font_system*`, and `configure_generic_families` (keep or move
//! the family catalog), move `assets/fonts` into this crate, drop the
//! renderer-owned `FontSystem` and its `CachedTextBuffer` cache in `text.rs`,
//! and remove quark-ui's `measure_text_width` cache.

mod cache;
pub mod fonts;
mod layout;
mod row;
mod system;

pub use cache::{LayoutCache, LayoutKey};
pub use cosmic_text;
pub use fonts::{FontRole, FontSettings, MONO_FAMILY, UI_FAMILY};
pub use layout::{
    Caret, DEFAULT_LINE_HEIGHT_FACTOR, GlyphColumns, GlyphRun, LineInfo, TextError, TextLayout,
    TextParams, TextSpan, TextStyle,
};
pub use row::{RowHeights, RowMeasure};
pub use system::TextSystem;
