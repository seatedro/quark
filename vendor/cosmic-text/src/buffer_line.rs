#[cfg(not(feature = "std"))]
use alloc::{string::String, vec::Vec};
use core::mem;
use core::ops::Range;

use crate::{
    Align, Attrs, AttrsList, Cached, FontSystem, LayoutGlyph, LayoutLine, LineEnding, ShapeLine,
    Shaping, Wrap,
};

/// A line (or paragraph) of text that is shaped and laid out
#[derive(Clone, Debug)]
pub struct BufferLine {
    text: String,
    ending: LineEnding,
    attrs_list: AttrsList,
    align: Option<Align>,
    shape_opt: Cached<ShapeLine>,
    layout_opt: Cached<Vec<LayoutLine>>,
    shaping: Shaping,
    metadata: Option<usize>,
    /// Byte ranges word wrapping keeps on one line when they fit on one;
    /// see [`Self::set_keep_together_ranges`].
    keep_together: Vec<Range<usize>>,
}

impl BufferLine {
    /// Create a new line with the given text and attributes list
    /// Cached shaping and layout can be done using the [`Self::shape`] and
    /// [`Self::layout`] functions
    pub fn new<T: Into<String>>(
        text: T,
        ending: LineEnding,
        attrs_list: AttrsList,
        shaping: Shaping,
    ) -> Self {
        Self {
            text: text.into(),
            ending,
            attrs_list,
            align: None,
            shape_opt: Cached::Empty,
            layout_opt: Cached::Empty,
            shaping,
            metadata: None,
            keep_together: Vec::new(),
        }
    }

    /// Resets the current line with new internal values.
    ///
    /// Avoids deallocating internal caches so they can be reused.
    pub fn reset_new<T: Into<String>>(
        &mut self,
        text: T,
        ending: LineEnding,
        attrs_list: AttrsList,
        shaping: Shaping,
    ) {
        self.text = text.into();
        self.ending = ending;
        self.attrs_list = attrs_list;
        self.align = None;
        self.shape_opt.set_unused();
        self.layout_opt.set_unused();
        self.shaping = shaping;
        self.metadata = None;
        self.keep_together.clear();
    }

    /// Heap bytes the line keeps, at capacity: its text, attributes, and
    /// shaping and layout, including those kept unused for reuse.
    pub fn storage_bytes(&self) -> usize {
        let shape = match &self.shape_opt {
            Cached::Empty => 0,
            Cached::Unused(shape) | Cached::Used(shape) => shape.storage_bytes(),
        };
        let layout = match &self.layout_opt {
            Cached::Empty => 0,
            Cached::Unused(lines) | Cached::Used(lines) => {
                lines.capacity() * mem::size_of::<LayoutLine>()
                    + lines
                        .iter()
                        .map(|line| line.glyphs.capacity() * mem::size_of::<LayoutGlyph>())
                        .sum::<usize>()
            }
        };
        self.text.capacity()
            + self.attrs_list.storage_bytes()
            + self.keep_together.capacity() * mem::size_of::<Range<usize>>()
            + shape
            + layout
    }

    /// Get current text
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Set text and attributes list
    ///
    /// Will reset shape and layout if it differs from current text and attributes list.
    /// New text clears the keep-together ranges.
    /// Returns true if the line was reset
    pub fn set_text<T: AsRef<str>>(
        &mut self,
        text: T,
        ending: LineEnding,
        attrs_list: AttrsList,
    ) -> bool {
        let text = text.as_ref();
        if text != self.text || ending != self.ending || attrs_list != self.attrs_list {
            if text != self.text {
                self.keep_together.clear();
            }
            self.text.clear();
            self.text.push_str(text);
            self.ending = ending;
            self.attrs_list = attrs_list;
            self.reset();
            true
        } else {
            false
        }
    }

    /// Consume this line, returning only its text contents as a String.
    pub fn into_text(self) -> String {
        self.text
    }

    /// Get line ending
    pub const fn ending(&self) -> LineEnding {
        self.ending
    }

    /// Set line ending
    ///
    /// Will reset shape and layout if it differs from current line ending.
    /// Returns true if the line was reset
    pub fn set_ending(&mut self, ending: LineEnding) -> bool {
        if ending != self.ending {
            self.ending = ending;
            self.reset_shaping();
            true
        } else {
            false
        }
    }

    /// Get attributes list
    pub const fn attrs_list(&self) -> &AttrsList {
        &self.attrs_list
    }

