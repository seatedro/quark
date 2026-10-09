//! Lowering `view!` nodes to builder calls.
//!
//! Every attribute becomes a method call whose name carries the
//! attribute's span, so the builder API is the single list of valid
//! attributes: an unknown one is rustc's "no method named `x`" error at
//! the attribute, and rust-analyzer resolves hover, go to definition, and
//! completion through the emitted call.

use std::cell::RefCell;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote, quote_spanned};
use syn::spanned::Spanned;
use syn::{Expr, ExprIf, Ident, LitStr};

use super::ast::*;
use super::text::{Segment, join, segments};
use crate::classes::{self, On};
use crate::suggest::closest;

pub(crate) struct Emit {
    pub scale: Option<Ident>,
    /// Errors found while lowering; any error replaces the expansion.
    errors: RefCell<Vec<syn::Error>>,
}

/// How a node joins its parent's children.
pub(crate) enum Mode {
    /// One child.
    Child(TokenStream2),
    /// An `Option` of one child.
    Optional(TokenStream2),
    /// An iterator of `AnyElement`s.
    Spread(TokenStream2),
}

/// Where `child_stmts` adds children.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Sink {
    /// A builder in `__quark_parent`, reassigned by each call.
    Chain,
    /// `__quark_children: Vec<AnyElement>`.
    Vec,
    /// `__quark_children` of a props component, filled through `Into`.
    Props,
}

