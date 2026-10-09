//! `view!` syntax. Errors here are syntax errors; anything that parses but
//! cannot be lowered is reported by `emit.rs`.

use proc_macro2::{TokenStream as TokenStream2, TokenTree};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprIf, Ident, LitStr, Pat, Result, Token, braced, parenthesized};

use crate::ast::*;

impl Parse for ViewInput {
    fn parse(input: ParseStream) -> Result<Self> {
        let scale = if input.peek(Ident::peek_any) && input.peek2(Token![,]) {
            let ident: Ident = input.parse()?;
            input.parse::<Token![,]>()?;
            Some(ident)
        } else {
            None
        };
        let typed = if input.peek(Token![->]) {
            input.parse::<Token![->]>()?;
            let ty: syn::Type = input.parse()?;
            input.parse::<Token![,]>()?;
            Some(ty)
        } else {
            None
        };
        let root: Node = input.parse()?;
        if !input.is_empty() {
            return Err(
                input.error("view! takes one root node; wrap siblings in a fragment: `<>...</>`")
            );
        }
        Ok(ViewInput { scale, typed, root })
    }
}

impl Parse for Node {
    fn parse(input: ParseStream) -> Result<Self> {
        if input.peek(Token![<]) {
            if input.peek2(Token![>]) {
                return parse_fragment(input);
            }
            return Ok(Node::Element(input.parse()?));
        }
        if input.peek(Token![if]) {
            return Ok(Node::If(input.parse()?));
        }
        if input.peek(Token![for]) {
            return Ok(Node::For(Box::new(input.parse()?)));
        }
        if input.peek(Token![match]) {
            return Ok(Node::Match(input.parse()?));
        }
        if input.peek(Token![let]) {
            return match input.parse::<syn::Stmt>()? {
                syn::Stmt::Local(local) => Ok(Node::Let(Box::new(local))),
                other => Err(syn::Error::new_spanned(
                    other,
                    "expected `let pattern = value;`",
                )),
            };
        }
        if input.peek(syn::token::Brace) {
            let content;
            braced!(content in input);
            if content.peek(Token![?]) {
                content.parse::<Token![?]>()?;
                return Ok(Node::OptionalExpr(content.parse()?));
            }
            if content.peek(Token![...]) {
                content.parse::<Token![...]>()?;
                return Ok(Node::SpreadExpr(content.parse()?));
            }
            return Ok(Node::Expr(content.parse()?));
        }
        if input.peek(LitStr) {
            return Ok(Node::Text(input.parse()?));
        }
        // Rust's lexer drops whitespace and rejects stray quotes, so
        // unquoted text cannot round-trip; ask for a string literal.
        let stray: TokenTree = input.parse()?;
        Err(syn::Error::new(
            stray.span(),
            format!(
                "unexpected `{stray}`: text must be a string literal, as in \
                 `\"{stray}\"`; a child is <element>, \"text\", {{expr}}, if, for, or match"
            ),
        ))
    }
}

fn parse_fragment(input: ParseStream) -> Result<Node> {
    input.parse::<Token![<]>()?;
    input.parse::<Token![>]>()?;
    let mut children = Vec::new();
    while !is_closing_tag(input) {
        if input.is_empty() {
            return Err(input.error("unclosed fragment: expected `</>`"));
        }
        children.push(input.parse()?);
    }
    input.parse::<Token![<]>()?;
    input.parse::<Token![/]>()?;
    if !input.peek(Token![>]) {
        let close: TokenTree = input.parse()?;
        return Err(syn::Error::new(
            close.span(),
            format!("expected closing tag `</>`, found `</{close}>`"),
        ));
    }
    input.parse::<Token![>]>()?;
    Ok(Node::Fragment(children))
}

impl Parse for IfNode {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Token![if]>()?;
        // Handles `if let` and let chains too: syn parses `let` as an
        // expression in this position.
        let cond = Expr::parse_without_eager_brace(input)?;
        let then_children = parse_braced_children(input)?;
        let (else_if, else_children) = if input.peek(Token![else]) {
            input.parse::<Token![else]>()?;
            if input.peek(Token![if]) {
                (Some(Box::new(input.parse()?)), None)
            } else {
                (None, Some(parse_braced_children(input)?))
            }
        } else {
            (None, None)
        };
        Ok(IfNode {
            cond,
            then_children,
            else_if,
            else_children,
        })
    }
}

