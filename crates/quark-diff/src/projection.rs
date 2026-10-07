//! Display rows of a document: file headers, hunk headers or collapsed
//! gaps of unchanged lines, and line rows, unified or side by side.
//!
//! A [`Projection`] is a flat column table rebuilt in O(rows) whenever the
//! mode or the [`Expansion`] changes. Rows refer to lines by store index,
//! so an app reads the text from the document.

use crate::model::{BlockKind, DiffDocument, Side, lines_before};

/// No line on this side.
pub const NONE: u32 = u32::MAX;

/// Gaps that hide this many lines or fewer are shown instead.
pub const MIN_HIDDEN: u32 = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum Mode {
    #[default]
    Unified,
    Split,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RowKind {
    FileHeader,
    /// A hunk's `@@` line, for patches without the surrounding text.
    HunkHeader,
    /// Unchanged lines left out; [`Projection::gap`] says which.
    Gap,
    Context,
    Removed,
    Added,
    /// Side by side only: a removed line beside the added line it became.
    Modified,
}

impl RowKind {
    /// Whether the row shows lines of the file.
    pub const fn is_line(self) -> bool {
        matches!(
            self,
            Self::Context | Self::Removed | Self::Added | Self::Modified
        )
    }

    /// How assistive tech describes a line of this kind.
    pub const fn description(self) -> &'static str {
        match self {
            Self::FileHeader => "file",
            Self::HunkHeader => "hunk",
            Self::Gap => "collapsed lines",
            Self::Context => "unchanged",
            Self::Removed => "removed",
            Self::Added => "added",
            Self::Modified => "changed",
        }
    }
}

/// A run of unchanged lines between hunks: above hunk `hunk`, or after the
/// last hunk of `file` when `hunk` is `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GapId {
    pub file: u32,
    pub hunk: Option<u32>,
}

/// Which end of a gap to reveal lines from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Reveal {
    /// Lines at the top of the gap, below the previous hunk.
    Down,
    /// Lines at the bottom of the gap, above the next hunk.
    Up,
    All,
}

/// Lines revealed from each end of every gap.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Expansion {
    /// Per hunk: lines shown from the top and bottom of the gap above it.
    above: Vec<(u32, u32)>,
    /// Per file: lines shown from the top of the gap after its last hunk.
    after: Vec<u32>,
}

impl Expansion {
    pub fn new(doc: &DiffDocument) -> Self {
        Self {
            above: vec![(0, 0); doc.hunk_count() as usize],
            after: vec![0; doc.file_count() as usize],
        }
    }

    /// Lines `gap` holds in all.
    pub fn gap_len(doc: &DiffDocument, gap: GapId) -> u32 {
        match gap.hunk {
            Some(h) => doc.gap_above(h),
            None => doc.gap_after(gap.file),
        }
    }

    /// Lines of `gap` shown from its top and its bottom.
    pub fn revealed(&self, gap: GapId) -> (u32, u32) {
        match gap.hunk {
            Some(h) => self.above.get(h as usize).copied().unwrap_or((0, 0)),
            None => (self.after.get(gap.file as usize).copied().unwrap_or(0), 0),
        }
    }

    /// Lines of `gap` still hidden. A remainder of [`MIN_HIDDEN`] lines or
    /// fewer is shown, so it counts as none.
    pub fn hidden(&self, doc: &DiffDocument, gap: GapId) -> u32 {
        let (top, bottom) = self.revealed(gap);
        let hidden = Self::gap_len(doc, gap).saturating_sub(top + bottom);
        if hidden <= MIN_HIDDEN { 0 } else { hidden }
    }

