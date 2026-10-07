//! The `class="..."` vocabulary: Tailwind-style names lowered at compile
//! time to builder calls. Nothing here exists at runtime; `p-4` becomes
//! `.p(16.0)` and `hover:bg-[c]` becomes `.hover(|s| s.bg(c))`.
//!
//! The tables below are the single list of classes. `__class_vocabulary!`
//! applies every entry to the builder it names, so a test in
//! quark-components fails to compile when an entry names a method the
//! builders do not have, and a unit test here keeps the reference table in
//! `docs/guide/writing-views.md` in sync.

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{quote, quote_spanned};
use syn::Ident;

use crate::suggest::closest;
use crate::view::text::respan;

/// The builder a class applies to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum On {
    /// `div()` and anything `Styled`.
    Box,
    /// `text()`.
    Text,
    /// `svg_icon()`.
    Icon,
}

impl On {
    fn tag(self) -> &'static str {
        match self {
            On::Box => "div",
            On::Text => "text",
            On::Icon => "icon",
        }
    }
}

/// A class with no value: `class` lowers to `.method(args)`.
pub(crate) struct Keyword {
    pub class: &'static str,
    pub method: &'static str,
    pub args: &'static str,
    pub on: On,
}

const fn kw(class: &'static str, method: &'static str, args: &'static str, on: On) -> Keyword {
    Keyword {
        class,
        method,
        args,
        on,
    }
}

pub(crate) const KEYWORDS: &[Keyword] = &[
    // Flex
    kw("flex-row", "flex_row", "", On::Box),
    kw("flex-col", "flex_col", "", On::Box),
    kw("flex-row-reverse", "flex_row_reverse", "", On::Box),
    kw("flex-col-reverse", "flex_col_reverse", "", On::Box),
    kw("flex-wrap", "flex_wrap", "", On::Box),
    kw("flex-1", "flex_1", "", On::Box),
    kw("flex-auto", "flex_auto", "", On::Box),
    kw("flex-none", "flex_none", "", On::Box),
    kw("grow", "flex_grow", "", On::Box),
    kw("grow-0", "flex_grow_val", "0.0", On::Box),
    kw("shrink-0", "flex_shrink_0", "", On::Box),
    kw("basis-auto", "basis_auto", "", On::Box),
    kw("basis-full", "basis_full", "", On::Box),
    // Alignment
    kw("items-start", "items_start", "", On::Box),
    kw("items-center", "items_center", "", On::Box),
    kw("items-end", "items_end", "", On::Box),
    kw("items-baseline", "items_baseline", "", On::Box),
    kw("items-stretch", "items_stretch", "", On::Box),
    kw("justify-start", "justify_start", "", On::Box),
    kw("justify-center", "justify_center", "", On::Box),
    kw("justify-end", "justify_end", "", On::Box),
    kw("justify-between", "justify_between", "", On::Box),
    kw("self-auto", "self_auto", "", On::Box),
    kw("self-start", "self_start", "", On::Box),
    kw("self-center", "self_center", "", On::Box),
    kw("self-end", "self_end", "", On::Box),
    kw("self-stretch", "self_stretch", "", On::Box),
    kw("self-baseline", "self_baseline", "", On::Box),
    // Grid
    kw("grid", "grid", "", On::Box),
    kw("grid-flow-row", "grid_flow_row", "", On::Box),
    kw("grid-flow-col", "grid_flow_col", "", On::Box),
    kw("grid-flow-dense", "grid_dense", "", On::Box),
    // Sizing
    kw("w-full", "w_full", "", On::Box),
    kw("h-full", "h_full", "", On::Box),
    kw("size-full", "size_full", "", On::Box),
    kw("size-max-content", "size_max_content", "", On::Box),
    kw("aspect-square", "aspect_ratio", "1.0", On::Box),
    kw("aspect-video", "aspect_ratio", "16.0 / 9.0", On::Box),
    // Position and visibility
    kw("absolute", "absolute", "", On::Box),
    kw("relative", "relative", "", On::Box),
    kw("hidden", "hidden", "", On::Box),
    kw("overflow-hidden", "overflow_hidden", "", On::Box),
    kw("overflow-x-hidden", "overflow_x_hidden", "", On::Box),
    kw("overflow-y-hidden", "overflow_y_hidden", "", On::Box),
    kw("overflow-scroll", "overflow_scroll", "", On::Box),
    kw("overflow-x-scroll", "overflow_x_scroll", "", On::Box),
    kw("overflow-y-scroll", "overflow_y_scroll", "", On::Box),
    kw("overflow-clip", "clip", "", On::Box),
    kw("scrollbar-none", "hide_scrollbar", "", On::Box),
    kw("scrollbar-auto-hide", "scrollbar_auto_hide", "", On::Box),
    // Corners
    kw("rounded-none", "rounded_none", "", On::Box),
    kw("rounded-sm", "rounded_sm", "", On::Box),
    kw("rounded-md", "rounded_md", "", On::Box),
    kw("rounded-lg", "rounded_lg", "", On::Box),
    kw("rounded-xl", "rounded_xl", "", On::Box),
    kw("rounded-full", "rounded_full", "", On::Box),
    // Cursor
    kw(
        "cursor-default",
        "cursor",
        "::quark::CursorHint::Default",
        On::Box,
    ),
    kw(
        "cursor-pointer",
        "cursor",
        "::quark::CursorHint::Pointer",
        On::Box,
    ),
    kw(
        "cursor-text",
        "cursor",
        "::quark::CursorHint::Text",
        On::Box,
    ),
    kw(
        "cursor-move",
        "cursor",
        "::quark::CursorHint::Move",
        On::Box,
    ),
    kw(
        "cursor-grab",
        "cursor",
        "::quark::CursorHint::Grab",
        On::Box,
    ),
    kw(
        "cursor-col-resize",
        "cursor",
        "::quark::CursorHint::ResizeCol",
        On::Box,
    ),
    kw(
        "cursor-row-resize",
        "cursor",
        "::quark::CursorHint::ResizeRow",
        On::Box,
    ),
    // Text
    kw("text-xs", "text_xs", "", On::Text),
    kw("text-sm", "text_sm", "", On::Text),
    kw("text-lg", "text_lg", "", On::Text),
    kw("text-center", "text_center", "", On::Text),
    kw("text-right", "text_right", "", On::Text),
    kw("font-medium", "medium", "", On::Text),
    kw("font-semibold", "semibold", "", On::Text),
    kw("font-bold", "bold", "", On::Text),
    kw("font-mono", "mono", "", On::Text),
    kw("truncate", "truncate", "", On::Text),
    kw("leading-none", "line_height", "1.0", On::Text),
    kw("leading-tight", "line_height", "1.25", On::Text),
    kw("leading-snug", "line_height", "1.375", On::Text),
    kw("leading-normal", "line_height", "1.5", On::Text),
    kw("leading-relaxed", "line_height", "1.625", On::Text),
    kw("leading-loose", "line_height", "2.0", On::Text),
];

