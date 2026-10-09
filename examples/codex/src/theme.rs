//! Colors and type, measured from the live Codex captures (dark first,
//! then light). The app reads [`Pal`] directly; the quark [`Theme`] it
//! builds from the same values only feeds the primitives that read theme
//! tokens themselves (the editor's caret and selection, the terminal, the
//! scrollbars).
//!
//! Codex derives its palette from three tokens (accent, background, ink)
//! plus a contrast slider; this demo ships the two resolved palettes.

use quark_app::quark_ui::theme::{Color, Theme, ThemeMode};

const fn hex(v: u32) -> Color {
    Color::rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, 255)
}

const fn hexa(v: u32, a: u8) -> Color {
    Color::rgba((v >> 16) as u8, (v >> 8) as u8, v as u8, a)
}

/// Every color a surface draws with.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pal {
    pub mode: ThemeMode,
    /// Main pane.
    pub bg: Color,
    /// The sidebar's material. Codex uses macOS vibrancy here; a window
    /// capture renders it flat, which is the value used.
    pub sidebar: Color,
    pub sidebar_text: Color,
    pub sidebar_muted: Color,
    pub sidebar_meta: Color,
    pub row_selected: Color,
    pub row_hover: Color,
    pub text: Color,
    pub text_soft: Color,
    pub muted: Color,
    pub faint: Color,
    pub placeholder: Color,
    pub icon: Color,
    pub icon_faint: Color,
    pub composer: Color,
    pub composer_rim_top: Color,
    pub composer_rim_bottom: Color,
    pub composer_border: Color,
    pub tray: Color,
    pub menu: Color,
    pub menu_border: Color,
    pub menu_hi: Color,
    pub menu_title: Color,
    pub menu_desc: Color,
    pub menu_header: Color,
    pub menu_check: Color,
    pub bubble: Color,
    pub notice: Color,
    pub notice_border: Color,
    pub hairline: Color,
    pub divider_text: Color,
    pub card_border: Color,
    pub card_title_dim: Color,
    pub card_desc_dim: Color,
    pub card_desc: Color,
    pub banner: Color,
    pub accent: Color,
    pub error: Color,
    pub success: Color,
    pub send_idle: Color,
    pub send_idle_glyph: Color,
    pub send_active: Color,
    pub send_active_glyph: Color,
    pub panel_tile: Color,
    pub file_header: Color,
    pub add_bar: Color,
    pub add_gutter: Color,
    pub add_code: Color,
    pub add_num: Color,
    pub del_bar: Color,
    pub del_gutter: Color,
    pub del_code: Color,
    pub del_num: Color,
    pub line_num: Color,
    pub code: Color,
    pub syn_comment: Color,
    pub syn_keyword: Color,
    pub syn_function: Color,
    pub syn_ident: Color,
    pub syn_number: Color,
    pub syn_string: Color,
    pub syn_punct: Color,
    pub float_pill: Color,
    pub float_text: Color,
    pub toggle_on: Color,
    pub settings_card: Color,
    pub settings_border: Color,
    pub kbd: Color,
    pub shadow: Color,
    pub avatar: Color,
    pub scrim: Color,
    /// The window frame around the inset cards (title bar, rail).
    pub frame: Color,
    /// Hairline around the inset sidebar and main cards.
    pub frame_border: Color,
    pub rail_icon: Color,
    pub rail_tile: Color,
    pub bubble_text: Color,
    pub chip: Color,
    pub shell: Color,
    pub shell_border: Color,
    pub change_card: Color,
    pub change_border: Color,
    pub tile: Color,
    pub link: Color,
    pub word_add: Color,
    pub word_del: Color,
    pub fold: Color,
    pub orange: Color,
    pub badge: Color,
    pub badge_text: Color,
}

