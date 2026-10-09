//! Vendored fonts, generic family resolution and the fallback chain for
//! [`crate::TextSystem`], and the family catalog shown in font pickers.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use cosmic_text::{Fallback, PlatformFallback, fontdb};
use serde::{Deserialize, Serialize};
use unicode_script::Script;

use crate::epoch::FontEpoch;

#[cfg(target_os = "macos")]
mod coretext;
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "android")))]
mod fontconfig;
mod platform;

pub(crate) use platform::Candidate;

pub const UI_FAMILY: &str = "Geist";
pub const MONO_FAMILY: &str = "Geist Mono";
pub const INTER_FAMILY: &str = "Inter";
pub const IBM_PLEX_SANS_FAMILY: &str = "IBM Plex Sans";
pub const SOURCE_SANS_3_FAMILY: &str = "Source Sans 3";
pub const JETBRAINS_MONO_FAMILY: &str = "JetBrains Mono";
pub const IBM_PLEX_MONO_FAMILY: &str = "IBM Plex Mono";
pub const FIRA_CODE_FAMILY: &str = "Fira Code";
/// Bundled with the `emoji-font` feature.
pub const EMOJI_FAMILY: &str = "Noto Color Emoji";
/// Bundled with the `cjk-font` feature: a renamed subset of Noto Sans CJK SC,
/// so system fallback lists naming the full font never pick the subset.
pub const CJK_FAMILY: &str = "Quark CJK Fallback";

/// The [`FontSettings`] family name that asks for the platform's UI font:
/// SF Pro through CoreText on macOS, Segoe UI Variable (then Segoe UI) on
/// Windows, fontconfig's `system-ui`/`sans-serif` match on Linux.
pub const SYSTEM_UI: &str = "system-ui";
/// The [`FontSettings`] family name that asks for the platform's monospace
/// font: SF Mono (then Menlo) on macOS, Cascadia Mono (then Consolas) on
/// Windows, fontconfig's `monospace` match on Linux.
pub const UI_MONOSPACE: &str = "ui-monospace";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    Ui,
    Mono,
}

/// A font family by name, for [`FontFamily::Named`]. Either a static name
/// ([`Self::from_static`]) or one a [`crate::TextSystem`] interned
/// ([`crate::TextSystem::family_id`]); an interned id means that family in
/// the system that issued it and in systems built from its recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FamilyId(FamilyRepr);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FamilyRepr {
    Static(&'static str),
    Interned(u32),
}

impl FamilyId {
    pub const fn from_static(name: &'static str) -> Self {
        Self(FamilyRepr::Static(name))
    }
}

/// Which family text draws in.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum FontFamily {
    /// The UI family [`FontSettings::ui_family`] resolves to: the platform
    /// UI font under [`FontSettings::system`], bundled Geist by default.
    #[default]
    SystemUi,
    /// The monospace family [`FontSettings::mono_family`] resolves to.
    UiMonospace,
    /// A family by name. A family the system lacks draws in the generic
    /// family of the text's [`quark::FontKind`].
    Named(FamilyId),
}

/// Family names a [`crate::TextSystem`] interned, by [`FamilyId`].
/// Recipes carry it, so a twin built on another thread resolves the same
/// ids to the same names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct FamilyNames(Vec<Arc<str>>);

impl FamilyNames {
    pub(crate) const fn new() -> Self {
        Self(Vec::new())
    }

    /// The id of `name`, interning it on first use. Families are few, so a
    /// scan beats hashing.
    pub(crate) fn intern(&mut self, name: &str) -> FamilyId {
        let index = match self.0.iter().position(|known| &**known == name) {
            Some(index) => index,
            None => {
                self.0.push(Arc::from(name));
                self.0.len() - 1
            }
        };
        FamilyId(FamilyRepr::Interned(index as u32))
    }

    /// `None` for an id another system interned.
    pub(crate) fn name(&self, id: FamilyId) -> Option<&str> {
        match id.0 {
            FamilyRepr::Static(name) => Some(name),
            FamilyRepr::Interned(index) => self.0.get(index as usize).map(|name| &**name),
        }
    }
}

/// What a [`FontSettings`] family resolved to, for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedFamily {
    /// The settings' name: a family, [`SYSTEM_UI`], or [`UI_MONOSPACE`].
    pub requested: String,
    /// The family text draws in, as the font database names it.
    pub family: String,
    pub source: ResolvedSource,
    /// Why `family` is not what was asked for, when it is not.
    pub fallback: Option<FallbackReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolvedSource {
    /// An installed or loaded family the settings named.
    Named,
    /// The family the platform's font API answered for a generic name.
    Platform,
    /// A bundled family.
    Bundled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FallbackReason {
    /// No installed or loaded face has the family the settings named.
    NotInstalled,
    /// The platform's font API gave no usable answer for a generic name.
    PlatformUnavailable,
    /// The system loads only bundled fonts
    /// ([`crate::TextSystem::vendored_only`]), so generic names resolve to
    /// bundled families on every machine.
    Deterministic,
}

/// The fonts of a [`crate::TextSystem`] at one [`FontEpoch`], shareable
/// across threads: for SVG text and other rasterizers that need the same
/// faces and generic families as text layout. Face data is shared, not
/// copied.
#[derive(Debug, Clone)]
pub struct FontSnapshot {
    pub(crate) epoch: FontEpoch,
    pub(crate) database: Arc<fontdb::Database>,
}

impl FontSnapshot {
    /// Changes whenever the fonts do; key anything rasterized with this
    /// snapshot by it.
    pub fn epoch(&self) -> FontEpoch {
        self.epoch
    }

    /// Every face, with the generic sans-serif and monospace families set
    /// to the resolved UI and monospace families.
    pub fn database(&self) -> &Arc<fontdb::Database> {
        &self.database
    }

    pub fn ui_family(&self) -> &str {
        self.database.family_name(&fontdb::Family::SansSerif)
    }

    pub fn mono_family(&self) -> &str {
        self.database.family_name(&fontdb::Family::Monospace)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FontSettings {
    pub ui_family: String,
    pub mono_family: String,
    pub bundled_fallback: BundledFallback,
    /// Standard and contextual ligatures (`fi`, `->` in Fira Code). Off
    /// shapes every character on its own.
    pub ligatures: bool,
}

/// Where the bundled emoji and CJK faces sit in the fallback chain. Either
/// way a character the chosen family lacks tries, in order: the bundled UI
/// and mono families, then system and bundled fallbacks in this order, then
/// any other installed face.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundledFallback {
    /// System fallback fonts first, so emoji and CJK look native; the
    /// bundled faces fill in what the system lacks.
    #[default]
    AfterSystem,
    /// Bundled faces first, so emoji and CJK look the same on every machine.
    BeforeSystem,
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            ui_family: UI_FAMILY.to_owned(),
            mono_family: MONO_FAMILY.to_owned(),
            bundled_fallback: BundledFallback::default(),
            ligatures: true,
        }
    }
}