/// How a family reads the part after its prefix.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Value {
    /// Tailwind's 4px scale: `p-4` is 16, `p-0.5` is 2, `p-px` is 1;
    /// `p-[13px]` and `p-[expr]` are taken as is.
    Spacing,
    /// Only bracketed: `[320px]`, `[1.5]`, or `[expr]`.
    Length,
    /// Points: `border-2` is 2; `[3px]` and `[expr]` as for `Length`.
    Px,
    /// `opacity-50` is 0.5.
    Percent,
    /// An integer: `z-10`.
    Int,
    /// A positive count: `grid-cols-3`.
    Count,
    /// Degrees, emitted in radians: `rotate-45`.
    Degrees,
    /// `aspect-[16/9]`.
    Ratio,
    /// `transparent`, `white`, `black`, `[#rrggbb]`, `[#rrggbbaa]`, or `[expr]`.
    Color,
}

/// A class with a value: `prefix-value` lowers to `.method(value)`.
pub(crate) struct Family {
    pub prefix: &'static str,
    pub method: &'static str,
    pub value: Value,
    pub on: On,
    /// The `StyleOverride` method this family maps to under `hover:`.
    pub hover: Option<&'static str>,
}

const fn fam(prefix: &'static str, method: &'static str, value: Value, on: On) -> Family {
    Family {
        prefix,
        method,
        value,
        on,
        hover: None,
    }
}

const fn hov(
    prefix: &'static str,
    method: &'static str,
    value: Value,
    on: On,
    hover: &'static str,
) -> Family {
    Family {
        prefix,
        method,
        value,
        on,
        hover: Some(hover),
    }
}

