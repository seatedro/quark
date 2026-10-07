use std::cell::RefCell;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Expr, ExprIf, ExprMatch, Ident, LitStr, Pat, Result, Token, braced, parenthesized};

// ---------------------------------------------------------------------------
// #[derive(Store)] — generate a parallel `XStore` struct where each field is a
// `Signal<T>` handle (or a nested `YStore` for `#[store(flatten)]` fields).
// ---------------------------------------------------------------------------

#[proc_macro_derive(Store, attributes(store))]
pub fn derive_store(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as syn::DeriveInput);
    match derive_store_impl(input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

enum FieldKind {
    Leaf,
    Flatten,
    Skip,
}

fn parse_store_field_attr(attrs: &[syn::Attribute]) -> Result<FieldKind> {
    let mut kind: Option<(FieldKind, proc_macro2::Span)> = None;
    for attr in attrs {
        if !attr.path().is_ident("store") {
            continue;
        }
        attr.parse_nested_meta(|m| {
            let (new_kind, label) = if m.path.is_ident("flatten") {
                (FieldKind::Flatten, "flatten")
            } else if m.path.is_ident("skip") {
                (FieldKind::Skip, "skip")
            } else {
                return Err(
                    m.error("unknown `#[store(...)]` attribute; supported: `flatten`, `skip`")
                );
            };
            if let Some((_, prev_span)) = kind {
                let mut err = syn::Error::new(
                    m.path.span(),
                    format!(
                        "conflicting `#[store({label})]` — field already has a store attribute"
                    ),
                );
                err.combine(syn::Error::new(
                    prev_span,
                    "previous `#[store(...)]` attribute here",
                ));
                return Err(err);
            }
            kind = Some((new_kind, m.path.span()));
            Ok(())
        })?;
    }
    Ok(kind.map(|(k, _)| k).unwrap_or(FieldKind::Leaf))
}

/// Struct-level `#[store(default)]`: returns whether `new_default` is wanted.
fn parse_store_struct_attr(attrs: &[syn::Attribute]) -> Result<bool> {
    let mut default = false;
    for attr in attrs {
        if !attr.path().is_ident("store") {
            continue;
        }
        attr.parse_nested_meta(|m| {
            if m.path.is_ident("default") {
                default = true;
                Ok(())
            } else {
                Err(m.error("unknown struct-level `#[store(...)]` attribute; supported: `default`"))
            }
        })?;
    }
    Ok(default)
}

fn flatten_store_type(ty: &syn::Type) -> Result<syn::Type> {
    let syn::Type::Path(tp) = ty else {
        return Err(syn::Error::new_spanned(
            ty,
            "`#[store(flatten)]` expects a named struct type (e.g. `Foo` or `path::to::Foo`); \
             arrays, tuples, references, and generics are not supported",
        ));
    };
    let mut new_path = tp.clone();
    let last = new_path.path.segments.last_mut().unwrap();
    last.ident = Ident::new(&format!("{}Store", last.ident), last.ident.span());
    Ok(syn::Type::Path(new_path))
}

fn derive_store_impl(input: syn::DeriveInput) -> Result<TokenStream2> {
    if !input.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &input.generics,
            "`#[derive(Store)]` cannot be applied to a generic type yet; \
             consider wrapping concrete instantiations instead",
        ));
    }

    let name = &input.ident;
    let store_name = Ident::new(&format!("{name}Store"), name.span());
    let vis = &input.vis;

    let fields = match &input.data {
        syn::Data::Struct(s) => match &s.fields {
            syn::Fields::Named(n) => &n.named,
            syn::Fields::Unnamed(_) => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "`#[derive(Store)]` requires a struct with named fields; \
                     tuple structs are not supported",
                ));
            }
            syn::Fields::Unit => {
                return Err(syn::Error::new_spanned(
                    &input.ident,
                    "`#[derive(Store)]` on a unit struct is meaningless — no fields to store",
                ));
            }
        },
        syn::Data::Enum(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`#[derive(Store)]` does not support enums; \
                 consider making the enum a leaf field of a struct that derives `Store`",
            ));
        }
        syn::Data::Union(_) => {
            return Err(syn::Error::new_spanned(
                &input.ident,
                "`#[derive(Store)]` does not support unions",
            ));
        }
    };

    let mut decls = Vec::new();
    let mut inits = Vec::new();
    let mut snapshot_fields = Vec::new();
    let mut any_skipped = false;

    for field in fields {
        let fname = field.ident.as_ref().unwrap();
        let fty = &field.ty;
        let fvis = &field.vis;

        match parse_store_field_attr(&field.attrs)? {
            FieldKind::Skip => {
                any_skipped = true;
            }
            FieldKind::Leaf => {
                decls.push(quote! {
                    #fvis #fname: ::quark::reactive::Signal<#fty>,
                });
                inits.push(quote! {
                    #fname: store.create(initial.#fname),
                });
                snapshot_fields.push(quote! {
                    #fname: store.read(self.#fname),
                });
            }
            FieldKind::Flatten => {
                let store_ty = flatten_store_type(fty)?;
                decls.push(quote! {
                    #fvis #fname: #store_ty,
                });
                inits.push(quote! {
                    #fname: <#store_ty>::new(store, initial.#fname),
                });
                snapshot_fields.push(quote! {
                    #fname: self.#fname.snapshot(store),
                });
            }
        }
    }

    // `snapshot()` can only reconstruct the original when every field is
    // represented in the store. If any field is `#[store(skip)]`, we can't
    // fill it in — omit the method.
    let snapshot_impl = if any_skipped {
        quote! {}
    } else {
        quote! {
            /// Read every signal and reconstruct the original plain struct.
            pub fn snapshot(&self, store: &::quark::reactive::SignalStore) -> #name {
                #name {
                    #(#snapshot_fields)*
                }
            }
        }
    };

    // `new_default` is opt-in: an unconditional `where Name: Default` bound
    // is trivially false on a non-`Default` struct and fails to compile.
    let new_default_impl = if parse_store_struct_attr(&input.attrs)? {
        quote! {
            /// Create a store initialized from `Original::default()`.
            pub fn new_default(store: &::quark::reactive::SignalStore) -> Self {
                Self::new(store, <#name as ::core::default::Default>::default())
            }
        }
    } else {
        quote! {}
    };

    Ok(quote! {
        #[derive(Clone, Copy, Debug)]
        #vis struct #store_name {
            #(#decls)*
        }

        impl #store_name {
            /// Create a new store by consuming an initial value, allocating
            /// signals for each leaf field in the provided `SignalStore`.
            pub fn new(
                store: &::quark::reactive::SignalStore,
                initial: #name,
            ) -> Self {
                Self {
                    #(#inits)*
                }
            }

            #new_default_impl
            #snapshot_impl
        }
    })
}

