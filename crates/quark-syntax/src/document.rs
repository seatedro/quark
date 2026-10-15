//! [`DocumentHighlighter`]: highlighting for the code blocks of Quark
//! documents, as a [`CodeHighlighter`] the app hands to its
//! `quark_ui::document::MarkdownDocument`.
//!
//! Highlighting runs on a [`HighlightWorker`] thread with the grammars of
//! a [`GrammarStore`]. A block stays plain while its grammar downloads,
//! and keeps its own language's colors while grammars of languages
//! embedded in it download, taking each newer result as they arrive.

use std::collections::HashMap;
use std::sync::Arc;

use quark::selection::BlockKey;
use quark_ui::document::{CodeHighlighter, SyntaxTone, ToneSpan};

use crate::{GrammarStore, HighlightKind, HighlightSpan, HighlightWorker, Highlighted, LanguageId};

struct Done {
    generation: u64,
    /// Counts results for `generation`: each grammar that arrives while it
    /// is pending recolors it with a higher revision.
    revision: u32,
    /// Distinct for every result taken, so rows rebuilt for a result that
    /// recolors the same generation (a grammar that arrived) see a new
    /// version.
    version: u64,
    source: Arc<str>,
    spans: Arc<[ToneSpan]>,
}

struct Slot {
    /// Newest request.
    generation: u64,
    language: LanguageId,
    requested: Arc<str>,
    done: Option<Done>,
}

impl Slot {
    fn pending(&self) -> bool {
        self.done
            .as_ref()
            .is_none_or(|done| done.generation < self.generation)
    }
}

impl Done {
    /// Spans of an earlier source still fit a source that only grew, which
    /// keeps a streaming block colored while its new highlight is
    /// computed.
    fn applies_to(&self, code: &str) -> bool {
        code.starts_with(&*self.source)
    }
}

/// Highlights document code blocks off the UI thread with the grammars of
/// a [`GrammarStore`]. One per document.
pub struct DocumentHighlighter {
    store: GrammarStore,
    worker: Option<HighlightWorker>,
    slots: HashMap<BlockKey, Slot>,
    generation: u64,
    /// Last [`Done::version`] handed out.
    version: u64,
}

impl DocumentHighlighter {
    pub fn new(store: GrammarStore) -> Self {
        Self {
            store,
            worker: None,
            slots: HashMap::new(),
            generation: 0,
            version: 0,
        }
    }

    /// Replaces a dead worker and sends it every request still waiting for
    /// a result.
    fn respawn(&mut self) {
        let worker = self.worker.insert(HighlightWorker::new(self.store.clone()));
        for (slot, entry) in &self.slots {
            if entry.pending() {
                worker.request(
                    slot.0,
                    entry.generation,
                    entry.language.clone(),
                    entry.requested.clone(),
                );
            }
        }
    }

    /// Replaces the worker with one whose thread is gone.
    #[cfg(test)]
    fn kill_worker(&mut self) {
        self.worker = Some(HighlightWorker::gone());
    }
}

impl CodeHighlighter for DocumentHighlighter {
    fn version(&mut self, slot: BlockKey, lang: &str, code: &str) -> u64 {
        // Checked without allocating when the block keeps its language,
        // since this runs for every code block on every rebuild.
        let language = match self.slots.get(&slot) {
            Some(entry) if entry.language.matches(lang) => entry.language.clone(),
            _ => match LanguageId::from_fence(lang) {
                Some(language) => language,
                None => return 0,
            },
        };
        let entry = self.slots.entry(slot).or_insert_with(|| Slot {
            generation: 0,
            language: language.clone(),
            requested: Arc::from(""),
            done: None,
        });
        // Length first: a streaming block grows, so most changes show there
        // without comparing the text.
        let changed = entry.requested.len() != code.len() || *entry.requested != *code;
        if entry.generation == 0 || entry.language != language || changed {
            self.generation += 1;
            entry.generation = self.generation;
            entry.language = language.clone();
            entry.requested = Arc::from(code);
            let store = &self.store;
            self.worker
                .get_or_insert_with(|| HighlightWorker::new(store.clone()))
                .request(slot.0, entry.generation, language, entry.requested.clone());
        }
        entry
            .done
            .as_ref()
            .filter(|done| done.applies_to(code))
            .map_or(0, |done| done.version)
    }

    fn spans(&self, slot: BlockKey) -> &[ToneSpan] {
        self.slots
            .get(&slot)
            .and_then(|s| s.done.as_ref())
            .map_or(&[], |done| &done.spans)
    }

    fn poll(&mut self) -> Vec<BlockKey> {
        let mut changed = Vec::new();
        loop {
            let Some(worker) = &self.worker else {
                return changed;
            };
            match worker.try_recv() {
                Ok(Some(result)) => {
                    changed.extend(take(&mut self.slots, &mut self.version, result))
                }
                Ok(None) => return changed,
                Err(_) => {
                    self.respawn();
                    return changed;
                }
            }
        }
    }

    fn finish_pending(&mut self) -> Vec<BlockKey> {
        let mut changed = self.poll();
        // One respawn per call: a worker that cannot be spawned again leaves
        // the pending blocks plain instead of hanging.
        let mut respawned = false;
        while self.slots.values().any(Slot::pending) {
            let Some(worker) = &self.worker else {
                break;
            };
            match worker.recv() {
                Ok(result) => changed.extend(take(&mut self.slots, &mut self.version, result)),
                Err(_) if !respawned => {
                    respawned = true;
                    self.respawn();
                }
                Err(_) => break,
            }
        }
        changed
    }