    /// Set attributes list
    ///
    /// Will reset shape and layout if it differs from current attributes list.
    /// Returns true if the line was reset
    pub fn set_attrs_list(&mut self, attrs_list: AttrsList) -> bool {
        if attrs_list != self.attrs_list {
            self.attrs_list = attrs_list;
            self.reset_shaping();
            true
        } else {
            false
        }
    }

    /// Get the Text alignment
    pub const fn align(&self) -> Option<Align> {
        self.align
    }

    /// Set the text alignment
    ///
    /// Will reset layout if it differs from current alignment.
    /// Setting to None will use `Align::Right` for RTL lines, and `Align::Left` for LTR lines.
    /// Returns true if the line was reset
    pub fn set_align(&mut self, align: Option<Align>) -> bool {
        if align != self.align {
            self.align = align;
            self.reset_layout();
            true
        } else {
            false
        }
    }

    /// Append line at end of this line
    ///
    /// The wrap setting of the appended line will be lost
    pub fn append(&mut self, other: &Self) {
        let len = self.text.len();
        self.text.push_str(other.text());
        self.keep_together.extend(
            other
                .keep_together
                .iter()
                .map(|range| range.start + len..range.end + len),
        );

        // To preserve line endings, we use the one from the other line
        self.ending = other.ending();

        if other.attrs_list.defaults() != self.attrs_list.defaults() {
            // If default formatting does not match, make a new span for it
            self.attrs_list
                .add_span(len..len + other.text().len(), &other.attrs_list.defaults());
        }

        for (other_range, attrs) in other.attrs_list.spans_iter() {
            // Add previous attrs spans
            let range = other_range.start + len..other_range.end + len;
            self.attrs_list.add_span(range, &attrs.as_attrs());
        }

        self.reset();
    }

    /// Split off new line at index
    pub fn split_off(&mut self, index: usize) -> Self {
        let text = self.text.split_off(index);
        let attrs_list = self.attrs_list.split_off(index);
        self.reset();

        let mut new = Self::new(text, self.ending, attrs_list, self.shaping);
        // Ranges past `index` move to the new line; one across it splits.
        let first_moved = self.keep_together.partition_point(|r| r.end <= index);
        new.keep_together.extend(
            self.keep_together[first_moved..]
                .iter()
                .map(|r| r.start.max(index) - index..r.end - index),
        );
        let straddles = self
            .keep_together
            .get(first_moved)
            .is_some_and(|r| r.start < index);
        self.keep_together
            .truncate(first_moved + usize::from(straddles));
        if straddles {
            self.keep_together[first_moved].end = index;
        }
        // To preserve line endings, it moves to the new line
        self.ending = LineEnding::None;
        new.align = self.align;
        new
    }

    /// Reset shaping, layout, and metadata caches
    pub fn reset(&mut self) {
        self.metadata = None;
        self.reset_shaping();
    }

    /// Reset shaping and layout caches
    pub fn reset_shaping(&mut self) {
        self.shape_opt.set_unused();
        self.reset_layout();
    }

    /// Reset only layout cache
    pub fn reset_layout(&mut self) {
        self.layout_opt.set_unused();
    }

    /// Shape line, will cache results
    #[allow(clippy::missing_panics_doc)]
    pub fn shape(&mut self, font_system: &mut FontSystem, tab_width: u16) -> &ShapeLine {
        if self.shape_opt.is_unused() {
            let mut line = self
                .shape_opt
                .take_unused()
                .unwrap_or_else(ShapeLine::empty);
            line.build(
                font_system,
                &self.text,
                &self.attrs_list,
                self.shaping,
                tab_width,
            );
            self.shape_opt.set_used(line);
            self.layout_opt.set_unused();
        }
        self.shape_opt.get().expect("shape not found")
    }

    /// Takes the shaping out, keeping the layout made from it: a line that
    /// is laid out for good then lends its shaping storage to another
    /// line's next shaping (see [`Self::lend_shape_storage`]). A used
    /// shaping leaves an empty one with the same direction and metrics
    /// behind, since [`crate::LayoutRunIter`] reads the direction of every
    /// line it lays out runs for.
    pub fn take_shape(&mut self) -> Option<ShapeLine> {
        match core::mem::replace(&mut self.shape_opt, Cached::Empty) {
            Cached::Empty => None,
            Cached::Unused(shape) => Some(shape),
            Cached::Used(shape) => {
                self.shape_opt = Cached::Used(ShapeLine {
                    rtl: shape.rtl,
                    spans: Vec::new(),
                    metrics_opt: shape.metrics_opt,
                });
                Some(shape)
            }
        }
    }

