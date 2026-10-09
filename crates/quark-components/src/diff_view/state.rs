//! The row model under both views: segments of the diff, the flat row
//! table over them, and the frame `prepare` builds from its window.
//!
//! A segment is one document with its own expansion and projection: the
//! whole document for the static view, one file revision for a session
//! view. Rows of every segment, plus metadata, annotation, and preview
//! rows, sit in one [`VariableList`] in display order. Changing a segment
//! re-projects only that segment; the flat row table is then rebuilt from
//! the segments' projections, which costs a pass over row keys but no
//! diffing, projection, or shaping of the other segments.
//!
//! A file is named inside the view by its unit: the document's file index
//! in the static view, a stable slot per [`super::FileId`] in a session.
//! Row keys, selection keys, and paint caches carry the unit, so they stay
//! valid while other files change around them.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;

use quark_diff::{
    BlockKind, ContextPolicy, DiffDocument, Expansion, FileStatus, Mode, Projection, RowKind, Side,
    inline_diff, line_detail,
};
use quark_render::FontKind;
use quark_text::{LayoutCache, TextParams, TextStyle, TextSystem};
use quark_ui::virtual_list::{RowKey, VariableList};

use super::prepared::{
    Columns, FileFact, FrameRow, LineDetail, LinePaint, Metrics, PreparedKind, RowPaint, ViewFrame,
    row_height,
};
use super::{DiffViewState, PrepareKey};

/// No row, slot, or segment.
pub(crate) const NONE: u32 = u32::MAX;

const UNIT_SHIFT: u32 = 38;
const UNIT_MASK: u64 = 0x3F_FFFF;
const FACT_TAG: u64 = 8 << 60;
const ANNOTATION_TAG: u64 = 9 << 60;
const MORE_TAG: u64 = 10 << 60;

/// One document of the view, with its own expansion and projection.
#[derive(Debug)]
pub(crate) struct Segment {
    pub doc: Arc<DiffDocument>,
    /// Unit of the document's first file.
    pub slot: u32,
    /// Distinguishes this document from every other the view has shown,
    /// for paint stamps.
    pub generation: u64,
    /// The content revision anchors attach to.
    pub revision: u64,
    pub expansion: Expansion,
    pub projection: Projection,
    /// List index of each projection row, or [`NONE`] where a preview cut
    /// it.
    pub list_rows: Vec<u32>,
}

impl Segment {
    pub fn new(
        doc: Arc<DiffDocument>,
        slot: u32,
        generation: u64,
        mode: Mode,
        policy: ContextPolicy,
    ) -> Self {
        let expansion = Expansion::with_policy(&doc, policy);
        let projection = Projection::new(&doc, mode, &expansion);
        Self {
            doc,
            slot,
            generation,
            revision: 0,
            expansion,
            projection,
            list_rows: Vec::new(),
        }
    }

    pub fn unit(&self, file: u32) -> u32 {
        self.slot + file
    }

    /// Re-projects after an expansion or mode change.
    pub fn rebuild(&mut self, mode: Mode) {
        self.projection.rebuild(&self.doc, mode, &self.expansion);
    }

    /// The row's line on `side` as `(unit, store index)`.
    pub fn line(&self, row: u32, side: Side) -> Option<(u32, u32)> {
        let file = self.projection.file[row as usize];
        Some((self.unit(file), self.projection.line(row, side)?))
    }
}

/// Which segment shows each session file. Empty for the static view.
#[derive(Debug, Default)]
pub(crate) struct FileMap {
    /// Segment position of each slot, or [`NONE`].
    pub slot_seg: Vec<u32>,
    pub slot_ids: Vec<super::FileId>,
    pub id_slots: HashMap<super::FileId, u32>,
    /// Session mode, even while it holds no files.
    pub session: bool,
}

impl FileMap {
    pub fn is_static(&self) -> bool {
        !self.session
    }
}

/// What a row of the list shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RowRef {
    Line { seg: u32, row: u32 },
    Fact { seg: u32, file: u32, fact: u32 },
    Annotation { index: u32 },
    More { hidden: u32 },
}

/// The scroll position to keep while rows change: the top row's key and
/// how far into it the view is scrolled, and the source line it shows.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Anchor {
    pub key: u64,
    pub delta: f32,
    /// `(unit, side, store index)`.
    pub line: Option<(u32, Side, u32)>,
}

