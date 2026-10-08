use std::mem;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::epoch::FontEpoch;
use crate::layout::{TextError, TextLayout, TextQuery, TextSpan, TextStyle};
use crate::source::TextSource;
use crate::system::TextSystem;

/// Text that changes in place over time (an editor paragraph, a transcript
/// block, a diff cell, a terminal row), laid out from storage kept across
/// its revisions.
///
/// Each revision's text is a [`TextSource`] snapshot, and each layout of it
/// shares that snapshot. An edit writes the next revision over the storage
/// of an earlier one once nothing else holds it, and lays out into an
/// earlier layout the same way, so edits of any length within the capacity
/// already grown allocate no copy of the text. Layouts and sources someone
/// still holds (a scene, a selection region) keep their revision unchanged;
/// the block keeps a few of them to reuse once released, and allocates when
/// it has none to spare. A font change lays the text out again, into the
/// same storage: no layout shaped with other fonts is returned.
///
/// For text with no owner to keep across frames, such as labels, use
/// [`LayoutCache`](crate::LayoutCache) instead.
#[derive(Debug)]
pub struct TextBlock {
    id: u64,
    revision: u64,
    source: TextSource,
    /// Layouts of this revision, oldest first.
    layouts: Vec<Arc<TextLayout>>,
    /// The fonts `layouts` were shaped with.
    fonts: Option<FontEpoch>,
    /// Layouts nothing else holds, their sources let go, to lay out into.
    spare_layouts: Vec<Arc<TextLayout>>,
    /// Layouts of earlier revisions something else held when they were
    /// replaced, oldest first.
    retired_layouts: Vec<Arc<TextLayout>>,
    /// Sources of earlier revisions, to write over once released, oldest
    /// first.
    spare_sources: Vec<TextSource>,
}

/// Layouts of one revision a block keeps (an unwrapped one for measuring
/// and a few widths, say).
const MAX_LAYOUTS: usize = 4;
/// Layouts and sources a block keeps for reuse.
const MAX_SPARE: usize = 4;

