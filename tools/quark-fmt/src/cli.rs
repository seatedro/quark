//! The `quark-fmt` and `cargo quark fmt` command line: argument parsing,
//! file selection, the rustfmt-then-views transaction per file, `--check`
//! diffs, and atomic writes.

use std::ffi::OsString;
use std::fs;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::config::{self, Overrides, Resolver, Settings};
use crate::workspace::{self, Selected};
use crate::{
    Coverage, Diagnostic, FormatOptions, Invocation, NewlineStyle, PassThrough, Severity,
    apply_edits,
};

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
    let name = opts
        .stdin_filepath
        .as_deref()
        .unwrap_or(Path::new("<stdin>"));
    let mut source = String::new();
    if let Err(e) = stdin.read_to_string(&mut source) {
        let _ = writeln!(stderr, "error: {}: {e}", name.display());
        return 2;
    }
    let context = match &opts.stdin_filepath {
        Some(p) => absolute(p),
        None => std::env::current_dir().map(|d| d.join("<stdin>")),
    };
    let dir = context
        .as_deref()
        .ok()
        .and_then(Path::parent)
        .unwrap_or(Path::new("/"));
    let settings = match Resolver::new(opts.overrides.clone()).settings(dir, None) {
        Ok(s) => s,
        Err(e) => {
            let _ = writeln!(stderr, "error: {e}");
            return 2;
        }
    };
    let excluded = context.as_deref().is_ok_and(|c| settings.quark.excludes(c));
    let formatted = if excluded {
        Formatted::unchanged(&source)
    } else {
        match format_source(&source, &settings, opts.view_only, dir) {
            Ok(f) => f,
            Err(problems) => {
                report(stderr, name, &problems, opts.verbose);
                return 2;
            }
        }
    };
    report(stderr, name, &formatted.notes, opts.verbose);
    if opts.check {
        if strict_failure(&formatted.notes) {
            return 2;
        }
        if formatted.text == source {
            return 0;
        }
        let _ = stdout.write_all(diff(name, &source, &formatted.text).as_bytes());
        return 1;
    }
    let _ = stdout.write_all(formatted.text.as_bytes());
    0
}

