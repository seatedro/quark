//! Embedded Rust inside `view!` templates, laid out by the pinned rustfmt.
//!
//! Each fragment (an attribute value, a child expression, a `let`, a
//! condition, a match arm head, constructor arguments, a type) goes into a
//! parse-only wrapper that gives it the right grammar context, sits at the
//! template's indentation, and starts after a lead-in as wide as the
//! template's prefix. Many wrappers share one rustfmt run. The result is
//! read back by aligning lexical tokens: rustfmt supplies only the
//! whitespace between tokens, and the token bytes are copied from the
//! original source, so comments and literals are byte-identical by
//! construction. A comma rustfmt adds or drops at the end of a list is
//! undone; any other change makes that fragment keep its source, with the
//! reason.
//!
//! Nested `view!` invocations become placeholders as wide as the child's
//! one-line form (or a multi-line placeholder when it has none), so
//! rustfmt's fit decisions see the child's real size. The template printer
//! then lays the child out at the indentation rustfmt chose, and it is
//! spliced in by token position, never by searching the text.
//!
//! The printer only knows a fragment's placement while printing, so
//! [`RustProvider::prepare`] formats every fragment speculatively in one
//! batch: its one-line probe and its layout where it sits in the source,
//! which is where a formatted file keeps it. A request that misses the
//! cache runs on demand, together with every prepared fragment shifted by
//! the same change of indentation.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::printer::rust::{
    Layout, LayoutLine, LayoutRequest, NestedViews, ProviderError, RustContext, RustFragment,
    RustProvider,
};
use crate::rustfmt::{RustfmtCommand, RustfmtError};
use crate::source::columns;
use crate::trivia::{LexKind, lex as lex_source};

/// An expression with at least this much before it is wrapped as the right
/// side of `_ = `, padded to the prefix width; a shorter prefix (a child's
/// `{`) is closer to a block's tail expression, which has none.
const ASSIGN_MIN: usize = 3;
/// A condition with this much before it follows `} else if `.
const ELSE_IF_PREFIX: usize = "} else if ".len();
/// At most this many wrappers go to one rustfmt process.
const BATCH: usize = 400;
/// At most this many rustfmt processes run at once (a shared machine).
const CONCURRENCY: usize = 2;
/// Views nested deeper than this inside embedded Rust keep their source.
const MAX_NESTING: usize = 8;

/// Counters for performance reports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// `flat` and `layout` requests answered.
    pub requests: usize,
    /// Requests answered from the cache, speculation included.
    pub cache_hits: usize,
    /// rustfmt processes started.
    pub rustfmt_calls: usize,
    /// Wrappers formatted across those processes.
    pub wrappers: usize,
    /// Requests that kept the fragment's source.
    pub failed: usize,
}

/// The rustfmt-backed [`RustProvider`] for one source file.
pub struct RustfmtProvider {
    command: RustfmtCommand,
    widths: RefCell<Option<Result<(usize, usize), String>>>,
    cache: RefCell<HashMap<Key, Result<Layout, String>>>,
    /// Prepared fragments with their speculative placement.
    seeds: RefCell<Vec<Key>>,
    /// Indentation shifts already speculated on.
    shifts: RefCell<HashSet<isize>>,
    stats: Cell<Stats>,
    depth: Cell<usize>,
}

/// A fragment at a placement: everything its layout depends on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct Key {
    context: RustContext,
    text: String,
    original_indent: usize,
    place: Place,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Place {
    indent: usize,
    prefix: usize,
    max_width: usize,
    tab_spaces: usize,
}