impl TextBlock {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            revision: 0,
            source: TextSource::empty(),
            layouts: Vec::new(),
            fonts: None,
            spare_layouts: Vec::new(),
            retired_layouts: Vec::new(),
            spare_sources: Vec::new(),
        }
    }

    /// Identifies this block for as long as it lives, whatever its text.
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Counts the changes to the text and spans; setting the same ones
    /// again is not a change.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn text(&self) -> &str {
        self.source.as_str()
    }

    pub fn spans(&self) -> &[TextSpan] {
        self.source.spans()
    }

    /// This revision's text and spans, shared.
    pub fn source(&self) -> &TextSource {
        &self.source
    }

    /// Sets the text, with no spans.
    pub fn set_text(&mut self, text: &str) {
        self.set(text, &[]);
    }

    /// Sets the spans over the current text.
    pub fn set_spans(&mut self, spans: &[TextSpan]) {
        // The text moves into the next storage, so it is read from a
        // shared handle rather than the storage being written.
        let source = self.source.clone();
        self.set(source.as_str(), spans);
    }

    /// Sets the text and spans as one revision.
    pub fn set(&mut self, text: &str, spans: &[TextSpan]) {
        if self.source.as_str() == text && self.source.spans() == spans {
            return;
        }
        self.revision += 1;
        self.retire_layouts();
        if self.source.overwrite(text, spans) {
            return;
        }
        // Someone holds this revision's text: it stays as it is for them.
        let spare = (0..self.spare_sources.len())
            .find(|&i| self.spare_sources[i].overwrite(text, spans))
            .map(|i| self.spare_sources.remove(i));
        let next = spare.unwrap_or_else(|| TextSource::new(text, spans));
        let previous = mem::replace(&mut self.source, next);
        // The empty source a block starts with has nothing to reuse.
        if previous.capacity() > 0 {
            if self.spare_sources.len() == MAX_SPARE {
                self.spare_sources.remove(0);
            }
            self.spare_sources.push(previous);
        }
    }

    /// This revision laid out with `style`, wrapped at `wrap_width` logical
    /// pixels (`None` for no wrapping), shaped at `scale_factor`. Returns
    /// the same layout until the text, spans, settings, or `system`'s fonts
    /// change.
    pub fn layout(
        &mut self,
        system: &mut TextSystem,
        style: TextStyle,
        wrap_width: Option<f32>,
        scale_factor: f32,
    ) -> Result<Arc<TextLayout>, TextError> {
        let same = |layout: &TextLayout| {
            layout.style() == style
                && layout.wrap_width().map(f32::to_bits) == wrap_width.map(f32::to_bits)
                && layout.scale_factor().to_bits() == scale_factor.to_bits()
        };
        let fonts = system.font_epoch();
        if self.fonts != Some(fonts) {
            // Shaped with other fonts: never returned again, only refilled.
            self.retire_layouts();
            self.fonts = Some(fonts);
        } else if let Some(layout) = self.layouts.iter().find(|layout| same(layout)) {
            return Ok(layout.clone());
        }
        self.collect_released();
        let source = &self.source;
        let query = TextQuery {
            text: source.as_str(),
            spans: source.spans(),
            style,
            wrap_width,
            scale_factor,
        };
        query.validate()?;
        let layout = TextLayout::refill(self.spare_layouts.pop(), |own| {
            own.share_source(source, &query);
            system.rebuild(own);
        });
        if self.layouts.len() == MAX_LAYOUTS {
            let oldest = self.layouts.remove(0);
            self.retire(oldest);
        }
        self.layouts.push(layout.clone());
        Ok(layout)
    }

    /// Whether this block keeps a layout its next revision can be laid out
    /// into: one released by its holders, or one of this revision nothing
    /// else holds.
    pub fn keeps_spare(&mut self) -> bool {
        self.collect_released();
        !self.spare_layouts.is_empty()
            || (self.layouts.iter_mut()).any(|layout| Arc::get_mut(layout).is_some())
    }

    /// Moves a released layout this block keeps to `to`, with a released
    /// source when `to` keeps none, returning whether it did. For a set of
    /// blocks whose text changes unevenly (rows of a terminal): a block
    /// whose earlier layouts are all still held lays its next revision out
    /// into storage another block has released, rather than allocating.
    pub fn give_spare(&mut self, to: &mut TextBlock) -> bool {
        if to.spare_layouts.len() == MAX_SPARE {
            return false;
        }
        let layout = match self.spare_layouts.pop() {
            Some(layout) => layout,
            None => {
                let released = (self.retired_layouts.iter_mut())
                    .position(|layout| Arc::get_mut(layout).is_some());
                let Some(i) = released else {
                    return false;
                };
                let mut layout = self.retired_layouts.remove(i);
                if let Some(own) = Arc::get_mut(&mut layout) {
                    own.detach_source();
                }
                layout
            }
        };
        to.spare_layouts.push(layout);
        if !to.spare_sources.iter_mut().any(TextSource::is_unshared)
            && let Some(i) =
                (0..self.spare_sources.len()).find(|&i| self.spare_sources[i].is_unshared())
        {
            to.spare_sources.push(self.spare_sources.remove(i));
        }
        true
    }

    /// Retires every layout of this revision.
    fn retire_layouts(&mut self) {
        let mut layouts = mem::take(&mut self.layouts);
        for layout in layouts.drain(..) {
            self.retire(layout);
        }
        self.layouts = layouts;
        self.collect_released();
    }

    /// Keeps a layout this block no longer returns: to lay out into when
    /// nothing else holds it, unchanged until released otherwise.
    fn retire(&mut self, mut layout: Arc<TextLayout>) {
        match Arc::get_mut(&mut layout) {
            Some(own) => {
                // Let go of the source, so the next revision can be written
                // over it.
                own.detach_source();
                if self.spare_layouts.len() < MAX_SPARE {
                    self.spare_layouts.push(layout);
                }
            }
            None => {
                // A holder that never lets go must not block later ones.
                if self.retired_layouts.len() == MAX_SPARE {
                    self.retired_layouts.remove(0);
                }
                self.retired_layouts.push(layout);
            }
        }
    }

    /// Moves retired layouts their holders released to the spares.
    fn collect_released(&mut self) {
        let released =
            (self.retired_layouts).extract_if(.., |layout| Arc::get_mut(layout).is_some());
        for mut layout in released {
            if let Some(own) = Arc::get_mut(&mut layout) {
                own.detach_source();
            }
            if self.spare_layouts.len() < MAX_SPARE {
                self.spare_layouts.push(layout);
            }
        }
    }
}

impl Default for TextBlock {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::layout::TextParams;
    use crate::system::test_system;

    const STYLE: TextStyle = TextStyle {
        font_kind: quark::FontKind::Ui,
        font_weight: quark::FontWeight::Normal,
        font_size: 14.0,
        line_height: 19.0,
        family: None,
        letter_spacing: 0.0,
        thicken: false,
        linear_correction: None,
    };

