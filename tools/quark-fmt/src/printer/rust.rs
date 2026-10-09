//! The contract between the template printer and whatever formats the Rust
//! embedded in a template.
//!
//! The printer hands every Rust fragment it may print to
//! [`RustProvider::prepare`] once per file, then asks for a one-line
//! spelling ([`RustProvider::flat`]) while measuring groups and for a laid
//! out spelling ([`RustProvider::format`]) at the exact column where the
//! fragment lands. Nested selected views inside a fragment are printed by
//! the template printer through [`NestedViews`], so the outer invocation
//! owns their bytes. [`PassThrough`] keeps every fragment as written apart
//! from re-indenting its continuation lines; the rustfmt provider replaces
//! it without changing the printer.
//!
//! Every method takes `&self` because a provider calls back into
//! [`NestedViews::render`], which prints a nested template and asks the
//! same provider about that template's fragments. Implementations cache
//! through interior mutability.

use std::ops::Range;

use crate::trivia::{LexKind, lex};

/// Where a fragment came from, which decides the parse-only wrapper an
/// implementation formats it in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum RustContext {
    /// An expression: `{expr}`, `{?expr}`, and `{...expr}` children after
    /// the marker, `name={expr}` and `name={@expr}` values after the `@`,
    /// a value `name={if c { a }}` with or without `else`, `@when {cond}`,
    /// `key={..}`, `<{expr}>` tags, match guards, and bare match-arm
    /// bodies. Format as a function body's tail expression.
    Expr,
    /// An expression where `{` would open a block: `for` and `@for`
    /// iterators and `match` scrutinees. Format inside `match <e> {}`.
    ExprNoBrace,
    /// An `if` condition, `if let` and let chains included. Format inside
    /// `if <c> {}`.
    Condition,
    /// A pattern, leading `|` allowed: `for` and `@for` patterns and
    /// match-arm patterns. Format inside `match x { <p> => {} }`.
    Pattern,
    /// A whole local statement from `let` through `;`.
    Local,
    /// Constructor or function-tag arguments without the parentheses,
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
    /// The exact original bytes.
    pub source: &'a str,
    /// Byte offset of `source` in the file.
    pub offset: usize,
    /// Display columns of indentation on the original line where the
    /// fragment starts; continuation lines in `source` are relative to it.
    pub original_indent: usize,
    /// Outermost nested selected invocations, as ranges of `source` from
    /// the macro path through its closing delimiter, in order.
    pub nested: Vec<Range<usize>>,
}

/// Where the printer will put a fragment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Display columns of indentation on the output line where the fragment
    /// starts; continuation lines indent relative to it.
    pub indent: usize,
    /// Display column where the fragment's first byte lands.
    pub column: usize,
    /// Columns the printer puts right after the fragment on its last line,
    /// such as a closing `}` and `>`.
    pub trailing: usize,
    pub max_width: usize,
    pub tab_spaces: usize,
    pub hard_tabs: bool,
    /// Spelling of newlines the provider introduces. Newlines inside
    /// literals and comments stay byte-identical.
    pub newline: &'static str,
}

impl Placement {
    /// The whitespace for `columns` of indentation in this style.
    pub fn indent_str(&self, columns: usize) -> String {
        crate::source::indent_string(columns, self.tab_spaces, self.hard_tabs)
    }
}