pub(crate) const FAMILIES: &[Family] = &[
    fam("p", "p", Value::Spacing, On::Box),
    fam("px", "px", Value::Spacing, On::Box),
    fam("py", "py", Value::Spacing, On::Box),
    fam("pt", "pt", Value::Spacing, On::Box),
    fam("pb", "pb", Value::Spacing, On::Box),
    fam("pl", "pl", Value::Spacing, On::Box),
    fam("pr", "pr", Value::Spacing, On::Box),
    fam("ml", "margin_left", Value::Spacing, On::Box),
    fam("gap", "gap", Value::Spacing, On::Box),
    fam("gap-x", "gap_x", Value::Spacing, On::Box),
    fam("gap-y", "gap_y", Value::Spacing, On::Box),
    fam("w", "w", Value::Spacing, On::Box),
    fam("h", "h", Value::Spacing, On::Box),
    fam("size", "size", Value::Spacing, On::Box),
    fam("min-w", "min_w", Value::Spacing, On::Box),
    fam("min-h", "min_h", Value::Spacing, On::Box),
    fam("max-w", "max_w", Value::Spacing, On::Box),
    fam("max-h", "max_h", Value::Spacing, On::Box),
    fam("basis", "basis", Value::Spacing, On::Box),
    fam("top", "top", Value::Spacing, On::Box),
    fam("bottom", "bottom", Value::Spacing, On::Box),
    fam("left", "left", Value::Spacing, On::Box),
    fam("right", "right", Value::Spacing, On::Box),
    fam("inset", "inset", Value::Spacing, On::Box),
    fam("grow", "flex_grow_val", Value::Length, On::Box),
    hov("rounded", "rounded", Value::Length, On::Box, "rounded"),
    fam("border", "border_w", Value::Px, On::Box),
    hov("opacity", "opacity", Value::Percent, On::Box, "opacity"),
    fam("z", "z_index", Value::Int, On::Box),
    fam("grid-cols", "grid_cols_n", Value::Count, On::Box),
    fam("col-span", "col_span", Value::Count, On::Box),
    fam("row-span", "row_span", Value::Count, On::Box),
    fam("col-start", "col_start", Value::Int, On::Box),
    fam("col-end", "col_end", Value::Int, On::Box),
    fam("row-start", "row_start", Value::Int, On::Box),
    fam("row-end", "row_end", Value::Int, On::Box),
    fam("aspect", "aspect_ratio", Value::Ratio, On::Box),
    fam("rotate", "rotate", Value::Degrees, On::Box),
    fam("scale", "scale", Value::Percent, On::Box),
    fam("blur", "blur", Value::Px, On::Box),
    hov("bg", "bg", Value::Color, On::Box, "bg"),
    hov("border", "border", Value::Color, On::Box, "border_color"),
    fam("border-t", "border_t", Value::Color, On::Box),
    fam("border-r", "border_r", Value::Color, On::Box),
    fam("border-b", "border_b", Value::Color, On::Box),
    fam("border-l", "border_l", Value::Color, On::Box),
    hov("text", "color", Value::Color, On::Text, "text_color"),
    fam("text", "size", Value::Length, On::Text),
    fam("leading", "line_height", Value::Length, On::Text),
    hov("fill", "color", Value::Color, On::Icon, "icon_color"),
];

/// Variants that exist in Tailwind but have no quark equivalent, with the
/// message saying what to use instead.
const UNSUPPORTED_VARIANTS: &[(&str, &str)] = &[
    (
        "focus",
        "div has no focus style state; use `@when {cx.is_focused(id)} { .. }`",
    ),
    (
        "focus-visible",
        "div has no focus style state; use `@when {cx.is_focused(id)} { .. }`",
    ),
    (
        "focus-within",
        "div has no focus style state; use `@when {..} { .. }`",
    ),
    (
        "active",
        "div has no pressed style state; use `@when {pressed} { .. }`",
    ),
    (
        "disabled",
        "div has no disabled style state; use `@when {disabled} { .. }`",
    ),
    (
        "dark",
        "styles resolve against the theme at paint time, not a dark mode switch; \
         pick colors from `cx.theme.colors`",
    ),
    (
        "group-hover",
        "only the hovered div itself restyles; use `hover:` on that div",
    ),
];

const BREAKPOINTS: &[&str] = &["sm", "md", "lg", "xl", "2xl"];

/// Builder calls for one `class="..."` literal.
#[derive(Default)]
pub(crate) struct Lowered {
    pub calls: Vec<TokenStream2>,
    /// Calls on the `StyleOverride` passed to `.hover(..)`.
    pub hover: Vec<TokenStream2>,
}

