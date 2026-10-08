//! Themes as JSON data.
//!
//! A theme file names a family and gives a light variant, a dark variant,
//! or both. Each variant starts from quark's own theme of its mode and
//! contrast (`"standard"` unless it says `"high"`) and overrides the tokens
//! it lists:
//!
//! ```json
//! {
//!   "name": "Harbor",
//!   "dark": {
//!     "colors": { "background": "#0f1419", "accent": "#59c2ff" },
//!     "metrics": { "ui_font_size": 15 }
//!   },
//!   "light": { "contrast": "high", "colors": { "background": "#fafafa" } }
//! }
//! ```
//!
//! Token names are the field names of [`ThemeColors`] and
//! [`ThemeMetrics`]. Colors are `#rgb`, `#rgba`, `#rrggbb`, or `#rrggbbaa`,
//! or `"$name"` for an entry of the variant's `"palette"`, so one edit to a
//! palette color reaches every token that names it:
//!
//! ```json
//! "light": {
//!   "palette": { "iris": "#6257d9" },
//!   "colors": { "accent": "$iris", "focus_border": "$iris" }
//! }
//! ```
//!
//! Parsing reports every problem in a file at once, each with its path in
//! the file and, for a misspelled token, the token it was probably meant
//! to be.

use std::fmt;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use super::{Color, Theme, ThemeColors, ThemeContrast, ThemeMetrics, ThemeMode};

/// Metrics that are font sizes, which must stay readable.
const FONT_SIZES: [&str; 4] = [
    "ui_font_size",
    "ui_small_font_size",
    "heading_font_size",
    "mono_font_size",
];
const FONT_SIZE_RANGE: std::ops::RangeInclusive<f32> = 4.0..=96.0;

/// A named light and dark theme pair; at least one is present.
#[derive(Debug, Clone, PartialEq)]
pub struct ThemeFamily {
    pub name: String,
    pub light: Option<Theme>,
    pub dark: Option<Theme>,
}

/// One problem in a theme file: where (`dark.colors.accent`) and what.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeIssue {
    pub path: String,
    pub message: String,
}

/// Why a theme file was rejected: every issue found in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeError {
    /// The file, when it came from one.
    pub file: Option<PathBuf>,
    pub issues: Vec<ThemeIssue>,
}

impl fmt::Display for ThemeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let file = self
            .file
            .as_deref()
            .map_or_else(|| "theme".to_owned(), |p| p.display().to_string());
        for (i, issue) in self.issues.iter().enumerate() {
            if i > 0 {
                writeln!(f)?;
            }
            match issue.path.is_empty() {
                true => write!(f, "{file}: {}", issue.message)?,
                false => write!(f, "{file}: {}: {}", issue.path, issue.message)?,
            }
        }
        Ok(())
    }
}

impl std::error::Error for ThemeError {}

impl ThemeFamily {
    /// Quark's own light and dark themes.
    pub fn builtin() -> Self {
        Self {
            name: "Quark".to_owned(),
            light: Some(Theme::default_light()),
            dark: Some(Theme::default_dark()),
        }
    }

    /// Quark's own high-contrast light and dark themes.
    pub fn high_contrast() -> Self {
        Self {
            name: "Quark High Contrast".to_owned(),
            light: Some(Theme::high_contrast_light()),
            dark: Some(Theme::high_contrast_dark()),
        }
    }

    /// The variant for `mode`, or the other one when the family has only
    /// that.
    pub fn theme(&self, mode: ThemeMode) -> Theme {
        let (wanted, other) = match mode {
            ThemeMode::Light => (&self.light, &self.dark),
            ThemeMode::Dark => (&self.dark, &self.light),
        };
        wanted
            .as_ref()
            .or(other.as_ref())
            .cloned()
            .unwrap_or_else(|| Theme::for_mode(mode))
    }

    /// Parse a theme file, reporting every issue in it.
    pub fn from_json(json: &str) -> Result<Self, ThemeError> {
        let mut issues = Vec::new();
        let family = parse_family(json, &mut issues);
        match family {
            Some(family) if issues.is_empty() => Ok(family),
            _ => Err(ThemeError { file: None, issues }),
        }
    }

