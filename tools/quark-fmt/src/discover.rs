//! Finds selected `view!` invocations in a Rust file.
//!
//! syn parses the whole file and a visitor checks every macro position
//! against the configured paths. syn cannot see into an opaque macro body,
//! so a view inside `vec![..]` stays as written with a coverage warning;
//! views nested inside a selected view are found later by walking its
//! template, and the outermost invocation owns their edits.

use std::ops::Range;

use proc_macro2::{TokenStream, TokenTree};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::source::{Diagnostic, DiagnosticKind};

/// The comment that keeps the next invocation, statement, or item as written.
pub const SKIP_DIRECTIVE: &str = "// quark-fmt: skip";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Delimiter {
    Brace,
    Paren,
    Bracket,
}

/// One outermost selected invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// The macro path as written, without spaces or a leading `::`.
    pub path: String,
    pub delimiter: Delimiter,
    /// From the first byte of the path through the closing delimiter.
    pub range: Range<usize>,
    /// Between the delimiters.
    pub body: Range<usize>,
    /// A skip directive or `#[rustfmt::skip]` covers it.
    pub skipped: bool,
}

/// The result of discovery: outermost invocations in source order, and
/// warnings about selected macros hidden inside other macros' bodies.
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    pub invocations: Vec<Invocation>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses `source` and returns its selected invocations. A file that is not
/// valid Rust yields a single error and must stay unchanged.
pub fn discover(source: &str, macro_names: &[String]) -> Result<Discovery, Diagnostic> {
    discover_with_tokens(source, macro_names).map(|(found, _)| found)
}

/// [`discover`], plus each invocation's body tokens, whose spans resolve
/// to byte offsets in `source`.
pub(crate) fn discover_with_tokens(
    source: &str,
    macro_names: &[String],
) -> Result<(Discovery, Vec<TokenStream>), Diagnostic> {
    let file: syn::File = syn::parse_str(&blank_preamble(source)).map_err(|e| {
        Diagnostic::error(
            DiagnosticKind::RustParse,
            span_range(e.span()),
            format!("not valid Rust: {e}"),
        )
    })?;
    let names = normalize(macro_names);
    let mut visitor = Visitor {
        source,
        names: &names,
        skip_depth: 0,
        found: Discovery::default(),
        tokens: Vec::new(),
    };
    visitor.visit_file(&file);
    Ok((visitor.found, visitor.tokens))
}

pub(crate) fn normalize(macro_names: &[String]) -> Vec<&str> {
    macro_names
        .iter()
        .map(|n| n.trim().trim_start_matches("::"))
        .collect()
}

/// Whether `path` (as written, spaces allowed) names a selected macro.
pub fn is_selected(path: &syn::Path, names: &[&str]) -> bool {
    let written = path_string(path);
    names.contains(&written.as_str())
}

/// `source` with a BOM and a shebang line turned into spaces of the same
/// byte length. syn would strip them and report offsets into the rest;
/// blanking keeps every span's byte range an offset into `source`.
fn blank_preamble(source: &str) -> std::borrow::Cow<'_, str> {
    let mut end = 0;
    if source.starts_with('\u{feff}') {
        end = '\u{feff}'.len_utf8();
    }
    let rest = &source[end..];
    if let Some(after) = rest.strip_prefix("#!") {
        // `#![attr]` is an inner attribute, not a shebang.
        if !after.trim_start().starts_with('[') {
            end += rest.find('\n').unwrap_or(rest.len());
        }
    }
    if end == 0 {
        return source.into();
    }
    let mut out = " ".repeat(end);
    out.push_str(&source[end..]);
    out.into()
}

pub(crate) fn span_range(span: proc_macro2::Span) -> Option<Range<usize>> {
    let r = span.byte_range();
    (r != (0..0)).then_some(r)
}

fn path_string(path: &syn::Path) -> String {
    path.segments
        .iter()
        .map(|s| s.ident.to_string())
        .collect::<Vec<_>>()
        .join("::")
}

/// True when the source just before `offset`, skipping whitespace, is a
/// line holding only the skip directive.
pub fn preceded_by_skip(source: &str, offset: usize) -> bool {
    let before = source[..offset].trim_end();
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    before[line_start..].trim() == SKIP_DIRECTIVE
}

fn has_rustfmt_skip(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        let p = &a.path().segments;
        p.len() == 2 && p[0].ident == "rustfmt" && p[1].ident == "skip"
    })
}

