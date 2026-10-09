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
    // syn strips a BOM and shebang itself but then reports offsets into
    // the remainder; strip here so offsets can be shifted back.
    let base = rust_start(source);
    let file: syn::File = syn::parse_str(&source[base..]).map_err(|e| {
        let range = span_range(e.span(), base);
        Diagnostic::error(
            DiagnosticKind::RustParse,
            range,
            format!("not valid Rust: {e}"),
        )
    })?;
    let names: Vec<&str> = macro_names
        .iter()
        .map(|n| n.trim().trim_start_matches("::"))
        .collect();
    let mut visitor = Visitor {
        source,
        base,
        names: &names,
        skip_depth: 0,
        found: Discovery::default(),
    };
    visitor.visit_file(&file);
    Ok(visitor.found)
}

/// Whether `path` (as written, spaces allowed) names a selected macro.
pub fn is_selected(path: &syn::Path, names: &[&str]) -> bool {
    let written = path_string(path);
    names.contains(&written.as_str())
}

/// Byte offset where Rust tokens may begin, after a BOM and a shebang line.
fn rust_start(source: &str) -> usize {
    let mut at = 0;
    if source.starts_with('\u{feff}') {
        at = '\u{feff}'.len_utf8();
    }
    let rest = &source[at..];
    if let Some(after) = rest.strip_prefix("#!") {
        // `#![attr]` is an inner attribute, not a shebang.
        if !after.trim_start().starts_with('[') {
            at += rest.find('\n').unwrap_or(rest.len());
        }
    }
    at
}

fn span_range(span: proc_macro2::Span, base: usize) -> Option<Range<usize>> {
    let r = span.byte_range();
    (r != (0..0)).then(|| r.start + base..r.end + base)
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
    base: usize,
    names: &'a [&'a str],
    /// Nonzero inside an item or statement under a skip.
    skip_depth: usize,
    found: Discovery,
}

impl Visitor<'_> {
    fn start_of(&self, node: &impl Spanned) -> usize {
        node.span().byte_range().start + self.base
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

    fn hidden_scan(&mut self, tokens: &TokenStream) {
        let tokens: Vec<TokenTree> = tokens.clone().into_iter().collect();
        for (i, tt) in tokens.iter().enumerate() {
            match tt {
                TokenTree::Group(g) => self.hidden_scan(&g.stream()),
                TokenTree::Punct(p) if p.as_char() == '!' && i > 0 => {
                    let Some(path) = trailing_path(&tokens[..i]) else {
                        continue;
                    };
                    if self.names.contains(&path.as_str()) {
                        self.found.diagnostics.push(Diagnostic::warning(
                            DiagnosticKind::HiddenMacro,
                            span_range(p.span(), self.base),
                            format!("`{path}!` inside another macro's body is left as written"),
                        ));
                    }
                }
                _ => {}
            }
        }
    }
}

/// The `a::b` path whose last segment ends `tokens`, as written.
fn trailing_path(tokens: &[TokenTree]) -> Option<String> {
    let mut segments = Vec::new();
    let mut i = tokens.len();
    while let Some(TokenTree::Ident(ident)) = i.checked_sub(1).and_then(|j| tokens.get(j)) {
        segments.push(ident.to_string());
        i -= 1;
        let colons = i >= 2
            && matches!(&tokens[i - 1], TokenTree::Punct(p) if p.as_char() == ':')
            && matches!(&tokens[i - 2], TokenTree::Punct(p) if p.as_char() == ':');
        if !colons {
            break;
        }
        i -= 2;
    }
    if segments.is_empty() {
        return None;
    }
    segments.reverse();
    Some(segments.join("::"))
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
            self.hidden_scan(&mac.tokens);
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
            range: start..close.end + self.base,
            body: open.end + self.base..close.start + self.base,
            skipped,
        });
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
