//! The theme adapter: the one place workbench colors and sizes reach
//! quark's theme.
//!
//! Colors and theme metrics come from `assets/themes/workbench.json`, a
//! quark theme file whose `palette` holds the design's color aliases
//! (canvas, sidebar, surface, accent, ...) and whose `colors` map each
//! semantic token onto one of them. Tokens the file leaves out inherit
//! quark's theme of the same mode, never the other mode. The file is built
//! in; with `QUARK_WORKBENCH_THEME_DIR` set, edits to the copy there apply
//! live (see [`super::reload`]).
//!
//! Translucent colors in the file (the scrim) are chosen for quark's
//! renderer, which blends in linear light: `#15171c92` dims like a CSS
//! 32% scrim, `#000000c8` like CSS 50% black (`1 - (1 - a)^2.2`, as for
//! the shadow tokens). Tints are mixed opaque (`recipes::tint`) for the
//! same reason: a light color at low alpha over the dark canvas brightens
//! it far more than the alpha suggests. `border` is the raised surfaces'
//! hairline (lighter than `border_variant` in dark, where shadows barely
//! show); panels and cards use `border_variant`.
//!
//! On top of the file the adapter sets the compact component recipe, the
//! zoom, high contrast (quark's high-contrast colors, workbench sizes), and
//! reduced motion. `ui_font_size` stays at quark's 16-point zoom anchor;
//! 13- and 14-point text comes from the recipe and [`super::recipes`].

use std::sync::OnceLock;

use quark_app::quark_ui::theme::{
    ComponentMetrics, ControlMetrics, Elevation, SurfaceMetrics, Theme, ThemeContrast, ThemeFamily,
    ThemeMode,
};

use super::tokens::{self, Shadow};
use crate::contracts::ThemeChoice;

/// The built-in theme file.
pub const THEME_JSON: &str = include_str!("../../assets/themes/workbench.json");
/// The family name the adapter looks for in theme files.
pub const FAMILY: &str = "Workbench";

/// What the user chose about how the app looks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Appearance {
    pub choice: ThemeChoice,
    pub contrast: ThemeContrast,
    pub reduced_motion: bool,
    /// 1.0 is 100%.
    pub zoom: f32,
}

impl Appearance {
    pub fn new(choice: ThemeChoice) -> Self {
        Self {
            choice,
            contrast: ThemeContrast::Standard,
            reduced_motion: false,
            zoom: 1.0,
        }
    }

    /// `choice` with the rest from the environment, for launching the
    /// demo without settings: `QUARK_WORKBENCH_REDUCED_MOTION=1`,
    /// `QUARK_WORKBENCH_CONTRAST=high`, `QUARK_WORKBENCH_ZOOM=1.25`.
    pub fn from_env(choice: ThemeChoice) -> Self {
        let var = |name: &str| std::env::var(name).ok();
        Self {
            choice,
            contrast: match var("QUARK_WORKBENCH_CONTRAST").as_deref() {
                Some("high") => ThemeContrast::High,
                _ => ThemeContrast::Standard,
            },
            reduced_motion: var("QUARK_WORKBENCH_REDUCED_MOTION").is_some_and(|v| v != "0"),
            zoom: var("QUARK_WORKBENCH_ZOOM")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
        }
    }
}

/// The built-in workbench family.
pub fn builtin_family() -> &'static ThemeFamily {
    static FAMILY: OnceLock<ThemeFamily> = OnceLock::new();
    FAMILY.get_or_init(|| {
        // A broken built-in file is a build defect; `tests/design.rs`
        // parses it, so this never falls back in a tested build.
        ThemeFamily::from_json(THEME_JSON).unwrap_or_else(|_| ThemeFamily::builtin())
    })
}

/// The workbench theme of `mode` from `family`, for `appearance`.
pub fn theme(family: &ThemeFamily, mode: ThemeMode, appearance: &Appearance) -> Theme {
    let variant = match mode {
        ThemeMode::Light => family.light.clone(),
        ThemeMode::Dark => family.dark.clone(),
    };
    let variant = variant.unwrap_or_else(|| Theme::for_mode(mode));
    let mut theme = match appearance.contrast {
        ThemeContrast::Standard => variant,
        // Quark's high-contrast colors at the workbench's sizes.
        ThemeContrast::High => Theme {
            metrics: variant.metrics,
            ..Theme::for_mode_and_contrast(mode, ThemeContrast::High)
        },
    };
    theme.components = recipe(mode);
    theme.reduced_motion = appearance.reduced_motion;
    theme.with_ui_scale(appearance.zoom)
}

