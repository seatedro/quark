use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

/// Horizontal scroll offset of a single-line field, in pixels.
///
/// The model owns it and the painting element reads and updates it each
/// frame, since only the element knows the text width. Clones share the
/// offset.
#[derive(Debug, Clone, Default)]
pub struct HorizontalScroll(Arc<AtomicU32>);

impl HorizontalScroll {
    pub fn get(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub fn set(&self, offset: f32) {
        let offset = if offset.is_finite() { offset } else { 0.0 };
        self.0.store(offset.to_bits(), Ordering::Relaxed);
    }
}

/// Scale factor of the frame an [`super::Editor`] was last painted in.
/// The editor's element writes it during paint and the editor reads it on
/// the next flush, so layouts follow the window's scale without the app
/// passing it along. Clones share the value.
#[derive(Debug, Clone, Default)]
pub(crate) struct FrameScale(Arc<AtomicU32>);

impl FrameScale {
    /// `None` until a frame has painted.
    pub(crate) fn get(&self) -> Option<f32> {
        let bits = self.0.load(Ordering::Relaxed);
        (bits != 0).then(|| f32::from_bits(bits))
    }

    pub(crate) fn set(&self, scale: f32) {
        if scale.is_finite() && scale > 0.0 {
            self.0.store(scale.to_bits(), Ordering::Relaxed);
        }
    }
}

/// The smallest change to `scroll` that keeps a caret of `caret_width` at
/// `caret_x` inside a `view_width` window over `content_width` of text.
pub fn reveal_offset(
    scroll: f32,
    caret_x: f32,
    caret_width: f32,
    content_width: f32,
    view_width: f32,
) -> f32 {
    let max = (content_width.max(caret_x) + caret_width - view_width).max(0.0);
    let mut scroll = scroll.clamp(0.0, max);
    if caret_x < scroll {
        scroll = caret_x;
    } else if caret_x + caret_width > scroll + view_width {
        scroll = caret_x + caret_width - view_width;
    }
    scroll.clamp(0.0, max)
}

/// Presses closer together than this (ms) chain into double/triple clicks.
pub const MULTI_CLICK_MS: u64 = 500;
/// ...and no further apart than this (px).
const MULTI_CLICK_SLOP: f32 = 4.0;

/// Counts consecutive presses: 1 places the caret, 2 selects a word, 3 a
/// line, then it cycles back to 1.
#[derive(Debug, Clone, Copy, Default)]
pub struct ClickCounter {
    last: Option<(f32, f32, u64)>,
    count: u8,
}

impl ClickCounter {
    pub fn press(&mut self, x: f32, y: f32, now_ms: u64) -> u8 {
        let chained = self.last.is_some_and(|(lx, ly, at)| {
            now_ms.saturating_sub(at) <= MULTI_CLICK_MS
                && (x - lx).abs() <= MULTI_CLICK_SLOP
                && (y - ly).abs() <= MULTI_CLICK_SLOP
        });
        self.count = if chained { self.count % 3 + 1 } else { 1 };
        self.last = Some((x, y, now_ms));
        self.count
    }
}

/// Caret on and off phase length.
pub const CARET_BLINK_MS: u64 = 530;

/// Whether the caret is shown at `clock_ms`, and when it next toggles. It
/// stays on for the first phase after it moved.
pub fn caret_blink(clock_ms: u64, moved_at_ms: u64) -> (bool, u64) {
    let phase = clock_ms.saturating_sub(moved_at_ms) / CARET_BLINK_MS;
    (
        phase.is_multiple_of(2),
        moved_at_ms + (phase + 1) * CARET_BLINK_MS,
    )
}
