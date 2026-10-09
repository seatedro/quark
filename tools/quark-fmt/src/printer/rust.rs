//! The contract between the template printer and whatever formats the Rust
//! embedded in a template.
//!
//! The printer hands every Rust fragment of a file to
//! [`RustProvider::prepare`] before printing, so an implementation can
//! batch. While measuring groups it asks for a one-line spelling
//! ([`RustProvider::flat`]); when a fragment cannot stay on one line it
//! asks for a [`Layout`] hanging from a known template indentation
//! ([`RustProvider::layout`]). Nested selected views inside a fragment are
//! printed by the template printer through [`NestedViews`], so the
//! outermost invocation owns their bytes. [`PassThrough`] keeps fragments
//! as written apart from moving their continuation lines; the rustfmt
//! provider replaces it without changes to the printer.
//!
//! Every method takes `&self` because a provider calls back into
//! [`NestedViews::layout`], which prints a nested template and asks the
//! same provider about that template's fragments. Implementations cache
//! through interior mutability.

use std::ops::Range;

use crate::source::columns;
use crate::trivia::{LexKind, lex};

/// Where a fragment came from, which decides the parse-only wrapper an
/// implementation formats it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RustContext {
    /// An expression: `{expr}`, `{?expr}`, and `{...expr}` children after
    /// the marker, `name={expr}` and `name={@expr}` values after the `@`,
    /// a value `name={if c { a }}` with or without `else`, `@when {cond}`,
    /// `key={..}`, `<{expr}>` tags, and bare match-arm bodies. Wrap as a
    /// function body's tail expression.
    Expr,
    /// A `match` scrutinee, where `{` would open the arms. Wrap as
    /// `match <e> {}`.
    Scrutinee,
    /// An `if` or `else if` condition, `if let` and let chains included.
    /// Wrap as `if <c> {}`.
    Condition,
    /// `pat in iter` of a `for` child or an `@for` attribute group. Wrap
    /// as `for <header> {}`.
    ForHeader,
    /// `pat` or `pat if guard` of a match arm, leading `|` allowed. Wrap as
    /// `match x { <head> => {} }`.
    ArmHead,
    /// A whole local statement from `let` through `;`.
    Local,
    /// Constructor or function-tag arguments between the parentheses,
    /// commas exactly as written, a trailing one included or not.
    Args,
    /// The type in a `-> Type,` header.
    Type,
}

/// One embedded Rust fragment, trimmed to its first and last token.
/// Comments before the first or after the last token belong to the
/// template around it, never to the fragment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RustFragment<'a> {
    pub context: RustContext,
    /// The whole original file.
    pub file: &'a str,
    /// The fragment's bytes in `file`.
    pub range: Range<usize>,
    /// Outermost nested selected invocations inside `range`, from the
    /// macro path through the closing delimiter, in order. A nested view's
    /// id for [`NestedViews`] is its index here.
    pub nested: Vec<Range<usize>>,
}

impl RustFragment<'_> {
    /// The exact original bytes.
    pub fn source(&self) -> &str {
        &self.file[self.range.clone()]
    }

    /// Display columns of indentation on the original line where the
    /// fragment starts.
    pub fn original_indent(&self, tab_spaces: usize) -> usize {
        let line_start = self.file[..self.range.start]
            .rfind('\n')
            .map_or(0, |i| i + 1);
        let line = &self.file[line_start..self.range.start];
        let ws = line.len() - line.trim_start_matches([' ', '\t']).len();
        columns(&line[..ws], tab_spaces)
    }
}

/// Where a multi-line fragment goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LayoutRequest {
    /// Display columns of the template indentation that continuation lines
    /// hang from: the indentation of the output line the fragment starts on.
    pub indent: usize,
    /// Columns between `indent` and the fragment's first byte: 6 for
    /// `name={`, 1 for `{`, 0 for constructor arguments, which start on a
    /// line of their own.
    pub prefix: usize,
    /// Columns the printer puts after the fragment on its last line, such
    /// as a closing `}` and `>`.
    pub suffix: usize,
    pub max_width: usize,
    pub tab_spaces: usize,
}

/// A fragment laid out over lines.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub lines: Vec<LayoutLine>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutLine {
    /// Columns relative to [`LayoutRequest::indent`]. Ignored on the first
    /// line, which continues after the prefix, and on verbatim lines.
    pub indent: usize,
    /// The line without its terminator. A `\r` that belongs to a literal
    /// stays at the end of the line before a verbatim line.
    pub text: String,
    /// The line starts inside a multi-line literal or block comment: the
    /// printer emits `text` exactly, without indentation, after a bare
    /// `\n`.
    pub verbatim: bool,
}

impl Layout {
    /// A one-line layout.
    pub fn single(text: impl Into<String>) -> Self {
        Layout {
            lines: vec![LayoutLine {
                indent: 0,
                text: text.into(),
                verbatim: false,
            }],
        }
    }
}

