use crate::palette::{self, Scale, Step};
use serde::{Deserialize, Serialize};

pub use quark::Color;

mod file;

pub use file::{ThemeError, ThemeFamily, ThemeIssue, ThemeRegistry, parse_color};

/// A struct of named tokens of one type, with lookup by name for theme
/// files.
macro_rules! tokens {
    (
        $(#[$meta:meta])*
        pub struct $name:ident: $ty:ty { $($field:ident,)* }
    ) => {
        $(#[$meta])*
        pub struct $name {
            $(pub $field: $ty,)*
        }

        impl $name {
            /// Every token name, in declaration order.
            pub const TOKENS: &'static [&'static str] = &[$(stringify!($field)),*];

            /// The token called `name`.
            pub fn get(&self, name: &str) -> Option<$ty> {
                match name {
                    $(stringify!($field) => Some(self.$field),)*
                    _ => None,
                }
            }

            /// Set the token called `name`; false when there is none.
            pub fn set(&mut self, name: &str, value: $ty) -> bool {
                match name {
                    $(stringify!($field) => self.$field = value,)*
                    _ => return false,
                }
                true
            }
        }
    };
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
}

tokens! {
    /// The semantic color tokens. Theme files name them as written here.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ThemeColors: Color {
        app_bg,
        canvas,
        panel,
        panel_strong,
        border_soft,
        text_strong,
        accent,
        accent_strong,
        selection_bg,
        background,
        surface,
        editor_surface,
        elevated_surface,
        modal_surface,
        overlay_scrim,
        border,
        border_variant,
        focus_border,
        text,
        text_muted,
        text_accent,
        icon,
        element_background,
        element_hover,
        element_active,
        element_selected,
        ghost_element_hover,
        ghost_element_active,
        ghost_element_selected,
        title_bar_background,
        status_bar_background,
        sidebar_background,
        sidebar_row_hover,
        sidebar_row_selected,
        empty_state_background,
        empty_state_border,
        scrollbar_thumb,
        status_info,
        status_warning,
        status_error,
        line_add,
        line_del,
        line_modified,
        gutter_bg,
        gutter_text,
        file_header_bg,
        hunk_header_bg,
        line_add_text,
        line_del_text,
        line_add_word_bg,
        line_del_word_bg,
        hover_overlay,
        search_match_bg,
        search_match_active_bg,
        syntax_keyword,
        syntax_string,
        syntax_comment,
        syntax_function,
        syntax_type,
        syntax_number,
        syntax_property,
        syntax_operator,
    }
}

tokens! {
    /// Sizes in logical points. Theme files name them as written here.
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct ThemeMetrics: f32 {
        title_bar_height,
        status_bar_height,
        sidebar_width,
        panel_radius,
        control_radius,
        modal_radius,
        spacing_xs,
        spacing_sm,
        spacing_md,
        spacing_lg,
        ui_font_size,
        ui_small_font_size,
        heading_font_size,
        mono_font_size,
        ui_row_height,
        code_row_height,
    }
}

impl ThemeMetrics {
    pub fn ui_scale(&self) -> f32 {
        (self.ui_font_size / 16.0).max(0.7)
    }

    pub fn scaled(self, scale: f32) -> Self {
        let scale = scale.clamp(0.5, 4.0);
        Self {
            title_bar_height: self.title_bar_height * scale,
            status_bar_height: self.status_bar_height * scale,
            sidebar_width: self.sidebar_width * scale,
            panel_radius: self.panel_radius * scale,
            control_radius: self.control_radius * scale,
            modal_radius: self.modal_radius * scale,
            spacing_xs: self.spacing_xs * scale,
            spacing_sm: self.spacing_sm * scale,
            spacing_md: self.spacing_md * scale,
            spacing_lg: self.spacing_lg * scale,
            ui_font_size: self.ui_font_size * scale,
            ui_small_font_size: self.ui_small_font_size * scale,
            heading_font_size: self.heading_font_size * scale,
            mono_font_size: self.mono_font_size * scale,
            ui_row_height: self.ui_row_height * scale,
            code_row_height: self.code_row_height * scale,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub mode: ThemeMode,
    pub sans_family: &'static str,
    pub mono_family: &'static str,
    pub colors: ThemeColors,
    pub metrics: ThemeMetrics,
    /// Jump instead of animating where motion is decoration (smooth
    /// scrolling), for users who ask the system to reduce motion.
    pub reduced_motion: bool,
}

impl Theme {
    pub fn for_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Dark => Self::default_dark(),
            ThemeMode::Light => Self::default_light(),
        }
    }

