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

#[cfg(test)]
mod test_alloc;

pub use action::{Action, ActionPayload, FocusId};
