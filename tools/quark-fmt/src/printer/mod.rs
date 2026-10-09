//! Template layout: parses each selected view with the shared parser,
//! builds its document ([`build`]), resolves embedded Rust through the
//! provider ([`rust`]), and prints it.
//!
//! Nested views inside embedded Rust are parsed and built with their
//! enclosing view, and printed through [`NestedViews`] when the provider
//! lays out the fragment holding them, so the outermost invocation owns
//! every byte of its body.

pub mod build;
pub mod rust;

use std::cell::RefCell;
use std::ops::Range;

use proc_macro2::TokenStream;
use quark_view_syntax::parse_with_syntax;
use syn::parse::Parser;

use self::build::Builder;
use self::rust::{
    Layout, LayoutLine, LayoutRequest, NestedViews, PassThrough, RustFragment, RustProvider,
    source_layout,
};
use crate::discover::{Delimiter, NestedCall, scan, span_range};
use crate::doc::{self, Doc, Embeds, PrintConfig};
use crate::source::{Diagnostic, DiagnosticKind};

/// One parsed view, ready to print.
pub(crate) struct View<'s> {
    /// For a nested view, the source from its path through the opening
    /// delimiter, and its closing delimiter.
    head: String,
    close: String,
    /// The body between the delimiters; a group whose `broken` flag says
    /// whether it fits on one line.
    doc: Doc,
    embeds: Vec<Embed<'s>>,
    /// A nested view on one line, path through closing delimiter.
    flat: Option<String>,
    /// Selected invocations in this view, nested ones included.
    pub count: usize,
}

struct Embed<'s> {
    fragment: RustFragment<'s>,
    nested: Vec<View<'s>>,
    flat: Option<String>,
}

/// Parses and builds the view whose body is `body` of `src`. `suffix` is
/// the width after the body on its closing line, delimiter included.
pub(crate) fn build_view<'s>(
    src: &'s str,
    tokens: TokenStream,
    body: Range<usize>,
    delimiter: Delimiter,
    suffix: usize,
    names: &[&str],
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<View<'s>, Diagnostic> {
    let (_, tree) = parse_with_syntax.parse2(tokens).map_err(|e| {
        Diagnostic::error(
            DiagnosticKind::ViewParse,
            span_range(e.span()).or(Some(body.clone())),
            format!("not valid view syntax: {e}"),
        )
    })?;
    let mut builder = Builder::new(src, &tree, body.clone()).map_err(|e| {
        Diagnostic::error(DiagnosticKind::Trivia, Some(e.offset..e.offset), e.message)
    })?;
    let doc = builder.view(delimiter == Delimiter::Brace, suffix);
    let mut count = 1;
    let mut embeds = Vec::new();
    for pending in builder.embeds {
        let mut calls: Vec<NestedCall> = Vec::new();
        for tokens in &pending.tokens {
            scan(tokens, names, false, &mut calls, diagnostics);
        }
        let mut nested = Vec::new();
        for call in &calls {
            let mut view = build_view(
                src,
                call.tokens.clone(),
                call.body.clone(),
                call.delimiter,
                0,
                names,
                diagnostics,
            )?;
            view.head = src[call.range.start..call.body.start].to_owned();
            view.close = src[call.body.end..call.range.end].to_owned();
            count += view.count;
            nested.push(view);
        }
        embeds.push(Embed {
            fragment: RustFragment {
                context: pending.context,
                file: src,
                range: pending.range,
                nested: calls.iter().map(|c| c.range.clone()).collect(),
            },
            nested,
            flat: None,
        });
    }
    Ok(View {
        head: String::new(),
        close: String::new(),
        doc,
        embeds,
        flat: None,
        count,
    })
}

/// Every fragment of `view` and its nested views, for
/// [`RustProvider::prepare`].
pub(crate) fn fragments<'s>(view: &View<'s>, out: &mut Vec<RustFragment<'s>>) {
    for e in &view.embeds {
        out.push(e.fragment.clone());
        for n in &e.nested {
            fragments(n, out);
        }
    }
}

