//! Configuration discovery: the rustfmt keys the view printer shares, plus
//! the optional `quark-fmt.toml`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Deserialize;

/// Rustfmt's own lookup order within one directory.
const RUSTFMT_NAMES: [&str; 2] = [".rustfmt.toml", "rustfmt.toml"];
const QUARK_NAME: &str = "quark-fmt.toml";
/// The edition used when neither a flag, rustfmt.toml, nor a manifest names one.
pub const DEFAULT_EDITION: &str = "2024";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewlineStyle {
    Auto,
    Native,
    Unix,
    Windows,
}

/// The subset of rustfmt configuration the view printer follows. Everything
/// else reaches rustfmt through `path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustfmtConfig {
    /// The file the values came from; passed to rustfmt as `--config-path`.
    pub path: Option<PathBuf>,
    pub max_width: usize,
    pub tab_spaces: usize,
    pub hard_tabs: bool,
    pub newline_style: NewlineStyle,
    pub edition: Option<String>,
    pub style_edition: Option<String>,
}

impl Default for RustfmtConfig {
    fn default() -> Self {
        Self {
            path: None,
            max_width: 100,
            tab_spaces: 4,
            hard_tabs: false,
            newline_style: NewlineStyle::Auto,
            edition: None,
            style_edition: None,
        }
    }
}

/// `quark-fmt.toml`. Unknown keys are errors so a typo cannot silently
/// change which files or macros are formatted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuarkConfig {
    /// Directory the `exclude` globs are relative to.
    pub root: Option<PathBuf>,
    pub style_version: u32,
    pub macro_names: Vec<String>,
    pub expr_macros: Vec<String>,
    pub exclude: Vec<glob::Pattern>,
}

