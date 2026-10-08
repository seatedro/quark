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
        pub struct $name:ident: $ty:ty { $($(#[$field_meta:meta])* $field:ident,)* }
    ) => {
        $(#[$meta])*
        pub struct $name {
            $($(#[$field_meta])* pub $field: $ty,)*
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

/// How strongly a theme separates text and controls from what is behind
/// them. Independent of [`ThemeMode`]: a high-contrast theme is still
/// light or dark.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeContrast {
    #[default]
    Standard,
    /// Opaque colors with at least 7:1 contrast for text and 3:1 for
    /// control boundaries and focus rings (WCAG 1.4.6 and 1.4.11).
    High,
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
        /// Marks and text drawn over `accent` or `accent_strong`: a
        /// checkbox's check, a switch's thumb, a filled button's label.
        on_accent,
        selection_bg,
        background,
        surface,
        editor_surface,
        elevated_surface,
        modal_surface,
        overlay_scrim,
        border,
        /// The outline that marks out a control with no fill of its own:
        /// an unchecked radio or checkbox. At least 3:1 against what it sits
        /// on (WCAG 1.4.11), unlike `border`, which separates surfaces.
        control_border,
        border_variant,
        focus_border,
        text,
        text_muted,
        /// Hint text in an empty field. Opaque, so its contrast does not
        /// depend on how the renderer blends: at least 4.5:1 over the
        /// field's fills (`element_background`, and `surface` while
        /// focused).
        placeholder,
        /// Text and marks of a disabled control. In high contrast it stays
        /// readable (4.5:1) but clearly lighter than `text` and
        /// `text_muted`, which are too close there to tell states apart.
        text_disabled,
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

/// Optional sizes for one kind of control, in logical points at 100% zoom.
///
/// Components multiply a set value by [`ThemeMetrics::ui_scale`] once, as
/// they do their own defaults, so a recipe can ask for 13-point control
/// text while `ui_font_size` stays at the 16-point zoom anchor. `None`
/// keeps the component's default.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ControlMetrics {
    pub height: Option<f32>,
    pub radius: Option<f32>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub gap: Option<f32>,
    pub font_size: Option<f32>,
    pub icon_size: Option<f32>,
}

impl ControlMetrics {
    /// Each field of `self`, or of `fallback` where `self` has none.
    pub fn or(self, fallback: Self) -> Self {
        Self {
            height: self.height.or(fallback.height),
            radius: self.radius.or(fallback.radius),
            padding_x: self.padding_x.or(fallback.padding_x),
            padding_y: self.padding_y.or(fallback.padding_y),
            gap: self.gap.or(fallback.gap),
            font_size: self.font_size.or(fallback.font_size),
            icon_size: self.icon_size.or(fallback.icon_size),
        }
    }
}

/// One black shadow layer under a floating surface, in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Elevation {
    pub offset_y: f32,
    pub blur: f32,
    /// Opacity of the black, 0 to 255.
    pub alpha: u8,
}

/// Optional sizes for a floating surface: popover, tooltip, toast, or
/// modal. Same units and scaling as [`ControlMetrics`]; a set `shadow`
/// replaces the component's preset with one layer.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct SurfaceMetrics {
    pub radius: Option<f32>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub gap: Option<f32>,
    pub font_size: Option<f32>,
    pub title_font_size: Option<f32>,
    pub shadow: Option<Elevation>,
}

/// Per-component overrides an app sets once on its theme, so every
/// instance in every window follows one recipe. The default overrides
/// nothing: components look as they always have.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ComponentMetrics {
    /// Buttons of `ButtonSize::Default`. `height` sets buttons without a
    /// fixed size; a button's own `fixed_size` keeps its square.
    pub button: ControlMetrics,
    /// Buttons of `ButtonSize::Compact`.
    pub button_compact: ControlMetrics,
    /// The select trigger and the combobox field.
    pub select: ControlMetrics,
    /// Rows of select and combobox lists.
    pub option: ControlMetrics,
    /// Popover panels: menus and select lists.
    pub popover: SurfaceMetrics,
    pub tooltip: SurfaceMetrics,
    pub toast: SurfaceMetrics,
    pub modal: SurfaceMetrics,
    /// Skeleton placeholders: `height` is one line, `gap` between lines,
    /// `padding_x` and `padding_y` around them.
    pub skeleton: ControlMetrics,
    /// Form fields: `font_size` for the label, `gap` between label, control,
    /// and message.
    pub field: ControlMetrics,
}

/// `value`, in unscaled points, at `scale` and rounded to whole points; or
/// `default`, which is already scaled.
pub fn scaled_or(value: Option<f32>, scale: f32, default: f32) -> f32 {
    value.map_or(default, |v| (v * scale).round())
}

#[derive(Debug, Clone, PartialEq)]
pub struct Theme {
    pub mode: ThemeMode,
    pub contrast: ThemeContrast,
    pub sans_family: &'static str,
    pub mono_family: &'static str,
    pub colors: ThemeColors,
    pub metrics: ThemeMetrics,
    /// Optional per-component sizes; see [`ComponentMetrics`].
    pub components: ComponentMetrics,
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

    /// Quark's own theme for `mode` at `contrast`.
    pub fn for_mode_and_contrast(mode: ThemeMode, contrast: ThemeContrast) -> Self {
        match contrast {
            ThemeContrast::Standard => Self::for_mode(mode),
            ThemeContrast::High => match mode {
                ThemeMode::Dark => Self::high_contrast_dark(),
                ThemeMode::Light => Self::high_contrast_light(),
            },
        }
    }

    pub fn with_ui_scale(mut self, scale: f32) -> Self {
        self.metrics = self.metrics.scaled(scale);
        self
    }

    /// Switch to quark's theme of the other mode, keeping the contrast.
    pub fn toggle_mode(&mut self) {
        let mode = match self.mode {
            ThemeMode::Dark => ThemeMode::Light,
            ThemeMode::Light => ThemeMode::Dark,
        };
        *self = Self::for_mode_and_contrast(mode, self.contrast);
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
            contrast: ThemeContrast::Standard,
            sans_family: default_sans_family(),
            mono_family: default_mono_family(),
            colors: dark_colors(&n, &blue, &red, &green, &yellow, &purple, &teal, &orange),
            metrics: default_metrics(),
            components: ComponentMetrics::default(),
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
            contrast: ThemeContrast::Standard,
            sans_family: default_sans_family(),
            mono_family: default_mono_family(),
            colors: light_colors(&n, &blue, &red, &green, &yellow, &purple, &teal, &orange),
            metrics: default_metrics(),
            components: ComponentMetrics::default(),
            reduced_motion: false,
        }
    }

    /// Quark's dark theme at [`ThemeContrast::High`]: white text on black,
    /// a light accent with black marks over it, and a yellow focus ring.
    pub fn high_contrast_dark() -> Self {
        Self {
            mode: ThemeMode::Dark,
            contrast: ThemeContrast::High,
            sans_family: default_sans_family(),
            mono_family: default_mono_family(),
            colors: high_contrast_dark_colors(),
            metrics: default_metrics(),
            components: ComponentMetrics::default(),
            reduced_motion: false,
        }
    }

    /// Quark's light theme at [`ThemeContrast::High`]: black text on white,
    /// a dark accent with white marks over it, and a violet focus ring.
    pub fn high_contrast_light() -> Self {
        Self {
            mode: ThemeMode::Light,
            contrast: ThemeContrast::High,
            sans_family: default_sans_family(),
            mono_family: default_mono_family(),
            colors: high_contrast_light_colors(),
            metrics: default_metrics(),
            components: ComponentMetrics::default(),
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
        control_border: n[Solid],
        border_variant: n[ElementHover],
        focus_border: b[Solid],
        empty_state_border: n[ElementActive],

        // Text — readable hierarchy
        text_strong: Color::rgba(240, 240, 245, 255),
        text: n[Text],
        text_muted: n[Solid],
        placeholder: n[TextSubtle],
        text_disabled: n[Solid],
        text_accent: b[TextSubtle],
        icon: n[TextSubtle],
        gutter_text: n[BorderStrong],

        // Accent — vibrant blue
        accent: b[Solid],
        accent_strong: b[TextSubtle],
        on_accent: Color::rgba(255, 255, 255, 255),
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
        control_border: n[Solid],
        border_variant: n[BorderSubtle],
        focus_border: b[Solid],
        empty_state_border: n[Border],

        // Text (darkest in light mode)
        text_strong: n[TextStrong],
        text: n[TextStrong],
        text_muted: n[TextSubtle],
        placeholder: n[TextSubtle],
        text_disabled: n[TextSubtle],
        text_accent: b[TextSubtle],
        icon: n[TextSubtle],
        gutter_text: n[Solid],

        // Accent
        accent: b[Solid],
        accent_strong: b[TextSubtle],
        on_accent: Color::rgba(255, 255, 255, 255),
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

/// The WCAG contrast ratio, 1 to 21, between `foreground` drawn over an
/// opaque `background` and that background. A translucent foreground is
/// composited first, so the ratio is the one a reader sees.
pub fn contrast_ratio(foreground: Color, background: Color) -> f32 {
    let linear = |c: u8| {
        let c = f32::from(c) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    // Blended in linear light, as the renderer's sRGB surface blends.
    let alpha = f32::from(foreground.a) / 255.0;
    let over = |fg: u8, bg: u8| linear(fg) * alpha + linear(bg) * (1.0 - alpha);
    let luminance = |r, g, b| 0.2126 * r + 0.7152 * g + 0.0722 * b;
    let fg = luminance(
        over(foreground.r, background.r),
        over(foreground.g, background.g),
        over(foreground.b, background.b),
    );
    let bg = luminance(
        linear(background.r),
        linear(background.g),
        linear(background.b),
    );
    (fg.max(bg) + 0.05) / (fg.min(bg) + 0.05)
}

/// `0xrrggbb` as an opaque color.
const fn rgb(hex: u32) -> Color {
    rgba(hex, 0xff)
}

const fn rgba(hex: u32, alpha: u8) -> Color {
    Color::rgba((hex >> 16) as u8, (hex >> 8) as u8, hex as u8, alpha)
}

// The high-contrast palettes are picked by hand rather than from the
// perceptual scales: every pair a control paints must clear 7:1 (text) or
// 3:1 (boundaries, marks, focus), as the tests below check. Text over a selected or hovered fill is the binding
// constraint: white text at 7:1 caps a dark fill's luminance at 0.1, so
// selection fills stay subtle and focus rings carry keyboard state. The
// selection that text inputs and diffs paint is `accent` at
// `Alpha::SOFT`, so the accent's lightness also bounds the syntax colors.
fn high_contrast_dark_colors() -> ThemeColors {
    ThemeColors {
        app_bg: rgb(0x000000),
        canvas: rgb(0x000000),
        panel: rgb(0x0a0a0a),
        panel_strong: rgb(0x141414),
        border_soft: rgb(0xa3a3a3),
        text_strong: rgb(0xffffff),
        accent: rgb(0x9ccfff),
        accent_strong: rgb(0xc4e2ff),
        on_accent: rgb(0x000000),
        selection_bg: rgb(0x14375e),
        background: rgb(0x000000),
        surface: rgb(0x0a0a0a),
        editor_surface: rgb(0x000000),
        elevated_surface: rgb(0x141414),
        modal_surface: rgb(0x141414),
        overlay_scrim: rgba(0x000000, 0xcc),
        border: rgb(0xbdbdbd),
        control_border: rgb(0xbdbdbd),
        border_variant: rgb(0xa3a3a3),
        focus_border: rgb(0xffd60a),
        text: rgb(0xffffff),
        text_muted: rgb(0xe0e0e0),
        placeholder: rgb(0xacacac),
        text_disabled: rgb(0x949494),
        text_accent: rgb(0xb0d9ff),
        icon: rgb(0xe6e6e6),
        element_background: rgb(0x0f0f0f),
        element_hover: rgb(0x262626),
        element_active: rgb(0x333333),
        element_selected: rgb(0x14375e),
        ghost_element_hover: rgb(0x102b4d),
        ghost_element_active: rgb(0x14375e),
        ghost_element_selected: rgb(0x14375e),
        title_bar_background: rgb(0x0a0a0a),
        status_bar_background: rgb(0x0a0a0a),
        sidebar_background: rgb(0x0a0a0a),
        sidebar_row_hover: rgb(0x102b4d),
        sidebar_row_selected: rgb(0x14375e),
        empty_state_background: rgb(0x0a0a0a),
        empty_state_border: rgb(0xa3a3a3),
        scrollbar_thumb: rgb(0xbdbdbd),
        status_info: rgb(0xb0d9ff),
        status_warning: rgb(0xffd60a),
        status_error: rgb(0xffc2c2),
        line_add: rgb(0x001a08),
        line_del: rgb(0x260004),
        line_modified: rgb(0x0a1a2e),
        gutter_bg: rgb(0x000000),
        gutter_text: rgb(0xcccccc),
        file_header_bg: rgb(0x1a1a1a),
        hunk_header_bg: rgb(0x0a1a2e),
        line_add_text: rgb(0x8cffa8),
        line_del_text: rgb(0xffb0b0),
        line_add_word_bg: rgb(0x003310),
        line_del_word_bg: rgb(0x470008),
        hover_overlay: rgb(0x102b4d),
        search_match_bg: rgb(0x2e2400),
        search_match_active_bg: rgb(0x4a3a00),
        syntax_keyword: rgb(0xf2d9ff),
        syntax_string: rgb(0xc7ffd0),
        syntax_comment: rgb(0xe3e3e3),
        syntax_function: rgb(0xc7e5ff),
        syntax_type: rgb(0xa8fff9),
        syntax_number: rgb(0xffdcb3),
        syntax_property: rgb(0xffdbdb),
        syntax_operator: rgb(0xf0f0f0),
    }
}

fn high_contrast_light_colors() -> ThemeColors {
    ThemeColors {
        app_bg: rgb(0xffffff),
        canvas: rgb(0xffffff),
        panel: rgb(0xfafafa),
        panel_strong: rgb(0xf2f2f2),
        border_soft: rgb(0x6b6b6b),
        text_strong: rgb(0x000000),
        accent: rgb(0x00317f),
        accent_strong: rgb(0x002152),
        on_accent: rgb(0xffffff),
        selection_bg: rgb(0xbfd9ff),
        background: rgb(0xffffff),
        surface: rgb(0xfafafa),
        editor_surface: rgb(0xffffff),
        elevated_surface: rgb(0xffffff),
        modal_surface: rgb(0xffffff),
        overlay_scrim: rgba(0x000000, 0x99),
        border: rgb(0x4d4d4d),
        control_border: rgb(0x4d4d4d),
        border_variant: rgb(0x6b6b6b),
        focus_border: rgb(0x7a00cc),
        text: rgb(0x000000),
        text_muted: rgb(0x2e2e2e),
        placeholder: rgb(0x595959),
        text_disabled: rgb(0x646464),
        text_accent: rgb(0x00317f),
        icon: rgb(0x1f1f1f),
        element_background: rgb(0xf0f0f0),
        element_hover: rgb(0xe8e8e8),
        element_active: rgb(0xdedede),
        element_selected: rgb(0xbfd9ff),
        ghost_element_hover: rgb(0xe6f0ff),
        ghost_element_active: rgb(0xbfd9ff),
        ghost_element_selected: rgb(0xbfd9ff),
        title_bar_background: rgb(0xf5f5f5),
        status_bar_background: rgb(0xf5f5f5),
        sidebar_background: rgb(0xfafafa),
        sidebar_row_hover: rgb(0xe6f0ff),
        sidebar_row_selected: rgb(0xbfd9ff),
        empty_state_background: rgb(0xfafafa),
        empty_state_border: rgb(0x6b6b6b),
        scrollbar_thumb: rgb(0x4d4d4d),
        status_info: rgb(0x00317f),
        status_warning: rgb(0x422c00),
        status_error: rgb(0x7a0000),
        line_add: rgb(0xddfbe3),
        line_del: rgb(0xffe0e0),
        line_modified: rgb(0xe6f0ff),
        gutter_bg: rgb(0xffffff),
        gutter_text: rgb(0x333333),
        file_header_bg: rgb(0xf0f0f0),
        hunk_header_bg: rgb(0xe6f0ff),
        line_add_text: rgb(0x004d14),
        line_del_text: rgb(0x800000),
        line_add_word_bg: rgb(0xb8f0c4),
        line_del_word_bg: rgb(0xffc7c7),
        hover_overlay: rgb(0xe6f0ff),
        search_match_bg: rgb(0xffeb99),
        search_match_active_bg: rgb(0xffcf40),
        syntax_keyword: rgb(0x420070),
        syntax_string: rgb(0x002e0f),
        syntax_comment: rgb(0x262626),
        syntax_function: rgb(0x002255),
        syntax_type: rgb(0x002e2e),
        syntax_number: rgb(0x451e00),
        syntax_property: rgb(0x5c0000),
        syntax_operator: rgb(0x1a1a1a),
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
    use super::{Color, Theme, ThemeColors, ThemeFamily, ThemeMode, contrast_ratio};

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

    #[test]
    fn contrast_ratio_matches_wcag_reference_values() {
        let white = Color::rgba(255, 255, 255, 255);
        let cases = [
            (Color::rgba(0, 0, 0, 255), white, 21.0),
            (Color::rgba(0x77, 0x77, 0x77, 255), white, 4.48),
            (Color::rgba(0, 0, 255, 255), white, 8.59),
            // Half-transparent black over white leaves 127/255 of white's
            // linear luminance: 1.05 / (127 / 255 + 0.05). Blending the
            // gamma-encoded channels instead gives #7f7f7f and 4.0.
            (Color::rgba(0, 0, 0, 128), white, 1.916),
            (white, white, 1.0),
        ];
        for (fg, bg, expected) in cases {
            let ratio = contrast_ratio(fg, bg);
            assert!((ratio - expected).abs() < 0.01, "{fg:?} on {bg:?}: {ratio}");
        }
    }

    // Catches a high-contrast token edited below its target for a pair a
    // control paints: text, code, and diff text at 7:1, marks over the
    // accent at 7:1, placeholders at 4.5:1, and borders, focus rings, and
    // accent marks at 3:1.
    #[test]
    fn high_contrast_pairs_meet_their_targets() {
        const SURFACES: &[&str] = &[
            "background",
            "canvas",
            "app_bg",
            "surface",
            "panel",
            "panel_strong",
            "editor_surface",
            "elevated_surface",
            "modal_surface",
            "title_bar_background",
            "status_bar_background",
            "sidebar_background",
            "empty_state_background",
            "element_background",
            "element_hover",
            "element_active",
            "element_selected",
            "ghost_element_hover",
            "ghost_element_active",
            "ghost_element_selected",
            "sidebar_row_hover",
            "sidebar_row_selected",
            "selection_bg",
            "hover_overlay",
            "file_header_bg",
            "hunk_header_bg",
        ];
        const TEXT: &[&str] = &[
            "text",
            "text_strong",
            "text_muted",
            "text_accent",
            "accent",
            "icon",
            "status_info",
            "status_warning",
            "status_error",
        ];
        const CODE: &[&str] = &[
            "text",
            "syntax_keyword",
            "syntax_string",
            "syntax_comment",
            "syntax_function",
            "syntax_type",
            "syntax_number",
            "syntax_property",
            "syntax_operator",
        ];
        const CODE_BACKGROUNDS: &[&str] = &[
            "editor_surface",
            "line_add",
            "line_del",
            "line_add_word_bg",
            "line_del_word_bg",
            "search_match_bg",
            "search_match_active_bg",
            "ghost_element_hover",
        ];
        const GUTTER: &[&str] = &[
            "gutter_text",
            "line_add_text",
            "line_del_text",
            "text_muted",
        ];
        const GUTTER_BACKGROUNDS: &[&str] = &[
            "gutter_bg",
            "line_add",
            "line_del",
            "file_header_bg",
            "hunk_header_bg",
            "editor_surface",
        ];
        const BOUNDARIES: &[&str] = &[
            "focus_border",
            "border",
            "control_border",
            "border_variant",
            "border_soft",
            "empty_state_border",
            "scrollbar_thumb",
            "accent",
        ];
        const CONTROL_BACKGROUNDS: &[&str] = &[
            "background",
            "surface",
            "panel",
            "element_background",
            "element_hover",
            "elevated_surface",
            "modal_surface",
            "editor_surface",
            "sidebar_background",
            "title_bar_background",
            "status_bar_background",
        ];
        let groups: &[(&[&str], &[&str], f32)] = &[
            (TEXT, SURFACES, 7.0),
            (CODE, CODE_BACKGROUNDS, 7.0),
            (GUTTER, GUTTER_BACKGROUNDS, 7.0),
            (&["on_accent"], &["accent", "accent_strong"], 7.0),
            (&["placeholder", "text_disabled"], CONTROL_BACKGROUNDS, 4.5),
            (BOUNDARIES, CONTROL_BACKGROUNDS, 3.0),
        ];
        for theme in [Theme::high_contrast_dark(), Theme::high_contrast_light()] {
            let c = |name: &str| theme.colors.get(name).expect(name);
            let mut failures = Vec::new();
            let mut check = |fg: &str, bg: &str, background: Color, min: f32| {
                let ratio = contrast_ratio(c(fg), background);
                if ratio < min {
                    failures.push(format!("{fg} on {bg}: {ratio:.2} < {min}"));
                }
            };
            for (fgs, bgs, min) in groups {
                for fg in *fgs {
                    for bg in *bgs {
                        check(fg, bg, c(bg), *min);
                    }
                }
            }
            // Text inputs, documents, and the diff view select with the
            // accent at `Alpha::SOFT`: plain text in fields, code in
            // editors and documents.
            let selection = c("accent").with_alpha(crate::design::Alpha::SOFT);
            for (fgs, bg) in [
                (&["text"][..], "surface"),
                (&["text"][..], "element_background"),
                (CODE, "editor_surface"),
                (CODE, "background"),
            ] {
                let composited = composite(selection, c(bg));
                for fg in fgs {
                    check(fg, &format!("selection over {bg}"), composited, 7.0);
                }
            }
            assert!(failures.is_empty(), "{:?}: {failures:#?}", theme.mode);
        }
    }

    // Catches disabled controls looking enabled in high contrast: the light
    // theme's disabled radio (#2E2E2E, text_muted) was hard to tell from an
    // enabled one (#000).
    #[test]
    fn high_contrast_disabled_text_stands_apart_from_enabled_text() {
        for theme in [Theme::high_contrast_dark(), Theme::high_contrast_light()] {
            let c = &theme.colors;
            for (name, enabled) in [("text", c.text), ("text_muted", c.text_muted)] {
                let ratio = contrast_ratio(c.text_disabled, enabled);
                assert!(
                    ratio >= 2.0,
                    "{:?}: text_disabled vs {name}: {ratio:.2}",
                    theme.mode
                );
            }
        }
    }

    // Catches an unchecked radio or checkbox fading into what holds it: the
    // dark theme drew their outlines in `border`, about 1.4:1.
    #[test]
    fn standard_control_outlines_stand_out_from_their_backgrounds() {
        for theme in [Theme::default_dark(), Theme::default_light()] {
            let c = &theme.colors;
            for bg in [
                "background",
                "surface",
                "panel",
                "element_background",
                "elevated_surface",
                "modal_surface",
                "editor_surface",
                "sidebar_background",
            ] {
                let ratio = contrast_ratio(c.control_border, c.get(bg).expect(bg));
                assert!(
                    ratio >= 3.0,
                    "{:?}: control_border on {bg}: {ratio:.2}",
                    theme.mode
                );
            }
        }
    }

    // Catches a standard theme's placeholder dropping below WCAG AA over
    // the fills a text field paints behind it, as the old translucent
    // text_muted did (about 2:1 once blended).
    #[test]
    fn standard_placeholders_are_readable_in_fields() {
        for theme in [Theme::default_dark(), Theme::default_light()] {
            let c = &theme.colors;
            for (name, fill) in [
                ("element_background", c.element_background),
                ("surface", c.surface),
            ] {
                let ratio = contrast_ratio(c.placeholder, fill);
                assert!(
                    c.placeholder.a == 255 && ratio >= 4.5,
                    "{:?}: placeholder on {name}: {ratio:.2}",
                    theme.mode
                );
            }
        }
    }

    // Catches a high-contrast token left translucent, which would make its
    // contrast depend on whatever sits behind it. The scrim dims a window
    // under a modal and is translucent by design.
    #[test]
    fn high_contrast_colors_are_opaque() {
        for theme in [Theme::high_contrast_dark(), Theme::high_contrast_light()] {
            let translucent: Vec<&str> = ThemeColors::TOKENS
                .iter()
                .copied()
                .filter(|&name| name != "overlay_scrim")
                .filter(|&name| theme.colors.get(name).is_some_and(|c| c.a != 255))
                .collect();
            assert!(translucent.is_empty(), "{:?}: {translucent:?}", theme.mode);
        }
    }

    fn composite(fg: Color, bg: Color) -> Color {
        let a = f32::from(fg.a) / 255.0;
        let mix = |f: u8, b: u8| (f32::from(f) * a + f32::from(b) * (1.0 - a)).round() as u8;
        Color::rgba(mix(fg.r, bg.r), mix(fg.g, bg.g), mix(fg.b, bg.b), 255)
    }
}
