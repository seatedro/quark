//! Vendored fonts, generic family resolution and the fallback chain for
//! [`crate::TextSystem`], and the family catalog shown in font pickers.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use cosmic_text::{Fallback, PlatformFallback, fontdb};
use serde::{Deserialize, Serialize};
use unicode_script::Script;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontRole {
    Ui,
    Mono,
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

pub(crate) fn configure_generic_families(db: &mut fontdb::Database, settings: &FontSettings) {
    let settings = settings.normalized();
    let ui = resolve_family(db, FontRole::Ui, &settings.ui_family);
    let mono = resolve_family(db, FontRole::Mono, &settings.mono_family);
    // A family picked for UI or code text with fewer weights than quark
    // asks for (Fira Code's variable face registers only as Light) would
    // otherwise lose to a fallback family of the exact weight.
    fill_weights(db, [ui.as_str(), mono.as_str()]);
    db.set_sans_serif_family(ui);
    db.set_monospace_family(mono);
}

fn resolve_family(db: &fontdb::Database, role: FontRole, selection: &str) -> String {
    let available = db
        .faces()
        .any(|face| face.families.iter().any(|(name, _)| name == selection));
    if available {
        selection.to_owned()
    } else {
        default_family(role).to_owned()
    }
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

/// Weights quark asks for (see `layout::weight_value`).
const TEXT_WEIGHTS: [u16; 5] = [400, 450, 500, 600, 700];

impl QuarkFallback {
    /// cosmic-text takes a fallback family only through a face of exactly
    /// the requested weight, so a family with fewer weights (the bundled
    /// emoji and CJK faces have one) is skipped at the UI's normal weight
    /// (450) and in bold, and an arbitrary face of the nearest weight wins
    /// instead. Registers each listed family again under every missing text
    /// weight, copying the face fontdb's own query picks for that weight;
    /// copies share the original's bytes.
    pub(crate) fn fill_weights(&self, db: &mut fontdb::Database) {
        let mut families: Vec<&str> = self.common.clone();
        families.extend(self.scripts.values().flatten());
        families.sort_unstable();
        families.dedup();
        fill_weights(db, families);
    }
}

/// Register each of `families` again under every text weight it lacks, as
/// a copy of the face fontdb's own query picks for that weight; copies
/// share the original's bytes.
fn fill_weights<'a>(db: &mut fontdb::Database, families: impl IntoIterator<Item = &'a str>) {
    let mut copies = Vec::new();
    for family in families {
        for weight in TEXT_WEIGHTS {
            let has_weight = db.faces().any(|face| {
                face.weight.0 == weight
                    && face.style == fontdb::Style::Normal
                    && face.families.iter().any(|(name, _)| name == family)
            });
            if has_weight {
                continue;
            }
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(family)],
                weight: fontdb::Weight(weight),
                ..fontdb::Query::default()
            };
            if let Some(face) = db.query(&query).and_then(|id| db.face(id)) {
                let mut copy = face.clone();
                copy.weight = fontdb::Weight(weight);
                copies.push(copy);
            }
        }
    }
    for copy in copies {
        db.push_face_info(copy);
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
pub(crate) type FacesKey = (Option<&'static str>, bool, u16);

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

    /// The family `c` should draw in instead of the fallback cosmic-text
    /// would pick, for text whose own face is `primary`; `None` to leave
    /// it to the fallback. Only characters `emoji` (the color emoji
    /// face) has are looked at: elsewhere the fallback cannot land on
    /// color.
    pub(crate) fn text_family(
        &mut self,
        db: &fontdb::Database,
        c: char,
        (primary, emoji): Faces,
    ) -> Option<Arc<str>> {
        let emoji = emoji?;
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
            .find(|&id| self.covers(db, id, c) && !self.is_color(db, id))
            .and_then(|id| db.face(id))
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
        let g = layout.glyphs();
        let db = system.font_system().db();
        (0..g.len())
            .map(|i| {
                let family = db
                    .face(g.font_id[i])
                    .map(|face| face.families[0].0.clone())
                    .unwrap_or_default();
                let range = g.byte_start[i] as usize..g.byte_end[i] as usize;
                let bytes = layout.text().get(range).unwrap_or_default();
                (family, g.glyph_id[i], g.advance[i], bytes.to_owned())
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
            let width = reference.glyphs().advance[0];
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
            layout.glyphs().glyph_id.to_vec()
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
