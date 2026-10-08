use super::*;

/// The app's one text system and layout cache. Elements shape with them
/// during a frame, and the renderer rasterizes those layouts with the same
/// system, since glyph keys refer to its font database.
pub struct AppText {
    pub system: TextSystem,
    pub layouts: LayoutCache,
}

impl AppText {
    /// Loads vendored and system fonts; slow, so the runner builds one.
    pub(super) fn new(fonts: &FontSettings) -> Self {
        Self {
            system: TextSystem::with_settings(fonts),
            layouts: LayoutCache::default(),
        }
    }

    /// Starts a frame of the window with `scope`, before it shapes text.
    /// Idle time counts in each window's own frames, so a busy window
    /// doesn't evict layouts an idle one still shows.
    pub(super) fn begin_frame(&mut self, scope: u64) {
        self.layouts.begin_frame_for(scope);
    }

    /// Maintenance once a frame is drawn. Every runner calls this, so tests
    /// see the cache, and the storage eviction frees for reuse, as the app
    /// does.
    pub(super) fn end_frame(&mut self) {
        // Trimming walks the whole cache, and entries live for many frames
        // anyway, so a periodic sweep evicts the same entries for less.
        if self.layouts.frame().is_multiple_of(TRIM_LAYOUTS_EVERY) {
            self.layouts.trim();
        }
    }
}

/// Frames between sweeps of idle text layouts out of the cache.
const TRIM_LAYOUTS_EVERY: u64 = 32;
