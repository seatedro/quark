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

pub use discover::{Delimiter, Invocation, discover};
pub use printer::rust::{
    NestedViews, PassThrough, Placement, ProviderError, RustContext, RustFragment, RustProvider,
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
pub fn format_source(
    source: &str,
    options: &FormatOptions,
    provider: &dyn RustProvider,
) -> FormatOutcome {
    let _ = (source, options, provider);
    FormatOutcome::default()
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