impl RustfmtProvider {
    pub fn new(command: RustfmtCommand) -> Self {
        RustfmtProvider {
            command,
            widths: RefCell::new(None),
            cache: RefCell::new(HashMap::new()),
            seeds: RefCell::new(Vec::new()),
            shifts: RefCell::new(HashSet::new()),
            stats: Cell::new(Stats::default()),
            depth: Cell::new(0),
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats.get()
    }

    fn bump(&self, f: impl FnOnce(&mut Stats)) {
        let mut s = self.stats.get();
        f(&mut s);
        self.stats.set(s);
    }

    /// Where one-line forms are probed: the configured width, one level in,
    /// nothing before the fragment.
    fn probe_place(&self) -> Result<Place, String> {
        let widths = self
            .widths
            .borrow_mut()
            .get_or_insert_with(|| self.command.widths().map_err(|e| e.to_string()))
            .clone();
        let (max_width, tab_spaces) = widths?;
        Ok(Place {
            indent: tab_spaces,
            prefix: 0,
            max_width,
            tab_spaces,
        })
    }

    /// A fragment's cache key. Without nested views the text is keyed
    /// with its line breaks re-based to column zero, so a fragment that
    /// only moved, as between the formatter's two passes, is a hit.
    fn key(&self, fragment: &RustFragment<'_>, place: Place) -> Key {
        let original_indent = fragment.original_indent(place.tab_spaces);
        let text = fragment.source();
        match lex(text) {
            Ok(toks) if fragment.nested.is_empty() => {
                let mut normal = String::with_capacity(text.len());
                for (n, t) in toks.iter().enumerate() {
                    if n > 0 {
                        let gap = &text[toks[n - 1].end..t.start];
                        normal.push_str(&rebase_gap(gap, original_indent, "", place.tab_spaces));
                    }
                    normal.push_str(&text[t.start..t.end]);
                }
                Key {
                    context: fragment.context,
                    text: normal,
                    original_indent: 0,
                    place,
                }
            }
            _ => Key {
                context: fragment.context,
                text: text.to_owned(),
                original_indent,
                place,
            },
        }
    }

    /// Formats every key not cached yet in shared rustfmt runs and caches
    /// the layouts. Keys must have no nested views.
    fn fill(&self, keys: Vec<Key>) {
        let mut seen = HashSet::new();
        let todo: Vec<Key> = {
            let cache = self.cache.borrow();
            keys.into_iter()
                .filter(|k| !cache.contains_key(k) && seen.insert(k.clone()))
                .collect()
        };
        if todo.is_empty() {
            return;
        }
        let jobs: Vec<Job<'_>> = todo
            .iter()
            .map(|k| Job {
                key: k,
                nested: Vec::new(),
                placeholders: Vec::new(),
            })
            .collect();
        let results = self.run(&jobs);
        let mut cache = self.cache.borrow_mut();
        for (key, result) in todo.iter().zip(results) {
            let layout = result.and_then(|(p, out)| assemble(&p, &out, &key.text, None));
            cache.insert(key.clone(), layout);
        }
    }

    /// The cached layout for `key`, formatting it first when needed, with
    /// the seeds shifted alike when `speculate` is set.
    fn cached(&self, key: &Key, speculate: bool) -> Result<Layout, String> {
        if let Some(hit) = self.cache.borrow().get(key) {
            self.bump(|s| s.cache_hits += 1);
            return hit.clone();
        }
        let mut keys = vec![key.clone()];
        if speculate {
            keys.extend(self.shifted_seeds(key));
        }
        self.fill(keys);
        self.cache
            .borrow()
            .get(key)
            .cloned()
            .unwrap_or_else(|| Err("formatting produced no result".to_owned()))
    }

    /// When a fragment is asked for at a different indentation than its
    /// seed, the rest of the file has probably moved by the same amount:
    /// speculate on all seeds at that shift, once per shift.
    fn shifted_seeds(&self, key: &Key) -> Vec<Key> {
        let seeds = self.seeds.borrow();
        let Some(seed) = seeds.iter().find(|s| {
            s.context == key.context
                && s.text == key.text
                && s.original_indent == key.original_indent
        }) else {
            return Vec::new();
        };
        let shift = key.place.indent as isize - seed.place.indent as isize;
        if shift == 0 || !self.shifts.borrow_mut().insert(shift) {
            return Vec::new();
        }
        seeds
            .iter()
            .filter_map(|s| {
                let indent = s.place.indent.checked_add_signed(shift)?;
                Some(Key {
                    place: Place {
                        indent,
                        max_width: key.place.max_width,
                        tab_spaces: key.place.tab_spaces,
                        ..s.place
                    },
                    ..s.clone()
                })
            })
            .collect()
    }

    /// Formats a fragment containing nested views, which cannot be cached:
    /// the children's forms come from the printer at this moment.
    fn with_nested(
        &self,
        key: &Key,
        fragment: &RustFragment<'_>,
        nested: &dyn NestedViews,
    ) -> Result<Layout, String> {
        if self.depth.get() >= MAX_NESTING {
            return Err("views are nested too deeply in embedded Rust".to_owned());
        }
        let ranges: Vec<Range<usize>> = fragment
            .nested
            .iter()
            .map(|r| {
                r.start
                    .checked_sub(fragment.range.start)
                    .filter(|_| r.end <= fragment.range.end)
                    .map(|s| s..r.end - fragment.range.start)
                    .ok_or("a nested view lies outside its fragment")
            })
            .collect::<Result<_, _>>()?;
        // One-line children get placeholders as wide as their one-line
        // form, so rustfmt's fit decisions see their size; children that
        // must break get a placeholder spanning lines.
        let placeholders: Vec<Option<usize>> = (0..ranges.len())
            .map(|id| nested.flat(id).map(|s| columns(s, key.place.tab_spaces)))
            .collect();
        let job = Job {
            key,
            nested: ranges,
            placeholders,
        };
        self.depth.set(self.depth.get() + 1);
        let result = self
            .run(std::slice::from_ref(&job))
            .pop()
            .expect("one job, one result")
            .and_then(|(p, out)| assemble(&p, &out, &key.text, Some(nested)));
        self.depth.set(self.depth.get() - 1);
        result
    }

    /// Prepares the jobs and runs them through rustfmt, grouped by width
    /// and split into bounded batches, at most two processes at a time.
    fn run(&self, jobs: &[Job<'_>]) -> Vec<Result<(Prepared, Output), String>> {
        let marker = marker_for(jobs.iter().map(|j| j.key.text.as_str()));
        let mut results: Vec<Option<Result<(Prepared, Output), String>>> =
            (0..jobs.len()).map(|_| None).collect();
        let mut prepared: Vec<(usize, Prepared)> = Vec::new();
        for (n, job) in jobs.iter().enumerate() {
            match prepare(job, &marker) {
                Ok(p) => prepared.push((n, p)),
                Err(e) => results[n] = Some(Err(e)),
            }
        }
        let mut groups: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
        for (slot, (_, p)) in prepared.iter().enumerate() {
            groups
                .entry((p.width, p.tab_spaces))
                .or_default()
                .push(slot);
        }
        let mut batches: Vec<((usize, usize), Vec<usize>)> = Vec::new();
        let mut keys: Vec<_> = groups.into_iter().collect();
        keys.sort();
        for (widths, members) in keys {
            for chunk in members.chunks(BATCH) {
                batches.push((widths, chunk.to_vec()));
            }
        }
        let mut outputs: Vec<Option<Result<Output, String>>> =
            (0..prepared.len()).map(|_| None).collect();
        let calls = AtomicUsize::new(0);
        for wave in batches.chunks(CONCURRENCY) {
            let done: Vec<Vec<(usize, Result<Output, String>)>> = std::thread::scope(|s| {
                let handles: Vec<_> = wave
                    .iter()
                    .map(|(widths, members)| {
                        let (prepared, marker, calls) = (&prepared, &marker, &calls);
                        let command = &self.command;
                        s.spawn(move || batch(command, prepared, members, *widths, marker, calls))
                    })
                    .collect();
                handles
                    .into_iter()
                    .map(|h| h.join().expect("rustfmt batch thread panicked"))
                    .collect()
            });
            for (slot, out) in done.into_iter().flatten() {
                outputs[slot] = Some(out);
            }
        }
        self.bump(|s| {
            s.rustfmt_calls += calls.load(Ordering::Relaxed);
            s.wrappers += prepared.len();
        });
        for ((n, p), out) in prepared.into_iter().zip(outputs) {
            let out = out.expect("every prepared job ran");
            results[n] = Some(out.map(|o| (p, o)));
        }
        results
            .into_iter()
            .map(|r| r.expect("every job has a result"))
            .collect()
    }

    fn answer<T>(&self, result: Result<T, String>) -> Result<T, ProviderError> {
        self.bump(|s| {
            s.requests += 1;
            s.failed += usize::from(result.is_err());
        });
        result.map_err(|message| ProviderError {
            range: None,
            message,
        })
    }
}

impl RustProvider for RustfmtProvider {
    fn prepare(&self, fragments: &[RustFragment<'_>]) {
        let Ok(probe) = self.probe_place() else {
            return;
        };
        let mut keys = Vec::new();
        let mut seeds = self.seeds.borrow_mut();
        for fragment in fragments.iter().filter(|f| f.nested.is_empty()) {
            keys.push(self.key(fragment, probe));
            let seed = self.key(fragment, seed_place(fragment, probe));
            keys.push(seed.clone());
            seeds.push(seed);
        }
        drop(seeds);
        self.fill(keys);
    }

    fn flat(
        &self,
        fragment: &RustFragment<'_>,
        nested: &dyn NestedViews,
    ) -> Result<Option<String>, ProviderError> {
        let result = self.probe_place().and_then(|place| {
            let key = self.key(fragment, place);
            if fragment.nested.is_empty() {
                self.cached(&key, false)
            } else if (0..fragment.nested.len()).any(|id| nested.flat(id).is_none()) {
                Ok(Layout::default())
            } else {
                self.with_nested(&key, fragment, nested)
            }
        });
        self.answer(result.map(|layout| match &layout.lines[..] {
            [line] => Some(line.text.clone()),
            _ => None,
        }))
    }

    fn layout(
        &self,
        fragment: &RustFragment<'_>,
        request: &LayoutRequest,
        nested: &dyn NestedViews,
    ) -> Result<Layout, ProviderError> {
        let key = self.key(
            fragment,
            Place {
                indent: request.indent,
                prefix: request.prefix,
                max_width: request.max_width,
                tab_spaces: request.tab_spaces,
            },
        );
        let result = if fragment.nested.is_empty() {
            self.cached(&key, true)
        } else {
            self.with_nested(&key, fragment, nested)
        };
        self.answer(result)
    }
}

/// One rustfmt run. When rustfmt rejects the batch, it is halved until
/// the culprit stands alone, so one odd fragment cannot sink the rest.
fn batch(
    command: &RustfmtCommand,
    prepared: &[(usize, Prepared)],
    members: &[usize],
    (width, tab_spaces): (usize, usize),
    marker: &str,
    calls: &AtomicUsize,
) -> Vec<(usize, Result<Output, String>)> {
    let mut input = String::new();
    let spans: Vec<WrapSpan> = members
        .iter()
        .enumerate()
        .map(|(slot, &m)| wrap(&mut input, &prepared[m].1, slot, marker))
        .collect();
    calls.fetch_add(1, Ordering::Relaxed);
    // The width makes the wrapper depth stand for the template
    // indentation. The rest keep literal and comment bytes unchanged;
    // rewriting them would only make the fragment fall back.
    let overrides = format!(
        "max_width={width},tab_spaces={tab_spaces},format_strings=false,\
         normalize_comments=false,wrap_comments=false"
    );
    match command.format(&input, &overrides) {
        Ok(text) => {
            let parts: Vec<&[Part]> = members.iter().map(|&m| &prepared[m].1.parts[..]).collect();
            let aligned = align_batch(&input, &spans, &parts, &text, marker, tab_spaces);
            members.iter().copied().zip(aligned).collect()
        }
        Err(RustfmtError::Rejected(m)) if members.len() > 1 => {
            // rustfmt names the line it stopped at: fail that wrapper and
            // run the rest again. Without a usable line, halve the batch.
            if let Some(slot) = rejected_slot(&m, &input, &spans) {
                let rest: Vec<usize> = members
                    .iter()
                    .enumerate()
                    .filter(|&(n, _)| n != slot)
                    .map(|(_, &m)| m)
                    .collect();
                let mut out = batch(command, prepared, &rest, (width, tab_spaces), marker, calls);
                out.push((members[slot], Err(RustfmtError::Rejected(m).to_string())));
                return out;
            }
            let (a, b) = members.split_at(members.len() / 2);
            let mut out = batch(command, prepared, a, (width, tab_spaces), marker, calls);
            out.extend(batch(
                command,
                prepared,
                b,
                (width, tab_spaces),
                marker,
                calls,
            ));
            out
        }
        Err(e) => members.iter().map(|&m| (m, Err(e.to_string()))).collect(),
    }
}

/// The wrapper holding the line a rustfmt error points at
/// (`--> <stdin>:LINE:COL`).
fn rejected_slot(message: &str, input: &str, spans: &[WrapSpan]) -> Option<usize> {
    let at = message.find("<stdin>:")? + "<stdin>:".len();
    let line: usize = message[at..]
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()?;
    let offset = input
        .split_inclusive('\n')
        .take(line.checked_sub(1)?)
        .map(str::len)
        .sum::<usize>();
    spans.iter().position(|s| s.whole.contains(&offset))
}

/// Where a fragment sits in its source, which is where the printer puts
/// it in an already formatted file. Constructor arguments start a line of
/// their own one level in.
fn seed_place(fragment: &RustFragment<'_>, probe: Place) -> Place {
    let tab = probe.tab_spaces.max(1);
    // Template indentation comes in whole levels; continuation lines
    // aligned under a tag name round down to the attribute level.
    let indent = fragment.original_indent(tab) / tab * tab;
    let line_start = fragment.file[..fragment.range.start]
        .rfind('\n')
        .map_or(0, |n| n + 1);
    let before = fragment.file[line_start..fragment.range.start].trim_start_matches([' ', '\t']);
    // A keyword lead-in (`if `, `} else if `) is the whole prefix; after an
    // attribute (`bg={`) only the attribute counts, as a broken tag puts
    // each attribute on a line of its own.
    let head = if before.ends_with([' ', '\t']) {
        before
    } else {
        before.rsplit([' ', '\t']).next().unwrap_or(before)
    };
    let (indent, prefix) = match fragment.context {
        RustContext::Args => (indent + tab, 0),
        _ => (indent, columns(head, tab)),
    };
    Place {
        indent,
        prefix,
        ..probe
    }
}

/// One fragment to format: its key, nested view ranges relative to the
/// text, and the width of each nested view's one-line placeholder (`None`
/// for a breaking one).
struct Job<'a> {
    key: &'a Key,
    nested: Vec<Range<usize>>,
    placeholders: Vec<Option<usize>>,
}

/// A wrapper identifier prefix that occurs in no fragment, so wrapper and
/// placeholder names cannot collide with user tokens.
fn marker_for<'a>(texts: impl Iterator<Item = &'a str> + Clone) -> String {
    let mut marker = "__qf".to_owned();
    while texts.clone().any(|t| t.contains(&marker)) {
        marker.push('q');
    }
    marker
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Comment,
    Literal,
    Comma,
    Punct,
    Word,
}

#[derive(Clone, Debug)]
struct Tok {
    start: usize,
    end: usize,
    kind: Kind,
}

/// Lexical tokens without whitespace; comments are tokens too.
fn lex(text: &str) -> Result<Vec<Tok>, String> {
    let mut out = Vec::new();
    for t in lex_source(text, 0) {
        let kind = match t.kind {
            LexKind::Whitespace => continue,
            LexKind::LineComment | LexKind::BlockComment => Kind::Comment,
            LexKind::Literal => Kind::Literal,
            LexKind::Ident | LexKind::Lifetime => Kind::Word,
            LexKind::Punct(',') => Kind::Comma,
            LexKind::Punct(_) => Kind::Punct,
            LexKind::Other => return Err(format!("cannot lex `{}`", &text[t.range])),
        };
        out.push(Tok {
            start: t.range.start,
            end: t.range.end,
            kind,
        });
    }
    Ok(out)
}

/// A source gap moved into the wrapper: line breaks keep their indentation
/// relative to the fragment's original line, now hanging from `base`. If
/// rustfmt leaves a construct alone (too long to fit), it still lines up.
fn rebase_gap(gap: &str, original_indent: usize, base: &str, tab_spaces: usize) -> String {
    let newlines = gap.matches('\n').count();
    if newlines == 0 {
        return gap.to_owned();
    }
    let last = &gap[gap.rfind('\n').expect("has a newline") + 1..];
    let rel = columns(last, tab_spaces).saturating_sub(original_indent);
    let mut s = "\n".repeat(newlines.min(2));
    s.push_str(base);
    s.push_str(&" ".repeat(rel));
    s
}

enum Part {
    /// An original token, at `body` in the wrapper and `src` in the text.
    Token {
        body: Range<usize>,
        src: Range<usize>,
    },
    /// The placeholder for nested view `nested`.
    Nested {
        body: Range<usize>,
        nested: usize,
        flat: bool,
    },
    /// The comment that keeps a list with a trailing comma broken.
    Pin { body: Range<usize> },
}

impl Part {
    fn body(&self) -> &Range<usize> {
        match self {
            Part::Token { body, .. } | Part::Nested { body, .. } | Part::Pin { body } => body,
        }
    }
}

/// A fragment ready for its wrapper.
struct Prepared {
    context: RustContext,
    prefix: usize,
    /// The fragment as it goes into the wrapper.
    body: String,
    parts: Vec<Part>,
    /// Block depth of the wrapper line; stands for the template indent.
    depth: usize,
    /// `max_width` for the run, so `depth` lines get the template's room.
    width: usize,
    tab_spaces: usize,
}

fn prepare(job: &Job<'_>, marker: &str) -> Result<Prepared, String> {
    let key = job.key;
    let Place {
        indent,
        prefix,
        max_width,
        tab_spaces,
    } = key.place;
    let tab = tab_spaces.max(1);
    let text = key.text.as_str();
    let toks = lex(text)?;
    let min_depth = if key.context == RustContext::ArmHead {
        2
    } else {
        1
    };
    let depth = (indent / tab).max(min_depth);
    let width = (max_width + depth * tab)
        .saturating_sub(indent)
        .max(depth * tab + 20);
    let unit = " ".repeat(tab);
    let base = unit.repeat(depth);
    let pinned = pinned_commas(text, &toks);

    let mut body = String::new();
    let mut parts = Vec::new();
    let mut last_end = 0;
    let mut k = 0;
    let mut i = 0;
    let mut need_newline = false;
    while i < toks.len() {
        let tok = &toks[i];
        if i > 0 {
            let gap = &text[last_end..tok.start];
            let mut gap = rebase_gap(gap, key.original_indent, &base, tab);
            if need_newline && !gap.contains('\n') {
                gap = format!("\n{base}");
            }
            body.push_str(&gap);
        }
        need_newline = false;
        if let Some(n) = job.nested.get(k).filter(|n| n.start == tok.start) {
            // The invocation's tokens become one placeholder; it must end
            // on a token end.
            let mut j = i;
            while j < toks.len() && toks[j].end < n.end {
                j += 1;
            }
            if j == toks.len() || toks[j].end != n.end {
                return Err("a nested view does not end on a token boundary".to_owned());
            }
            let name = format!("{marker}v{k}");
            let start = body.len();
            let flat = job.placeholders[k];
            match flat {
                Some(width) => {
                    body.push_str(&name);
                    body.push_str(&"_".repeat(width.saturating_sub(name.len())));
                }
                None => body.push_str(&format!("{name}! {{\n{base}{unit}_\n{base}}}")),
            }
            parts.push(Part::Nested {
                body: start..body.len(),
                nested: k,
                flat: flat.is_some(),
            });
            last_end = toks[j].end;
            i = j + 1;
            k += 1;
            continue;
        }
        if job.nested.get(k).is_some_and(|n| n.start < tok.start) {
            return Err("a nested view does not start on a token boundary".to_owned());
        }
        let start = body.len();
        body.push_str(&text[tok.start..tok.end]);
        parts.push(Part::Token {
            body: start..body.len(),
            src: tok.start..tok.end,
        });
        if pinned[i] {
            // A line comment after the comma keeps the item's line
            // break through rustfmt; the comment is left out again.
            let start = body.len() + 1;
            body.push_str(&format!(" //{marker}k"));
            parts.push(Part::Pin {
                body: start..body.len(),
            });
            need_newline = true;
        }
        last_end = tok.end;
        i += 1;
    }
    if k != job.nested.len() {
        return Err("a nested view is not inside its fragment".to_owned());
    }
    Ok(Prepared {
        context: wrapper_context(key.context, text, &toks),
        prefix,
        body,
        parts,
        depth,
        width,
        tab_spaces: tab,
    })
}

/// The wrapper for a fragment: an expression with a `let` outside any
/// brackets (`@when {let Some(x) = y}`, a let chain) is only valid as a
/// condition.
fn wrapper_context(context: RustContext, text: &str, toks: &[Tok]) -> RustContext {
    if context != RustContext::Expr {
        return context;
    }
    let mut depth = 0i32;
    for t in toks {
        match &text[t.start..t.end] {
            "(" | "[" | "{" => depth += 1,
            ")" | "]" | "}" => depth -= 1,
            "let" if depth == 0 => return RustContext::Condition,
            _ => {}
        }
    }
    context
}

/// Where one wrapper sits in a batch's input.
struct WrapSpan {
    /// The whole wrapper function.
    whole: Range<usize>,
    /// The fragment inside it.
    body: Range<usize>,
}

/// Appends `p`'s wrapper function to `input`.
fn wrap(input: &mut String, p: &Prepared, slot: usize, marker: &str) -> WrapSpan {
    let whole_start = input.len();
    let unit = " ".repeat(p.tab_spaces);
    let assign = p.prefix >= ASSIGN_MIN;
    let outer = p.depth - usize::from(p.context == RustContext::ArmHead);
    input.push_str(&format!("fn {marker}f{slot}() {{\n"));
    for d in 1..outer {
        input.push_str(&unit.repeat(d));
        input.push_str("{\n");
    }
    let ind = unit.repeat(outer);
    input.push_str(&ind);
    let (lead, trail) = match p.context {
        RustContext::Expr if assign => (
            format!("{} = ", "_".repeat(p.prefix.saturating_sub(3).max(1))),
            ";".to_owned(),
        ),
        // A tail expression after a statement, so rustfmt never collapses
        // the enclosing block onto one line.
        RustContext::Expr => (format!("{marker}d;\n{ind}"), String::new()),
        RustContext::Scrutinee => ("match ".to_owned(), " {}".to_owned()),
        RustContext::Condition if p.prefix >= ELSE_IF_PREFIX => {
            (format!("if {marker}c {{}} else if "), " {}".to_owned())
        }
        RustContext::Condition => ("if ".to_owned(), " {}".to_owned()),
        RustContext::ForHeader => ("for ".to_owned(), " {}".to_owned()),
        RustContext::ArmHead => (
            format!("match {marker}m {{\n{ind}{unit}"),
            format!(" => {{}}\n{ind}}}"),
        ),
        RustContext::Local => (String::new(), String::new()),
        // The prefix ends at the opening parenthesis.
        RustContext::Args => (
            format!("{}(", "_".repeat(p.prefix.saturating_sub(1).max(2))),
            ");".to_owned(),
        ),
        RustContext::Type => (format!("type {marker}t = "), ";".to_owned()),
    };
    input.push_str(&lead);
    let start = input.len();
    input.push_str(&p.body);
    let end = input.len();
    input.push_str(&trail);
    input.push('\n');
    for d in (1..outer).rev() {
        input.push_str(&unit.repeat(d));
        input.push_str("}\n");
    }
    input.push_str("}\n");
    WrapSpan {
        whole: whole_start..input.len(),
        body: start..end,
    }
}

/// A fragment's tokens in rustfmt's output, with the whitespace before each.
struct Output {
    /// Whitespace between the wrapper's lead-in and the fragment.
    lead: String,
    pieces: Vec<Piece>,
    /// Whitespace between the fragment and the wrapper's next token.
    trail: String,
}

struct Piece {
    gap: String,
    /// Index into `Prepared::parts`.
    part: usize,
    /// Tokens rustfmt dropped after this part, to put back in order.
    after: Vec<After>,
}

/// Builds the fragment's lines from rustfmt's whitespace and the original
/// token bytes, splicing in nested views.
fn assemble(
    p: &Prepared,
    out: &Output,
    text: &str,
    nested: Option<&dyn NestedViews>,
) -> Result<Layout, String> {
    let mut b = LineBuilder::new(p.depth * p.tab_spaces, p.tab_spaces);
    // A fragment that starts its wrapper line has nothing before it.
    // Otherwise a line break after the lead-in means rustfmt moved the
    // fragment to the next line (`x =` then the value); argument lists
    // keep their whitespace, framed by their parentheses.
    let starts_line = matches!(p.context, RustContext::Local | RustContext::ArmHead)
        || (p.context == RustContext::Expr && p.prefix < ASSIGN_MIN);
    let moved = !starts_line && p.context != RustContext::Args && out.lead.contains('\n');
    if p.context == RustContext::Args || moved {
        b.gap(&out.lead);
    }
    for piece in &out.pieces {
        let part = &p.parts[piece.part];
        if let Part::Pin { .. } = part {
            continue;
        }
        b.gap(&piece.gap);
        match part {
            Part::Pin { .. } => {}
            Part::Token { src, .. } => b.token(&text[src.clone()]),
            Part::Nested {
                nested: k, flat, ..
            } => {
                let views = nested.ok_or("a nested view without a printer")?;
                if *flat {
                    b.token(
                        views
                            .flat(*k)
                            .ok_or("a nested view lost its one-line form")?,
                    );
                } else {
                    let host = b.current_indent();
                    b.splice(&views.layout(*k, host));
                }
            }
        }
        for a in &piece.after {
            match a {
                After::Comma => b.token(","),
                After::Open => b.open_block(),
                After::Close => b.close_block(),
            }
        }
    }
    if p.context == RustContext::Args {
        b.gap(&out.trail);
    } else if moved {
        // The closing delimiter goes back to the template indentation.
        b.newline(0, false, String::new());
    }
    Ok(b.finish())
}

/// Commas of lists written with a trailing comma, which therefore stay
/// one item per line (the "magic trailing comma" of Black and dart
/// format). rustfmt would join a short list and drop the comma, which must
/// stay. A one-element tuple's comma is required and pins nothing. Commas
/// already followed by a comment are left alone.
fn pinned_commas(text: &str, toks: &[Tok]) -> Vec<bool> {
    let mut pinned = vec![false; toks.len()];
    // (opening token, commas directly inside it)
    let mut stack: Vec<(usize, Vec<usize>)> = Vec::new();
    for (n, t) in toks.iter().enumerate() {
        match &text[t.start..t.end] {
            "(" | "[" | "{" => stack.push((n, Vec::new())),
            ")" | "]" | "}" => {
                let Some((open, commas)) = stack.pop() else {
                    continue;
                };
                let trailing = n > 0 && toks[n - 1].kind == Kind::Comma;
                let one_tuple = &text[toks[open].start..toks[open].end] == "(" && commas.len() == 1;
                if trailing && !one_tuple {
                    for c in commas {
                        pinned[c] = toks[c + 1].kind != Kind::Comment;
                    }
                }
            }
            "," => {
                if let Some(top) = stack.last_mut() {
                    top.1.push(n);
                }
            }
            _ => {}
        }
    }
    pinned
}

/// Who an input token belongs to: its wrapper slot, and the fragment part
/// when it came from the fragment.
#[derive(Clone, Copy)]
struct Origin {
    slot: usize,
    part: Option<usize>,
}

fn same_token(input: &str, a: &Tok, output: &str, b: &Tok) -> bool {
    let (x, y) = (&input[a.start..a.end], &output[b.start..b.end]);
    if a.kind != b.kind {
        return false;
    }
    if a.kind == Kind::Comment {
        // rustfmt re-indents block comment interiors and trims trailing
        // spaces; the original bytes are restored, so compare the words.
        return x.split_whitespace().eq(y.split_whitespace());
    }
    x == y
}

/// Pairs the batch's input tokens with rustfmt's output tokens. A comma
/// rustfmt inserted is dropped and one it removed is put back; any other
/// difference fails that wrapper's fragment only, and alignment resumes at
/// the next wrapper function.
fn align_batch(
    input: &str,
    spans: &[WrapSpan],
    parts: &[&[Part]],
    output: &str,
    marker: &str,
    tab_spaces: usize,
) -> Vec<Result<Output, String>> {
    let fail_all = |m: String| spans.iter().map(|_| Err(m.clone())).collect();
    let ins = match lex(input) {
        Ok(t) => t,
        Err(m) => return fail_all(format!("wrapper does not lex: {m}")),
    };
    let outs = match lex(output) {
        Ok(t) => t,
        Err(m) => return fail_all(format!("rustfmt output does not lex: {m}")),
    };

    // Origins, and the first input token of each wrapper.
    let mut origins = Vec::with_capacity(ins.len());
    let mut first_tok = vec![ins.len(); spans.len() + 1];
    let mut slot = 0;
    let mut part = 0;
    for (n, t) in ins.iter().enumerate() {
        while slot + 1 < spans.len() && t.start >= spans[slot].whole.end {
            slot += 1;
            part = 0;
        }
        if first_tok[slot] == ins.len() {
            first_tok[slot] = n;
        }
        let body = &spans[slot].body;
        let in_part = if body.contains(&t.start) {
            let rel = t.start - body.start;
            while part < parts[slot].len() && parts[slot][part].body().end <= rel {
                part += 1;
            }
            Some(part)
        } else {
            None
        };
        origins.push(Origin {
            slot,
            part: in_part,
        });
    }

    let mut pairs: Vec<Option<usize>> = vec![None; ins.len()];
    let mut out_in: Vec<Option<usize>> = vec![None; outs.len()];
    let mut dropped = vec![Dropped::No; outs.len()];
    let mut restore: Vec<Vec<After>> = vec![Vec::new(); outs.len()];
    let mut failed: Vec<Option<String>> = vec![None; spans.len()];
    let (mut i, mut j) = (0, 0);
    let mut last_paired: Option<usize> = None;
    // Brace depth of the output so far, and the depths at which rustfmt
    // opened a block of its own.
    let mut depth = 0usize;
    let mut inserted: Vec<usize> = Vec::new();
    // The same for the input, and the blocks rustfmt removed.
    let mut in_depth = 0usize;
    let mut removed: Vec<usize> = Vec::new();
    while i < ins.len() {
        let s = origins[i].slot;
        if failed[s].is_none() && j >= outs.len() {
            failed[s] = Some("rustfmt output ended early".to_owned());
        }
        if failed[s].is_some() {
            // Resume at the next wrapper function, found by its name.
            let next = s + 1;
            i = first_tok[next];
            if next < spans.len() {
                let name = format!("{marker}f{next}");
                match (j..outs.len()).find(|&k| output[outs[k].start..outs[k].end] == name) {
                    Some(k) if k > 0 => {
                        j = k - 1;
                        depth = 0;
                        inserted.clear();
                        in_depth = 0;
                        removed.clear();
                    }
                    _ => {
                        for f in failed.iter_mut().skip(next) {
                            f.get_or_insert_with(|| "rustfmt output lost a wrapper".to_owned());
                        }
                        break;
                    }
                }
            }
            continue;
        }
        let (a, b) = (&ins[i], &outs[j]);
        let a_text = &input[a.start..a.end];
        let b_text = &output[b.start..b.end];
        let in_fragment = origins[i].part.is_some();
        let after_head = |p: Option<usize>| {
            p.is_some_and(|p| matches!(&output[outs[p].start..outs[p].end], "|" | ">"))
        };
        if a_text == "}" && in_fragment && removed.last() == Some(&in_depth) {
            // The end of a block rustfmt took away; put it back.
            removed.pop();
            in_depth -= 1;
            match last_paired {
                Some(p) => restore[p].push(After::Close),
                None => failed[s] = Some("rustfmt removed a leading block".to_owned()),
            }
            i += 1;
        } else if b_text == "}" && inserted.last() == Some(&depth) {
            // Blocks nest, so this closes the block rustfmt added, even
            // when the input has a `}` here too.
            inserted.pop();
            depth -= 1;
            dropped[j] = Dropped::Close;
            j += 1;
        } else if same_token(input, a, output, b) {
            pairs[i] = Some(j);
            out_in[j] = Some(i);
            last_paired = Some(j);
            match b_text {
                "{" => {
                    depth += 1;
                    in_depth += 1;
                }
                "}" => {
                    depth = depth.saturating_sub(1);
                    in_depth = in_depth.saturating_sub(1);
                }
                _ => {}
            }
            i += 1;
            j += 1;
        } else if b.kind == Kind::Comma {
            dropped[j] = Dropped::Comma;
            j += 1;
        } else if b_text == ";"
            && in_fragment
            && outs
                .get(j + 1)
                .is_some_and(|t| &output[t.start..t.end] == "}")
        {
            // rustfmt ends `continue`, `break`, and `return` with a
            // semicolon at the end of a block.
            dropped[j] = Dropped::Comma;
            j += 1;
        } else if a_text == "{" && in_fragment && after_head(last_paired) {
            // rustfmt took the block off a closure or arm body that fits
            // on one line; the braces come back around it.
            in_depth += 1;
            removed.push(in_depth);
            restore[last_paired.expect("checked")].push(After::Open);
            i += 1;
        } else if b_text == "{" && in_fragment && after_head(last_paired) {
            // rustfmt wraps a closure body or a match arm body that spans
            // lines in a block of its own; the braces go and the lines
            // inside move back out.
            depth += 1;
            inserted.push(depth);
            dropped[j] = Dropped::Open;
            j += 1;
        } else if a.kind == Kind::Comma && in_fragment {
            match last_paired {
                Some(p) => restore[p].push(After::Comma),
                None => failed[s] = Some("rustfmt removed a leading comma".to_owned()),
            }
            i += 1;
        } else {
            failed[s] = Some(format!(
                "rustfmt changed `{}` into `{}`",
                &input[a.start..a.end],
                &output[b.start..b.end]
            ));
        }
    }

    // Operator characters that touch form one operator (`::`, `..=`,
    // `&&`), so whether they touch must not change.
    for n in 0..ins.len().saturating_sub(1) {
        let s = origins[n].slot;
        if failed[s].is_some() || origins[n].part.is_none() || origins[n + 1].part.is_none() {
            continue;
        }
        let glues = |t: &Tok| {
            t.kind == Kind::Punct
                && input[t.start..t.end]
                    .chars()
                    .all(|c| "=<>!&|+-*/%^.:#".contains(c))
        };
        if !glues(&ins[n]) || !glues(&ins[n + 1]) {
            continue;
        }
        let joint_in = ins[n].end == ins[n + 1].start;
        let joint_out = match (pairs[n], pairs[n + 1]) {
            (Some(x), Some(y)) => y == x + 1 && outs[x].end == outs[y].start,
            _ => false,
        };
        if joint_in != joint_out {
            failed[s] = Some(format!(
                "rustfmt changed the spacing of `{}{}`",
                &input[ins[n].start..ins[n].end],
                &input[ins[n + 1].start..ins[n + 1].end]
            ));
        }
    }

    (0..spans.len())
        .map(|s| {
            if let Some(m) = failed[s].take() {
                return Err(m);
            }
            let toks: Vec<usize> = (first_tok[s]..first_tok[s + 1])
                .filter(|&n| origins[n].part.is_some())
                .collect();
            let (Some(&first), Some(&last)) = (toks.first(), toks.last()) else {
                return Ok(Output {
                    lead: String::new(),
                    pieces: Vec::new(),
                    trail: String::new(),
                });
            };
            let paired = |n: usize| pairs[n].ok_or("a wrapper token went missing");
            let before = paired(first - 1)?;
            let after = paired(last + 1)?;
            let mut pieces = Vec::new();
            let mut gap = String::new();
            let mut lead = None;
            let mut prev_end = outs[before].end;
            let mut last_part = None;
            let mut blocks = 0;
            let mut after_open = false;
            for k in before + 1..after {
                let segment = &output[prev_end..outs[k].start];
                prev_end = outs[k].end;
                // The whitespace after a dropped `{` and before a dropped
                // `}` goes with it; lines inside move out one level each.
                if !after_open && dropped[k] != Dropped::Close {
                    gap.push_str(&dedent(segment, blocks * tab_spaces, tab_spaces));
                }
                after_open = false;
                match dropped[k] {
                    Dropped::No => {}
                    Dropped::Comma => continue,
                    Dropped::Open => {
                        blocks += 1;
                        after_open = true;
                        continue;
                    }
                    Dropped::Close => {
                        blocks -= 1;
                        continue;
                    }
                }
                let n = out_in[k].ok_or("an unpaired token in a fragment")?;
                let part = origins[n].part.ok_or("a wrapper token inside a fragment")?;
                // Later tokens of a multi-line placeholder: skip them and
                // their whitespace.
                if last_part == Some(part) {
                    gap.clear();
                    continue;
                }
                last_part = Some(part);
                let g = std::mem::take(&mut gap);
                if lead.is_none() {
                    lead = Some(g);
                    pieces.push(Piece {
                        gap: String::new(),
                        part,
                        after: restore[k].clone(),
                    });
                } else {
                    pieces.push(Piece {
                        gap: g,
                        part,
                        after: restore[k].clone(),
                    });
                }
            }
            if blocks != 0 {
                return Err("rustfmt added a block that does not close".to_owned());
            }
            gap.push_str(&output[prev_end..outs[after].start]);
            Ok(Output {
                lead: lead.unwrap_or_default(),
                pieces,
                trail: gap,
            })
        })
        .collect()
}

/// An input token rustfmt dropped, restored after the output token before
/// it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum After {
    Comma,
    /// The braces of a closure or arm body block.
    Open,
    Close,
}

