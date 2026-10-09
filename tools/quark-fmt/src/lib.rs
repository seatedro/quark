//! Formats quark `view!` templates inside Rust source.
//!
//! The formatter only changes whitespace between tokens: attribute order,
//! utility order, literal bytes, comments, and syntax choices survive. The
//! public path is pure: [`format_source`] takes source text, options, and
//! a [`RustProvider`] for embedded Rust, and returns byte edits plus
//! diagnostics. File enumeration, subprocesses, and writes belong to the
//! CLI layer.
//!
//! Modules:
//! - `source`: edits, diagnostics, line and column bookkeeping.
//! - `discover`: finds selected macro invocations with a syn visitor.
//! - `trivia`: lexes view bodies and assigns comments and blank lines.
//! - `doc`: the document IR and its width-aware printer.
//! - `printer`: template layout rules and the embedded-Rust provider contract.
//! - `verify`: token equivalence of a candidate against the original.
//! - `embedded`, `rustfmt`: the rustfmt-backed provider (stream C).
//! - `cli`, `workspace`, `config`: the command line (stream D).

pub mod cli;
pub mod config;
pub mod discover;
pub mod doc;
pub mod embedded;
pub mod printer;
pub mod rustfmt;
pub mod source;
pub mod trivia;
pub mod verify;
pub mod workspace;

pub use discover::{Delimiter, Discovery, Invocation, discover};
pub use printer::rust::{
    Layout, LayoutLine, LayoutRequest, NestedViews, PassThrough, ProviderError, RustContext,
    RustFragment, RustProvider,
};
pub use source::{Diagnostic, DiagnosticKind, Severity, TextEdit, apply_edits};

/// How newlines introduced outside immutable slices are spelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewlineStyle {
    /// The style of the first newline in the file, or `\n` without one.
    Auto,
    Unix,
    Windows,
}

/// Layout settings shared with rustfmt plus quark's own keys.
#[derive(Clone, Debug)]
pub struct FormatOptions {
    /// rustfmt `max_width`: a soft limit for preserved literals and comments.
    pub max_width: usize,
    /// rustfmt `tab_spaces`: columns per indentation level.
    pub tab_spaces: usize,
    /// rustfmt `hard_tabs`: indent with tabs instead of spaces.
    pub hard_tabs: bool,
    pub newline_style: NewlineStyle,
    /// Complete macro paths treated as quark views, compared after
    /// stripping a leading `::`. Default `["view", "quark::view"]`.
    pub macro_names: Vec<String>,
}

impl Default for FormatOptions {
    fn default() -> Self {
        FormatOptions {
            max_width: 100,
            tab_spaces: 4,
            hard_tabs: false,
            newline_style: NewlineStyle::Auto,
            macro_names: vec!["view".to_owned(), "quark::view".to_owned()],
        }
    }
}

/// Counts for the coverage report: every discovered outermost invocation
/// lands in exactly one of the last three buckets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    /// Selected invocations, nested ones included.
    pub discovered: usize,
    /// Outermost invocations whose body was printed (changed or not).
    pub formatted: usize,
    /// Outermost invocations left alone by a skip directive.
    pub skipped: usize,
    /// Outermost invocations that could not be formatted.
    pub failed: usize,
}

/// The result of formatting one file.
#[derive(Clone, Debug, Default)]
pub struct FormatOutcome {
    /// Disjoint edits in ascending order. Empty whenever any diagnostic is
    /// an error: a file is formatted completely or not at all.
    pub edits: Vec<TextEdit>,
    pub diagnostics: Vec<Diagnostic>,
    pub coverage: Coverage,
}

impl FormatOutcome {
    /// True when a diagnostic forced the whole file to stay unchanged.
    pub fn has_errors(&self) -> bool {
        self.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Error)
    }
}

/// Formats every selected `view!` in `source`. Rust outside the
/// invocations is untouched; embedded Rust goes through `provider`.
///
/// The result is checked before it is returned: each new body must have
/// the original's tokens and parse as a view, and formatting the result
/// again must change nothing.
pub fn format_source(
    source: &str,
    options: &FormatOptions,
    provider: &dyn RustProvider,
) -> FormatOutcome {
    let mut outcome = format_once(source, options, provider);
    if !outcome.has_errors() && !outcome.edits.is_empty() {
        let formatted = apply_edits(source, &outcome.edits);
        let again = format_once(&formatted, options, provider);
        if again.has_errors() || !again.edits.is_empty() {
            let at = again.edits.first().map(|e| e.range.clone());
            outcome.diagnostics.push(Diagnostic::error(
                DiagnosticKind::Unstable,
                None,
                format!(
                    "formatting the result again changes it{}",
                    at.map(|r| format!(" at formatted bytes {r:?}"))
                        .unwrap_or_default()
                ),
            ));
            outcome.edits.clear();
        }
    }
    // Spans from this file's parse are no longer needed; free them.
    proc_macro2::extra::invalidate_current_thread_spans();
    outcome
}

