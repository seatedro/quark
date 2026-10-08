//! A plain Rust copy of the terminal's viewport: rows of styled runs, the
//! cursor, and the colors, refreshed from libghostty-vt's render state by
//! [`crate::vt::Terminal::snapshot`]. The view reads only this, so painting
//! never calls into C, and rows keep their buffers between refreshes.

use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};

/// An sRGB color.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

impl std::fmt::Display for Rgb {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{:02x}{:02x}{:02x}", self.r, self.g, self.b)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Underline {
    #[default]
    None,
    Single,
    Double,
    Curly,
    Dotted,
    Dashed,
}

/// How a run of cells is drawn, with SGR inverse and invisible already
/// applied: `fg` is the color the glyphs take and `bg` is `None` where the
/// terminal background shows through.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct CellStyle {
    pub fg: Rgb,
    pub bg: Option<Rgb>,
    pub underline_color: Option<Rgb>,
    pub underline: Underline,
    pub bold: bool,
    pub italic: bool,
    pub faint: bool,
    pub blink: bool,
    pub strikethrough: bool,
    pub overline: bool,
    /// Part of an OSC 8 hyperlink.
    pub hyperlink: bool,
}

/// Cells of one row that draw the same way: one style, starting at `col`
/// and `cols` wide. Plain narrow text merges into long runs; every
/// non-ASCII or wide grapheme gets a run of its own, so a fallback font's
/// advance can never push later cells off the grid.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Run {
    pub col: u16,
    pub cols: u16,
    /// Bytes of [`GridRow::text`].
    pub text: Range<u32>,
    pub style: CellStyle,
}

/// One viewport row.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GridRow {
    /// The row's text: one grapheme per cell (a space for an empty cell,
    /// nothing for the second half of a wide character), trailing blanks
    /// trimmed.
    pub text: String,
    pub runs: Vec<Run>,
    /// Selected columns, inclusive.
    pub selection: Option<(u16, u16)>,
    /// The row continues on the next one (a soft wrap).
    pub wrapped: bool,
    /// Identifies the row's content, runs, and selection: equal hashes draw
    /// the same pixels.
    pub hash: u64,
    /// Identifies this row's storage for as long as it lives, unique in the
    /// process. A row that scrolls to another line keeps it when the
    /// terminal can tell the content is the same, so per-row state a view
    /// keys by it (cached drawing, text layouts) follows the content; a row
    /// read anew reuses some row's storage and id.
    pub id: u64,
    /// The raw cells the row was read from, to recognize it after it
    /// moves.
    pub(crate) cells: Vec<u64>,
    /// Some cell's text or style came from the terminal's tables (a style,
    /// a color, a grapheme cluster) rather than from its raw value alone.
    pub(crate) looked_up: bool,
}