/// `(light, dark)` from `family` for `appearance`: both modes for
/// `System`, the chosen one twice otherwise.
pub fn themes(family: &ThemeFamily, appearance: &Appearance) -> (Theme, Theme) {
    let light = || theme(family, ThemeMode::Light, appearance);
    let dark = || theme(family, ThemeMode::Dark, appearance);
    match appearance.choice {
        ThemeChoice::System => (light(), dark()),
        ThemeChoice::Light => (light(), light()),
        ThemeChoice::Dark => (dark(), dark()),
    }
}

pub fn light() -> Theme {
    themes_for(ThemeChoice::Light).0
}

pub fn dark() -> Theme {
    themes_for(ThemeChoice::Dark).1
}

/// `(light, dark)` for `choice`, from the live theme file when one is
/// being watched and the built-in one otherwise, with the rest of the
/// appearance from the environment.
pub fn themes_for(choice: ThemeChoice) -> (Theme, Theme) {
    let appearance = Appearance::from_env(choice);
    match super::reload::live_family() {
        Some(family) => themes(&family, &appearance),
        None => themes(builtin_family(), &appearance),
    }
}

fn elevation(shadow: Shadow, mode: ThemeMode) -> Elevation {
    Elevation {
        offset_y: shadow.offset_y,
        blur: shadow.blur,
        alpha: match mode {
            ThemeMode::Light => shadow.alpha_light,
            ThemeMode::Dark => shadow.alpha_dark,
        },
    }
}

/// The compact component recipe, in points at 100% zoom: 28-point
/// controls with 13-point labels, 6-point rows and buttons, 8-point menus,
/// and a 16-point modal.
pub fn recipe(mode: ThemeMode) -> ComponentMetrics {
    use tokens::*;
    let control = ControlMetrics {
        height: Some(ICON_BUTTON),
        radius: Some(RADIUS_ROW),
        padding_x: Some(SPACE_12 - 2.0),
        padding_y: Some(SPACE_4),
        gap: Some(SPACE_6),
        font_size: Some(TYPE_CONTROL.0),
        icon_size: Some(ICON),
    };
    let menu = SurfaceMetrics {
        radius: Some(RADIUS_MENU),
        padding_x: Some(SPACE_8),
        padding_y: Some(SPACE_4),
        gap: Some(SPACE_4),
        font_size: Some(TYPE_CONTROL.0),
        title_font_size: None,
        shadow: Some(elevation(SHADOW_MENU, mode)),
    };
    ComponentMetrics {
        button: control,
        button_compact: ControlMetrics {
            height: Some(24.0),
            padding_x: Some(SPACE_8),
            gap: Some(SPACE_4),
            icon_size: Some(14.0),
            ..control
        },
        select: ControlMetrics {
            icon_size: Some(14.0),
            ..control
        },
        option: ControlMetrics {
            radius: None,
            gap: Some(SPACE_8),
            icon_size: Some(14.0),
            ..control
        },
        popover: menu,
        tooltip: SurfaceMetrics {
            radius: Some(RADIUS_ROW),
            padding_y: Some(SPACE_4),
            font_size: Some(TYPE_META.0 + 1.0),
            ..menu
        },
        toast: SurfaceMetrics {
            padding_x: Some(SPACE_12),
            padding_y: Some(SPACE_12),
            ..menu
        },
        modal: SurfaceMetrics {
            radius: Some(RADIUS_MODAL),
            padding_x: Some(SPACE_24),
            padding_y: Some(SPACE_24),
            gap: Some(SPACE_16),
            font_size: Some(TYPE_CONTROL.0),
            title_font_size: Some(TYPE_HEADING.0),
            shadow: Some(elevation(SHADOW_MODAL, mode)),
        },
        // Three prose lines: 12-point bars on the 22-point line pitch.
        skeleton: ControlMetrics {
            height: Some(12.0),
            radius: Some(RADIUS_BADGE),
            padding_x: Some(0.0),
            padding_y: Some(SPACE_4 + 1.0),
            gap: Some(TYPE_PROSE.1 - 12.0),
            ..ControlMetrics::default()
        },
        field: ControlMetrics {
            font_size: Some(TYPE_CONTROL.0),
            gap: Some(SPACE_6),
            ..ControlMetrics::default()
        },
    }
}