impl Parse for ForNode {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Token![for]>()?;
        let pat = Pat::parse_multi_with_leading_vert(input)?;
        input.parse::<Token![in]>()?;
        let iter = Expr::parse_without_eager_brace(input)?;
        let key = if input.peek(Ident) && input.peek2(Token![=]) {
            let name: Ident = input.parse()?;
            if name != "key" {
                return Err(syn::Error::new(
                    name.span(),
                    format!("expected `key={{..}}` or the loop body, found `{name}`"),
                ));
            }
            input.parse::<Token![=]>()?;
            let content;
            braced!(content in input);
            Some((name, content.parse()?))
        } else {
            None
        };
        let body = parse_braced_children(input)?;
        Ok(ForNode {
            pat,
            iter,
            key,
            body,
        })
    }
}

impl Parse for MatchNode {
    fn parse(input: ParseStream) -> Result<Self> {
        let match_token = input.parse()?;
        let scrutinee = Expr::parse_without_eager_brace(input)?;
        let content;
        braced!(content in input);
        let mut arms = Vec::new();
        while !content.is_empty() {
            let pat = Pat::parse_multi_with_leading_vert(&content)?;
            let guard = if content.peek(Token![if]) {
                content.parse::<Token![if]>()?;
                Some(content.parse()?)
            } else {
                None
            };
            content.parse::<Token![=>]>()?;
            // Markup and `{ children }` bodies need no comma; a plain Rust
            // expression ends at one.
            let body = if content.peek(Token![<]) {
                vec![content.parse()?]
            } else if content.peek(syn::token::Brace) {
                parse_braced_children(&content)?
            } else {
                let expr: Expr = content.parse()?;
                if !content.is_empty() {
                    content.parse::<Token![,]>()?;
                }
                vec![Node::Expr(expr)]
            };
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
            arms.push(MatchArm { pat, guard, body });
        }
        Ok(MatchNode {
            match_token,
            scrutinee,
            arms,
        })
    }
}

fn parse_braced_children(input: ParseStream) -> Result<Vec<Node>> {
    let content;
    braced!(content in input);
    let mut children = Vec::new();
    while !content.is_empty() {
        children.push(content.parse()?);
    }
    Ok(children)
}

impl Parse for Element {
    fn parse(input: ParseStream) -> Result<Self> {
        input.parse::<Token![<]>()?;
        let (mut tag, span) = parse_tag(input)?;

        let ctor_args = if input.peek(syn::token::Paren) {
            // A lowercase name with arguments is a function call:
            // `<canvas(paint)>`, `<widgets::panel(theme)>`.
            tag = match tag {
                Tag::Builtin(name) => Tag::Function(syn::Path::from(name)),
                Tag::Component(path)
                    if path.segments.last().is_some_and(|s| {
                        s.ident
                            .to_string()
                            .starts_with(|c: char| c.is_ascii_lowercase())
                    }) =>
                {
                    Tag::Function(path)
                }
                Tag::Slot(_) => {
                    return Err(input.error("slot tags take no arguments"));
                }
                other => other,
            };
            let content;
            parenthesized!(content in input);
            Some(content.parse_terminated(Expr::parse, Token![,])?)
        } else {
            None
        };

        let mut attrs = Vec::new();
        while !input.peek(Token![>]) && !(input.peek(Token![/]) && input.peek2(Token![>])) {
            if input.is_empty() {
                return Err(syn::Error::new(span, "unclosed tag: expected `>` or `/>`"));
            }
            attrs.push(input.parse()?);
        }

        if input.peek(Token![/]) {
            input.parse::<Token![/]>()?;
            input.parse::<Token![>]>()?;
            return Ok(Element {
                tag,
                span,
                ctor_args,
                attrs,
                children: Vec::new(),
            });
        }
        input.parse::<Token![>]>()?;

        let mut children = Vec::new();
        while !is_closing_tag(input) {
            if input.is_empty() {
                return Err(syn::Error::new(
                    span,
                    format!(
                        "unclosed `<{}>`: expected `</{}>`",
                        tag_name(&tag),
                        tag_name(&tag)
                    ),
                ));
            }
            children.push(input.parse()?);
        }
        parse_closing_tag(input, &tag)?;
        Ok(Element {
            tag,
            span,
            ctor_args,
            attrs,
            children,
        })
    }
}