/// Facts about `file` its line rows do not show. See [`FileFact`]. A
/// missing final newline is a fact only where it differs between the sides
/// (or the file was added or deleted), since an unchanged one is not part
/// of the change.
pub(crate) fn file_facts(doc: &DiffDocument, file: u32) -> Vec<FileFact> {
    let facts = doc.facts(file);
    let mut out = Vec::new();
    if facts.binary {
        out.push(FileFact::Binary);
    }
    if let Some((old, new)) = facts.mode_change {
        out.push(FileFact::ModeChange { old, new });
    }
    match facts.status {
        FileStatus::Renamed if !facts.has_hunks && !facts.binary => {
            out.push(FileFact::RenameOnly);
        }
        FileStatus::Copied if !facts.has_hunks && !facts.binary => out.push(FileFact::CopyOnly),
        _ => {}
    }
    let eof = [facts.old_missing_newline, facts.new_missing_newline];
    for side in [Side::Old, Side::New] {
        let differs = match facts.status {
            FileStatus::Added => side == Side::New,
            FileStatus::Deleted => side == Side::Old,
            _ => eof[0] != eof[1],
        };
        if eof[side as usize] && differs {
            out.push(FileFact::NoNewlineAtEof(side));
        }
    }
    out
}

/// How a metadata row reads.
pub(crate) fn fact_title(fact: &FileFact) -> String {
    match fact {
        FileFact::Binary => "Binary file not shown".to_owned(),
        FileFact::ModeChange { old, new } => format!("File mode changed from {old} to {new}"),
        FileFact::RenameOnly => "Renamed without changes".to_owned(),
        FileFact::CopyOnly => "Copied without changes".to_owned(),
        FileFact::NoNewlineAtEof(Side::Old) => "No newline at end of the old file".to_owned(),
        FileFact::NoNewlineAtEof(Side::New) => "No newline at end of the new file".to_owned(),
    }
}

/// Bytes of `text` a line limited to `limit` bytes shows; see
/// [`quark_diff::line_detail`].
pub(crate) fn shown_len(text: &str, limit: usize) -> usize {
    match line_detail(text, limit) {
        LineDetail::Complete => text.len(),
        LineDetail::Prefix { shown_bytes, .. } => shown_bytes as usize,
    }
}