struct Visitor<'a> {
    source: &'a str,
    names: &'a [&'a str],
    /// Nonzero inside an item or statement under a skip.
    skip_depth: usize,
    found: Discovery,
    tokens: Vec<TokenStream>,
}

impl Visitor<'_> {
    fn start_of(&self, node: &impl Spanned) -> usize {
        node.span().byte_range().start
    }

    /// Visits a node that may carry a skip, counting it while inside.
    fn scoped(
        &mut self,
        attrs: &[syn::Attribute],
        node: &impl Spanned,
        visit: impl FnOnce(&mut Self),
    ) {
        let skip = has_rustfmt_skip(attrs) || preceded_by_skip(self.source, self.start_of(node));
        self.skip_depth += usize::from(skip);
        visit(self);
        self.skip_depth -= usize::from(skip);
    }
}

/// A selected invocation inside an embedded Rust fragment.
#[derive(Clone, Debug)]
pub(crate) struct NestedCall {
    /// From the macro path through the closing delimiter.
    pub range: Range<usize>,
    /// Between the delimiters.
    pub body: Range<usize>,
    pub delimiter: Delimiter,
    pub tokens: TokenStream,
}

/// Rust keywords that can precede `!` without naming a macro: `if !x`.
const KEYWORDS: &[&str] = &[
    "as", "break", "else", "for", "if", "in", "let", "loop", "match", "move", "mut", "ref",
    "return", "while", "yield", "await", "dyn", "impl", "where", "unsafe", "async", "const",
];

/// Scans tokens for selected invocations. Outside any other macro they
/// are collected in `nested`; inside one they are opaque and only produce
/// a coverage warning.
pub(crate) fn scan(
    tokens: &TokenStream,
    names: &[&str],
    inside_macro: bool,
    nested: &mut Vec<NestedCall>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let tokens: Vec<TokenTree> = tokens.clone().into_iter().collect();
    let mut i = 0;
    while i < tokens.len() {
        let call = match (&tokens[i], tokens.get(i + 1)) {
            (TokenTree::Punct(p), Some(TokenTree::Group(g))) if p.as_char() == '!' && i > 0 => {
                trailing_path(&tokens[..i]).map(|(path, start)| (path, start, p, g))
            }
            _ => None,
        };
        match call {
            Some((path, start, bang, group)) if !KEYWORDS.contains(&path.as_str()) => {
                if !names.contains(&path.as_str()) {
                    scan(&group.stream(), names, true, nested, diagnostics);
                } else if inside_macro {
                    diagnostics.push(Diagnostic::warning(
                        DiagnosticKind::HiddenMacro,
                        span_range(bang.span()),
                        format!("`{path}!` inside another macro's body is left as written"),
                    ));
                } else {
                    let delimiter = match group.delimiter() {
                        proc_macro2::Delimiter::Parenthesis => Delimiter::Paren,
                        proc_macro2::Delimiter::Bracket => Delimiter::Bracket,
                        _ => Delimiter::Brace,
                    };
                    let open = group.span_open().byte_range();
                    let close = group.span_close().byte_range();
                    nested.push(NestedCall {
                        range: tokens[start].span().byte_range().start..close.end,
                        body: open.end..close.start,
                        delimiter,
                        tokens: group.stream(),
                    });
                }
                i += 2;
            }
            _ => {
                if let TokenTree::Group(g) = &tokens[i] {
                    scan(&g.stream(), names, inside_macro, nested, diagnostics);
                }
                i += 1;
            }
        }
    }
}

/// The `a::b` path whose last segment ends `tokens`, as written, and the
/// index of its first token (a leading `::` included).
fn trailing_path(tokens: &[TokenTree]) -> Option<(String, usize)> {
    let mut segments = Vec::new();
    let mut i = tokens.len();
    let mut start = i;
    while let Some(TokenTree::Ident(ident)) = i.checked_sub(1).and_then(|j| tokens.get(j)) {
        segments.push(ident.to_string());
        i -= 1;
        start = i;
        let colons = i >= 2
            && matches!(&tokens[i - 1], TokenTree::Punct(p) if p.as_char() == ':')
            && matches!(&tokens[i - 2], TokenTree::Punct(p) if p.as_char() == ':');
        if !colons {
            break;
        }
        i -= 2;
        start = i;
    }
    if segments.is_empty() {
        return None;
    }
    segments.reverse();
    Some((segments.join("::"), start))
}

