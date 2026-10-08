use cosmic_text::{FontSystem, fontdb};

use crate::fonts::{
    FontSettings, QuarkFallback, configure_generic_families, emoji_family, vendored_font_sources,
};
use crate::layout::{LayoutScratch, SyntheticItalic, TextError, TextLayout, TextParams};

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
    /// The color emoji family emoji clusters ask for first.
    emoji_family: Option<&'static str>,
    /// Built by [`Self::vendored_only`]; another thread builds its twin the
    /// same way.
    vendored_only: bool,
    scratch: LayoutScratch,
}

/// How a [`TextSystem`] was built: enough for another thread to build one
/// that shapes identically, since a `TextSystem` cannot be shared across
/// threads. The vendored fonts are static bytes, so the copy costs only the
/// database scan (plus the system fonts when the original loaded them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSystemRecipe {
    settings: FontSettings,
    vendored_only: bool,
}

impl TextSystemRecipe {
    pub fn build(&self) -> TextSystem {
        if self.vendored_only {
            TextSystem::vendored_only(&self.settings)
        } else {
            TextSystem::with_settings(&self.settings)
        }
    }
}

impl TextSystem {
    /// Loads system fonts as well as the vendored ones; slow (up to a second in
    /// release), so build one and share it.
    pub fn new() -> Self {
        Self::with_settings(&FontSettings::default())
    }

    pub fn with_settings(settings: &FontSettings) -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        for source in vendored_font_sources() {
            db.load_font_source(source);
        }
        let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());
        let font_system = font_system(locale, db, settings);
        Self::from_parts(font_system, settings, false)
    }

    /// Loads only the vendored fonts, with a fixed locale, so shaping does
    /// not depend on the machine. Characters the vendored fonts lack shape as
    /// `.notdef`. Tests and fuzzing use this.
    pub fn vendored_only(settings: &FontSettings) -> Self {
        let mut db = fontdb::Database::new();
        for source in vendored_font_sources() {
            db.load_font_source(source);
        }
        let font_system = font_system("en-US".to_owned(), db, settings);
        Self::from_parts(font_system, settings, true)
    }

    fn from_parts(font_system: FontSystem, settings: &FontSettings, vendored_only: bool) -> Self {
        Self {
            synthetic_italic: SyntheticItalic::new(&font_system),
            emoji_family: emoji_family(font_system.db(), settings.bundled_fallback),
            font_system,
            settings: settings.normalized(),
            generation: 0,
            vendored_only,
            scratch: LayoutScratch::default(),
        }
    }

    /// How to build a system that shapes like this one, on another thread.
    /// Changes made through [`Self::font_system_mut`] are not part of it.
    pub fn recipe(&self) -> TextSystemRecipe {
        TextSystemRecipe {
            settings: self.settings.clone(),
            vendored_only: self.vendored_only,
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
        if settings.bundled_fallback != self.settings.bundled_fallback {
            // cosmic-text fixes the fallback chain at construction; the
            // rebuild reuses the font database, so nothing is rescanned.
            let empty = FontSystem::new_with_locale_and_db(String::new(), fontdb::Database::new());
            let (locale, db) = std::mem::replace(&mut self.font_system, empty).into_locale_and_db();
            self.font_system = font_system(locale, db, &settings);
        } else {
            configure_generic_families(self.font_system.db_mut(), &settings);
        }
        self.synthetic_italic = SyntheticItalic::new(&self.font_system);
        self.emoji_family = emoji_family(self.font_system.db(), settings.bundled_fallback);
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
        TextLayout::build(
            &mut self.font_system,
            &mut self.scratch,
            params,
            self.synthetic_italic,
            self.emoji_family,
            self.settings.ligatures,
        )
    }

    /// Shapes and lays out `layout`'s inputs again in its own storage; see
    /// [`TextLayout::rebuild`].
    pub(crate) fn rebuild(&mut self, layout: &mut TextLayout) {
        profile_scope!("text_shape");
        layout.rebuild(
            &mut self.font_system,
            &mut self.scratch,
            self.synthetic_italic,
            self.emoji_family,
            self.settings.ligatures,
        );
    }
}

fn font_system(locale: String, mut db: fontdb::Database, settings: &FontSettings) -> FontSystem {
    configure_generic_families(&mut db, settings);
    let fallback = QuarkFallback::new(settings.bundled_fallback, &locale);
    fallback.fill_weights(&mut db);
    FontSystem::new_with_locale_and_db_and_fallback(locale, db, fallback)
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
