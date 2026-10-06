//! Vendored fonts, generic family resolution for [`crate::TextSystem`], and
//! the family catalog shown in font pickers.

use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

use cosmic_text::fontdb;
use serde::{Deserialize, Serialize};

const LEGACY_SYSTEM_FONT_SELECTION: &str = "__diffy_system_font__";

pub const UI_FAMILY: &str = "Geist";
pub const MONO_FAMILY: &str = "Geist Mono";
pub const INTER_FAMILY: &str = "Inter";
pub const IBM_PLEX_SANS_FAMILY: &str = "IBM Plex Sans";
pub const SOURCE_SANS_3_FAMILY: &str = "Source Sans 3";
pub const JETBRAINS_MONO_FAMILY: &str = "JetBrains Mono";
pub const IBM_PLEX_MONO_FAMILY: &str = "IBM Plex Mono";
pub const FIRA_CODE_FAMILY: &str = "Fira Code";

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
}

impl Default for FontSettings {
    fn default() -> Self {
        Self {
            ui_family: UI_FAMILY.to_owned(),
            mono_family: MONO_FAMILY.to_owned(),
        }
    }
}

impl FontSettings {
    pub fn normalized(&self) -> Self {
        Self {
            ui_family: normalize_font_selection(FontRole::Ui, &self.ui_family),
            mono_family: normalize_font_selection(FontRole::Mono, &self.mono_family),
        }
    }
}

macro_rules! font {
    ($file:literal) => {
        include_bytes!(concat!("../assets/fonts/", $file)) as &[u8]
    };
}

const VENDORED_FONT_BYTES: &[&[u8]] = &[
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
];

pub fn normalize_font_selection(role: FontRole, family: &str) -> String {
    let trimmed = family.trim();
    if trimmed.is_empty() || trimmed == LEGACY_SYSTEM_FONT_SELECTION {
        return default_family(role).to_owned();
    }
    trimmed.to_owned()
}

pub(crate) fn vendored_font_sources() -> impl Iterator<Item = fontdb::Source> {
    VENDORED_FONT_BYTES
        .iter()
        .map(|bytes| fontdb::Source::Binary(Arc::new(bytes.to_vec())))
}

pub(crate) fn configure_generic_families(db: &mut fontdb::Database, settings: &FontSettings) {
    let settings = settings.normalized();
    let ui = resolve_family(db, FontRole::Ui, &settings.ui_family);
    let mono = resolve_family(db, FontRole::Mono, &settings.mono_family);
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
    !family.is_empty() && (source == FontFamilySource::Bundled || !family.starts_with('.'))
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