    /// Reveals up to `amount` more lines of `gap`. A trailing gap only
    /// reveals downward. Returns whether anything changed.
    pub fn reveal(&mut self, doc: &DiffDocument, gap: GapId, reveal: Reveal, amount: u32) -> bool {
        let hidden = self.hidden(doc, gap);
        if hidden == 0 {
            return false;
        }
        let take = if reveal == Reveal::All || hidden - amount.min(hidden) <= MIN_HIDDEN {
            hidden
        } else {
            amount
        };
        match gap.hunk {
            Some(h) => {
                let slot = &mut self.above[h as usize];
                match reveal {
                    Reveal::Up => slot.1 += take,
                    Reveal::Down | Reveal::All => slot.0 += take,
                }
            }
            None => self.after[gap.file as usize] += take,
        }
        true
    }
}

/// One collapsed gap row: the gap and the store indices of its hidden
/// lines on each side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapRow {
    pub id: GapId,
    pub old_start: u32,
    pub new_start: u32,
    pub hidden: u32,
}

/// Display rows as columns. See the [module docs](self).
#[derive(Debug, Clone, Default)]
pub struct Projection {
    pub mode: Mode,
    pub kind: Vec<RowKind>,
    pub file: Vec<u32>,
    /// The hunk of hunk header and line rows that belong to one; [`NONE`]
    /// for expanded gap lines and file headers. For gap rows, the index
    /// into [`Self::gaps`].
    pub hunk: Vec<u32>,
    /// Store index of the row's old and new line, or [`NONE`].
    pub old: Vec<u32>,
    pub new: Vec<u32>,
    /// Unified change rows: the store index of the line on the other side
    /// at the same offset of the change, for word diffs; else [`NONE`].
    pub pair: Vec<u32>,
    pub gaps: Vec<GapRow>,
    /// First row of each file and of each hunk's lines.
    pub file_rows: Vec<u32>,
    pub hunk_rows: Vec<u32>,
    /// Rows with an old line and rows with a new line, in order: the
    /// inverse of `old` and `new` per file.
    old_rows: Vec<u32>,
    new_rows: Vec<u32>,
}

impl Projection {
    pub fn new(doc: &DiffDocument, mode: Mode, expansion: &Expansion) -> Self {
        let mut p = Self {
            mode,
            ..Self::default()
        };
        p.rebuild(doc, mode, expansion);
        p
    }

    pub fn len(&self) -> u32 {
        self.kind.len() as u32
    }

    pub fn is_empty(&self) -> bool {
        self.kind.is_empty()
    }

    /// The gap a [`RowKind::Gap`] row shows.
    pub fn gap(&self, row: u32) -> Option<GapRow> {
        let r = row as usize;
        (self.kind.get(r) == Some(&RowKind::Gap)).then(|| self.gaps[self.hunk[r] as usize])
    }

    /// Store index of the row's line on `side`.
    pub fn line(&self, row: u32, side: Side) -> Option<u32> {
        let column = match side {
            Side::Old => &self.old,
            Side::New => &self.new,
        };
        column.get(row as usize).copied().filter(|&i| i != NONE)
    }

    /// The row showing line `index` of `file`'s `side` store, if any.
    pub fn row_of(&self, file: u32, side: Side, index: u32) -> Option<u32> {
        let (rows, column) = match side {
            Side::Old => (&self.old_rows, &self.old),
            Side::New => (&self.new_rows, &self.new),
        };
        let at =
            rows.partition_point(|&r| (self.file[r as usize], column[r as usize]) < (file, index));
        rows.get(at)
            .copied()
            .filter(|&r| self.file[r as usize] == file && column[r as usize] == index)
    }

    /// A key unique among this projection's rows and stable across
    /// expansion changes, for row tables and caches.
    pub fn row_key(&self, row: u32) -> u64 {
        let r = row as usize;
        let tag = self.kind[r] as u64;
        let file = u64::from(self.file[r]) & 0x3F_FFFF;
        let index = match self.kind[r] {
            RowKind::FileHeader => 0,
            RowKind::HunkHeader => u64::from(self.hunk[r]),
            RowKind::Gap => {
                let gap = self.gaps[self.hunk[r] as usize];
                gap.id.hunk.map_or(u64::from(u32::MAX), u64::from)
            }
            RowKind::Removed | RowKind::Modified => u64::from(self.old[r]),
            RowKind::Context | RowKind::Added => u64::from(self.new[r]),
        };
        (tag << 60) | (file << 38) | (index & 0x3F_FFFF_FFFF)
    }