impl Default for QuarkConfig {
    fn default() -> Self {
        Self {
            root: None,
            style_version: 1,
            macro_names: vec!["view".into(), "quark::view".into()],
            expr_macros: vec!["vec".into()],
            exclude: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct QuarkFile {
    style_version: Option<u32>,
    macro_names: Option<Vec<String>>,
    expr_macros: Option<Vec<String>>,
    exclude: Option<Vec<String>>,
}

impl QuarkConfig {
    /// Whether `path` (absolute) matches an `exclude` glob.
    pub fn excludes(&self, path: &Path) -> bool {
        let Some(root) = &self.root else { return false };
        let Ok(rel) = path.strip_prefix(root) else {
            return false;
        };
        let opts = glob::MatchOptions {
            case_sensitive: true,
            require_literal_separator: true,
            require_literal_leading_dot: false,
        };
        self.exclude.iter().any(|p| p.matches_path_with(rel, opts))
    }
}

/// Configuration in effect for one source file.
#[derive(Clone, Debug)]
pub struct Settings {
    pub rustfmt: Arc<RustfmtConfig>,
    pub quark: Arc<QuarkConfig>,
    pub edition: String,
    pub style_edition: Option<String>,
}

/// Overrides from the command line.
#[derive(Clone, Debug, Default)]
pub struct Overrides {
    /// `--config-path`: a config file, or a directory to search upward from.
    pub config_path: Option<PathBuf>,
    pub edition: Option<String>,
    pub style_edition: Option<String>,
}

/// Looks up configuration per directory, caching each directory's answer.
pub struct Resolver {
    overrides: Overrides,
    rustfmt: HashMap<PathBuf, Arc<RustfmtConfig>>,
    quark: HashMap<PathBuf, Arc<QuarkConfig>>,
}

impl Resolver {
    pub fn new(overrides: Overrides) -> Self {
        Self {
            overrides,
            rustfmt: HashMap::new(),
            quark: HashMap::new(),
        }
    }

    /// Settings for a file in `dir` (absolute). `package_edition` comes from
    /// the owning Cargo package when known.
    pub fn settings(
        &mut self,
        dir: &Path,
        package_edition: Option<&str>,
    ) -> Result<Settings, String> {
        // An explicit --config-path is searched first, as rustfmt does, and
        // the file's own directory is the fallback.
        let explicit = self.overrides.config_path.clone();
        let rustfmt = match explicit
            .as_deref()
            .map(|p| self.rustfmt_from(p))
            .transpose()?
        {
            Some(c) if c.path.is_some() => c,
            _ => self.rustfmt_from(dir)?,
        };
        let quark = match explicit
            .as_deref()
            .map(|p| self.quark_from(p))
            .transpose()?
        {
            Some(c) if c.root.is_some() => c,
            _ => self.quark_from(dir)?,
        };
        let edition = self
            .overrides
            .edition
            .clone()
            .or_else(|| rustfmt.edition.clone())
            .or_else(|| package_edition.map(str::to_owned))
            .or_else(|| manifest_edition(dir))
            .unwrap_or_else(|| DEFAULT_EDITION.to_owned());
        let style_edition = self
            .overrides
            .style_edition
            .clone()
            .or_else(|| rustfmt.style_edition.clone());
        Ok(Settings {
            rustfmt,
            quark,
            edition,
            style_edition,
        })
    }

    fn rustfmt_from(&mut self, start: &Path) -> Result<Arc<RustfmtConfig>, String> {
        if start.is_file() {
            return read_rustfmt(start).map(Arc::new);
        }
        if let Some(c) = self.rustfmt.get(start) {
            return Ok(c.clone());
        }
        let found = RUSTFMT_NAMES
            .iter()
            .map(|n| start.join(n))
            .find(|p| p.is_file());
        let config = match (found, start.parent()) {
            (Some(file), _) => Arc::new(read_rustfmt(&file)?),
            (None, Some(parent)) => self.rustfmt_from(parent)?,
            (None, None) => Arc::new(RustfmtConfig::default()),
        };
        self.rustfmt.insert(start.to_owned(), config.clone());
        Ok(config)
    }

    fn quark_from(&mut self, start: &Path) -> Result<Arc<QuarkConfig>, String> {
        if start.is_file() {
            let dir = start.parent().unwrap_or(start).to_owned();
            return self.quark_from(&dir);
        }
        if let Some(c) = self.quark.get(start) {
            return Ok(c.clone());
        }
        let file = start.join(QUARK_NAME);
        let config = match (file.is_file(), start.parent()) {
            (true, _) => Arc::new(read_quark(&file)?),
            (false, Some(parent)) => self.quark_from(parent)?,
            (false, None) => Arc::new(QuarkConfig::default()),
        };
        self.quark.insert(start.to_owned(), config.clone());
        Ok(config)
    }
}

fn read_toml(path: &Path) -> Result<toml::Table, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_rustfmt(path: &Path) -> Result<RustfmtConfig, String> {
    let table = read_toml(path)?;
    let bad = |key: &str| format!("{}: invalid `{key}`", path.display());
    let mut c = RustfmtConfig {
        path: Some(path.to_owned()),
        ..RustfmtConfig::default()
    };
    let int = |key: &str| -> Result<Option<usize>, String> {
        table
            .get(key)
            .map(|v| {
                v.as_integer()
                    .and_then(|n| usize::try_from(n).ok())
                    .ok_or_else(|| bad(key))
            })
            .transpose()
    };
    let string = |key: &str| -> Result<Option<String>, String> {
        table
            .get(key)
            .map(|v| v.as_str().map(str::to_owned).ok_or_else(|| bad(key)))
            .transpose()
    };
    if let Some(n) = int("max_width")? {
        c.max_width = n;
    }
    if let Some(n) = int("tab_spaces")? {
        c.tab_spaces = n;
    }
    if let Some(v) = table.get("hard_tabs") {
        c.hard_tabs = v.as_bool().ok_or_else(|| bad("hard_tabs"))?;
    }
    if let Some(s) = string("newline_style")? {
        c.newline_style = match s.as_str() {
            "Auto" => NewlineStyle::Auto,
            "Native" => NewlineStyle::Native,
            "Unix" => NewlineStyle::Unix,
            "Windows" => NewlineStyle::Windows,
            _ => return Err(bad("newline_style")),
        };
    }
    c.edition = string("edition")?;
    c.style_edition = string("style_edition")?;
    Ok(c)
}

fn read_quark(path: &Path) -> Result<QuarkConfig, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let file: QuarkFile = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut c = QuarkConfig {
        root: path.parent().map(Path::to_owned),
        ..QuarkConfig::default()
    };
    if let Some(v) = file.style_version {
        if v != 1 {
            return Err(format!(
                "{}: unsupported style_version {v} (supported: 1)",
                path.display()
            ));
        }
        c.style_version = v;
    }
    if let Some(names) = file.macro_names {
        c.macro_names = names;
    }
    if let Some(names) = file.expr_macros {
        c.expr_macros = names;
    }
    for glob in file.exclude.unwrap_or_default() {
        let pattern = glob::Pattern::new(&glob)
            .map_err(|e| format!("{}: exclude `{glob}`: {e}", path.display()))?;
        c.exclude.push(pattern);
    }
    Ok(c)
}

/// The edition of the nearest Cargo package above `dir`, following
/// `edition.workspace = true` to the workspace root. Used when no Cargo
/// metadata is available (explicit files and stdin).
fn manifest_edition(dir: &Path) -> Option<String> {
    let (manifest_dir, table) = dir.ancestors().find_map(|d| {
        let table = read_toml(&d.join("Cargo.toml")).ok()?;
        table.contains_key("package").then(|| (d.to_owned(), table))
    })?;
    let edition = table.get("package")?.get("edition")?;
    if let Some(s) = edition.as_str() {
        return Some(s.to_owned());
    }
    edition.get("workspace")?.as_bool().filter(|w| *w)?;
    manifest_dir.ancestors().find_map(|d| {
        let table = read_toml(&d.join("Cargo.toml")).ok()?;
        let ws = table.get("workspace")?;
        Some(ws.get("package")?.get("edition")?.as_str()?.to_owned())
    })
}
