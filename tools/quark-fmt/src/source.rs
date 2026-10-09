//! Byte edits, diagnostics, and line bookkeeping over the original source,
//! which stays the authority for every byte the formatter does not change.

use std::ops::Range;

/// Replace `range` of the original source with `replacement`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub range: Range<usize>,
    pub replacement: String,
}

/// Applies disjoint edits in one forward copy. Edits must be sorted,
/// non-overlapping, and on UTF-8 boundaries, as [`crate::format_source`]
/// returns them.
pub fn apply_edits(source: &str, edits: &[TextEdit]) -> String {
    debug_assert!(
        verify_edits(source, edits).is_ok(),
        "{:?}",
        verify_edits(source, edits)
    );
    let mut out = String::with_capacity(source.len());
    let mut at = 0;
    for edit in edits {
        out.push_str(&source[at..edit.range.start]);
        out.push_str(&edit.replacement);
        at = edit.range.end;
    }
    out.push_str(&source[at..]);
    out
}

/// The edit-plan invariant: ordered, inside the source, disjoint, and on
/// character boundaries.
pub fn verify_edits(source: &str, edits: &[TextEdit]) -> Result<(), String> {
    let mut prev_end = 0;
    for edit in edits {
        let Range { start, end } = edit.range;
        if start < prev_end || end < start || end > source.len() {
            return Err(format!("edit {start}..{end} out of order or bounds"));
        }
        if !source.is_char_boundary(start) || !source.is_char_boundary(end) {
            return Err(format!("edit {start}..{end} splits a character"));
        }
        prev_end = end;
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    /// The file stays unchanged.
    Error,
    /// Formatting went ahead with a reduced result, such as an embedded
    /// fragment kept as written. A strict check may treat it as failure.
    Warning,
    /// Coverage notes: skip directives and exclusions.
    Info,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticKind {
    /// The file is not valid Rust.
    RustParse,
    /// A selected invocation is not valid view syntax.
    ViewParse,
    /// A comment or blank line could not be given a place in the output.
    Trivia,
    /// The candidate's tokens differ from the original's.
    TokenMismatch,
    /// The candidate changed between two passes.
    Unstable,
    /// The embedded-Rust provider failed; the fragment kept its source.
    Provider,
    /// `// quark-fmt: skip` or `#[rustfmt::skip]` kept an invocation as is.
    Skipped,
    /// The invocation uses a span-sensitive macro such as `line!`.
    SpanSensitive,
    /// A selected macro inside another macro's opaque body.
    HiddenMacro,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub severity: Severity,
    pub kind: DiagnosticKind,
    /// Byte range in the original source, when one applies.
    pub range: Option<Range<usize>>,
    pub message: String,
}

impl Diagnostic {
    pub fn error(
        kind: DiagnosticKind,
        range: Option<Range<usize>>,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            severity: Severity::Error,
            kind,
            range,
            message: message.into(),
        }
    }

    pub fn warning(
        kind: DiagnosticKind,
        range: Option<Range<usize>>,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            severity: Severity::Warning,
            kind,
            range,
            message: message.into(),
        }
    }

    pub fn info(
        kind: DiagnosticKind,
        range: Option<Range<usize>>,
        message: impl Into<String>,
    ) -> Self {
        Diagnostic {
            severity: Severity::Info,
            kind,
            range,
            message: message.into(),
        }
    }
}

/// Display columns of `text` on one line: tabs count `tab_spaces`, other
/// characters their Unicode width.
pub fn columns(text: &str, tab_spaces: usize) -> usize {
    text.chars()
        .map(|c| match c {
            '\t' => tab_spaces,
            c => unicode_width::UnicodeWidthChar::width(c).unwrap_or(0),
        })
        .sum()
}

/// Indentation of `columns` display columns: tabs then spaces with
/// `hard_tabs`, otherwise spaces.
pub fn indent_string(columns: usize, tab_spaces: usize, hard_tabs: bool) -> String {
    if hard_tabs && tab_spaces > 0 {
        let mut s = "\t".repeat(columns / tab_spaces);
        s.push_str(&" ".repeat(columns % tab_spaces));
        s
    } else {
        " ".repeat(columns)
    }
}