    pub fn with_ui_scale(mut self, scale: f32) -> Self {
        self.metrics = self.metrics.scaled(scale);
        self
    }

    pub fn toggle_mode(&mut self) {
        *self = Self::for_mode(match self.mode {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        });
    }

    pub fn default_dark() -> Self {
        let n = palette::dark_scale(palette::NEUTRAL_HUE, palette::NEUTRAL_CHROMA);
        let blue = palette::dark_scale(palette::BLUE_HUE, palette::BLUE_CHROMA);
        let red = palette::dark_scale(palette::RED_HUE, palette::RED_CHROMA);
        let green = palette::dark_scale(palette::GREEN_HUE, palette::GREEN_CHROMA);
        let yellow = palette::dark_scale(palette::YELLOW_HUE, palette::YELLOW_CHROMA);
        let purple = palette::dark_scale(palette::PURPLE_HUE, palette::PURPLE_CHROMA);
        let teal = palette::dark_scale(palette::TEAL_HUE, palette::TEAL_CHROMA);
        let orange = palette::dark_scale(palette::ORANGE_HUE, palette::ORANGE_CHROMA);

        Self {
            mode: ThemeMode::Dark,
            sans_family: default_sans_family(),
            mono_family: default_mono_family(),
            colors: dark_colors(&n, &blue, &red, &green, &yellow, &purple, &teal, &orange),
            metrics: default_metrics(),
            reduced_motion: false,
        }
    }

    pub fn default_light() -> Self {
        let n = palette::light_scale(palette::NEUTRAL_HUE, palette::NEUTRAL_CHROMA);
        let blue = palette::light_scale(palette::BLUE_HUE, palette::BLUE_CHROMA);
        let red = palette::light_scale(palette::RED_HUE, palette::RED_CHROMA);
        let green = palette::light_scale(palette::GREEN_HUE, palette::GREEN_CHROMA);
        let yellow = palette::light_scale(palette::YELLOW_HUE, palette::YELLOW_CHROMA);
        let purple = palette::light_scale(palette::PURPLE_HUE, palette::PURPLE_CHROMA);
        let teal = palette::light_scale(palette::TEAL_HUE, palette::TEAL_CHROMA);
        let orange = palette::light_scale(palette::ORANGE_HUE, palette::ORANGE_CHROMA);

        Self {
            mode: ThemeMode::Light,
            sans_family: default_sans_family(),
            mono_family: default_mono_family(),
            colors: light_colors(&n, &blue, &red, &green, &yellow, &purple, &teal, &orange),
            metrics: default_metrics(),
            reduced_motion: false,
        }
    }
}

fn default_metrics() -> ThemeMetrics {
    ThemeMetrics {
        title_bar_height: 44.0,
        status_bar_height: 40.0,
        sidebar_width: 280.0,
        panel_radius: 12.0,
        control_radius: 8.0,
        modal_radius: 16.0,
        spacing_xs: 4.0,
        spacing_sm: 6.0,
        spacing_md: 12.0,
        spacing_lg: 16.0,
        ui_font_size: 16.0,
        ui_small_font_size: 14.0,
        heading_font_size: 20.0,
        mono_font_size: 15.0,
        ui_row_height: 40.0,
        code_row_height: 24.0,
    }
}

