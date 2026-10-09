//! The `quark-fmt` and `cargo quark fmt` command line: argument parsing,
//! file selection, the rustfmt-then-views transaction per file, `--check`
//! diffs, and atomic writes.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::{Overrides, Resolver, Settings};
use crate::workspace::{self, Selected};

/// Composed transformations tried before giving up on a fixed point.
const MAX_TRANSFORMS: usize = 3;
/// Files formatted at once. Each spawns rustfmt, so stay modest on shared
/// machines.
const MAX_JOBS: usize = 4;

const USAGE: &str = "\
Format Rust sources and quark view! templates: rustfmt first, then views.

Usage:
  quark-fmt [options] <path>...    format files; directories are walked
  quark-fmt [options] --all        format every workspace member
  quark-fmt [options] --stdin      format stdin to stdout
  cargo quark fmt [options] ...    the same, as a Cargo subcommand

Options:
  --check                  write nothing; print a diff, exit 1 if anything would change
  --view-only              format views only (for use after `cargo fmt`)
  --all                    select every workspace member's sources
  --stdin                  read stdin, print the formatted source to stdout
  --stdin-filepath <path>  path used to find configuration for stdin (implies --stdin)
  --manifest-path <path>   Cargo.toml for --all
  --config-path <path>     rustfmt.toml / quark-fmt.toml, or a directory to search from
  --edition <year>         Rust edition (default: rustfmt.toml, then the package, then 2024)
  --style-edition <year>   rustfmt style edition
  --emit <files|stdout>    write files in place (default) or print them
  -v, --verbose            report unchanged and excluded files
  -h, --help               print this help
  -V, --version            print the version

Exit status: 0 success, 1 --check found changes, 2 error. A file that fails
is left byte-for-byte unchanged.
";

/// Which binary was invoked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Entry {
    QuarkFmt,
    /// `cargo-quark`, run by Cargo as `cargo-quark quark fmt ...`.
    CargoQuark,
}

#[derive(Debug, Default)]
struct Options {
    check: bool,
    view_only: bool,
    all: bool,
    stdin: bool,
    stdin_filepath: Option<PathBuf>,
    manifest_path: Option<PathBuf>,
    emit_stdout: bool,
    verbose: bool,
    overrides: Overrides,
    paths: Vec<PathBuf>,
}

enum Parsed {
    Run(Box<Options>),
    Print(String),
}

/// Runs the command with the process's arguments and streams.
pub fn main(entry: Entry) -> ExitCode {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let code = run(
        entry,
        args,
        &mut io::stdin().lock(),
        &mut io::stdout().lock(),
        &mut io::stderr(),
    );
    ExitCode::from(code)
}

/// Runs the command; returns the exit status.
pub fn run(
    entry: Entry,
    mut args: Vec<OsString>,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    if entry == Entry::CargoQuark {
        // Cargo passes the subcommand name first; accept a direct
        // `cargo-quark fmt` too.
        if args.first().is_some_and(|a| a == "quark") {
            args.remove(0);
        }
        if args.first().is_none_or(|a| a != "fmt") {
            let _ = writeln!(
                stderr,
                "usage: cargo quark fmt [options] [<path>...]\n\n{USAGE}"
            );
            return 2;
        }
        args.remove(0);
    }
    let opts = match parse(args) {
        Ok(Parsed::Run(opts)) => opts,
        Ok(Parsed::Print(text)) => {
            let _ = stdout.write_all(text.as_bytes());
            return 0;
        }
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}\n\nRun with --help for usage.");
            return 2;
        }
    };
    if opts.stdin {
        run_stdin(&opts, stdin, stdout, stderr)
    } else {
        run_files(&opts, stdout, stderr)
    }
}

