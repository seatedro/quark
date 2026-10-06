//! Transitional: catalog and settings come from quark-text.

use std::sync::Arc;

use glyphon::{FontSystem, fontdb};

pub use quark_text::fonts::*;

pub const UI_REGULAR_OTF: &[u8] = include_bytes!("../../quark-text/assets/fonts/Geist-Regular.otf");
pub const UI_MEDIUM_OTF: &[u8] = include_bytes!("../../quark-text/assets/fonts/Geist-Medium.otf");
pub const UI_SEMIBOLD_OTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/Geist-SemiBold.otf");
pub const UI_BOLD_OTF: &[u8] = include_bytes!("../../quark-text/assets/fonts/Geist-Bold.otf");

pub const MONO_REGULAR_OTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/GeistMono-Regular.otf");
pub const MONO_MEDIUM_OTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/GeistMono-Medium.otf");
pub const MONO_SEMIBOLD_OTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/GeistMono-SemiBold.otf");
pub const MONO_BOLD_OTF: &[u8] = include_bytes!("../../quark-text/assets/fonts/GeistMono-Bold.otf");

pub const INTER_VARIABLE_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/Inter-Variable.ttf");
pub const IBM_PLEX_SANS_VARIABLE_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/IBMPlexSans-Variable.ttf");
pub const SOURCE_SANS_3_REGULAR_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/SourceSans3-Regular.ttf");
pub const SOURCE_SANS_3_MEDIUM_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/SourceSans3-Medium.ttf");
pub const SOURCE_SANS_3_SEMIBOLD_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/SourceSans3-Semibold.ttf");
pub const SOURCE_SANS_3_BOLD_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/SourceSans3-Bold.ttf");

pub const JETBRAINS_MONO_VARIABLE_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/JetBrainsMono-Variable.ttf");
pub const JETBRAINS_MONO_ITALIC_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/JetBrainsMono-Italic.ttf");
pub const FIRA_CODE_VARIABLE_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/FiraCode-Variable.ttf");
pub const IBM_PLEX_MONO_REGULAR_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/IBMPlexMono-Regular.ttf");
pub const IBM_PLEX_MONO_ITALIC_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/IBMPlexMono-Italic.ttf");
pub const IBM_PLEX_MONO_MEDIUM_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/IBMPlexMono-Medium.ttf");
pub const IBM_PLEX_MONO_SEMIBOLD_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/IBMPlexMono-SemiBold.ttf");
pub const IBM_PLEX_MONO_BOLD_TTF: &[u8] =
    include_bytes!("../../quark-text/assets/fonts/IBMPlexMono-Bold.ttf");

const VENDORED_FONT_BYTES: &[&[u8]] = &[
    UI_REGULAR_OTF,
    UI_MEDIUM_OTF,
    UI_SEMIBOLD_OTF,
    UI_BOLD_OTF,
    MONO_REGULAR_OTF,
    MONO_MEDIUM_OTF,
    MONO_SEMIBOLD_OTF,
    MONO_BOLD_OTF,
    INTER_VARIABLE_TTF,
    IBM_PLEX_SANS_VARIABLE_TTF,
    SOURCE_SANS_3_REGULAR_TTF,
    SOURCE_SANS_3_MEDIUM_TTF,
    SOURCE_SANS_3_SEMIBOLD_TTF,
    SOURCE_SANS_3_BOLD_TTF,
    JETBRAINS_MONO_VARIABLE_TTF,
    JETBRAINS_MONO_ITALIC_TTF,
    FIRA_CODE_VARIABLE_TTF,
    IBM_PLEX_MONO_REGULAR_TTF,
    IBM_PLEX_MONO_ITALIC_TTF,
    IBM_PLEX_MONO_MEDIUM_TTF,
    IBM_PLEX_MONO_SEMIBOLD_TTF,
    IBM_PLEX_MONO_BOLD_TTF,
];

pub fn new_font_system() -> FontSystem {
    new_font_system_with_settings(&FontSettings::default())
}

pub fn new_font_system_with_settings(settings: &FontSettings) -> FontSystem {
    let mut font_system = FontSystem::new_with_fonts(vendored_font_sources());
    configure_generic_families(font_system.db_mut(), settings);
    font_system
}

pub fn configure_font_system(font_system: &mut FontSystem) {
    configure_font_system_with_settings(font_system, &FontSettings::default());
}

pub fn configure_font_system_with_settings(font_system: &mut FontSystem, settings: &FontSettings) {
    let db = font_system.db_mut();
    for font_bytes in VENDORED_FONT_BYTES.iter().copied() {
        db.load_font_data(font_bytes.to_vec());
    }
    configure_generic_families(db, settings);
}

fn configure_generic_families(db: &mut fontdb::Database, settings: &FontSettings) {
    let settings = settings.normalized();
    let pick = |role, selection: &str| {
        let available = db
            .faces()
            .any(|face| face.families.iter().any(|(name, _)| name == selection));
        if available {
            selection.to_owned()
        } else {
            normalize_font_selection(role, "")
        }
    };
    let ui = pick(FontRole::Ui, &settings.ui_family);
    let mono = pick(FontRole::Mono, &settings.mono_family);
    db.set_sans_serif_family(ui);
    db.set_monospace_family(mono);
}

fn vendored_font_sources() -> impl Iterator<Item = fontdb::Source> {
    VENDORED_FONT_BYTES
        .iter()
        .copied()
        .map(|bytes| fontdb::Source::Binary(Arc::new(bytes.to_vec())))
}