    /// Everything a layout shows and hits against.
    fn dump(layout: &TextLayout) -> String {
        format!(
            "{:?}\n{:?}\n{:?}\n{:?}",
            layout.text(),
            layout.size(),
            layout.lines().collect::<Vec<_>>(),
            layout.glyphs(),
        )
    }

    /// Text of one revision: multibyte, combining, emoji, right-to-left,
    /// and line-ending pieces, so edits change byte length unevenly.
    fn revision_text() -> impl Strategy<Value = String> {
        const PIECES: &[&str] = &[
            "a",
            "word ",
            "na\u{ef}ve ",
            "e\u{301}",
            "\u{65e5}\u{672c}\u{8a9e}",
            "\u{1f469}\u{200d}\u{1f4bb}",
            "\u{5e9}\u{5dc}\u{5d5}\u{5dd} ",
            "\n",
        ];
        prop::collection::vec(prop::sample::select(PIECES), 0..14).prop_map(|p| p.concat())
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(24))]

        // Each revision is written over storage an earlier one used, at
        // another length; leftover text, spans, or shaping from that
        // revision must not leak into the new layout. Every other layout
        // is held, as a scene would, so both the in-place and the spare
        // storage paths run.
        #[test]
        fn every_revision_lays_out_like_fresh_text(
            revisions in prop::collection::vec(revision_text(), 1..8),
            wrap in prop::option::of(40.0f32..300.0),
        ) {
            let mut system = test_system();
            let mut block = TextBlock::new();
            let mut held = Vec::new();
            for (i, text) in revisions.iter().enumerate() {
                block.set_text(text);
                let layout = block.layout(&mut system, STYLE, wrap, 1.0).expect("layout");
                let fresh = system
                    .layout(&TextParams::new(text.as_str(), STYLE).wrap_width(wrap))
                    .expect("layout");
                prop_assert_eq!(dump(&layout), dump(&fresh), "revision {}", i);
                if i % 2 == 0 {
                    held.push(layout);
                }
            }
        }
    }

    // A font change must reach a block's layouts, which it otherwise
    // returns until the text or settings change: the next layout is shaped
    // with the new fonts.
    #[test]
    fn a_font_change_lays_the_text_out_again() {
        // Names a family only the loaded font has, so until then the
        // default one stands in. Its own system: changing fonts on the
        // shared one would race other tests.
        let mut system = TextSystem::vendored_only(&crate::FontSettings {
            ui_family: crate::system::RENAMED_INTER.into(),
            ..crate::FontSettings::default()
        });
        let mut block = TextBlock::new();
        block.set_text("iiiiMMMM");
        let before = block.layout(&mut system, STYLE, None, 1.0).expect("layout");
        let before = dump(&before);
        system.load_font_data(Arc::new(crate::system::renamed_inter()));

        let after = block.layout(&mut system, STYLE, None, 1.0).expect("layout");
        let fresh = system
            .layout(&TextParams::new("iiiiMMMM", STYLE))
            .expect("layout");
        assert_ne!(before, dump(&fresh), "the loaded font changes the text");
        assert_eq!(dump(&after), dump(&fresh));
    }

    // A selection region or a scene keeps the layout it was painted with
    // while the text is edited. Each must keep its text and glyphs, so
    // copying a selection from it yields what was on screen.
    #[test]
    fn layouts_held_across_revisions_keep_their_text_and_glyphs() {
        let mut system = test_system();
        let mut block = TextBlock::new();
        let texts = [
            "the quick brown fox",
            "fox",
            "\u{65e5}\u{672c}\u{8a9e} fox \u{1f469}\u{200d}\u{1f4bb}",
            "a much longer line that grows past every earlier fox revision",
            "na\u{ef}ve fox",
        ];
        let mut held = Vec::new();
        for text in texts {
            block.set_text(text);
            let layout = block.layout(&mut system, STYLE, None, 1.0).expect("layout");
            let fox = text.find("fox").expect("fixture has a fox");
            held.push((layout.source().clone(), dump(&layout), layout, fox));
        }
        block.set_text("ok");
        block.layout(&mut system, STYLE, None, 1.0).expect("layout");

        for ((source, before, layout, fox), text) in held.iter().zip(texts) {
            assert_eq!(&dump(layout), before);
            assert_eq!(source.as_str(), text);
            assert_eq!(layout.text().get(*fox..fox + 3), Some("fox"));
        }
    }
}