/// The nested views of the fragment being laid out, printed by the
/// template printer.
pub trait NestedViews {
    /// Nested view `id` on one line, path through closing delimiter, or
    /// `None` when it must span lines.
    fn flat(&self, id: usize) -> Option<&str>;
    /// Nested view `id` laid out with its continuation lines hanging from
    /// `indent`, given relative to the enclosing [`LayoutRequest::indent`];
    /// the returned line indents are relative to the same base. The first
    /// line continues wherever the provider put the macro path.
    fn layout(&self, id: usize, indent: usize) -> Layout;
}

/// Why a provider could not lay out a fragment. The printer keeps the
/// fragment as written and reports a warning.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProviderError {
    /// Byte range in the file, when narrower than the fragment.
    pub range: Option<Range<usize>>,
    pub message: String,
}

/// Formats embedded Rust for the template printer.
///
/// Output must keep the fragment's tokens, literal bytes, comments, and
/// punctuation, with each nested range replaced by what [`NestedViews`]
/// returns; the printer re-lexes and rejects anything else.
pub trait RustProvider {
    /// Every fragment of a file, outer and nested, before printing starts.
    fn prepare(&self, fragments: &[RustFragment<'_>]) {
        let _ = fragments;
    }

    /// The fragment on one line, nested views replaced by their one-line
    /// forms, or `None` when it cannot share a line: a block body, a line
    /// comment, a nested view without a one-line form.
    fn flat(
        &self,
        fragment: &RustFragment<'_>,
        nested: &dyn NestedViews,
    ) -> Result<Option<String>, ProviderError>;

    /// The fragment laid out for `request`.
    fn layout(
        &self,
        fragment: &RustFragment<'_>,
        request: &LayoutRequest,
        nested: &dyn NestedViews,
    ) -> Result<Layout, ProviderError>;
}

/// Keeps every fragment as written. Continuation lines keep their
/// indentation relative to the line the fragment starts on, so they move
/// with it; lines inside literals and block comments are copied exactly.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassThrough;

impl RustProvider for PassThrough {
    fn flat(
        &self,
        fragment: &RustFragment<'_>,
        nested: &dyn NestedViews,
    ) -> Result<Option<String>, ProviderError> {
        let src = fragment.source();
        let mut out = String::with_capacity(src.len());
        let mut at = fragment.range.start;
        for (id, range) in fragment.nested.iter().enumerate() {
            let Some(view) = nested.flat(id) else {
                return Ok(None);
            };
            out.push_str(&fragment.file[at..range.start]);
            out.push_str(view);
            at = range.end;
        }
        out.push_str(&fragment.file[at..fragment.range.end]);
        Ok((!out.contains('\n')).then_some(out))
    }

    fn layout(
        &self,
        fragment: &RustFragment<'_>,
        request: &LayoutRequest,
        nested: &dyn NestedViews,
    ) -> Result<Layout, ProviderError> {
        Ok(source_layout(fragment, request.tab_spaces, nested))
    }
}

/// The fragment's own line structure: each line after a newline outside
/// literals and comments indents by its original indentation less the
/// fragment's starting line's, and nested views are laid out in place.
/// Blank lines survive as empty lines.
pub fn source_layout(
    fragment: &RustFragment<'_>,
    tab_spaces: usize,
    nested: &dyn NestedViews,
) -> Layout {
    let base = fragment.original_indent(tab_spaces);
    let mut lines = vec![LayoutLine {
        indent: 0,
        text: String::new(),
        verbatim: false,
    }];
    let mut nested_iter = fragment.nested.iter().enumerate().peekable();
    for tok in lex(fragment.source(), fragment.range.start) {
        if let Some(&(id, range)) = nested_iter.peek() {
            if tok.range.start >= range.end {
                nested_iter.next();
            } else if tok.range.start >= range.start {
                if tok.range.start == range.start {
                    let current = lines.last().expect("at least one line");
                    let indent = if lines.len() == 1 { 0 } else { current.indent };
                    let mut view = nested.layout(id, indent).lines.into_iter();
                    if let Some(first) = view.next() {
                        lines
                            .last_mut()
                            .expect("at least one line")
                            .text
                            .push_str(&first.text);
                    }
                    lines.extend(view);
                }
                continue;
            }
        }
        let text = &fragment.file[tok.range.clone()];
        if tok.kind == LexKind::Whitespace && text.contains('\n') {
            let newlines = text.matches('\n').count();
            let tail = &text[text.rfind('\n').map_or(0, |i| i + 1)..];
            let indent = columns(tail, tab_spaces).saturating_sub(base);
            for _ in 1..newlines {
                lines.push(LayoutLine {
                    indent: 0,
                    text: String::new(),
                    verbatim: false,
                });
            }
            lines.push(LayoutLine {
                indent,
                text: String::new(),
                verbatim: false,
            });
            continue;
        }
        // A literal or block comment spanning lines continues verbatim.
        let mut parts = text.split('\n');
        if let Some(first) = parts.next() {
            lines
                .last_mut()
                .expect("at least one line")
                .text
                .push_str(first);
        }
        for part in parts {
            lines.push(LayoutLine {
                indent: 0,
                text: part.to_owned(),
                verbatim: true,
            });
        }
    }
    Layout { lines }
}