impl Sink {
    /// The statement adding one lowered node.
    fn add(self, mode: Mode) -> TokenStream2 {
        let into = quote!(::core::convert::Into::into);
        match (self, mode) {
            (Sink::Chain, Mode::Child(t)) => {
                quote!(__quark_parent = __quark_parent.child(#t);)
            }
            (Sink::Chain, Mode::Optional(t)) => {
                quote!(__quark_parent = __quark_parent.optional_child(#t);)
            }
            (Sink::Chain, Mode::Spread(t)) => {
                quote!(__quark_parent = __quark_parent.children(#t);)
            }
            (Sink::Vec, Mode::Child(t)) => quote!(__quark_children.push((#t).into_any());),
            (Sink::Vec, Mode::Optional(t)) => quote! {
                if let ::core::option::Option::Some(__quark_child) = (#t) {
                    __quark_children.push(__quark_child.into_any());
                }
            },
            (Sink::Vec, Mode::Spread(t)) => quote!(__quark_children.extend(#t);),
            (Sink::Props, Mode::Child(t)) => {
                quote!(__quark_children.push(#into((#t).into_any()));)
            }
            (Sink::Props, Mode::Optional(t)) => quote! {
                if let ::core::option::Option::Some(__quark_child) = (#t) {
                    __quark_children.push(#into(__quark_child.into_any()));
                }
            },
            (Sink::Props, Mode::Spread(t)) => quote! {
                __quark_children.extend((#t).into_iter().map(|__quark_child| #into(__quark_child)));
            },
        }
    }
}

/// What attributes are lowered against. Builders take flags as no-argument
/// calls; `#[derive(Props)]` builders take every prop as one value, so a
/// flag is `true` there.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Target {
    Div,
    Text,
    Rich,
    Icon,
    /// `<Component(args)>`: a builder with `::new(args)`.
    Builder,
    /// `<Component>`: a `#[derive(Props)]` builder.
    Props,
    /// An inline rich text tag: calls on `StyledSpan`.
    Span,
}

const BUILTIN_TAGS: &[&str] = &["div", "text", "p", "icon", "spacer", "fragment"];
const INLINE_TAGS: &[&str] = &[
    "b", "strong", "i", "em", "code", "u", "s", "del", "a", "span", "br",
];

/// Spatial builder methods that `view! { scale, .. }` multiplies by scale.
const SPATIAL_ATTRS: &[&str] = &[
    "gap", "gap_x", "gap_y", "p", "px", "py", "pt", "pb", "pl", "pr", "rounded",
];

/// `role="..."` values: ARIA role names and the `SemanticRole` they set.
const ROLES: &[(&str, &str)] = &[
    ("alert", "Alert"),
    ("button", "Button"),
    ("cell", "Cell"),
    ("checkbox", "CheckBox"),
    ("combobox", "ComboBox"),
    ("dialog", "Dialog"),
    ("document", "Document"),
    ("grid", "Grid"),
    ("gridcell", "Cell"),
    ("group", "Group"),
    ("heading", "Heading"),
    ("img", "Image"),
    ("label", "Label"),
    ("link", "Link"),
    ("list", "List"),
    ("listbox", "ListBox"),
    ("listitem", "ListItem"),
    ("menu", "Menu"),
    ("menubar", "MenuBar"),
    ("menuitem", "MenuItem"),
    ("option", "ListBoxOption"),
    ("progressbar", "ProgressIndicator"),
    ("radio", "RadioButton"),
    ("radiogroup", "RadioGroup"),
    ("row", "Row"),
    ("scrollarea", "ScrollArea"),
    ("separator", "Separator"),
    ("slider", "Slider"),
    ("spinbutton", "SpinButton"),
    ("status", "Status"),
    ("switch", "Switch"),
    ("tab", "Tab"),
    ("table", "Table"),
    ("tablist", "TabList"),
    ("tabpanel", "TabPanel"),
    ("textbox", "TextInput"),
    ("toolbar", "Toolbar"),
    ("tooltip", "Tooltip"),
    ("tree", "Tree"),
    ("treeitem", "TreeItem"),
];

/// `aria-*` attributes and the accessibility builder each calls.
const ARIA: &[(&str, &str)] = &[
    ("aria-label", "accessibility_label"),
    ("aria-description", "accessibility_description"),
    ("aria-valuetext", "accessibility_value"),
    ("aria-selected", "accessibility_selected"),
    ("aria-checked", "accessibility_toggled"),
    ("aria-pressed", "accessibility_toggled"),
    ("aria-expanded", "accessibility_expanded"),
    ("aria-disabled", "accessibility_disabled"),
    ("aria-invalid", "accessibility_invalid"),
    ("aria-required", "accessibility_required"),
    ("aria-readonly", "accessibility_read_only"),
    ("aria-multiselectable", "accessibility_multiselectable"),
    ("aria-level", "accessibility_level"),
    ("aria-rowindex", "accessibility_row_index"),
    ("aria-colindex", "accessibility_column_index"),
    ("aria-sort", "accessibility_sort"),
    ("aria-live", "live"),
];

const KEY_MODIFIERS: &[&str] = &[
    "mod", "primary", "cmd", "command", "super", "meta", "ctrl", "control", "alt", "option", "opt",
    "shift",
];

impl Emit {
    pub fn new(scale: Option<Ident>) -> Self {
        Emit {
            scale,
            errors: RefCell::new(Vec::new()),
        }
    }

    pub fn into_errors(self) -> Option<syn::Error> {
        let mut errors = self.errors.into_inner().into_iter();
        let mut first = errors.next()?;
        for e in errors {
            first.combine(e);
        }
        Some(first)
    }

    fn error(&self, span: Span, message: impl std::fmt::Display) {
        self.errors
            .borrow_mut()
            .push(syn::Error::new(span, message));
    }

    /// The root of a `view!`.
    pub fn root(&self, node: &Node) -> TokenStream2 {
        // A root fragment or loop fills a `div` in place, as children do.
        if matches!(node, Node::Fragment(_) | Node::For(_))
            || matches!(node, Node::Element(el) if is_fragment_tag(el))
        {
            let f = Ident::new("div", Span::call_site());
            let chain = self.append_children(quote!(#f()), std::slice::from_ref(node));
            return quote!(#chain.into_any());
        }
        // Elements already end in `.into_any()`; a root `{expr}` passes
        // through as written.
        match self.node(node) {
            Mode::Child(t) | Mode::Optional(t) => t,
            Mode::Spread(t) => quote!(div().children(#t).into_any()),
        }
    }

    fn node(&self, node: &Node) -> Mode {
        match node {
            Node::Element(el) => self.element(el, None),
            Node::Fragment(children) => Mode::Spread(self.children_vec(children)),
            Node::Expr(expr) => Mode::Child(quote!(#expr)),
            Node::OptionalExpr(expr) => Mode::Optional(quote!(#expr)),
            Node::SpreadExpr(expr) => Mode::Spread(quote! {
                (#expr).into_iter().map(|__quark_child| __quark_child.into_any())
            }),
            Node::Text(lit) => Mode::Child(self.text_value(&[lit])),
            Node::If(chain) => self.if_chain(chain),
            Node::For(fl) => Mode::Spread(self.for_loop(fl)),
            Node::Match(m) => self.match_node(m),
        }
    }

    /// A text literal, with any `{expr}` interpolation, as one value.
    fn text_value(&self, lits: &[&LitStr]) -> TokenStream2 {
        let mut parts = Vec::new();
        for lit in lits {
            match segments(lit) {
                Ok(segs) => parts.push((segs, lit.span())),
                Err(e) => {
                    self.errors.borrow_mut().push(e);
                    return quote!("");
                }
            }
        }
        join(&parts, lits.first().copied().filter(|_| lits.len() == 1))
    }

    // -----------------------------------------------------------------------
    // Children
    // -----------------------------------------------------------------------

    /// Children as a `Vec<AnyElement>` expression.
    fn children_vec<'a>(&self, children: impl IntoIterator<Item = &'a Node>) -> TokenStream2 {
        let stmts: Vec<_> = children
            .into_iter()
            .map(|child| self.child_stmts(child, Sink::Vec))
            .collect();
        quote! {{
            let mut __quark_children = ::std::vec::Vec::new();
            #(#stmts)*
            __quark_children
        }}
    }

    /// A `#[derive(Props)]` component's children, converted with `Into` to
    /// whatever its `children` prop holds. Text stays text until then, so a
    /// component whose child type keeps strings can read them (a button
    /// names itself from its text); `Vec<AnyElement>` works too.
    fn prop_children_vec(&self, children: &[&Node]) -> TokenStream2 {
        let stmts: Vec<_> = children
            .iter()
            .map(|child| self.child_stmts(child, Sink::Props))
            .collect();
        quote! {{
            let mut __quark_children = ::std::vec::Vec::new();
            #(#stmts)*
            __quark_children
        }}
    }

    /// `children` added to a builder `chain` with `.child`,
    /// `.optional_child`, and `.children`. Control flow becomes statements
    /// that add to the builder in place, so an `if`, `match`, or `for`
    /// costs no more than the hand-written builder code: no `Vec` per
    /// branch or iteration.
    fn append_children<'a>(
        &self,
        mut chain: TokenStream2,
        children: impl IntoIterator<Item = &'a Node>,
    ) -> TokenStream2 {
        let children: Vec<&Node> = children.into_iter().collect();
        if !children.iter().any(|c| is_control_flow(c)) {
            for child in children {
                chain = self.append_child(chain, child);
            }
            return chain;
        }
        let stmts: Vec<_> = children
            .iter()
            .map(|child| self.child_stmts(child, Sink::Chain))
            .collect();
        quote! {{
            #[allow(unused_mut)]
            let mut __quark_parent = #chain;
            #(#stmts)*
            __quark_parent
        }}
    }

    fn append_child(&self, chain: TokenStream2, child: &Node) -> TokenStream2 {
        match self.node(child) {
            Mode::Child(t) => quote!(#chain.child(#t)),
            Mode::Optional(t) => quote!(#chain.optional_child(#t)),
            Mode::Spread(t) => quote!(#chain.children(#t)),
        }
    }

    /// Statements adding `node`'s children to `sink`. Fragments, `if`,
    /// `match`, and `for` become the matching Rust statements around their
    /// children's statements, so every child is added where it is built.
    fn child_stmts(&self, node: &Node, sink: Sink) -> TokenStream2 {
        match node {
            Node::Fragment(children) => self.stmts(children, sink),
            Node::Element(el) if is_fragment_tag(el) => {
                self.reject_attrs(el, "a fragment takes no attributes");
                self.stmts(&el.children, sink)
            }
            Node::If(chain) => {
                let mut tokens = TokenStream2::new();
                let mut link = Some(chain);
                let mut first = true;
                while let Some(c) = link {
                    let cond = &c.cond;
                    let body = self.stmts(&c.then_children, sink);
                    if !first {
                        tokens.extend(quote!(else));
                    }
                    first = false;
                    tokens.extend(quote!(if #cond { #body }));
                    if let Some(else_children) = &c.else_children {
                        let body = self.stmts(else_children, sink);
                        tokens.extend(quote!(else { #body }));
                    }
                    link = c.else_if.as_deref();
                }
                tokens
            }
            Node::Match(m) => {
                let arms = m.arms.iter().map(|arm| {
                    let pat = &arm.pat;
                    let guard = arm.guard.as_ref().map(|g| quote!(if #g));
                    let body = self.stmts(&arm.body, sink);
                    quote!(#pat #guard => { #body })
                });
                let match_token = m.match_token;
                let scrutinee = &m.scrutinee;
                quote!(#match_token #scrutinee { #(#arms)* })
            }
            Node::For(fl) => {
                let pat = &fl.pat;
                let iter = &fl.iter;
                let body = match (&fl.key, fl.body.as_slice()) {
                    (None, body) => self.stmts(body, sink),
                    (Some(key), [Node::Element(el)]) => {
                        let mode = self.element(el, Some(key));
                        sink.add(mode)
                    }
                    (Some((name, _)), _) => {
                        self.error(
                            name.span(),
                            "a keyed `for` needs exactly one element in its body to put the key on",
                        );
                        TokenStream2::new()
                    }
                };
                quote!(for #pat in #iter { #body })
            }
            // Text stays text for a props component's children (see
            // `prop_children_vec`).
            Node::Text(lit) if sink == Sink::Props => {
                let text = self.text_value(&[lit]);
                quote!(__quark_children.push(::core::convert::Into::into(#text));)
            }
            other => sink.add(self.node(other)),
        }
    }

    fn stmts(&self, children: &[Node], sink: Sink) -> TokenStream2 {
        children
            .iter()
            .map(|child| self.child_stmts(child, sink))
            .collect()
    }

    /// Branches of an `if` chain or `match`, unified to the widest mode
    /// any branch needs: `Spread` when a branch has zero or several
    /// children (each branch becomes a `Vec`, so children flow into the
    /// parent with no wrapper node), `Optional` when a branch is optional
    /// or can be missing, and `Child` otherwise.
    fn branches(&self, bodies: &[&[Node]], exhaustive: bool) -> (Vec<TokenStream2>, Mode) {
        let singles: Vec<Option<Mode>> = bodies
            .iter()
            .map(|body| match body {
                [only] => match self.node(only) {
                    Mode::Spread(_) => None,
                    mode => Some(mode),
                },
                _ => None,
            })
            .collect();
        if singles.iter().any(Option::is_none) {
            let branches = bodies.iter().map(|b| self.children_vec(b.iter())).collect();
            return (branches, Mode::Spread(quote!(::std::vec::Vec::new())));
        }
        if !exhaustive || singles.iter().any(|m| matches!(m, Some(Mode::Optional(_)))) {
            let branches = singles
                .into_iter()
                .map(|mode| match mode {
                    Some(Mode::Child(t)) => quote!(::core::option::Option::Some((#t).into_any())),
                    Some(Mode::Optional(t)) => {
                        quote!((#t).map(|__quark_child| __quark_child.into_any()))
                    }
                    _ => unreachable!("spread branches take the Vec path"),
                })
                .collect();
            return (
                branches,
                Mode::Optional(quote!(::core::option::Option::None)),
            );
        }
        let branches = singles
            .into_iter()
            .map(|mode| match mode {
                Some(Mode::Child(t)) => quote!((#t).into_any()),
                _ => unreachable!("only plain children reach the Child path"),
            })
            .collect();
        (branches, Mode::Child(TokenStream2::new()))
    }

    fn if_chain(&self, chain: &IfNode) -> Mode {
        let mut conds = Vec::new();
        let mut bodies: Vec<&[Node]> = Vec::new();
        let mut link = Some(chain);
        let mut else_body = None;
        while let Some(c) = link {
            conds.push(&c.cond);
            bodies.push(&c.then_children);
            else_body = c.else_children.as_deref();
            link = c.else_if.as_deref();
        }
        if let Some(body) = else_body {
            bodies.push(body);
        }
        let (branches, mode) = self.branches(&bodies, else_body.is_some());
        let mut branches = branches.into_iter();
        let mut tokens = TokenStream2::new();
        for (i, cond) in conds.iter().enumerate() {
            let body = branches.next().expect("one branch per condition");
            if i > 0 {
                tokens.extend(quote!(else));
            }
            tokens.extend(quote!(if #cond { #body }));
        }
        let (missing, wrap): (TokenStream2, fn(TokenStream2) -> Mode) = match mode {
            Mode::Child(m) => (m, Mode::Child),
            Mode::Optional(m) => (m, Mode::Optional),
            Mode::Spread(m) => (m, Mode::Spread),
        };
        let tail = branches.next().unwrap_or(missing);
        if !tail.is_empty() {
            tokens.extend(quote!(else { #tail }));
        }
        wrap(tokens)
    }

    fn match_node(&self, m: &MatchNode) -> Mode {
        let bodies: Vec<&[Node]> = m.arms.iter().map(|a| a.body.as_slice()).collect();
        let (branches, mode) = self.branches(&bodies, true);
        let arms = m.arms.iter().zip(branches).map(|(arm, body)| {
            let pat = &arm.pat;
            let guard = arm.guard.as_ref().map(|g| quote!(if #g));
            quote!(#pat #guard => { #body })
        });
        let match_token = m.match_token;
        let scrutinee = &m.scrutinee;
        let tokens = quote!(#match_token #scrutinee { #(#arms)* });
        match mode {
            Mode::Child(_) => Mode::Child(tokens),
            Mode::Optional(_) => Mode::Optional(tokens),
            Mode::Spread(_) => Mode::Spread(tokens),
        }
    }

    fn for_loop(&self, fl: &ForNode) -> TokenStream2 {
        let pat = &fl.pat;
        let iter = &fl.iter;
        let body = match (&fl.key, fl.body.as_slice()) {
            (None, _) => self.children_vec(&fl.body),
            (Some(key), [Node::Element(el)]) => {
                let mode = self.element(el, Some(key));
                self.children_vec_of(mode)
            }
            (Some((name, _)), _) => {
                self.error(
                    name.span(),
                    "a keyed `for` needs exactly one element in its body to put the key on",
                );
                quote!(::std::vec::Vec::new())
            }
        };
        quote! {
            (#iter)
                .into_iter()
                .flat_map(|#pat| #body)
                .collect::<::std::vec::Vec<_>>()
        }
    }

    fn children_vec_of(&self, mode: Mode) -> TokenStream2 {
        match mode {
            Mode::Child(t) => quote!(::std::vec![(#t).into_any()]),
            Mode::Optional(t) => {
                quote!((#t).into_iter().map(|__quark_child| __quark_child.into_any()).collect::<::std::vec::Vec<_>>())
            }
            Mode::Spread(t) => quote!((#t).into_iter().collect::<::std::vec::Vec<_>>()),
        }
    }

    // -----------------------------------------------------------------------
    // Elements
    // -----------------------------------------------------------------------

    fn element(&self, el: &Element, key: Option<&(Ident, Expr)>) -> Mode {
        if is_fragment_tag(el) {
            self.reject_attrs(el, "a fragment takes no attributes");
            return Mode::Spread(self.children_vec(&el.children));
        }
        let chain = self.builder(el, key);
        Mode::Child(quote!(#chain.into_any()))
    }

    /// `view! { -> Type, <root/> }`: the root element's builder, not
    /// erased to `AnyElement`, so a helper can return a `Div` (or any
    /// builder) for its callers to keep styling.
    pub fn typed_root(&self, node: &Node, ty: &syn::Type) -> TokenStream2 {
        match node {
            Node::Element(el) if !is_fragment_tag(el) => {
                let chain = self.builder(el, None);
                let root = Ident::new("__quark_root", Span::call_site());
                quote_spanned!(ty.span()=> { let #root: #ty = #chain; #root })
            }
            other => {
                self.error(
                    node_span(other),
                    "`view! { -> Type, .. }` returns the root element's builder, so the root \
                     must be one element; fragments, control flow, and `{expr}` have none",
                );
                quote!(())
            }
        }
    }

    /// One element's builder chain, before `.into_any()`.
    fn builder(&self, el: &Element, key: Option<&(Ident, Expr)>) -> TokenStream2 {
        let key_call = key.map(|(name, expr)| {
            let m = Ident::new("key", name.span());
            quote!(.#m(#expr))
        });
        match &el.tag {
            Tag::Builtin(name) => {
                let tag = name.to_string();
                match tag.as_str() {
                    "div" => self.div(el, key_call),
                    "text" => self.text(el),
                    "p" => self.rich_text(el),
                    "icon" => self.icon(el),
                    "spacer" => {
                        self.reject_attrs(el, "<spacer> takes no attributes");
                        self.reject_children(el, "<spacer> takes no children");
                        let f = Ident::new("spacer", el.span);
                        quote!(#f())
                    }
                    t if INLINE_TAGS.contains(&t) => {
                        self.error(
                            el.span,
                            format!("<{t}> is inline rich text; put it inside <p>...</p>"),
                        );
                        quote!("")
                    }
                    t => {
                        let hint = closest(t, BUILTIN_TAGS.iter().copied())
                            .map(|s| format!("did you mean <{s}>? "))
                            .unwrap_or_default();
                        self.error(
                            el.span,
                            format!(
                                "unknown tag <{t}>; {hint}Built-in tags are {}, and components \
                                 start with an uppercase letter",
                                BUILTIN_TAGS.join(", ")
                            ),
                        );
                        quote!("")
                    }
                }
            }
            Tag::Component(path) => self.component(Some(path), el, key_call),
            Tag::Function(path) => self.function(path, el, key_call),
            Tag::Value(_) => self.component(None, el, key_call),
            Tag::Slot(name) => {
                self.error(
                    name.span(),
                    format!("slot <.{name}> must be a direct child of a component"),
                );
                quote!("")
            }
        }
    }

    fn reject_attrs(&self, el: &Element, message: &str) {
        for attr in &el.attrs {
            self.error(attr.span(), message);
        }
    }

    fn reject_children(&self, el: &Element, message: &str) {
        for child in &el.children {
            self.error(node_span(child), message);
        }
    }

    fn div(&self, el: &Element, key_call: Option<TokenStream2>) -> TokenStream2 {
        let f = Ident::new("div", el.span);
        let mut chain = self.attrs(quote!(#f()), &el.attrs, Target::Div);
        chain.extend(key_call);
        self.append_children(chain, &el.children)
    }

    /// `<text>`: plain label text. Literal and `{expr}` children join into
    /// one string.
    fn text(&self, el: &Element) -> TokenStream2 {
        let content = match el.children.as_slice() {
            [] => quote!(""),
            // A lone expression passes through as is (`text` takes
            // `impl Into<String>`), so it is not formatted or copied.
            [Node::Expr(e)] => quote!(#e),
            children => {
                let mut parts = Vec::new();
                for child in children {
                    match child {
                        Node::Text(lit) => match segments(lit) {
                            Ok(segs) => parts.push((segs, lit.span())),
                            Err(e) => self.errors.borrow_mut().push(e),
                        },
                        Node::Expr(e) => {
                            parts.push((vec![Segment::Expr(quote!(#e), None)], e.span()))
                        }
                        Node::Element(inner) if matches!(&inner.tag, Tag::Builtin(n) if INLINE_TAGS.contains(&n.to_string().as_str())) =>
                        {
                            self.error(
                                inner.span,
                                "<text> is plain text; use <p> for inline markup like <b>",
                            );
                        }
                        other => self.error(
                            node_span(other),
                            "<text> takes \"text\" and {expressions}; put elements in a <div>",
                        ),
                    }
                }
                let lone = match children {
                    [Node::Text(lit)] => Some(lit),
                    _ => None,
                };
                join(&parts, lone)
            }
        };
        let f = Ident::new("text", el.span);
        self.attrs(quote!(#f(#content)), &el.attrs, Target::Text)
    }

    /// `<p>`: selectable rich text. Inline tags style runs of it.
    fn rich_text(&self, el: &Element) -> TokenStream2 {
        let mut spans = Vec::new();
        self.spans(&el.children, &TokenStream2::new(), &mut spans);
        let f = Ident::new("selectable_rich_text", el.span);
        self.attrs(
            quote! {
                #f({
                    let mut __quark_spans = ::std::vec::Vec::new();
                    #(#spans)*
                    __quark_spans
                })
            },
            &el.attrs,
            Target::Rich,
        )
    }

    /// Statements pushing `StyledSpan`s for `children`, each with `style`
    /// calls applied.
    fn spans(&self, children: &[Node], style: &TokenStream2, out: &mut Vec<TokenStream2>) {
        for child in children {
            match child {
                Node::Text(lit) => {
                    let text = self.text_value(&[lit]);
                    out.push(quote!(__quark_spans.push(StyledSpan::plain(#text) #style);));
                }
                Node::Expr(e) => out.push(quote! {
                    __quark_spans.push(
                        StyledSpan::plain(::std::string::ToString::to_string(&(#e))) #style
                    );
                }),
                // `{...spans}`: an iterator of ready-made `StyledSpan`s.
                Node::SpreadExpr(e) => out.push(quote!(__quark_spans.extend(#e);)),
                Node::Element(inner) => self.inline(inner, style, out),
                other => self.error(
                    node_span(other),
                    "<p> takes \"text\", {expressions}, {...spans}, and inline tags \
                     (<b>, <i>, <code>, <a href>, <br/>, ...)",
                ),
            }
        }
    }

    fn inline(&self, el: &Element, style: &TokenStream2, out: &mut Vec<TokenStream2>) {
        let Tag::Builtin(name) = &el.tag else {
            self.error(
                el.span,
                "<p> takes only inline tags; put components in a <div>",
            );
            return;
        };
        let tag = name.to_string();
        let span = el.span;
        let mut style = style.clone();
        let mut attrs: Vec<&Attr> = el.attrs.iter().collect();
        match tag.as_str() {
            "b" | "strong" => style.extend(quote_spanned!(span=> .bold())),
            "i" | "em" => style.extend(quote_spanned!(span=> .italic())),
            "code" => style.extend(quote_spanned!(span=> .code())),
            "u" => style.extend(quote_spanned!(span=> .underline())),
            "s" | "del" => style.extend(quote_spanned!(span=> .strikethrough())),
            "span" => {}
            "a" => {
                let href = attrs
                    .iter()
                    .position(|a| matches!(a, Attr::Method { name, .. } if name.written == "href"));
                match href.map(|i| attrs.remove(i)) {
                    Some(Attr::Method {
                        name,
                        value: AttrValue::Expr(url),
                    }) => {
                        let m = Ident::new("link", name.first.span());
                        style.extend(quote!(.#m(#url)));
                    }
                    _ => self.error(span, "<a> needs `href=\"..\"` or `href={..}`"),
                }
            }
            "br" => {
                self.reject_attrs(el, "<br/> takes no attributes");
                self.reject_children(el, "<br/> takes no children");
                out.push(quote!(__quark_spans.push(StyledSpan::plain("\n") #style);));
                return;
            }
            t => {
                let hint = closest(t, INLINE_TAGS.iter().copied())
                    .map(|s| format!("did you mean <{s}>? "))
                    .unwrap_or_default();
                self.error(
                    span,
                    format!(
                        "<{t}> cannot go inside <p>; {hint}inline tags are {}",
                        INLINE_TAGS.join(", ")
                    ),
                );
                return;
            }
        }
        for attr in attrs {
            style = self.attr(style, attr, Target::Span);
        }
        self.spans(&el.children, &style, out);
    }

    fn icon(&self, el: &Element) -> TokenStream2 {
        self.reject_children(el, "<icon> takes no children; pass `svg={...}`");
        let mut svg = quote!("");
        let mut size = quote!(16.0);
        let mut rest = Vec::new();
        for attr in &el.attrs {
            match attr {
                Attr::Method { name, value } if name.written == "svg" || name.written == "size" => {
                    let v = match value {
                        AttrValue::Expr(e) => quote!(#e),
                        AttrValue::Reactive(e) => quote!(cx.read(#e)),
                        AttrValue::If(e) if e.else_branch.is_some() => quote!(#e),
                        _ => {
                            self.error(
                                name.first.span(),
                                format!("`{}` takes a value", name.written),
                            );
                            continue;
                        }
                    };
                    if name.written == "svg" {
                        svg = v;
                    } else {
                        size = v;
                    }
                }
                other => rest.push(other),
            }
        }
        let f = Ident::new("svg_icon", el.span);
        let mut chain = quote!(#f(#svg, #size));
        for attr in rest {
            chain = self.attr(chain, attr, Target::Icon);
        }
        chain
    }

    /// `<Name>`, `<Name(args)>`, `<name(args)>`, and `<{expr}>` (no path).
    fn component(
        &self,
        path: Option<&syn::Path>,
        el: &Element,
        key_call: Option<TokenStream2>,
    ) -> TokenStream2 {
        let function = matches!(el.tag, Tag::Function(_));
        let value = match &el.tag {
            Tag::Value(expr) => Some(expr),
            _ => None,
        };
        let props = el.ctor_args.is_none() && value.is_none();
        let target = if props {
            Target::Props
        } else {
            Target::Builder
        };
        let mut chain = if let Some(expr) = value {
            quote!((#expr))
        } else if function {
            let args = el.ctor_args.iter().flatten();
            quote!(#path(#(#args),*))
        } else if props {
            let builder = Ident::new("builder", el.span);
            quote!(#path::#builder())
        } else {
            let args = el.ctor_args.iter().flatten();
            let new = Ident::new("new", el.span);
            quote!(#path::#new(#(#args),*))
        };
        chain = self.attrs(chain, &el.attrs, target);
        chain.extend(key_call);

        let mut rest = Vec::new();
        for child in &el.children {
            match child {
                Node::Element(Element {
                    tag: Tag::Slot(method),
                    attrs,
                    children,
                    ..
                }) => {
                    for attr in attrs {
                        self.error(
                            attr.span(),
                            "slot tags take no attributes; put builder calls on the component",
                        );
                    }
                    if children.is_empty() {
                        self.error(method.span(), format!("slot <.{method}> has no children"));
                    }
                    chain = self.slot(chain, method, children);
                }
                other => rest.push(other),
            }
        }
        if props {
            if !rest.is_empty() {
                let vec = self.prop_children_vec(&rest);
                let m = Ident::new("children", node_span(rest[0]));
                chain = quote!(#chain.#m(#vec));
            }
            let build = Ident::new("build", el.span);
            quote!(#chain.#build())
        } else {
            self.append_children(chain, rest)
        }
    }

    /// `<name(args) attrs>children</name>`: `name(args)`, then the same
    /// attribute calls, slots, and `.child(..)` calls as a builder
    /// component.
    fn function(
        &self,
        path: &syn::Path,
        el: &Element,
        key_call: Option<TokenStream2>,
    ) -> TokenStream2 {
        if let Some(name) = path.get_ident().map(Ident::to_string)
            && (BUILTIN_TAGS.contains(&name.as_str()) || INLINE_TAGS.contains(&name.as_str()))
        {
            self.error(
                el.span,
                format!("<{name}> is a built-in tag and takes no arguments; use attributes"),
            );
            return quote!("");
        }
        self.component(Some(path), el, key_call)
    }

    /// `.method(child)` once per child of a `<.method>` slot. Slot values
    /// pass through unconverted: a slot can take any type.
    fn slot(&self, mut chain: TokenStream2, method: &Ident, children: &[Node]) -> TokenStream2 {
        for child in children {
            chain = match self.node(child) {
                Mode::Child(t) => quote!(#chain.#method(#t)),
                Mode::Optional(t) => quote! {{
                    let __quark_slot = #chain;
                    match #t {
                        ::core::option::Option::Some(__quark_child) => __quark_slot.#method(__quark_child),
                        ::core::option::Option::None => __quark_slot,
                    }
                }},
                Mode::Spread(t) => quote! {
                    (#t).into_iter().fold(#chain, |__quark_slot, __quark_child| {
                        __quark_slot.#method(__quark_child)
                    })
                },
            };
        }
        chain
    }

    // -----------------------------------------------------------------------
    // Attributes
    // -----------------------------------------------------------------------

    fn attrs(&self, mut chain: TokenStream2, attrs: &[Attr], target: Target) -> TokenStream2 {
        for attr in attrs {
            chain = self.attr(chain, attr, target);
        }
        chain
    }

    fn attr(&self, chain: TokenStream2, attr: &Attr, target: Target) -> TokenStream2 {
        match attr {
            Attr::Class(lit) => self.class(chain, lit, target),
            Attr::When(cond, attrs) => {
                let inner = self.attrs(quote!(__w), attrs, target);
                quote!({ let __w = #chain; if #cond { #inner } else { __w } })
            }
            Attr::For(pat, iter, attrs) => {
                let inner = self.attrs(quote!(__w), attrs, target);
                quote!({
                    let mut __w = #chain;
                    for #pat in #iter {
                        __w = #inner;
                    }
                    __w
                })
            }
            Attr::Method { name, value } => {
                if let Some(event) = &name.event {
                    return self.event(chain, name, event, value);
                }
                if name.written == "role" {
                    return self.role(chain, name, value);
                }
                if name.written.starts_with("aria-") {
                    return self.aria(chain, name, value);
                }
                let method = method_ident(&name.segments, name.first.span());
                self.call(chain, &method, value, target)
            }
        }
    }

    /// `.method(value)`, with flags, tuples, `{@sig}`, `{if ..}`, and
    /// auto-scaling handled.
    fn call(
        &self,
        chain: TokenStream2,
        method: &Ident,
        value: &AttrValue,
        target: Target,
    ) -> TokenStream2 {
        let scale = self.scale_for(method, target);
        let scaled = |v: TokenStream2| match &scale {
            Some(s) => quote!(((#v) * #s).round()),
            None => v,
        };
        match value {
            AttrValue::Flag if target == Target::Props => {
                quote_spanned!(method.span()=> #chain.#method(true))
            }
            AttrValue::Flag => quote!(#chain.#method()),
            // `name={(a, b)}` splats into a multi-argument call; a prop
            // takes the tuple as its one value.
            AttrValue::Expr(Expr::Tuple(tuple)) if target != Target::Props => {
                let elems = &tuple.elems;
                quote!(#chain.#method(#elems))
            }
            AttrValue::Expr(e) => {
                let v = scaled(quote!(#e));
                quote!(#chain.#method(#v))
            }
            AttrValue::Reactive(e) => {
                let v = scaled(quote!(cx.read(#e)));
                quote!(#chain.#method(#v))
            }
            AttrValue::If(if_expr) => self.if_attr(chain, method, if_expr, &scale),
        }
    }

    fn scale_for(&self, method: &Ident, target: Target) -> Option<Ident> {
        let scale = self.scale.as_ref()?;
        (target == Target::Div && SPATIAL_ATTRS.contains(&method.to_string().as_str()))
            .then(|| scale.clone())
    }

    fn if_attr(
        &self,
        chain: TokenStream2,
        method: &Ident,
        if_expr: &ExprIf,
        scale: &Option<Ident>,
    ) -> TokenStream2 {
        let cond = &if_expr.cond;
        let scaled = |v: TokenStream2| match scale {
            Some(s) => quote!(((#v) * #s).round()),
            None => v,
        };
        let then_val = block_value(&if_expr.then_branch);
        match &if_expr.else_branch {
            Some((_, else_expr)) => {
                let else_val = match &**else_expr {
                    Expr::Block(b) => block_value(&b.block),
                    other => quote!(#other),
                };
                let v = scaled(quote!(if #cond { #then_val } else { #else_val }));
                quote!(#chain.#method(#v))
            }
            None => {
                let v = scaled(then_val);
                quote!({ let __t = #chain; if #cond { __t.#method(#v) } else { __t } })
            }
        }
    }

    fn class(&self, chain: TokenStream2, lit: &LitStr, target: Target) -> TokenStream2 {
        let on = match target {
            Target::Div => Some(On::Box),
            Target::Text => Some(On::Text),
            Target::Icon => Some(On::Icon),
            Target::Builder | Target::Props => None,
            Target::Rich | Target::Span => {
                self.error(
                    lit.span(),
                    "`class` is not supported on rich text; use attributes such as `color={..}`",
                );
                return chain;
            }
        };
        let mut errors = Vec::new();
        let lowered = classes::lower(&lit.value(), lit.span(), on, &mut errors);
        self.errors.borrow_mut().extend(errors);
        let calls = lowered.calls;
        let mut chain = quote!(#chain #(#calls)*);
        if !lowered.hover.is_empty() {
            let hover = Ident::new("hover", lit.span());
            let calls = lowered.hover;
            chain = quote!(#chain.#hover(|__quark_hover| __quark_hover #(#calls)*));
        }
        chain
    }

    /// `on:click={..}` -> `.on_click(..)`; `on:key:mod+s={..}` ->
    /// `.on_key("mod+s", ..)`.
    fn event(
        &self,
        chain: TokenStream2,
        name: &AttrName,
        event: &EventName,
        value: &AttrValue,
    ) -> TokenStream2 {
        let span = event.name.span();
        let method = format_ident!(
            "on_{}",
            event.name.to_string().replace('-', "_"),
            span = span
        );
        let handler = match value {
            AttrValue::Expr(e) => quote!(#e),
            AttrValue::Reactive(e) => quote!(cx.read(#e)),
            // `on:click={if let Some(m) = msg { m }}`: no `else` leaves
            // the handler unset.
            AttrValue::If(if_expr) if event.binding.is_none() => {
                return self.if_attr(chain, &method, if_expr, &None);
            }
            _ => {
                self.error(
                    name.first.span(),
                    format!(
                        "`{}` needs a handler, as in `{}={{action}}`",
                        name.written, name.written
                    ),
                );
                return chain;
            }
        };
        match &event.binding {
            Some(binding) => {
                if event.name != "key" {
                    self.error(
                        span,
                        format!("only `on:key` takes a binding; found `{}`", name.written),
                    );
                }
                self.check_binding(binding);
                quote!(#chain.#method(#binding, #handler))
            }
            None => quote!(#chain.#method(#handler)),
        }
    }

    /// Modifiers are checked here so a typo fails the build instead of
    /// being logged when the binding is registered.
    fn check_binding(&self, binding: &LitStr) {
        let text = binding.value();
        let mods = match text.strip_suffix("++") {
            Some(mods) => mods,
            None => text.rsplit_once('+').map_or("", |(m, _)| m),
        };
        for part in mods.split('+').filter(|p| !p.is_empty()) {
            if !KEY_MODIFIERS.contains(&part.to_ascii_lowercase().as_str()) {
                let hint = closest(part, KEY_MODIFIERS.iter().copied())
                    .map(|s| format!("did you mean `{s}`? "))
                    .unwrap_or_default();
                self.error(
                    binding.span(),
                    format!(
                        "unknown modifier `{part}` in key binding `{text}`; {hint}modifiers are \
                         mod, cmd, ctrl, alt, shift"
                    ),
                );
            }
        }
    }

    fn role(&self, chain: TokenStream2, name: &AttrName, value: &AttrValue) -> TokenStream2 {
        let span = name.first.span();
        let method = Ident::new("semantic_role", span);
        match value {
            AttrValue::Expr(Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(lit),
                ..
            })) => {
                let role = lit.value();
                match ROLES.iter().find(|(aria, _)| *aria == role) {
                    Some((_, variant)) => {
                        let variant = Ident::new(variant, lit.span());
                        quote!(#chain.#method(::quark::SemanticRole::#variant))
                    }
                    None => {
                        let hint = closest(&role, ROLES.iter().map(|(r, _)| *r))
                            .map(|s| format!("did you mean \"{s}\"? "))
                            .unwrap_or_default();
                        self.error(
                            lit.span(),
                            format!(
                                "unknown role \"{role}\"; {hint}roles are {}",
                                ROLES.iter().map(|(r, _)| *r).collect::<Vec<_>>().join(", ")
                            ),
                        );
                        chain
                    }
                }
            }
            AttrValue::Flag => {
                self.error(span, "`role` takes a value, as in `role=\"button\"`");
                chain
            }
            other => self.call(chain, &method, other, Target::Builder),
        }
    }

    fn aria(&self, chain: TokenStream2, name: &AttrName, value: &AttrValue) -> TokenStream2 {
        let span = name.first.span();
        let Some((_, method)) = ARIA.iter().find(|(aria, _)| *aria == name.written) else {
            let hint = closest(&name.written, ARIA.iter().map(|(a, _)| *a))
                .map(|s| format!("did you mean `{s}`? "))
                .unwrap_or_default();
            self.error(
                span,
                format!(
                    "unknown attribute `{}`; {hint}supported: {}",
                    name.written,
                    ARIA.iter().map(|(a, _)| *a).collect::<Vec<_>>().join(", ")
                ),
            );
            return chain;
        };
        let method = Ident::new(method, span);
        match value {
            // `aria-disabled` alone means true, as in HTML.
            AttrValue::Flag => quote!(#chain.#method(true)),
            other => self.call(chain, &method, other, Target::Builder),
        }
    }
}

/// A block's value without its braces when it is one expression, so the
/// emitted code does not trip `unused_braces`.
fn block_value(block: &syn::Block) -> TokenStream2 {
    match block.stmts.as_slice() {
        [syn::Stmt::Expr(e, None)] => quote!(#e),
        _ => quote!(#block),
    }
}

/// `min-w` -> `min_w`, spanned at the attribute so errors and
/// rust-analyzer point there. Keywords become raw identifiers.
fn method_ident(segments: &[Ident], span: Span) -> Ident {
    let name = segments
        .iter()
        .map(|s| s.to_string())
        .collect::<Vec<_>>()
        .join("_");
    match syn::parse_str::<Ident>(&name) {
        Ok(_) => Ident::new(&name, span),
        Err(_) => Ident::new_raw(&name, span),
    }
}

/// Children that `append_children` lowers to statements.
fn is_control_flow(node: &Node) -> bool {
    match node {
        Node::If(_) | Node::For(_) | Node::Match(_) | Node::Fragment(_) => true,
        Node::Element(el) => is_fragment_tag(el),
        _ => false,
    }
}

fn is_fragment_tag(el: &Element) -> bool {
    matches!(&el.tag, Tag::Builtin(name) if name == "fragment")
}

fn node_span(node: &Node) -> Span {
    match node {
        Node::Element(el) => el.span,
        Node::Fragment(children) => children.first().map_or_else(Span::call_site, node_span),
        Node::Expr(e) | Node::OptionalExpr(e) | Node::SpreadExpr(e) => e.span(),
        Node::Text(lit) => lit.span(),
        Node::If(chain) => chain.cond.span(),
        Node::For(fl) => fl.iter.span(),
        Node::Match(m) => m.match_token.span,
    }
}
