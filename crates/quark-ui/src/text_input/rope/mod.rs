//! [`Rope`]: the editor's text as a B-tree of chunks, so an edit, a line
//! lookup, or a snapshot costs O(log n) however long the text is.
//!
//! Leaves hold chunks of at most 1 KiB, cut on char boundaries; internal
//! nodes keep their subtree's byte, newline, and UTF-16 counts. Nodes are
//! immutable and shared through `Arc`: cloning a rope is one reference
//! count, and an edit copies only the nodes on its path that a clone still
//! shares. There is no `unsafe` here.
//!
//! Lines are split at `'\n'` only: line `i` runs from just after the
//! `i`th `'\n'` through the next one, and a text of `n` newlines has
//! `n + 1` lines. Grapheme, word, and line navigation works on the chunks
//! or the one line it needs (see `nav`), never on the whole text.

mod nav;
mod node;
#[cfg(test)]
mod tests;

use std::borrow::Cow;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

use unicode_segmentation::{GraphemeCursor, GraphemeIncomplete};

pub use node::TextSummary;
use node::{MAX_CHILDREN, MAX_LEAF, MIN_CHILDREN, MIN_LEAF, Node, child_at};

/// Text stored as a B-tree of chunks. See the module docs.
#[derive(Clone)]
pub struct Rope {
    root: Arc<Node>,
}

/// A broken rope, found by [`Rope::verify_integrity`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RopeIntegrityError {
    /// A leaf at `depth` where another was at `expected`.
    UnevenDepth { depth: usize, expected: usize },
    /// A leaf of `len` bytes outside its bounds.
    LeafSize { len: usize },
    /// An internal node with `count` children outside its bounds.
    ChildCount { count: usize },
    /// A node whose summary is not what its contents add up to.
    Summary {
        stored: TextSummary,
        actual: TextSummary,
    },
    /// An internal node whose height is not one more than its children's.
    Height { stored: usize, actual: usize },
}

impl fmt::Display for RopeIntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnevenDepth { depth, expected } => {
                write!(f, "leaf at depth {depth}, others at {expected}")
            }
            Self::LeafSize { len } => write!(f, "leaf of {len} bytes"),
            Self::ChildCount { count } => write!(f, "internal node of {count} children"),
            Self::Summary { stored, actual } => {
                write!(f, "summary {stored:?}, contents add up to {actual:?}")
            }
            Self::Height { stored, actual } => write!(f, "height {stored}, should be {actual}"),
        }
    }
}

impl std::error::Error for RopeIntegrityError {}

impl Default for Rope {
    fn default() -> Self {
        Self::new()
    }
}

impl Rope {
    pub fn new() -> Self {
        Self {
            root: Node::empty(),
        }
    }

    /// Length in bytes.
    pub fn len(&self) -> usize {
        self.root.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Byte, newline, and UTF-16 totals of the whole text. O(1).
    pub fn summary(&self) -> TextSummary {
        self.root.summary()
    }

    /// Lines, split at `'\n'`: one more than the newlines.
    pub fn line_count(&self) -> usize {
        self.summary().newlines + 1
    }

    /// The text when it is one chunk (at most 1 KiB), borrowed.
    pub fn as_single_chunk(&self) -> Option<&str> {
        match &*self.root {
            Node::Leaf { text, .. } => Some(text),
            Node::Internal { .. } => None,
        }
    }

    /// Whether `byte` is a char boundary (the end counts).
    pub fn is_char_boundary(&self, byte: usize) -> bool {
        if byte > self.len() {
            return false;
        }
        let (chunk, start) = self.chunk_at_byte(byte);
        chunk.is_char_boundary(byte - start)
    }

    /// The char boundary at or before `byte`, clamped to the text.
    pub fn floor_char_boundary(&self, byte: usize) -> usize {
        let byte = byte.min(self.len());
        let (chunk, start) = self.chunk_at_byte(byte);
        start + node::floor_char_boundary(chunk, byte - start)
    }

    /// The char boundary at or after `byte`, clamped to the text.
    pub fn ceil_char_boundary(&self, byte: usize) -> usize {
        let byte = byte.min(self.len());
        let (chunk, start) = self.chunk_at_byte(byte);
        start + node::ceil_char_boundary(chunk, byte - start)
    }

    /// The chunk holding `byte` and the byte it starts at. A byte on a
    /// boundary between chunks is in the later one; the end of the text is
    /// in the last chunk.
    pub fn chunk_at_byte(&self, byte: usize) -> (&str, usize) {
        let mut node = &*self.root;
        let mut start = 0;
        loop {
            match node {
                Node::Leaf { text, .. } => return (text, start),
                Node::Internal { children, .. } => {
                    let (i, child_start) = child_at(children, byte - start);
                    match children.get(i) {
                        Some(child) => {
                            node = child;
                            start += child_start;
                        }
                        None => return ("", start),
                    }
                }
            }
        }
    }

    /// The chunks covering `range` (clamped to char boundaries), each cut
    /// to it. Each step descends from the root, so this allocates nothing.
    pub fn chunks_in_range(&self, range: Range<usize>) -> impl Iterator<Item = &str> + '_ {
        let end = self.floor_char_boundary(range.end);
        let mut at = self.floor_char_boundary(range.start).min(end);
        std::iter::from_fn(move || {
            if at >= end {
                return None;
            }
            let (chunk, start) = self.chunk_at_byte(at);
            let piece_end = (start + chunk.len()).min(end);
            let piece = chunk.get(at - start..piece_end - start)?;
            at = piece_end;
            Some(piece)
        })
    }

