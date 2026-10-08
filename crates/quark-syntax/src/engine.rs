//! Loading grammars from packs, parsing, and query evaluation, including
//! languages embedded in others (tree-sitter injections).
//!
//! A [`Grammar`] is shared by every thread; parsers are not `Sync`, so each
//! thread keeps its own per grammar.
//!
//! [`highlight`] parses the host layer, runs its grammar's injection query
//! to find embedded regions, and parses each region as a layer of its own
//! over the original source (tree-sitter included ranges), so every node
//! keeps its byte and row coordinates. Layers are processed breadth first,
//! one parse at a time, so one parser per grammar serves every layer.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tree_sitter as ts;
use tree_sitter::StreamingIterator;

use crate::pack::{PackError, PackManifest, supported_abi, verify_files};
use crate::store::Tag;
use crate::{HighlightKind, HighlightSpan, LanguageId};

/// A loaded language: its tree-sitter grammar, compiled highlight query,
/// and compiled injection query when its pack has one.
pub(crate) struct Grammar {
    id: u64,
    language: ts::Language,
    query: ts::Query,
    capture_kinds: Vec<HighlightKind>,
    injections: Option<Injections>,
}

static NEXT_ID: AtomicU64 = AtomicU64::new(1);

thread_local! {
    static PARSERS: RefCell<HashMap<u64, ts::Parser>> = RefCell::new(HashMap::new());
}

