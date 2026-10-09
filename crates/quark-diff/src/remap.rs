//! Source line correspondence between two revisions of a file, for moving
//! selections, expanded context, and scroll anchors onto a new revision.
//!
//! A line corresponds only where a line diff of the two revisions' whole
//! sources found it unchanged; equal paths, row numbers, or byte offsets
//! prove nothing.

use std::fmt;
use std::ops::Range;

use crate::compute::line_changes;
use crate::model::Side;
use crate::session::{FileDiffSnapshot, FileId, Revision};
use crate::source::SourceCoverage;
use crate::text::TextStore;

/// Where a source line of the earlier revision went.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineMap {
    /// Unchanged, now at this line.
    Kept(u32),
    /// Changed or deleted. `at` is the later revision's line where the
    /// replacement starts: the nearest surviving boundary, equal to the
    /// line count when the replacement runs to the end.
    Replaced { at: u32 },
}

/// One side's unchanged runs: `(from_start, to_start, len)`, sorted and
/// disjoint on both revisions.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct SideMap {
    from_lines: u32,
    to_lines: u32,
    runs: Vec<(u32, u32, u32)>,
}

impl SideMap {
    fn new(from: &TextStore, to: &TextStore) -> Self {
        let mut runs = Vec::new();
        let (mut old_at, mut new_at) = (0, 0);
        for change in line_changes(from, to) {
            if change.old.start > old_at {
                runs.push((old_at, new_at, change.old.start - old_at));
            }
            old_at = change.old.end;
            new_at = change.new.end;
        }
        if from.line_count() > old_at {
            runs.push((old_at, new_at, from.line_count() - old_at));
        }
        Self {
            from_lines: from.line_count(),
            to_lines: to.line_count(),
            runs,
        }
    }

    fn map(&self, line: u32) -> LineMap {
        let at = self.runs.partition_point(|r| r.0 <= line);
        let Some(&(from, to, len)) = at.checked_sub(1).map(|i| &self.runs[i]) else {
            return LineMap::Replaced { at: 0 };
        };
        if line < from + len {
            LineMap::Kept(to + line - from)
        } else {
            LineMap::Replaced {
                at: (to + len).min(self.to_lines),
            }
        }
    }

    fn verify(&self) -> bool {
        let mut ends = (0, 0);
        self.runs.iter().all(|&(from, to, len)| {
            let ok = len > 0
                && from >= ends.0
                && to >= ends.1
                && from + len <= self.from_lines
                && to + len <= self.to_lines;
            ends = (from + len, to + len);
            ok
        })
    }
}

/// Line correspondence for both sides of one file between two revisions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceRemap {
    pub file: FileId,
    pub from: Revision,
    pub to: Revision,
    sides: [SideMap; 2],
}

/// Why [`SourceRemap::between`] could not map two snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemapError {
    DifferentFiles,
    /// `to` is not a later revision than `from`.
    NotNewer,
    /// A snapshot lacks whole sources, so unchanged lines cannot be told
    /// from lines the patch left out.
    PatchOnly,
}

impl fmt::Display for RemapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "source remap: {self:?}")
    }
}

impl std::error::Error for RemapError {}

impl SourceRemap {
    /// Maps each side of `from` to the same side of `to` by a line diff of
    /// their whole sources. O(lines) plus the diff; run it off the UI
    /// thread for large files.
    pub fn between(from: &FileDiffSnapshot, to: &FileDiffSnapshot) -> Result<Self, RemapError> {
        if from.id != to.id {
            return Err(RemapError::DifferentFiles);
        }
        if to.revision <= from.revision {
            return Err(RemapError::NotNewer);
        }
        if from.coverage() != SourceCoverage::Full || to.coverage() != SourceCoverage::Full {
            return Err(RemapError::PatchOnly);
        }
        let side = |side| SideMap::new(from.diff.text(0, side), to.diff.text(0, side));
        let remap = Self {
            file: from.id,
            from: from.revision,
            to: to.revision,
            sides: [side(Side::Old), side(Side::New)],
        };
        debug_assert!(remap.sides.iter().all(SideMap::verify));
        Ok(remap)
    }

    /// Where line `line` of `side` in the earlier revision is now.
    pub fn map_line(&self, side: Side, line: u32) -> LineMap {
        self.sides[side as usize].map(line)
    }

    /// The later revision's lines for `lines`, when every one of them is
    /// unchanged and they stay contiguous; `None` when any was replaced,
    /// so a selection over them should be cleared.
    pub fn map_range(&self, side: Side, lines: Range<u32>) -> Option<Range<u32>> {
        if lines.is_empty() {
            return match self.map_line(side, lines.start) {
                LineMap::Kept(at) | LineMap::Replaced { at } => Some(at..at),
            };
        }
        let (LineMap::Kept(start), LineMap::Kept(last)) = (
            self.map_line(side, lines.start),
            self.map_line(side, lines.end - 1),
        ) else {
            return None;
        };
        (last - start == lines.end - 1 - lines.start).then_some(start..last + 1)
    }

    /// Whether this remap leads from `current` to `next`.
    pub(crate) fn fits(&self, current: &FileDiffSnapshot, next: &FileDiffSnapshot) -> bool {
        let counts = |snapshot: &FileDiffSnapshot, side| snapshot.diff.text(0, side).line_count();
        self.file == current.id
            && self.from == current.revision
            && self.to == next.revision
            && next.coverage() == SourceCoverage::Full
            && current.coverage() == SourceCoverage::Full
            && [Side::Old, Side::New].iter().all(|&side| {
                let map = &self.sides[side as usize];
                map.from_lines == counts(current, side) && map.to_lines == counts(next, side)
            })
    }
}