impl FontSettings {
    /// The platform's UI and monospace fonts ([`SYSTEM_UI`] and
    /// [`UI_MONOSPACE`]), where [`Self::default`] picks the bundled ones,
    /// which look the same on every machine.
    pub fn system() -> Self {
        Self {
            ui_family: SYSTEM_UI.to_owned(),
            mono_family: UI_MONOSPACE.to_owned(),
            ..Self::default()
        }
    }

    pub fn normalized(&self) -> Self {
        Self {
            ui_family: normalize_font_selection(FontRole::Ui, &self.ui_family),
            mono_family: normalize_font_selection(FontRole::Mono, &self.mono_family),
            bundled_fallback: self.bundled_fallback,
            ligatures: self.ligatures,
        }
    }
}

macro_rules! font {
    ($file:literal) => {
        include_bytes!(concat!("../assets/fonts/", $file)) as &[u8]
    };
}

// A static, not a const: a const may be instantiated (and its font bytes
// embedded) once per use.
static VENDORED_FONT_BYTES: &[&[u8]] = &[
    font!("Geist-Regular.otf"),
    font!("Geist-Medium.otf"),
    font!("Geist-SemiBold.otf"),
    font!("Geist-Bold.otf"),
    font!("GeistMono-Regular.otf"),
    font!("GeistMono-Medium.otf"),
    font!("GeistMono-SemiBold.otf"),
    font!("GeistMono-Bold.otf"),
    font!("Inter-Variable.ttf"),
    font!("IBMPlexSans-Variable.ttf"),
    font!("SourceSans3-Regular.ttf"),
    font!("SourceSans3-Medium.ttf"),
    font!("SourceSans3-Semibold.ttf"),
    font!("SourceSans3-Bold.ttf"),
    font!("JetBrainsMono-Variable.ttf"),
    font!("JetBrainsMono-Italic.ttf"),
    font!("FiraCode-Variable.ttf"),
    font!("IBMPlexMono-Regular.ttf"),
    font!("IBMPlexMono-Italic.ttf"),
    font!("IBMPlexMono-Medium.ttf"),
    font!("IBMPlexMono-SemiBold.ttf"),
    font!("IBMPlexMono-Bold.ttf"),
    #[cfg(feature = "emoji-font")]
    font!("NotoColorEmoji.ttf"),
    #[cfg(feature = "cjk-font")]
    font!("QuarkCJKFallback-Regular.otf"),
];

pub fn normalize_font_selection(role: FontRole, family: &str) -> String {
    let trimmed = family.trim();
    if trimmed.is_empty() {
        return default_family(role).to_owned();
    }
    trimmed.to_owned()
}

/// Sources that read the fonts embedded in the binary in place, so each
/// [`crate::TextSystem`] costs no copy of the font files.
pub(crate) fn vendored_font_sources() -> impl Iterator<Item = fontdb::Source> {
    VENDORED_FONT_BYTES
        .iter()
        .map(|&bytes| fontdb::Source::Binary(Arc::new(bytes)))
}

/// Where [`SYSTEM_UI`] and [`UI_MONOSPACE`] resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GenericFonts {
    /// To the bundled families, on every machine.
    Bundled,
    /// To what the platform's font API answers for the locale, asked when
    /// first needed.
    Platform { locale: String },
    /// To the first of these candidates (UI, then monospace) that is
    /// loaded: the platform's answers once asked, which a recipe passes on
    /// so worker systems need not ask again, or a test's fixture.
    Pinned([Vec<Candidate>; 2]),
}

impl GenericFonts {
    fn candidates(&mut self, role: FontRole) -> Option<&[Candidate]> {
        if let Self::Platform { locale } = self {
            let ask = |role| platform::candidates(role, locale);
            *self = Self::Pinned([ask(FontRole::Ui), ask(FontRole::Mono)]);
        }
        match (self, role) {
            (Self::Bundled | Self::Platform { .. }, _) => None,
            (Self::Pinned([ui, _]), FontRole::Ui) => Some(ui),
            (Self::Pinned([_, mono]), FontRole::Mono) => Some(mono),
        }
    }
}

/// Points the generic sans-serif and monospace families at the settings'
/// UI and monospace families, and reports what each resolved to.
pub(crate) fn configure_generic_families(
    db: &mut fontdb::Database,
    settings: &FontSettings,
    generics: &mut GenericFonts,
) -> [ResolvedFamily; 2] {
    let settings = settings.normalized();
    let ui = resolve_family(db, FontRole::Ui, &settings.ui_family, generics);
    let mono = resolve_family(db, FontRole::Mono, &settings.mono_family, generics);
    db.set_sans_serif_family(ui.family.clone());
    db.set_monospace_family(mono.family.clone());
    [ui, mono]
}

fn resolve_family(
    db: &mut fontdb::Database,
    role: FontRole,
    selection: &str,
    generics: &mut GenericFonts,
) -> ResolvedFamily {
    let generic = match selection {
        SYSTEM_UI => Some(FontRole::Ui),
        UI_MONOSPACE => Some(FontRole::Mono),
        _ => None,
    };
    let resolved = |family: String, source, fallback| ResolvedFamily {
        requested: selection.to_owned(),
        family,
        source,
        fallback,
    };
    let Some(generic) = generic else {
        return match family_source(db, selection) {
            Some(source) => resolved(selection.to_owned(), source, None),
            None => resolved(
                default_family(role).to_owned(),
                ResolvedSource::Bundled,
                Some(FallbackReason::NotInstalled),
            ),
        };
    };
    let Some(candidates) = generics.candidates(generic) else {
        return resolved(
            default_family(role).to_owned(),
            ResolvedSource::Bundled,
            Some(FallbackReason::Deterministic),
        );
    };
    for candidate in candidates {
        if let Some(family) = find_candidate(db, candidate) {
            let fallback = (!candidate.from_api).then_some(FallbackReason::PlatformUnavailable);
            return resolved(family, ResolvedSource::Platform, fallback);
        }
    }
    resolved(
        default_family(role).to_owned(),
        ResolvedSource::Bundled,
        Some(FallbackReason::PlatformUnavailable),
    )
}