impl Grammar {
    /// Verifies the pack's files against `manifest` (already validated),
    /// loads its library, and compiles its highlight and injection queries.
    ///
    /// The library stays loaded for the rest of the process: tree-sitter
    /// languages, parsers, and trees on other threads point into it, and
    /// nothing tracks when the last of them is gone.
    pub(crate) fn load(dir: &Path, manifest: &PackManifest) -> Result<Self, PackError> {
        verify_files(dir, manifest)?;
        let highlights = std::fs::read_to_string(dir.join(&manifest.highlights.path))?;
        let injections = match &manifest.injections {
            Some(file) => Some(std::fs::read_to_string(dir.join(&file.path))?),
            None => None,
        };
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
        let injections = injections
            .map(|text| Injections::compile(&language, &text))
            .transpose()?;
        Ok(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            language,
            query,
            capture_kinds,
            injections,
        })
    }

    /// Parses `source`, reading only `ranges` when there are any.
    fn parse(&self, source: &str, ranges: &[ts::Range]) -> Option<ts::Tree> {
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
            // An empty list resets the parser to the whole document.
            parser.set_included_ranges(ranges).ok()?;
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

/// A grammar's compiled injection query and what each pattern sets, per
/// <https://tree-sitter.github.io/tree-sitter/3-syntax-highlighting.html#language-injection>.
struct Injections {
    query: ts::Query,
    /// `@injection.content`: the nodes whose text is the embedded document.
    content: Option<u32>,
    /// `@injection.language`: a node whose text names the language.
    language: Option<u32>,
    patterns: Vec<InjectionPattern>,
}

#[derive(Default)]
struct InjectionPattern {
    /// `#set! injection.language "name"`.
    language: Option<LanguageId>,
    /// `injection.combined`: every match of the pattern (per language) is
    /// one document, as for template fragments split by interpolations.
    combined: bool,
    /// `injection.include-children`: the content nodes' children are part
    /// of the document; by default their named children's text is cut out
    /// of it.
    include_children: bool,
    /// `injection.self`: the language of the layer the match is in.
    itself: bool,
    /// `injection.parent`: the language of the layer that embedded that one.
    parent: bool,
}

impl Injections {
    fn compile(language: &ts::Language, text: &str) -> Result<Self, PackError> {
        let query =
            ts::Query::new(language, text).map_err(|e| PackError::InjectionQuery(e.to_string()))?;
        let patterns = (0..query.pattern_count())
            .map(|index| {
                let mut pattern = InjectionPattern::default();
                for property in query.property_settings(index) {
                    match &*property.key {
                        "injection.language" => {
                            pattern.language =
                                property.value.as_deref().and_then(LanguageId::from_fence);
                        }
                        "injection.combined" => pattern.combined = true,
                        "injection.include-children" => pattern.include_children = true,
                        "injection.self" => pattern.itself = true,
                        "injection.parent" => pattern.parent = true,
                        _ => {}
                    }
                }
                pattern
            })
            .collect();
        Ok(Self {
            content: query.capture_index_for_name("injection.content"),
            language: query.capture_index_for_name("injection.language"),
            patterns,
            query,
        })
    }

    /// The embedded documents in `tree`: each one's language and its
    /// ranges, clipped to `parent` (the ranges of the layer `tree` parsed).
    fn find(
        &self,
        tree: &ts::Tree,
        source: &str,
        parent: &[ts::Range],
    ) -> Vec<(Target, Vec<ts::Range>)> {
        let Some(content) = self.content else {
            return Vec::new();
        };
        let mut found = Vec::new();
        // One document per pattern and language, in order of first match.
        let mut combined: Vec<(usize, Target, Vec<ts::Node<'_>>)> = Vec::new();
        let mut cursor = ts::QueryCursor::new();
        let mut matches = cursor.matches(&self.query, tree.root_node(), source.as_bytes());
        while let Some(found_match) = matches.next() {
            let pattern = &self.patterns[found_match.pattern_index];
            let mut nodes = Vec::new();
            let mut named = None;
            for capture in found_match.captures {
                if capture.index == content {
                    nodes.push(capture.node);
                } else if Some(capture.index) == self.language {
                    named = capture
                        .node
                        .utf8_text(source.as_bytes())
                        .ok()
                        .and_then(|text| LanguageId::from_fence(text.trim()));
                }
            }
            // A captured name wins over the pattern's properties, as in
            // tree-sitter-highlight.
            let target = match named.or_else(|| pattern.language.clone()) {
                Some(language) => Target::Named(language),
                None if pattern.itself => Target::Itself,
                None if pattern.parent => Target::Parent,
                None => continue,
            };
            if nodes.is_empty() {
                continue;
            }
            if pattern.combined {
                let index = found_match.pattern_index;
                match combined
                    .iter_mut()
                    .find(|(i, t, _)| *i == index && *t == target)
                {
                    Some((_, _, all)) => all.extend(nodes),
                    None => combined.push((index, target, nodes)),
                }
            } else {
                found.push((
                    target,
                    content_ranges(&nodes, pattern.include_children, parent),
                ));
            }
        }
        for (index, target, nodes) in combined {
            let include_children = self.patterns[index].include_children;
            found.push((target, content_ranges(&nodes, include_children, parent)));
        }
        found.retain(|(_, ranges)| !ranges.is_empty());
        found
    }
}

/// The language an injection asks for.
#[derive(Debug, PartialEq, Eq)]
enum Target {
    Named(LanguageId),
    Itself,
    Parent,
}

/// The ranges of `nodes` (without their children's unless
/// `include_children`), sorted, merged, and clipped to `parent`.
fn content_ranges(
    nodes: &[ts::Node<'_>],
    include_children: bool,
    parent: &[ts::Range],
) -> Vec<ts::Range> {
    let mut own = Vec::new();
    for node in nodes {
        if include_children {
            own.push(node.range());
            continue;
        }
        let mut at = (node.start_byte(), node.start_position());
        // Named children only, as Neovim does: grammars like Markdown's
        // block grammar give content nodes anonymous punctuation children
        // that are part of the embedded text, while named ones (a block
        // quote's `> ` continuation) are not.
        let mut cursor = node.walk();
        for child in node.named_children(&mut cursor) {
            if child.start_byte() > at.0 {
                own.push(range(at, (child.start_byte(), child.start_position())));
            }
            if child.end_byte() > at.0 {
                at = (child.end_byte(), child.end_position());
            }
        }
        if node.end_byte() > at.0 {
            own.push(range(at, (node.end_byte(), node.end_position())));
        }
    }
    own.sort_by_key(|r| r.start_byte);
    // Tree-sitter wants disjoint ranges; nested content nodes overlap.
    let mut merged: Vec<ts::Range> = Vec::with_capacity(own.len());
    for next in own {
        match merged.last_mut() {
            Some(last) if next.start_byte <= last.end_byte => {
                if next.end_byte > last.end_byte {
                    last.end_byte = next.end_byte;
                    last.end_point = next.end_point;
                }
            }
            _ => merged.push(next),
        }
    }
    intersect(&merged, parent)
}

fn range(start: (usize, ts::Point), end: (usize, ts::Point)) -> ts::Range {
    ts::Range {
        start_byte: start.0,
        end_byte: end.0,
        start_point: start.1,
        end_point: end.1,
    }
}

/// The overlap of two sorted, disjoint range lists.
fn intersect(a: &[ts::Range], b: &[ts::Range]) -> Vec<ts::Range> {
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while let (Some(x), Some(y)) = (a.get(i), b.get(j)) {
        let start = if x.start_byte >= y.start_byte { x } else { y };
        let end = if x.end_byte <= y.end_byte { x } else { y };
        if start.start_byte < end.end_byte {
            out.push(range(
                (start.start_byte, start.start_point),
                (end.end_byte, end.end_point),
            ));
        }
        if x.end_byte <= y.end_byte {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// Bounds on the embedded layers of one highlight, so a deeply nested or
/// adversarial source costs a bounded amount of parsing.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Limits {
    /// Nesting below the host layer: 1 allows embedded languages but not
    /// languages embedded in those.
    pub(crate) depth: u32,
    /// Embedded layers in total.
    pub(crate) layers: usize,
    /// Bytes parsed across embedded layers.
    pub(crate) bytes: usize,
}

impl Limits {
    pub(crate) fn for_source(len: usize) -> Self {
        Self {
            depth: 8,
            // One per paragraph of a long Markdown document.
            layers: 4096,
            bytes: len.saturating_mul(4).saturating_add(64 << 10),
        }
    }
}

/// The result of [`highlight`].
#[derive(Debug, Default)]
pub(crate) struct Highlights {
    pub(crate) spans: Vec<HighlightSpan>,
    /// Embedded languages whose grammars are still arriving; their regions
    /// keep the host's colors until they do.
    pub(crate) unresolved: Vec<LanguageId>,
    /// A limit left embedded regions unparsed.
    pub(crate) truncated: bool,
}

/// One parse: a grammar over some ranges of the source.
struct Layer {
    grammar: Arc<Grammar>,
    /// The grammar of the layer that embedded this one.
    parent: Option<Arc<Grammar>>,
    ranges: Vec<ts::Range>,
    depth: u32,
}

/// `(start, end, kind)`: a span before conversion to [`HighlightSpan`].
type Raw = (usize, usize, HighlightKind);

/// Highlights `source` with `root` and every language embedded in it,
/// looking embedded languages up with `resolve` (which returns shared
/// handles, so no store lock is held while parsing). Deeper layers take
/// precedence over the host spans they overlap.
pub(crate) fn highlight(
    root: &Arc<Grammar>,
    source: &str,
    limits: Limits,
    resolve: &mut dyn FnMut(&LanguageId) -> Tag,
) -> Highlights {
    let mut out = Highlights::default();
    if source.is_empty() {
        return out;
    }
    let whole = range((0, ts::Point::default()), (source.len(), end_point(source)));
    // A grammar over ranges it already parsed would embed the same layers
    // again, forever (a self-injection of a whole node, or two languages
    // embedding each other over the same text).
    let mut seen: HashSet<(u64, Vec<(usize, usize)>)> = HashSet::new();
    seen.insert((root.id, vec![(0, source.len())]));
    let mut queue = VecDeque::from([Layer {
        grammar: root.clone(),
        parent: None,
        ranges: vec![whole],
        depth: 0,
    }]);
    let (mut layers, mut bytes) = (0usize, 0usize);
    let mut merged: Vec<Raw> = Vec::new();
    while let Some(layer) = queue.pop_front() {
        let included = if layer.depth == 0 {
            &[][..]
        } else {
            &layer.ranges
        };
        let Some(tree) = layer.grammar.parse(source, included) else {
            continue;
        };
        // A node spanning several ranges (an embedded document's root) also
        // covers the host text between them, which stays the host's.
        let spans = clip(
            compact_spans(layer.grammar.collect_spans(&tree, source)),
            &layer.ranges,
        );
        // Breadth first, so layers arrive shallowest first and each deeper
        // one overrides what it covers.
        merged = overlay(merged, &spans);
        let Some(injections) = &layer.grammar.injections else {
            continue;
        };
        for (target, ranges) in injections.find(&tree, source, &layer.ranges) {
            if layer.depth >= limits.depth {
                out.truncated = true;
                break;
            }
            let grammar = match target {
                Target::Itself => layer.grammar.clone(),
                Target::Parent => match &layer.parent {
                    Some(parent) => parent.clone(),
                    None => continue,
                },
                Target::Named(language) => match resolve(&language) {
                    Tag::Ready(grammar) => grammar,
                    Tag::Pending => {
                        if !out.unresolved.contains(&language) {
                            out.unresolved.push(language);
                        }
                        continue;
                    }
                    Tag::Unavailable => continue,
                },
            };
            let key = (
                grammar.id,
                ranges.iter().map(|r| (r.start_byte, r.end_byte)).collect(),
            );
            if seen.contains(&key) {
                continue;
            }
            let len: usize = ranges.iter().map(|r| r.end_byte - r.start_byte).sum();
            if layers >= limits.layers || bytes.saturating_add(len) > limits.bytes {
                out.truncated = true;
                continue;
            }
            seen.insert(key);
            layers += 1;
            bytes += len;
            queue.push_back(Layer {
                grammar,
                parent: Some(layer.grammar.clone()),
                ranges,
                depth: layer.depth + 1,
            });
        }
    }
    out.spans = finish(merged, source);
    out
}

/// The row and byte column just past the end of `source`.
fn end_point(source: &str) -> ts::Point {
    let row = source.bytes().filter(|&b| b == b'\n').count();
    let column = source.len() - source.rfind('\n').map_or(0, |i| i + 1);
    ts::Point { row, column }
}

/// Resolves overlapping captures into sorted, disjoint spans: at the same
/// start the later query pattern wins (generic `(identifier) @variable`
/// rules come first in the queries), and a span starting inside an earlier
/// one is dropped.
fn compact_spans(mut raw: Vec<(usize, usize, HighlightKind, usize)>) -> Vec<Raw> {
    raw.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.3.cmp(&a.3)));
    let mut covered = 0usize;
    let mut spans = Vec::with_capacity(raw.len());
    for (start, end, kind, _) in raw {
        if start < covered {
            continue;
        }
        spans.push((start, end, kind));
        covered = end;
    }
    spans
}

/// The parts of sorted, disjoint `spans` inside sorted, disjoint `ranges`.
fn clip(spans: Vec<Raw>, ranges: &[ts::Range]) -> Vec<Raw> {
    let mut out = Vec::with_capacity(spans.len());
    let mut first = 0;
    for (start, end, kind) in spans {
        while ranges.get(first).is_some_and(|r| r.end_byte <= start) {
            first += 1;
        }
        for r in ranges[first..].iter().take_while(|r| r.start_byte < end) {
            let (start, end) = (start.max(r.start_byte), end.min(r.end_byte));
            if start < end {
                out.push((start, end, kind));
            }
        }
    }
    out
}

/// `top` laid over `base` (both sorted and disjoint): base spans are cut
/// where top spans overlap them, and the result is sorted and disjoint.
fn overlay(base: Vec<Raw>, top: &[Raw]) -> Vec<Raw> {
    if top.is_empty() {
        return base;
    }
    let mut out = Vec::with_capacity(base.len() + top.len() * 2);
    let mut first = 0;
    for (start, end, kind) in base {
        while top.get(first).is_some_and(|t| t.1 <= start) {
            first += 1;
        }
        let mut at = start;
        for t in top[first..].iter().take_while(|t| t.0 < end) {
            if t.0 > at {
                out.push((at, t.0, kind));
            }
            at = at.max(t.1);
        }
        if at < end {
            out.push((at, end, kind));
        }
    }
    out.extend_from_slice(top);
    out.sort_unstable_by_key(|span| span.0);
    out
}

/// Spans as the API returns them: on char boundaries (embedded ranges
/// come from nodes, so this only guards against a misbehaving grammar),
/// with offsets saturating for sources past 4 GiB.
fn finish(spans: Vec<Raw>, source: &str) -> Vec<HighlightSpan> {
    spans
        .into_iter()
        .filter_map(|(start, end, kind)| {
            let start = source.ceil_char_boundary(start);
            let end = source.floor_char_boundary(end);
            (start < end).then(|| HighlightSpan {
                offset: u32::try_from(start).unwrap_or(u32::MAX),
                length: u32::try_from(end - start).unwrap_or(u32::MAX),
                kind,
            })
        })
        .collect()
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