/// Lower `classes` (the literal's value) for a builder of kind `on`
/// (`None` for components, which take any class their builder has).
/// Errors name the class; the literal's span is the closest stable Rust
/// lets a macro point.
pub(crate) fn lower(
    classes: &str,
    span: Span,
    on: Option<On>,
    errors: &mut Vec<syn::Error>,
) -> Lowered {
    let mut out = Lowered::default();
    for class in split_classes(classes) {
        if let Err(message) = lower_one(class, span, on, &mut out) {
            errors.push(syn::Error::new(span, message));
        }
    }
    out
}

/// Split on whitespace outside brackets, so `bg-[Color::rgba(1, 2, 3, 4)]`
/// stays one class.
fn split_classes(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut start = None;
    for (i, c) in s.char_indices() {
        match c {
            '[' | '(' => depth += 1,
            ']' | ')' => depth -= 1,
            c if c.is_whitespace() && depth == 0 => {
                if let Some(st) = start.take() {
                    out.push(&s[st..i]);
                }
                continue;
            }
            _ => {}
        }
        start.get_or_insert(i);
    }
    if let Some(st) = start {
        out.push(&s[st..]);
    }
    out
}

/// `hover:bg-[x]` -> (["hover"], "bg-[x]"), splitting only outside brackets.
fn split_variants(class: &str) -> (Vec<&str>, &str) {
    let mut variants = Vec::new();
    let mut rest = class;
    loop {
        let mut depth = 0i32;
        let mut cut = None;
        for (i, c) in rest.char_indices() {
            match c {
                '[' | '(' => depth += 1,
                ']' | ')' => depth -= 1,
                ':' if depth == 0 => {
                    cut = Some(i);
                    break;
                }
                _ => {}
            }
        }
        match cut {
            Some(i) => {
                variants.push(&rest[..i]);
                rest = &rest[i + 1..];
            }
            None => return (variants, rest),
        }
    }
}