/// The family, as the font database names it, of the face `candidate`
/// describes: the face of its file (with its PostScript name, when the
/// file has several), loading the file when no face of it is loaded; else
/// a face with its family name in any language. `None` when there is no
/// such face.
fn find_candidate(db: &mut fontdb::Database, candidate: &Candidate) -> Option<String> {
    if let Some(path) = &candidate.path {
        let in_file = |face: &&fontdb::FaceInfo| match &face.source {
            fontdb::Source::File(file) | fontdb::Source::SharedFile(file, _) => file == path,
            fontdb::Source::Binary(_) => false,
        };
        if !db.faces().any(|face| in_file(&face)) {
            // Not under a directory the database scanned; a missing or
            // unreadable file loads nothing.
            let _ = db.load_font_file(path);
        }
        let named = |face: &&fontdb::FaceInfo| {
            candidate
                .post_script_name
                .as_ref()
                .is_none_or(|name| &face.post_script_name == name)
        };
        let face = db
            .faces()
            .filter(in_file)
            .find(named)
            .or_else(|| db.faces().find(in_file));
        if let Some((family, _)) = face.and_then(|face| face.families.first()) {
            return Some(family.clone());
        }
    }
    let name = candidate.family.as_deref()?;
    db.faces()
        .find(|face| {
            face.families
                .iter()
                .any(|(family, _)| family.eq_ignore_ascii_case(name))
        })
        .and_then(|face| face.families.first())
        .map(|(family, _)| family.clone())
}

/// Whether a face of `family` is loaded, and from where: the vendored
/// fonts are the only ones read from static bytes.
fn family_source(db: &fontdb::Database, family: &str) -> Option<ResolvedSource> {
    let face = db
        .faces()
        .find(|face| face.families.iter().any(|(name, _)| name == family))?;
    let bundled = matches!(&face.source, fontdb::Source::Binary(data)
        if VENDORED_FONT_BYTES.iter().any(|font| std::ptr::eq((**data).as_ref().as_ptr(), font.as_ptr())));
    Some(if bundled {
        ResolvedSource::Bundled
    } else {
        ResolvedSource::Named
    })
}

/// The fallback chain: cosmic-text tries the requested family, then the
/// list for each script in the text, then the common list, then every other
/// face. The bundled UI and mono families lead the common list, and the
/// bundled emoji and CJK faces go before or after the platform's lists.
pub(crate) struct QuarkFallback {
    common: Vec<&'static str>,
    /// CJK scripts only; other scripts use the platform's lists unchanged.
    scripts: HashMap<Script, Vec<&'static str>>,
}

/// Scripts the bundled CJK face covers.
const CJK_SCRIPTS: &[Script] = &[
    Script::Han,
    Script::Hiragana,
    Script::Katakana,
    Script::Hangul,
    Script::Bopomofo,
];

impl QuarkFallback {
    pub(crate) fn new(order: BundledFallback, locale: &str) -> Self {
        let platform = PlatformFallback;
        let place = |system: &[&'static str], bundled: &[&'static str]| {
            let (first, second) = match order {
                BundledFallback::AfterSystem => (system, bundled),
                BundledFallback::BeforeSystem => (bundled, system),
            };
            let mut list = first.to_vec();
            list.extend(second.iter().filter(|family| !first.contains(family)));
            list
        };

        let cjk: &[&'static str] = if cfg!(feature = "cjk-font") {
            &[CJK_FAMILY]
        } else {
            &[]
        };
        let mut bundled = Vec::new();
        if cfg!(feature = "emoji-font") {
            bundled.push(EMOJI_FAMILY);
        }
        // CJK punctuation and fullwidth forms are script Common, so only the
        // common list reaches them.
        bundled.extend_from_slice(cjk);

        let mut common = vec![UI_FAMILY, MONO_FAMILY];
        common.extend(place(platform.common_fallback(), &bundled));
        let scripts = CJK_SCRIPTS
            .iter()
            .map(|&script| (script, place(platform.script_fallback(script, locale), cjk)))
            .collect();
        Self { common, scripts }
    }
}

impl Fallback for QuarkFallback {
    fn common_fallback(&self) -> &[&'static str] {
        &self.common
    }

    fn forbidden_fallback(&self) -> &[&'static str] {
        PlatformFallback.forbidden_fallback()
    }

    fn script_fallback(&self, script: Script, locale: &str) -> &[&'static str] {
        match self.scripts.get(&script) {
            Some(list) => list,
            None => PlatformFallback.script_fallback(script, locale),
        }
    }
}

/// The color emoji family that emoji-presentation clusters ask for first:
/// the platform's emoji font or the bundled one, in `order`.
pub(crate) fn emoji_family(db: &fontdb::Database, order: BundledFallback) -> Option<&'static str> {
    const SYSTEM: [&str; 3] = ["Apple Color Emoji", "Segoe UI Emoji", "Noto Color Emoji"];
    let installed = |name: &str| {
        db.faces()
            .any(|face| face.families.iter().any(|(family, _)| family == name))
    };
    let bundled = cfg!(feature = "emoji-font").then_some(EMOJI_FAMILY);
    let system = SYSTEM.into_iter().find(|name| installed(name));
    match order {
        BundledFallback::AfterSystem => system.or(bundled),
        BundledFallback::BeforeSystem => bundled.or(system),
    }
}

/// Families searched first for a character that should draw as text but is
/// missing from the text's own font, before any other face without color
/// glyphs: symbol and wide-coverage fonts on each platform, monospaced
/// ones first.
const TEXT_SYMBOL_FAMILIES: [&str; 12] = [
    "Menlo",
    "Apple Symbols",
    "STIX Two Math",
    "Segoe UI Symbol",
    "Cambria Math",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
    "DejaVu Sans",
    "Noto Sans Symbols 2",
    "Noto Sans Symbols",
    "Noto Sans Math",
    "Symbola",
];

/// Faces for characters that should draw as text (default text
/// presentation, or VS15) but that the text's own font lacks and a color
/// emoji face has. cosmic-text's fallback takes the first face that has a
/// character, color or not, so without this ⏺ or ✔ would draw as a color
/// emoji. Like Ghostty, a fallback must match the presentation: the first
/// face without color glyphs that has the character, the color face only
/// when there is none. Answers are kept per character and per face, and
/// dropped when the fonts change.
/// A text's named family, whether its generic family is the monospace
/// one, and its weight.
pub(crate) type FacesKey = (Option<FamilyId>, bool, u16);

/// A text's own face and the color emoji face.
pub(crate) type Faces = (Option<fontdb::ID>, Option<fontdb::ID>);

#[derive(Debug, Default)]
pub(crate) struct TextFaces {
    /// The family a character draws in, `None` when no text face has it.
    families: HashMap<char, Option<Arc<str>>>,
    /// Whether a face has a character.
    coverage: HashMap<(fontdb::ID, char), bool>,
    /// Whether a face has color glyphs (COLR, CBDT, sbix, or SVG tables).
    color: HashMap<fontdb::ID, bool>,
    /// The face text shapes with and the color emoji face, by the text's
    /// family (named, else generic) and weight. Font queries allocate, so
    /// layouts reuse the answers.
    faces: HashMap<FacesKey, Faces>,
    /// Rasterizes a candidate's glyph once, to skip faces swash cannot
    /// draw (bitmap-only glyphs, such as Unifont's on some systems).
    raster: Option<cosmic_text::SwashCache>,
}

impl TextFaces {
    pub(crate) fn clear(&mut self) {
        self.families.clear();
        self.coverage.clear();
        self.color.clear();
        self.faces.clear();
    }