    /// Rebuilds the rows in place, reusing the columns' memory.
    pub fn rebuild(&mut self, doc: &DiffDocument, mode: Mode, expansion: &Expansion) {
        self.mode = mode;
        for column in [
            &mut self.file,
            &mut self.hunk,
            &mut self.old,
            &mut self.new,
            &mut self.pair,
            &mut self.file_rows,
            &mut self.hunk_rows,
            &mut self.old_rows,
            &mut self.new_rows,
        ] {
            column.clear();
        }
        self.kind.clear();
        self.gaps.clear();
        let (files, h, b) = (doc.files(), doc.hunks(), doc.blocks());
        for file in 0..doc.file_count() {
            let f = file as usize;
            self.file_rows.push(self.len());
            self.push(RowKind::FileHeader, file, NONE, NONE, NONE, NONE);
            let partial = files.partial[f];
            // Next unshown line of each store, for the gaps between hunks.
            let (mut old_next, mut new_next) = (0u32, 0u32);
            for hunk in files.hunks[f].clone() {
                let hi = hunk as usize;
                let old_before = lines_before(h.old_start[hi], h.old_len[hi]);
                let new_before = lines_before(h.new_start[hi], h.new_len[hi]);
                if partial {
                    self.push(RowKind::HunkHeader, file, hunk, NONE, NONE, NONE);
                } else {
                    let id = GapId {
                        file,
                        hunk: Some(hunk),
                    };
                    self.push_gap(id, (old_next, new_next), new_before - new_next, expansion);
                }
                self.hunk_rows.push(self.len());
                for block in h.blocks[hi].clone() {
                    let bi = block as usize;
                    let (os, ns) = (b.old_store[bi], b.new_store[bi]);
                    let (ol, nl) = (b.old_len[bi], b.new_len[bi]);
                    match (b.kind[bi], mode) {
                        (BlockKind::Context, _) => {
                            for k in 0..ol {
                                self.push(RowKind::Context, file, hunk, os + k, ns + k, NONE);
                            }
                        }
                        (BlockKind::Change, Mode::Unified) => {
                            for k in 0..ol {
                                let pair = if k < nl { ns + k } else { NONE };
                                self.push(RowKind::Removed, file, hunk, os + k, NONE, pair);
                            }
                            for k in 0..nl {
                                let pair = if k < ol { os + k } else { NONE };
                                self.push(RowKind::Added, file, hunk, NONE, ns + k, pair);
                            }
                        }
                        (BlockKind::Change, Mode::Split) => {
                            for k in 0..ol.max(nl) {
                                let (old, new) = (
                                    if k < ol { os + k } else { NONE },
                                    if k < nl { ns + k } else { NONE },
                                );
                                let kind = match (old != NONE, new != NONE) {
                                    (true, true) => RowKind::Modified,
                                    (true, false) => RowKind::Removed,
                                    _ => RowKind::Added,
                                };
                                self.push(kind, file, hunk, old, new, NONE);
                            }
                        }
                    }
                }
                old_next = old_before + h.old_len[hi];
                new_next = new_before + h.new_len[hi];
            }
            if !partial && !files.hunks[f].is_empty() {
                let id = GapId { file, hunk: None };
                let len = doc.gap_after(file);
                self.push_gap(id, (old_next, new_next), len, expansion);
            }
        }
        debug_assert_eq!(self.verify_integrity(doc), Ok(()));
    }

