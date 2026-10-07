use cosmic_text::{FontSystem, fontdb};

use crate::fonts::{FontSettings, configure_generic_families, vendored_font_sources};
use crate::layout::{TextError, TextLayout, TextParams};

/// Owns the cosmic-text [`FontSystem`]: vendored fonts, system fonts (for
/// fallback), and the generic sans/mono family mapping.
pub struct TextSystem {
    font_system: FontSystem,
    settings: FontSettings,
    generation: u64,
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
        Self {
            font_system,
            settings: settings.normalized(),
            generation: 0,
        }
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
        Self {
            font_system: FontSystem::new_with_locale_and_db("en-US".to_owned(), db),
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
    /// database through this does not bump [`Self::generation`].
    pub fn font_system_mut(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// Shapes and lays out text without caching. Prefer
    /// [`crate::LayoutCache::layout`] for per-frame use.
    pub fn layout(&mut self, params: &TextParams) -> Result<TextLayout, TextError> {
        TextLayout::build(&mut self.font_system, params)
    }
}

impl Default for TextSystem {
    fn default() -> Self {
        Self::new()
    }
}
