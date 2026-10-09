//! The parsed form of `view!` input. Parsing (`parse.rs`) only checks
//! syntax; what each node means is decided when it is lowered (`emit.rs`).

use proc_macro2::Span;
use syn::punctuated::Punctuated;
use syn::{Expr, ExprIf, Ident, LitStr, Pat, Token};

pub(crate) struct ViewInput {
    pub scale: Option<Ident>,
    /// `view! { -> Type, <root/> }`: the root builder itself, ascribed to
    /// `Type`, instead of an `AnyElement`.
    pub typed: Option<syn::Type>,
    pub root: Node,
}

pub(crate) enum Node {
    Element(Element),
    /// `<>...</>` or `<fragment>...</fragment>`: children flow into the parent.
    Fragment(Vec<Node>),
    /// `{expr}`
    Expr(Expr),
    /// `{?expr}`: an `Option` child.
    OptionalExpr(Expr),
    /// `{...expr}`: every item of an iterator.
    SpreadExpr(Expr),
    /// `"text"`, with `{expr}` interpolation.
    Text(LitStr),
    If(IfNode),
    For(Box<ForNode>),
    Match(MatchNode),
}

pub(crate) struct IfNode {
    pub cond: Expr,
    pub then_children: Vec<Node>,
    pub else_if: Option<Box<IfNode>>,
    pub else_children: Option<Vec<Node>>,
}

pub(crate) struct ForNode {
    pub pat: Pat,
    pub iter: Expr,
    /// `for x in xs key={x.id} { ... }`: `.key(..)` on the body's root.
    pub key: Option<(Ident, Expr)>,
    pub body: Vec<Node>,
}

pub(crate) struct MatchNode {
    pub match_token: Token![match],
    pub scrutinee: Expr,
    pub arms: Vec<MatchArm>,
}

pub(crate) struct MatchArm {
    pub pat: Pat,
    pub guard: Option<Expr>,
    pub body: Vec<Node>,
}

pub(crate) struct Element {
    pub tag: Tag,
    /// Span of the tag name, where errors about the element point.
    pub span: Span,
    /// `<Component(a, b)>`: arguments for `Component::new`; `<name(a, b)>`:
    /// arguments for the function `name`.
    pub ctor_args: Option<Punctuated<Expr, Token![,]>>,
    pub attrs: Vec<Attr>,
    pub children: Vec<Node>,
}

#[derive(Clone)]
pub(crate) enum Tag {
    /// A lowercase name: `div`, `text`, `p`, `b`, ... Resolved at emit time
    /// so unknown names get a suggestion.
    Builtin(Ident),
    Component(syn::Path),
    /// `<name(args)>`: a lowercase function that returns a builder, such as
    /// `<canvas(paint)>` or `<popover_panel(theme)>`.
    Function(syn::Path),
    /// `<.method>` inside a component: each child becomes `.method(child)`.
    Slot(Ident),
    /// `<{expr} attrs/>`: attributes applied to a builder value, as in
    /// `<{self.input} focused={f} />`.
    Value(Box<Expr>),
}

pub(crate) enum Attr {
    /// Any builder method: `name`, `name={expr}`, `name="lit"`,
    /// `kebab-name=..`, and the `on:`, `aria-`, and `role` forms, which
    /// emit picks apart by `name.written`.
    Method {
        name: AttrName,
        value: AttrValue,
    },
    Class(LitStr),
    /// `@when {cond} { attrs }`
    When(Expr, Vec<Attr>),
    /// `@for pat in iter { attrs }`: the attributes once per item.
    For(Box<Pat>, Expr, Vec<Attr>),
}

pub(crate) struct AttrName {
    /// The name as written, segments joined: `aria-label`, `on:key:mod+s`.
    pub written: String,
    /// The first segment, so emitted method names map back to the source.
    pub first: Ident,
    /// `-`/`_` separated segments after any `on:` prefix.
    pub segments: Vec<Ident>,
    /// `on:` events: `Some(rest)` where rest is `click`, `key:mod+s`, ...
    pub event: Option<EventName>,
}

pub(crate) struct EventName {
    pub name: Ident,
    /// `on:key:<binding>`: the binding text.
    pub binding: Option<LitStr>,
}

pub(crate) enum AttrValue {
    /// Bare `name`.
    Flag,
    Expr(Expr),
    /// `name={@sig}`: lowers to `cx.read(sig)`.
    Reactive(Expr),
    /// `name={if c { a } else { b }}`; with no `else` the call is skipped.
    If(ExprIf),
}

impl Attr {
    pub fn span(&self) -> Span {
        match self {
            Attr::Method { name, .. } => name.first.span(),
            Attr::Class(lit) => lit.span(),
            Attr::When(cond, _) => syn::spanned::Spanned::span(cond),
            Attr::For(pat, _, _) => syn::spanned::Spanned::span(pat),
        }
    }
}
