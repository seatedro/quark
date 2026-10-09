use std::sync::Arc;

use cosmic_text::{FontSystem, fontdb};

use crate::epoch::{FontEpoch, TextSystemId};
use crate::fonts::{
    FamilyId, FamilyNames, FontRole, FontSettings, FontSnapshot, GenericFonts, QuarkFallback,
    ResolvedFamily, configure_generic_families, emoji_family, vendored_font_sources,
};
use crate::layout::{LayoutScratch, ShapeEnv, SyntheticItalic, TextError, TextLayout, TextParams};

/// Owns the cosmic-text [`FontSystem`]: vendored fonts, system fonts (for
/// fallback), and the generic sans/mono family mapping.
pub struct TextSystem {
    font_system: FontSystem,
    settings: FontSettings,
    id: TextSystemId,
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
    /// Fonts added by [`Self::load_font_data`], in order.
    loaded: Vec<FontData>,
    /// Family names [`Self::family_id`] interned.
    families: FamilyNames,
    /// Where the system UI and monospace names resolve.
    generics: GenericFonts,
    /// What the settings' UI and monospace families resolved to.
    resolved: [ResolvedFamily; 2],
    /// The last [`Self::font_snapshot`], while the fonts are unchanged.
    snapshot: Option<FontSnapshot>,
    /// What [`Self::set_shape_plan_capacity`] set, for a reload to keep.
    shape_plan_capacity: Option<usize>,
    scratch: LayoutScratch,
}

/// A font file's bytes, loaded into a [`TextSystem`]. Recipes compare them
/// by identity: the same bytes loaded twice are two loads.
#[derive(Clone)]
struct FontData(Arc<dyn AsRef<[u8]> + Send + Sync>);

impl PartialEq for FontData {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for FontData {}

impl std::fmt::Debug for FontData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FontData({} bytes)", (*self.0).as_ref().len())
    }
}

/// How a [`TextSystem`] was built: enough for another thread to build one
/// that shapes identically, since a `TextSystem` cannot be shared across
/// threads. The vendored fonts are static bytes, and loaded fonts are
/// shared, so the copy costs only the database scan (plus the system fonts
/// when the original loaded them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextSystemRecipe {
    settings: FontSettings,
    vendored_only: bool,
    loaded: Vec<FontData>,
    families: FamilyNames,
    /// The platform's answers for the generic families, so a twin resolves
    /// them as the original did without asking again.
    generics: GenericFonts,
}

impl TextSystemRecipe {
    pub fn build(&self) -> TextSystem {
        let mut system =
            TextSystem::build(&self.settings, self.generics.clone(), self.vendored_only);
        // Loaded one at a time as the original did, so families resolve
        // and fill in weights the same way.
        for font in &self.loaded {
            system.load_font_data(font.0.clone());
        }
        system.families = self.families.clone();
        system
    }
}

impl TextSystem {
    /// Loads system fonts as well as the vendored ones; slow (up to a second in
    /// release), so build one and share it.
    pub fn new() -> Self {
        Self::with_settings(&FontSettings::default())
    }

    /// [`crate::fonts::SYSTEM_UI`] and [`crate::fonts::UI_MONOSPACE`] in the
    /// settings resolve through the platform's font API.
    pub fn with_settings(settings: &FontSettings) -> Self {
        let locale = system_locale();
        Self::build(settings, GenericFonts::Platform { locale }, false)
    }

    /// Loads only the vendored fonts, with a fixed locale, so shaping does
    /// not depend on the machine. Characters the vendored fonts lack shape as
    /// `.notdef`, and the system UI and monospace names resolve to the
    /// bundled families. Tests and fuzzing use this.
    pub fn vendored_only(settings: &FontSettings) -> Self {
        Self::build(settings, GenericFonts::Bundled, true)
    }

