//! Code block highlighting for markdown messages.
//!
//! With the `syntax` feature, highlighting runs on a `quark-syntax` worker
//! thread. A code block renders plain (or with its previous highlight,
//! while its source only grew) until its result arrives; the app calls
//! [`SyntaxHighlighter::poll`] each frame and rebuilds its markdown
//! messages when it returns blocks. Without the feature every block is
//! plain.

use quark::selection::BlockKey;
use quark_render::FontKind;

use super::SpanTone;
use crate::element::StyledSpan;

/// One line of code as spans with their tones.
pub(crate) type CodeLine = Vec<(StyledSpan, SpanTone)>;

/// Plain monospace lines of `code`.
pub(crate) fn plain_lines(code: &str) -> Vec<CodeLine> {
    code.split('\n')
        .map(|line| vec![(mono(line), SpanTone::Plain)])
        .collect()
}

fn mono(text: &str) -> StyledSpan {
    StyledSpan {
        font_kind: FontKind::Mono,
        ..StyledSpan::plain(text)
    }
}

/// Highlights code blocks off the UI thread. One per app, shared by every
/// markdown message.
#[derive(Default)]
pub struct SyntaxHighlighter {
    #[cfg(feature = "syntax")]
    inner: imp::Inner,
}

impl SyntaxHighlighter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Takes finished highlights and returns the code blocks they are
    /// for; rebuild those blocks' messages so they pick the colors up.
    pub fn poll(&mut self) -> Vec<BlockKey> {
        #[cfg(feature = "syntax")]
        {
            self.inner.poll()
        }
        #[cfg(not(feature = "syntax"))]
        {
            Vec::new()
        }
    }

    /// Blocks until every requested highlight has arrived and been taken.
    /// For tests and screenshots, which need the colored state at once.
    /// Returns the blocks that got a highlight, as [`Self::poll`] does.
    pub fn finish_pending(&mut self) -> Vec<BlockKey> {
        #[cfg(feature = "syntax")]
        {
            self.inner.finish_pending()
        }
        #[cfg(not(feature = "syntax"))]
        {
            Vec::new()
        }
    }

    /// Drops everything kept for the block `slot`: its source, its
    /// highlight, and any pending request's result. Call it when the block
    /// leaves its message or its message is removed.
    pub fn forget(&mut self, slot: BlockKey) {
        #[cfg(feature = "syntax")]
        self.inner.forget(slot);
        #[cfg(not(feature = "syntax"))]
        let _ = slot;
    }

    /// Number of blocks with kept state.
    pub fn len(&self) -> usize {
        #[cfg(feature = "syntax")]
        {
            self.inner.len()
        }
        #[cfg(not(feature = "syntax"))]
        {
            0
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Requests a highlight of `code` for the block `slot` when its source
    /// changed, and returns the version of the highlight that applies to
    /// `code` now: 0 for plain. Blocks with an unchanged version can be
    /// reused as they are.
    pub(crate) fn version(&mut self, slot: BlockKey, lang: &str, code: &str) -> u64 {
        #[cfg(feature = "syntax")]
        {
            self.inner.version(slot, lang, code)
        }
        #[cfg(not(feature = "syntax"))]
        {
            let _ = (slot, lang, code);
            0
        }
    }

    /// The lines of `code` toned by the highlight [`Self::version`]
    /// reported for `slot`.
    pub(crate) fn lines(&self, slot: BlockKey, code: &str) -> Vec<CodeLine> {
        #[cfg(feature = "syntax")]
        {
            self.inner.lines(slot, code)
        }
        #[cfg(not(feature = "syntax"))]
        {
            let _ = slot;
            plain_lines(code)
        }
    }

    /// Replaces the worker with one whose thread is gone.
    #[cfg(all(test, feature = "syntax"))]
    pub(crate) fn kill_worker(&mut self) {
        self.inner.kill_worker();
    }
}

#[cfg(feature = "syntax")]
mod imp {
    use std::collections::HashMap;
    use std::sync::Arc;

    use quark::selection::BlockKey;
    use quark_syntax::{HighlightKind, HighlightSpan, HighlightWorker, LanguageId};

    use super::{CodeLine, mono, plain_lines};
    use crate::document::{SpanTone, SyntaxTone};

    struct Done {
        generation: u64,
        source: Arc<str>,
        spans: Arc<[HighlightSpan]>,
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
        /// Spans of an earlier source still fit a source that only grew,
        /// which keeps a streaming block colored while its new highlight
        /// is computed.
        fn applies_to(&self, code: &str) -> bool {
            code.starts_with(&*self.source)
        }
    }

    #[derive(Default)]
    pub(super) struct Inner {
        worker: Option<HighlightWorker>,
        slots: HashMap<BlockKey, Slot>,
        generation: u64,
    }

    impl Inner {
        pub(super) fn version(&mut self, slot: BlockKey, lang: &str, code: &str) -> u64 {
            let Some(language) = LanguageId::from_fence(lang) else {
                return 0;
            };
            let entry = self.slots.entry(slot).or_insert_with(|| Slot {
                generation: 0,
                language,
                requested: Arc::from(""),
                done: None,
            });
            // Length first: a streaming block grows, so most changes show
            // there without comparing the text.
            let changed = entry.requested.len() != code.len() || *entry.requested != *code;
            if entry.generation == 0 || entry.language != language || changed {
                self.generation += 1;
                entry.generation = self.generation;
                entry.language = language;
                entry.requested = Arc::from(code);
                self.worker
                    .get_or_insert_with(HighlightWorker::new)
                    .request(slot.0, entry.generation, language, entry.requested.clone());
            }
            entry
                .done
                .as_ref()
                .filter(|done| done.applies_to(code))
                .map_or(0, |done| done.generation)
        }

        pub(super) fn lines(&self, slot: BlockKey, code: &str) -> Vec<CodeLine> {
            match self
                .slots
                .get(&slot)
                .and_then(|s| s.done.as_ref())
                .filter(|done| done.applies_to(code))
            {
                Some(done) => colored_lines(code, &done.spans),
                None => plain_lines(code),
            }
        }

        pub(super) fn forget(&mut self, slot: BlockKey) {
            // A result still in flight finds no slot and is dropped.
            self.slots.remove(&slot);
        }

        pub(super) fn len(&self) -> usize {
            self.slots.len()
        }

        pub(super) fn poll(&mut self) -> Vec<BlockKey> {
            let mut changed = Vec::new();
            loop {
                let Some(worker) = &self.worker else {
                    return changed;
                };
                match worker.try_recv() {
                    Ok(Some(result)) => changed.extend(take(&mut self.slots, result)),
                    Ok(None) => return changed,
                    Err(_) => {
                        self.respawn();
                        return changed;
                    }
                }
            }
        }

        pub(super) fn finish_pending(&mut self) -> Vec<BlockKey> {
            let mut changed = self.poll();
            // One respawn per call: a worker that cannot be spawned again
            // leaves the pending blocks plain instead of hanging.
            let mut respawned = false;
            while self.slots.values().any(Slot::pending) {
                let Some(worker) = &self.worker else {
                    break;
                };
                match worker.recv() {
                    Ok(result) => changed.extend(take(&mut self.slots, result)),
                    Err(_) if !respawned => {
                        respawned = true;
                        self.respawn();
                    }
                    Err(_) => break,
                }
            }
            changed
        }

        /// Replaces a dead worker and sends it every request still waiting
        /// for a result.
        fn respawn(&mut self) {
            let worker = self.worker.insert(HighlightWorker::new());
            for (slot, entry) in &self.slots {
                if entry.pending() {
                    worker.request(
                        slot.0,
                        entry.generation,
                        entry.language,
                        entry.requested.clone(),
                    );
                }
            }
        }

        #[cfg(test)]
        pub(super) fn kill_worker(&mut self) {
            self.worker = Some(HighlightWorker::gone());
        }
    }

    /// Keeps a result unless the slot already holds a newer one. A result
    /// older than the newest request is still kept: it applies while the
    /// source only grew, and the newest one may be dropped by the worker's
    /// coalescing in favor of an even newer one.
    fn take(
        slots: &mut HashMap<BlockKey, Slot>,
        result: quark_syntax::Highlighted,
    ) -> Option<BlockKey> {
        let key = BlockKey(result.slot);
        let slot = slots.get_mut(&key)?;
        if slot
            .done
            .as_ref()
            .is_some_and(|done| done.generation >= result.generation)
        {
            return None;
        }
        slot.done = Some(Done {
            generation: result.generation,
            source: result.source,
            spans: result.spans.into(),
        });
        Some(key)
    }

    fn tone(kind: HighlightKind) -> SpanTone {
        let tone = match kind {
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
                return SpanTone::Plain;
            }
        };
        SpanTone::Syntax(tone)
    }

    /// Splits `code` into lines of spans, toning the bytes each highlight
    /// span covers. `spans` are sorted, disjoint, and on char boundaries;
    /// spans reaching past the end of `code` are clipped to it.
    pub(crate) fn colored_lines(code: &str, spans: &[HighlightSpan]) -> Vec<CodeLine> {
        let mut lines = Vec::new();
        let mut next = 0;
        let mut line_start = 0;
        for line in code.split('\n') {
            let line_end = line_start + line.len();
            let mut out = Vec::new();
            let mut at = line_start;
            while next < spans.len() && spans[next].range().end <= line_start {
                next += 1;
            }
            for span in &spans[next..] {
                let range = span.range();
                if range.start >= line_end {
                    break;
                }
                let (start, end) = (range.start.max(at), range.end.min(line_end));
                if start > at {
                    out.push((mono(&code[at..start]), SpanTone::Plain));
                }
                if end > start {
                    out.push((mono(&code[start..end]), tone(span.kind)));
                    at = end;
                }
            }
            if at < line_end || out.is_empty() {
                out.push((mono(&code[at..line_end]), SpanTone::Plain));
            }
            lines.push(out);
            line_start = line_end + 1;
        }
        lines
    }
}