pub const DARK: Pal = Pal {
    mode: ThemeMode::Dark,
    bg: hex(0x181818),
    sidebar: hex(0x222222),
    sidebar_text: hex(0xdcdcdc),
    sidebar_muted: hex(0x8f8f8f),
    sidebar_meta: hex(0xa0a0a0),
    row_selected: hex(0x333333),
    row_hover: hex(0x2c2c2c),
    text: hex(0xffffff),
    text_soft: hex(0xdddddd),
    muted: hex(0x949494),
    faint: hex(0x7a7a7a),
    placeholder: hex(0x7a7a7a),
    icon: hex(0x8a8a8a),
    icon_faint: hex(0x5a5a5a),
    composer: hex(0x363636),
    composer_rim_top: hex(0x2f2f2f),
    composer_rim_bottom: hex(0x353535),
    composer_border: hex(0x3a3a3a),
    tray: hex(0x282828),
    menu: hexa(0x2c2c2c, 250),
    menu_border: hex(0x3d3d3d),
    menu_hi: hex(0x3c3c3c),
    menu_title: hex(0xdddddd),
    menu_desc: hex(0x939393),
    menu_header: hex(0x8e8e8e),
    menu_check: hex(0xcacaca),
    bubble: hex(0x2f2f2f),
    notice: hex(0x2c2c2c),
    notice_border: hex(0x2c2c2c),
    hairline: hex(0x2b2b2b),
    divider_text: hex(0xa0a0a0),
    card_border: hex(0x2b2b2b),
    card_title_dim: hex(0x5d5d5d),
    card_desc_dim: hex(0x3b3b3b),
    card_desc: hex(0x8b8b8b),
    banner: hex(0x252525),
    accent: hex(0x339cff),
    error: hex(0xff6764),
    success: hex(0x40c977),
    send_idle: hex(0xffffff),
    send_idle_glyph: hex(0x111111),
    send_active: hex(0xffffff),
    send_active_glyph: hex(0x111111),
    panel_tile: hex(0x2f2f2f),
    file_header: hex(0x222222),
    add_bar: hex(0x40c977),
    add_gutter: hex(0x2d3d32),
    add_code: hex(0x2d3d32),
    add_num: hex(0x3fc776),
    del_bar: hex(0xc93935),
    del_gutter: hex(0x48302b),
    del_code: hex(0x48302b),
    del_num: hex(0xf8413d),
    line_num: hex(0xa1a1a1),
    code: hex(0xd6d6d6),
    syn_comment: hex(0x959595),
    syn_keyword: hex(0xe56e6f),
    syn_function: hex(0xb07ce8),
    syn_ident: hex(0xdb8544),
    syn_number: hex(0x5eb8f0),
    syn_string: hex(0x8fce7c),
    syn_punct: hex(0xd6d6d6),
    float_pill: hex(0x2a2a2a),
    float_text: hex(0x8e8e8e),
    toggle_on: hex(0x339cff),
    settings_card: hex(0x1d1d1d),
    settings_border: hex(0x2e2e2e),
    kbd: hex(0x8a8a8a),
    shadow: hexa(0x000000, 150),
    avatar: hex(0x838f90),
    scrim: hexa(0x000000, 0),
    frame: hex(0x343434),
    frame_border: hex(0x3a3a3a),
    rail_icon: hex(0x9a9a9a),
    rail_tile: hex(0x454545),
    bubble_text: hex(0xffffff),
    chip: hex(0x303030),
    shell: hex(0x2a2a2a),
    shell_border: hex(0x3a3a3a),
    change_card: hex(0x222222),
    change_border: hex(0x2a2a2a),
    tile: hex(0x151515),
    link: hex(0x6fa6f2),
    word_add: hex(0x3f6a4b),
    word_del: hex(0x6e3a32),
    fold: hex(0x2f2f2f),
    orange: hex(0xf08a3c),
    badge: hex(0x203048),
    badge_text: hex(0x6fa6f2),
};

