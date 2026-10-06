//! Minimal font loading for [`crate::TextSystem`].
//!
//! Duplicated from `quark-render/src/fonts.rs` (loading + generic family
//! resolution only; the family catalog stays in quark-render for now). The
//! font assets are read from quark-render's asset directory until the
//! integration task moves them here.

use std::sync::Arc;

use cosmic_text::fontdb;
use serde::{Deserialize, Serialize};

const LEGACY_SYSTEM_FONT_SELECTION: &str = "__diffy_system_font__";

pub const UI_FAMILY: &str = "Geist";
pub const MONO_FAMILY: &str = "Geist Mono";

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
        include_bytes!(concat!("../../quark-render/assets/fonts/", $file)) as &[u8]
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
