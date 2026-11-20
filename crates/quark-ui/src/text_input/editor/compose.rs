//! An IME composition on screen: the lines the selection touches, laid out
//! again with the preedit spliced in over the selection, painted in their
//! place while the IME composes. Only those lines are laid out again.

use std::sync::Arc;

use quark_text::{TextLayout, TextOffset, TextSystem};

use super::Editor;
use super::spans::{SyntaxTokenKind, shift_spans};
use crate::text_input::ime::{Composition, compose};

/// The composed lines `first..=last`.
#[derive(Debug, Clone)]
pub(super) struct ComposedBlock {
    pub first: usize,
    pub last: usize,
    /// Where line `first` starts in the text.
    pub start: usize,
    /// The lines' text with the preedit, laid out as one.
    pub layout: Arc<TextLayout>,
    pub kinds: Arc<[SyntaxTokenKind]>,
    /// Offsets into `layout`'s text.
    pub composition: Composition,
    pub height: f32,
}

impl Editor {
    /// Height the composed lines take when not composing.
    pub(super) fn block_committed_height(&self, block: &ComposedBlock) -> f32 {
        self.paragraphs.top(block.last + 1) - self.paragraphs.top(block.first)
    }

    /// Lay out the lines under the selection with the preedit spliced in,
    /// reusing their spans shifted around it.
    pub(super) fn rebuild_composition(&mut self, text_system: &mut TextSystem) {
        self.composition = None;
        let Some(preedit) = self.buffer.preedit().cloned() else {
            return;
        };
        let rope = self.buffer.rope().clone();
        let selection = self.buffer.selection();
        let (first, last) = (
            rope.byte_to_line(selection.start.get()),
            rope.byte_to_line(selection.end.get()),
        );
        let start = rope.line_to_byte(first);
        let text = rope.slice(start..rope.line_content_range(last).end);
        let local = TextOffset::snap(&text, selection.start.get() - start)
            ..TextOffset::snap(&text, selection.end.get() - start);
        let composition = compose(&text, local.clone(), &preedit);
        let (mut spans, mut kinds) = (Vec::new(), Vec::new());
        for line in first..=last {
            let Some(shaped) = self.paragraphs.shaped(line) else {
                continue;
            };
            let base = rope.line_to_byte(line) - start;
            for (span, kind) in shaped.layout.spans().iter().zip(shaped.kinds.iter()) {
                let mut span = span.clone();
                span.range = span.range.start + base..span.range.end + base;
                spans.push(span);
                kinds.push(*kind);
            }
        }
        let inserted = composition.preedit.end.get() - composition.preedit.start.get();
        let (spans, kinds) =
            shift_spans(&spans, &kinds, local.start.get()..local.end.get(), inserted);
        let params = self.layout_params(Arc::from(composition.text.as_str()), spans);
        let Ok(layout) = text_system.layout(&params) else {
            return;
        };
        let height = layout.size().1.max(self.line_height());
        self.composition = Some(ComposedBlock {
            first,
            last,
            start,
            layout: Arc::new(layout),
            kinds,
            composition,
            height,
        });
    }
}