/// Shared state while resolving and printing.
pub(crate) struct Ctx<'p> {
    pub provider: &'p dyn RustProvider,
    pub cfg: PrintConfig,
    pub diagnostics: RefCell<Vec<Diagnostic>>,
}

impl Ctx<'_> {
    fn provider_failed(&self, fragment: &RustFragment<'_>, err: rust::ProviderError) {
        self.diagnostics.borrow_mut().push(Diagnostic::warning(
            DiagnosticKind::Provider,
            err.range.or(Some(fragment.range.clone())),
            format!("embedded Rust kept as written: {}", err.message),
        ));
    }
}

/// Asks the provider for every fragment's one-line form, innermost views
/// first, then marks the groups that must break.
pub(crate) fn resolve(view: &mut View<'_>, ctx: &Ctx<'_>) {
    for e in &mut view.embeds {
        for n in &mut e.nested {
            resolve(n, ctx);
        }
        let nested = Nested {
            views: &e.nested,
            ctx,
            base: 0,
        };
        e.flat = match ctx.provider.flat(&e.fragment, &nested) {
            Ok(flat) => flat,
            Err(err) => {
                ctx.provider_failed(&e.fragment, err);
                PassThrough.flat(&e.fragment, &nested).unwrap_or(None)
            }
        };
    }
    let mut body = std::mem::replace(&mut view.doc, Doc::nil());
    let printing = Printing { view: &*view, ctx };
    doc::propagate(&mut body, &printing);
    let whole = Doc::concat([
        Doc::text(view.head.clone()),
        body.clone(),
        Doc::text(view.close.clone()),
    ]);
    let flat = doc::print_flat(&whole, &printing);
    view.flat = flat;
    view.doc = body;
}

/// Prints a resolved top-level body starting at `column` on a line
/// indented `indent` columns.
pub(crate) fn print(
    view: &View<'_>,
    ctx: &Ctx<'_>,
    indent: usize,
    column: usize,
) -> Vec<LayoutLine> {
    doc::print(&view.doc, indent, column, ctx.cfg, &Printing { view, ctx })
}

struct Printing<'a, 's> {
    view: &'a View<'s>,
    ctx: &'a Ctx<'a>,
}

impl Embeds for Printing<'_, '_> {
    fn flat(&self, id: usize) -> Option<&str> {
        self.view.embeds[id].flat.as_deref()
    }

    fn layout(&self, id: usize, request: &LayoutRequest) -> Layout {
        let e = &self.view.embeds[id];
        let nested = Nested {
            views: &e.nested,
            ctx: self.ctx,
            base: request.indent,
        };
        match self.ctx.provider.layout(&e.fragment, request, &nested) {
            Ok(layout) => layout,
            Err(err) => {
                self.ctx.provider_failed(&e.fragment, err);
                source_layout(&e.fragment, request.tab_spaces, &nested)
            }
        }
    }
}

struct Nested<'a, 's> {
    views: &'a [View<'s>],
    ctx: &'a Ctx<'a>,
    /// The enclosing request's indentation, which relative indents use.
    base: usize,
}

impl NestedViews for Nested<'_, '_> {
    fn flat(&self, id: usize) -> Option<&str> {
        self.views[id].flat.as_deref()
    }

    fn layout(&self, id: usize, indent: usize) -> Layout {
        let view = &self.views[id];
        let mut body = view.doc.clone();
        if let Doc::Group { broken, .. } = &mut body {
            *broken = true;
        }
        let whole = Doc::concat([
            Doc::text(view.head.clone()),
            body,
            Doc::text(view.close.clone()),
        ]);
        let at = self.base + indent;
        let lines = doc::print(
            &whole,
            at,
            at,
            self.ctx.cfg,
            &Printing {
                view,
                ctx: self.ctx,
            },
        );
        Layout {
            lines: lines
                .into_iter()
                .enumerate()
                .map(|(i, l)| LayoutLine {
                    indent: if i == 0 || l.verbatim {
                        0
                    } else {
                        l.indent.saturating_sub(self.base)
                    },
                    ..l
                })
                .collect(),
        }
    }
}