    /// The family as a theme file listing every token, so the file reads
    /// back to the same family.
    pub fn to_json(&self) -> String {
        let mut root = Map::new();
        root.insert("name".to_owned(), Value::String(self.name.clone()));
        for (key, theme) in [("light", &self.light), ("dark", &self.dark)] {
            if let Some(theme) = theme {
                root.insert(key.to_owned(), variant_json(theme));
            }
        }
        serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default()
    }
}

fn variant_json(theme: &Theme) -> Value {
    let colors = ThemeColors::TOKENS
        .iter()
        .filter_map(|&name| {
            Some((
                name.to_owned(),
                Value::String(color_hex(theme.colors.get(name)?)),
            ))
        })
        .collect();
    let metrics = ThemeMetrics::TOKENS
        .iter()
        .filter_map(|&name| {
            let value = serde_json::Number::from_f64(f64::from(theme.metrics.get(name)?))?;
            Some((name.to_owned(), Value::Number(value)))
        })
        .collect();
    let mut variant = Map::new();
    variant.insert(
        "contrast".to_owned(),
        Value::String(contrast_name(theme.contrast).to_owned()),
    );
    variant.insert("colors".to_owned(), Value::Object(colors));
    variant.insert("metrics".to_owned(), Value::Object(metrics));
    variant.insert(
        "reduced_motion".to_owned(),
        Value::Bool(theme.reduced_motion),
    );
    Value::Object(variant)
}

fn contrast_name(contrast: ThemeContrast) -> &'static str {
    match contrast {
        ThemeContrast::Standard => "standard",
        ThemeContrast::High => "high",
    }
}

fn color_hex(c: Color) -> String {
    if c.a == 255 {
        format!("#{:02x}{:02x}{:02x}", c.r, c.g, c.b)
    } else {
        format!("#{:02x}{:02x}{:02x}{:02x}", c.r, c.g, c.b, c.a)
    }
}

/// `#rgb`, `#rgba`, `#rrggbb`, or `#rrggbbaa`.
pub fn parse_color(text: &str) -> Result<Color, String> {
    let expected = || format!("expected a color like \"#1e2030\" or \"#1e2030cc\", found {text:?}");
    let hex = text.strip_prefix('#').ok_or_else(expected)?;
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(expected());
    }
    let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1).unwrap_or("0"), 16).unwrap_or(0);
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2).unwrap_or("00"), 16).unwrap_or(0);
    match hex.len() {
        3 | 4 => {
            let alpha = if hex.len() == 4 { digit(3) * 17 } else { 255 };
            Ok(Color::rgba(
                digit(0) * 17,
                digit(1) * 17,
                digit(2) * 17,
                alpha,
            ))
        }
        6 | 8 => {
            let alpha = if hex.len() == 8 { byte(6) } else { 255 };
            Ok(Color::rgba(byte(0), byte(2), byte(4), alpha))
        }
        _ => Err(expected()),
    }
}

fn issue(issues: &mut Vec<ThemeIssue>, path: impl Into<String>, message: impl Into<String>) {
    issues.push(ThemeIssue {
        path: path.into(),
        message: message.into(),
    });
}

fn parse_family(json: &str, issues: &mut Vec<ThemeIssue>) -> Option<ThemeFamily> {
    let root: Value = match serde_json::from_str(json) {
        Ok(root) => root,
        Err(error) => {
            issue(issues, "", format!("not valid JSON: {error}"));
            return None;
        }
    };
    let Value::Object(root) = root else {
        issue(
            issues,
            "",
            "expected an object with \"name\" and \"light\" or \"dark\"",
        );
        return None;
    };
    let name = match root.get("name") {
        Some(Value::String(name)) if !name.trim().is_empty() => name.trim().to_owned(),
        Some(_) => {
            issue(issues, "name", "expected a non-empty string");
            String::new()
        }
        None => {
            issue(issues, "name", "missing; every theme needs a name");
            String::new()
        }
    };
    for key in root.keys() {
        if !matches!(key.as_str(), "name" | "light" | "dark" | "$schema") {
            unknown(issues, key, key, &["name", "light", "dark"]);
        }
    }
    let light = root
        .get("light")
        .map(|v| parse_variant(v, ThemeMode::Light, "light", issues));
    let dark = root
        .get("dark")
        .map(|v| parse_variant(v, ThemeMode::Dark, "dark", issues));
    if light.is_none() && dark.is_none() {
        issue(
            issues,
            "",
            "needs a \"light\" or \"dark\" variant (or both)",
        );
    }
    Some(ThemeFamily { name, light, dark })
}