    fn build(settings: &FontSettings, mut generics: GenericFonts, vendored_only: bool) -> Self {
        let mut db = fontdb::Database::new();
        let locale = if vendored_only {
            "en-US".to_owned()
        } else {
            db.load_system_fonts();
            system_locale()
        };
        for source in vendored_font_sources() {
            db.load_font_source(source);
        }
        let (font_system, resolved) = font_system(locale, db, settings, &mut generics);
        Self {
            synthetic_italic: SyntheticItalic::new(&font_system),
            emoji_family: emoji_family(font_system.db(), settings.bundled_fallback),
            font_system,
            settings: settings.normalized(),
            id: TextSystemId::next(),
            generation: 0,
            vendored_only,
            loaded: Vec::new(),
            families: FamilyNames::new(),
            generics,
            resolved,
            snapshot: None,
            shape_plan_capacity: None,
            scratch: LayoutScratch::default(),
        }
    }

    /// A system with `generics` in place of the platform's answers, for
    /// tests of generic resolution.
    #[cfg(test)]
    pub(crate) fn with_generics(settings: &FontSettings, generics: GenericFonts) -> Self {
        Self::build(settings, generics, true)
    }

    /// How to build a system that shapes like this one, on another thread.
    pub fn recipe(&self) -> TextSystemRecipe {
        TextSystemRecipe {
            settings: self.settings.clone(),
            vendored_only: self.vendored_only,
            loaded: self.loaded.clone(),
            families: self.families.clone(),
            generics: self.generics.clone(),
        }
    }

    pub fn font_settings(&self) -> &FontSettings {
        &self.settings
    }

    /// What the settings' family for `role` resolved to: the family text
    /// draws in, and why it is not the one asked for when it is not.
    pub fn resolved_family(&self, role: FontRole) -> &ResolvedFamily {
        match role {
            FontRole::Ui => &self.resolved[0],
            FontRole::Mono => &self.resolved[1],
        }
    }

    /// The id of family `name`, for [`crate::fonts::FontFamily::Named`].
    /// Interns the name on first use; later calls allocate nothing, and
    /// [`Self::recipe`] carries the ids to systems built from it. A family
    /// no face has draws in the text's generic family.
    pub fn family_id(&mut self, name: &str) -> FamilyId {
        self.families.intern(name)
    }

    /// The name of `id`, or `None` when another system interned it.
    pub fn family_name(&self, id: FamilyId) -> Option<&str> {
        self.families.name(id)
    }

    /// The fonts as they are now, for rasterizers on any thread. Copies
    /// the font database's face list (not the face data) once per
    /// [`Self::font_epoch`]; later calls share that copy.
    pub fn font_snapshot(&mut self) -> FontSnapshot {
        let epoch = self.font_epoch();
        match &self.snapshot {
            Some(snapshot) if snapshot.epoch == epoch => snapshot.clone(),
            _ => {
                let snapshot = FontSnapshot {
                    epoch,
                    database: Arc::new(self.font_system.db().clone()),
                };
                self.snapshot = Some(snapshot.clone());
                snapshot
            }
        }
    }