fn parse_tag(input: ParseStream) -> Result<(Tag, proc_macro2::Span)> {
    if input.peek(syn::token::Brace) {
        let content;
        let brace = braced!(content in input);
        let expr: Expr = content.parse()?;
        return Ok((Tag::Value(Box::new(expr)), brace.span.join()));
    }
    if input.peek(Token![.]) {
        input.parse::<Token![.]>()?;
        let name = input.call(Ident::parse_any)?;
        let span = name.span();
        return Ok((Tag::Slot(name), span));
    }
    let first = input.call(Ident::parse_any)?;
    let span = first.span();
    if !input.peek(Token![::])
        && first
            .to_string()
            .starts_with(|c: char| c.is_ascii_lowercase())
    {
        return Ok((Tag::Builtin(first), span));
    }
    let mut path = syn::Path::from(first);
    while input.peek(Token![::]) {
        input.parse::<Token![::]>()?;
        let seg: Ident = input.parse()?;
        path.segments.push(seg.into());
    }
    Ok((Tag::Component(path), span))
}

pub fn tag_name(tag: &Tag) -> String {
    match tag {
        Tag::Builtin(name) => name.to_string(),
        Tag::Component(path) | Tag::Function(path) => path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect::<Vec<_>>()
            .join("::"),
        Tag::Slot(name) => format!(".{name}"),
        Tag::Value(_) => "{..}".to_owned(),
    }
}

fn is_closing_tag(input: ParseStream) -> bool {
    input.peek(Token![<]) && input.peek2(Token![/])
}

fn parse_closing_tag(input: ParseStream, open: &Tag) -> Result<()> {
    input.parse::<Token![<]>()?;
    input.parse::<Token![/]>()?;
    let expected = tag_name(open);
    if matches!(open, Tag::Value(_)) {
        if !input.peek(Token![>]) {
            return Err(input.error("an expression tag `<{..}>` closes with `</>`"));
        }
        input.parse::<Token![>]>()?;
        return Ok(());
    }
    if input.peek(Token![>]) {
        return Err(input.error(format!("expected closing tag `</{expected}>`, found `</>`")));
    }
    let mut found = String::new();
    let start = input.span();
    if input.peek(Token![.]) {
        input.parse::<Token![.]>()?;
        found.push('.');
    }
    found.push_str(&input.call(Ident::parse_any)?.to_string());
    while input.peek(Token![::]) {
        input.parse::<Token![::]>()?;
        found.push_str("::");
        found.push_str(&input.call(Ident::parse_any)?.to_string());
    }
    // `</Button>` closes `<widgets::Button>`: JSX compares the whole name,
    // but repeating a module path in the closing tag is noise.
    let matches = found == expected
        || matches!(open, Tag::Component(p) | Tag::Function(p) if p.segments.last().is_some_and(|s| s.ident == found));
    if !matches {
        return Err(syn::Error::new(
            start,
            format!("expected closing tag `</{expected}>`, found `</{found}>`"),
        ));
    }
    input.parse::<Token![>]>()?;
    Ok(())
}