/// Build dark-mode theme colors from perceptual scales.
///
/// Mapping convention — `n` is the 12-step neutral, `b` blue accent,
/// `r` red, `g` green, `y` yellow. Indexed via `Step` enum.
// One scale per hue; a struct would only rename the same eight arguments.
#[allow(clippy::too_many_arguments)]
fn dark_colors(
    n: &Scale,
    b: &Scale,
    r: &Scale,
    g: &Scale,
    y: &Scale,
    purple: &Scale,
    teal: &Scale,
    orange: &Scale,
) -> ThemeColors {
    use Step::*;

    ThemeColors {
        // Backgrounds — clear visual hierarchy between layers.
        app_bg: n[Bg],
        canvas: n[BgAlt],
        panel: n[Element],
        panel_strong: n[ElementHover],
        background: n[Bg],
        surface: n[Element],
        editor_surface: n[BgAlt],
        elevated_surface: n[ElementHover],
        modal_surface: n[ElementActive],
        title_bar_background: n[Element],
        status_bar_background: n[BgAlt],
        sidebar_background: n[BgAlt],
        empty_state_background: n[Element],
        gutter_bg: n[Bg],
        file_header_bg: n[ElementHover],
        hunk_header_bg: b[ElementHover],

        // Interactive elements — clear affordance
        element_background: n[ElementHover],
        element_hover: n[ElementActive],
        element_active: n[BorderSubtle],
        element_selected: b[BorderSubtle],

        // Ghost elements — accent-tinted so they feel integrated with the theme
        ghost_element_hover: b[ElementHover],
        ghost_element_active: b[ElementActive],
        ghost_element_selected: b[ElementActive],
        hover_overlay: b[ElementHover],

        sidebar_row_hover: b[ElementHover],
        sidebar_row_selected: b[ElementActive],

        // Borders — subtle but visible separation
        border_soft: n[ElementHover],
        border: n[ElementActive],
        border_variant: n[ElementHover],
        focus_border: b[Solid],
        empty_state_border: n[ElementActive],

        // Text — readable hierarchy
        text_strong: Color::rgba(240, 240, 245, 255),
        text: n[Text],
        text_muted: n[Solid],
        text_accent: b[TextSubtle],
        icon: n[TextSubtle],
        gutter_text: n[BorderStrong],

        // Accent — vibrant blue
        accent: b[Solid],
        accent_strong: b[TextSubtle],
        selection_bg: b[ElementActive],

        // Overlay
        overlay_scrim: Color::rgba(0, 0, 0, 180),

        // Scrollbar
        scrollbar_thumb: Color::rgba(255, 255, 255, 100),

        // Status indicators
        status_info: b[Solid],
        status_warning: y[Solid],
        status_error: r[Solid],

        // Diff colors
        line_add: g[Element],
        line_del: r[Element],
        line_modified: b[Element],
        line_add_text: g[Solid],
        line_del_text: r[Solid],
        line_add_word_bg: g[ElementActive],
        line_del_word_bg: r[ElementActive],

        search_match_bg: Color::rgba(y[Border].r, y[Border].g, y[Border].b, 90),
        search_match_active_bg: Color::rgba(y[Solid].r, y[Solid].g, y[Solid].b, 180),

        syntax_keyword: purple[TextSubtle],
        syntax_string: g[TextSubtle],
        syntax_comment: n[Solid],
        syntax_function: b[TextSubtle],
        syntax_type: teal[TextSubtle],
        syntax_number: orange[TextSubtle],
        syntax_property: r[TextSubtle],
        syntax_operator: n[TextSubtle],
    }
}

/// Build light-mode theme colors from perceptual scales.
#[allow(clippy::too_many_arguments)]
fn light_colors(
    n: &Scale,
    b: &Scale,
    r: &Scale,
    g: &Scale,
    y: &Scale,
    purple: &Scale,
    teal: &Scale,
    orange: &Scale,
) -> ThemeColors {
    use Step::*;

    ThemeColors {
        // Backgrounds (lightest first in light mode)
        app_bg: n[Bg],
        canvas: n[BgAlt],
        panel: n[Element],
        panel_strong: n[ElementHover],
        background: n[Bg],
        surface: n[Element],
        editor_surface: n[BgAlt],
        elevated_surface: n[Element],
        modal_surface: n[Element],
        title_bar_background: n[ElementHover],
        status_bar_background: n[ElementHover],
        sidebar_background: n[BgAlt],
        empty_state_background: n[Element],
        gutter_bg: n[ElementHover],
        file_header_bg: n[ElementHover],
        hunk_header_bg: b[Element],

        // Interactive elements
        element_background: n[ElementHover],
        element_hover: n[ElementActive],
        element_active: n[BorderSubtle],
        element_selected: b[ElementActive],

        // Ghost elements — accent-tinted so they feel integrated with the theme
        ghost_element_hover: b[Element],
        ghost_element_active: b[ElementHover],
        ghost_element_selected: b[ElementHover],
        hover_overlay: b[Element],

        sidebar_row_hover: b[Element],
        sidebar_row_selected: b[ElementHover],

        // Borders
        border_soft: n[Border],
        border: n[Border],
        border_variant: n[BorderSubtle],
        focus_border: b[Solid],
        empty_state_border: n[Border],

        // Text (darkest in light mode)
        text_strong: n[TextStrong],
        text: n[TextStrong],
        text_muted: n[TextSubtle],
        text_accent: b[TextSubtle],
        icon: n[TextSubtle],
        gutter_text: n[Solid],

        // Accent
        accent: b[Solid],
        accent_strong: b[TextSubtle],
        selection_bg: b[ElementHover],

        // Overlay
        overlay_scrim: Color::rgba(11, 21, 32, 51),

        // Scrollbar
        scrollbar_thumb: n[BorderStrong],

        // Status
        status_info: b[Solid],
        status_warning: y[Solid],
        status_error: r[Solid],

        // Diff
        line_add: g[Element],
        line_del: r[Element],
        line_modified: b[Element],
        line_add_text: g[TextSubtle],
        line_del_text: r[TextSubtle],
        line_add_word_bg: g[ElementActive],
        line_del_word_bg: r[ElementActive],

        search_match_bg: Color::rgba(
            y[ElementActive].r,
            y[ElementActive].g,
            y[ElementActive].b,
            120,
        ),
        search_match_active_bg: Color::rgba(y[Border].r, y[Border].g, y[Border].b, 200),

        syntax_keyword: purple[TextSubtle],
        syntax_string: g[TextSubtle],
        syntax_comment: n[Solid],
        syntax_function: b[TextSubtle],
        syntax_type: teal[TextSubtle],
        syntax_number: orange[TextSubtle],
        syntax_property: r[TextSubtle],
        syntax_operator: n[TextSubtle],
    }
}