#[proc_macro]
pub fn view(input: TokenStream) -> TokenStream {
    let view_input: ViewInput = match syn::parse(input) {
        Ok(v) => v,
        Err(e) => return e.to_compile_error().into(),
    };
    let ctx = EmitCtx {
        scale: view_input.scale,
        errors: RefCell::new(None),
    };
    let tokens = match ctx.emit_node(&view_input.root) {
        ChildMode::Child(t) | ChildMode::Optional(t) => t,
        ChildMode::Spread(t) => quote! { div().children(#t).into_any() },
    };
    // Any error replaces the whole expansion, so a rejected input never
    // compiles into partial or silently altered UI.
    match ctx.errors.into_inner() {
        Some(err) => {
            // A block, so several `compile_error!`s still form one expression.
            let errors = err.to_compile_error();
            quote! {{ #errors }}.into()
        }
        None => tokens.into(),
    }
}

// ---------------------------------------------------------------------------
// Top-level input: optional `scale,` then a node
// ---------------------------------------------------------------------------

struct ViewInput {
    scale: Option<Ident>,
    root: Node,
}

impl Parse for ViewInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let scale = if input.peek(Ident::peek_any) && input.peek2(Token![,]) {
            let ident: Ident = input.parse()?;
            input.parse::<Token![,]>()?;
            Some(ident)
        } else {
            None
        };
        let root = input.parse()?;
        Ok(ViewInput { scale, root })
    }
}

// ---------------------------------------------------------------------------
// AST
// ---------------------------------------------------------------------------

enum Node {
    Element(ElementNode),
    Expr(Expr),
    OptionalExpr(Expr),
    SpreadExpr(Expr),
    Text(LitStr),
    IfChain(IfChainNode),
    ForLoop(ForLoopNode),
    MatchExpr(MatchNode),
}

struct IfChainNode {
    cond: Expr,
    then_children: Vec<Node>,
    else_if: Option<Box<IfChainNode>>,
    else_children: Option<Vec<Node>>,
}

struct ForLoopNode {
    pat: Pat,
    iter: Expr,
    body_children: Vec<Node>,
}

struct MatchNode {
    expr: ExprMatch,
}

struct ElementNode {
    tag: Tag,
    /// `<Component(a, b)>`: arguments for `Component::new`.
    ctor_args: Option<Punctuated<Expr, Token![,]>>,
    attrs: Vec<Attr>,
    children: Vec<Node>,
}

#[derive(Clone)]
enum Tag {
    Div,
    Text,
    Icon,
    Spacer,
    Fragment,
    Component(syn::Path),
    /// `<.method>` inside a component: each child becomes `.method(child)`.
    Slot(Ident),
}

enum Attr {
    Flag(Ident),
    KeyValue(Ident, Expr),
    /// `name={@sig}` — value is a Signal/Memo, emitted as `cx.read(sig)`.
    /// Requires `cx` to be in scope at the view! call site.
    ReactiveKeyValue(Ident, Expr),
    Class(LitStr),
    IfAttr(Ident, ExprIf),
    When(Expr, Vec<Attr>),
}

impl Attr {
    fn span(&self) -> Span {
        match self {
            Attr::Flag(name)
            | Attr::KeyValue(name, _)
            | Attr::ReactiveKeyValue(name, _)
            | Attr::IfAttr(name, _) => name.span(),
            Attr::Class(lit) => lit.span(),
            Attr::When(cond, _) => cond.span(),
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

impl Parse for Node {
    fn parse(input: ParseStream) -> Result<Self> {
        if input.peek(Token![<]) {
            Ok(Node::Element(input.parse()?))
        } else if input.peek(Token![if]) {
            Ok(Node::IfChain(input.parse()?))
        } else if input.peek(Token![for]) {
            Ok(Node::ForLoop(input.parse()?))
        } else if input.peek(Token![match]) {
            let expr: ExprMatch = input.parse()?;
            Ok(Node::MatchExpr(MatchNode { expr }))
        } else if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            if content.peek(Token![?]) {
                content.parse::<Token![?]>()?;
                let expr: Expr = content.parse()?;
                Ok(Node::OptionalExpr(expr))
            } else if content.peek(Token![...]) {
                content.parse::<Token![...]>()?;
                let expr: Expr = content.parse()?;
                Ok(Node::SpreadExpr(expr))
            } else {
                let expr: Expr = content.parse()?;
                Ok(Node::Expr(expr))
            }
        } else if input.peek(LitStr) {
            Ok(Node::Text(input.parse()?))
        } else {
            Err(input.error("expected <element>, if, for, match, {expr}, or \"string\""))
        }
    }
}

impl Parse for IfChainNode {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Token![if]>()?;

        let cond: Expr = Expr::parse_without_eager_brace(input)?;

        let then_brace;
        braced!(then_brace in input);
        let then_children = parse_children(&then_brace)?;

        let (else_if, else_children) = if input.peek(Token![else]) {
            input.parse::<Token![else]>()?;
            if input.peek(Token![if]) {
                let nested: IfChainNode = input.parse()?;
                (Some(Box::new(nested)), None)
            } else {
                let else_brace;
                braced!(else_brace in input);
                let children = parse_children(&else_brace)?;
                (None, Some(children))
            }
        } else {
            (None, None)
        };

        Ok(IfChainNode {
            cond,
            then_children,
            else_if,
            else_children,
        })
    }
}

impl Parse for ForLoopNode {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Token![for]>()?;
        let pat = Pat::parse_multi_with_leading_vert(input)?;
        input.parse::<Token![in]>()?;
        let iter: Expr = Expr::parse_without_eager_brace(input)?;

        let body_brace;
        braced!(body_brace in input);
        let body_children = parse_children(&body_brace)?;

        Ok(ForLoopNode {
            pat,
            iter,
            body_children,
        })
    }
}

fn parse_children(input: ParseStream) -> Result<Vec<Node>> {
    let mut children = Vec::new();
    while !input.is_empty() {
        children.push(input.parse::<Node>()?);
    }
    Ok(children)
}

impl Parse for ElementNode {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Token![<]>()?;

        let tag = parse_tag(input)?;

        let ctor_args = if input.peek(syn::token::Paren) {
            if !matches!(tag, Tag::Component(_)) {
                return Err(input.error("only component tags take constructor arguments"));
            }
            let content;
            parenthesized!(content in input);
            Some(content.parse_terminated(Expr::parse, Token![,])?)
        } else {
            None
        };

        let mut attrs = Vec::new();
        while !input.peek(Token![>]) && !input.peek(Token![/]) {
            attrs.push(input.parse::<Attr>()?);
        }

        if input.peek(Token![/]) {
            input.parse::<Token![/]>()?;
            input.parse::<Token![>]>()?;
            return Ok(ElementNode {
                tag,
                ctor_args,
                attrs,
                children: Vec::new(),
            });
        }

        input.parse::<Token![>]>()?;

        let mut children = Vec::new();
        while !is_closing_tag(input) {
            children.push(input.parse::<Node>()?);
        }

        parse_closing_tag(input, &tag)?;

        Ok(ElementNode {
            tag,
            ctor_args,
            attrs,
            children,
        })
    }
}

fn parse_tag(input: ParseStream) -> Result<Tag> {
    if input.peek(Token![.]) {
        input.parse::<Token![.]>()?;
        return Ok(Tag::Slot(input.call(Ident::parse_any)?));
    }
    let ident: Ident = input.parse()?;
    match ident.to_string().as_str() {
        "div" => Ok(Tag::Div),
        "text" => Ok(Tag::Text),
        "icon" => Ok(Tag::Icon),
        "spacer" => Ok(Tag::Spacer),
        "fragment" => Ok(Tag::Fragment),
        _ => {
            let mut path = syn::Path::from(ident);
            while input.peek(Token![::]) {
                input.parse::<Token![::]>()?;
                let seg: Ident = input.parse()?;
                path.segments.push(seg.into());
            }
            Ok(Tag::Component(path))
        }
    }
}

fn is_closing_tag(input: ParseStream) -> bool {
    input.peek(Token![<]) && input.peek2(Token![/])
}

fn parse_closing_tag(input: ParseStream, open_tag: &Tag) -> Result<()> {
    input.parse::<Token![<]>()?;
    input.parse::<Token![/]>()?;
    let expected = match open_tag {
        Tag::Slot(name) => {
            input.parse::<Token![.]>()?;
            name.to_string()
        }
        Tag::Component(p) => p
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default(),
        _ => tag_name(open_tag),
    };
    let close_ident = input.call(Ident::parse_any)?;
    if close_ident != expected.as_str() {
        return Err(syn::Error::new(
            close_ident.span(),
            format!("expected closing tag `{expected}`, found `{close_ident}`"),
        ));
    }
    input.parse::<Token![>]>()?;
    Ok(())
}

impl Parse for Attr {
    fn parse(input: ParseStream) -> Result<Self> {
        // @when {condition} { attr1 attr2=val ... }
        if input.peek(Token![@]) {
            input.parse::<Token![@]>()?;
            let kw: Ident = input.parse()?;
            if kw != "when" {
                return Err(syn::Error::new(kw.span(), "expected `when` after `@`"));
            }
            let cond_content;
            braced!(cond_content in input);
            let cond: Expr = cond_content.parse()?;
            let attrs_content;
            braced!(attrs_content in input);
            let mut attrs = Vec::new();
            while !attrs_content.is_empty() {
                attrs.push(attrs_content.parse::<Attr>()?);
            }
            return Ok(Attr::When(cond, attrs));
        }

        let name: Ident = input.parse()?;
        if name == "class" {
            input.parse::<Token![=]>()?;
            let lit: LitStr = input.parse()?;
            Ok(Attr::Class(lit))
        } else if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            if input.peek(syn::token::Brace) {
                let content;
                braced!(content in input);
                if content.peek(Token![if]) {
                    let if_expr: ExprIf = content.parse()?;
                    Ok(Attr::IfAttr(name, if_expr))
                } else if content.peek(Token![@]) {
                    content.parse::<Token![@]>()?;
                    let expr: Expr = content.parse()?;
                    Ok(Attr::ReactiveKeyValue(name, expr))
                } else {
                    let expr: Expr = content.parse()?;
                    Ok(Attr::KeyValue(name, expr))
                }
            } else if input.peek(LitStr) {
                let lit: LitStr = input.parse()?;
                let expr: Expr = syn::parse_quote!(#lit);
                Ok(Attr::KeyValue(name, expr))
            } else {
                let expr: Expr = input.parse()?;
                Ok(Attr::KeyValue(name, expr))
            }
        } else {
            Ok(Attr::Flag(name))
        }
    }
}

// ---------------------------------------------------------------------------
// Code generation
// ---------------------------------------------------------------------------

struct EmitCtx {
    scale: Option<Ident>,
    /// Errors found while emitting; any error replaces the expansion.
    errors: RefCell<Option<syn::Error>>,
}

enum ChildMode {
    Child(TokenStream2),
    Optional(TokenStream2),
    Spread(TokenStream2),
}

const SPATIAL_ATTRS: &[&str] = &[
    "gap", "gap_x", "gap_y", "p", "px", "py", "pt", "pb", "pl", "pr", "rounded",
];

impl EmitCtx {
    fn error(&self, span: Span, message: impl std::fmt::Display) {
        let err = syn::Error::new(span, message);
        let mut errors = self.errors.borrow_mut();
        match errors.as_mut() {
            Some(existing) => existing.combine(err),
            None => *errors = Some(err),
        }
    }

    fn emit_node(&self, node: &Node) -> ChildMode {
        match node {
            Node::Element(el) => match &el.tag {
                Tag::Fragment => {
                    self.reject_attrs(el, "fragment");
                    ChildMode::Spread(self.emit_children_spread(&el.children))
                }
                Tag::Slot(name) => {
                    self.error(
                        name.span(),
                        format!("slot `<.{name}>` must be a direct child of a component"),
                    );
                    ChildMode::Child(quote! { () })
                }
                _ => ChildMode::Child(self.emit_element(el)),
            },
            Node::Expr(expr) => ChildMode::Child(quote! { #expr }),
            Node::OptionalExpr(expr) => ChildMode::Optional(quote! { #expr }),
            Node::SpreadExpr(expr) => ChildMode::Spread(quote! { #expr }),
            Node::Text(lit) => ChildMode::Child(quote! { #lit }),
            Node::IfChain(chain) => self.emit_if_chain(chain),
            Node::ForLoop(fl) => ChildMode::Spread(self.emit_for_loop(fl)),
            Node::MatchExpr(m) => ChildMode::Child(self.emit_match(m)),
        }
    }

    fn reject_attrs(&self, el: &ElementNode, tag: &str) {
        for attr in &el.attrs {
            self.error(attr.span(), format!("`<{tag}>` takes no attributes"));
        }
    }

    /// Lower `if` / `else if` / `else` to one Rust `if` chain. The chain's
    /// mode is the widest any branch needs: `Spread` when a branch holds
    /// zero or several children or spreads (each branch becomes a `Vec`,
    /// so children flow into the parent with no wrapper node), `Optional`
    /// when a branch is optional or there is no final `else` (a missing
    /// branch yields no child), and `Child` otherwise.
    fn emit_if_chain(&self, chain: &IfChainNode) -> ChildMode {
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

        let singles: Vec<Option<ChildMode>> = bodies
            .iter()
            .map(|body| match body {
                [only] => match self.emit_node(only) {
                    ChildMode::Spread(_) => None,
                    mode => Some(mode),
                },
                _ => None,
            })
            .collect();

        let branches: Vec<TokenStream2>;
        let missing: TokenStream2;
        let wrap: fn(TokenStream2) -> ChildMode;
        if singles.iter().any(Option::is_none) {
            branches = bodies
                .iter()
                .map(|body| self.emit_children_spread(body))
                .collect();
            missing = quote! { ::std::vec::Vec::new() };
            wrap = ChildMode::Spread;
        } else if else_body.is_none()
            || singles
                .iter()
                .any(|m| matches!(m, Some(ChildMode::Optional(_))))
        {
            branches = singles
                .into_iter()
                .map(|mode| match mode {
                    Some(ChildMode::Child(t)) => quote! { ::core::option::Option::Some(#t) },
                    Some(ChildMode::Optional(t)) => t,
                    _ => unreachable!("spread branches take the Vec path"),
                })
                .collect();
            missing = quote! { ::core::option::Option::None };
            wrap = ChildMode::Optional;
        } else {
            branches = singles
                .into_iter()
                .map(|mode| match mode {
                    Some(ChildMode::Child(t)) => t,
                    _ => unreachable!("only plain children reach the Child path"),
                })
                .collect();
            missing = TokenStream2::new();
            wrap = ChildMode::Child;
        }

        let mut branches = branches.into_iter();
        let mut tokens = TokenStream2::new();
        for (i, cond) in conds.iter().enumerate() {
            let body = branches.next().expect("one branch per condition");
            if i > 0 {
                tokens.extend(quote! { else });
            }
            tokens.extend(quote! { if #cond { #body } });
        }
        let tail = branches.next().unwrap_or(missing);
        if !tail.is_empty() {
            tokens.extend(quote! { else { #tail } });
        }
        wrap(tokens)
    }

    fn emit_for_loop(&self, fl: &ForLoopNode) -> TokenStream2 {
        let pat = &fl.pat;
        let iter = &fl.iter;
        let body = self.emit_children_spread(&fl.body_children);
        quote! {
            (#iter)
                .into_iter()
                .flat_map(|#pat| #body)
                .collect::<Vec<_>>()
        }
    }

    fn emit_match(&self, m: &MatchNode) -> TokenStream2 {
        let expr = &m.expr;
        quote! { #expr }
    }

    fn emit_children_spread(&self, children: &[Node]) -> TokenStream2 {
        let mut stmts = Vec::new();
        for child in children {
            let stmt = match self.emit_node(child) {
                ChildMode::Child(tokens) => quote! {
                    __quark_children.push(#tokens);
                },
                ChildMode::Optional(tokens) => quote! {
                    if let Some(__quark_child) = (#tokens) {
                        __quark_children.push(__quark_child);
                    }
                },
                ChildMode::Spread(tokens) => quote! {
                    __quark_children.extend(#tokens);
                },
            };
            stmts.push(stmt);
        }

        quote! {{
            let mut __quark_children = Vec::new();
            #(#stmts)*
            __quark_children
        }}
    }

    fn emit_element(&self, el: &ElementNode) -> TokenStream2 {
        match &el.tag {
            Tag::Div => self.emit_div(el),
            Tag::Text => self.emit_text(el),
            Tag::Icon => self.emit_icon(el),
            Tag::Spacer => {
                self.reject_attrs(el, "spacer");
                self.reject_children(el, "`<spacer>` takes no children");
                quote! { spacer().into_any() }
            }
            Tag::Fragment | Tag::Slot(_) => unreachable!("handled in emit_node"),
            Tag::Component(path) => self.emit_component(path, el),
        }
    }

    fn reject_children(&self, el: &ElementNode, message: &str) {
        for child in &el.children {
            self.error(node_span(child), message);
        }
    }

    fn emit_div(&self, el: &ElementNode) -> TokenStream2 {
        let mut chain = quote! { div() };

        for attr in &el.attrs {
            chain = self.emit_styled_attr(chain, attr);
        }

        for child in &el.children {
            chain = self.append_child(chain, child);
        }

        quote! { #chain.into_any() }
    }

    fn append_child(&self, chain: TokenStream2, child: &Node) -> TokenStream2 {
        match self.emit_node(child) {
            ChildMode::Child(tokens) => quote! { #chain.child(#tokens) },
            ChildMode::Optional(tokens) => quote! { #chain.optional_child(#tokens) },
            ChildMode::Spread(tokens) => quote! { #chain.children(#tokens) },
        }
    }

    fn emit_text(&self, el: &ElementNode) -> TokenStream2 {
        let content = match el.children.as_slice() {
            [] => quote! { "" },
            [Node::Expr(e), rest @ ..] => {
                self.reject_extra_text(rest);
                quote! { #e }
            }
            [Node::Text(lit), rest @ ..] => {
                self.reject_extra_text(rest);
                quote! { #lit }
            }
            [first, ..] => {
                self.error(
                    node_span(first),
                    "`<text>` content must be a \"string\" or a {expression}",
                );
                quote! { "" }
            }
        };

        let mut chain = quote! { text(#content) };
        for attr in &el.attrs {
            chain = self.emit_plain_attr(chain, attr);
        }

        quote! { #chain.into_any() }
    }

    fn reject_extra_text(&self, rest: &[Node]) {
        for child in rest {
            self.error(
                node_span(child),
                "`<text>` takes one child; join the pieces with format!()",
            );
        }
    }

    fn emit_icon(&self, el: &ElementNode) -> TokenStream2 {
        self.reject_children(el, "`<icon>` takes no children; pass `svg={...}`");
        let mut svg = quote! { "" };
        let mut size = quote! { 16.0 };
        let mut extra = Vec::new();
        for attr in &el.attrs {
            match attr {
                Attr::KeyValue(name, expr) if name == "svg" => svg = quote! { #expr },
                Attr::KeyValue(name, expr) if name == "size" => size = quote! { #expr },
                Attr::ReactiveKeyValue(name, expr) if name == "svg" => {
                    svg = quote! { cx.read(#expr) };
                }
                Attr::ReactiveKeyValue(name, expr) if name == "size" => {
                    size = quote! { cx.read(#expr) };
                }
                other => extra.push(other),
            }
        }

        let mut chain = quote! { svg_icon(#svg, #size) };
        for attr in extra {
            chain = self.emit_plain_attr(chain, attr);
        }
        quote! { #chain.into_any() }
    }

    fn emit_component(&self, path: &syn::Path, el: &ElementNode) -> TokenStream2 {
        let args = el.ctor_args.iter().flatten();
        let mut chain = quote! { #path::new(#(#args),*) };

        for attr in &el.attrs {
            chain = self.emit_plain_attr(chain, attr);
        }

        for child in &el.children {
            chain = match child {
                Node::Element(ElementNode {
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
                        self.error(method.span(), format!("slot `<.{method}>` has no children"));
                    }
                    self.emit_slot(chain, method, children)
                }
                _ => self.append_child(chain, child),
            };
        }

        quote! { #chain.into_any() }
    }

    /// Call `.method(child)` once per child of a `<.method>` slot.
    fn emit_slot(
        &self,
        mut chain: TokenStream2,
        method: &Ident,
        children: &[Node],
    ) -> TokenStream2 {
        for child in children {
            chain = match self.emit_node(child) {
                ChildMode::Child(tokens) => quote! { #chain.#method(#tokens) },
                ChildMode::Optional(tokens) => quote! {{
                    let __quark_slot = #chain;
                    if let Some(__quark_child) = (#tokens) {
                        __quark_slot.#method(__quark_child)
                    } else {
                        __quark_slot
                    }
                }},
                ChildMode::Spread(tokens) => quote! {
                    (#tokens)
                        .into_iter()
                        .fold(#chain, |__quark_slot, __quark_child| {
                            __quark_slot.#method(__quark_child)
                        })
                },
            };
        }
        chain
    }

    // -----------------------------------------------------------------------
    // class="..." expansion
    // -----------------------------------------------------------------------

    fn class_to_calls(&self, lit: &LitStr) -> Vec<TokenStream2> {
        lit.value()
            .split_whitespace()
            .filter_map(|class| self.class_token_to_call(class, lit.span()))
            .collect()
    }

    fn class_token_to_call(&self, class: &str, span: Span) -> Option<TokenStream2> {
        let mapped = match class {
            "shrink-0" => "flex_shrink_0",
            "grow" => "flex_grow",
            "grow-0" => return Some(quote_spanned! { span=> .flex_grow_val(0.0) }),
            "font-bold" => "bold",
            "font-semibold" => "semibold",
            "font-medium" => "medium",
            "font-mono" => "mono",
            other => &other.replace('-', "_"),
        };
        // `syn::parse_str` rejects keywords and non-identifier characters
        // (`w-1/2`) instead of panicking like `Ident::new`.
        match syn::parse_str::<Ident>(mapped) {
            Ok(mut method) => {
                method.set_span(span);
                Some(quote! { .#method() })
            }
            Err(_) => {
                self.error(
                    span,
                    format!("class `{class}` does not name a builder method"),
                );
                None
            }
        }
    }

    // -----------------------------------------------------------------------
    // Attribute emission with auto-scale
    // -----------------------------------------------------------------------

    fn emit_styled_attr(&self, chain: TokenStream2, attr: &Attr) -> TokenStream2 {
        match attr {
            Attr::KeyValue(name, expr) => {
                if let Expr::Tuple(tup) = expr {
                    let elems = &tup.elems;
                    quote! { #chain.#name(#elems) }
                } else if self.should_autoscale(name) {
                    let scale = self.scale.as_ref().unwrap();
                    quote! { #chain.#name((#expr * #scale).round()) }
                } else {
                    quote! { #chain.#name(#expr) }
                }
            }
            Attr::ReactiveKeyValue(name, expr) if self.should_autoscale(name) => {
                let scale = self.scale.as_ref().unwrap();
                quote! { #chain.#name((cx.read(#expr) * #scale).round()) }
            }
            Attr::When(cond, attrs) => {
                let mut inner = quote! { __w };
                for a in attrs {
                    inner = self.emit_styled_attr(inner, a);
                }
                quote! { { let __w = #chain; if #cond { #inner } else { __w } } }
            }
            _ => self.emit_plain_attr(chain, attr),
        }
    }

    fn emit_if_attr(&self, chain: TokenStream2, name: &Ident, if_expr: &ExprIf) -> TokenStream2 {
        let cond = &if_expr.cond;
        let then_stmts = &if_expr.then_branch.stmts;
        let then_val = if then_stmts.len() == 1 {
            let stmt = &then_stmts[0];
            quote! { #stmt }
        } else {
            quote! { { #(#then_stmts)* } }
        };

        let scaled_then = if self.should_autoscale(name) {
            let scale = self.scale.as_ref().unwrap();
            quote! { (#then_val * #scale).round() }
        } else {
            then_val.clone()
        };

        match &if_expr.else_branch {
            Some((_, else_expr)) => {
                let unwrapped_else = unwrap_block_expr(else_expr);
                let scaled_else = if self.should_autoscale(name) {
                    let scale = self.scale.as_ref().unwrap();
                    quote! { (#unwrapped_else * #scale).round() }
                } else {
                    quote! { #unwrapped_else }
                };
                quote! { #chain.#name(if #cond { #scaled_then } else { #scaled_else }) }
            }
            None => {
                quote! { { let __t = #chain; if #cond { __t.#name(#scaled_then) } else { __t } } }
            }
        }
    }

    /// Builder calls with no auto-scaling or tuple splatting: text, icon,
    /// and component attributes.
    fn emit_plain_attr(&self, chain: TokenStream2, attr: &Attr) -> TokenStream2 {
        match attr {
            Attr::Flag(name) => quote! { #chain.#name() },
            Attr::KeyValue(name, expr) => quote! { #chain.#name(#expr) },
            Attr::ReactiveKeyValue(name, expr) => quote! { #chain.#name(cx.read(#expr)) },
            Attr::Class(lit) => {
                let calls = self.class_to_calls(lit);
                quote! { #chain #(#calls)* }
            }
            Attr::IfAttr(name, if_expr) => self.emit_if_attr(chain, name, if_expr),
            Attr::When(cond, attrs) => {
                let mut inner = quote! { __w };
                for a in attrs {
                    inner = self.emit_plain_attr(inner, a);
                }
                quote! { { let __w = #chain; if #cond { #inner } else { __w } } }
            }
        }
    }

    fn should_autoscale(&self, name: &Ident) -> bool {
        if self.scale.is_none() {
            return false;
        }
        let s = name.to_string();
        SPATIAL_ATTRS.contains(&s.as_str())
    }
}

fn unwrap_block_expr(expr: &Expr) -> TokenStream2 {
    if let Expr::Block(block) = expr
        && block.block.stmts.len() == 1
    {
        let stmt = &block.block.stmts[0];
        return quote! { #stmt };
    }
    quote! { #expr }
}

fn tag_name(tag: &Tag) -> String {
    match tag {
        Tag::Div => "div".into(),
        Tag::Text => "text".into(),
        Tag::Icon => "icon".into(),
        Tag::Spacer => "spacer".into(),
        Tag::Fragment => "fragment".into(),
        Tag::Component(path) => quote!(#path).to_string(),
        Tag::Slot(name) => format!(".{name}"),
    }
}

fn node_span(node: &Node) -> Span {
    match node {
        Node::Element(el) => match &el.tag {
            Tag::Component(path) => path.span(),
            Tag::Slot(name) => name.span(),
            _ => el
                .attrs
                .first()
                .map(Attr::span)
                .unwrap_or_else(Span::call_site),
        },
        Node::Expr(e) | Node::OptionalExpr(e) | Node::SpreadExpr(e) => e.span(),
        Node::Text(lit) => lit.span(),
        Node::IfChain(chain) => chain.cond.span(),
        Node::ForLoop(fl) => fl.iter.span(),
        Node::MatchExpr(m) => m.expr.span(),
    }
}