#[cfg(all(test, feature = "syntax"))]
mod tests {
    use proptest::prelude::*;
    use quark_syntax::{HighlightKind, HighlightSpan};

    use super::imp::colored_lines;

    /// 1- to 4-byte chars and newlines, so span edges land next to
    /// multibyte chars and line breaks.
    const PIECES: &[&str] = &["a", " ", "\n", "\u{e9}", "\u{65e5}", "\u{1f600}", "{"];

    proptest! {
        // Catches dropped, repeated, or misordered bytes when highlight
        // spans cross line breaks or sit next to multibyte chars.
        #[test]
        fn colored_lines_join_back_into_the_code(
            pieces in prop::collection::vec(prop::sample::select(PIECES), 0..24),
            cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
        ) {
            let code: String = pieces.concat();
            // Sorted, distinct char boundaries pair up into disjoint spans.
            let bounds: Vec<usize> = code.char_indices().map(|(i, _)| i).chain([code.len()]).collect();
            let mut cuts: Vec<usize> = cuts.iter().map(|c| bounds[c.index(bounds.len())]).collect();
            cuts.sort_unstable();
            cuts.dedup();
            let spans: Vec<HighlightSpan> = cuts
                .chunks_exact(2)
                .map(|pair| HighlightSpan {
                    offset: pair[0] as u32,
                    length: (pair[1] - pair[0]) as u32,
                    kind: HighlightKind::Keyword,
                })
                .collect();

            let lines = colored_lines(&code, &spans);

            let joined: Vec<String> = lines
                .iter()
                .map(|line| line.iter().map(|(span, _)| span.text.as_str()).collect())
                .collect();
            prop_assert_eq!(joined.join("\n"), code);
        }
    }
}