    fn forget(&mut self, slot: BlockKey) {
        // A result still in flight finds no slot and is dropped.
        self.slots.remove(&slot);
    }

    fn len(&self) -> usize {
        self.slots.len()
    }
}

/// Keeps a result unless the slot already holds a newer one: a later
/// generation, or a later revision of the same one (results that recolor
/// a generation as its grammars arrive). A result older than the newest
/// request is still kept: it applies while the source only grew, and the
/// newest one may be dropped by the worker's coalescing in favor of an
/// even newer one.
fn take(
    slots: &mut HashMap<BlockKey, Slot>,
    version: &mut u64,
    result: Highlighted,
) -> Option<BlockKey> {
    let key = BlockKey(result.slot);
    let slot = slots.get_mut(&key)?;
    if slot.done.as_ref().is_some_and(|done| {
        (done.generation, done.revision) >= (result.generation, result.revision)
    }) {
        return None;
    }
    *version += 1;
    slot.done = Some(Done {
        generation: result.generation,
        revision: result.revision,
        version: *version,
        source: result.source,
        spans: result.spans.iter().filter_map(tone_span).collect(),
    });
    Some(key)
}

/// `span` as a document span, or `None` for kinds that render plain.
fn tone_span(span: &HighlightSpan) -> Option<ToneSpan> {
    let tone = match span.kind {
        HighlightKind::Keyword | HighlightKind::Preprocessor => SyntaxTone::Keyword,
        HighlightKind::String => SyntaxTone::String,
        HighlightKind::Comment => SyntaxTone::Comment,
        HighlightKind::Function => SyntaxTone::Function,
        HighlightKind::Type | HighlightKind::Namespace => SyntaxTone::Type,
        HighlightKind::Number | HighlightKind::Constant | HighlightKind::Builtin => {
            SyntaxTone::Number
        }
        HighlightKind::Property
        | HighlightKind::Attribute
        | HighlightKind::Tag
        | HighlightKind::Label => SyntaxTone::Property,
        HighlightKind::Operator => SyntaxTone::Operator,
        HighlightKind::Normal | HighlightKind::Punctuation | HighlightKind::Variable => {
            return None;
        }
    };
    let range = span.range();
    Some(ToneSpan {
        range: range.start as u32..range.end as u32,
        tone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Catches progressive results being dropped or stale ones winning:
    // later revisions of the held generation (grammars that arrived) and
    // later generations replace what a block holds, while repeats, earlier
    // revisions, and earlier generations do not.
    #[test]
    fn results_replace_a_block_only_when_newer() {
        let code: Arc<str> = Arc::from("fn");
        let key = BlockKey(1);
        let mut slots = HashMap::from([(
            key,
            Slot {
                generation: 2,
                language: LanguageId::from_fence("rust").unwrap(),
                requested: code.clone(),
                done: None,
            },
        )]);
        let mut version = 0;
        let mut held = Vec::new();
        for (generation, revision) in [(1, 0), (1, 1), (1, 1), (1, 0), (2, 0), (1, 2), (2, 1)] {
            let result = Highlighted {
                slot: key.0,
                generation,
                revision,
                source: code.clone(),
                spans: Vec::new(),
                pending: false,
                unresolved: Vec::new(),
                part: None,
            };
            take(&mut slots, &mut version, result);
            let done = slots[&key].done.as_ref().unwrap();
            held.push((done.generation, done.revision, done.version));
        }

        assert_eq!(
            held,
            [
                (1, 0, 1),
                (1, 1, 2),
                (1, 1, 2),
                (1, 1, 2),
                (2, 0, 3),
                (2, 0, 3),
                (2, 1, 4)
            ]
        );
    }

    /// The source texts `highlighter` colors in `code` for `slot`.
    fn colored(highlighter: &mut DocumentHighlighter, slot: BlockKey, code: &str) -> Vec<String> {
        if highlighter.version(slot, "rust", code) == 0 {
            return Vec::new();
        }
        highlighter
            .spans(slot)
            .iter()
            .map(|span| code[span.range.start as usize..span.range.end as usize].to_owned())
            .collect()
    }

    // Catches the real engine's results not reaching document spans: a
    // block is plain until its highlight is taken, then colored.
    #[test]
    fn rust_block_is_colored_once_its_highlight_arrives() {
        let Some(store) = crate::testing::store_with("rust") else {
            return;
        };
        let mut highlighter = DocumentHighlighter::new(store);
        let (slot, code) = (BlockKey(0), "fn main() { let s = \"hi\"; }");
        let first = colored(&mut highlighter, slot, code);

        let taken = highlighter.finish_pending();
        let second = colored(&mut highlighter, slot, code);

        assert_eq!(
            (first, taken, second),
            (
                vec![],
                vec![slot],
                vec!["fn".into(), "main".into(), "let".into(), "\"hi\"".into()]
            )
        );
    }

    // Catches a dead worker leaving every later block plain, or hanging the
    // caller waiting for results that never come.
    #[test]
    fn dead_highlight_worker_is_replaced_and_does_not_hang() {
        let Some(store) = crate::testing::store_with("rust") else {
            return;
        };
        let mut highlighter = DocumentHighlighter::new(store);
        let (slot, code) = (BlockKey(0), "fn main() {}");
        highlighter.version(slot, "rust", code);
        highlighter.kill_worker();

        highlighter.finish_pending();

        assert_eq!(
            colored(&mut highlighter, slot, code),
            vec!["fn".to_owned(), "main".to_owned()]
        );
    }
}
