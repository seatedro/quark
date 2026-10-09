//! `view!` syntax. Errors here are syntax errors; anything that parses but
//! cannot be lowered is reported by `quark-macros`' emitter.
//!
//! Every function takes a [`Recorder`]. The macro passes one that is off;
//! [`parse_with_syntax`] passes one that builds the [`SyntaxTree`]. Both
//! run the same functions, so the tree cannot drift from the grammar:
//! record each token right after parsing it, and close nodes in the order
//! their tokens appear.

use proc_macro2::{Delimiter, TokenStream as TokenStream2, TokenTree};
use syn::ext::IdentExt;
use syn::parse::{Parse, ParseStream};
use syn::{Expr, ExprIf, Ident, LitStr, Pat, Result, Token, braced, parenthesized};

use crate::ast::*;
use crate::syntax::{
    AttrForm, ElementForm, ExprMarker, NodeKind, Recorder, RustContext, SpanRange, SyntaxTree,
    TagKind, TokenKind,
};

const BRACE_OPEN: TokenKind = TokenKind::Open(Delimiter::Brace);
const BRACE_CLOSE: TokenKind = TokenKind::Close(Delimiter::Brace);

impl Parse for ViewInput {
    fn parse(input: ParseStream) -> Result<Self> {
        view_input(input, &mut Recorder::off())
    }
}

/// Parses `view!` input and records its concrete syntax. Use it with
/// `syn::parse::Parser`, as in `parse_with_syntax.parse2(tokens)`.
pub fn parse_with_syntax(input: ParseStream) -> Result<(ViewInput, SyntaxTree)> {
    let mut rec = Recorder::on();
    let view = view_input(input, &mut rec)?;
    let tree = rec.into_tree().expect("recorder is on");
    debug_assert_eq!(tree.verify_integrity(), Ok(()));
    Ok((view, tree))
}

fn punct(rec: &mut Recorder, text: &'static str, spans: &[proc_macro2::Span]) {
    rec.tokens(
        TokenKind::Punct(text),
        SpanRange {
            first: spans[0],
            last: spans[spans.len() - 1],
        },
    );
}

/// Records the Rust parsed by `parse`, from where it started to where it
/// stopped.
fn rust<T>(
    input: ParseStream,
    rec: &mut Recorder,
    context: RustContext,
    parse: impl FnOnce(ParseStream) -> Result<T>,
) -> Result<T> {
    let begin = input.cursor();
    let value = parse(input)?;
    rec.rust(context, begin, input.cursor());
    Ok(value)
}

fn view_input(input: ParseStream, rec: &mut Recorder) -> Result<ViewInput> {
    rec.start(NodeKind::View);
    let scale = if input.peek(Ident::peek_any) && input.peek2(Token![,]) {
        rec.start(NodeKind::ScaleHeader);
        let ident: Ident = input.parse()?;
        rec.token(TokenKind::Ident, ident.span());
        let comma = input.parse::<Token![,]>()?;
        punct(rec, ",", &comma.spans);
        rec.finish();
        Some(ident)
    } else {
        None
    };
    let typed = if input.peek(Token![->]) {
        rec.start(NodeKind::TypedHeader);
        let arrow = input.parse::<Token![->]>()?;
        punct(rec, "->", &arrow.spans);
        let ty: syn::Type = rust(input, rec, RustContext::Type, |i| i.parse())?;
        let comma = input.parse::<Token![,]>()?;
        punct(rec, ",", &comma.spans);
        rec.finish();
        Some(ty)
    } else {
        None
    };
    let root = node(input, rec)?;
    if !input.is_empty() {
        return Err(
            input.error("view! takes one root node; wrap siblings in a fragment: `<>...</>`")
        );
    }
    rec.finish();
    Ok(ViewInput { scale, typed, root })
}