    /// Every chunk, in order.
    pub fn chunks(&self) -> impl Iterator<Item = &str> + '_ {
        self.chunks_in_range(0..self.len())
    }

    /// `range` (clamped to char boundaries) as text, borrowed when it lies
    /// in one chunk.
    pub fn slice(&self, range: Range<usize>) -> Cow<'_, str> {
        let mut chunks = self.chunks_in_range(range.clone());
        let Some(first) = chunks.next() else {
            return Cow::Borrowed("");
        };
        let Some(second) = chunks.next() else {
            return Cow::Borrowed(first);
        };
        let mut out = String::with_capacity(range.len());
        out.push_str(first);
        out.push_str(second);
        chunks.for_each(|chunk| out.push_str(chunk));
        Cow::Owned(out)
    }

    /// `range` (clamped to char boundaries) as an owned string.
    pub fn slice_to_string(&self, range: Range<usize>) -> String {
        self.slice(range).into_owned()
    }

    /// Appends `range` (clamped to char boundaries) to `out`.
    pub fn write_range(&self, range: Range<usize>, out: &mut String) {
        self.chunks_in_range(range)
            .for_each(|chunk| out.push_str(chunk));
    }

    /// Whether the text at `at` is exactly `expected`.
    pub fn slice_eq(&self, at: usize, expected: &str) -> bool {
        let end = at.saturating_add(expected.len());
        if end > self.len() || !self.is_char_boundary(at) || !self.is_char_boundary(end) {
            return false;
        }
        let mut rest = expected;
        for chunk in self.chunks_in_range(at..end) {
            match rest.strip_prefix(chunk) {
                Some(after) => rest = after,
                None => return false,
            }
        }
        rest.is_empty()
    }

    /// The char starting at `byte`, if it is a char boundary before the end.
    pub fn char_at(&self, byte: usize) -> Option<char> {
        if byte >= self.len() {
            return None;
        }
        let (chunk, start) = self.chunk_at_byte(byte);
        chunk.get(byte - start..)?.chars().next()
    }

    /// The line holding `byte`: how many `'\n'` come before it.
    pub fn byte_to_line(&self, byte: usize) -> usize {
        let byte = byte.min(self.len());
        let mut node = &*self.root;
        let (mut start, mut line) = (0, 0);
        loop {
            match node {
                Node::Leaf { text, .. } => {
                    let head = text.as_bytes().get(..byte - start).unwrap_or_default();
                    return line + head.iter().filter(|&&b| b == b'\n').count();
                }
                Node::Internal { children, .. } => {
                    let (i, child_start) = child_at(children, byte - start);
                    for child in children.iter().take(i) {
                        line += child.summary().newlines;
                    }
                    let Some(child) = children.get(i) else {
                        return line;
                    };
                    start += child_start;
                    node = child;
                }
            }
        }
    }

    /// Where line `line` starts; past the last line, the end of the text.
    pub fn line_to_byte(&self, line: usize) -> usize {
        if line == 0 {
            return 0;
        }
        if line > self.summary().newlines {
            return self.len();
        }
        // The byte after the `line`th newline.
        let mut node = &*self.root;
        let (mut start, mut want) = (0, line);
        loop {
            match node {
                Node::Leaf { text, .. } => {
                    let at = text
                        .bytes()
                        .enumerate()
                        .filter(|&(_, b)| b == b'\n')
                        .nth(want - 1)
                        .map_or(text.len(), |(i, _)| i + 1);
                    return start + at;
                }
                Node::Internal { children, .. } => {
                    let mut next = None;
                    for child in children {
                        let newlines = child.summary().newlines;
                        if want <= newlines {
                            next = Some(child);
                            break;
                        }
                        want -= newlines;
                        start += child.len();
                    }
                    match next {
                        Some(child) => node = child,
                        None => return start,
                    }
                }
            }
        }
    }

    /// Line `line` including its `'\n'`.
    pub fn line_byte_range(&self, line: usize) -> Range<usize> {
        self.line_to_byte(line)..self.line_to_byte(line + 1)
    }

    /// Line `line` without its line ending (`"\n"` or `"\r\n"`).
    pub fn line_content_range(&self, line: usize) -> Range<usize> {
        let Range { start, mut end } = self.line_byte_range(line);
        if end > start && self.char_at(end - 1) == Some('\n') {
            end -= 1;
            if end > start && self.char_at(end - 1) == Some('\r') {
                end -= 1;
            }
        }
        start..end
    }

    /// UTF-16 code units before `byte` (snapped down to a char boundary).
    pub fn byte_to_utf16(&self, byte: usize) -> usize {
        let byte = self.floor_char_boundary(byte);
        let mut node = &*self.root;
        let (mut start, mut units) = (0, 0);
        loop {
            match node {
                Node::Leaf { text, .. } => {
                    let head = text.get(..byte - start).unwrap_or_default();
                    return units + head.chars().map(char::len_utf16).sum::<usize>();
                }
                Node::Internal { children, .. } => {
                    let (i, child_start) = child_at(children, byte - start);
                    for child in children.iter().take(i) {
                        units += child.summary().utf16;
                    }
                    let Some(child) = children.get(i) else {
                        return units;
                    };
                    start += child_start;
                    node = child;
                }
            }
        }
    }

    /// The byte at UTF-16 offset `units`, or of the char holding it when
    /// it falls inside a surrogate pair; the end past the last unit.
    pub fn utf16_to_byte(&self, units: usize) -> usize {
        if units >= self.summary().utf16 {
            return self.len();
        }
        let mut node = &*self.root;
        let (mut start, mut want) = (0, units);
        loop {
            match node {
                Node::Leaf { text, .. } => {
                    let mut seen = 0;
                    for (i, ch) in text.char_indices() {
                        seen += ch.len_utf16();
                        if seen > want {
                            return start + i;
                        }
                    }
                    return start + text.len();
                }
                Node::Internal { children, .. } => {
                    let mut next = None;
                    for child in children {
                        let child_units = child.summary().utf16;
                        if want < child_units {
                            next = Some(child);
                            break;
                        }
                        want -= child_units;
                        start += child.len();
                    }
                    match next {
                        Some(child) => node = child,
                        None => return start,
                    }
                }
            }
        }
    }

    /// Replace `range` with `text`. The ends snap down to char boundaries,
    /// and a range past the end is clamped. O(log n + text length).
    pub fn replace(&mut self, range: Range<usize>, text: &str) {
        let end = self.floor_char_boundary(range.end);
        let start = self.floor_char_boundary(range.start).min(end);
        if start == end && text.is_empty() {
            return;
        }
        if node::fits_in_leaf(&self.root, start..end, text.len(), true) {
            node::edit_in_leaf(&mut self.root, start..end, text);
        } else {
            let (head, rest) = node::split(&self.root, start);
            let (_, tail) = node::split(&rest, end - start);
            let middle = node::build(text);
            self.root = node::normalize(node::concat(node::concat(head, middle), tail));
        }
        self.debug_check();
    }

    pub fn insert(&mut self, at: usize, text: &str) {
        self.replace(at..at, text);
    }

    pub fn remove(&mut self, range: Range<usize>) {
        self.replace(range, "");
    }

    /// Append `text` at the end.
    pub fn push_str(&mut self, text: &str) {
        self.replace(self.len()..self.len(), text);
    }

    /// The grapheme boundary at or before `byte`, clamped to the text.
    pub fn floor_grapheme(&self, byte: usize) -> usize {
        let byte = self.floor_char_boundary(byte);
        if self.is_grapheme_boundary(byte) {
            byte
        } else {
            self.prev_grapheme_boundary(byte)
        }
    }

    /// The grapheme boundary at or after `byte`, clamped to the text.
    pub fn ceil_grapheme(&self, byte: usize) -> usize {
        let byte = self.ceil_char_boundary(byte);
        if self.is_grapheme_boundary(byte) {
            byte
        } else {
            self.next_grapheme_boundary(byte)
        }
    }

    /// Whether `byte` is an extended grapheme cluster boundary.
    pub fn is_grapheme_boundary(&self, byte: usize) -> bool {
        if !self.is_char_boundary(byte) {
            return false;
        }
        let mut cursor = GraphemeCursor::new(byte, self.len(), true);
        let (mut chunk, mut start) = self.chunk_at_byte(byte);
        loop {
            match cursor.is_boundary(chunk, start) {
                Ok(boundary) => return boundary,
                Err(GraphemeIncomplete::PreContext(at)) => {
                    let (context, context_start) = self.chunk_at_byte(at.saturating_sub(1));
                    cursor.provide_context(context, context_start);
                }
                Err(GraphemeIncomplete::NextChunk) if start + chunk.len() < self.len() => {
                    (chunk, start) = self.chunk_at_byte(start + chunk.len());
                }
                Err(_) => return true,
            }
        }
    }

    /// The grapheme boundary before `byte` (a char boundary), or 0.
    pub fn prev_grapheme_boundary(&self, byte: usize) -> usize {
        let byte = self.floor_char_boundary(byte);
        let mut cursor = GraphemeCursor::new(byte, self.len(), true);
        let (mut chunk, mut start) = self.chunk_at_byte(byte);
        loop {
            match cursor.prev_boundary(chunk, start) {
                Ok(at) => return at.unwrap_or(0),
                Err(GraphemeIncomplete::PrevChunk) if start > 0 => {
                    (chunk, start) = self.chunk_at_byte(start - 1);
                }
                Err(GraphemeIncomplete::PreContext(at)) => {
                    let (context, context_start) = self.chunk_at_byte(at.saturating_sub(1));
                    cursor.provide_context(context, context_start);
                }
                Err(_) => return 0,
            }
        }
    }

    /// The grapheme boundary after `byte` (a char boundary), or the end.
    pub fn next_grapheme_boundary(&self, byte: usize) -> usize {
        let byte = self.floor_char_boundary(byte);
        let mut cursor = GraphemeCursor::new(byte, self.len(), true);
        let (mut chunk, mut start) = self.chunk_at_byte(byte);
        loop {
            match cursor.next_boundary(chunk, start) {
                Ok(at) => return at.unwrap_or(self.len()),
                Err(GraphemeIncomplete::NextChunk) if start + chunk.len() < self.len() => {
                    (chunk, start) = self.chunk_at_byte(start + chunk.len());
                }
                Err(GraphemeIncomplete::PreContext(at)) => {
                    let (context, context_start) = self.chunk_at_byte(at.saturating_sub(1));
                    cursor.provide_context(context, context_start);
                }
                Err(_) => return self.len(),
            }
        }
    }

    /// Checks the B-tree invariants: even depth, leaf and fan-out bounds,
    /// UTF-8 leaves (by construction: they are `String`s), and summaries
    /// that add up exactly.
    pub fn verify_integrity(&self) -> Result<(), RopeIntegrityError> {
        let mut leaf_depth = None;
        verify(&self.root, 0, true, &mut leaf_depth)
    }

    /// After a mutation: the full check with `integrity-checks`, else only
    /// the root's own bounds. Compiled out of release builds.
    fn debug_check(&self) {
        if cfg!(all(debug_assertions, feature = "integrity-checks")) {
            debug_assert_eq!(self.verify_integrity(), Ok(()));
        } else if cfg!(debug_assertions) {
            debug_assert!(match &*self.root {
                Node::Leaf { text, .. } => text.len() <= MAX_LEAF,
                Node::Internal { children, .. } => (2..=MAX_CHILDREN).contains(&children.len()),
            });
        }
    }
}

