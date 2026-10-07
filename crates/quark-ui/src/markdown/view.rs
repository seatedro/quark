//! `markdown_view`: renders a [`MarkdownDoc`] with SelectableText and
//! CodeBlock, one selectable region per block (and per table cell).

use std::ops::Range;
use std::sync::Arc;

use quark_render::{FontKind, FontWeight, Scene};
use quark_text::TextStyle;

use super::model::{BlockKind, ListMarker, MarkdownDoc, SpanFlags};
use crate::design::{Alpha, Sp, Sz};
use crate::element::{
    AnyElement, Bounds, Element, ElementContext, IntoAnyElement, LayoutEngine, LayoutId,
    LinkHandler, StyledSpan, code_block, div, selectable_rich_text, styled_params, text,
};
use crate::style::Styled;
use crate::theme::Theme;

/// Region key of a block: `prefix + (block << 16)`. Recover the block with
/// `(key - prefix) >> 16`.
pub fn block_key(prefix: u64, block: usize) -> u64 {
    prefix.wrapping_add((block as u64) << 16)
}

/// Region key of the `cell`-th cell (row-major, from 0) of table `block`.
pub fn cell_key(prefix: u64, block: usize, cell: usize) -> u64 {
    block_key(prefix, block).wrapping_add(cell as u64 + 1)
}

/// Body line height factor, shared with SelectableText.
const LINE: f32 = 1.35;

/// Renders markdown blocks. Every frame re-parses nothing when built from a
/// shared [`MarkdownDoc`]; text layouts come from the frame's layout cache,
/// keyed by content, so while a document streams in only blocks whose text
/// changed are shaped again.
pub struct MarkdownView {
    doc: Arc<MarkdownDoc>,
    width: f32,
    font_size: f32,
    key_prefix: u64,
    selections: Vec<(u64, (usize, usize))>,
    on_link: LinkHandler,
    tree: Option<AnyElement>,
}

pub fn markdown_view(md: &str) -> MarkdownView {
    markdown_doc_view(Arc::new(MarkdownDoc::parse(md)))
}

pub fn markdown_doc_view(doc: Arc<MarkdownDoc>) -> MarkdownView {
    MarkdownView {
        doc,
        width: 0.0,
        font_size: 14.0,
        key_prefix: 0,
        selections: Vec::new(),
        on_link: LinkHandler::default(),
        tree: None,
    }
}

impl MarkdownView {
    pub fn width(mut self, w: f32) -> Self {
        self.width = w;
        self
    }

    pub fn size(mut self, s: f32) -> Self {
        self.font_size = s;
        self
    }

    /// Base of every region key; see [`block_key`] and [`cell_key`].
    pub fn key_prefix(mut self, prefix: u64) -> Self {
        self.key_prefix = prefix;
        self
    }

    /// Highlights `range` (bytes of the region's text) in the region `key`.
    pub fn selection(mut self, key: u64, range: (usize, usize)) -> Self {
        self.selections.push((key, range));
        self
    }

    /// Action a link click emits, given its URL. Defaults to
    /// [`crate::element::LinkClicked`].
    pub fn on_link(mut self, f: impl Fn(&Arc<str>) -> crate::Action + 'static) -> Self {
        self.on_link = LinkHandler::new(f);
        self
    }

    fn selection_for(&self, key: u64) -> Option<(usize, usize)> {
        self.selections
            .iter()
            .find(|(k, _)| *k == key)
            .map(|(_, range)| *range)
    }

    /// Span weights override the block's base weight in SelectableText, so
    /// the base (heading, table header) is folded into every span here.
    fn styled(&self, spans: Range<usize>, base: FontWeight, theme: &Theme) -> Vec<StyledSpan> {
        spans
            .map(|s| {
                let (text, flags, url) = self.doc.span(s);
                let code = flags.contains(SpanFlags::CODE);
                let image = flags.contains(SpanFlags::IMAGE);
                StyledSpan {
                    font_kind: if code { FontKind::Mono } else { FontKind::Ui },
                    font_weight: if flags.contains(SpanFlags::BOLD) {
                        FontWeight::Bold
                    } else {
                        base
                    },
                    italic: flags.contains(SpanFlags::ITALIC) || image,
                    color: image.then_some(theme.colors.text_muted),
                    pill: code.then_some(theme.colors.element_background),
                    strikethrough: flags.contains(SpanFlags::STRIKE),
                    link: url.cloned(),
                    ..StyledSpan::plain(text)
                }
            })
            .collect()
    }