fn node(input: ParseStream, rec: &mut Recorder) -> Result<Node> {
    if input.peek(Token![<]) {
        if input.peek2(Token![>]) {
            return parse_fragment(input, rec);
        }
        return Ok(Node::Element(element(input, rec)?));
    }
    if input.peek(Token![if]) {
        return Ok(Node::If(if_node(input, rec)?));
    }
    if input.peek(Token![for]) {
        return Ok(Node::For(Box::new(for_node(input, rec)?)));
    }
    if input.peek(Token![match]) {
        return Ok(Node::Match(match_node(input, rec)?));
    }
    if input.peek(Token![let]) {
        rec.start(NodeKind::Let);
        let stmt = rust(input, rec, RustContext::Local, |i| i.parse::<syn::Stmt>())?;
        rec.finish();
        return match stmt {
            syn::Stmt::Local(local) => Ok(Node::Let(Box::new(local))),
            other => Err(syn::Error::new_spanned(
                other,
                "expected `let pattern = value;`",
            )),
        };
    }
    if input.peek(syn::token::Brace) {
        let start = rec.start(NodeKind::ExprChild(ExprMarker::Plain));
        let content;
        let brace = braced!(content in input);
        rec.token(BRACE_OPEN, brace.span.open());
        let child = if content.peek(Token![?]) {
            let q = content.parse::<Token![?]>()?;
            rec.set_kind(start, NodeKind::ExprChild(ExprMarker::Optional));
            punct(rec, "?", &q.spans);
            Node::OptionalExpr(rust(&content, rec, RustContext::Expr, |i| i.parse())?)
        } else if content.peek(Token![...]) {
            let dots = content.parse::<Token![...]>()?;
            rec.set_kind(start, NodeKind::ExprChild(ExprMarker::Spread));
            punct(rec, "...", &dots.spans);
            Node::SpreadExpr(rust(&content, rec, RustContext::Expr, |i| i.parse())?)
        } else {
            Node::Expr(rust(&content, rec, RustContext::Expr, |i| i.parse())?)
        };
        rec.token(BRACE_CLOSE, brace.span.close());
        rec.finish();
        return Ok(child);
    }
    if input.peek(LitStr) {
        rec.start(NodeKind::Text);
        let lit: LitStr = input.parse()?;
        rec.token(TokenKind::Literal, lit.span());
        rec.finish();
        return Ok(Node::Text(lit));
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

fn parse_fragment(input: ParseStream, rec: &mut Recorder) -> Result<Node> {
    rec.start(NodeKind::Fragment);
    rec.start(NodeKind::OpenTag);
    let lt = input.parse::<Token![<]>()?;
    punct(rec, "<", &lt.spans);
    let gt = input.parse::<Token![>]>()?;
    punct(rec, ">", &gt.spans);
    rec.finish();
    let mut children = Vec::new();
    while !is_closing_tag(input) {
        if input.is_empty() {
            return Err(input.error("unclosed fragment: expected `</>`"));
        }
        children.push(node(input, rec)?);
    }
    rec.start(NodeKind::CloseTag);
    let lt = input.parse::<Token![<]>()?;
    punct(rec, "<", &lt.spans);
    let slash = input.parse::<Token![/]>()?;
    punct(rec, "/", &slash.spans);
    if !input.peek(Token![>]) {
        let close: TokenTree = input.parse()?;
        return Err(syn::Error::new(
            close.span(),
            format!("expected closing tag `</>`, found `</{close}>`"),
        ));
    }
    let gt = input.parse::<Token![>]>()?;
    punct(rec, ">", &gt.spans);
    rec.finish();
    rec.finish();
    Ok(Node::Fragment(children))
}

fn if_node(input: ParseStream, rec: &mut Recorder) -> Result<IfNode> {
    rec.start(NodeKind::If);
    let if_token = input.parse::<Token![if]>()?;
    rec.token(TokenKind::Keyword("if"), if_token.span);
    // Handles `if let` and let chains too: syn parses `let` as an
    // expression in this position.
    let cond = rust(
        input,
        rec,
        RustContext::Condition,
        Expr::parse_without_eager_brace,
    )?;
    let then_children = parse_braced_children(input, rec)?;
    let (else_if, else_children) = if input.peek(Token![else]) {
        let else_token = input.parse::<Token![else]>()?;
        rec.token(TokenKind::Keyword("else"), else_token.span);
        if input.peek(Token![if]) {
            (Some(Box::new(if_node(input, rec)?)), None)
        } else {
            (None, Some(parse_braced_children(input, rec)?))
        }
    } else {
        (None, None)
    };
    rec.finish();
    Ok(IfNode {
        cond,
        then_children,
        else_if,
        else_children,
    })
}

fn for_node(input: ParseStream, rec: &mut Recorder) -> Result<ForNode> {
    rec.start(NodeKind::For);
    let for_token = input.parse::<Token![for]>()?;
    rec.token(TokenKind::Keyword("for"), for_token.span);
    let pat = rust(
        input,
        rec,
        RustContext::Pattern,
        Pat::parse_multi_with_leading_vert,
    )?;
    let in_token = input.parse::<Token![in]>()?;
    rec.token(TokenKind::Keyword("in"), in_token.span);
    let iter = rust(
        input,
        rec,
        RustContext::Iterator,
        Expr::parse_without_eager_brace,
    )?;
    let key = if input.peek(Ident) && input.peek2(Token![=]) {
        let name: Ident = input.parse()?;
        if name != "key" {
            return Err(syn::Error::new(
                name.span(),
                format!("expected `key={{..}}` or the loop body, found `{name}`"),
            ));
        }
        rec.start(NodeKind::ForKey);
        rec.token(TokenKind::Keyword("key"), name.span());
        let eq = input.parse::<Token![=]>()?;
        punct(rec, "=", &eq.spans);
        let content;
        let brace = braced!(content in input);
        rec.token(BRACE_OPEN, brace.span.open());
        let value = rust(&content, rec, RustContext::Expr, |i| i.parse())?;
        rec.token(BRACE_CLOSE, brace.span.close());
        rec.finish();
        Some((name, value))
    } else {
        None
    };
    let body = parse_braced_children(input, rec)?;
    rec.finish();
    Ok(ForNode {
        pat,
        iter,
        key,
        body,
    })
}

fn match_node(input: ParseStream, rec: &mut Recorder) -> Result<MatchNode> {
    rec.start(NodeKind::Match);
    let match_token: Token![match] = input.parse()?;
    rec.token(TokenKind::Keyword("match"), match_token.span);
    let scrutinee = rust(
        input,
        rec,
        RustContext::Scrutinee,
        Expr::parse_without_eager_brace,
    )?;
    let content;
    let brace = braced!(content in input);
    rec.token(BRACE_OPEN, brace.span.open());
    let mut arms = Vec::new();
    while !content.is_empty() {
        rec.start(NodeKind::MatchArm);
        let pat = rust(
            &content,
            rec,
            RustContext::Pattern,
            Pat::parse_multi_with_leading_vert,
        )?;
        let guard = if content.peek(Token![if]) {
            let if_token = content.parse::<Token![if]>()?;
            rec.token(TokenKind::Keyword("if"), if_token.span);
            Some(rust(&content, rec, RustContext::Expr, |i| i.parse())?)
        } else {
            None
        };
        let arrow = content.parse::<Token![=>]>()?;
        punct(rec, "=>", &arrow.spans);
        // Markup and `{ children }` bodies need no comma; a plain Rust
        // expression ends at one.
        let body = if content.peek(Token![<]) {
            vec![node(&content, rec)?]
        } else if content.peek(syn::token::Brace) {
            parse_braced_children(&content, rec)?
        } else {
            rec.start(NodeKind::ExprChild(ExprMarker::Bare));
            let expr: Expr = rust(&content, rec, RustContext::Expr, |i| i.parse())?;
            rec.finish();
            if !content.is_empty() {
                let comma = content.parse::<Token![,]>()?;
                punct(rec, ",", &comma.spans);
            }
            vec![Node::Expr(expr)]
        };
        if content.peek(Token![,]) {
            let comma = content.parse::<Token![,]>()?;
            punct(rec, ",", &comma.spans);
        }
        rec.finish();
        arms.push(MatchArm { pat, guard, body });
    }
    rec.token(BRACE_CLOSE, brace.span.close());
    rec.finish();
    Ok(MatchNode {
        match_token,
        scrutinee,
        arms,
    })
}

fn parse_braced_children(input: ParseStream, rec: &mut Recorder) -> Result<Vec<Node>> {
    rec.start(NodeKind::ChildList);
    let content;
    let brace = braced!(content in input);
    rec.token(BRACE_OPEN, brace.span.open());
    let mut children = Vec::new();
    while !content.is_empty() {
        children.push(node(&content, rec)?);
    }
    rec.token(BRACE_CLOSE, brace.span.close());
    rec.finish();
    Ok(children)
}

fn element(input: ParseStream, rec: &mut Recorder) -> Result<Element> {
    let element = rec.start(NodeKind::Element(ElementForm::Paired));
    rec.start(NodeKind::OpenTag);
    let lt = input.parse::<Token![<]>()?;
    punct(rec, "<", &lt.spans);
    let name = rec.start(NodeKind::TagName(TagKind::Builtin));
    let (mut tag, span) = parse_tag(input, rec)?;
    rec.finish();

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
        rec.start(NodeKind::CtorArgs);
        let content;
        let paren = parenthesized!(content in input);
        rec.token(TokenKind::Open(Delimiter::Parenthesis), paren.span.open());
        let args = ctor_args(&content, rec)?;
        rec.token(TokenKind::Close(Delimiter::Parenthesis), paren.span.close());
        rec.finish();
        Some(args)
    } else {
        None
    };
    rec.set_kind(name, NodeKind::TagName(tag_kind(&tag)));

    let mut attrs = Vec::new();
    while !input.peek(Token![>]) && !(input.peek(Token![/]) && input.peek2(Token![>])) {
        if input.is_empty() {
            return Err(syn::Error::new(span, "unclosed tag: expected `>` or `/>`"));
        }
        attrs.push(attr(input, rec)?);
    }

    if input.peek(Token![/]) {
        let slash = input.parse::<Token![/]>()?;
        punct(rec, "/", &slash.spans);
        let gt = input.parse::<Token![>]>()?;
        punct(rec, ">", &gt.spans);
        rec.finish();
        rec.set_kind(element, NodeKind::Element(ElementForm::SelfClosing));
        rec.finish();
        return Ok(Element {
            tag,
            span,
            ctor_args,
            attrs,
            children: Vec::new(),
        });
    }
    let gt = input.parse::<Token![>]>()?;
    punct(rec, ">", &gt.spans);
    rec.finish();

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
        children.push(node(input, rec)?);
    }
    parse_closing_tag(input, rec, &tag)?;
    rec.finish();
    Ok(Element {
        tag,
        span,
        ctor_args,
        attrs,
        children,
    })
}