fn lower_one(class: &str, span: Span, on: Option<On>, out: &mut Lowered) -> Result<(), String> {
    let (variants, base) = split_variants(class);
    let mut hover = false;
    for variant in variants {
        match variant {
            "hover" => hover = true,
            v if BREAKPOINTS.contains(&v) => {
                return Err(format!(
                    "class `{class}`: responsive breakpoints (`{v}:`) are not supported; \
                     quark has no viewport media queries. Branch on the window size in the \
                     view instead, as in `@when {{width >= 768.0}} {{ class=\"..\" }}`"
                ));
            }
            v => {
                if let Some((_, why)) = UNSUPPORTED_VARIANTS.iter().find(|(name, _)| *name == v) {
                    return Err(format!("class `{class}`: `{v}:` is not supported: {why}"));
                }
                let mut names: Vec<&str> = vec!["hover"];
                names.extend(UNSUPPORTED_VARIANTS.iter().map(|(n, _)| *n));
                let hint = closest(v, names.iter().copied())
                    .map(|s| format!("; did you mean `{s}:`?"))
                    .unwrap_or_default();
                return Err(format!(
                    "class `{class}`: unknown variant `{v}:`{hint} The only supported variant is `hover:`"
                ));
            }
        }
    }

    let (method, args, class_on, hover_method) =
        resolve(base, span).map_err(|e| e.unwrap_or_else(|| unknown_class(class, base)))?;

    if let Some(on) = on
        && on != class_on
    {
        return Err(format!(
            "class `{base}` styles <{}>; it does not apply to <{}>",
            class_on.tag(),
            on.tag()
        ));
    }

    if hover {
        let Some(hover_method) = hover_method else {
            return Err(format!(
                "class `{class}`: `hover:` restyles only background, border color, text and \
                 icon color, opacity, and `rounded-[..]`; `{base}` is none of these"
            ));
        };
        let m = Ident::new(hover_method, span);
        out.hover.push(quote!(.#m(#args)));
    } else {
        let m = Ident::new(method, span);
        out.calls.push(quote!(.#m(#args)));
    }
    Ok(())
}

/// `Err(None)`: not in the vocabulary. `Err(Some(msg))`: a known family
/// with a bad value.
type Resolved = (&'static str, TokenStream2, On, Option<&'static str>);

fn resolve(base: &str, span: Span) -> Result<Resolved, Option<String>> {
    if let Some(k) = KEYWORDS.iter().find(|k| k.class == base) {
        let args: TokenStream2 = k.args.parse().expect("keyword args are valid Rust");
        return Ok((k.method, respan(args, span), k.on, None));
    }
    // Longest prefix first, so `gap-x-2` is `gap-x`, not `gap`. A prefix
    // can name two families with different value kinds (`text-sm`,
    // `text-[14px]`, `text-[#fff]`); the first that accepts the value wins.
    let mut matches: Vec<&Family> = FAMILIES
        .iter()
        .filter(|f| {
            base.len() > f.prefix.len() + 1
                && base.starts_with(f.prefix)
                && base.as_bytes()[f.prefix.len()] == b'-'
        })
        .collect();
    matches.sort_by_key(|f| std::cmp::Reverse(f.prefix.len()));
    let mut first_error = None;
    for family in &matches {
        let value = &base[family.prefix.len() + 1..];
        // Where a prefix names a color and a length (`text-[..]`,
        // `border-[..]`), a bracketed expression is the color; Tailwind
        // makes the same call.
        let shared = matches.iter().filter(|g| g.prefix == family.prefix).count() > 1;
        let allow_expr = !shared || family.value == Value::Color;
        match parse_value(family, value, allow_expr, span) {
            Ok(args) => return Ok((family.method, args, family.on, family.hover)),
            Err(e) => {
                first_error.get_or_insert(e);
            }
        }
    }
    Err(first_error.flatten())
}

/// Values a family cannot read report `Err(None)` when another family with
/// the same prefix might (a color for `text-[..]`), and a message otherwise.
fn parse_value(
    family: &Family,
    value: &str,
    allow_expr: bool,
    span: Span,
) -> Result<TokenStream2, Option<String>> {
    let class = format!("{}-{value}", family.prefix);
    let bracket = value
        .strip_prefix('[')
        .and_then(|v| v.strip_suffix(']'))
        .map(str::to_owned);
    match family.value {
        Value::Color => {
            let color = |r: u8, g: u8, b: u8, a: u8| quote_spanned!(span=> ::quark::Color::rgba(#r, #g, #b, #a));
            match (value, &bracket) {
                ("transparent", _) => Ok(quote_spanned!(span=> ::quark::Color::TRANSPARENT)),
                ("white", _) => Ok(color(255, 255, 255, 255)),
                ("black", _) => Ok(color(0, 0, 0, 255)),
                (_, Some(inner)) if inner.starts_with('#') => {
                    let [r, g, b, a] = parse_hex(&inner[1..]).ok_or_else(|| {
                        Some(format!(
                            "class `{class}`: `{inner}` is not a #rgb, #rrggbb, or #rrggbbaa color"
                        ))
                    })?;
                    Ok(color(r, g, b, a))
                }
                // A number in brackets is a length, for a sibling family.
                (_, Some(inner)) if is_length(inner) => Err(None),
                (_, Some(inner)) => expr(inner, &class, span),
                _ => Err(None),
            }
        }
        Value::Spacing => match (value, &bracket) {
            ("px", _) => Ok(f32_lit(1.0, span)),
            (_, Some(inner)) => length(inner, &class, allow_expr, span),
            _ => match parse_number(value) {
                Some(n) if n >= 0.0 && (n * 2.0).fract() == 0.0 => Ok(f32_lit(n * 4.0, span)),
                _ if value.contains('/') => Err(Some(format!(
                    "class `{class}`: fractions need percent sizes, which `{}` does not take; \
                     use `{}-full` or `{}-[px]`",
                    family.method, family.prefix, family.prefix
                ))),
                _ => Err(None),
            },
        },
        Value::Length => match &bracket {
            Some(inner) => length(inner, &class, allow_expr, span),
            None => Err(None),
        },
        Value::Px => match (parse_number(value), &bracket) {
            (Some(n), None) if n >= 0.0 => Ok(f32_lit(n, span)),
            (_, Some(inner)) => length(inner, &class, allow_expr, span),
            _ => Err(None),
        },
        Value::Percent => match parse_number(value) {
            Some(n) if bracket.is_none() && n >= 0.0 => Ok(f32_lit(n / 100.0, span)),
            _ => match &bracket {
                Some(inner) => expr(inner, &class, span),
                None => Err(None),
            },
        },
        // Unsuffixed, so the builder's parameter picks the integer type.
        Value::Int => match (value.parse::<i32>(), &bracket) {
            (Ok(n), _) => Ok(int_lit(n.into(), span)),
            (_, Some(inner)) => expr(inner, &class, span),
            _ => Err(None),
        },
        Value::Count => match (value.parse::<u16>(), &bracket) {
            (Ok(n), _) if n > 0 => Ok(int_lit(n.into(), span)),
            (_, Some(inner)) => expr(inner, &class, span),
            _ => Err(None),
        },
        Value::Degrees => match parse_number(value) {
            // Converted at runtime: a precomputed literal can trip clippy's
            // `approx_constant` in the caller's crate (45 degrees is π/4).
            Some(deg) if bracket.is_none() => {
                let deg = f32_lit(deg, span);
                Ok(quote_spanned!(span=> #deg.to_radians()))
            }
            _ => match &bracket {
                Some(inner) => {
                    let deg = expr(inner, &class, span)?;
                    Ok(quote_spanned!(span=> (#deg as f32).to_radians()))
                }
                None => Err(None),
            },
        },
        Value::Ratio => match &bracket {
            Some(inner) => match inner.split_once('/') {
                Some((a, b)) => match (parse_number(a.trim()), parse_number(b.trim())) {
                    (Some(a), Some(b)) if b != 0.0 => {
                        let (a, b) = (f32_lit(a, span), f32_lit(b, span));
                        Ok(quote_spanned!(span=> #a / #b))
                    }
                    _ => Err(Some(format!(
                        "class `{class}`: expected `[w/h]`, as in `aspect-[16/9]`"
                    ))),
                },
                None => length(inner, &class, allow_expr, span),
            },
            None => Err(None),
        },
    }
}

fn is_length(inner: &str) -> bool {
    inner.ends_with('%') || parse_number(inner.strip_suffix("px").unwrap_or(inner)).is_some()
}

/// `[320px]`, `[1.5]`, or `[expr]`.
fn length(
    inner: &str,
    class: &str,
    allow_expr: bool,
    span: Span,
) -> Result<TokenStream2, Option<String>> {
    if inner.ends_with('%') {
        return Err(Some(format!(
            "class `{class}`: percent lengths are not supported; builder sizes are points \
             (use a `-full` class for 100%)"
        )));
    }
    let number = inner.strip_suffix("px").unwrap_or(inner);
    match parse_number(number) {
        Some(n) => Ok(f32_lit(n, span)),
        None if allow_expr => expr(inner, class, span),
        None => Err(None),
    }
}

fn expr(inner: &str, class: &str, span: Span) -> Result<TokenStream2, Option<String>> {
    match syn::parse_str::<syn::Expr>(inner) {
        Ok(e) => Ok(respan(quote!((#e)), span)),
        Err(e) => Err(Some(format!(
            "class `{class}`: `[{inner}]` is neither a number nor a Rust expression: {e}"
        ))),
    }
}

fn parse_number(s: &str) -> Option<f32> {
    if s.is_empty()
        || !s
            .bytes()
            .all(|b| b.is_ascii_digit() || b == b'.' || b == b'-')
    {
        return None;
    }
    s.parse::<f32>().ok().filter(|n| n.is_finite())
}

fn f32_lit(n: f32, span: Span) -> TokenStream2 {
    let lit = proc_macro2::Literal::f32_suffixed(n);
    let mut lit = lit;
    lit.set_span(span);
    quote!(#lit)
}

fn int_lit(n: i64, span: Span) -> TokenStream2 {
    let mut lit = proc_macro2::Literal::i64_unsuffixed(n);
    lit.set_span(span);
    quote!(#lit)
}

fn parse_hex(hex: &str) -> Option<[u8; 4]> {
    let digit = |i: usize| u8::from_str_radix(hex.get(i..i + 1)?, 16).ok();
    let byte = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    match hex.len() {
        3 => Some([digit(0)? * 17, digit(1)? * 17, digit(2)? * 17, 255]),
        6 => Some([byte(0)?, byte(2)?, byte(4)?, 255]),
        8 => Some([byte(0)?, byte(2)?, byte(4)?, byte(6)?]),
        _ => None,
    }
}

fn unknown_class(class: &str, base: &str) -> String {
    // Candidates: every keyword, and every family prefix with the value
    // the user wrote, so `gpa-2` finds `gap-2`.
    let value = base.rsplit_once('-').map(|(_, v)| v);
    let mut candidates: Vec<String> = KEYWORDS.iter().map(|k| k.class.to_owned()).collect();
    if let Some(value) = value {
        candidates.extend(FAMILIES.iter().map(|f| format!("{}-{value}", f.prefix)));
    }
    // A builder method name (`bold`) suggests the class that calls it.
    let by_method = KEYWORDS
        .iter()
        .find(|k| k.method == base.replace('-', "_"))
        .map(|k| k.class);
    let hint = by_method
        .or_else(|| closest(base, candidates.iter().map(String::as_str)))
        .map(|s| format!("; did you mean `{s}`?"))
        .unwrap_or_default();
    format!("unknown class `{class}`{hint} See the class reference in docs/guide/writing-views.md")
}

/// `__class_vocabulary!()`: every class applied to the builder it names,
/// for the test that checks the table against the builder API.
pub(crate) fn vocabulary_check() -> TokenStream2 {
    let span = Span::call_site();
    let mut by_on: Vec<(On, Vec<TokenStream2>)> =
        vec![(On::Box, vec![]), (On::Text, vec![]), (On::Icon, vec![])];
    let mut hover = Vec::new();
    for k in KEYWORDS {
        let mut out = Lowered::default();
        lower_one(k.class, span, Some(k.on), &mut out).expect("keywords lower");
        by_on
            .iter_mut()
            .find(|(on, _)| *on == k.on)
            .unwrap()
            .1
            .extend(out.calls);
    }
    for f in FAMILIES {
        let mut out = Lowered::default();
        let class = format!("{}-{}", f.prefix, sample_value(f.value));
        lower_one(&class, span, Some(f.on), &mut out)
            .unwrap_or_else(|e| panic!("sample class {class}: {e}"));
        by_on
            .iter_mut()
            .find(|(on, _)| *on == f.on)
            .unwrap()
            .1
            .extend(out.calls);
        if f.hover.is_some() {
            let mut out = Lowered::default();
            lower_one(&format!("hover:{class}"), span, Some(f.on), &mut out).expect("hover lowers");
            hover.extend(out.hover);
        }
    }
    let [(_, boxed), (_, text), (_, icon)] = <[_; 3]>::try_from(by_on).ok().unwrap();
    quote! {{
        let _ = div() #(#boxed)* .hover(|__quark_hover| __quark_hover #(#hover)*);
        let _ = text("") #(#text)*;
        let _ = svg_icon("", 16.0) #(#icon)*;
    }}
}

fn sample_value(value: Value) -> &'static str {
    match value {
        Value::Spacing | Value::Count | Value::Int | Value::Px => "2",
        Value::Length => "[3px]",
        Value::Percent => "50",
        Value::Degrees => "45",
        Value::Ratio => "[4/3]",
        Value::Color => "[#336699]",
    }
}

/// The class reference as Markdown, for docs/guide/writing-views.md.
#[cfg(test)]
fn reference_markdown() -> String {
    let mut out = String::from("| Class | Builder call | On |\n|---|---|---|\n");
    for k in KEYWORDS {
        let args = k.args.replace("::quark::", "");
        out.push_str(&format!(
            "| `{}` | `.{}({args})` | `<{}>` |\n",
            k.class,
            k.method,
            k.on.tag()
        ));
    }
    out.push_str("\n| Family | Builder call | Values | On | `hover:` |\n|---|---|---|---|---|\n");
    for f in FAMILIES {
        let values = match f.value {
            Value::Spacing => "`N` (N x 4px, halves allowed), `px`, `[13px]`, `[expr]`",
            Value::Length => "`[3px]`, `[1.5]`, `[expr]`",
            Value::Px => "`N` points, `[3px]`, `[expr]`",
            Value::Percent => "`N` (N / 100), `[expr]`",
            Value::Int => "`N`, `[expr]`",
            Value::Count => "`N` (N > 0), `[expr]`",
            Value::Degrees => "`N` degrees, `[expr]`",
            Value::Ratio => "`[w/h]`, `[expr]`",
            Value::Color => {
                "`transparent`, `white`, `black`, `[#rgb]`, `[#rrggbb]`, `[#rrggbbaa]`, `[expr]`"
            }
        };
        let hover = f
            .hover
            .map(|h| format!("`.{h}(..)`"))
            .unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "| `{}-*` | `.{}(..)` | {values} | `<{}>` | {hover} |\n",
            f.prefix,
            f.method,
            f.on.tag()
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lowered(class: &str, on: On) -> Result<String, String> {
        let mut errors = Vec::new();
        let out = lower(class, Span::call_site(), Some(on), &mut errors);
        if let Some(e) = errors.first() {
            return Err(e.to_string());
        }
        let mut s: String = out
            .calls
            .iter()
            .map(|c| c.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        if !out.hover.is_empty() {
            let h: Vec<String> = out.hover.iter().map(|c| c.to_string()).collect();
            s.push_str(&format!(" hover[{}]", h.join(" ")));
        }
        Ok(s.replace(' ', ""))
    }

    // Catches a regression in how values become builder arguments: the
    // spacing scale, arbitrary values, colors, and family disambiguation.
    #[test]
    fn classes_lower_to_builder_calls() {
        let cases: &[(&str, On, &str)] = &[
            (
                "flex-row items-center",
                On::Box,
                ".flex_row().items_center()",
            ),
            (
                "p-4 gap-x-2 px-0.5",
                On::Box,
                ".p(16f32).gap_x(8f32).px(2f32)",
            ),
            (
                "w-[320px] gap-[6] h-px",
                On::Box,
                ".w(320f32).gap(6f32).h(1f32)",
            ),
            ("w-[sidebar_w]", On::Box, ".w((sidebar_w))"),
            (
                "bg-[Color::rgba(1, 2, 3, 4)]",
                On::Box,
                ".bg((Color::rgba(1,2,3,4)))",
            ),
            (
                "bg-[#ff8000]",
                On::Box,
                ".bg(::quark::Color::rgba(255u8,128u8,0u8,255u8))",
            ),
            ("bg-[#f00a]", On::Box, ""),
            (
                "opacity-50 z-10 grid-cols-3",
                On::Box,
                ".opacity(0.5f32).z_index(10).grid_cols_n(3)",
            ),
            (
                "text-sm text-[14px] text-[c.text]",
                On::Text,
                ".text_sm().size(14f32).color((c.text))",
            ),
            ("aspect-[16/9] rotate-45", On::Box, ".aspect_ratio(16f32/9f32).rotate(45f32.to_radians())"),
            (
                "hover:bg-[c] hover:opacity-80",
                On::Box,
                "hover[.bg((c)).opacity(0.8f32)]",
            ),
        ];
        for (class, on, want) in cases {
            match lowered(class, *on) {
                Ok(got) if !want.is_empty() => assert_eq!(&got, want, "{class}"),
                Ok(got) => panic!("{class} lowered to {got}, expected an error"),
                Err(e) if want.is_empty() => assert!(e.contains("is not a #rgb"), "{e}"),
                Err(e) => panic!("{class}: {e}"),
            }
        }
    }

    // Catches unhelpful or missing diagnostics for classes that cannot lower.
    #[test]
    fn rejected_classes_explain_themselves() {
        let cases: &[(&str, On, &str)] = &[
            ("itmes-center", On::Box, "did you mean `items-center`?"),
            ("gpa-2", On::Box, "did you mean `gap-2`?"),
            ("bold", On::Text, "did you mean `font-bold`?"),
            (
                "md:flex-row",
                On::Box,
                "responsive breakpoints (`md:`) are not supported",
            ),
            ("focus:bg-[c]", On::Box, "`focus:` is not supported"),
            ("dark:bg-[c]", On::Box, "`dark:` is not supported"),
            ("hover:p-4", On::Box, "`hover:` restyles only"),
            (
                "text-sm",
                On::Box,
                "styles <text>; it does not apply to <div>",
            ),
            ("w-1/2", On::Box, "fractions need percent sizes"),
            ("w-[50%]", On::Box, "percent lengths are not supported"),
        ];
        for (class, on, want) in cases {
            let err = lowered(class, *on).expect_err(class);
            assert!(err.contains(want), "{class}: {err}");
        }
    }

    // Catches the docs reference drifting from the tables. Run with
    // QUARK_BLESS=1 to rewrite it.
    #[test]
    fn docs_reference_matches_the_tables() {
        let path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/guide/writing-views.md"
        );
        let Ok(doc) = std::fs::read_to_string(path) else {
            return; // Packaged crate: the docs are not shipped.
        };
        let (start, end) = (
            "<!-- class-reference:start -->\n",
            "<!-- class-reference:end -->",
        );
        let a = doc.find(start).expect("start marker") + start.len();
        let b = doc.find(end).expect("end marker");
        let want = reference_markdown();
        if doc[a..b] != want {
            if std::env::var_os("QUARK_BLESS").is_some() {
                std::fs::write(path, format!("{}{want}{}", &doc[..a], &doc[b..])).unwrap();
            } else {
                panic!("class reference in {path} is stale; rerun with QUARK_BLESS=1");
            }
        }
    }
}