fn parse(args: Vec<OsString>) -> Result<Parsed, String> {
    let mut opts = Options::default();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        let Some(text) = arg.to_str() else {
            opts.paths.push(arg.into());
            continue;
        };
        if !text.starts_with('-') || text == "-" {
            opts.paths.push(arg.into());
            continue;
        }
        // Accept both `--flag value` and `--flag=value`.
        let (flag, inline) = match text.split_once('=') {
            Some((f, v)) if f.starts_with("--") => (f.to_owned(), Some(OsString::from(v))),
            _ => (text.to_owned(), None),
        };
        let mut value = |name: &str| {
            inline
                .clone()
                .or_else(|| args.next())
                .ok_or_else(|| format!("{name} needs a value"))
        };
        match flag.as_str() {
            "--check" => opts.check = true,
            "--view-only" => opts.view_only = true,
            "--all" => opts.all = true,
            "--stdin" => opts.stdin = true,
            "-v" | "--verbose" => opts.verbose = true,
            "-h" | "--help" => return Ok(Parsed::Print(USAGE.to_owned())),
            "-V" | "--version" => {
                let version = env!("CARGO_PKG_VERSION");
                return Ok(Parsed::Print(format!(
                    "quark-fmt {version} (style_version 1)\n"
                )));
            }
            "--stdin-filepath" => {
                opts.stdin = true;
                opts.stdin_filepath = Some(value(&flag)?.into());
            }
            "--manifest-path" => opts.manifest_path = Some(value(&flag)?.into()),
            "--config-path" => opts.overrides.config_path = Some(value(&flag)?.into()),
            "--edition" => opts.overrides.edition = Some(utf8(value(&flag)?)?),
            "--style-edition" => opts.overrides.style_edition = Some(utf8(value(&flag)?)?),
            "--emit" => match utf8(value(&flag)?)?.as_str() {
                "files" => opts.emit_stdout = false,
                "stdout" => opts.emit_stdout = true,
                other => return Err(format!("unsupported --emit {other}; use files or stdout")),
            },
            _ => return Err(format!("unsupported argument `{text}`")),
        }
    }
    if opts.stdin && (opts.all || !opts.paths.is_empty()) {
        return Err("--stdin takes no paths and no --all".into());
    }
    if opts.all && !opts.paths.is_empty() {
        return Err("--all takes no paths".into());
    }
    if !opts.stdin && !opts.all && opts.paths.is_empty() {
        return Err("nothing to format; pass paths, --all, or --stdin".into());
    }
    if opts.check && opts.emit_stdout {
        return Err("--check and --emit stdout are exclusive".into());
    }
    // rustfmt resolves a relative --config-path against the working
    // directory; make it absolute so per-file lookups agree.
    if let Some(p) = &mut opts.overrides.config_path {
        *p = absolute(p).map_err(|e| format!("--config-path {}: {e}", p.display()))?;
    }
    Ok(Parsed::Run(Box::new(opts)))
}

fn utf8(value: OsString) -> Result<String, String> {
    value
        .into_string()
        .map_err(|v| format!("not UTF-8: {}", v.to_string_lossy()))
}

fn absolute(path: &Path) -> io::Result<PathBuf> {
    std::path::absolute(path)
}

/// Editor mode: the buffer arrives on stdin and leaves on stdout. Files are
/// never touched; on failure stdout stays empty so the editor keeps its
/// buffer.
fn run_stdin(
    opts: &Options,
    stdin: &mut dyn Read,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> u8 {
    let mut source = String::new();
    if let Err(e) = stdin.read_to_string(&mut source) {
        let _ = writeln!(stderr, "error: <stdin>: {e}");
        return 2;
    }
    let context = match &opts.stdin_filepath {
        Some(p) => absolute(p),
        None => std::env::current_dir().map(|d| d.join("<stdin>")),
    };
    let context = match context {
        Ok(p) => p,
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}");
            return 2;
        }
    };
    let name = opts
        .stdin_filepath
        .as_deref()
        .unwrap_or(Path::new("<stdin>"));
    let mut resolver = Resolver::new(opts.overrides.clone());
    let dir = context.parent().unwrap_or(Path::new("/"));
    let settings = match resolver.settings(dir, None) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}");
            return 2;
        }
    };
    let formatted = if settings.quark.excludes(&context) {
        source.clone()
    } else {
        match format_source(&source, &settings, opts.view_only, dir) {
            Ok(f) => f,
            Err(problem) => {
                let _ = writeln!(stderr, "{}", problem.render(name, &source));
                return 2;
            }
        }
    };
    if opts.check {
        if formatted == source {
            return 0;
        }
        let _ = stdout.write_all(diff(name, &source, &formatted).as_bytes());
        return 1;
    }
    let _ = stdout.write_all(formatted.as_bytes());
    0
}