/// An output token that has no counterpart in the input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dropped {
    No,
    /// A comma rustfmt added to a list, or a semicolon before `}`.
    Comma,
    /// The braces of a block rustfmt added.
    Open,
    Close,
}

/// `ws` with the indentation after its last line break reduced by `cols`.
fn dedent(ws: &str, cols: usize, tab_spaces: usize) -> String {
    match ws.rfind('\n') {
        Some(n) if cols > 0 => {
            let indent = columns(&ws[n + 1..], tab_spaces).saturating_sub(cols);
            format!("{}{}", &ws[..=n], " ".repeat(indent))
        }
        _ => ws.to_owned(),
    }
}

/// Accumulates [`LayoutLine`]s.
struct LineBuilder {
    lines: Vec<LayoutLine>,
    /// Indentation of the lines that opened restored blocks.
    blocks: Vec<usize>,
    /// The next token starts a restored block's first line.
    block_start: bool,
    /// Columns of the wrapper depth, which stand for the template indent.
    base: usize,
    tab_spaces: usize,
}

impl LineBuilder {
    fn new(base: usize, tab_spaces: usize) -> Self {
        LineBuilder {
            lines: vec![LayoutLine {
                indent: 0,
                text: String::new(),
                verbatim: false,
            }],
            blocks: Vec::new(),
            block_start: false,
            base,
            tab_spaces,
        }
    }