impl GridRow {
    /// An empty row with a new [`Self::id`].
    pub(crate) fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            ..Self::default()
        }
    }

    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.runs.clear();
        self.selection = None;
        self.wrapped = false;
        self.looked_up = false;
    }

    pub(crate) fn rehash(&mut self) {
        let mut h = std::hash::DefaultHasher::new();
        self.text.hash(&mut h);
        h.write_usize(self.runs.len());
        // Each run packed into three words: a few large writes hash several
        // times faster than the derived field-by-field ones.
        let rgb = |c: Rgb| u64::from(c.r) << 16 | u64::from(c.g) << 8 | u64::from(c.b);
        let some_rgb = |c: Option<Rgb>| c.map_or(0, |c| 1 << 24 | rgb(c));
        for run in &self.runs {
            let s = &run.style;
            h.write_u64(
                u64::from(run.col) << 48 | u64::from(run.cols) << 32 | u64::from(run.text.end),
            );
            let flags = [
                s.bold,
                s.italic,
                s.faint,
                s.blink,
                s.strikethrough,
                s.overline,
                s.hyperlink,
            ]
            .iter()
            .fold(0, |bits, &on| bits << 1 | u64::from(on));
            h.write_u64(rgb(s.fg) << 40 | some_rgb(s.bg) << 15 | (s.underline as u64) << 7 | flags);
            h.write_u64(some_rgb(s.underline_color) << 32 | u64::from(run.text.start));
        }
        self.selection.hash(&mut h);
        self.wrapped.hash(&mut h);
        self.hash = h.finish();
    }

    /// Checks that runs tile the row from column 0 without gaps, cover the
    /// text in order, and that a run is either one byte per cell (ASCII) or
    /// a single grapheme of at most two cells.
    pub fn verify_integrity(&self) -> Result<(), String> {
        let mut col = 0u16;
        let mut byte = 0u32;
        for (i, run) in self.runs.iter().enumerate() {
            if run.col != col || run.cols == 0 {
                return Err(format!(
                    "run {i} at column {} does not follow {col}",
                    run.col
                ));
            }
            if run.text.start != byte || run.text.end as usize > self.text.len() {
                return Err(format!("run {i} bytes {:?} do not follow {byte}", run.text));
            }
            let len = run.text.len() as u16;
            if len != run.cols && run.cols > 2 {
                return Err(format!("run {i} has {len} bytes over {} cells", run.cols));
            }
            col = run.col + run.cols;
            byte = run.text.end;
        }
        if byte as usize != self.text.len() {
            return Err(format!("runs end at byte {byte} of {}", self.text.len()));
        }
        Ok(())
    }

    /// The run covering column `col`, if any.
    pub fn run_at(&self, col: u16) -> Option<&Run> {
        self.runs
            .iter()
            .find(|r| (r.col..r.col + r.cols).contains(&col))
    }

    /// Text of a run.
    pub fn run_text(&self, run: &Run) -> &str {
        &self.text[run.text.start as usize..run.text.end as usize]
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum CursorShape {
    #[default]
    Block,
    BlockHollow,
    Bar,
    Underline,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct Cursor {
    /// Column and row in the viewport, when the cursor is on screen and
    /// visible (DECTCEM).
    pub at: Option<(u16, u16)>,
    /// Column and row in the viewport when on screen, shown or hidden:
    /// where IME composition goes.
    pub cell: Option<(u16, u16)>,
    /// The cell under the cursor is a wide character.
    pub wide: bool,
    pub shape: CursorShape,
    pub blinking: bool,
    /// Set by OSC 12; the view picks a color otherwise.
    pub color: Option<Rgb>,
}

/// The default colors and palette the terminal resolved cells against.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Colors {
    pub foreground: Rgb,
    pub background: Rgb,
}

impl Default for Colors {
    fn default() -> Self {
        Self {
            foreground: Rgb::new(0xdd, 0xdd, 0xdd),
            background: Rgb::new(0x1e, 0x1e, 0x1e),
        }
    }
}

/// The viewport as of the last snapshot.
#[derive(Debug, Clone, Default)]
pub struct Grid {
    pub cols: u16,
    pub rows: Vec<GridRow>,
    pub cursor: Cursor,
    pub colors: Colors,
}

impl Grid {
    /// Visible text, rows joined by `\n`, trailing blank rows dropped.
    pub fn text(&self) -> String {
        let mut out = String::new();
        self.write_text(&mut out);
        out
    }

    /// Replaces `out` with [`Self::text`], reusing its buffer.
    pub fn write_text(&self, out: &mut String) {
        out.clear();
        let last = self
            .rows
            .iter()
            .rposition(|r| !r.text.is_empty())
            .map_or(0, |i| i + 1);
        for (i, row) in self.rows[..last].iter().enumerate() {
            if i > 0 {
                out.push('\n');
            }
            out.push_str(&row.text);
        }
    }

    /// Byte offset of viewport cell `(col, row)` in [`Self::text`]: the
    /// screen reader caret.
    pub fn text_offset(&self, col: u16, row: u16) -> usize {
        let row = (row as usize).min(self.rows.len());
        let before: usize = self.rows[..row].iter().map(|r| r.text.len() + 1).sum();
        let Some(line) = self.rows.get(row) else {
            return before;
        };
        // Runs cover every cell up to the trimmed end; a multi-cell run is
        // ASCII (one byte per cell) unless it is a single wide grapheme.
        let byte = match line.run_at(col) {
            Some(run) if run.text.len() as u16 == run.cols => {
                run.text.start + u32::from(col - run.col)
            }
            Some(run) => run.text.start,
            None => line.text.len() as u32,
        };
        before + byte as usize
    }
}
