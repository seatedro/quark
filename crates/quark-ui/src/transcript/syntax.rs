//! Code block highlighting for markdown messages.
//!
//! With the `syntax` feature, highlighting runs on a `quark-syntax` worker
//! thread. A code block renders plain (or with its previous highlight,
//! while its source only grew) until its result arrives; the app calls
//! [`SyntaxHighlighter::poll`] each frame and rebuilds its markdown
//! messages when it returns `true`. Without the feature every block is
//! plain.

use quark::selection::BlockKey;
use quark_render::FontKind;

use crate::element::StyledSpan;
use crate::theme::Theme;

/// Plain monospace lines of `code`.
pub(crate) fn plain_lines(code: &str) -> Vec<Vec<StyledSpan>> {
    code.split('\n')
        .map(|line| vec![mono(line, None)])
        .collect()
}

fn mono(text: &str, color: Option<quark::Color>) -> StyledSpan {
    StyledSpan {
        font_kind: FontKind::Mono,
        color,
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
    /// for; rebuild those blocks' messages (see
    /// [`super::markdown_block_row`]) so they pick the colors up.
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

    /// The lines of `code` colored by the highlight [`Self::version`]
    /// reported for `slot`.
    pub(crate) fn lines(&self, slot: BlockKey, code: &str, theme: &Theme) -> Vec<Vec<StyledSpan>> {
        #[cfg(feature = "syntax")]
        {
            self.inner.lines(slot, code, theme)
        }
        #[cfg(not(feature = "syntax"))]
        {
            let _ = (slot, theme);
            plain_lines(code)
        }
    }
}

#[cfg(feature = "syntax")]
mod imp {
    use std::collections::HashMap;
    use std::sync::Arc;

    use quark::selection::BlockKey;
    use quark_syntax::{HighlightKind, HighlightSpan, HighlightWorker, LanguageId};

    use super::{mono, plain_lines};
    use crate::element::StyledSpan;
    use crate::theme::Theme;

    struct Done {
        generation: u64,
        source: Arc<str>,
        spans: Arc<[HighlightSpan]>,
    }

    struct Slot {
        /// Newest request.
        generation: u64,
        requested: Arc<str>,
        done: Option<Done>,
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
                requested: Arc::from(""),
                done: None,
            });
            if entry.generation == 0 || *entry.requested != *code {
                self.generation += 1;
                entry.generation = self.generation;
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

        pub(super) fn lines(
            &self,
            slot: BlockKey,
            code: &str,
            theme: &Theme,
        ) -> Vec<Vec<StyledSpan>> {
            match self
                .slots
                .get(&slot)
                .and_then(|s| s.done.as_ref())
                .filter(|done| done.applies_to(code))
            {
                Some(done) => colored_lines(code, &done.spans, theme),
                None => plain_lines(code),
            }
        }

        pub(super) fn poll(&mut self) -> Vec<BlockKey> {
            let mut changed = Vec::new();
            let Some(worker) = &self.worker else {
                return changed;
            };
            while let Some(result) = worker.try_recv() {
                changed.extend(take(&mut self.slots, result));
            }
            changed
        }

        pub(super) fn finish_pending(&mut self) -> Vec<BlockKey> {
            let mut changed = self.poll();
            loop {
                let pending = self.slots.values().any(|slot| {
                    slot.done
                        .as_ref()
                        .is_none_or(|done| done.generation < slot.generation)
                });
                let Some(worker) = self.worker.as_ref().filter(|_| pending) else {
                    return changed;
                };
                let Some(result) = worker.recv() else {
                    return changed;
                };
                changed.extend(take(&mut self.slots, result));
            }
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

    fn color(kind: HighlightKind, theme: &Theme) -> Option<quark::Color> {
        let c = &theme.colors;
        Some(match kind {
            HighlightKind::Keyword | HighlightKind::Preprocessor => c.syntax_keyword,
            HighlightKind::String => c.syntax_string,
            HighlightKind::Comment => c.syntax_comment,
            HighlightKind::Function => c.syntax_function,
            HighlightKind::Type | HighlightKind::Namespace => c.syntax_type,
            HighlightKind::Number | HighlightKind::Constant | HighlightKind::Builtin => {
                c.syntax_number
            }
            HighlightKind::Property
            | HighlightKind::Attribute
            | HighlightKind::Tag
            | HighlightKind::Label => c.syntax_property,
            HighlightKind::Operator => c.syntax_operator,
            HighlightKind::Normal | HighlightKind::Punctuation | HighlightKind::Variable => {
                return None;
            }
        })
    }

    /// Splits `code` into lines of spans, coloring the bytes each highlight
    /// span covers. `spans` are sorted, disjoint, and on char boundaries.
    fn colored_lines(code: &str, spans: &[HighlightSpan], theme: &Theme) -> Vec<Vec<StyledSpan>> {
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
                let (start, end) = (range.start.max(line_start), range.end.min(line_end));
                if start > at {
                    out.push(mono(&code[at..start], None));
                }
                if end > start {
                    out.push(mono(&code[start..end], color(span.kind, theme)));
                    at = end;
                }
            }
            if at < line_end || out.is_empty() {
                out.push(mono(&code[at..line_end], None));
            }
            lines.push(out);
            line_start = line_end + 1;
        }
        lines
    }
}