    fn cur(&mut self) -> &mut LayoutLine {
        self.lines.last_mut().expect("never empty")
    }

    /// Indentation of the current line relative to the template indent.
    fn current_indent(&self) -> usize {
        let line = self.lines.last().expect("never empty");
        if line.verbatim { 0 } else { line.indent }
    }

    fn newline(&mut self, indent: usize, verbatim: bool, text: String) {
        self.lines.push(LayoutLine {
            indent,
            text,
            verbatim,
        });
    }

    /// rustfmt's whitespace: spaces stay, a line break starts a line
    /// indented relative to the wrapper depth. At most one blank line.
    fn gap(&mut self, gap: &str) {
        let extra = self.blocks.len() * self.tab_spaces;
        if std::mem::take(&mut self.block_start) {
            let host = *self.blocks.last().expect("inside a block");
            self.newline(host + self.tab_spaces, false, String::new());
            return;
        }
        let newlines = gap.matches('\n').count();
        if newlines == 0 {
            self.cur().text.push_str(gap);
            return;
        }
        let last = &gap[gap.rfind('\n').expect("has a newline") + 1..];
        let indent = columns(last, self.tab_spaces).saturating_sub(self.base) + extra;
        if newlines > 1 {
            self.newline(0, false, String::new());
        }
        self.newline(indent, false, String::new());
    }

    /// ` {` of a restored block; its body starts on the next line, one
    /// level in, and every line inside moves in one level.
    fn open_block(&mut self) {
        let host = self.current_indent();
        self.token(" {");
        self.blocks.push(host);
        self.block_start = true;
    }

    fn close_block(&mut self) {
        let host = self.blocks.pop().expect("a restored block is open");
        self.newline(host, false, "}".to_owned());
    }

    /// Original token bytes; lines after a newline inside the token are
    /// verbatim.
    fn token(&mut self, text: &str) {
        let mut parts = text.split('\n');
        self.cur()
            .text
            .push_str(parts.next().expect("split yields one"));
        for part in parts {
            self.newline(0, true, part.to_owned());
        }
    }

    /// A nested view's layout; its indents already share our base.
    fn splice(&mut self, child: &Layout) {
        let mut lines = child.lines.iter();
        if let Some(first) = lines.next() {
            self.cur().text.push_str(&first.text);
        }
        self.lines.extend(lines.cloned());
    }

    fn finish(self) -> Layout {
        Layout { lines: self.lines }
    }
}
