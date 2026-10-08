//! One text layout shared by measurement, hit-testing, selection, and
//! painting.
//!
//! [`TextSystem`] wraps cosmic-text's font system with Quark's bundled and
//! system fonts ([`fonts`]). A [`TextLayout`] is shaped once per frame and
//! shared through the [`LayoutCache`], so the size layout measures is the
//! layout paint draws and pointer hits map onto. Positions are
//! [`TextOffset`]s, byte offsets kept on grapheme boundaries.
//!
//! The `emoji-font` and `cjk-font` features (on by default) bundle Noto
//! Color Emoji (10.7 MB) and a 3.7 MB Noto Sans CJK subset as fallbacks.
// Byte slicing of strings lives in `offset`, which snaps every index
// onto a grapheme boundary first.
#![deny(clippy::string_slice)]

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

#[cfg(test)]
mod alloc_budget;
mod cache;
mod epoch;
pub mod fonts;
mod layout;
pub mod offset;
mod row;
mod system;

pub use cache::{LayoutCache, LayoutCacheLimits, LayoutCacheMemory, LayoutCacheStats, LayoutKey};
pub use cosmic_text;
pub use epoch::{FontEpoch, TextSystemId};
pub use fonts::{BundledFallback, FontRole, FontSettings, MONO_FAMILY, UI_FAMILY};
pub use layout::{
    Caret, DEFAULT_LINE_HEIGHT_FACTOR, GlyphColumns, GlyphRun, IntegrityError, LineInfo, TextError,
    TextLayout, TextParams, TextQuery, TextSpan, TextStyle,
};
pub use offset::{TextOffset, ToTextOffset};
pub use row::{RowHeights, RowMeasure};
pub use system::{TextSystem, TextSystemRecipe};
