//! Table blocks: a markdown table laid out as a grid of cells.
//!
//! The block's text is the table as markdown pipe rows, so selection,
//! find, and copy work on it like on any block and a copied table pastes
//! as one. Each cell's text is a slice of that text; the grid shows the
//! slices in columns as wide as their widest cell, a header row on a tint,
//! and separators between rows. A table wider than the column scrolls
//! sideways like wide code.

use std::ops::Range;
use std::sync::Arc;

use quark_render::scene::Rect;
use quark_text::{TextLayout, TextParams};

use super::{Block, BlockContent, BlockStyle, SpanTone, next_revision};
use crate::element::{ParagraphStyle, SelectableText, StyledSpan};

/// Horizontal padding inside a cell, in multiples of the font size.
const CELL_PAD_X: f32 = 0.7;
/// Vertical padding inside a cell, in multiples of the font size.
const CELL_PAD_Y: f32 = 0.35;
/// Narrowest column content, in multiples of the font size.
const MIN_COLUMN: f32 = 1.5;

/// One cell of a table block.
#[derive(Debug, Clone)]
pub struct TableCell {
    /// Row 0 is the header.
    pub row: usize,
    pub column: usize,
    /// The cell's text as styled runs; their texts joined are the block
    /// text at `range`.
    pub spans: Arc<[StyledSpan]>,
    /// One per span, when any span takes a theme color.
    pub tones: Option<Arc<[SpanTone]>>,
    /// Where the cell's text sits in the block text.
    pub range: Range<usize>,
}

/// The cells of a table block, row-major, `columns` per row.
#[derive(Debug, Clone)]
pub struct TableCells {
    pub cells: Arc<[TableCell]>,
    pub columns: usize,
    pub rows: usize,
}

impl Block {
    /// A table of `rows`, each a list of cells, each a list of styled runs
    /// with their tones; `rows[0]` is the header. Rows with fewer cells are
    /// padded with empty ones. The block text is the table as markdown
    /// (`| a | b |`, a `| --- |` rule under the header), with `|` inside a
    /// cell escaped and line breaks turned into spaces, which the cell
    /// shows as it is copied.
    pub fn table(
        key: quark::selection::BlockKey,
        rows: Vec<Vec<Vec<(StyledSpan, SpanTone)>>>,
    ) -> Self {
        let columns = rows.iter().map(Vec::len).max().unwrap_or(0).max(1);
        let mut text = String::new();
        let mut cells = Vec::with_capacity(rows.len() * columns);
        for (r, mut row) in rows.into_iter().enumerate() {
            row.resize_with(columns, Vec::new);
            if r > 0 {
                text.push('\n');
            }
            text.push('|');
            for (c, runs) in row.into_iter().enumerate() {
                text.push(' ');
                let start = text.len();
                let (spans, tones): (Vec<StyledSpan>, Vec<SpanTone>) = runs
                    .into_iter()
                    .map(|(span, tone)| {
                        let escaped = span.text.replace('|', "\\|").replace('\n', " ");
                        text.push_str(&escaped);
                        (
                            StyledSpan {
                                text: escaped,
                                ..span
                            },
                            tone,
                        )
                    })
                    .unzip();
                cells.push(TableCell {
                    row: r,
                    column: c,
                    spans: spans.into(),
                    tones: tones
                        .iter()
                        .any(|t| *t != SpanTone::Plain)
                        .then(|| tones.into()),
                    range: start..text.len(),
                });
                text.push_str(" |");
            }
            if r == 0 {
                text.push('\n');
                text.push('|');
                for _ in 0..columns {
                    text.push_str(" --- |");
                }
            }
        }
        let rows = cells.len() / columns;
        Self {
            key,
            content: BlockContent::Table(TableCells {
                cells: cells.into(),
                columns,
                rows,
            }),
            style: BlockStyle::default(),
            text: text.into(),
            tones: None,
            revision: next_revision(),
        }
    }
}

/// Where a table's rows and columns fall, for a font size and the natural
/// widths of its cells.
#[derive(Debug, Clone, PartialEq)]
pub struct TableMetrics {
    /// Left edge of each column from the table's left, then its right
    /// edge: `columns + 1` values.
    pub edges: Arc<[f32]>,
    pub row_height: f32,
    pub pad: (f32, f32),
}

impl TableMetrics {
    /// Height of a cell's text line at `font_size` and the default line
    /// height.
    pub fn line_height(font_size: f32) -> f32 {
        SelectableText::line_height_for(font_size)
    }