impl<'ast> Visit<'ast> for Visitor<'_> {
    fn visit_item(&mut self, item: &'ast syn::Item) {
        let attrs = item_attrs(item);
        self.scoped(attrs, item, |v| syn::visit::visit_item(v, item));
    }

    fn visit_impl_item(&mut self, item: &'ast syn::ImplItem) {
        let attrs: &[syn::Attribute] = match item {
            syn::ImplItem::Const(i) => &i.attrs,
            syn::ImplItem::Fn(i) => &i.attrs,
            syn::ImplItem::Type(i) => &i.attrs,
            syn::ImplItem::Macro(i) => &i.attrs,
            _ => &[],
        };
        self.scoped(attrs, item, |v| syn::visit::visit_impl_item(v, item));
    }

    fn visit_trait_item(&mut self, item: &'ast syn::TraitItem) {
        let attrs: &[syn::Attribute] = match item {
            syn::TraitItem::Const(i) => &i.attrs,
            syn::TraitItem::Fn(i) => &i.attrs,
            syn::TraitItem::Type(i) => &i.attrs,
            syn::TraitItem::Macro(i) => &i.attrs,
            _ => &[],
        };
        self.scoped(attrs, item, |v| syn::visit::visit_trait_item(v, item));
    }

    fn visit_stmt(&mut self, stmt: &'ast syn::Stmt) {
        let attrs: &[syn::Attribute] = match stmt {
            syn::Stmt::Local(l) => &l.attrs,
            syn::Stmt::Macro(m) => &m.attrs,
            syn::Stmt::Expr(e, _) => expr_attrs(e),
            syn::Stmt::Item(_) => &[],
        };
        self.scoped(attrs, stmt, |v| syn::visit::visit_stmt(v, stmt));
    }

    fn visit_macro(&mut self, mac: &'ast syn::Macro) {
        if !is_selected(&mac.path, self.names) {
            let mut ignored = Vec::new();
            scan(
                &mac.tokens,
                self.names,
                true,
                &mut ignored,
                &mut self.found.diagnostics,
            );
            return;
        }
        let (delimiter, span) = match &mac.delimiter {
            syn::MacroDelimiter::Brace(b) => (Delimiter::Brace, b.span),
            syn::MacroDelimiter::Paren(p) => (Delimiter::Paren, p.span),
            syn::MacroDelimiter::Bracket(b) => (Delimiter::Bracket, b.span),
        };
        let start = self.start_of(&mac.path);
        let open = span.open().byte_range();
        let close = span.close().byte_range();
        let skipped = self.skip_depth > 0 || preceded_by_skip(self.source, start);
        self.found.invocations.push(Invocation {
            path: path_string(&mac.path),
            delimiter,
            range: start..close.end,
            body: open.end..close.start,
            skipped,
        });
        self.tokens.push(mac.tokens.clone());
    }
}

fn item_attrs(item: &syn::Item) -> &[syn::Attribute] {
    use syn::Item;
    match item {
        Item::Const(i) => &i.attrs,
        Item::Enum(i) => &i.attrs,
        Item::ExternCrate(i) => &i.attrs,
        Item::Fn(i) => &i.attrs,
        Item::ForeignMod(i) => &i.attrs,
        Item::Impl(i) => &i.attrs,
        Item::Macro(i) => &i.attrs,
        Item::Mod(i) => &i.attrs,
        Item::Static(i) => &i.attrs,
        Item::Struct(i) => &i.attrs,
        Item::Trait(i) => &i.attrs,
        Item::TraitAlias(i) => &i.attrs,
        Item::Type(i) => &i.attrs,
        Item::Union(i) => &i.attrs,
        Item::Use(i) => &i.attrs,
        _ => &[],
    }
}

/// Outer attributes of the expression kinds that commonly hold a view.
fn expr_attrs(expr: &syn::Expr) -> &[syn::Attribute] {
    use syn::Expr;
    match expr {
        Expr::Macro(e) => &e.attrs,
        Expr::Call(e) => &e.attrs,
        Expr::MethodCall(e) => &e.attrs,
        Expr::Block(e) => &e.attrs,
        Expr::If(e) => &e.attrs,
        Expr::Match(e) => &e.attrs,
        Expr::Closure(e) => &e.attrs,
        Expr::Return(e) => &e.attrs,
        _ => &[],
    }
}
