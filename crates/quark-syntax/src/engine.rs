//! Parsing and query evaluation.
//! Parsers and compiled queries are cached per thread, so a worker thread
//! compiles each language once.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use tree_sitter as ts;
use tree_sitter::StreamingIterator;

use crate::{HighlightKind, HighlightSpan, LanguageId};

struct CompiledLanguage {
    query: ts::Query,
    capture_kinds: Vec<HighlightKind>,
}

thread_local! {
    /// `None` records a language whose query failed to compile, so it is
    /// not retried on every block.
    static COMPILED: RefCell<HashMap<LanguageId, Option<Rc<CompiledLanguage>>>> =
        RefCell::new(HashMap::new());
    static PARSERS: RefCell<HashMap<LanguageId, ts::Parser>> = RefCell::new(HashMap::new());
}

/// Grammar and highlight query of a compiled-in language.
fn grammar(language: LanguageId) -> Option<(ts::Language, String)> {
    match language {
        #[cfg(feature = "rust")]
        LanguageId::Rust => Some((
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY.to_owned(),
        )),
        #[cfg(feature = "javascript")]
        LanguageId::JavaScript => Some((
            tree_sitter_javascript::LANGUAGE.into(),
            [
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ]
            .join("\n"),
        )),
        // TypeScript's query only covers what it adds to JavaScript; the
        // grammar repository layers it over JavaScript's query.
        #[cfg(feature = "typescript")]
        LanguageId::TypeScript => Some((
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            [
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ]
            .join("\n"),
        )),
        #[cfg(feature = "python")]
        LanguageId::Python => Some((
            tree_sitter_python::LANGUAGE.into(),
            tree_sitter_python::HIGHLIGHTS_QUERY.to_owned(),
        )),
        #[cfg(feature = "bash")]
        LanguageId::Bash => Some((
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY.to_owned(),
        )),
        #[cfg(feature = "json")]
        LanguageId::Json => Some((
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY.to_owned(),
        )),
        #[cfg(feature = "go")]
        LanguageId::Go => Some((
            tree_sitter_go::LANGUAGE.into(),
            tree_sitter_go::HIGHLIGHTS_QUERY.to_owned(),
        )),
        #[allow(unreachable_patterns)]
        _ => None,
    }
}

pub(crate) fn highlight(language: LanguageId, source: &str) -> Vec<HighlightSpan> {
    if source.is_empty() {
        return Vec::new();
    }
    let Some((ts_language, compiled)) = compiled(language) else {
        return Vec::new();
    };
    let Some(tree) = parse(language, &ts_language, source) else {
        return Vec::new();
    };
    compact_spans(collect_spans(&compiled, &tree, source))
}

fn compiled(language: LanguageId) -> Option<(ts::Language, Rc<CompiledLanguage>)> {
    let (ts_language, query_source) = grammar(language)?;
    let compiled = COMPILED.with(|cache| {
        cache
            .borrow_mut()
            .entry(language)
            .or_insert_with(|| {
                let query = ts::Query::new(&ts_language, &query_source).ok()?;
                let capture_kinds = query
                    .capture_names()
                    .iter()
                    .map(|name| capture_name_to_highlight_kind(name))
                    .collect();
                Some(Rc::new(CompiledLanguage {
                    query,
                    capture_kinds,
                }))
            })
            .clone()
    })?;
    Some((ts_language, compiled))
}

fn parse(language: LanguageId, ts_language: &ts::Language, source: &str) -> Option<ts::Tree> {
    PARSERS.with(|cache| {
        let mut cache = cache.borrow_mut();
        let parser = match cache.entry(language) {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let mut parser = ts::Parser::new();
                parser.set_language(ts_language).ok()?;
                entry.insert(parser)
            }
        };
        parser.parse(source, None)
    })
}

/// `(start, end, kind, pattern)` for every capture with a highlight kind.
fn collect_spans(
    compiled: &CompiledLanguage,
    tree: &ts::Tree,
    source: &str,
) -> Vec<(usize, usize, HighlightKind, usize)> {
    let mut cursor = ts::QueryCursor::new();
    let mut captures = cursor.captures(&compiled.query, tree.root_node(), source.as_bytes());
    let mut raw = Vec::new();
    while let Some((query_match, capture_index)) = captures.next() {
        let capture = query_match.captures[*capture_index];
        let kind = compiled
            .capture_kinds
            .get(capture.index as usize)
            .copied()
            .unwrap_or_default();
        let (start, end) = (capture.node.start_byte(), capture.node.end_byte());
        if kind != HighlightKind::Normal && end > start {
            raw.push((start, end, kind, query_match.pattern_index));
        }
    }
    raw
}

/// Resolves overlapping captures into sorted, disjoint spans: at the same
/// start the later query pattern wins (generic `(identifier) @variable`
/// rules come first in the queries), and a span starting inside an earlier
/// one is dropped.
fn compact_spans(mut raw: Vec<(usize, usize, HighlightKind, usize)>) -> Vec<HighlightSpan> {
    raw.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.3.cmp(&a.3)));
    let mut covered = 0usize;
    let mut spans = Vec::with_capacity(raw.len());
    for (start, end, kind, _) in raw {
        if start < covered {
            continue;
        }
        spans.push(HighlightSpan {
            offset: u32::try_from(start).unwrap_or(u32::MAX),
            length: u32::try_from(end - start).unwrap_or(u32::MAX),
            kind,
        });
        covered = end;
    }
    spans
}

/// Maps tree-sitter capture names (nvim and grammar-repository
/// conventions) to highlight kinds; unknown names are `Normal`.
fn capture_name_to_highlight_kind(name: &str) -> HighlightKind {
    if name.starts_with("variable.builtin") || name.starts_with("function.builtin") {
        HighlightKind::Builtin
    } else if name.starts_with("variable.member") {
        HighlightKind::Property
    } else if name.starts_with("function") {
        HighlightKind::Function
    } else if name.starts_with("module.builtin") {
        HighlightKind::Builtin
    } else if name.starts_with("module") {
        HighlightKind::Namespace
    } else if name.starts_with("keyword") {
        HighlightKind::Keyword
    } else if name.starts_with("string")
        || name.starts_with("escape")
        || name.starts_with("character")
    {
        HighlightKind::String
    } else if name.starts_with("comment") {
        HighlightKind::Comment
    } else if name.starts_with("number") {
        HighlightKind::Number
    } else if name.starts_with("type") || name.starts_with("constructor") {
        HighlightKind::Type
    } else if name.starts_with("operator") {
        HighlightKind::Operator
    } else if name.starts_with("punctuation") {
        HighlightKind::Punctuation
    } else if name.starts_with("variable") || name.starts_with("parameter") {
        HighlightKind::Variable
    } else if name.starts_with("constant") || name.starts_with("boolean") {
        HighlightKind::Constant
    } else if name.starts_with("builtin") {
        HighlightKind::Builtin
    } else if name.starts_with("attribute") {
        HighlightKind::Attribute
    } else if name.starts_with("tag") {
        HighlightKind::Tag
    } else if name.starts_with("property") {
        HighlightKind::Property
    } else if name.starts_with("namespace") {
        HighlightKind::Namespace
    } else if name.starts_with("label") {
        HighlightKind::Label
    } else if name.starts_with("preproc") {
        HighlightKind::Preprocessor
    } else {
        HighlightKind::Normal
    }
}