    /// Gives a line that holds no shaping storage `shape`'s, for its next
    /// shaping to build in rather than allocate; a line with storage of its
    /// own hands `shape` back. The empty shaping [`Self::take_shape`]
    /// leaves holds none, once the line's text has changed.
    pub fn lend_shape_storage(&mut self, shape: ShapeLine) -> Option<ShapeLine> {
        let holds_none = match &self.shape_opt {
            Cached::Empty => true,
            Cached::Unused(own) => own.spans.capacity() == 0,
            Cached::Used(_) => false,
        };
        if holds_none {
            self.shape_opt = Cached::Unused(shape);
            None
        } else {
            Some(shape)
        }
    }

    /// Sets byte ranges of the line (sorted, disjoint, on char boundaries)
    /// that word wrapping keeps on one line when each fits on one: it does
    /// not break between the words a range's bytes overlap, and moves them
    /// to the next line together. A range wider than the line wraps as if it
    /// were not there. Keeps the storage. Resets layout if they changed.
    /// Returns true if the line was reset
    pub fn set_keep_together_ranges(&mut self, ranges: &[Range<usize>]) -> bool {
        if ranges == self.keep_together.as_slice() {
            return false;
        }
        debug_assert!(
            ranges
                .windows(2)
                .all(|pair| pair[0].start < pair[0].end && pair[0].end <= pair[1].start),
            "keep-together ranges must be sorted and disjoint"
        );
        self.keep_together.clear();
        self.keep_together.extend_from_slice(ranges);
        self.reset_layout();
        true
    }

    /// Get line shaping cache
    pub const fn shape_opt(&self) -> Option<&ShapeLine> {
        self.shape_opt.get()
    }

    /// Layout line, will cache results
    #[allow(clippy::missing_panics_doc)]
    pub fn layout(
        &mut self,
        font_system: &mut FontSystem,
        font_size: f32,
        width_opt: Option<f32>,
        wrap: Wrap,
        match_mono_width: Option<f32>,
        tab_width: u16,
    ) -> &[LayoutLine] {
        if self.layout_opt.is_unused() {
            let align = self.align;
            let mut layout = self
                .layout_opt
                .take_unused()
                .unwrap_or_else(|| Vec::with_capacity(1));
            self.shape(font_system, tab_width);
            let shape = self.shape_opt.get().expect("shape not found");
            shape.layout_to_buffer_keeping(
                &mut font_system.shape_buffer,
                font_size,
                width_opt,
                wrap,
                align,
                &mut layout,
                match_mono_width,
                &self.keep_together,
            );
            self.layout_opt.set_used(layout);
        }
        self.layout_opt.get().expect("layout not found")
    }

    /// Get line layout cache
    pub const fn layout_opt(&self) -> Option<&Vec<LayoutLine>> {
        self.layout_opt.get()
    }

    /// Get line metadata. This will be None if [`BufferLine::set_metadata`] has not been called
    /// after the last reset of shaping and layout caches
    pub const fn metadata(&self) -> Option<usize> {
        self.metadata
    }

    /// Set line metadata. This is stored until the next line reset
    pub fn set_metadata(&mut self, metadata: usize) {
        self.metadata = Some(metadata);
    }

    /// Makes an empty buffer line.
    ///
    /// The buffer line is in an invalid state after this is called. See [`Self::reset_new`].
    pub(crate) fn empty() -> Self {
        Self {
            text: String::default(),
            ending: LineEnding::None,
            attrs_list: AttrsList::new(&Attrs::new()),
            align: None,
            shape_opt: Cached::Empty,
            layout_opt: Cached::Empty,
            shaping: Shaping::Advanced,
            metadata: None,
            keep_together: Vec::new(),
        }
    }

    /// Reclaim attributes list memory that isn't needed any longer.
    ///
    /// The buffer line is in an invalid state after this is called. See [`Self::reset_new`].
    pub(crate) fn reclaim_attrs(&mut self) -> AttrsList {
        mem::replace(&mut self.attrs_list, AttrsList::new(&Attrs::new()))
    }

    /// Reclaim text memory that isn't needed any longer.
    ///
    /// The buffer line is in an invalid state after this is called. See [`Self::reset_new`].
    pub(crate) fn reclaim_text(&mut self) -> String {
        let mut text = mem::take(&mut self.text);
        text.clear();
        text
    }
}
