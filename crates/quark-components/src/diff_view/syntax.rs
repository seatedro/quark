//! Syntax coordination for the diff view: one highlight request per file
//! side to a `quark-syntax` worker, the spans each side holds, and the
//! per-file generation row stamps hash so rows repaint as files finish.
//!
//! This is the bridge between the view state and the syntax layer. The
//! state calls only the methods below; everything else here may change
//! without touching the state.
//!
//! Coordinates: `file` is the document's file index, `side` the
//! [`Side`], and every range is a half-open UTF-8 byte range of that
//! side's [`quark_diff::TextStore`]. Layout spans returned by
//! [`DiffSyntax::spans`] are relative to the start of the requested range.
//!
//! Errors: requests for files without a known language, empty sides, and
//! files out of range are skipped; results for another document
//! generation, an unknown file, or an older revision than the one held
//! are dropped. Nothing here panics on stale input.

use std::ops::Range;
use std::sync::Arc;

use quark_diff::{DiffDocument, Side};
use quark_syntax::{
    GrammarStore, HighlightKind, HighlightSpan, HighlightWorker, Highlighted, LanguageId,
};
use quark_text::TextSpan;

/// Highlights held for one document. See the [module docs](self).
#[derive(Default)]
pub(crate) struct DiffSyntax {
    worker: Option<HighlightWorker>,
    highlights: Vec<[Option<Arc<[HighlightSpan]>>; 2]>,
    /// Revision of each held highlight, which a newer result of the same
    /// document must exceed (grammars arriving recolor it).
    revisions: Vec<[Option<u32>; 2]>,
    /// Bumped per file each time one of its sides recolors.
    generations: Vec<u32>,
}

impl DiffSyntax {
    /// Starts highlighting on a background thread with `store`'s grammars.
    pub fn enable(&mut self, store: GrammarStore) {
        self.worker = Some(HighlightWorker::new(store));
    }

    /// Forgets every held highlight, for a document of `files` files.
    pub fn reset(&mut self, files: usize) {
        self.highlights = vec![[None, None]; files];
        self.revisions = vec![[None, None]; files];
        self.generations = vec![0; files];
    }

    /// Asks the worker for every side of `doc` whose path names a known
    /// language, tagged with the document's `generation`.
    pub fn request(&self, doc: &DiffDocument, generation: u64) {
        let Some(worker) = &self.worker else {
            return;
        };
        for file in 0..doc.file_count() {
            let Some(language) = LanguageId::from_path(doc.path(file)) else {
                continue;
            };
            for side in [Side::Old, Side::New] {
                let source = doc.text(file, side).shared().clone();
                if !source.is_empty() {
                    let slot = u64::from(file) * 2 + side as u64;
                    worker.request(slot, generation, language.clone(), source);
                }
            }
        }
    }

    /// Takes every finished result. Returns whether any recolored a file.
    pub fn poll(&mut self, generation: u64) -> bool {
        let Some(worker) = &self.worker else {
            return false;
        };
        let mut results = Vec::new();
        while let Ok(Some(done)) = worker.try_recv() {
            results.push(done);
        }
        let mut changed = false;
        for done in results {
            changed |= self.take(done, generation);
        }
        changed
    }

    /// Keeps a result for the document of `generation` unless the file
    /// side holds the same or a later revision; results for earlier
    /// documents are dropped. Returns whether it was kept.
    pub fn take(&mut self, done: Highlighted, generation: u64) -> bool {
        let (file, side) = ((done.slot / 2) as usize, (done.slot % 2) as usize);
        if done.generation != generation || file >= self.highlights.len() {
            return false;
        }
        if self.revisions[file][side].is_some_and(|held| held >= done.revision) {
            return false;
        }
        self.revisions[file][side] = Some(done.revision);
        self.highlights[file][side] = Some(done.spans.into());
        self.generations[file] += 1;
        true
    }

    /// How many times `file` has recolored, for row stamps.
    pub fn file_generation(&self, file: u32) -> Option<u32> {
        self.generations.get(file as usize).copied()
    }

    /// Layout spans splitting `range` of a store at highlight boundaries,
    /// with each span's kind. Empty when the side holds no highlight.
    pub fn spans(
        &self,
        file: u32,
        side: Side,
        range: Range<usize>,
    ) -> (Vec<TextSpan>, Arc<[HighlightKind]>) {
        let Some(spans) = self
            .highlights
            .get(file as usize)
            .and_then(|h| h[side as usize].as_ref())
        else {
            return (Vec::new(), Arc::from([]));
        };
        let mut out = Vec::new();
        let mut tones = Vec::new();
        let base = range.start;
        let mut push = |from: usize, to: usize, kind: HighlightKind| {
            if from < to {
                out.push(TextSpan {
                    range: from - base..to - base,
                    weight: None,
                    style: None,
                    kind: None,
                });
                tones.push(kind);
            }
        };
        let mut at = range.start;
        let first = spans.partition_point(|s| s.range().end <= range.start);
        for span in &spans[first..] {
            let r = span.range();
            if r.start >= range.end {
                break;
            }
            let (from, to) = (r.start.max(range.start), r.end.min(range.end));
            push(at, from, HighlightKind::Normal);
            push(from, to, span.kind);
            at = to;
        }
        if at > range.start {
            push(at, range.end, HighlightKind::Normal);
        }
        (out, tones.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Catches progressive highlights being dropped or stale ones winning:
    // later revisions for the shown document recolor it, while earlier
    // revisions and results for a replaced document do not.
    #[test]
    fn highlights_replace_only_when_newer_for_the_shown_document() {
        let mut syntax = DiffSyntax::default();
        syntax.reset(1);
        let slot = Side::New as u64;
        let mut held = Vec::new();
        // (document generation, revision, highlighted length)
        for (generation, revision, length) in
            [(1, 0, 2), (1, 1, 5), (1, 0, 1), (0, 2, 9), (1, 1, 7)]
        {
            syntax.take(
                Highlighted {
                    slot,
                    generation,
                    revision,
                    source: Arc::from("fn b() {}\n"),
                    spans: vec![HighlightSpan {
                        offset: 0,
                        length,
                        kind: HighlightKind::Keyword,
                    }],
                    pending: false,
                    unresolved: Vec::new(),
                },
                1,
            );
            let (spans, _) = syntax.spans(0, Side::New, 0..9);
            held.push(spans.first().map(|span| span.range.clone()));
        }

        assert_eq!(
            held,
            [Some(0..2), Some(0..5), Some(0..5), Some(0..5), Some(0..5)]
        );
    }
}
