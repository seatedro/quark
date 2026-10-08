//! Which fonts a layout was shaped with.

use std::sync::atomic::{AtomicU64, Ordering};

/// Identifies one [`crate::TextSystem`] for as long as the process runs.
/// Drawn from a counter, so a system built where a dropped one lived (or
/// moved into its place) never passes for it the way an address could.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TextSystemId(u64);

impl TextSystemId {
    pub(crate) fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// The fonts a layout was shaped with: the text system and its generation,
/// which every font change advances. Anything derived from shaping (cached
/// layouts, recorded geometry, cell metrics) stays valid while the epoch it
/// was computed under is unchanged. Each system's generations start at
/// zero, so the generation alone cannot tell two systems apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FontEpoch {
    pub system: TextSystemId,
    pub generation: u64,
}