    /// Changes the generic families. Advances [`Self::font_epoch`] so layout
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
            (self.font_system, self.resolved) =
                font_system(locale, db, &settings, &mut self.generics);
        } else {
            self.resolved = configure_generic_families(
                self.font_system.db_mut(),
                &settings,
                &mut self.generics,
            );
        }
        self.settings = settings;
        self.fonts_changed();
    }

    /// Adds the faces in a font file (TrueType, OpenType, or a collection),
    /// such as a font an app ships, for [`FontSettings`] to name or for
    /// fallback. Advances [`Self::font_epoch`], and [`Self::recipe`] carries
    /// the font, so systems built from it on other threads shape with it
    /// too.
    pub fn load_font_data(&mut self, data: Arc<dyn AsRef<[u8]> + Send + Sync>) {
        let db = self.font_system.db_mut();
        db.load_font_source(fontdb::Source::Binary(data.clone()));
        // The settings may name a family only this font has.
        self.resolved = configure_generic_families(db, &self.settings, &mut self.generics);
        self.loaded.push(FontData(data));
        self.fonts_changed();
    }

    /// Does nothing. Text in a family with fewer weights than it asks for
    /// (one variable face, say) used to fall back to another family unless
    /// this registered the family at every weight; matching now takes the
    /// family's face nearest the weight. Kept until callers drop it.
    pub fn fill_family_weights(&mut self, _family: &str) {}

    /// Rederives what depends on the font database, and advances the
    /// generation so everything shaped before is shaped again.
    fn fonts_changed(&mut self) {
        self.synthetic_italic = SyntheticItalic::new(&self.font_system);
        self.emoji_family = emoji_family(self.font_system.db(), self.settings.bundled_fallback);
        self.scratch.text_faces.clear();
        self.snapshot = None;
        self.generation = self.generation.wrapping_add(1);
    }

    /// The fonts layouts from this system are shaped with now. It changes
    /// with every font change, and no two systems share one.
    pub fn font_epoch(&self) -> FontEpoch {
        FontEpoch {
            system: self.id,
            generation: self.generation,
        }
    }

    pub fn font_system(&self) -> &FontSystem {
        &self.font_system
    }

    /// For rasterizers (glyphon's prepare, `SwashCache`), which take the
    /// font system mutably for their own caches. Not for changing fonts:
    /// nothing here notices a change made through it, so caches would keep
    /// layouts shaped with the old fonts. Use [`Self::set_font_settings`]
    /// and [`Self::load_font_data`].
    ///
    /// It stays a plain `&mut FontSystem` because glyphon's `prepare` and
    /// cosmic-text's `SwashCache::get_image` take one, and a `FontSystem`
    /// borrowed mutably can always reach its database: a handle that
    /// rasterizes without changing fonts needs both vendored crates to
    /// accept it.
    pub fn raster_font_system(&mut self) -> &mut FontSystem {
        &mut self.font_system
    }

    /// How many shaping plans the font system keeps. Plans only speed up
    /// shaping, so changing this keeps the font epoch.
    pub fn set_shape_plan_capacity(&mut self, capacity: usize) {
        self.font_system.set_shape_plan_capacity(capacity);
        self.shape_plan_capacity = Some(capacity);
    }

    /// Scans the installed fonts again and asks the platform again for the
    /// system UI and monospace families, for an app the OS told that fonts
    /// were installed, removed, or changed. Keeps loaded fonts, settings,
    /// interned family ids, and the shape plan capacity; becomes a new
    /// [`Self::font_epoch`], so
    /// caches lay everything out again. Slow, like [`Self::new`]; a
    /// [`Self::vendored_only`] system has nothing to rescan.
    pub fn reload_system_fonts(&mut self) {
        if self.vendored_only {
            return;
        }
        let mut fresh = Self::with_settings(&self.settings);
        for font in &self.loaded {
            fresh.load_font_data(font.0.clone());
        }
        fresh.families = std::mem::take(&mut self.families);
        if let Some(capacity) = self.shape_plan_capacity {
            fresh.set_shape_plan_capacity(capacity);
        }
        *self = fresh;
    }

    /// Shapes and lays out text without caching. Prefer
    /// [`crate::LayoutCache::layout`] for per-frame use.
    pub fn layout(&mut self, params: &TextParams) -> Result<TextLayout, TextError> {
        profile_scope!("text_shape");
        let env = ShapeEnv {
            synth: self.synthetic_italic,
            emoji: self.emoji_family,
            ligatures: self.settings.ligatures,
            names: &self.families,
        };
        TextLayout::build(&mut self.font_system, &mut self.scratch, params, &env)
    }

    /// Shapes and lays out `layout`'s inputs again in its own storage; see
    /// [`TextLayout::rebuild`].
    pub(crate) fn rebuild(&mut self, layout: &mut TextLayout) {
        profile_scope!("text_shape");
        let env = ShapeEnv {
            synth: self.synthetic_italic,
            emoji: self.emoji_family,
            ligatures: self.settings.ligatures,
            names: &self.families,
        };
        layout.rebuild(&mut self.font_system, &mut self.scratch, &env);
    }
}

fn system_locale() -> String {
    sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned())
}