impl Parse for Attr {
    fn parse(input: ParseStream) -> Result<Self> {
        // @when {condition} { attr1 attr2=val ... }
        // @for pat in iter { attr1 attr2=val ... }
        if input.peek(Token![@]) {
            input.parse::<Token![@]>()?;
            if input.peek(Token![for]) {
                input.parse::<Token![for]>()?;
                let pat = Pat::parse_multi_with_leading_vert(input)?;
                input.parse::<Token![in]>()?;
                let iter = Expr::parse_without_eager_brace(input)?;
                let attrs_content;
                braced!(attrs_content in input);
                let mut attrs = Vec::new();
                while !attrs_content.is_empty() {
                    attrs.push(attrs_content.parse()?);
                }
                return Ok(Attr::For(Box::new(pat), iter, attrs));
            }
            let kw: Ident = input.parse()?;
            if kw != "when" {
                return Err(syn::Error::new(
                    kw.span(),
                    "expected `when` or `for` after `@`",
                ));
            }
            let cond_content;
            braced!(cond_content in input);
            let cond: Expr = cond_content.parse()?;
            let attrs_content;
            braced!(attrs_content in input);
            let mut attrs = Vec::new();
            while !attrs_content.is_empty() {
                attrs.push(attrs_content.parse()?);
            }
            return Ok(Attr::When(cond, attrs));
        }

        let name = parse_attr_name(input)?;
        if name.written == "class" {
            input.parse::<Token![=]>()?;
            if !input.peek(LitStr) {
                return Err(input.error(
                    "`class` takes a string literal of class names, as in `class=\"flex-row gap-2\"`; \
                     use attributes or `@when` for computed values",
                ));
            }
            return Ok(Attr::Class(input.parse()?));
        }
        let value = if input.peek(Token![=]) {
            input.parse::<Token![=]>()?;
            parse_attr_value(input)?
        } else {
            AttrValue::Flag
        };
        Ok(Attr::Method { name, value })
    }
}

fn parse_attr_name(input: ParseStream) -> Result<AttrName> {
    let first = input.call(Ident::parse_any)?;
    if first == "on" && input.peek(Token![:]) && !input.peek(Token![::]) {
        input.parse::<Token![:]>()?;
        let event = input.call(Ident::parse_any)?;
        let mut written = format!("on:{event}");
        let binding = if input.peek(Token![:]) && !input.peek(Token![::]) {
            input.parse::<Token![:]>()?;
            let (text, span) = parse_binding(input)?;
            written.push(':');
            written.push_str(&text);
            Some(LitStr::new(&text, span))
        } else {
            None
        };
        return Ok(AttrName {
            written,
            first,
            segments: vec![event.clone()],
            event: Some(EventName {
                name: event,
                binding,
            }),
        });
    }
    let mut written = first.to_string();
    let mut segments = vec![first.clone()];
    while input.peek(Token![-]) && input.peek2(Ident::peek_any) {
        input.parse::<Token![-]>()?;
        let seg = input.call(Ident::parse_any)?;
        written.push('-');
        written.push_str(&seg.to_string());
        segments.push(seg);
    }
    Ok(AttrName {
        written,
        first,
        segments,
        event: None,
    })
}

/// The tokens of `mod+s` in `on:key:mod+s={..}`, up to the `=`, joined
/// without spaces (the lexer has already dropped them).
fn parse_binding(input: ParseStream) -> Result<(String, proc_macro2::Span)> {
    let span = input.span();
    let mut tokens = TokenStream2::new();
    while !input.is_empty() && !input.peek(Token![=]) {
        tokens.extend([input.parse::<TokenTree>()?]);
    }
    let text: String = tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Literal(lit) => {
                let s = lit.to_string();
                s.trim_matches('"').to_owned()
            }
            other => other.to_string(),
        })
        .collect();
    if text.is_empty() {
        return Err(syn::Error::new(
            span,
            "expected a key binding after `on:key:`, as in `on:key:mod+s`",
        ));
    }
    Ok((text, span))
}

fn parse_attr_value(input: ParseStream) -> Result<AttrValue> {
    if input.peek(syn::token::Brace) {
        let content;
        braced!(content in input);
        if content.peek(Token![if]) {
            let if_expr: ExprIf = content.parse()?;
            return Ok(AttrValue::If(if_expr));
        }
        if content.peek(Token![@]) {
            content.parse::<Token![@]>()?;
            return Ok(AttrValue::Reactive(content.parse()?));
        }
        return Ok(AttrValue::Expr(content.parse()?));
    }
    if input.peek(syn::Lit) {
        let lit: syn::Lit = input.parse()?;
        return Ok(AttrValue::Expr(syn::parse_quote!(#lit)));
    }
    if input.peek(Token![-]) && input.peek2(syn::Lit) {
        let minus: Token![-] = input.parse()?;
        let lit: syn::Lit = input.parse()?;
        return Ok(AttrValue::Expr(syn::parse_quote!(#minus #lit)));
    }
    let found: TokenTree = input.parse()?;
    Err(syn::Error::new(
        found.span(),
        format!("attribute values are a literal or a braced expression: write `{{{found}}}`"),
    ))
}