    /// The face of the first family in `families` at `weight` (the text's
    /// own face), and the face of `emoji`.
    pub(crate) fn faces(
        &mut self,
        db: &fontdb::Database,
        key: FacesKey,
        family: fontdb::Family,
        emoji: &str,
    ) -> Faces {
        *self.faces.entry(key).or_insert_with(|| {
            let query = |family: fontdb::Family, weight: u16| {
                db.query(&fontdb::Query {
                    families: &[family],
                    weight: fontdb::Weight(weight),
                    ..fontdb::Query::default()
                })
            };
            (
                query(family, key.2),
                query(fontdb::Family::Name(emoji), 400),
            )
        })
    }

    fn covers(&mut self, db: &fontdb::Database, id: fontdb::ID, c: char) -> bool {
        *self.coverage.entry((id, c)).or_insert_with(|| {
            db.with_face_data(id, |data, index| {
                cosmic_text::skrifa::FontRef::from_index(data, index).is_ok_and(|font| {
                    use cosmic_text::skrifa::MetadataProvider as _;
                    font.charmap().map(c).is_some()
                })
            })
            .unwrap_or(false)
        })
    }

    fn is_color(&mut self, db: &fontdb::Database, id: fontdb::ID) -> bool {
        *self.color.entry(id).or_insert_with(|| {
            db.with_face_data(id, |data, index| {
                use cosmic_text::skrifa::raw::{TableProvider as _, types::Tag};
                let Ok(font) = cosmic_text::skrifa::FontRef::from_index(data, index) else {
                    return false;
                };
                [b"COLR", b"CBDT", b"sbix", b"SVG "]
                    .iter()
                    .any(|tag| font.data_for_tag(Tag::new(tag)).is_some())
            })
            .unwrap_or(false)
        })
    }

    /// Whether face `id` draws `c` with any ink.
    fn draws(&mut self, fs: &mut cosmic_text::FontSystem, id: fontdb::ID, c: char) -> bool {
        use cosmic_text::skrifa::MetadataProvider as _;
        let Some((glyph, weight)) = fs.db().face(id).and_then(|face| {
            let weight = face.weight;
            fs.db()
                .with_face_data(id, |data, index| {
                    let font = cosmic_text::skrifa::FontRef::from_index(data, index).ok()?;
                    font.charmap().map(c)
                })
                .flatten()
                .map(|glyph| (glyph, weight))
        }) else {
            return false;
        };
        let Ok(glyph) = u16::try_from(glyph.to_u32()) else {
            return false;
        };
        let (key, _, _) = cosmic_text::CacheKey::new(
            id,
            glyph,
            16.0,
            (0.0, 0.0),
            weight,
            cosmic_text::CacheKeyFlags::empty(),
        );
        self.raster
            .get_or_insert_with(cosmic_text::SwashCache::new)
            .get_image_uncached(fs, key)
            .is_some_and(|image| image.data.iter().any(|&a| a != 0))
    }

    /// The family `c` should draw in instead of the fallback cosmic-text
    /// would pick, for text whose own face is `primary`; `None` to leave
    /// it to the fallback. Only characters `emoji` (the color emoji
    /// face) has are looked at: elsewhere the fallback cannot land on
    /// color.
    pub(crate) fn text_family(
        &mut self,
        fs: &mut cosmic_text::FontSystem,
        c: char,
        (primary, emoji): Faces,
    ) -> Option<Arc<str>> {
        let emoji = emoji?;
        let db = fs.db();
        if !self.covers(db, emoji, c) || primary.is_some_and(|id| self.covers(db, id, c)) {
            return None;
        }
        if let Some(found) = self.families.get(&c) {
            return found.clone();
        }
        let named = TEXT_SYMBOL_FAMILIES.iter().filter_map(|family| {
            db.query(&fontdb::Query {
                families: &[fontdb::Family::Name(family)],
                ..fontdb::Query::default()
            })
        });
        let candidates: Vec<fontdb::ID> = named
            .chain(
                db.faces()
                    .filter(|f| f.style == fontdb::Style::Normal)
                    .map(|f| f.id),
            )
            .collect();
        let found = candidates
            .into_iter()
            .find(|&id| {
                let db = fs.db();
                self.covers(db, id, c) && !self.is_color(db, id) && self.draws(fs, id, c)
            })
            .and_then(|id| fs.db().face(id))
            .and_then(|face| face.families.first())
            .map(|(name, _)| Arc::from(name.as_str()));
        self.families.insert(c, found.clone());
        found
    }
}

/// Whether a grapheme cluster should draw as a color emoji: an emoji
/// presentation selector, a keycap, a skin tone, a flag, or a first
/// character that defaults to emoji presentation. Without this, a text font
/// that happens to cover the base character (DejaVu Sans has a heart and
/// most emoticons, Geist has U+263A) draws it in monochrome.
pub(crate) fn is_emoji_presentation(grapheme: &str) -> bool {
    let Some(first) = grapheme.chars().next() else {
        return false;
    };
    if grapheme.contains('\u{fe0e}') {
        return false;
    }
    default_emoji_presentation(first)
        || grapheme
            .chars()
            .any(|c| matches!(c, '\u{fe0f}' | '\u{20e3}' | '\u{1f3fb}'..='\u{1f3ff}'))
}