fn verify(
    node: &Node,
    depth: usize,
    is_root: bool,
    leaf_depth: &mut Option<usize>,
) -> Result<(), RopeIntegrityError> {
    match node {
        Node::Leaf { text, summary } => {
            let expected = *leaf_depth.get_or_insert(depth);
            if expected != depth {
                return Err(RopeIntegrityError::UnevenDepth { depth, expected });
            }
            let len = text.len();
            if len > MAX_LEAF || (!is_root && len < MIN_LEAF) {
                return Err(RopeIntegrityError::LeafSize { len });
            }
            let actual = TextSummary::of(text);
            if actual != *summary {
                return Err(RopeIntegrityError::Summary {
                    stored: *summary,
                    actual,
                });
            }
        }
        Node::Internal {
            children,
            summary,
            height,
        } => {
            let count = children.len();
            let min = if is_root { 2 } else { MIN_CHILDREN };
            if !(min..=MAX_CHILDREN).contains(&count) {
                return Err(RopeIntegrityError::ChildCount { count });
            }
            let mut actual = TextSummary::default();
            for child in children {
                if child.height() + 1 != *height {
                    return Err(RopeIntegrityError::Height {
                        stored: *height,
                        actual: child.height() + 1,
                    });
                }
                verify(child, depth + 1, false, leaf_depth)?;
                actual += child.summary();
            }
            if actual != *summary {
                return Err(RopeIntegrityError::Summary {
                    stored: *summary,
                    actual,
                });
            }
        }
    }
    Ok(())
}