    fn build(&self, cx: &mut ElementContext) -> AnyElement {
        let theme = cx.theme;
        let mut column = div().flex_col().w(self.width);
        for block in 0..self.doc.len() {
            let top = if block == 0 {
                0.0
            } else {
                self.gap_before(block)
            };
            column = column.child(div().pt(top).child(self.block(block, theme, cx)));
        }
        column.into_any()
    }

    fn gap_before(&self, block: usize) -> f32 {
        let doc = &self.doc;
        match doc.kind(block) {
            BlockKind::Heading(_) => Sp::LG,
            _ if doc.indent(block) > 0 && doc.indent(block - 1) > 0 => Sp::XS,
            _ => Sp::SM,
        }
    }

    fn block(&self, block: usize, theme: &Theme, cx: &mut ElementContext) -> AnyElement {
        let doc = &self.doc;
        let fs = self.font_size;
        let quote = doc.quote_depth(block) as f32;
        let indent = doc.indent(block) as f32;
        let gutter = fs * 1.75;
        let quote_w = quote * (Sz::GUTTER_STRIPE_W + Sp::MD);
        let list_w = indent * gutter;
        let inner_w = (self.width - quote_w - list_w).max(1.0);
        let key = block_key(self.key_prefix, block);

        let content = match doc.kind(block) {
            BlockKind::Paragraph | BlockKind::Heading(_) => {
                let (size, weight) = match doc.kind(block) {
                    BlockKind::Heading(1) => (fs * 1.5, FontWeight::Bold),
                    BlockKind::Heading(2) => (fs * 1.3, FontWeight::Bold),
                    BlockKind::Heading(3) => (fs * 1.15, FontWeight::Semibold),
                    BlockKind::Heading(_) => (fs, FontWeight::Semibold),
                    _ => (fs, FontWeight::Normal),
                };
                let mut el = selectable_rich_text(self.styled(doc.spans(block), weight, theme))
                    .width(inner_w)
                    .size(size)
                    .weight(weight)
                    .source(key)
                    .selection(self.selection_for(key))
                    .link_handler(self.on_link.clone());
                if quote > 0.0 {
                    el = el.color(theme.colors.text_muted);
                }
                el.into_any()
            }
            BlockKind::CodeBlock => {
                let lines = doc
                    .text(block)
                    .split('\n')
                    .map(|line| {
                        vec![StyledSpan {
                            font_kind: FontKind::Mono,
                            ..StyledSpan::plain(line)
                        }]
                    })
                    .collect();
                code_block(lines)
                    .width(inner_w)
                    .size(fs * 0.92)
                    .source(key)
                    .selection(self.selection_for(key))
                    .into_any()
            }
            BlockKind::Table => self.table(block, inner_w, theme, cx),
            BlockKind::Rule => div()
                .w(inner_w)
                .py(Sp::SM)
                .child(div().w(inner_w).h(1.0).bg(theme.colors.border))
                .into_any(),
        };

        let mut row = content;
        if indent > 0.0 {
            row = div()
                .flex_row()
                .child(div().w(list_w - gutter).flex_shrink_0())
                .child(self.marker(doc.marker(block), gutter, theme))
                .child(row)
                .into_any();
        }
        for _ in 0..doc.quote_depth(block) {
            row = div()
                .flex_row()
                .gap(Sp::MD)
                .child(
                    div()
                        .w(Sz::GUTTER_STRIPE_W)
                        .flex_shrink_0()
                        .bg(theme.colors.border),
                )
                .child(row)
                .into_any();
        }
        row
    }

    /// The list gutter: marker right-aligned against the item text, centered
    /// on the first line.
    fn marker(&self, marker: ListMarker, gutter: f32, theme: &Theme) -> AnyElement {
        let fs = self.font_size;
        let cell = div()
            .w(gutter)
            .h(fs * LINE)
            .flex_shrink_0()
            .flex_row()
            .items_center()
            .justify_end()
            .pr(Sp::SM);
        let muted = theme.colors.text_muted;
        match marker {
            ListMarker::None => cell.into_any(),
            ListMarker::Bullet => cell
                .child(text("\u{2022}").size(fs).color(muted))
                .into_any(),
            ListMarker::Ordered(n) => cell
                .child(text(format!("{n}.")).size(fs).color(muted))
                .into_any(),
            ListMarker::Task(checked) => {
                let side = (fs * 0.85).round();
                let mut check = div()
                    .w(side)
                    .h(side)
                    .rounded(3.0)
                    .border(theme.colors.border);
                if checked {
                    check = check.bg(theme.colors.accent);
                }
                cell.child(check).into_any()
            }
        }
    }