fn default_sans_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Segoe UI"
    } else if cfg!(target_os = "macos") {
        "Arial"
    } else {
        "DejaVu Sans"
    }
}

fn default_mono_family() -> &'static str {
    if cfg!(target_os = "windows") {
        "Consolas"
    } else if cfg!(target_os = "macos") {
        "Menlo"
    } else {
        "DejaVu Sans Mono"
    }
}

#[cfg(test)]
mod tests {
    use super::{Theme, ThemeFamily, ThemeMode};

    // Catches a theme switch reshaping text: a new theme with the same
    // metrics must repaint in its colors from the layouts already cached.
    #[test]
    fn switching_themes_repaints_from_cached_text_layouts() {
        use crate::element::{ElementContext, IntoAnyElement, render_element, text};
        use quark::reactive::SignalStore;
        use quark_render::{Primitive, Scene};

        let harbor = ThemeFamily::from_json(
            r##"{"name": "Harbor", "dark": {"colors": {"text": "#59c2ff"}}}"##,
        )
        .expect("valid theme")
        .theme(ThemeMode::Dark);
        let mut system = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let signals = SignalStore::new();
        let mut paint = |theme: &Theme| {
            layouts.begin_frame();
            let mut cx = ElementContext::new(theme, 1.0, &mut system, &mut layouts, None, &signals);
            let mut root = text("switch me live").into_any();
            let mut scene = Scene::default();
            render_element(&mut root, &mut scene, &mut cx, 300.0, 100.0);
            let color = scene.primitives.iter().find_map(|p| match p {
                Primitive::TextRun(run) => Some(run.color),
                _ => None,
            });
            (color, layouts.stats().misses)
        };
        let (color, shaped) = paint(&Theme::default_dark());
        assert_eq!(color, Some(Theme::default_dark().colors.text));
        assert_eq!(
            paint(&harbor),
            (Some(harbor.colors.text), shaped),
            "repainted in the new color without reshaping"
        );
    }

    #[test]
    fn dark_focus_border_is_blue_accent() {
        let theme = Theme::default_dark();
        // focus_border comes from blue scale step 9 — should be a vivid blue.
        let c = theme.colors.focus_border;
        assert!(
            c.b > c.r && c.b > c.g,
            "focus_border should be distinctly blue"
        );
        assert!(c.a == 255);
    }

    #[test]
    fn mode_factory_returns_light_theme() {
        let theme = Theme::for_mode(ThemeMode::Light);
        assert_eq!(theme.mode, ThemeMode::Light);
        // Light background should be very bright.
        let bg = theme.colors.background;
        assert!(bg.r > 230 && bg.g > 230 && bg.b > 230);
    }

    #[test]
    fn dark_neutral_steps_are_distinguishable() {
        let theme = Theme::default_dark();
        // Each surface tier should be brighter than the one below.
        let tiers = [
            theme.colors.background,
            theme.colors.editor_surface,
            theme.colors.surface,
            theme.colors.elevated_surface,
        ];
        for i in 1..tiers.len() {
            let prev = tiers[i - 1].r as u16 + tiers[i - 1].g as u16 + tiers[i - 1].b as u16;
            let curr = tiers[i].r as u16 + tiers[i].g as u16 + tiers[i].b as u16;
            assert!(
                curr >= prev,
                "surface tier {} should be >= tier {} ({} vs {})",
                i,
                i - 1,
                curr,
                prev,
            );
        }
    }
}
