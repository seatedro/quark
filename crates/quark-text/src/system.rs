use cosmic_text::{FontSystem, fontdb};

use crate::fonts::{FontSettings, configure_generic_families, vendored_font_sources};
use crate::layout::{SyntheticItalic, TextError, TextLayout, TextParams};

/// Owns the cosmic-text [`FontSystem`]: vendored fonts, system fonts (for
/// fallback), and the generic sans/mono family mapping.
pub struct TextSystem {
    font_system: FontSystem,
    settings: FontSettings,
    generation: u64,
    /// Which generic families need slanted glyphs for italic spans. Finding
    /// out scans every face, so it runs when the fonts change rather than on
    /// every layout.
    synthetic_italic: SyntheticItalic,
}

impl TextSystem {
    /// Loads system fonts as well as the vendored ones; slow (up to a second in
    /// release), so build one and share it.
    pub fn new() -> Self {
        Self::with_settings(&FontSettings::default())
    }

    pub fn with_settings(settings: &FontSettings) -> Self {
        let mut font_system = FontSystem::new_with_fonts(vendored_font_sources());
        configure_generic_families(font_system.db_mut(), settings);
        Self::from_parts(font_system, settings)
    }

    /// Loads only the vendored fonts, with a fixed locale, so shaping does
    /// not depend on the machine. Characters the vendored fonts lack shape as
    /// `.notdef`. Tests and fuzzing use this.
    pub fn vendored_only(settings: &FontSettings) -> Self {
        let mut db = fontdb::Database::new();
        for source in vendored_font_sources() {
            db.load_font_source(source);
        }
        configure_generic_families(&mut db, settings);
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_owned(), db);
        Self::from_parts(font_system, settings)
    }

    fn from_parts(font_system: FontSystem, settings: &FontSettings) -> Self {
        Self {
            synthetic_italic: SyntheticItalic::new(&font_system),
            font_system,
            settings: settings.normalized(),
            generation: 0,
        }
    }

    pub fn font_settings(&self) -> &FontSettings {
        &self.settings
    }

    /// Changes the generic families. Bumps [`Self::generation`] so layout
    /// caches drop layouts shaped with the old fonts.
    pub fn set_font_settings(&mut self, settings: &FontSettings) {
        let settings = settings.normalized();
        if settings == self.settings {
            return;
        }
        configure_generic_families(self.font_system.db_mut(), &settings);
        self.synthetic_italic = SyntheticItalic::new(&self.font_system);
        self.settings = settings;
        self.generation = self.generation.wrapping_add(1);
    }

    /// Incremented whenever font configuration changes.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn font_system(&self) -> &FontSystem {
        &self.font_system
    }

    /// Needed by rasterizers (glyphon prepare, SwashCache). Changing the font
    /// database through this does not bump [`Self::generation`] or recheck
    /// which families lack an italic face.
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Shapes and lays out text without caching. Prefer
    /// [`crate::LayoutCache::layout`] for per-frame use.
    pub fn layout(&mut self, params: &TextParams) -> Result<TextLayout, TextError> {
        profile_scope!("text_shape");
        TextLayout::build(&mut self.font_system, params, self.synthetic_italic)
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}

/// One vendored-only system per test binary; building it parses every font.
#[cfg(test)]
pub(crate) fn test_system() -> std::sync::MutexGuard<'static, TextSystem> {
    use std::sync::{Mutex, OnceLock};
    static SYSTEM: OnceLock<Mutex<TextSystem>> = OnceLock::new();
    SYSTEM
        .get_or_init(|| Mutex::new(TextSystem::vendored_only(&FontSettings::default())))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