pub const LIGHT: Pal = Pal {
    mode: ThemeMode::Light,
    bg: hex(0xffffff),
    sidebar: hex(0xfbfbfb),
    sidebar_text: hex(0x202020),
    sidebar_muted: hex(0x7f8081),
    sidebar_meta: hex(0x8b8b8b),
    row_selected: hex(0xeeefef),
    row_hover: hex(0xf2f2f2),
    text: hex(0x1a1c1f),
    text_soft: hex(0x303030),
    muted: hex(0x7b7b7b),
    faint: hex(0x9a9a9a),
    placeholder: hex(0xa3a3a3),
    icon: hex(0x7d7d7d),
    icon_faint: hex(0xc4c4c4),
    composer: hex(0xffffff),
    composer_rim_top: hex(0xeaeaea),
    composer_rim_bottom: hex(0xdddddd),
    composer_border: hex(0xe7e7e7),
    tray: hex(0xf4f4f4),
    menu: hexa(0xffffff, 250),
    menu_border: hex(0xe3e3e3),
    menu_hi: hex(0xf0f0f0),
    menu_title: hex(0x202020),
    menu_desc: hex(0x8b8b8b),
    menu_header: hex(0x8b8b8b),
    menu_check: hex(0x404040),
    bubble: hex(0x000000),
    notice: hex(0xffffff),
    notice_border: hex(0xe3e3e4),
    hairline: hex(0xe6e6e6),
    divider_text: hex(0x7b7b7b),
    card_border: hex(0xe6e6e6),
    card_title_dim: hex(0xb5b5b5),
    card_desc_dim: hex(0xcfcfcf),
    card_desc: hex(0x7b7b7b),
    banner: hex(0xf6f6f6),
    accent: hex(0x339cff),
    error: hex(0xe5534b),
    success: hex(0x22a35a),
    send_idle: hex(0x000000),
    send_idle_glyph: hex(0xffffff),
    send_active: hex(0x111111),
    send_active_glyph: hex(0xffffff),
    panel_tile: hex(0xececec),
    file_header: hex(0xf7f7f7),
    add_bar: hex(0x00a240),
    add_gutter: hex(0xd8e3d8),
    add_code: hex(0xd8e3d8),
    add_num: hex(0x1f9d4b),
    del_bar: hex(0xe0524b),
    del_gutter: hex(0xe7d4d1),
    del_code: hex(0xe7d4d1),
    del_num: hex(0xd8443d),
    line_num: hex(0x9a9a9a),
    code: hex(0x2b2b2b),
    syn_comment: hex(0x8f8f8f),
    syn_keyword: hex(0xd23f45),
    syn_function: hex(0x8250df),
    syn_ident: hex(0xb8611f),
    syn_number: hex(0x0f6fc6),
    syn_string: hex(0x2f8a3a),
    syn_punct: hex(0x2b2b2b),
    float_pill: hex(0xffffff),
    float_text: hex(0x7b7b7b),
    toggle_on: hex(0x339cff),
    settings_card: hex(0xffffff),
    settings_border: hex(0xe6e6e6),
    kbd: hex(0x9a9a9a),
    shadow: hexa(0x000000, 40),
    avatar: hex(0x838f90),
    scrim: hexa(0x000000, 0),
    frame: hex(0xf2f2f2),
    frame_border: hex(0xe1e1e1),
    rail_icon: hex(0x7a7a7a),
    rail_tile: hex(0xe4e4e4),
    bubble_text: hex(0xffffff),
    chip: hex(0xececec),
    shell: hex(0xf6f6f6),
    shell_border: hex(0xe3e3e3),
    change_card: hex(0xffffff),
    change_border: hex(0xe7e7e7),
    tile: hex(0xf1f1f1),
    link: hex(0x2b6fd6),
    word_add: hex(0xb9e2c4),
    word_del: hex(0xf0b8b3),
    fold: hex(0xf1f1f1),
    orange: hex(0xe8742a),
    badge: hex(0xdbe8fb),
    badge_text: hex(0x2b6fd6),
};

pub fn pal(mode: ThemeMode) -> &'static Pal {
    match mode {
        ThemeMode::Dark => &DARK,
        ThemeMode::Light => &LIGHT,
    }
}

/// Type scale (points): Codex's UI font is 14, secondary text 13, the
/// headline 28. On macOS the window resolves `system-ui` to SF Pro, the
/// app's own font, so these are the app's sizes.
#[cfg(target_os = "macos")]
mod scale {
    pub const BODY: f32 = 14.0;
    pub const SMALL: f32 = 13.0;
    pub const TINY: f32 = 11.0;
    pub const CODE: f32 = 12.0;
    pub const HEADING: f32 = 28.0;
}

/// Elsewhere the bundled Inter stands in for SF Pro (see
/// [`crate::window_options`]). It runs about 4% wider than SF Pro Text at
/// the same size, and SF Pro Display at 28 runs narrower still, so these
/// sizes match the captures' measured text widths instead.
#[cfg(not(target_os = "macos"))]
mod scale {
    pub const BODY: f32 = 13.5;
    pub const SMALL: f32 = 12.5;
    pub const TINY: f32 = 11.0;
    pub const CODE: f32 = 12.0;
    pub const HEADING: f32 = 25.5;
}

pub use scale::*;

/// Transcript prose sits on 22-point lines, as the app's 14-point body does.
pub const LINE_PT: f32 = 22.0;

/// quark's theme of `mode` with the Codex surfaces swapped in.
pub fn quark_theme(mode: ThemeMode) -> Theme {
    let p = pal(mode);
    let mut theme = Theme::for_mode(mode);
    let c = &mut theme.colors;
    c.background = p.bg;
    c.app_bg = p.bg;
    c.canvas = p.bg;
    c.editor_surface = p.bg;
    c.surface = p.composer;
    c.text = p.text;
    c.text_strong = p.text;
    c.text_muted = p.muted;
    c.placeholder = p.placeholder;
    c.accent = p.accent;
    c.focus_border = p.accent;
    c.sidebar_background = p.sidebar;
    theme
}