fn format_once(
    source: &str,
    options: &FormatOptions,
    provider: &dyn RustProvider,
) -> FormatOutcome {
    let mut outcome = FormatOutcome::default();
    let (found, tokens) = match discover::discover_with_tokens(source, &options.macro_names) {
        Ok(found) => found,
        Err(diagnostic) => {
            outcome.diagnostics.push(diagnostic);
            return outcome;
        }
    };
    outcome.diagnostics.extend(found.diagnostics);
    let names = discover::normalize(&options.macro_names);
    let newline = match options.newline_style {
        NewlineStyle::Unix => "\n",
        NewlineStyle::Windows => "\r\n",
        NewlineStyle::Auto => match source.find('\n') {
            Some(i) if source[..i].ends_with('\r') => "\r\n",
            _ => "\n",
        },
    };
    let mut views = Vec::new();
    let starts: Vec<usize> = found.invocations.iter().map(|i| i.range.start).collect();
    for (k, (inv, tokens)) in found.invocations.iter().zip(tokens).enumerate() {
        outcome.coverage.discovered += 1;
        if inv.skipped {
            outcome.coverage.skipped += 1;
            outcome.diagnostics.push(Diagnostic::info(
                DiagnosticKind::Skipped,
                Some(inv.range.clone()),
                "skipped",
            ));
            continue;
        }
        if let Some(at) = span_sensitive(source, inv.body.clone()) {
            outcome.coverage.failed += 1;
            outcome.diagnostics.push(Diagnostic::error(
                DiagnosticKind::SpanSensitive,
                Some(at),
                "`line!` or `column!` in a view depends on its position; skip the file",
            ));
            continue;
        }
        let next = starts.get(k + 1).copied();
        let suffix = closing_suffix(source, inv.body.end, next, options.tab_spaces);
        match printer::build_view(
            source,
            tokens,
            inv.body.clone(),
            inv.delimiter,
            suffix,
            &names,
            &mut outcome.diagnostics,
        ) {
            Ok(view) => {
                outcome.coverage.discovered += view.count - 1;
                views.push((inv, view));
            }
            Err(diagnostic) => {
                outcome.coverage.failed += 1;
                outcome.diagnostics.push(diagnostic);
            }
        }
    }
    if outcome.has_errors() {
        return outcome;
    }

    let mut fragments = Vec::new();
    for (_, view) in &views {
        printer::fragments(view, &mut fragments);
    }
    provider.prepare(&fragments);
    let ctx = printer::Ctx {
        provider,
        cfg: doc::PrintConfig {
            max_width: options.max_width,
            tab_spaces: options.tab_spaces,
        },
        diagnostics: Default::default(),
    };
    // The end of the previous body and the output column there, for a view
    // that shares a line with the one before it.
    let mut prev: Option<(usize, usize)> = None;
    for (inv, mut view) in views {
        printer::resolve(&mut view, &ctx);
        let (indent, mut column) = line_position(source, inv.body.start, options.tab_spaces);
        if let Some((end, col)) = prev {
            let between = &source[end..inv.body.start];
            if !between.contains('\n') {
                column = col + source::columns(between, options.tab_spaces);
            }
        }
        let lines = printer::print(&view, &ctx, indent, column);
        let text = doc::render(&lines, options.tab_spaces, options.hard_tabs, newline);
        let original = &source[inv.body.clone()];
        if let Err(m) = verify::equivalent(original, &text) {
            outcome.coverage.failed += 1;
            outcome.diagnostics.push(Diagnostic::error(
                DiagnosticKind::TokenMismatch,
                Some(inv.body.start + m.before..inv.body.start + m.before),
                format!("formatting would change tokens: {}", m.message),
            ));
            continue;
        }
        outcome.coverage.formatted += 1;
        let end_col = match text.rfind('\n') {
            Some(i) => source::columns(&text[i + 1..], options.tab_spaces),
            None => column + source::columns(&text, options.tab_spaces),
        };
        prev = Some((inv.body.end, end_col));
        if text != original {
            outcome.edits.push(TextEdit {
                range: inv.body.clone(),
                replacement: text,
            });
        }
    }
    outcome.diagnostics.extend(ctx.diagnostics.into_inner());
    if outcome.has_errors() {
        outcome.edits.clear();
    }
    outcome
}

/// Indentation of the line holding `offset`, and the column of `offset`.
fn line_position(source: &str, offset: usize, tab_spaces: usize) -> (usize, usize) {
    let start = source[..offset].rfind('\n').map_or(0, |i| i + 1);
    let line = &source[start..offset];
    let ws = line.len() - line.trim_start_matches([' ', '\t']).len();
    (
        source::columns(&line[..ws], tab_spaces),
        source::columns(line, tab_spaces),
    )
}

/// Width of the closing delimiter at `offset` and the rest of its line, up
/// to the next view on that line, whose own width depends on this one.
fn closing_suffix(source: &str, offset: usize, next: Option<usize>, tab_spaces: usize) -> usize {
    let limit = next.map_or(source.len(), |n| n.max(offset));
    let rest = &source[offset..limit];
    let end = rest.find('\n').unwrap_or(rest.len());
    source::columns(rest[..end].trim_end(), tab_spaces)
}

/// The first `line!` or `column!` in `range`, whose value formatting
/// would change.
fn span_sensitive(source: &str, range: std::ops::Range<usize>) -> Option<std::ops::Range<usize>> {
    let toks = verify::tokens(&source[range.clone()]);
    toks.windows(2).find_map(|w| {
        (matches!(w[0].text, "line" | "column") && w[1].text == "!")
            .then(|| range.start + w[0].offset..range.start + w[1].offset + 1)
    })
}

/// [`format_source`] followed by [`apply_edits`]: the formatted text, or
/// the diagnostics that kept the file unchanged.
pub fn format_to_string(
    source: &str,
    options: &FormatOptions,
    provider: &dyn RustProvider,
) -> Result<String, Vec<Diagnostic>> {
    let outcome = format_source(source, options, provider);
    if outcome.has_errors() {
        return Err(outcome.diagnostics);
    }
    Ok(apply_edits(source, &outcome.edits))
}
