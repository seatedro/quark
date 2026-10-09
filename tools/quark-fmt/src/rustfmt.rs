//! The external rustfmt process: the project's pinned toolchain formats
//! synthetic wrapper files read from stdin and prints them on stdout.
//!
//! Nothing here touches the user's files. Input arrives on stdin and rustfmt
//! runs with `--emit stdout`, so it never writes a file and never follows a
//! `mod` declaration to another one. It runs in `cwd`, where it discovers
//! `rustfmt.toml` for stdin input and where a rustup proxy picks the pinned
//! toolchain, unless `config_path` names the configuration.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};

/// Why a rustfmt run produced no output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RustfmtError {
    /// The program could not be started; every input fails alike.
    Spawn(String),
    /// rustfmt ran and rejected the input; the first stderr lines.
    Rejected(String),
    /// rustfmt printed something unusable.
    Output(String),
}

impl std::fmt::Display for RustfmtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RustfmtError::Spawn(m) => write!(f, "could not run rustfmt: {m}"),
            RustfmtError::Rejected(m) => write!(f, "rustfmt rejected the input: {m}"),
            RustfmtError::Output(m) => write!(f, "unexpected rustfmt output: {m}"),
        }
    }
}

/// `max_width` and `tab_spaces`, or why they could not be read.
type Widths = Result<(usize, usize), RustfmtError>;

/// How to run rustfmt for one source file.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RustfmtCommand {
    /// The rustfmt executable. Never taken from `RUSTFMT`, which editors
    /// point at quark-fmt itself; following it would recurse.
    pub program: OsString,
    /// `--edition`; rustfmt's own default, 2015, rejects let chains.
    pub edition: String,
    /// `--style-edition`, when not left to the configuration.
    pub style_edition: Option<String>,
    /// An explicit `rustfmt.toml`; otherwise rustfmt discovers one from
    /// `cwd`.
    pub config_path: Option<PathBuf>,
    /// An existing directory, normally the source file's.
    pub cwd: PathBuf,
}

impl RustfmtCommand {
    /// `$QUARK_FMT_RUSTFMT` or `rustfmt` from `PATH`, edition 2024, the
    /// configuration discovered from `cwd`.
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        RustfmtCommand {
            program: std::env::var_os("QUARK_FMT_RUSTFMT").unwrap_or_else(|| "rustfmt".into()),
            edition: "2024".to_owned(),
            style_edition: None,
            config_path: None,
            cwd: cwd.into(),
        }
    }

    /// `max_width` and `tab_spaces` of the effective configuration, read
    /// once per program, directory, and config path for the whole process.
    pub fn widths(&self) -> Result<(usize, usize), RustfmtError> {
        static SEEN: OnceLock<Mutex<HashMap<RustfmtCommand, Widths>>> = OnceLock::new();
        let seen = SEEN.get_or_init(Default::default);
        if let Some(hit) = seen.lock().unwrap_or_else(|e| e.into_inner()).get(self) {
            return hit.clone();
        }
        let result = self.print_config();
        seen.lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(self.clone(), result.clone());
        result
    }

    fn print_config(&self) -> Result<(usize, usize), RustfmtError> {
        let mut cmd = self.command();
        cmd.args(["--print-config", "current"]);
        if let Some(path) = &self.config_path {
            cmd.arg("--config-path").arg(path);
        }
        // A file name in `cwd` resolves the configuration stdin input gets.
        cmd.arg("stdin.rs");
        let out = cmd.output().map_err(|e| self.spawn_error(e))?;
        if !out.status.success() {
            return Err(RustfmtError::Rejected(first_lines(&out.stderr)));
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let value = |key: &str| {
            text.lines()
                .filter_map(|l| l.split_once('='))
                .find(|(k, _)| k.trim() == key)
                .and_then(|(_, v)| v.trim().parse().ok())
                .ok_or_else(|| RustfmtError::Output(format!("no `{key}` in --print-config")))
        };
        Ok((value("max_width")?, value("tab_spaces")?))
    }

    /// Formats `input` as a whole Rust file with `--config` `overrides`.
    pub fn format(&self, input: &str, overrides: &str) -> Result<String, RustfmtError> {
        let mut cmd = self.command();
        cmd.args(["--emit", "stdout", "--quiet", "--edition", &self.edition]);
        if let Some(style) = &self.style_edition {
            cmd.args(["--style-edition", style]);
        }
        if let Some(path) = &self.config_path {
            cmd.arg("--config-path").arg(path);
        }
        if !overrides.is_empty() {
            cmd.args(["--config", overrides]);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = cmd.spawn().map_err(|e| self.spawn_error(e))?;
        // Feed stdin from a thread: a large input would otherwise fill the
        // stdout pipe while this thread is still writing, and deadlock.
        let mut stdin = child.stdin.take().expect("stdin is piped");
        let output = std::thread::scope(|s| {
            s.spawn(move || {
                let _ = stdin.write_all(input.as_bytes());
            });
            child.wait_with_output()
        })
        .map_err(|e| RustfmtError::Spawn(e.to_string()))?;
        if !output.status.success() {
            return Err(RustfmtError::Rejected(first_lines(&output.stderr)));
        }
        String::from_utf8(output.stdout)
            .map_err(|_| RustfmtError::Output("formatted text is not UTF-8".to_owned()))
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.program);
        cmd.current_dir(&self.cwd).env_remove("RUSTFMT");
        cmd
    }

    fn spawn_error(&self, e: std::io::Error) -> RustfmtError {
        RustfmtError::Spawn(format!("{}: {e}", self.program.to_string_lossy()))
    }
}

/// The first few nonblank lines of stderr, enough to locate a rejection.
fn first_lines(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .take(4)
        .collect();
    if lines.is_empty() {
        "no diagnostics".to_owned()
    } else {
        lines.join(" | ")
    }
}
