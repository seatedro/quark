//! Loading grammars from packs, parsing, and query evaluation.
//!
//! A [`Grammar`] is shared by every thread; parsers are not `Sync`, so each
//! thread keeps its own per grammar.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use tree_sitter as ts;
use tree_sitter::StreamingIterator;

use crate::pack::{PackError, PackManifest, supported_abi, verify_files};
use crate::{HighlightKind, HighlightSpan};

/// A loaded language: its tree-sitter grammar and compiled highlight query.
pub(crate) struct Grammar {
    id: u64,
    language: ts::Language,
    query: ts::Query,
    capture_kinds: Vec<HighlightKind>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static PARSERS: RefCell<HashMap<u64, ts::Parser>> = RefCell::new(HashMap::new());
}

impl Grammar {
    /// Verifies the pack's files against `manifest` (already validated),
    /// loads its library, and compiles its highlight query.
    ///
    /// The library stays loaded for the rest of the process: tree-sitter
    /// languages, parsers, and trees on other threads point into it, and
    /// nothing tracks when the last of them is gone.
    pub(crate) fn load(dir: &Path, manifest: &PackManifest) -> Result<Self, PackError> {
        verify_files(dir, manifest)?;
        let highlights = std::fs::read_to_string(dir.join(&manifest.highlights.path))?;
        let library_path = dir.join(&manifest.library.path);
        // SAFETY: loading a library runs its initializers, and calling the
        // symbol runs its code. The pack passed its SHA-256 check against a
        // manifest that is either signed (downloaded packs) or in a
        // directory the app opted into (local packs); see the threat model
        // in the crate docs.
        let language = unsafe {
            let library = libloading::Library::new(&library_path)
                .map_err(|e| PackError::Library(e.to_string()))?;
            let library: &'static libloading::Library = Box::leak(Box::new(library));
            let mut name = manifest.symbol.clone().into_bytes();
            name.push(0);
            let symbol = library
                .get::<unsafe extern "C" fn() -> *const ()>(&name)
                .map_err(|e| PackError::Library(e.to_string()))?;
            ts::Language::new(tree_sitter_language::LanguageFn::from_raw(*symbol))
        };
        // The manifest's ABI was checked; this checks the library agrees.
        let abi = language.abi_version() as u32;
        if abi != manifest.abi || !supported_abi().contains(&abi) {
            let range = supported_abi();
            return Err(PackError::Abi {
                abi,
                min: *range.start(),
                max: *range.end(),
            });
        }
        let query =
            ts::Query::new(&language, &highlights).map_err(|e| PackError::Query(e.to_string()))?;
        let capture_kinds = query
            .capture_names()
            .iter()
            .map(|name| capture_name_to_highlight_kind(name))
            .collect();
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            language,
            query,
            capture_kinds,
        })
    }

    pub(crate) fn highlight(&self, source: &str) -> Vec<HighlightSpan> {
        if source.is_empty() {
            return Vec::new();
        }
        let Some(tree) = self.parse(source) else {
            return Vec::new();
        };
        compact_spans(self.collect_spans(&tree, source))
    }

    fn parse(&self, source: &str) -> Option<ts::Tree> {
        PARSERS.with(|cache| {
            let mut cache = cache.borrow_mut();
            let parser = match cache.entry(self.id) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let mut parser = ts::Parser::new();
                    parser.set_language(&self.language).ok()?;
                    entry.insert(parser)
                }
            };
            parser.parse(source, None)
        })
    }

    /// `(start, end, kind, pattern)` for every capture with a highlight kind.
    fn collect_spans(
        &self,
        tree: &ts::Tree,
        source: &str,
    ) -> Vec<(usize, usize, HighlightKind, usize)> {
        let mut cursor = ts::QueryCursor::new();
        let mut captures = cursor.captures(&self.query, tree.root_node(), source.as_bytes());
        let mut raw = Vec::new();
        while let Some((query_match, capture_index)) = captures.next() {
            let capture = query_match.captures[*capture_index];
            let kind = self
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