fn parse_variant(
    value: &Value,
    mode: ThemeMode,
    path: &str,
    issues: &mut Vec<ThemeIssue>,
) -> Theme {
    let Value::Object(variant) = value else {
        issue(
            issues,
            path,
            "expected an object with \"colors\" and \"metrics\"",
        );
        return Theme::for_mode(mode);
    };
    // The contrast picks the theme the tokens override, so it is read
    // before them.
    let contrast = match variant.get("contrast") {
        None => ThemeContrast::Standard,
        Some(Value::String(name)) if name == "standard" => ThemeContrast::Standard,
        Some(Value::String(name)) if name == "high" => ThemeContrast::High,
        Some(value) => {
            let at = format!("{path}.contrast");
            issue(
                issues,
                at,
                format!("expected \"standard\" or \"high\", found {value}"),
            );
            ThemeContrast::Standard
        }
    };
    let mut theme = Theme::for_mode_and_contrast(mode, contrast);
    // Read before the colors that name its entries.
    let palette = parse_palette(variant.get("palette"), &format!("{path}.palette"), issues);
    for (key, value) in variant {
        let at = format!("{path}.{key}");
        match (key.as_str(), value) {
            ("contrast" | "palette", _) => {}
            ("colors", Value::Object(colors)) => {
                for (token, value) in colors {
                    let at = format!("{at}.{token}");
                    let color = match value {
                        Value::String(text) => match text.strip_prefix('$') {
                            Some(name) => match palette.iter().find(|(n, _)| n == name) {
                                Some((_, color)) => Ok(*color),
                                None => {
                                    let names: Vec<&str> =
                                        palette.iter().map(|(n, _)| n.as_str()).collect();
                                    unknown(issues, &at, name, &names);
                                    continue;
                                }
                            },
                            None => parse_color(text),
                        },
                        _ => Err(format!("expected a color string, found {value}")),
                    };
                    match color {
                        Ok(color) if theme.colors.set(token, color) => {}
                        Ok(_) => unknown(issues, &at, token, ThemeColors::TOKENS),
                        Err(message) => issue(issues, at, message),
                    }
                }
            }
            ("metrics", Value::Object(metrics)) => {
                for (token, value) in metrics {
                    let at = format!("{at}.{token}");
                    let Some(number) = value.as_f64().map(|n| n as f32) else {
                        issue(issues, at, format!("expected a number, found {value}"));
                        continue;
                    };
                    if FONT_SIZES.contains(&token.as_str()) && !FONT_SIZE_RANGE.contains(&number) {
                        let (lo, hi) = (FONT_SIZE_RANGE.start(), FONT_SIZE_RANGE.end());
                        issue(
                            issues,
                            at,
                            format!("font size {number} is outside {lo}..={hi}"),
                        );
                    } else if !number.is_finite() || number < 0.0 {
                        issue(
                            issues,
                            at,
                            format!("expected a size of 0 or more, found {number}"),
                        );
                    } else if !theme.metrics.set(token, number) {
                        unknown(issues, &at, token, ThemeMetrics::TOKENS);
                    }
                }
            }
            ("reduced_motion", Value::Bool(reduced)) => theme.reduced_motion = *reduced,
            ("colors" | "metrics", _) => issue(issues, at, "expected an object of tokens"),
            ("reduced_motion", _) => issue(issues, at, "expected true or false"),
            _ => unknown(
                issues,
                &at,
                key,
                &["colors", "metrics", "reduced_motion", "contrast", "palette"],
            ),
        }
    }
    theme
}

/// A variant's named colors, in file order.
fn parse_palette(
    value: Option<&Value>,
    path: &str,
    issues: &mut Vec<ThemeIssue>,
) -> Vec<(String, Color)> {
    let entries = match value {
        None => return Vec::new(),
        Some(Value::Object(entries)) => entries,
        Some(_) => {
            issue(issues, path, "expected an object of named colors");
            return Vec::new();
        }
    };
    entries
        .iter()
        .filter_map(|(name, value)| {
            let color = match value {
                Value::String(text) => parse_color(text),
                _ => Err(format!("expected a color string, found {value}")),
            };
            color
                .map_err(|message| issue(issues, format!("{path}.{name}"), message))
                .ok()
                .map(|color| (name.clone(), color))
        })
        .collect()
}