/// The nested views of the fragment being formatted, printed by the
/// template printer.
pub trait NestedViews {
    /// Nested view `index` on one line, path through closing delimiter, or
    /// `None` when it must span lines.
    fn flat(&self, index: usize) -> Option<&str>;
    /// Nested view `index` with its path at `placement.column`; continuation
    /// lines carry their complete indentation.
    fn render(&self, index: usize, placement: &Placement) -> String;
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
    /// Every fragment of a file, outer and nested, before printing starts,
    /// so an implementation can batch its work.
    fn prepare(&self, fragments: &[RustFragment<'_>]) {
        let _ = fragments;
    }

    /// The fragment on one line, nested views replaced by their one-line
    /// forms, or `None` when it cannot share a line (a block body, a line
    /// comment, a nested view without a one-line form).
    fn flat(
        &self,
        fragment: &RustFragment<'_>,
        nested: &dyn NestedViews,
    ) -> Result<Option<String>, ProviderError>;

    /// The fragment laid out at `placement`: the first line starts at
    /// `placement.column` and every later line carries its complete
    /// indentation.
    fn format(
        &self,
        fragment: &RustFragment<'_>,
        placement: &Placement,
        nested: &dyn NestedViews,
    ) -> Result<String, ProviderError>;
}

/// Keeps every fragment as written. Continuation lines move with the
/// fragment: each keeps its indentation relative to the line the fragment
/// starts on. Lines inside literals and block comments are copied exactly.
#[derive(Clone, Copy, Debug, Default)]
pub struct PassThrough;

impl RustProvider for PassThrough {
    fn flat(
        &self,
        fragment: &RustFragment<'_>,
        nested: &dyn NestedViews,
    ) -> Result<Option<String>, ProviderError> {
        let mut out = String::with_capacity(fragment.source.len());
        let mut at = 0;
        for (i, range) in fragment.nested.iter().enumerate() {
            let Some(view) = nested.flat(i) else {
                return Ok(None);
            };
            out.push_str(&fragment.source[at..range.start]);
            out.push_str(view);
            at = range.end;
        }
        out.push_str(&fragment.source[at..]);
        Ok((!out.contains('\n')).then_some(out))
    }

    fn format(
        &self,
        fragment: &RustFragment<'_>,
        placement: &Placement,
        nested: &dyn NestedViews,
    ) -> Result<String, ProviderError> {
        Ok(reindent(fragment, placement, nested))
    }
}

/// Copies `fragment.source`, shifting the indentation after every newline
/// that is not inside a literal or comment by `placement.indent -
/// fragment.original_indent`, and printing nested views in place.
pub fn reindent(
    fragment: &RustFragment<'_>,
    placement: &Placement,
    nested: &dyn NestedViews,
) -> String {
    let src = fragment.source;
    let shift = placement.indent as isize - fragment.original_indent as isize;
    let mut out = String::with_capacity(src.len() + 16);
    let mut nested_iter = fragment.nested.iter().enumerate().peekable();
    // Tracks the output line's indentation and column for nested views.
    let mut line_indent = placement.indent;
    let mut column = placement.column;
    for tok in lex(src, 0) {
        if let Some((i, range)) = nested_iter.peek().cloned() {
            if tok.range.start >= range.end {
                nested_iter.next();
            } else if tok.range.start >= range.start {
                if tok.range.start == range.start {
                    let at = Placement {
                        indent: line_indent,
                        column,
                        ..*placement
                    };
                    let text = nested.render(i, &at);
                    advance(&text, placement.tab_spaces, &mut column, &mut line_indent);
                    out.push_str(&text);
                }
                continue;
            }
        }
        let text = &src[tok.range.clone()];
        if tok.kind == LexKind::Whitespace && text.contains('\n') {
            // Keep the blank lines, re-indent the last one.
            let newlines = text.matches('\n').count();
            let tail = &text[text.rfind('\n').map_or(0, |i| i + 1)..];
            let old = crate::source::columns(tail, placement.tab_spaces);
            let new = (old as isize + shift).max(0) as usize;
            for _ in 0..newlines {
                out.push_str(placement.newline);
            }
            out.push_str(&placement.indent_str(new));
            line_indent = new;
            column = new;
            continue;
        }
        advance(text, placement.tab_spaces, &mut column, &mut line_indent);
        out.push_str(text);
    }
    out
}

/// Moves `column` past `text`; a newline inside it (a multiline literal)
/// starts a line whose indentation is its leading whitespace.
fn advance(text: &str, tab_spaces: usize, column: &mut usize, line_indent: &mut usize) {
    match text.rfind('\n') {
        Some(i) => {
            let last = &text[i + 1..];
            let ws = last.len() - last.trim_start_matches([' ', '\t']).len();
            *line_indent = crate::source::columns(&last[..ws], tab_spaces);
            *column = crate::source::columns(last, tab_spaces);
        }
        None => *column += crate::source::columns(text, tab_spaces),
    }
}
