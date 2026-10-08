//! The theme adapter: maps the workbench color aliases onto quark's
//! semantic theme colors (design section 3). Placeholder: quark's default
//! themes; stream B supplies the porcelain/charcoal palette and metrics.

use quark_app::quark_ui::theme::Theme;

use crate::contracts::ThemeChoice;

pub fn light() -> Theme {
    Theme::default_light()
}

pub fn dark() -> Theme {
    Theme::default_dark()
}

/// `(light, dark)` for `choice`: both modes for `System`, the same theme
/// twice for a fixed choice.
pub fn themes_for(choice: ThemeChoice) -> (Theme, Theme) {
    match choice {
        ThemeChoice::System => (light(), dark()),
        ThemeChoice::Light => (light(), light()),
        ThemeChoice::Dark => (dark(), dark()),
    }
}