    fn push(&mut self, kind: RowKind, file: u32, hunk: u32, old: u32, new: u32, pair: u32) {
        let row = self.len();
        if old != NONE {
            self.old_rows.push(row);
        }
        if new != NONE {
            self.new_rows.push(row);
        }
        self.kind.push(kind);
        self.file.push(file);
        self.hunk.push(hunk);
        self.old.push(old);
        self.new.push(new);
        self.pair.push(pair);
    }

    /// The rows of a gap of `len` unchanged lines starting at the given
    /// store indices: revealed lines at the top, a gap row for the hidden
    /// middle (unless it is short enough to show), revealed lines below.
    fn push_gap(&mut self, id: GapId, (old, new): (u32, u32), len: u32, expansion: &Expansion) {
        let (top, bottom) = expansion.revealed(id);
        let (mut top, bottom) = (top.min(len), bottom.min(len - top.min(len)));
        let mut hidden = len - top - bottom;
        if hidden <= MIN_HIDDEN {
            top += hidden;
            hidden = 0;
        }
        for k in 0..top {
            self.push(RowKind::Context, id.file, NONE, old + k, new + k, NONE);
        }
        if hidden > 0 {
            self.gaps.push(GapRow {
                id,
                old_start: old + top,
                new_start: new + top,
                hidden,
            });
            self.push(
                RowKind::Gap,
                id.file,
                self.gaps.len() as u32 - 1,
                NONE,
                NONE,
                NONE,
            );
        }
        for k in len - bottom..len {
            self.push(RowKind::Context, id.file, NONE, old + k, new + k, NONE);
        }
    }

    /// Checks the columns against each other and `doc`: equal lengths,
    /// line indices inside the stores and increasing per file and side,
    /// and the inverse row lists. O(rows).
    pub fn verify_integrity(&self, doc: &DiffDocument) -> Result<(), ProjectionError> {
        let n = self.kind.len();
        if [
            self.file.len(),
            self.hunk.len(),
            self.old.len(),
            self.new.len(),
            self.pair.len(),
        ]
        .iter()
        .any(|&len| len != n)
            || self.file_rows.len() != doc.file_count() as usize
        {
            return Err(ProjectionError::ColumnLength);
        }
        let mut last: [(u32, Option<u32>); 2] = [(0, None), (0, None)];
        let (mut old_rows, mut new_rows) = (0, 0);
        for row in 0..n {
            let file = self.file[row];
            for (side, column, rows, count) in [
                (0, &self.old, &self.old_rows, &mut old_rows),
                (1, &self.new, &self.new_rows, &mut new_rows),
            ] {
                let index = column[row];
                if index == NONE {
                    continue;
                }
                let store = doc.text(file, if side == 0 { Side::Old } else { Side::New });
                let increasing = match last[side] {
                    (f, Some(prev)) if f == file => index > prev,
                    _ => true,
                };
                if index >= store.line_count() || !increasing {
                    return Err(ProjectionError::Line { row: row as u32 });
                }
                if rows.get(*count) != Some(&(row as u32)) {
                    return Err(ProjectionError::RowList { row: row as u32 });
                }
                *count += 1;
                last[side] = (file, Some(index));
            }
            let lines = (self.old[row] != NONE, self.new[row] != NONE);
            let shape = match self.kind[row] {
                RowKind::Context | RowKind::Modified => lines == (true, true),
                RowKind::Removed => lines == (true, false),
                RowKind::Added => lines == (false, true),
                RowKind::Gap => self.hunk[row] < self.gaps.len() as u32,
                RowKind::FileHeader | RowKind::HunkHeader => lines == (false, false),
            };
            if !shape {
                return Err(ProjectionError::Line { row: row as u32 });
            }
        }
        if old_rows != self.old_rows.len() || new_rows != self.new_rows.len() {
            return Err(ProjectionError::RowList { row: n as u32 });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectionError {
    ColumnLength,
    Line { row: u32 },
    RowList { row: u32 },
}