/// Report `name` at `path` as unknown, suggesting the closest of `known`.
fn unknown(issues: &mut Vec<ThemeIssue>, path: &str, name: &str, known: &[&str]) {
    let closest = known
        .iter()
        .map(|k| (edit_distance(name, k), *k))
        .min()
        .filter(|(distance, _)| *distance <= 3);
    let message = match closest {
        Some((_, suggestion)) => format!("unknown key {name:?}; did you mean {suggestion:?}?"),
        None => format!("unknown key {name:?}"),
    };
    issue(issues, path, message);
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1)
                .min(row[j] + 1)
                .min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

/// The theme families an app can pick from: quark's own (standard, then
/// high contrast), then those loaded from theme files (a later family replaces an earlier one of the
/// same name).
#[derive(Debug, Clone)]
pub struct ThemeRegistry {
    families: Vec<ThemeFamily>,
}

impl Default for ThemeRegistry {
    fn default() -> Self {
        Self {
            families: vec![ThemeFamily::builtin(), ThemeFamily::high_contrast()],
        }
    }
}

impl ThemeRegistry {
    pub fn families(&self) -> &[ThemeFamily] {
        &self.families
    }

    pub fn get(&self, name: &str) -> Option<&ThemeFamily> {
        self.families.iter().find(|f| f.name == name)
    }

    /// Add `family`, replacing any of the same name.
    pub fn insert(&mut self, family: ThemeFamily) {
        match self.families.iter_mut().find(|f| f.name == family.name) {
            Some(slot) => *slot = family,
            None => self.families.push(family),
        }
    }

    /// Load every `*.json` file in `dir`, in name order. Returns the files
    /// that were rejected, with their issues; a missing directory is not
    /// an error (the user has no themes yet).
    pub fn load_dir(&mut self, dir: &Path) -> Vec<ThemeError> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|entry| Some(entry.ok()?.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        let mut errors = Vec::new();
        for path in paths {
            let parsed = std::fs::read_to_string(&path)
                .map_err(|error| ThemeError {
                    file: None,
                    issues: vec![ThemeIssue {
                        path: String::new(),
                        message: format!("could not read: {error}"),
                    }],
                })
                .and_then(|json| ThemeFamily::from_json(&json));
            match parsed {
                Ok(family) => self.insert(family),
                Err(mut error) => {
                    error.file = Some(path);
                    errors.push(error);
                }
            }
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_family_survives_a_round_trip_through_json() {
        let mut dark = Theme::high_contrast_dark();
        dark.colors.accent = Color::rgba(1, 2, 3, 4);
        dark.metrics.ui_font_size = 13.5;
        dark.reduced_motion = true;
        let family = ThemeFamily {
            name: "Round".into(),
            light: Some(Theme::default_light()),
            dark: Some(dark),
        };
        assert_eq!(ThemeFamily::from_json(&family.to_json()), Ok(family));
    }

    #[test]
    fn a_variant_overrides_only_the_tokens_it_names() {
        let family = ThemeFamily::from_json(
            r##"{"name": "Harbor", "dark": {"colors": {"accent": "#59c2ff", "text": "#fff8"}}}"##,
        )
        .expect("valid");
        let mut expected = Theme::default_dark();
        expected.colors.accent = Color::rgba(0x59, 0xc2, 0xff, 255);
        expected.colors.text = Color::rgba(255, 255, 255, 0x88);
        // A family without a light variant uses its dark one in light mode.
        assert_eq!(family.theme(ThemeMode::Light), expected);
    }

    #[test]
    fn colors_can_name_palette_entries() {
        let family = ThemeFamily::from_json(
            r##"{"name": "Iris", "light": {
                "palette": {"iris": "#6257d9"},
                "colors": {"accent": "$iris", "focus_border": "$iris"}
            }}"##,
        )
        .expect("valid");
        let theme = family.theme(ThemeMode::Light);
        let iris = Color::rgba(0x62, 0x57, 0xd9, 255);
        assert_eq!(
            (theme.colors.accent, theme.colors.focus_border),
            (iris, iris)
        );
    }

