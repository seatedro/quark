//! Reading a [`TerminalStyle`] from a Ghostty config file, so a terminal
//! looks the way the user set Ghostty up.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Mutex;

use crate::grid::Rgb;
use crate::state::{AlphaBlending, TerminalStyle};

/// Ghostty's default font, bundled with quark-text.
pub const GHOSTTY_FONT_FAMILY: &str = "JetBrains Mono";

impl TerminalStyle {
    /// Ghostty's defaults: JetBrains Mono at 13 points, Ghostty's
    /// macOS size. Colors stay the theme's.
    pub fn ghostty() -> Self {
        Self {
            font_family: Some(GHOSTTY_FONT_FAMILY),
            font_size: 13.0,
            ..Self::default()
        }
    }

    /// [`Self::ghostty`] with the settings of a Ghostty config file
    /// applied, and a message for each line that was not understood or is
    /// not supported. Supported keys: `font-family` (the first one),
    /// `font-size`, `adjust-cell-height`, `font-thicken`, `alpha-blending`,
    /// `minimum-contrast`, `foreground`, `background`, `cursor-color`,
    /// `cursor-text`, `selection-background`, and `palette`. Colors are
    /// hex (`#rrggbb`, `rrggbb`, or `#rgb`); `theme` and `config-file` are
    /// not followed.
    pub fn from_ghostty_config(text: &str) -> (Self, Vec<String>) {
        let mut style = Self::ghostty();
        let mut warnings = Vec::new();
        let mut family_set = false;
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                warnings.push(format!("line {}: expected key = value", n + 1));
                continue;
            };
            let (key, value) = (key.trim(), unquote(value.trim()));
            if let Err(why) = apply(&mut style, key, value, &mut family_set) {
                warnings.push(format!("line {}: {key}: {why}", n + 1));
            }
        }
        (style, warnings)
    }

    /// The user's Ghostty config (`$XDG_CONFIG_HOME/ghostty/config`, else
    /// `~/.config/ghostty/config`, and on macOS also
    /// `~/Library/Application Support/com.mitchellh.ghostty/config`) read
    /// with [`Self::from_ghostty_config`]; `None` when there is none.
    pub fn load_ghostty_config() -> Option<(Self, Vec<String>)> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let xdg = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".config")));
        let mut paths: Vec<PathBuf> = xdg.map(|d| d.join("ghostty/config")).into_iter().collect();
        if cfg!(target_os = "macos")
            && let Some(home) = &home
        {
            paths.push(home.join("Library/Application Support/com.mitchellh.ghostty/config"));
        }
        let text = paths
            .iter()
            .find_map(|path| std::fs::read_to_string(path).ok())?;
        Some(Self::from_ghostty_config(&text))
    }
}

fn unquote(value: &str) -> &str {
    value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .unwrap_or(value)
}

fn apply(
    style: &mut TerminalStyle,
    key: &str,
    value: &str,
    family_set: &mut bool,
) -> Result<(), String> {
    let number = |v: &str| v.parse::<f32>().map_err(|_| format!("not a number: {v:?}"));
    let boolean = |v: &str| match v {
        "true" | "" => Ok(true),
        "false" => Ok(false),
        _ => Err(format!("not true or false: {v:?}")),
    };
    let color = |v: &str| parse_color(v).ok_or_else(|| format!("not a hex color: {v:?}"));
    let c = &mut style.colors;
    match key {
        // Repeatable: later families are fallbacks, which quark-text's own
        // fallback chain covers. An empty value resets the list.
        "font-family" if value.is_empty() => {
            style.font_family = Some(GHOSTTY_FONT_FAMILY);
            *family_set = false;
        }
        "font-family" if !*family_set => {
            style.font_family = Some(intern(value));
            *family_set = true;
        }
        "font-family" => {}
        "font-size" => style.font_size = number(value)?.max(1.0),
        "adjust-cell-height" if value.is_empty() => style.adjust_cell_height = None,
        "adjust-cell-height" => {
            style.adjust_cell_height = Some(value.parse().map_err(|e| format!("{e}"))?);
        }
        "font-thicken" => style.font_thicken = boolean(value)?,
        // Ghostty's native blending on macOS looks like linear-corrected.
        "alpha-blending" => {
            style.alpha_blending = match value {
                "native" | "linear-corrected" => AlphaBlending::LinearCorrected,
                "linear" => AlphaBlending::Linear,
                _ => return Err(format!("unknown mode {value:?}")),
            }
        }
        "minimum-contrast" => style.minimum_contrast = number(value)?.clamp(1.0, 21.0),
        "foreground" => c.foreground = Some(color(value)?),
        "background" => c.background = Some(color(value)?),
        "cursor-color" => c.cursor = Some(color(value)?),
        "cursor-text" => c.cursor_text = Some(color(value)?),
        "selection-background" => c.selection_background = Some(color(value)?),
        "palette" => {
            let (index, rgb) = value
                .split_once('=')
                .ok_or_else(|| format!("expected N=COLOR: {value:?}"))?;
            let index = parse_index(index.trim()).ok_or_else(|| format!("bad index {index:?}"))?;
            let rgb = color(rgb.trim())?;
            c.palette.retain(|&(i, _)| i != index);
            c.palette.push((index, rgb));
        }
        _ => return Err("not supported".to_owned()),
    }
    Ok(())
}