/// Largest char boundary at or below `byte`, clamped to the text.
pub(crate) fn floor_boundary(text: &str, byte: usize) -> usize {
    let mut at = byte.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// Zero-based source line of store line `index` of `file`'s `side`.
pub(crate) fn source_line(doc: &DiffDocument, file: u32, side: Side, index: u32) -> u32 {
    if !doc.files().partial[file as usize] {
        return index;
    }
    let (h, b) = (doc.hunks(), doc.blocks());
    for hunk in doc.files().hunks[file as usize].clone() {
        for block in h.blocks[hunk as usize].clone() {
            let bi = block as usize;
            let (store, len, line) = match side {
                Side::Old => (b.old_store[bi], b.old_len[bi], b.old_line[bi]),
                Side::New => (b.new_store[bi], b.new_len[bi], b.new_line[bi]),
            };
            if (store..store + len).contains(&index) {
                debug_assert!(b.kind[bi] == BlockKind::Context || len > 0);
                return (line + index - store).saturating_sub(1);
            }
        }
    }
    index
}

/// Store index of zero-based source `line` of `file`'s `side`, when the
/// store holds it.
pub(crate) fn store_index(doc: &DiffDocument, file: u32, side: Side, line: u32) -> Option<u32> {
    let store = doc.text(file, side);
    if !doc.files().partial[file as usize] {
        return (line < store.line_count()).then_some(line);
    }
    let (h, b) = (doc.hunks(), doc.blocks());
    for hunk in doc.files().hunks[file as usize].clone() {
        for block in h.blocks[hunk as usize].clone() {
            let bi = block as usize;
            let (first, len, start) = match side {
                Side::Old => (b.old_store[bi], b.old_len[bi], b.old_line[bi]),
                Side::New => (b.new_store[bi], b.new_len[bi], b.new_line[bi]),
            };
            let one_based = line + 1;
            if len > 0 && (start..start + len).contains(&one_based) {
                return Some(first + one_based - start);
            }
        }
    }
    None
}

/// `@@ -a,b +c,d @@ section` of a hunk.
fn hunk_title(doc: &DiffDocument, hunk: u32) -> String {
    let (h, i) = (doc.hunks(), hunk as usize);
    let section = &h.section[i];
    format!(
        "@@ -{},{} +{},{} @@{}{section}",
        h.old_start[i],
        h.old_len[i],
        h.new_start[i],
        h.new_len[i],
        if section.is_empty() { "" } else { " " }
    )
}

/// Sizes for a font of `font_size` points whose digits are `char_w` wide.
fn metrics_for(font_size: f32, line_height: f32, char_w: f32, segments: &[Segment]) -> Metrics {
    let lines = segments
        .iter()
        .flat_map(|s| {
            let doc = &s.doc;
            (0..doc.file_count())
                .flat_map(move |f| [doc.text(f, Side::Old), doc.text(f, Side::New)])
        })
        .map(|t| t.line_count())
        .max()
        .unwrap_or(0)
        .max(1);
    // Partial stores hold fewer lines than the files; leave room for a
    // few more digits than they need.
    let digits = (lines.ilog10() + 1).max(3) as f32;
    Metrics {
        font_size,
        line_h: (font_size * line_height).round(),
        char_w,
        number_w: (digits * char_w + char_w * 2.0).ceil(),
        sign_w: (char_w * 2.0).ceil(),
        text_pad: (char_w * 0.5).ceil(),
    }
}

/// The scroll handle a side's text column uses: unified has one column.
pub(crate) fn column_slot(mode: Mode, side: Side) -> usize {
    match mode {
        Mode::Unified => Side::New as usize,
        Mode::Split => side as usize,
    }
}

impl DiffViewState {
    // ---- Units and keys ------------------------------------------------

    /// Segment position and file index within it of `unit`.
    pub(crate) fn locate(&self, unit: u32) -> Option<(usize, u32)> {
        if self.files.is_static() {
            let doc = &self.segments.first()?.doc;
            return (unit < doc.file_count()).then_some((0, unit));
        }
        let seg = *self.files.slot_seg.get(unit as usize)?;
        (seg != NONE).then_some((seg as usize, 0))
    }

    /// Row key of projection row `row` of segment `seg`: the projection's
    /// key with the file replaced by its unit.
    pub(crate) fn line_key(&self, seg: usize, row: u32) -> u64 {
        let segment = &self.segments[seg];
        let key = segment.projection.row_key(row);
        let file = (key >> UNIT_SHIFT) & UNIT_MASK;
        let unit = (u64::from(segment.slot) + file) & UNIT_MASK;
        (key & !(UNIT_MASK << UNIT_SHIFT)) | (unit << UNIT_SHIFT)
    }

    pub(crate) fn ref_key(&self, r: RowRef) -> u64 {
        match r {
            RowRef::Line { seg, row } => self.line_key(seg as usize, row),
            RowRef::Fact { seg, file, fact } => {
                let unit = u64::from(self.segments[seg as usize].unit(file)) & UNIT_MASK;
                FACT_TAG | (unit << UNIT_SHIFT) | u64::from(fact)
            }
            RowRef::Annotation { index } => {
                ANNOTATION_TAG | (self.annotations.entry(index).annotation.id.0 & ((1 << 60) - 1))
            }
            RowRef::More { .. } => MORE_TAG,
        }
    }

    /// The projection row a list row belongs to, for the static view.
    pub(crate) fn projection_row_of(&self, index: usize) -> Option<u32> {
        let segment = self.segments.first()?;
        match *self.refs.get(index)? {
            RowRef::Line { row, .. } => Some(row),
            RowRef::Fact { file, .. } => segment.projection.file_rows.get(file as usize).copied(),
            RowRef::Annotation { .. } | RowRef::More { .. } => {
                (0..index).rev().find_map(|i| match self.refs[i] {
                    RowRef::Line { row, .. } => Some(row),
                    _ => None,
                })
            }
        }
    }

    // ---- Rows ----------------------------------------------------------

    /// The first visible row, how far into it the view is scrolled, and
    /// the source line it shows, to restore after the rows change.
    pub(crate) fn anchor(&self) -> Option<Anchor> {
        let rows = self.list.rows();
        let index = rows.row_at(self.list.scroll_offset())?;
        let line = match self.refs.get(index) {
            Some(&RowRef::Line { seg, row }) => {
                let segment = &self.segments[seg as usize];
                [Side::New, Side::Old]
                    .into_iter()
                    .find_map(|side| segment.line(row, side).map(|(u, i)| (u, side, i)))
            }
            _ => None,
        };
        Some(Anchor {
            key: rows.keys()[index].0,
            delta: self.list.scroll_offset() - rows.offset_of_index(index),
            line,
        })
    }

    /// List index of the row showing store line `index` of `unit`'s `side`.
    pub(crate) fn list_index_of_line(&self, unit: u32, side: Side, index: u32) -> Option<u32> {
        let (seg, file) = self.locate(unit)?;
        let segment = &self.segments[seg];
        let row = segment.projection.row_of(file, side, index)?;
        segment
            .list_rows
            .get(row as usize)
            .copied()
            .filter(|&i| i != NONE)
    }

    /// Rebuilds the row table from the segments' projections, keeping
    /// `anchor` where it was: by the source line it showed, else by its
    /// key, else at the same offset.
    pub(crate) fn rebuild_rows(&mut self, anchor: Option<Anchor>) {
        let m = self.metrics();
        let placed = self.annotation_placement();
        let limit = self.preview.map(|p| p.max_rows);
        let collapsed = &self.collapsed;
        let mut refs = Vec::with_capacity(self.refs.len());
        let mut hidden = 0u32;
        for (si, segment) in self.segments.iter_mut().enumerate() {
            let seg = si as u32;
            let p = &segment.projection;
            segment.list_rows.clear();
            segment.list_rows.resize(p.len() as usize, NONE);
            for row in 0..p.len() {
                let (kind, file) = (p.kind[row as usize], p.file[row as usize]);
                if kind != RowKind::FileHeader && collapsed.contains(&(segment.slot + file)) {
                    continue;
                }
                if limit.is_some_and(|max| refs.len() as u32 >= max) {
                    hidden += 1;
                    continue;
                }
                segment.list_rows[row as usize] = refs.len() as u32;
                refs.push(RowRef::Line { seg, row });
                let unit = segment.slot + file;
                if kind == RowKind::FileHeader && !collapsed.contains(&unit) {
                    let facts = file_facts(&segment.doc, file).len() as u32;
                    refs.extend((0..facts).map(|fact| RowRef::Fact { seg, file, fact }));
                    if let Some(outdated) = placed.outdated.get(&unit) {
                        refs.extend(outdated.iter().map(|&index| RowRef::Annotation { index }));
                    }
                }
                if kind.is_line() {
                    for side in [Side::Old, Side::New] {
                        let Some(index) = p.line(row, side) else {
                            continue;
                        };
                        if let Some(list) = placed.at_line.get(&(unit, side, index)) {
                            refs.extend(list.iter().map(|&index| RowRef::Annotation { index }));
                        }
                    }
                }
            }
        }
        if hidden > 0 {
            refs.push(RowRef::More { hidden });
        }
        self.refs = refs;
        let keys: Vec<RowKey> = self.refs.iter().map(|&r| RowKey(self.ref_key(r))).collect();
        self.key_index = keys.iter().zip(0..).map(|(k, i)| (k.0, i)).collect();
        let mut list = VariableList::new(m.line_h, self.viewport.1);
        // A fresh list is pinned to the bottom; a diff opens at the top.
        list.set_scroll_offset(0.0);
        let _ = list.extend(&keys);
        for (index, key) in keys.iter().enumerate() {
            let height = match self.refs[index] {
                RowRef::Line { seg, row } => {
                    let kind = self.segments[seg as usize].projection.kind[row as usize];
                    (!kind.is_line()).then(|| row_height(kind, &m, &self.presentation))
                }
                RowRef::Fact { .. } => None,
                RowRef::Annotation { index } => Some(self.annotation_height(index, &m)),
                RowRef::More { .. } => Some((m.line_h * 1.4).round()),
            };
            if let Some(height) = height {
                let _ = list.set_height(*key, height);
            }
        }
        let offset = anchor
            .and_then(|a| {
                let by_line = a.line.and_then(|(unit, side, index)| {
                    let i = self.list_index_of_line(unit, side, index)?;
                    Some(list.rows().offset_of_index(i as usize))
                });
                let top = by_line.or_else(|| list.rows().offset_of(RowKey(a.key)))?;
                Some(top + a.delta)
            })
            .unwrap_or_else(|| self.list.scroll_offset());
        list.set_scroll_offset(offset);
        self.list = list;
        self.revision += 1;
    }

    // ---- Frame ---------------------------------------------------------

    pub(crate) fn overscan(&self) -> f32 {
        self.metrics().line_h * 8.0
    }

    pub(crate) fn metrics(&self) -> Metrics {
        self.metrics
            .map_or_else(|| self.estimated_metrics(), |(m, _)| m)
    }

    /// Metrics before the font has been measured.
    fn estimated_metrics(&self) -> Metrics {
        let font_size = self.style.font_size;
        metrics_for(
            font_size,
            self.style.line_height,
            font_size * 0.6,
            &self.segments,
        )
    }

    /// Follows an automatic layout across the viewport width (see
    /// [`super::presentation::auto_mode`]), keeping the top source line in
    /// place. Explicit layouts stay put.
    fn resolve_layout(&mut self) {
        let m = self.metrics();
        if let Some(mode) = super::presentation::auto_mode(
            self.presentation.layout,
            self.mode,
            self.viewport.0,
            &m,
            &self.presentation,
        ) {
            self.switch_mode(mode);
        }
    }

    pub(crate) fn build_frame(
        &mut self,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
    ) {
        let measured = (scale.to_bits(), text.font_epoch());
        if self.metrics.is_none_or(|(_, key)| key != measured) {
            self.measure_font(text, layouts, scale);
        }
        self.resolve_layout();
        let m = self.metrics();
        let columns = Columns::new(self.mode, self.viewport.0, &m, &self.presentation);
        if self.style.wrap {
            self.measure_window(text, layouts, scale, &columns);
        }
        let window = self.list.window(self.overscan()).range;
        let key = PrepareKey {
            window: (window.start, window.end),
            scroll: self.list.scroll_offset().to_bits(),
            revision: self.revision,
            scale: scale.to_bits(),
        };
        if self.prepared == Some(key) && self.frame.is_some() {
            return;
        }
        self.prepared = Some(key);
        self.frame_id += 1;
        if self.files.is_static() {
            let file_of = |r: &RowRef| match *r {
                RowRef::Line { seg, row } => {
                    Some(self.segments[seg as usize].projection.file[row as usize])
                }
                RowRef::Fact { file, .. } => Some(file),
                _ => None,
            };
            let refs = &self.refs[window.clone()];
            let first = refs.iter().find_map(file_of);
            let last = refs.iter().rev().find_map(file_of);
            if let (Some(first), Some(last)) = (first, last) {
                self.syntax.set_visible_files(first..last + 1);
            }
        }

        let scroll = self.list.scroll_offset();
        let ordered = self.ordered_selection();
        let mut kept = HashMap::with_capacity(window.len());
        let mut rows = Vec::with_capacity(window.len());
        for index in window {
            let r = self.refs[index];
            let key = self.ref_key(r);
            let stamp = self.stamp(r, &columns, scale);
            let paint = match self.painted.remove(&key) {
                Some(p) if p.stamp == stamp => p,
                _ => Rc::new(self.build_row(r, stamp, text, layouts, scale, &columns)),
            };
            for (side, line) in [Side::Old, Side::New].into_iter().zip(&paint.sides) {
                let Some(line) = line else {
                    continue;
                };
                let slot = column_slot(self.mode, side);
                self.content_w[slot] = self.content_w[slot]
                    .max((line.layout.size().0 + m.text_pad * 2.0 + m.char_w).ceil());
            }
            let (selected, search) = match r {
                RowRef::Line { seg, row } => {
                    let selected = [Side::Old, Side::New].map(|side| {
                        let line = paint.sides[side as usize].as_ref()?;
                        let len = line.layout.text().len();
                        self.row_selection(ordered, (seg, row), side, len)
                    });
                    (selected, self.search_marks(seg as usize, row, &paint))
                }
                _ => Default::default(),
            };
            let rows_table = self.list.rows();
            rows.push(FrameRow {
                key,
                index: index as u32,
                top: rows_table.offset_of_index(index) - scroll,
                height: rows_table.height_of(RowKey(key)).unwrap_or(m.line_h),
                paint: paint.clone(),
                selected,
                search,
                focused: self.focused == Some(key),
            });
            kept.insert(key, paint);
        }
        let sticky_header =
            self.sticky_header(scroll, &columns, scale, text, layouts, &mut kept, &m);
        self.painted = kept;
        self.frame = Some(Rc::new(ViewFrame {
            id: self.id,
            label: self.label,
            focus: self.focus,
            viewport: self.viewport,
            metrics: m,
            columns,
            presentation: self.presentation,
            appearance: self.appearance,
            wrap: self.style.wrap,
            rows,
            sticky_header,
            content_w: self.content_w,
            scroll,
            total: self.list.rows().total_extent(),
            row_count: self.refs.len() as u32,
            hscroll: self.hscroll.clone(),
            scrollbar_auto_hide: self.scrollbar_auto_hide,
            scrollbar: self.scrollbar.clone(),
        }));
    }

    /// The header to pin at the top: with sticky headers on, the file
    /// header of the top row's file when that header is above the
    /// viewport. Its paint is kept with the window's.
    #[allow(clippy::too_many_arguments)]
    fn sticky_header(
        &mut self,
        scroll: f32,
        columns: &Columns,
        scale: f32,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        kept: &mut HashMap<u64, Rc<RowPaint>>,
        m: &Metrics,
    ) -> Option<FrameRow> {
        if !self.presentation.sticky_headers {
            return None;
        }
        let top = self.list.rows().row_at(scroll)?;
        let (seg, file) = match self.refs[top] {
            RowRef::Line { seg, row } => (
                seg,
                self.segments[seg as usize].projection.file[row as usize],
            ),
            RowRef::Fact { seg, file, .. } => (seg, file),
            _ => return None,
        };
        let segment = &self.segments[seg as usize];
        let header = *segment.projection.file_rows.get(file as usize)?;
        let index = *segment.list_rows.get(header as usize)?;
        if index == NONE || index as usize >= top {
            return None;
        }
        let r = RowRef::Line { seg, row: header };
        let key = self.ref_key(r);
        let stamp = self.stamp(r, columns, scale);
        let paint = match kept.get(&key).or_else(|| self.painted.get(&key)) {
            Some(p) if p.stamp == stamp => p.clone(),
            _ => Rc::new(self.build_row(r, stamp, text, layouts, scale, columns)),
        };
        kept.insert(key, paint.clone());
        let height = self.list.rows().height_of(RowKey(key)).unwrap_or(m.line_h);
        Some(FrameRow {
            key,
            index,
            top: 0.0,
            height,
            paint,
            selected: [None, None],
            search: Default::default(),
            focused: self.focused == Some(key),
        })
    }

    /// Heights of rows entering the window, from their wrapped layouts.
    fn measure_window(
        &mut self,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
        columns: &Columns,
    ) {
        let m = self.metrics();
        let overscan = self.overscan();
        let mut list = std::mem::replace(&mut self.list, VariableList::new(1.0, 0.0));
        let mut painted = std::mem::take(&mut self.painted);
        let this = &*self;
        list.measure_visible(this.viewport.0, overscan, |key, _| {
            let Some(&index) = this.key_index.get(&key) else {
                return m.line_h;
            };
            let r = this.refs[index as usize];
            if let RowRef::Annotation { index } = r {
                return this.annotation_height(index, &m);
            }
            let stamp = this.stamp(r, columns, scale);
            match painted.get(&key) {
                Some(p) if p.stamp == stamp => p.height(&m, &this.presentation),
                _ => {
                    let p = this.build_row(r, stamp, text, layouts, scale, columns);
                    let height = p.height(&m, &this.presentation);
                    painted.insert(key, Rc::new(p));
                    height
                }
            }
        });
        self.list = list;
        self.painted = painted;
    }

    /// Identifies everything a row's paint depends on besides its key.
    fn stamp(&self, r: RowRef, columns: &Columns, scale: f32) -> u64 {
        let mut h = std::hash::DefaultHasher::new();
        (
            scale.to_bits(),
            self.style.font_size.to_bits(),
            self.style.line_height.to_bits(),
            self.mode,
            self.limits,
        )
            .hash(&mut h);
        match r {
            RowRef::Line { seg, row } => {
                let segment = &self.segments[seg as usize];
                let p = &segment.projection;
                let file = p.file[row as usize];
                (segment.generation, segment.slot).hash(&mut h);
                if self.files.is_static() {
                    self.syntax.file_generation(file).hash(&mut h);
                }
                if self.style.wrap {
                    let m = self.metrics();
                    for side in [Side::Old, Side::New] {
                        columns.wrap_width(side, &m).to_bits().hash(&mut h);
                    }
                }
                if p.kind[row as usize] == RowKind::FileHeader {
                    let unit = segment.unit(file);
                    (self.collapsed.contains(&unit), self.file_annotations(unit)).hash(&mut h);
                }
                if let Some(gap) = p.gap(row) {
                    // Its count changes as lines are revealed.
                    gap.hidden.hash(&mut h);
                    self.gap_annotations(segment, gap).hash(&mut h);
                }
            }
            RowRef::Fact { seg, .. } => self.segments[seg as usize].generation.hash(&mut h),
            RowRef::Annotation { index } => self.annotations.entry(index).hash(&mut h),
            RowRef::More { hidden } => hidden.hash(&mut h),
        }
        h.finish()
    }

    fn measure_font(&mut self, text: &mut TextSystem, layouts: &mut LayoutCache, scale: f32) {
        let style = self.text_style();
        let params = TextParams::new("0000000000", style).scale_factor(scale);
        let char_w = layouts
            .layout(text, &params)
            .map_or(self.style.font_size * 0.6, |l| l.size().0 / 10.0);
        let m = metrics_for(
            self.style.font_size,
            self.style.line_height,
            char_w,
            &self.segments,
        );
        let fonts = text.font_epoch();
        let changed = self.metrics.is_none_or(|(old, _)| old != m);
        let new_fonts = self.metrics.is_some_and(|(_, (_, old))| old != fonts);
        self.metrics = Some((m, (scale.to_bits(), fonts)));
        if new_fonts {
            // Rows were shaped and wrapped with the old fonts.
            self.painted.clear();
            self.content_w = [0.0; 2];
        }
        if changed || new_fonts {
            let anchor = self.anchor();
            self.rebuild_rows(anchor);
        }
    }

    fn text_style(&self) -> TextStyle {
        TextStyle::new(self.style.font_size)
            .kind(FontKind::Mono)
            .line_height((self.style.font_size * self.style.line_height).round())
    }

    fn build_row(
        &self,
        r: RowRef,
        stamp: u64,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
        columns: &Columns,
    ) -> RowPaint {
        let blank = |kind: PreparedKind, file: u32| RowPaint {
            stamp,
            kind,
            file,
            sides: [None, None],
            source_lines: [None, None],
            title: Arc::from(""),
            gap: None,
            status: FileStatus::Modified,
            stats: (0, 0),
            binary: false,
            syntax: None,
            collapsed: false,
            annotations: 0,
        };
        match r {
            RowRef::Line { seg, row } => {
                self.build_line_row(seg as usize, row, stamp, text, layouts, scale, columns)
            }
            RowRef::Fact { seg, file, fact } => {
                let segment = &self.segments[seg as usize];
                let facts = file_facts(&segment.doc, file);
                let fact = facts[fact as usize].clone();
                RowPaint {
                    title: fact_title(&fact).into(),
                    ..blank(PreparedKind::Fact(fact), segment.unit(file))
                }
            }
            RowRef::Annotation { index } => {
                let (slot, unit, title) = self.annotation_slot(index);
                RowPaint {
                    title,
                    ..blank(PreparedKind::Annotation(slot), unit)
                }
            }
            RowRef::More { hidden } => {
                let rows = if hidden == 1 { "row" } else { "rows" };
                RowPaint {
                    title: format!("{hidden} more {rows}").into(),
                    ..blank(
                        PreparedKind::More {
                            hidden_rows: hidden,
                        },
                        0,
                    )
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn build_line_row(
        &self,
        seg: usize,
        row: u32,
        stamp: u64,
        text: &mut TextSystem,
        layouts: &mut LayoutCache,
        scale: f32,
        columns: &Columns,
    ) -> RowPaint {
        let segment = &self.segments[seg];
        let (doc, p) = (&segment.doc, &segment.projection);
        let r = row as usize;
        let (kind, file) = (p.kind[r], p.file[r]);
        let meta = &doc.files().meta[file as usize];
        let mut paint = RowPaint {
            stamp,
            kind: PreparedKind::Diff(kind),
            file: segment.unit(file),
            sides: [None, None],
            source_lines: [None, None],
            title: Arc::from(""),
            gap: None,
            status: meta.status,
            stats: (
                doc.files().additions[file as usize],
                doc.files().deletions[file as usize],
            ),
            binary: meta.binary,
            syntax: None,
            collapsed: false,
            annotations: 0,
        };
        match kind {
            RowKind::FileHeader => {
                let unit = segment.unit(file);
                if self.files.is_static() {
                    paint.syntax = self.syntax.file_status(file);
                }
                paint.collapsed = self.collapsed.contains(&unit);
                paint.annotations = self.file_annotations(unit);
                paint.title = match (&meta.old_path, &meta.new_path) {
                    (Some(old), Some(new)) if old != new => format!("{old} \u{2192} {new}").into(),
                    _ => doc.path(file).into(),
                };
            }
            RowKind::HunkHeader => paint.title = hunk_title(doc, p.hunk[r]).into(),
            RowKind::Gap => {
                let gap = p.gap(row).expect("gap row");
                paint.gap = Some(quark_diff::GapId {
                    file: segment.unit(file),
                    ..gap.id
                });
                let lines = if gap.hidden == 1 { "line" } else { "lines" };
                let mut title = format!("{} unchanged {lines}", gap.hidden);
                let notes = self.gap_annotations(segment, gap);
                if notes > 0 {
                    let s = if notes == 1 { "" } else { "s" };
                    title.push_str(&format!(", {notes} annotation{s}"));
                }
                if let Some(h) = gap.id.hunk {
                    title.push_str("    ");
                    title.push_str(&hunk_title(doc, h));
                }
                paint.title = title.into();
            }
            _ => {
                let m = self.metrics();
                let words = self.word_ranges(segment, row);
                for side in [Side::Old, Side::New] {
                    let Some(index) = p.line(row, side) else {
                        continue;
                    };
                    paint.source_lines[side as usize] = Some(source_line(doc, file, side, index));
                    if p.mode == Mode::Unified && kind == RowKind::Context && side == Side::Old {
                        // Unified context shows the new side's text.
                        continue;
                    }
                    let store = doc.text(file, side);
                    let line = store.display_line(index).unwrap_or("");
                    let detail = line_detail(line, self.limits.shaped_line_bytes);
                    let shown = shown_len(line, self.limits.shaped_line_bytes);
                    let line = &line[..shown];
                    let range = store.display_range(index).unwrap_or(0..0);
                    let range = range.start..range.start + shown;
                    let (spans, tones) = if self.files.is_static() {
                        self.syntax.spans(file, side, range)
                    } else {
                        (Vec::new(), Arc::from([]))
                    };
                    let params = TextParams::new(line, self.text_style())
                        .spans(spans)
                        .scale_factor(scale)
                        .wrap_width(self.style.wrap.then(|| columns.wrap_width(side, &m)));
                    let Ok(layout) = layouts.layout(text, &params) else {
                        continue;
                    };
                    let words = clip_ranges(&words[side as usize], shown);
                    paint.sides[side as usize] = Some(LinePaint {
                        layout,
                        tones,
                        words,
                        detail,
                    });
                }
            }
        }
        paint
    }

    /// Changed words of `row`'s lines, by side.
    fn word_ranges(&self, segment: &Segment, row: u32) -> [Vec<Range<usize>>; 2] {
        let p = &segment.projection;
        let r = row as usize;
        let (old, new) = match p.kind[r] {
            RowKind::Modified => (p.old[r], p.new[r]),
            RowKind::Removed => (p.old[r], p.pair[r]),
            RowKind::Added => (p.pair[r], p.new[r]),
            _ => return Default::default(),
        };
        if old == quark_diff::NONE || new == quark_diff::NONE {
            return Default::default();
        }
        let file = p.file[r];
        let line = |side, i| segment.doc.text(file, side).display_line(i).unwrap_or("");
        let (a, b) = (line(Side::Old, old), line(Side::New, new));
        if a.len().max(b.len()) > self.limits.inline_line_bytes {
            return Default::default();
        }
        let d = inline_diff(a, b);
        let bytes = |v: Vec<Range<u32>>| -> Vec<Range<usize>> {
            v.into_iter()
                .map(|r| r.start as usize..r.end as usize)
                .collect()
        };
        [bytes(d.old), bytes(d.new)]
    }
}

/// `ranges` cut to the first `len` bytes.
fn clip_ranges(ranges: &[Range<usize>], len: usize) -> Vec<Range<usize>> {
    ranges
        .iter()
        .filter(|r| r.start < len)
        .map(|r| r.start..r.end.min(len))
        .collect()
}

#[cfg(test)]
mod tests {
    use quark_ui::FocusId;

    use super::*;
    use crate::diff_view::DiffStyle;

    // Catches rows shaped and wrapped before a font change being kept: a
    // wrapped diff must lay out as a fresh view does in the new fonts.
    #[test]
    fn font_change_reshapes_and_rewraps_every_row() {
        let long = "let wrapped = some_function(first_argument, second_argument, third);";
        let patch = format!(
            "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-{long}\n+{long} // x\n"
        );
        let doc = || quark_diff::parse_unified(&patch).unwrap();
        let open = || {
            let mut state = DiffViewState::new("test.diff", FocusId::new(1), doc());
            state.set_style(DiffStyle {
                wrap: true,
                ..DiffStyle::default()
            });
            state.set_viewport(260.0, 400.0);
            state
        };
        let mut text = TextSystem::vendored_only(&Default::default());
        let mut layouts = LayoutCache::default();
        // Row heights, and each side's text in pixels wide.
        let layout =
            |state: &mut DiffViewState, text: &mut TextSystem, layouts: &mut LayoutCache| {
                state.prepare(text, layouts, 1.0, 0);
                let frame = state.frame.as_ref().unwrap();
                let rows: Vec<String> = frame
                    .rows
                    .iter()
                    .map(|row| {
                        let widths: Vec<f32> = row
                            .paint
                            .sides
                            .iter()
                            .flatten()
                            .map(|side| side.layout.size().0)
                            .collect();
                        format!("{}:{widths:?}", row.height)
                    })
                    .collect();
                rows.join(" ")
            };
        let mut state = open();
        let before = layout(&mut state, &mut text, &mut layouts);

        text.set_font_settings(&quark_text::FontSettings {
            // Proportional, so the lines wrap elsewhere.
            mono_family: "Inter".into(),
            ..Default::default()
        });
        let after = layout(&mut state, &mut text, &mut layouts);

        let expected = layout(&mut open(), &mut text, &mut layouts);
        assert_ne!(before, expected, "the fonts lay the rows out differently");
        assert_eq!(after, expected);
    }
}
