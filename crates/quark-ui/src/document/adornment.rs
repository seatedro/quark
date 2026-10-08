//! Row adornments: app elements that sit between a row's blocks.
//!
//! The document draws blocks; a tool card's disclosure header, a status
//! line, or an action bar under a message is app UI. A [`RowAdornment`]
//! reserves a band of its row, before a block or at either end, and the
//! app's builder fills it with an ordinary element tree (buttons, icons,
//! text) on the UI thread when the row is rebuilt.
//!
//! The document never measures an adornment: the app states its height,
//! and a stated [`revision`](RowAdornment::revision) stands for everything
//! the builder reads. Rows are cached on those, so change the revision
//! whenever the builder would build something else. Background measurement
//! sees only the slots and heights, never the builders, which stay on the
//! UI thread.

use std::hash::{Hash, Hasher};
use std::rc::Rc;

use quark::selection::BlockKey;

use crate::element::AnyElement;
use crate::theme::Theme;
use crate::virtual_list::RowKey;

/// Identifies an adornment among its row's adornments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct AdornmentKey(pub u64);

/// Where an adornment sits in its row's flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AdornmentSlot {
    /// Above the row's first block, below the header band.
    Start,
    /// Right above this block of the row. When the row does not hold the
    /// block, the adornment goes to the end.
    Before(BlockKey),
    /// Below the row's last block.
    End,
}

/// How an adornment appears to assistive technology.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum AdornmentAccessibility {
    /// Its text and controls are published, also in a row whose
    /// [`RowChrome::label`](super::RowChrome::label) names it.
    #[default]
    Exposed,
    /// Its text is hidden because the row's label already says it (an
    /// author line); controls that carry their own accessible label, such
    /// as buttons and toggles, stay reachable.
    ControlsOnly,
}

/// What a builder knows about the band it fills.
pub struct AdornmentCx<'a> {
    pub row: RowKey,
    pub key: AdornmentKey,
    /// Size of the band: the row's block column wide, the adornment's
    /// height tall.
    pub width: f32,
    pub height: f32,
    pub theme: &'a Theme,
}

type Build = Rc<dyn Fn(&AdornmentCx<'_>) -> AnyElement>;

/// An app element in a band of its row. Two adornments are equal when
/// everything but the builder is; the revision stands for the builder.
#[derive(Clone)]
pub struct RowAdornment {
    pub key: AdornmentKey,
    pub slot: AdornmentSlot,
    /// Height of the band in logical points, measured by the app at the
    /// document's width. It counts toward the row's height; zero takes no
    /// space and builds nothing.
    pub height: f32,
    /// Changes whenever the builder would build something else (a state,
    /// a label, a count it shows). The row rebuilds when it does.
    pub revision: u64,
    pub accessibility: AdornmentAccessibility,
    build: Build,
}

impl RowAdornment {
    pub fn new(
        key: AdornmentKey,
        slot: AdornmentSlot,
        height: f32,
        revision: u64,
        build: impl Fn(&AdornmentCx<'_>) -> AnyElement + 'static,
    ) -> Self {
        Self {
            key,
            slot,
            height,
            revision,
            accessibility: AdornmentAccessibility::default(),
            build: Rc::new(build),
        }
    }

    pub fn accessibility(mut self, policy: AdornmentAccessibility) -> Self {
        self.accessibility = policy;
        self
    }

    /// The band's element. Called on the UI thread when the row rebuilds.
    pub(super) fn build(&self, cx: &AdornmentCx<'_>) -> AnyElement {
        (self.build)(cx)
    }
}

impl std::fmt::Debug for RowAdornment {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowAdornment")
            .field("key", &self.key)
            .field("slot", &self.slot)
            .field("height", &self.height)
            .field("revision", &self.revision)
            .field("accessibility", &self.accessibility)
            .finish_non_exhaustive()
    }
}

impl PartialEq for RowAdornment {
    fn eq(&self, other: &Self) -> bool {
        (self.key, self.slot, self.height.to_bits(), self.revision)
            == (
                other.key,
                other.slot,
                other.height.to_bits(),
                other.revision,
            )
            && self.accessibility == other.accessibility
    }
}

impl Hash for RowAdornment {
    fn hash<H: Hasher>(&self, state: &mut H) {
        (self.key, self.slot, self.height.to_bits(), self.revision).hash(state);
        self.accessibility.hash(state);
    }
}

/// What a row's height needs from an adornment, for the UI thread's
/// [`RowAdornment`]s and the background thread's copies alike.
pub(super) trait AdornmentShape {
    fn slot(&self) -> AdornmentSlot;
    fn height(&self) -> f32;
}

impl AdornmentShape for RowAdornment {
    fn slot(&self) -> AdornmentSlot {
        self.slot
    }

    fn height(&self) -> f32 {
        self.height
    }
}

impl AdornmentShape for (AdornmentSlot, f32) {
    fn slot(&self) -> AdornmentSlot {
        self.0
    }

    fn height(&self) -> f32 {
        self.1
    }
}