/// What happened to one file.
enum Outcome {
    Excluded,
    Done {
        original: String,
        formatted: Formatted,
    },
    /// Left unchanged.
    Failed(Vec<Problem>),
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
    let mut files = [0usize; 4]; // changed, unchanged, excluded, failed
    let mut views = Coverage::default();
    for ((file, _), outcome) in work.iter().zip(outcomes) {
        let name = file.path.strip_prefix(&cwd).unwrap_or(&file.path);
        match outcome {
            Outcome::Excluded => {
                files[2] += 1;
                if opts.verbose {
                    let _ = writeln!(stderr, "excluded by quark-fmt.toml: {}", name.display());
                }
            }
            Outcome::Failed(problems) => {
                files[3] += 1;
                report(stderr, name, &problems, opts.verbose);
                code = 2;
            }
            Outcome::Done {
                original,
                formatted,
            } => {
                report(stderr, name, &formatted.notes, opts.verbose);
                add_coverage(&mut views, formatted.coverage);
                let changed = formatted.text != original;
                files[usize::from(!changed)] += 1;
                if opts.check && strict_failure(&formatted.notes) {
                    code = 2;
                }
                if opts.emit_stdout {
                    let _ = stdout.write_all(formatted.text.as_bytes());
                } else if changed && opts.check {
                    let _ = stdout.write_all(diff(name, &original, &formatted.text).as_bytes());
                    code = code.max(1);
                } else if changed && opts.verbose {
                    let _ = writeln!(stderr, "formatted {}", name.display());
                }
            }
        }
    }
    if opts.verbose {
        let [changed, unchanged, excluded, failed] = files;
        let verb = if opts.check { "to format" } else { "formatted" };
        let _ = writeln!(
            stderr,
            "files: {changed} {verb}, {unchanged} unchanged, {excluded} excluded, {failed} failed; \
             views: {} found, {} printed, {} skipped, {} failed",
            views.discovered, views.formatted, views.skipped, views.failed
        );
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
        Err(e) => return Outcome::Failed(vec![Problem::error(e.to_string())]),
    };
    let Ok(original) = String::from_utf8(bytes) else {
        return Outcome::Failed(vec![Problem::error("not UTF-8")]);
    };
    let dir = file.path.parent().unwrap_or(Path::new("/"));
    let formatted = match format_source(&original, settings, opts.view_only, dir) {
        Ok(f) => f,
        Err(problems) => return Outcome::Failed(problems),
    };
    if formatted.text != original
        && !opts.check
        && !opts.emit_stdout
        && let Err(e) = write_atomic(&file.path, original.as_bytes(), formatted.text.as_bytes())
    {
        return Outcome::Failed(vec![Problem::error(format!("{e}; file left unchanged"))]);
    }
    Outcome::Done {
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

fn add_coverage(total: &mut Coverage, file: Coverage) {
    total.discovered += file.discovered;
    total.formatted += file.formatted;
    total.skipped += file.skipped;
    total.failed += file.failed;
}

/// A diagnostic for the user, located in the text it was found in.
#[derive(Clone, Debug)]
pub struct Problem {
    pub severity: Severity,
    /// 1-based line and column.
    pub location: Option<(usize, usize)>,
    pub message: String,
}

impl Problem {
    fn error(message: impl Into<String>) -> Self {
        Self {
            severity: Severity::Error,
            location: None,
            message: message.into(),
        }
    }

    fn from_diagnostic(d: &Diagnostic, text: &str) -> Self {
        let location = d.range.as_ref().map(|r| {
            let before = &text[..r.start.min(text.len())];
            let line = before.matches('\n').count() + 1;
            let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
            (line, column)
        });
        Self {
            severity: d.severity,
            location,
            message: d.message.clone(),
        }
    }
}

/// Prints errors and warnings; informational notes (skips) only when verbose.
fn report(out: &mut dyn Write, name: &Path, problems: &[Problem], verbose: bool) {
    for p in problems {
        let label = match p.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
            Severity::Info if verbose => "note",
            Severity::Info => continue,
        };
        let _ = match p.location {
            Some((line, col)) => {
                writeln!(
                    out,
                    "{label}: {}:{line}:{col}: {}",
                    name.display(),
                    p.message
                )
            }
            None => writeln!(out, "{label}: {}: {}", name.display(), p.message),
        };
    }
}

/// `--check` is strict: a warning means part of the file was kept as
/// written for a reason the formatter could not resolve (a view hidden in
/// another macro, embedded Rust it could not place), and CI must not report
/// that file as clean.
fn strict_failure(notes: &[Problem]) -> bool {
    notes.iter().any(|p| p.severity == Severity::Warning)
}

/// A successfully formatted file.
#[derive(Debug)]
struct Formatted {
    text: String,
    /// Warnings and notes about parts kept as written.
    notes: Vec<Problem>,
    coverage: Coverage,
}

impl Formatted {
    /// `source` as the result, with nothing to report.
    fn unchanged(source: &str) -> Self {
        Self {
            text: source.to_owned(),
            notes: Vec::new(),
            coverage: Coverage::default(),
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
) -> Result<Formatted, Vec<Problem>> {
    if view_only {
        let once = view_pass(source, settings)?;
        let twice = view_pass(&once.text, settings)?;
        if twice.text != once.text {
            return Err(vec![Problem::error(
                "view formatting is not idempotent; file left unchanged",
            )]);
        }
        return Ok(once);
    }
    // Rustfmt's output can move a view, which changes the width the view
    // printer sees, which can change rustfmt's next decision. Accept only a
    // state that one more round leaves alone, within a fixed budget.
    // Most files hold no views. Their result is rustfmt's alone, as `cargo
    // fmt` would write it, so the verifying round is skipped.
    let discovery = crate::discover(source, &settings.quark.macro_names);
    if discovery.is_ok_and(|d| d.invocations.is_empty() && d.diagnostics.is_empty()) {
        let text = rustfmt(source, settings, dir).map_err(|p| vec![p])?;
        return Ok(Formatted::unchanged(&text));
    }
    let mut seen = vec![source.to_owned()];
    let mut current = compose(source, settings, dir, true)?;
    for _ in 1..MAX_TRANSFORMS {
        let next = compose(&current.text, settings, dir, false)?;
        if next.text == current.text {
            // The verifying round's notes are located in the final text.
            return Ok(next);
        }
        if seen.contains(&next.text) {
            break;
        }
        seen.push(std::mem::replace(&mut current, next).text);
    }
    Err(vec![Problem::error(format!(
        "rustfmt and the view printer did not converge within {MAX_TRANSFORMS} passes; \
         file left unchanged"
    ))])
}

/// One round: rustfmt the whole file, put each view body back exactly as it
/// was (rustfmt's macro heuristics must not touch templates), then format the
/// views. `first` marks the round whose input is the user's source.
fn compose(
    source: &str,
    settings: &Settings,
    dir: &Path,
    first: bool,
) -> Result<Formatted, Vec<Problem>> {
    let before = discover(source, settings)?;
    let rustfmt = rustfmt(source, settings, dir).map_err(|p| vec![p])?;
    let after = discover(&rustfmt, settings)?;
    let restored = restore_bodies(source, &before, &rustfmt, &after)?;
    view_pass(&restored, settings).map_err(|problems| {
        // These positions are in rustfmt's output, which the user never
        // sees. On the first round the views alone usually fail the same way
        // on the original, which gives positions in the user's file.
        match first.then(|| view_pass(source, settings)) {
            Some(Err(original)) => original,
            _ => problems
                .into_iter()
                .map(|p| Problem {
                    location: None,
                    ..p
                })
                .collect(),
        }
    })
}

fn discover(source: &str, settings: &Settings) -> Result<Vec<Invocation>, Vec<Problem>> {
    crate::discover(source, &settings.quark.macro_names)
        .map(|d| d.invocations)
        .map_err(|d| vec![Problem::from_diagnostic(&d, source)])
}

/// Copies each original view body over its counterpart in rustfmt's output.
/// Both lists are outermost invocations in source order.
fn restore_bodies(
    original: &str,
    before: &[Invocation],
    formatted: &str,
    after: &[Invocation],
) -> Result<String, Vec<Problem>> {
    let changed = before.len() != after.len()
        || before
            .iter()
            .zip(after)
            .any(|(old, new)| old.path != new.path || old.delimiter != new.delimiter);
    if changed {
        return Err(vec![Problem::error(
            "rustfmt changed which view! invocations the file has; file left unchanged",
        )]);
    }
    let mut out = String::with_capacity(formatted.len());
    let mut copied = 0;
    for (old, new) in before.iter().zip(after) {
        out.push_str(&formatted[copied..new.body.start]);
        out.push_str(&original[old.body.clone()]);
        copied = new.body.end;
    }
    out.push_str(&formatted[copied..]);
    Ok(out)
}

/// The view printer over a whole file.
fn view_pass(source: &str, settings: &Settings) -> Result<Formatted, Vec<Problem>> {
    let r = &settings.rustfmt;
    let newline_style = match r.newline_style {
        config::NewlineStyle::Auto => NewlineStyle::Auto,
        config::NewlineStyle::Unix => NewlineStyle::Unix,
        config::NewlineStyle::Windows => NewlineStyle::Windows,
        config::NewlineStyle::Native if cfg!(windows) => NewlineStyle::Windows,
        config::NewlineStyle::Native => NewlineStyle::Unix,
    };
    let options = FormatOptions {
        max_width: r.max_width,
        tab_spaces: r.tab_spaces,
        hard_tabs: r.hard_tabs,
        newline_style,
        macro_names: settings.quark.macro_names.clone(),
    };
    let outcome = crate::format_source(source, &options, &PassThrough);
    let notes = outcome
        .diagnostics
        .iter()
        .map(|d| Problem::from_diagnostic(d, source))
        .collect();
    if outcome.has_errors() {
        return Err(notes);
    }
    Ok(Formatted {
        text: apply_edits(source, &outcome.edits),
        notes,
        coverage: outcome.coverage,
    })
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
        .map_err(|e| Problem::error(format!("{program}: {e}")))?;
    let mut input = child.stdin.take().expect("piped stdin");
    // Write from another thread: a large file can fill the stdout pipe
    // before rustfmt has read all of stdin.
    let source = source.to_owned();
    let writer = std::thread::spawn(move || input.write_all(source.as_bytes()));
    let out = child
        .wait_with_output()
        .map_err(|e| Problem::error(format!("{program}: {e}")))?;
    let _ = writer.join();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(Problem::error(format!(
            "{program} failed: {}",
            stderr.trim()
        )));
    }
    String::from_utf8(out.stdout)
        .map_err(|_| Problem::error(format!("{program}: output not UTF-8")))
}