/// `Punctuated::parse_terminated` with each argument and comma recorded.
fn ctor_args(
    input: ParseStream,
    rec: &mut Recorder,
) -> Result<syn::punctuated::Punctuated<Expr, Token![,]>> {
    let mut args = syn::punctuated::Punctuated::new();
    loop {
        if input.is_empty() {
            break;
        }
        args.push_value(rust(input, rec, RustContext::Expr, Expr::parse)?);
        if input.is_empty() {
            break;
        }
        let comma: Token![,] = input.parse()?;
        punct(rec, ",", &comma.spans);
        args.push_punct(comma);
    }
    Ok(args)
}

fn tag_kind(tag: &Tag) -> TagKind {
    match tag {
        Tag::Builtin(_) => TagKind::Builtin,
        Tag::Component(_) => TagKind::Component,
        Tag::Function(_) => TagKind::Function,
        Tag::Slot(_) => TagKind::Slot,
        Tag::Value(_) => TagKind::Value,
    }
}

fn parse_tag(input: ParseStream, rec: &mut Recorder) -> Result<(Tag, proc_macro2::Span)> {
    if input.peek(syn::token::Brace) {
        let content;
        let brace = braced!(content in input);
        rec.token(BRACE_OPEN, brace.span.open());
        let expr: Expr = rust(&content, rec, RustContext::Expr, |i| i.parse())?;
        rec.token(BRACE_CLOSE, brace.span.close());
        return Ok((Tag::Value(Box::new(expr)), brace.span.join()));
    }
    if input.peek(Token![.]) {
        let dot = input.parse::<Token![.]>()?;
        punct(rec, ".", &dot.spans);
        let name = input.call(Ident::parse_any)?;
        rec.token(TokenKind::Ident, name.span());
        let span = name.span();
        return Ok((Tag::Slot(name), span));
    }
    let first = input.call(Ident::parse_any)?;
    rec.token(TokenKind::Ident, first.span());
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
        let sep = input.parse::<Token![::]>()?;
        punct(rec, "::", &sep.spans);
        let seg: Ident = input.parse()?;
        rec.token(TokenKind::Ident, seg.span());
        path.segments.push(seg.into());
    }
    Ok((Tag::Component(path), span))
}