impl From<&str> for Rope {
    fn from(text: &str) -> Self {
        let rope = Self {
            root: node::build(text),
        };
        rope.debug_check();
        rope
    }
}

impl From<String> for Rope {
    fn from(text: String) -> Self {
        if text.len() <= MAX_LEAF {
            return Self {
                root: Node::leaf(text),
            };
        }
        Self::from(text.as_str())
    }
}

impl fmt::Display for Rope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.chunks().try_for_each(|chunk| f.write_str(chunk))
    }
}

impl fmt::Debug for Rope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rope")
            .field("len", &self.len())
            .field("lines", &self.line_count())
            .finish()
    }
}

impl PartialEq for Rope {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.root, &other.root) {
            return true;
        }
        self.summary() == other.summary() && chunks_eq(self.chunks(), other.chunks())
    }
}

impl Eq for Rope {}

impl PartialEq<str> for Rope {
    fn eq(&self, other: &str) -> bool {
        self.len() == other.len() && self.slice_eq(0, other)
    }
}

impl PartialEq<&str> for Rope {
    fn eq(&self, other: &&str) -> bool {
        *self == **other
    }
}

/// Whether two chunk sequences spell the same bytes, however they are cut.
fn chunks_eq<'a>(a: impl Iterator<Item = &'a str>, b: impl Iterator<Item = &'a str>) -> bool {
    let (mut a, mut b) = (a.map(str::as_bytes), b.map(str::as_bytes));
    let (mut x, mut y): (&[u8], &[u8]) = (&[], &[]);
    loop {
        if x.is_empty() {
            x = a.next().unwrap_or_default();
        }
        if y.is_empty() {
            y = b.next().unwrap_or_default();
        }
        if x.is_empty() || y.is_empty() {
            return x.is_empty() && y.is_empty();
        }
        let n = x.len().min(y.len());
        let ((xa, xb), (ya, yb)) = (x.split_at(n), y.split_at(n));
        if xa != ya {
            return false;
        }
        (x, y) = (xb, yb);
    }
}

impl quark_text::SegmentedText for Rope {
    fn byte_len(&self) -> usize {
        self.len()
    }

    fn floor_grapheme(&self, byte: usize) -> usize {
        Rope::floor_grapheme(self, byte)
    }
}