/// What happened to one file.
enum Outcome {
    Unchanged,
    Excluded,
    /// Reformatted: written, or reported with its diff under `--check`.
    Changed {
        original: String,
        formatted: String,
    },
    /// Left unchanged; `source` locates the problem's offset.
    Failed {
        problem: Problem,
        source: String,
    },
}

impl Outcome {
    fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            problem: Problem::new(message),
            source: String::new(),
        }
    }
}

fn run_files(opts: &Options, stdout: &mut dyn Write, stderr: &mut dyn Write) -> u8 {
    let selected = if opts.all {
        workspace::members(opts.manifest_path.as_deref())
    } else {
        workspace::paths(&opts.paths)
    };
    let selected = match selected {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}");
            return 2;
        }
    };

    // Resolve configuration up front, on one thread, so lookups are cached
    // and a bad config file fails before any file is written.
    let mut resolver = Resolver::new(opts.overrides.clone());
    let mut work = Vec::with_capacity(selected.len());
    for file in selected {
        let dir = file.path.parent().unwrap_or(Path::new("/")).to_owned();
        match resolver.settings(&dir, file.edition.as_deref()) {
            Ok(settings) => work.push((file, settings)),
            Err(e) => {
                let _ = writeln!(stderr, "error: {e}");
                return 2;
            }
        }
    }

    let outcomes = parallel_map(&work, |(file, settings)| process(opts, file, settings));

    let cwd = std::env::current_dir().unwrap_or_default();
    let mut code = 0;
    for ((file, _), outcome) in work.iter().zip(outcomes) {
        let name = file.path.strip_prefix(&cwd).unwrap_or(&file.path);
        match outcome {
            Outcome::Unchanged if opts.emit_stdout => {
                let _ = stdout.write_all(fs::read(&file.path).unwrap_or_default().as_slice());
            }
            Outcome::Unchanged if opts.verbose => {
                let _ = writeln!(stderr, "unchanged {}", name.display());
            }
            Outcome::Excluded if opts.verbose => {
                let _ = writeln!(stderr, "excluded by quark-fmt.toml: {}", name.display());
            }
            Outcome::Unchanged | Outcome::Excluded => {}
            Outcome::Changed {
                original,
                formatted,
            } => {
                if opts.check {
                    let _ = stdout.write_all(diff(name, &original, &formatted).as_bytes());
                    code = code.max(1);
                } else if opts.emit_stdout {
                    let _ = stdout.write_all(formatted.as_bytes());
                } else if opts.verbose {
                    let _ = writeln!(stderr, "formatted {}", name.display());
                }
            }
            Outcome::Failed { problem, source } => {
                let _ = writeln!(stderr, "{}", problem.render(name, &source));
                code = 2;
            }
        }
    }
    code
}

/// Formats one file and, unless checking or printing, writes it back.
fn process(opts: &Options, file: &Selected, settings: &Settings) -> Outcome {
    if settings.quark.excludes(&file.path) {
        return Outcome::Excluded;
    }
    let bytes = match fs::read(&file.path) {
        Ok(b) => b,
        Err(e) => return Outcome::failed(e.to_string()),
    };
    let Ok(original) = String::from_utf8(bytes) else {
        return Outcome::failed("not UTF-8");
    };
    let dir = file.path.parent().unwrap_or(Path::new("/"));
    let formatted = match format_source(&original, settings, opts.view_only, dir) {
        Ok(f) => f,
        Err(problem) => {
            return Outcome::Failed {
                problem,
                source: original,
            };
        }
    };
    if formatted == original {
        return Outcome::Unchanged;
    }
    if !opts.check
        && !opts.emit_stdout
        && let Err(e) = write_atomic(&file.path, original.as_bytes(), formatted.as_bytes())
    {
        return Outcome::failed(format!("{e}; file left unchanged"));
    }
    Outcome::Changed {
        original,
        formatted,
    }
}

