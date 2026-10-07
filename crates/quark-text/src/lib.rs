//! One text layout shared by measurement, hit-testing, selection, and
//! painting.

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
