//! One text layout shared by measurement, hit-testing, selection, and
//! painting.

/// A profiler scope plus a `tracing` span for the rest of the block, both
/// compiled only with the `profile` feature.
#[allow(unused_macros)]
macro_rules! profile_scope {
    ($name:literal) => {
        #[cfg(feature = "profile")]
        profiling::scope!($name);
        #[cfg(feature = "profile")]
        let _profile_span = tracing::trace_span!($name).entered();
    };
}

mod cache;
pub mod fonts;
mod layout;
mod row;
mod system;

pub use cache::{LayoutCache, LayoutCacheStats, LayoutKey};
pub use cosmic_text;
pub use fonts::{FontRole, FontSettings, MONO_FAMILY, UI_FAMILY};
pub use layout::{
    Caret, DEFAULT_LINE_HEIGHT_FACTOR, GlyphColumns, GlyphRun, IntegrityError, LineInfo, TextError,
    TextLayout, TextParams, TextSpan, TextStyle,
};
pub use row::{RowHeights, RowMeasure};
pub use system::TextSystem;