    fn table(
        &self,
        block: usize,
        avail: f32,
        theme: &Theme,
        cx: &mut ElementContext,
    ) -> AnyElement {
        let doc = &self.doc;
        let fs = self.font_size;
        let pad = Sp::SM;
        let columns = doc.table_columns(block).max(1);
        // 1px table border on each side.
        let avail = (avail - 2.0).max(columns as f32);
        let weight = |row: usize| {
            if row == 0 {
                FontWeight::Semibold
            } else {
                FontWeight::Normal
            }
        };

        let cells: Vec<(usize, usize, Vec<StyledSpan>)> = doc
            .cells(block)
            .map(|cell| {
                let (row, col) = doc.cell_position(cell);
                (
                    row,
                    col,
                    self.styled(doc.cell_spans(cell), weight(row), theme),
                )
            })
            .collect();
        let mut natural = vec![2.0 * pad; columns];
        for (row, col, spans) in &cells {
            let style = TextStyle::new(fs)
                .weight(weight(*row))
                .line_height(fs * LINE);
            let width = cx
                .layout_text(&styled_params(spans, style, None))
                .map_or(0.0, |layout| layout.size().0.ceil());
            if let Some(slot) = natural.get_mut(*col) {
                *slot = slot.max(width + 2.0 * pad);
            }
        }
        let widths = fit_columns(&natural, avail);

        let mut table = div()
            .flex_col()
            .border(theme.colors.border)
            .rounded(4.0)
            .overflow_hidden();
        let mut row_el = None;
        let mut current_row = usize::MAX;
        for (i, (row, col, spans)) in cells.into_iter().enumerate() {
            if row != current_row {
                if let Some(done) = row_el.take() {
                    table = table.child(done);
                }
                let mut next = div().flex_row();
                if row == 0 {
                    next = next
                        .bg(theme.colors.element_background)
                        .border_b(theme.colors.border);
                } else if row > 1 {
                    next = next.border_t(theme.colors.border.with_alpha(Alpha::MEDIUM));
                }
                row_el = Some(next);
                current_row = row;
            }
            let width = widths.get(col).copied().unwrap_or(2.0 * pad);
            let key = cell_key(self.key_prefix, block, i);
            let cell = div().w(width).flex_shrink_0().px(pad).py(Sp::XS).child(
                selectable_rich_text(spans)
                    .width((width - 2.0 * pad).max(1.0))
                    .size(fs)
                    .weight(weight(row))
                    .source(key)
                    .selection(self.selection_for(key))
                    .link_handler(self.on_link.clone()),
            );
            row_el = row_el.map(|r| r.child(cell));
        }
        if let Some(done) = row_el {
            table = table.child(done);
        }
        table.into_any()
    }
}

/// Column widths for a table: natural widths when they fit, otherwise
/// water-filling, so narrow columns keep their natural width and the rest
/// share what is left equally (and wrap).
pub(crate) fn fit_columns(natural: &[f32], avail: f32) -> Vec<f32> {
    if natural.iter().sum::<f32>() <= avail {
        return natural.to_vec();
    }
    let mut order: Vec<usize> = (0..natural.len()).collect();
    order.sort_by(|a, b| natural[*a].total_cmp(&natural[*b]));
    let mut widths = vec![0.0; natural.len()];
    let mut remaining = avail;
    for (k, &i) in order.iter().enumerate() {
        let share = remaining / (order.len() - k) as f32;
        widths[i] = natural[i].min(share).floor();
        remaining -= widths[i];
    }
    widths
}

impl Element for MarkdownView {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let mut tree = self.build(cx);
        let id = tree.request_layout(engine, cx);
        self.tree = Some(tree);
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        if let Some(tree) = &mut self.tree {
            tree.prepaint(engine, cx);
        }
    }

    fn paint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        if let Some(tree) = &mut self.tree {
            tree.paint(engine, scene, cx);
        }
    }
}

impl IntoAnyElement for MarkdownView {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