fn default_emoji_presentation(c: char) -> bool {
    let c = c as u32;
    EMOJI_PRESENTATION
        .binary_search_by(|&(lo, hi)| {
            if hi < c {
                std::cmp::Ordering::Less
            } else if lo > c {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `Emoji_Presentation=Yes` ranges from Unicode 17 emoji-data.txt.
#[rustfmt::skip]
const EMOJI_PRESENTATION: &[(u32, u32)] = &[
    (0x231A, 0x231B), (0x23E9, 0x23EC), (0x23F0, 0x23F0), (0x23F3, 0x23F3), (0x25FD, 0x25FE),
    (0x2614, 0x2615), (0x2648, 0x2653), (0x267F, 0x267F), (0x2693, 0x2693), (0x26A1, 0x26A1),
    (0x26AA, 0x26AB), (0x26BD, 0x26BE), (0x26C4, 0x26C5), (0x26CE, 0x26CE), (0x26D4, 0x26D4),
    (0x26EA, 0x26EA), (0x26F2, 0x26F3), (0x26F5, 0x26F5), (0x26FA, 0x26FA), (0x26FD, 0x26FD),
    (0x2705, 0x2705), (0x270A, 0x270B), (0x2728, 0x2728), (0x274C, 0x274C), (0x274E, 0x274E),
    (0x2753, 0x2755), (0x2757, 0x2757), (0x2795, 0x2797), (0x27B0, 0x27B0), (0x27BF, 0x27BF),
    (0x2B1B, 0x2B1C), (0x2B50, 0x2B50), (0x2B55, 0x2B55), (0x1F004, 0x1F004), (0x1F0CF, 0x1F0CF),
    (0x1F18E, 0x1F18E), (0x1F191, 0x1F19A), (0x1F1E6, 0x1F1FF), (0x1F201, 0x1F201),
    (0x1F21A, 0x1F21A), (0x1F22F, 0x1F22F), (0x1F232, 0x1F236), (0x1F238, 0x1F23A),
    (0x1F250, 0x1F251), (0x1F300, 0x1F320), (0x1F32D, 0x1F335), (0x1F337, 0x1F37C),
    (0x1F37E, 0x1F393), (0x1F3A0, 0x1F3CA), (0x1F3CF, 0x1F3D3), (0x1F3E0, 0x1F3F0),
    (0x1F3F4, 0x1F3F4), (0x1F3F8, 0x1F43E), (0x1F440, 0x1F440), (0x1F442, 0x1F4FC),
    (0x1F4FF, 0x1F53D), (0x1F54B, 0x1F54E), (0x1F550, 0x1F567), (0x1F57A, 0x1F57A),
    (0x1F595, 0x1F596), (0x1F5A4, 0x1F5A4), (0x1F5FB, 0x1F64F), (0x1F680, 0x1F6C5),
    (0x1F6CC, 0x1F6CC), (0x1F6D0, 0x1F6D2), (0x1F6D5, 0x1F6D8), (0x1F6DC, 0x1F6DF),
    (0x1F6EB, 0x1F6EC), (0x1F6F4, 0x1F6FC), (0x1F7E0, 0x1F7EB), (0x1F7F0, 0x1F7F0),
    (0x1F90C, 0x1F93A), (0x1F93C, 0x1F945), (0x1F947, 0x1F9FF), (0x1FA70, 0x1FA7C),
    (0x1FA80, 0x1FA8A), (0x1FA8E, 0x1FAC6), (0x1FAC8, 0x1FAC8), (0x1FACD, 0x1FADC),
    (0x1FADF, 0x1FAEA), (0x1FAEF, 0x1FAF8),
];

fn default_family(role: FontRole) -> &'static str {
    match role {
        FontRole::Ui => UI_FAMILY,
        FontRole::Mono => MONO_FAMILY,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontFamilyOption {
    pub label: &'static str,
    pub family: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontFamilySource {
    Bundled,
    System,
}

impl FontFamilySource {
    pub fn label(self) -> &'static str {
        match self {
            Self::Bundled => "Bundled",
            Self::System => "System",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFamilyEntry {
    pub label: String,
    pub family: String,
    pub source: FontFamilySource,
    pub monospaced: bool,
}

const UI_FONT_OPTIONS: &[FontFamilyOption] = &[
    FontFamilyOption {
        label: "Geist",
        family: UI_FAMILY,
    },
    FontFamilyOption {
        label: "Inter",
        family: INTER_FAMILY,
    },
    FontFamilyOption {
        label: "IBM Plex Sans",
        family: IBM_PLEX_SANS_FAMILY,
    },
    FontFamilyOption {
        label: "Source Sans 3",
        family: SOURCE_SANS_3_FAMILY,
    },
];

const MONO_FONT_OPTIONS: &[FontFamilyOption] = &[
    FontFamilyOption {
        label: "Geist Mono",
        family: MONO_FAMILY,
    },
    FontFamilyOption {
        label: "JetBrains Mono",
        family: JETBRAINS_MONO_FAMILY,
    },
    FontFamilyOption {
        label: "IBM Plex Mono",
        family: IBM_PLEX_MONO_FAMILY,
    },
    FontFamilyOption {
        label: "Fira Code",
        family: FIRA_CODE_FAMILY,
    },
];

static FONT_CATALOG: OnceLock<FontCatalog> = OnceLock::new();

pub fn bundled_font_options(role: FontRole) -> &'static [FontFamilyOption] {
    match role {
        FontRole::Ui => UI_FONT_OPTIONS,
        FontRole::Mono => MONO_FONT_OPTIONS,
    }
}

/// Bundled families first, then installed system families. Scans system
/// fonts once per process.
pub fn font_family_entries(role: FontRole) -> &'static [FontFamilyEntry] {
    let catalog = FONT_CATALOG.get_or_init(build_font_catalog);
    match role {
        FontRole::Ui => &catalog.ui,
        FontRole::Mono => &catalog.mono,
    }
}

pub fn font_selection_label(selection: &str) -> String {
    let selection = selection.trim();
    UI_FONT_OPTIONS
        .iter()
        .chain(MONO_FONT_OPTIONS.iter())
        .find(|option| option.family == selection)
        .map(|option| option.label.to_owned())
        .unwrap_or_else(|| selection.to_owned())
}

#[derive(Debug)]
struct FontCatalog {
    ui: Vec<FontFamilyEntry>,
    mono: Vec<FontFamilyEntry>,
}

#[derive(Debug, Clone, Copy)]
struct CatalogFamily {
    source: FontFamilySource,
    monospaced: bool,
}

impl Default for CatalogFamily {
    fn default() -> Self {
        Self {
            source: FontFamilySource::System,
            monospaced: false,
        }
    }
}

fn build_font_catalog() -> FontCatalog {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    for source in vendored_font_sources() {
        db.load_font_source(source);
    }

    let mut families = BTreeMap::<String, CatalogFamily>::new();
    for face in db.faces() {
        let source = match &face.source {
            fontdb::Source::Binary(_) => FontFamilySource::Bundled,
            _ => FontFamilySource::System,
        };
        for (family, _) in &face.families {
            if !show_catalog_family(family, source) {
                continue;
            }
            let entry = families.entry(family.clone()).or_default();
            entry.monospaced |= face.monospaced;
            if source == FontFamilySource::Bundled {
                entry.source = FontFamilySource::Bundled;
            }
        }
    }

    FontCatalog {
        ui: build_role_catalog(FontRole::Ui, &families),
        mono: build_role_catalog(FontRole::Mono, &families),
    }
}

fn show_catalog_family(family: &str, source: FontFamilySource) -> bool {
    // The bundled fallback faces fill gaps; they are not text fonts to pick.
    !family.is_empty()
        && family != EMOJI_FAMILY
        && family != CJK_FAMILY
        && (source == FontFamilySource::Bundled || !family.starts_with('.'))
}

fn build_role_catalog(
    role: FontRole,
    families: &BTreeMap<String, CatalogFamily>,
) -> Vec<FontFamilyEntry> {
    let pinned = bundled_font_options(role);
    let mut entries = Vec::new();

    for option in pinned {
        let monospaced = families
            .get(option.family)
            .map(|family| family.monospaced)
            .unwrap_or(matches!(role, FontRole::Mono));
        entries.push(FontFamilyEntry {
            label: option.label.to_owned(),
            family: option.family.to_owned(),
            source: FontFamilySource::Bundled,
            monospaced,
        });
    }

    for (family, info) in families {
        if pinned.iter().any(|option| option.family == family) {
            continue;
        }
        if role == FontRole::Mono && !info.monospaced {
            continue;
        }
        entries.push(FontFamilyEntry {
            label: family.clone(),
            family: family.clone(),
            source: info.source,
            monospaced: info.monospaced,
        });
    }

    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::test_system;
    #[cfg(any(feature = "emoji-font", feature = "cjk-font"))]
    use crate::{TextLayout, TextParams, TextStyle, TextSystem};
    #[cfg(any(feature = "emoji-font", feature = "cjk-font"))]
    use quark::FontKind;

    /// `(family, glyph_id, advance, bytes)` of each glyph, in order.
    #[cfg(any(feature = "emoji-font", feature = "cjk-font"))]
    fn glyph_table(system: &TextSystem, layout: &TextLayout) -> Vec<(String, u16, f32, String)> {
        let db = system.font_system().db();
        layout
            .glyphs()
            .iter()
            .map(|g| {
                let family = db
                    .face(g.font_id)
                    .map(|face| face.families[0].0.clone())
                    .unwrap_or_default();
                let bytes = layout.text().get(g.bytes()).unwrap_or_default();
                (family, g.glyph_id, g.advance, bytes.to_owned())
            })
            .collect()
    }

    // Each emoji sequence must shape as one color glyph covering the whole
    // sequence, one emoji wide, in UI and mono text alike. A sequence split
    // into pieces (a family as four people, a flag as two letters, a keycap
    // as a digit and a box) is the regression.
    #[cfg(feature = "emoji-font")]
    #[test]
    fn emoji_sequences_shape_as_single_clusters() {
        const SEQUENCES: &[(&str, &str)] = &[
            (
                "zwj family",
                "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}\u{200d}\u{1f466}",
            ),
            ("zwj profession", "\u{1f469}\u{200d}\u{1f4bb}"),
            ("zwj with skin tone", "\u{1f9d1}\u{1f3ff}\u{200d}\u{1f680}"),
            ("zwj flag", "\u{1f3f3}\u{fe0f}\u{200d}\u{1f308}"),
            ("skin tone", "\u{1f44d}\u{1f3fd}"),
            ("flag", "\u{1f1ef}\u{1f1f5}"),
            (
                "subdivision flag",
                "\u{1f3f4}\u{e0067}\u{e0062}\u{e0073}\u{e0063}\u{e0074}\u{e007f}",
            ),
            ("keycap digit", "1\u{fe0f}\u{20e3}"),
            ("keycap hash", "#\u{fe0f}\u{20e3}"),
            ("emoji presentation selector", "\u{2764}\u{fe0f}"),
            // Geist has a text U+263A; the selector must still pick the emoji.
            ("selector on a text character", "\u{263a}\u{fe0f}"),
        ];
        let mut system = test_system();
        for kind in [FontKind::Ui, FontKind::Mono] {
            let style = TextStyle::new(20.0).kind(kind);
            let reference = system
                .layout(&TextParams::new("\u{1f600}", style))
                .expect("layout");
            let width = reference.glyphs()[0].advance;
            for &(name, text) in SEQUENCES {
                let layout = system
                    .layout(&TextParams::new(text, style))
                    .expect("layout");
                let table = glyph_table(&system, &layout);
                let [(family, glyph, advance, bytes)] = table.as_slice() else {
                    panic!("{kind:?} {name}: {} glyphs: {table:?}", table.len());
                };
                assert_eq!(family, EMOJI_FAMILY, "{kind:?} {name}");
                assert_ne!(*glyph, 0, "{kind:?} {name} is .notdef");
                assert_eq!(bytes, text, "{kind:?} {name}: cluster");
                assert_eq!(*advance, width, "{kind:?} {name}: advance");
            }
        }
    }

    // Ghostty's presentation rules: a character that defaults to text
    // presentation, or asks for it with VS15, draws from a face without
    // color glyphs when one has it, even though the color emoji face comes
    // first in the fallback chain; VS16 and emoji-presentation characters
    // draw in color; and a character no text face has falls back to color
    // whatever it asked for.
    #[cfg(feature = "emoji-font")]
    #[test]
    fn presentation_picks_a_text_or_color_face() {
        const TABLE: &[(&str, &str, bool)] = &[
            ("check mark", "\u{2714}", false),
            ("check mark, VS15", "\u{2714}\u{fe0e}", false),
            ("check mark, VS16", "\u{2714}\u{fe0f}", true),
            ("heart", "\u{2764}", false),
            ("warning sign", "\u{26a0}", false),
            ("warning sign, VS16", "\u{26a0}\u{fe0f}", true),
            ("grinning face", "\u{1f600}", true),
            (
                "grinning face, VS15 with no text face",
                "\u{1f600}\u{fe0e}",
                true,
            ),
        ];
        let mut system = test_system();
        for kind in [FontKind::Ui, FontKind::Mono] {
            let style = TextStyle::new(20.0).kind(kind);
            for &(name, text, color) in TABLE {
                let layout = system
                    .layout(&TextParams::new(text, style))
                    .expect("layout");
                let table = glyph_table(&system, &layout);
                let (family, glyph, ..) = &table[0];
                assert_ne!(*glyph, 0, "{kind:?} {name} is .notdef");
                assert_eq!(family == EMOJI_FAMILY, color, "{kind:?} {name}: {family}");
            }
        }
    }

    // With no system fonts, Chinese, Japanese, and Korean text (and CJK
    // punctuation, which is script Common) must come from the bundled face
    // instead of `.notdef`. Column expectation for mono text: Han, kana, and
    // fullwidth punctuation are exactly 1 em wide, so they line up with each
    // other but not with Latin mono cells (two Geist Mono cells are 1.2 em);
    // Hangul keeps the face's narrower 0.92 em.
    #[cfg(feature = "cjk-font")]
    #[test]
    fn cjk_falls_back_to_the_bundled_face_without_system_fonts() {
        let text = "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{30c6}\u{30ad}\u{30b9}\u{30c8}\u{3002}\u{d55c}\u{ad6d}\u{c5b4}\u{ff0c}\u{4e2d}\u{6587}\u{5b57}\u{7b26}";
        let mut system = test_system();
        for kind in [FontKind::Ui, FontKind::Mono] {
            let style = TextStyle::new(16.0).kind(kind);
            let layout = system
                .layout(&TextParams::new(text, style))
                .expect("layout");
            let table = glyph_table(&system, &layout);
            assert_eq!(table.len(), text.chars().count(), "{kind:?}: {table:?}");
            for (family, glyph, advance, bytes) in &table {
                assert_eq!(family, CJK_FAMILY, "{kind:?} {bytes}");
                assert_ne!(*glyph, 0, "{kind:?} {bytes} is .notdef");
                let hangul = bytes
                    .chars()
                    .all(|c| ('\u{ac00}'..='\u{d7a3}').contains(&c));
                if !hangul {
                    assert_eq!(*advance, 16.0, "{kind:?} {bytes}: advance");
                }
            }
        }
    }

    // The policy in FontSettings::bundled_fallback: the bundled emoji and CJK
    // faces go after the platform's fallback families by default and before
    // them when asked, while the bundled UI and mono families always lead.
    #[test]
    fn bundled_fallback_order_places_bundled_faces_around_system_lists() {
        let bundled = [EMOJI_FAMILY, CJK_FAMILY];
        for (order, bundled_first) in [
            (BundledFallback::AfterSystem, false),
            (BundledFallback::BeforeSystem, true),
        ] {
            let fallback = QuarkFallback::new(order, "ja");
            let common = fallback.common_fallback();
            assert_eq!(&common[..2], &[UI_FAMILY, MONO_FAMILY], "{order:?}");
            let lists = [(common, PlatformFallback.common_fallback())];
            let han = fallback.script_fallback(Script::Han, "ja");
            let platform_han = PlatformFallback.script_fallback(Script::Han, "ja");
            for (list, platform) in lists.into_iter().chain([(han, platform_han)]) {
                let system = |f: &&str| platform.contains(f) && !bundled.contains(f);
                let Some(first_system) = list.iter().position(system) else {
                    continue;
                };
                for (i, family) in list.iter().enumerate() {
                    if bundled.contains(family) && !platform.contains(family) {
                        assert_eq!(
                            i < first_system,
                            bundled_first,
                            "{order:?} {family} in {list:?}"
                        );
                    }
                }
            }
        }
    }

    // The ligatures setting: with it off, Fira Code's `==`, `<=`, and `&&`
    // must shape as the same glyphs as each character alone; with it on
    // (the default) the font's contextual alternates join them. Also
    // catches a picked family with fewer weights (Fira Code registers only
    // as Light) losing to Geist Mono.
    #[test]
    fn ligatures_off_shapes_each_character_alone() {
        use crate::{TextParams, TextStyle, TextSystem};
        use quark::FontKind;
        let glyphs = |system: &mut TextSystem, text: &str| {
            let style = TextStyle::new(16.0).kind(FontKind::Mono);
            let layout = system
                .layout(&TextParams::new(text, style))
                .expect("layout");
            layout
                .glyphs()
                .iter()
                .map(|g| g.glyph_id)
                .collect::<Vec<_>>()
        };
        let fira = FontSettings {
            mono_family: FIRA_CODE_FAMILY.to_owned(),
            ..FontSettings::default()
        };
        let mut system = TextSystem::vendored_only(&fira);
        for text in ["==", "<=", "&&"] {
            let alone: Vec<u16> = text
                .chars()
                .flat_map(|c| glyphs(&mut system, &c.to_string()))
                .collect();
            system.set_font_settings(&fira);
            let joined = glyphs(&mut system, text);
            system.set_font_settings(&FontSettings {
                ligatures: false,
                ..fira.clone()
            });
            let separate = glyphs(&mut system, text);
            assert_ne!(joined, alone, "{text} with ligatures");
            assert_eq!(separate, alone, "{text} without ligatures");
        }
    }

    // Regression: UI text at Normal asked for weight 450 (mono for 400),
    // which a variable UI family such as Inter draws heavier than its
    // regular weight. Normal is 400 in every family, the weight glyphs
    // rasterize at.
    #[test]
    fn normal_weight_is_400_in_every_family() {
        let settings = FontSettings {
            ui_family: INTER_FAMILY.to_owned(),
            ..FontSettings::default()
        };
        let mut system = crate::TextSystem::vendored_only(&settings);
        for kind in [quark::FontKind::Ui, quark::FontKind::Mono] {
            let style = crate::TextStyle::new(14.0).kind(kind);
            let layout = system
                .layout(&crate::TextParams::new("Hamburg", style))
                .expect("layout");
            for glyph in layout.glyph_iter() {
                assert_eq!(glyph.font_weight.0, 400, "{kind:?}");
            }
        }
    }

    // A weight a family has no face for takes the family's face nearest
    // it, as CSS matching does, rather than another family that has the
    // exact weight: before, Geist at 300 drew in Fira Code (whose variable
    // face registers as 300) and Fira Code at 700 in Geist Bold. fontdb
    // breaks the 400 to 500 tie at 450, looking down first from there.
    #[test]
    fn weight_without_a_face_takes_the_family_face_nearest_it() {
        use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, Weight};
        let cases: [(&str, u16, (&str, u16)); 4] = [
            (UI_FAMILY, 300, (UI_FAMILY, 400)),
            (UI_FAMILY, 450, (UI_FAMILY, 400)),
            (FIRA_CODE_FAMILY, 700, (FIRA_CODE_FAMILY, 300)),
            (SOURCE_SANS_3_FAMILY, 300, (SOURCE_SANS_3_FAMILY, 400)),
        ];
        let mut system = test_system();
        let fs = system.raster_font_system();
        for (family, weight, expected) in cases {
            let attrs = Attrs::new()
                .family(Family::Name(family))
                .weight(Weight(weight));
            let mut buffer = Buffer::new(fs, Metrics::new(14.0, 20.0));
            buffer.set_text(fs, "Hamburg", &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(fs, false);
            let run = buffer.layout_runs().next().expect("run");
            for glyph in run.glyphs {
                let face = fs.db().face(glyph.font_id).expect("face");
                let got = (face.families[0].0.as_str(), face.weight.0);
                assert_eq!(got, expected, "{family} at {weight}");
            }
        }
    }

    // A static family taken at a lighter weight than its lightest face
    // draws that face as it is: no synthetic thinning (or emboldening)
    // makes up the difference.
    #[test]
    fn static_face_at_a_missing_weight_rasterizes_as_its_own_weight() {
        use cosmic_text::{Attrs, Buffer, Family, Metrics, Shaping, SwashCache, Weight};
        let mut system = test_system();
        let fs = system.raster_font_system();
        let mut raster = SwashCache::new();
        let mut images = Vec::new();
        for weight in [300, 400] {
            let attrs = Attrs::new()
                .family(Family::Name(SOURCE_SANS_3_FAMILY))
                .weight(Weight(weight));
            let mut buffer = Buffer::new(fs, Metrics::new(20.0, 28.0));
            buffer.set_text(fs, "R", &attrs, Shaping::Advanced, None);
            buffer.shape_until_scroll(fs, false);
            let run = buffer.layout_runs().next().expect("run");
            let key = run.glyphs[0].physical((0.0, 0.0), 1.0).cache_key;
            assert_eq!(key.font_weight.0, weight);
            let image = raster.get_image_uncached(fs, key).expect("image");
            images.push(image.data);
        }
        assert!(images[0].iter().any(|&a| a != 0), "ink");
        assert_eq!(images[0], images[1]);
    }

    /// The family of the face the first glyph of UI text (mono text with
    /// `mono`) draws from.
    fn drawn_family(system: &mut crate::TextSystem, mono: bool) -> String {
        let kind = if mono {
            quark::FontKind::Mono
        } else {
            quark::FontKind::Ui
        };
        let style = crate::TextStyle::new(14.0).kind(kind);
        let layout = system
            .layout(&crate::TextParams::new("Hamburg", style))
            .expect("layout");
        let glyph = layout.glyph(0).expect("glyph");
        let face = system.font_system().db().face(glyph.font_id).expect("face");
        face.families[0].0.clone()
    }

    // How the system UI and monospace names resolve, given what the
    // platform answered: the first answer a loaded face has (by name in
    // any case), a guessed family flagged as a fallback, the bundled family
    // when nothing matches, and the bundled family without asking at all in
    // the deterministic mode. Named families resolve as before. The text
    // must draw in the reported family.
    #[test]
    fn generic_names_resolve_to_the_first_loaded_platform_candidate() {
        let api = |name: &str| Candidate::family(name, true);
        let guess = |name: &str| Candidate::family(name, false);
        let platform = |ui: Vec<Candidate>| GenericFonts::Pinned([ui.clone(), ui]);
        let cases = [
            (
                SYSTEM_UI,
                platform(vec![api("No Such Sans"), api(INTER_FAMILY)]),
                (INTER_FAMILY, ResolvedSource::Platform, None),
            ),
            (
                UI_MONOSPACE,
                platform(vec![api("jetbrains mono")]),
                (JETBRAINS_MONO_FAMILY, ResolvedSource::Platform, None),
            ),
            (
                SYSTEM_UI,
                platform(vec![api("No Such Sans"), guess(IBM_PLEX_SANS_FAMILY)]),
                (
                    IBM_PLEX_SANS_FAMILY,
                    ResolvedSource::Platform,
                    Some(FallbackReason::PlatformUnavailable),
                ),
            ),
            (
                SYSTEM_UI,
                platform(vec![api("No Such Sans")]),
                (
                    UI_FAMILY,
                    ResolvedSource::Bundled,
                    Some(FallbackReason::PlatformUnavailable),
                ),
            ),
            (
                SYSTEM_UI,
                GenericFonts::Bundled,
                (
                    UI_FAMILY,
                    ResolvedSource::Bundled,
                    Some(FallbackReason::Deterministic),
                ),
            ),
            (
                "No Such Sans",
                platform(vec![api(INTER_FAMILY)]),
                (
                    UI_FAMILY,
                    ResolvedSource::Bundled,
                    Some(FallbackReason::NotInstalled),
                ),
            ),
        ];
        for (requested, generics, (family, source, fallback)) in cases {
            let mono = requested == UI_MONOSPACE;
            let settings = if mono {
                FontSettings {
                    mono_family: requested.to_owned(),
                    ..FontSettings::default()
                }
            } else {
                FontSettings {
                    ui_family: requested.to_owned(),
                    ..FontSettings::default()
                }
            };
            let mut system = crate::TextSystem::with_generics(&settings, generics);
            let role = if mono { FontRole::Mono } else { FontRole::Ui };
            let resolved = system.resolved_family(role).clone();
            let expected = ResolvedFamily {
                requested: requested.to_owned(),
                family: family.to_owned(),
                source,
                fallback,
            };
            assert_eq!(resolved, expected);
            assert_eq!(drawn_family(&mut system, mono), family, "{requested}");
        }
    }

    // macOS reports the UI font by file (its family is private), and the
    // file can lie outside the directories the font database scanned: the
    // face is loaded from it and picked by PostScript name, and a worker
    // built from the recipe draws in it too.
    #[test]
    fn generic_name_loads_the_platform_font_file() {
        let dir = std::env::temp_dir().join(format!("quark-text-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("platform-ui.ttf");
        std::fs::write(&path, crate::system::renamed_inter()).expect("font file");
        let candidate = Candidate {
            family: Some(".PrivateUIFont".to_owned()),
            path: Some(path.clone()),
            post_script_name: None,
            from_api: true,
        };
        let generics = GenericFonts::Pinned([vec![candidate], Vec::new()]);
        let mut system = crate::TextSystem::with_generics(&FontSettings::system(), generics);
        let mut worker = system.recipe().build();
        let drawn = [
            drawn_family(&mut system, false),
            drawn_family(&mut worker, false),
        ];
        std::fs::remove_dir_all(&dir).ok();
        assert_eq!(
            system.resolved_family(FontRole::Ui).family,
            crate::system::RENAMED_INTER
        );
        assert_eq!(drawn, [crate::system::RENAMED_INTER; 2]);
    }

    // Platform check, run by hand on each OS: system-ui and ui-monospace
    // text draws in the face the platform's font API names (by file where
    // it gives one, else by family).
    #[test]
    #[ignore = "platform integration: needs the platform's installed fonts"]
    fn system_fonts_draw_in_the_platform_api_faces() {
        let locale = sys_locale::get_locale().unwrap_or_else(|| "en-US".to_owned());
        let mut system = crate::TextSystem::with_settings(&FontSettings::system());
        for role in [FontRole::Ui, FontRole::Mono] {
            let resolved = system.resolved_family(role).clone();
            eprintln!("{role:?}: {resolved:?}");
            let candidates = platform::candidates(role, &locale);
            let api = candidates
                .iter()
                .find(|c| c.from_api)
                .expect("platform answer");
            assert_eq!(resolved.source, ResolvedSource::Platform, "{role:?}");
            assert_eq!(resolved.fallback, None, "{role:?}");
            let style = crate::TextStyle::new(14.0).kind(match role {
                FontRole::Ui => quark::FontKind::Ui,
                FontRole::Mono => quark::FontKind::Mono,
            });
            let layout = system
                .layout(&crate::TextParams::new("Hamburg", style))
                .expect("layout");
            let db = system.font_system().db();
            let face = db
                .face(layout.glyph(0).expect("glyph").font_id)
                .expect("face");
            match (&api.path, &face.source) {
                (Some(path), fontdb::Source::File(file) | fontdb::Source::SharedFile(file, _)) => {
                    assert_eq!(file, path, "{role:?}")
                }
                _ => assert!(
                    face.families
                        .iter()
                        .any(|(name, _)| Some(name) == api.family.as_ref()),
                    "{role:?}: {:?} vs {api:?}",
                    face.families
                ),
            }
        }
    }

    // Regression: every TextSystem copied the vendored fonts (several MB)
    // onto the heap instead of reading the bytes embedded in the binary.
    #[test]
    fn vendored_faces_read_the_embedded_font_bytes() {
        let system = test_system();
        let db = system.font_system().db();
        let embedded = |data: &[u8]| {
            VENDORED_FONT_BYTES
                .iter()
                .any(|font| font.as_ptr_range().contains(&data.as_ptr()))
        };
        for face in db.faces() {
            let in_place = db.with_face_data(face.id, |data, _| embedded(data));
            assert_eq!(in_place, Some(true), "{:?} was copied", face.families);
        }
    }
}