/// A palette index in decimal, or with a `0x`, `0o`, or `0b` prefix.
fn parse_index(s: &str) -> Option<u8> {
    let (digits, radix) = match s.get(..2) {
        Some("0x") => (&s[2..], 16),
        Some("0o") => (&s[2..], 8),
        Some("0b") => (&s[2..], 2),
        _ => (s, 10),
    };
    u8::from_str_radix(digits, radix).ok()
}

fn parse_color(s: &str) -> Option<Rgb> {
    let hex = s.strip_prefix('#').unwrap_or(s);
    let digit = |i: usize, n: usize| u8::from_str_radix(hex.get(i..i + n)?, 16).ok();
    match hex.len() {
        6 => Some(Rgb::new(digit(0, 2)?, digit(2, 2)?, digit(4, 2)?)),
        3 => Some(Rgb::new(
            digit(0, 1)? * 17,
            digit(1, 1)? * 17,
            digit(2, 1)? * 17,
        )),
        _ => None,
    }
}

/// `name` as a string that lives for the process, as text styles name
/// families. Each distinct name is kept once.
fn intern(name: &str) -> &'static str {
    static NAMES: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);
    if name == GHOSTTY_FONT_FAMILY {
        return GHOSTTY_FONT_FAMILY;
    }
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    let names = names.get_or_insert_with(HashSet::new);
    if let Some(&kept) = names.get(name) {
        return kept;
    }
    let kept: &'static str = Box::leak(name.to_owned().into_boxed_str());
    names.insert(kept);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metrics::MetricModifier;

    /// Each supported key lands in the style; unknown keys, bad values,
    /// and `theme` come back as warnings without stopping the rest.
    #[test]
    fn a_ghostty_config_sets_the_style() {
        let config = r##"
# comment
font-family = "Fira Code"
font-family = Symbols Nerd Font
font-size = 14.5
adjust-cell-height = 10%
font-thicken = true
minimum-contrast = 1.1
background = #1e1e2e
foreground = cdd6f4
cursor-color = #fff
palette = 1=#f38ba8
palette = 0x09=#ff0000
palette = 1=#e78284
theme = catppuccin-mocha
font-size = big
"##;
        let (style, warnings) = TerminalStyle::from_ghostty_config(config);
        assert_eq!(style.font_family, Some("Fira Code"));
        assert_eq!(style.font_size, 14.5);
        assert_eq!(style.adjust_cell_height, Some(MetricModifier::Percent(1.1)));
        assert!(style.font_thicken);
        assert_eq!(style.minimum_contrast, 1.1);
        let c = &style.colors;
        assert_eq!(c.background, Some(Rgb::new(0x1e, 0x1e, 0x2e)));
        assert_eq!(c.foreground, Some(Rgb::new(0xcd, 0xd6, 0xf4)));
        assert_eq!(c.cursor, Some(Rgb::new(0xff, 0xff, 0xff)));
        assert_eq!(
            c.palette,
            [(9, Rgb::new(0xff, 0, 0)), (1, Rgb::new(0xe7, 0x82, 0x84))]
        );
        assert_eq!(
            warnings,
            [
                "line 15: theme: not supported",
                "line 16: font-size: not a number: \"big\""
            ]
        );
    }
}