/// The tag name as `view!` writes it, for messages: `div`, `a::B`,
/// `.slot`, or `{..}` for a value tag.
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

fn parse_closing_tag(input: ParseStream, rec: &mut Recorder, open: &Tag) -> Result<()> {
    rec.start(NodeKind::CloseTag);
    let lt = input.parse::<Token![<]>()?;
    punct(rec, "<", &lt.spans);
    let slash = input.parse::<Token![/]>()?;
    punct(rec, "/", &slash.spans);
    let expected = tag_name(open);
    if matches!(open, Tag::Value(_)) {
        if !input.peek(Token![>]) {
            return Err(input.error("an expression tag `<{..}>` closes with `</>`"));
        }
        let gt = input.parse::<Token![>]>()?;
        punct(rec, ">", &gt.spans);
        rec.finish();
        return Ok(());
    }
    if input.peek(Token![>]) {
        return Err(input.error(format!("expected closing tag `</{expected}>`, found `</>`")));
    }
    let mut found = String::new();
    let start = input.span();
    if input.peek(Token![.]) {
        let dot = input.parse::<Token![.]>()?;
        punct(rec, ".", &dot.spans);
        found.push('.');
    }
    let name = input.call(Ident::parse_any)?;
    rec.token(TokenKind::Ident, name.span());
    found.push_str(&name.to_string());
    while input.peek(Token![::]) {
        let sep = input.parse::<Token![::]>()?;
        punct(rec, "::", &sep.spans);
        found.push_str("::");
        let seg = input.call(Ident::parse_any)?;
        rec.token(TokenKind::Ident, seg.span());
        found.push_str(&seg.to_string());
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
    let gt = input.parse::<Token![>]>()?;
    punct(rec, ">", &gt.spans);
    rec.finish();
    Ok(())
}

fn attr(input: ParseStream, rec: &mut Recorder) -> Result<Attr> {
    // @when {condition} { attr1 attr2=val ... }
    // @for pat in iter { attr1 attr2=val ... }
    if input.peek(Token![@]) {
        let group = rec.start(NodeKind::AttrWhen);
        let at = input.parse::<Token![@]>()?;
        punct(rec, "@", &at.spans);
        if input.peek(Token![for]) {
            rec.set_kind(group, NodeKind::AttrFor);
            let for_token = input.parse::<Token![for]>()?;
            rec.token(TokenKind::Keyword("for"), for_token.span);
            let pat = rust(
                input,
                rec,
                RustContext::Pattern,
                Pat::parse_multi_with_leading_vert,
            )?;
            let in_token = input.parse::<Token![in]>()?;
            rec.token(TokenKind::Keyword("in"), in_token.span);
            let iter = rust(
                input,
                rec,
                RustContext::Iterator,
                Expr::parse_without_eager_brace,
            )?;
            let attrs = attr_list(input, rec)?;
            rec.finish();
            return Ok(Attr::For(Box::new(pat), iter, attrs));
        }
        let kw: Ident = input.parse()?;
        if kw != "when" {
            return Err(syn::Error::new(
                kw.span(),
                "expected `when` or `for` after `@`",
            ));
        }
        rec.token(TokenKind::Keyword("when"), kw.span());
        let cond_content;
        let brace = braced!(cond_content in input);
        rec.token(BRACE_OPEN, brace.span.open());
        let cond: Expr = rust(&cond_content, rec, RustContext::Expr, |i| i.parse())?;
        rec.token(BRACE_CLOSE, brace.span.close());
        let attrs = attr_list(input, rec)?;
        rec.finish();
        return Ok(Attr::When(cond, attrs));
    }

    let start = rec.start(NodeKind::Attr(AttrForm::Flag));
    let name = parse_attr_name(input, rec)?;
    if name.written == "class" {
        rec.set_kind(start, NodeKind::Attr(AttrForm::Class));
        let eq = input.parse::<Token![=]>()?;
        punct(rec, "=", &eq.spans);
        if !input.peek(LitStr) {
            return Err(input.error(
                "`class` takes a string literal of class names, as in `class=\"flex-row gap-2\"`; \
                 use attributes or `@when` for computed values",
            ));
        }
        let lit: LitStr = input.parse()?;
        rec.token(TokenKind::Literal, lit.span());
        rec.finish();
        return Ok(Attr::Class(lit));
    }
    let value = if input.peek(Token![=]) {
        let eq = input.parse::<Token![=]>()?;
        punct(rec, "=", &eq.spans);
        let (value, form) = parse_attr_value(input, rec)?;
        rec.set_kind(start, NodeKind::Attr(form));
        value
    } else {
        AttrValue::Flag
    };
    rec.finish();
    Ok(Attr::Method { name, value })
}

/// `{ attrs }` of an `@when` or `@for` group.
fn attr_list(input: ParseStream, rec: &mut Recorder) -> Result<Vec<Attr>> {
    rec.start(NodeKind::AttrList);
    let attrs_content;
    let brace = braced!(attrs_content in input);
    rec.token(BRACE_OPEN, brace.span.open());
    let mut attrs = Vec::new();
    while !attrs_content.is_empty() {
        attrs.push(attr(&attrs_content, rec)?);
    }
    rec.token(BRACE_CLOSE, brace.span.close());
    rec.finish();
    Ok(attrs)
}

fn parse_attr_name(input: ParseStream, rec: &mut Recorder) -> Result<AttrName> {
    rec.start(NodeKind::AttrName);
    let first = input.call(Ident::parse_any)?;
    rec.token(TokenKind::Ident, first.span());
    if first == "on" && input.peek(Token![:]) && !input.peek(Token![::]) {
        let colon = input.parse::<Token![:]>()?;
        punct(rec, ":", &colon.spans);
        let event = input.call(Ident::parse_any)?;
        rec.token(TokenKind::Ident, event.span());
        let mut written = format!("on:{event}");
        let binding = if input.peek(Token![:]) && !input.peek(Token![::]) {
            let colon = input.parse::<Token![:]>()?;
            punct(rec, ":", &colon.spans);
            let (text, span) = parse_binding(input, rec)?;
            written.push(':');
            written.push_str(&text);
            Some(LitStr::new(&text, span))
        } else {
            None
        };
        rec.finish();
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
        let dash = input.parse::<Token![-]>()?;
        punct(rec, "-", &dash.spans);
        let seg = input.call(Ident::parse_any)?;
        rec.token(TokenKind::Ident, seg.span());
        written.push('-');
        written.push_str(&seg.to_string());
        segments.push(seg);
    }
    rec.finish();
    Ok(AttrName {
        written,
        first,
        segments,
        event: None,
    })
}

/// The tokens of `mod+s` in `on:key:mod+s={..}`, up to the `=`, joined
/// without spaces (the lexer has already dropped them).
fn parse_binding(input: ParseStream, rec: &mut Recorder) -> Result<(String, proc_macro2::Span)> {
    let span = input.span();
    let mut tokens = TokenStream2::new();
    while !input.is_empty() && !input.peek(Token![=]) {
        let tt = input.parse::<TokenTree>()?;
        rec.token(TokenKind::Binding, tt.span());
        tokens.extend([tt]);
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

fn parse_attr_value(input: ParseStream, rec: &mut Recorder) -> Result<(AttrValue, AttrForm)> {
    if input.peek(syn::token::Brace) {
        let content;
        let brace = braced!(content in input);
        rec.token(BRACE_OPEN, brace.span.open());
        let value = if content.peek(Token![if]) {
            let if_expr: ExprIf = rust(&content, rec, RustContext::Expr, |i| i.parse())?;
            (AttrValue::If(if_expr), AttrForm::If)
        } else if content.peek(Token![@]) {
            let at = content.parse::<Token![@]>()?;
            punct(rec, "@", &at.spans);
            let expr = rust(&content, rec, RustContext::Expr, |i| i.parse())?;
            (AttrValue::Reactive(expr), AttrForm::Reactive)
        } else {
            let expr = rust(&content, rec, RustContext::Expr, |i| i.parse())?;
            (AttrValue::Expr(expr), AttrForm::Expr)
        };
        rec.token(BRACE_CLOSE, brace.span.close());
        return Ok(value);
    }
    if input.peek(syn::Lit) {
        // syn reads `-1.5` as one literal, its span joined over both tokens.
        let lit: syn::Lit = input.parse()?;
        rec.token(TokenKind::Literal, lit.span());
        return Ok((AttrValue::Expr(syn::parse_quote!(#lit)), AttrForm::Literal));
    }
    if input.peek(Token![-]) && input.peek2(syn::Lit) {
        let minus: Token![-] = input.parse()?;
        punct(rec, "-", &minus.spans);
        let lit: syn::Lit = input.parse()?;
        rec.token(TokenKind::Literal, lit.span());
        return Ok((
            AttrValue::Expr(syn::parse_quote!(#minus #lit)),
            AttrForm::NegativeLiteral,
        ));
    }
    let found: TokenTree = input.parse()?;
    Err(syn::Error::new(
        found.span(),
        format!("attribute values are a literal or a braced expression: write `{{{found}}}`"),
    ))
}