    fn new(paragraph: &ParagraphStyle, columns: usize, natural: impl Fn(usize) -> f32) -> Self {
        let font_size = paragraph.font_size;
        let pad = (
            (font_size * CELL_PAD_X).round(),
            (font_size * CELL_PAD_Y).round(),
        );
        let mut edges = Vec::with_capacity(columns + 1);
        let mut x = 0.0;
        edges.push(x);
        for column in 0..columns {
            // A point of slack, so text laid out at the content width
            // never wraps on rounding.
            let content = (natural(column).ceil() + 1.0).max(font_size * MIN_COLUMN);
            x += content + pad.0 * 2.0;
            edges.push(x);
        }
        Self {
            edges: edges.into(),
            row_height: (paragraph.line_height_points() + pad.1 * 2.0).ceil(),
            pad,
        }
    }

    pub fn width(&self) -> f32 {
        self.edges.last().copied().unwrap_or(0.0)
    }

    pub fn height(&self, rows: usize) -> f32 {
        self.row_height * rows as f32
    }

    /// The rectangle of cell `(row, column)` from the table's top left.
    pub fn cell(&self, row: usize, column: usize) -> Rect {
        let x = self.edges[column];
        Rect {
            x,
            y: self.row_height * row as f32,
            width: self.edges[column + 1] - x,
            height: self.row_height,
        }
    }
}

/// The params a cell's text is shaped with: one unwrapped line in the
/// body paragraph style. The element lays the same text out at its
/// column's width.
pub(super) fn cell_params(spans: &[StyledSpan], paragraph: &ParagraphStyle) -> TextParams {
    SelectableText::paragraph_params(spans, paragraph, f32::INFINITY)
}

/// A table block's measured grid, for hit-testing and highlights.
#[derive(Debug, Clone)]
pub struct TableGeometry {
    pub metrics: TableMetrics,
    columns: usize,
    cells: Vec<(Range<usize>, Option<Arc<TextLayout>>)>,
}

impl TableGeometry {
    /// Lays out `table` in `paragraph`, shaping each cell with `layout`.
    pub(super) fn new(
        table: &TableCells,
        paragraph: &ParagraphStyle,
        mut layout: impl FnMut(TextParams) -> Option<Arc<TextLayout>>,
    ) -> Self {
        let cells: Vec<(Range<usize>, Option<Arc<TextLayout>>)> = table
            .cells
            .iter()
            .map(|cell| {
                (
                    cell.range.clone(),
                    layout(cell_params(&cell.spans, paragraph)),
                )
            })
            .collect();
        let natural = |column: usize| {
            cells
                .iter()
                .skip(column)
                .step_by(table.columns.max(1))
                .filter_map(|(_, l)| l.as_ref().map(|l| l.size().0))
                .fold(0.0, f32::max)
        };
        Self {
            metrics: TableMetrics::new(paragraph, table.columns, natural),
            columns: table.columns,
            cells,
        }
    }

    fn text_origin(&self, index: usize) -> (f32, f32) {
        let cell = self
            .metrics
            .cell(index / self.columns, index % self.columns);
        (cell.x + self.metrics.pad.0, cell.y + self.metrics.pad.1)
    }

    /// Byte offset nearest `(x, y)` from the table's top left: the cell
    /// under the point, clamped into the grid.
    pub fn hit(&self, x: f32, y: f32) -> usize {
        if self.cells.is_empty() {
            return 0;
        }
        let rows = self.cells.len() / self.columns;
        let row = ((y / self.metrics.row_height).floor().max(0.0) as usize).min(rows - 1);
        let column = self.metrics.edges[1..]
            .iter()
            .position(|right| x < *right)
            .unwrap_or(self.columns - 1);
        let index = row * self.columns + column;
        let (range, layout) = &self.cells[index];
        let (ox, oy) = self.text_origin(index);
        match layout {
            Some(layout) => range.start + layout.hit(x - ox, y - oy).get().min(range.len()),
            None => range.start,
        }
    }

    /// Rectangles covering `range` of the block text, from the table's top
    /// left: the selected part of each cell's text.
    pub fn range_rects(&self, range: Range<usize>, out: &mut Vec<Rect>) {
        for (index, (cell, layout)) in self.cells.iter().enumerate() {
            let (start, end) = (range.start.max(cell.start), range.end.min(cell.end));
            let Some(layout) = layout.as_ref().filter(|_| start < end) else {
                continue;
            };
            let (ox, oy) = self.text_origin(index);
            out.extend(
                layout
                    .selection_rects(start - cell.start..end - cell.start)
                    .map(|r| r.offset(ox, oy)),
            );
        }
    }
}
