//! Element tree, layout, styling, theming, and text input for Quark.

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

pub mod accessibility;
pub mod action;
pub mod animation;
pub mod design;
pub mod element;
pub mod hud;
pub mod icons;
#[cfg(feature = "devtools")]
pub mod inspector;
pub mod markdown;
pub mod palette;
pub mod style;
pub mod text_input;
pub mod theme;
pub mod transcript;
pub mod virtual_list;

#[cfg(any(test, feature = "test-alloc"))]
#[doc(hidden)]
pub mod test_alloc;

#[cfg(test)]
#[global_allocator]
static ALLOCATOR: test_alloc::Counting = test_alloc::Counting;

pub use action::{Action, ActionPayload, FocusId};
/// Localized messages and formats; see [`quark_i18n`].
pub use quark_i18n as i18n;
