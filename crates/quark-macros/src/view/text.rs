//! Text literals with `{expr}` interpolation: `"Hello, {name}!"` lowers to
//! `format!("Hello, {}!", name)`. `{{` and `}}` are literal braces, and
//! `{value:.2}` passes a format spec through.

use proc_macro2::{Group, Span, TokenStream as TokenStream2, TokenTree};
use quote::quote;
use syn::{Expr, LitStr};

pub(crate) enum Segment {
    Lit(String),
    Expr(TokenStream2, Option<String>),
}

/// Split a text literal into literal runs and interpolated expressions.
pub(crate) fn segments(lit: &LitStr) -> syn::Result<Vec<Segment>> {
    let value = lit.value();
    let span = lit.span();
    let mut out = Vec::new();
    let mut run = String::new();
    let mut chars = value.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        match c {
            '{' if chars.peek().is_some_and(|&(_, n)| n == '{') => {
                chars.next();
                run.push('{');
            }
            '}' if chars.peek().is_some_and(|&(_, n)| n == '}') => {
                chars.next();
                run.push('}');
            }
            '}' => {
                return Err(syn::Error::new(
                    span,
                    format!("unmatched `}}` in text {value:?}; write `}}}}` for a literal brace"),
                ));
            }
            '{' => {
                // Find the matching `}`, allowing braces inside the expression.
                let mut depth = 1;
                let mut end = None;
                for (j, n) in chars.by_ref() {
                    match n {
                        '{' => depth += 1,
                        '}' => {
                            depth -= 1;
                            if depth == 0 {
                                end = Some(j);
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                let Some(end) = end else {
                    return Err(syn::Error::new(
                        span,
                        format!(
                            "unclosed `{{` in text {value:?}; write `{{{{` for a literal brace"
                        ),
                    ));
                };
                let inner = &value[i + 1..end];
                if inner.trim().is_empty() {
                    return Err(syn::Error::new(
                        span,
                        "empty `{}` in text: put the expression inside, as in \"{count}\"",
                    ));
                }
                let (expr_src, spec) = split_spec(inner);
                let expr: Expr = syn::parse_str(expr_src).map_err(|e| {
                    syn::Error::new(span, format!("in `{{{inner}}}` of this text: {e}"))
                })?;
                if !run.is_empty() {
                    out.push(Segment::Lit(std::mem::take(&mut run)));
                }
                out.push(Segment::Expr(respan(quote!(#expr), span), spec));
            }
            c => run.push(c),
        }
    }
    if !run.is_empty() || out.is_empty() {
        out.push(Segment::Lit(run));
    }
    Ok(out)
}

/// `value:.2` -> (`value`, `.2`). Only a single `:` outside brackets
/// starts a spec, so paths like `a::b` stay whole.
fn split_spec(inner: &str) -> (&str, Option<String>) {
    let bytes = inner.as_bytes();
    let mut depth = 0i32;
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 => {
                let prev = i.checked_sub(1).map(|p| bytes[p]);
                let next = bytes.get(i + 1).copied();
                if prev != Some(b':') && next != Some(b':') {
                    return (&inner[..i], Some(inner[i + 1..].to_owned()));
                }
            }
            _ => {}
        }
    }
    (inner, None)
}

/// Text segments joined into one `String`-like expression: the literal
/// itself when nothing is interpolated, `format!(..)` otherwise.
pub(crate) fn join(parts: &[(Vec<Segment>, Span)], lone_lit: Option<&LitStr>) -> TokenStream2 {
    if let (Some(lit), [(segs, _)]) = (lone_lit, parts)
        && let [Segment::Lit(text)] = segs.as_slice()
    {
        if *text == lit.value() {
            return quote!(#lit);
        }
        let unescaped = LitStr::new(text, lit.span());
        return quote!(#unescaped);
    }
    let mut fmt = String::new();
    let mut args = Vec::new();
    let mut span = Span::call_site();
    for (segs, s) in parts {
        span = *s;
        for seg in segs {
            match seg {
                Segment::Lit(text) => fmt.push_str(&text.replace('{', "{{").replace('}', "}}")),
                Segment::Expr(expr, spec) => {
                    match spec {
                        Some(spec) => {
                            fmt.push_str("{:");
                            fmt.push_str(spec);
                            fmt.push('}');
                        }
                        None => fmt.push_str("{}"),
                    }
                    args.push(expr.clone());
                }
            }
        }
    }
    if args.is_empty() {
        let lit = LitStr::new(&fmt.replace("{{", "{").replace("}}", "}"), span);
        return quote!(#lit);
    }
    let fmt = LitStr::new(&fmt, span);
    quote!(::std::format!(#fmt, #(#args),*))
}

/// Give every token `span`, so errors in an interpolated expression point
/// at the literal it came from and its names resolve at the call site.
pub(crate) fn respan(tokens: TokenStream2, span: Span) -> TokenStream2 {
    tokens
        .into_iter()
        .map(|tt| match tt {
            TokenTree::Group(g) => {
                let mut group = Group::new(g.delimiter(), respan(g.stream(), span));
                group.set_span(span);
                TokenTree::Group(group)
            }
            mut other => {
                other.set_span(span);
                other
            }
        })
        .collect()
}