/// Replaces `path` with `contents` by renaming a sibling temporary file over
/// it, so readers see the old or the new file and never a partial one. The
/// temporary must share the target's filesystem for the rename to be atomic,
/// so it lives in the same directory, under a hidden name, and is removed if
/// anything fails. Refuses to write when the file changed since it was read.
fn write_atomic(path: &Path, expected: &[u8], contents: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let permissions = fs::metadata(path)?.permissions();
    let mut temp = tempfile::Builder::new()
        .prefix(".quark-fmt-")
        .suffix(".tmp")
        .tempfile_in(dir)?;
    temp.write_all(contents)?;
    temp.as_file().set_permissions(permissions)?;
    temp.as_file().sync_all()?;
    if fs::read(path)? != expected {
        return Err(io::Error::other("changed on disk while formatting"));
    }
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Maps `f` over `items` on up to `MAX_JOBS` threads, keeping input order.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let jobs = std::thread::available_parallelism()
        .map_or(1, |n| n.get())
        .min(MAX_JOBS);
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, R)> = std::thread::scope(|s| {
        let workers: Vec<_> = (0..jobs.min(items.len()))
            .map(|_| {
                s.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        done.push((i, f(item)));
                    }
                    done
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|w| w.join().expect("formatter thread panicked"))
            .collect()
    });
    results.sort_by_key(|(i, _)| *i);
    results.into_iter().map(|(_, r)| r).collect()
}

fn diff(name: &Path, original: &str, formatted: &str) -> String {
    let name = name.to_string_lossy();
    similar::TextDiff::from_lines(original, formatted)
        .unified_diff()
        .context_radius(3)
        .header(&format!("a/{name}"), &format!("b/{name}"))
        .to_string()
}

/// Why a file was left unchanged.
#[derive(Debug)]
pub struct Problem {
    /// Byte offset in the source the message refers to, when known.
    pub offset: Option<usize>,
    pub message: String,
}

impl Problem {
    fn new(message: impl Into<String>) -> Self {
        Self {
            offset: None,
            message: message.into(),
        }
    }

    fn render(&self, name: &Path, source: &str) -> String {
        let name = name.display();
        match self.offset {
            Some(offset) => {
                let before = &source[..offset.min(source.len())];
                let line = before.matches('\n').count() + 1;
                let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
                format!("error: {name}:{line}:{column}: {}", self.message)
            }
            None => format!("error: {name}: {}", self.message),
        }
    }
}

/// The complete transformation of one file. Full mode composes rustfmt and
/// the view pass and only accepts a fixed point; view-only mode requires a
/// second view pass to change nothing.
fn format_source(
    source: &str,
    settings: &Settings,
    view_only: bool,
    dir: &Path,
) -> Result<String, Problem> {
    if view_only {
        let once = engine::view_pass(source, settings)?;
        let twice = engine::view_pass(&once, settings)?;
        if twice != once {
            return Err(Problem::new(
                "view formatting is not idempotent; file left unchanged",
            ));
        }
        return Ok(once);
    }
    // Rustfmt's own output can move a view, which changes the width the view
    // printer sees, which can change rustfmt's next decision. Accept only a
    // state that one more round leaves alone, within a fixed budget.
    let mut seen = vec![source.to_owned()];
    let mut current = compose(source, settings, dir)?;
    for _ in 1..MAX_TRANSFORMS {
        let next = compose(&current, settings, dir)?;
        if next == current {
            return Ok(current);
        }
        if seen.contains(&next) {
            break;
        }
        seen.push(std::mem::replace(&mut current, next));
    }
    Err(Problem::new(format!(
        "rustfmt and the view printer did not converge within {MAX_TRANSFORMS} passes; \
         file left unchanged"
    )))
}

