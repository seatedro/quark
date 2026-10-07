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
}