    #[test]
    fn a_high_contrast_variant_overrides_the_high_contrast_theme() {
        let family = ThemeFamily::from_json(
            r##"{"name": "Harbor HC", "light": {"colors": {"accent": "#002266"}, "contrast": "high"}}"##,
        )
        .expect("valid");
        let mut expected = Theme::high_contrast_light();
        expected.colors.accent = Color::rgba(0x00, 0x22, 0x66, 255);
        assert_eq!(family.theme(ThemeMode::Light), expected);
    }

    #[test]
    fn invalid_files_report_every_issue_with_its_path() {
        let cases: &[(&str, &str, &[&str])] = &[
            ("not json", "{", &[": not valid JSON"]),
            (
                "no variant",
                r#"{"name": "x"}"#,
                &[": needs a \"light\" or \"dark\""],
            ),
            (
                "missing name and a misspelled key",
                r#"{"drak": {}}"#,
                &[
                    "name: missing",
                    "drak: unknown key \"drak\"; did you mean \"dark\"?",
                    ": needs a \"light\"",
                ],
            ),
            (
                "a misspelled token and a bad color",
                r##"{"name": "x", "dark": {"colors": {"backgound": "#000", "accent": "blue"}}}"##,
                &[
                    "dark.colors.accent: expected a color like",
                    "dark.colors.backgound: unknown key \"backgound\"; did you mean \"background\"?",
                ],
            ),
            (
                "a misspelled palette name and a bad palette color",
                r##"{"name": "x", "dark": {"palette": {"iris": "#123456", "sky": "blue"},
                    "colors": {"accent": "$irs"}}}"##,
                &[
                    "dark.palette.sky: expected a color like",
                    "dark.colors.accent: unknown key \"irs\"; did you mean \"iris\"?",
                ],
            ),
            (
                "an unknown contrast",
                r#"{"name": "x", "dark": {"contrast": "max"}}"#,
                &["dark.contrast: expected \"standard\" or \"high\", found \"max\""],
            ),
            (
                "an unreadable font size and a negative metric",
                r#"{"name": "x", "light": {"metrics": {"ui_font_size": 1, "panel_radius": -2}}}"#,
                &[
                    "light.metrics.panel_radius: expected a size of 0 or more",
                    "light.metrics.ui_font_size: font size 1 is outside 4..=96",
                ],
            ),
        ];
        for (name, json, expected) in cases {
            let error = ThemeFamily::from_json(json).expect_err(name);
            let lines: Vec<String> = error
                .issues
                .iter()
                .map(|i| format!("{}: {}", i.path, i.message))
                .collect();
            assert_eq!(lines.len(), expected.len(), "{name}: {lines:#?}");
            for (line, prefix) in lines.iter().zip(expected.iter()) {
                assert!(line.starts_with(prefix), "{name}: {line:?} vs {prefix:?}");
            }
        }
    }

    #[test]
    fn a_theme_directory_loads_good_files_and_reports_bad_ones() {
        let dir = std::env::temp_dir().join(format!("quark-themes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let write = |name: &str, json: &str| std::fs::write(dir.join(name), json).expect("write");
        write(
            "a.json",
            r##"{"name": "Quark", "dark": {"colors": {"accent": "#ff0000"}}}"##,
        );
        write("b.json", r#"{"name": "Bad"}"#);
        write("notes.txt", "not a theme");
        let mut registry = ThemeRegistry::default();
        let errors = registry.load_dir(&dir);
        std::fs::remove_dir_all(&dir).ok();

        let names: Vec<&str> = registry
            .families()
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["Quark", "Quark High Contrast"],
            "a.json replaced the builtin family"
        );
        let accent = registry
            .get("Quark")
            .map(|f| f.theme(ThemeMode::Dark).colors.accent);
        assert_eq!(accent, Some(Color::rgba(255, 0, 0, 255)));
        let files: Vec<_> = errors
            .iter()
            .filter_map(|e| e.file.as_ref()?.file_name())
            .collect();
        assert_eq!(files, ["b.json"]);
    }
}