/// One round: rustfmt the whole file, put each view body back exactly as it
/// was (rustfmt's macro heuristics must not touch templates), then format the
/// views.
fn compose(source: &str, settings: &Settings, dir: &Path) -> Result<String, Problem> {
    let before = engine::view_bodies(source, settings)?;
    let rustfmt = rustfmt(source, settings, dir)?;
    let after = engine::view_bodies(&rustfmt, settings)?;
    let restored = restore_bodies(source, &before, &rustfmt, &after)?;
    engine::view_pass(&restored, settings)
}

/// Copies each original view body over its counterpart in rustfmt's output.
/// Both lists are outermost invocations in source order; ranges cover the
/// body including its delimiters.
fn restore_bodies(
    original: &str,
    before: &[Range<usize>],
    formatted: &str,
    after: &[Range<usize>],
) -> Result<String, Problem> {
    if before.len() != after.len() {
        return Err(Problem::new(format!(
            "rustfmt changed the number of view! invocations ({} before, {} after)",
            before.len(),
            after.len()
        )));
    }
    let mut out = String::with_capacity(formatted.len());
    let mut copied = 0;
    for (old, new) in before.iter().zip(after) {
        let (old_text, new_text) = (&original[old.clone()], &formatted[new.clone()]);
        if old_text.chars().next() != new_text.chars().next() {
            return Err(Problem {
                offset: Some(old.start),
                message: "rustfmt changed a view! delimiter".into(),
            });
        }
        out.push_str(&formatted[copied..new.start]);
        out.push_str(old_text);
        copied = new.end;
    }
    out.push_str(&formatted[copied..]);
    Ok(out)
}

/// Runs the toolchain's rustfmt on the whole file through stdin, so it never
/// reads or writes files and never follows `mod` declarations.
/// `QUARK_FMT_RUSTFMT` names a different binary.
fn rustfmt(source: &str, settings: &Settings, dir: &Path) -> Result<String, Problem> {
    let program = std::env::var_os("QUARK_FMT_RUSTFMT").unwrap_or_else(|| "rustfmt".into());
    let mut cmd = Command::new(&program);
    // The working directory picks the pinned toolchain via rustup. A
    // `--stdin-filepath` for an unsaved file may name a missing directory.
    let cwd = dir
        .ancestors()
        .find(|d| d.is_dir())
        .unwrap_or(Path::new("/"));
    cmd.current_dir(cwd)
        .args(["--emit", "stdout", "--edition", &settings.edition])
        .env_remove("RUSTFMT")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(style) = &settings.style_edition {
        cmd.args(["--style-edition", style]);
    }
    if let Some(path) = &settings.rustfmt.path {
        cmd.arg("--config-path").arg(path);
    }
    let program = program.to_string_lossy();
    let mut child = cmd
        .spawn()
        .map_err(|e| Problem::new(format!("{program}: {e}")))?;
    let mut input = child.stdin.take().expect("piped stdin");
    // Write from another thread: a large file can fill the stdout pipe
    // before rustfmt has read all of stdin.
    let source = source.to_owned();
    let writer = std::thread::spawn(move || input.write_all(source.as_bytes()));
    let out = child
        .wait_with_output()
        .map_err(|e| Problem::new(format!("{program}: {e}")))?;
    let _ = writer.join();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Problem::new(format!("{program} failed: {}", stderr.trim())));
    }
    String::from_utf8(out.stdout).map_err(|_| Problem::new(format!("{program}: output not UTF-8")))
}

/// The seam to the formatter library.
mod engine {
    use std::ops::Range;

    use super::Problem;
    use crate::config::Settings;

    /// Outermost selected view invocations, in source order, as byte ranges
    /// of their delimited bodies.
    pub fn view_bodies(_source: &str, _settings: &Settings) -> Result<Vec<Range<usize>>, Problem> {
        Ok(Vec::new())
    }

    /// The view printer over a whole file.
    pub fn view_pass(source: &str, _settings: &Settings) -> Result<String, Problem> {
        Ok(source.to_owned())
    }
}