fn font_system(
    locale: String,
    mut db: fontdb::Database,
    settings: &FontSettings,
    generics: &mut GenericFonts,
) -> (FontSystem, [ResolvedFamily; 2]) {
    let resolved = configure_generic_families(&mut db, settings, generics);
    let fallback = QuarkFallback::new(settings.bundled_fallback, &locale);
    let font_system = FontSystem::new_with_locale_and_db_and_fallback(locale, db, fallback);
    (font_system, resolved)
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

/// The family [`renamed_inter`] registers.
#[cfg(test)]
pub(crate) const RENAMED_INTER: &str = "Quark";

/// Inter's font file with its family renamed to [`RENAMED_INTER`]: a font
/// no vendored-only system has, for tests that load one. Same length, so
/// only the name strings change.
#[cfg(test)]
pub(crate) fn renamed_inter() -> Vec<u8> {
    let mut bytes = include_bytes!("../assets/fonts/Inter-Variable.ttf").to_vec();
    let utf16 =
        |name: &str| -> Vec<u8> { name.encode_utf16().flat_map(u16::to_be_bytes).collect() };
    for (from, to) in [
        (b"Inter".to_vec(), RENAMED_INTER.as_bytes().to_vec()),
        (utf16("Inter"), utf16(RENAMED_INTER)),
    ] {
        let mut at = 0;
        while let Some(i) = bytes[at..].windows(from.len()).position(|w| w == from) {
            bytes[at + i..at + i + from.len()].copy_from_slice(&to);
            at += i + from.len();
        }
    }
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::TextStyle;

    // Catches a recipe that leaves out loaded fonts: a worker's twin would
    // shape the family they add with the default font instead.
    #[test]
    fn recipe_builds_a_system_with_the_loaded_fonts() {
        let mut system = TextSystem::vendored_only(&FontSettings {
            ui_family: RENAMED_INTER.into(),
            ..FontSettings::default()
        });
        system.load_font_data(Arc::new(renamed_inter()));
        let mut inter = TextSystem::vendored_only(&FontSettings {
            ui_family: "Inter".into(),
            ..FontSettings::default()
        });
        let probe = TextParams::new("iiiiMMMM", TextStyle::new(14.0));
        let width = |system: &mut TextSystem| system.layout(&probe).expect("layout").size().0;

        let widths = [width(&mut system), width(&mut system.recipe().build())];
        assert_eq!(widths, [width(&mut inter); 2]);
    }

    // A family named by an interned id draws in that family, in the system
    // that interned it and in a worker built from its recipe, which must
    // resolve the same ids; another system does not know the id and draws
    // in the generic family.
    #[test]
    fn interned_family_draws_in_that_family_in_recipe_twins() {
        let mut system = TextSystem::vendored_only(&FontSettings::default());
        let id = system.family_id("JetBrains Mono");
        let style = TextStyle::new(14.0).font_family(crate::fonts::FontFamily::Named(id));
        let family = |system: &mut TextSystem| {
            let layout = system
                .layout(&TextParams::new("ok", style))
                .expect("layout");
            let glyph = layout.glyph(0).expect("glyph");
            let face = system.font_system().db().face(glyph.font_id).expect("face");
            face.families[0].0.clone()
        };
        let mut twin = system.recipe().build();
        let mut other = TextSystem::vendored_only(&FontSettings::default());
        let drawn = [family(&mut system), family(&mut twin), family(&mut other)];
        assert_eq!(drawn, ["JetBrains Mono", "JetBrains Mono", "Geist"]);
    }

    // Catches bold text in a family named by the style falling back to
    // another family: JetBrains Mono ships one variable face, registered
    // at a single weight, which cosmic-text took only at that weight.
    #[test]
    fn bold_text_in_a_named_family_keeps_the_family() {
        let mut system = TextSystem::vendored_only(&FontSettings::default());
        let style = TextStyle::new(14.0)
            .family(Some("JetBrains Mono"))
            .weight(quark::FontWeight::Bold);
        let layout = system
            .layout(&TextParams::new("bold", style))
            .expect("layout");
        let db = system.font_system().db();
        for glyph in layout.glyphs() {
            let family = &db.face(glyph.font_id).expect("face").families[0].0;
            assert_eq!(family, "JetBrains Mono");
        }
    }
}
