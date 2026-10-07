//! The transcript's element: the materialized rows placed at the positions
//! [`Transcript::prepare`] computed, painted through `SelectableText` and
//! `CodeBlock`, with drag-select, wheel, and accessibility wiring.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use accesskit::Role as AccessibilityRole;
use quark::hit::{CursorHint, HitFlags, HitId};
use quark::{SemanticActions, SemanticNode, SemanticRole};
use quark_render::scene::Rect;
use quark_render::{RoundedRectPrimitive, Scene};

use quark::selection::BlockKey;

use super::{
    BlockContent, BlockGeometry, LIST_STEP, Palette, QUOTE_STEP, Transcript, TranscriptBlock,
    TranscriptMessage, TranscriptRole, TranscriptSource, VisibleRow,
};
use crate::accessibility::{AccessibilityAction, AccessibilityNode};
use crate::action::Action;
use crate::design::Alpha;
use crate::element::{
    AnyElement, Bounds, CacheKey, ClickEvent, DragHandler, DragReleaseResult, DragStart, Element,
    ElementContext, IntoAnyElement, LayoutEngine, LayoutId, LinkClicked, LinkHandler,
    ScrollActionBuilder, ScrollTarget, SelectableText, StyledSpan, cached, code_block_joined, div,
    inputs_hash, selectable_rich_text, text,
};
use crate::style::Styled;
use crate::theme::Theme;
use crate::virtual_list::RowKey;
use quark::Color;

thread_local! {
    /// The list node's key, and the last static label it was given, shared
    /// so the per-frame list node allocates nothing.
    static LIST_KEY: Arc<str> = Arc::from("transcript");
    static LIST_LABEL: RefCell<(&'static str, Arc<str>)> = RefCell::new(("", Arc::from("")));
}

fn list_key() -> Arc<str> {
    LIST_KEY.with(Arc::clone)
}

fn list_label(label: &'static str) -> Arc<str> {
    LIST_LABEL.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.0 != label {
            *slot = (label, Arc::from(label));
        }
        slot.1.clone()
    })
}

/// Input from the transcript element, in coordinates relative to its top
/// left. Pass each to [`Transcript::handle`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TranscriptEvent {
    PointerDown { x: f32, y: f32 },
    PointerDrag { x: f32, y: f32 },
    PointerUp,
    Wheel(i32),
    JumpToLatest,
}

/// Frame interval requested while a drag autoscrolls.
const AUTOSCROLL_FRAME_MS: u64 = 16;

type EventMap = Rc<dyn Fn(TranscriptEvent) -> Action>;

/// The link action of a built element. Its blocks are built before
/// [`TranscriptElement::on_link`] can be called, so they share this slot.
type LinkSlot = Rc<RefCell<Option<Rc<dyn Fn(&Arc<str>) -> Action>>>>;

/// A block's spans with their tones resolved, kept while the block stays
/// materialized and the theme stays the same.
#[derive(Debug, Clone)]
pub(super) struct PaintedSpans {
    source: Arc<[StyledSpan]>,
    palette: Palette,
    spans: Arc<[StyledSpan]>,
}

/// `block`'s spans for painting with `palette`, from `cache` when they were
/// resolved from the same spans and palette.
fn painted_spans(
    cache: &mut HashMap<BlockKey, PaintedSpans>,
    kept: &mut HashMap<BlockKey, PaintedSpans>,
    block: &TranscriptBlock,
    palette: &Palette,
) -> Option<Arc<[StyledSpan]>> {
    let source = block.content.spans()?;
    if block.tones().is_none() {
        return Some(source.clone());
    }
    let entry = match cache.remove(&block.key) {
        Some(entry) if Arc::ptr_eq(&entry.source, source) && entry.palette == *palette => entry,
        _ => PaintedSpans {
            source: source.clone(),
            palette: *palette,
            spans: block.painted_spans(palette)?,
        },
    };
    let spans = entry.spans.clone();
    kept.insert(block.key, entry);
    Some(spans)
}

struct Placed {
    rect: Rect,
    element: AnyElement,
}

/// A row's cached subtree's inputs: what [`RowElement`] is built from, by
/// the hash of everything in it.
#[derive(Clone)]
pub(super) struct RowEntry {
    hash: u64,
    build: Arc<RowBuild>,
}

impl std::fmt::Debug for RowEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RowEntry")
            .field("hash", &self.hash)
            .finish()
    }
}

/// Everything one row's subtree reads, in coordinates relative to the
/// row's top left, so a row that only scrolls replays.
struct RowBuild {
    key: RowKey,
    /// 1-based position among all rows, and the row count.
    position: usize,
    row_count: usize,
    role: TranscriptRole,
    size: (f32, f32),
    author: Arc<str>,
    header: Rect,
    font_size: f32,
    colors: RowColors,
    blocks: Vec<BlockBuild>,
    /// Find highlights: rectangles relative to the row, and whether each is
    /// the current match.
    highlights: Vec<(Rect, bool)>,
}

struct BlockBuild {
    block: TranscriptBlock,
    spans: Option<Arc<[StyledSpan]>>,
    rect: Rect,
    selection: Option<(usize, usize)>,
}

/// The theme colors a row paints with besides its spans' own.
#[derive(Clone, Copy, PartialEq)]
struct RowColors {
    row_background: Color,
    muted: Color,
    border: Color,
    selection: Color,
    highlight: Color,
    current_highlight: Color,
}

impl RowColors {
    fn new(theme: &Theme) -> Self {
        let c = &theme.colors;
        Self {
            row_background: c.surface.with_alpha(Alpha::SOFT),
            muted: c.text_muted,
            border: c.border,
            selection: c.accent.with_alpha(Alpha::SOFT),
            highlight: c.search_match_bg,
            current_highlight: c.search_match_active_bg,
        }
    }

    fn hash_into(&self, hasher: &mut impl Hasher) {
        for color in [
            self.row_background,
            self.muted,
            self.border,
            self.selection,
            self.highlight,
            self.current_highlight,
        ] {
            (color.r, color.g, color.b, color.a).hash(hasher);
        }
    }
}

/// Built by [`Transcript::element`].
pub struct TranscriptElement {
    size: (f32, f32),
    scroll: f32,
    max_scroll: f32,
    total_extent: f32,
    row_count: usize,
    /// One cached row subtree per materialized row.
    rows: Vec<Placed>,
    jump: Option<Placed>,
    on_event: EventMap,
    on_link: LinkSlot,
    label: Cow<'static, str>,
    /// A drag is autoscrolling; ask for the next frame.
    animating: bool,
}

/// Cache keys of transcript rows, apart from other cached boundaries.
fn row_cache_key(row: RowKey) -> CacheKey {
    CacheKey(inputs_hash(&("quark.transcript.row", row.0)))
}

impl<G: BlockGeometry> Transcript<G> {
    /// The element for the rows materialized by the last
    /// [`Transcript::prepare`]. `on_event` wraps input into the app's
    /// action type. Theme colors are resolved here, so a theme change shows
    /// on the next element without rebuilding any block.
    ///
    /// Each row is a [`cached`](crate::element::cached) subtree keyed by its
    /// row key and hashed over its blocks' revisions, its selection, its
    /// find highlights, its size, and the theme, so a frame that changes
    /// nothing replays every row and a streaming frame rebuilds only the
    /// rows that changed.
    pub fn element(
        &mut self,
        source: &impl TranscriptSource,
        theme: &Theme,
        on_event: impl Fn(TranscriptEvent) -> Action + 'static,
    ) -> TranscriptElement {
        let style = self.style;
        let (width, height) = self.size;
        let palette = Palette::new(theme);
        let colors = RowColors::new(theme);
        let theme_hash = {
            let mut hasher = DefaultHasher::new();
            (theme.mode == crate::theme::ThemeMode::Dark).hash(&mut hasher);
            palette.hash_into(&mut hasher);
            colors.hash_into(&mut hasher);
            hasher.finish()
        };
        let on_link: LinkSlot = Rc::default();
        let link_slot = on_link.clone();
        let links = LinkHandler::new(move |url| match &*link_slot.borrow() {
            Some(f) => f(url),
            None => LinkClicked { url: url.clone() }.into(),
        });

        let mut painted = std::mem::take(&mut self.painted);
        let mut kept = std::mem::take(&mut self.painted_spare);
        let mut builds = std::mem::take(&mut self.row_builds);
        let mut kept_builds = std::mem::take(&mut self.row_builds_spare);
        let row_count = self.list.rows().len();
        let mut placed = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let message = source.message(row.key);
            let blocks = message.map_or(&[][..], |m| &m.blocks[..]);
            let hash = self.row_hash(row, message, blocks, theme_hash, row_count);
            let build = match builds.remove(&row.key) {
                Some(entry) if entry.hash == hash => entry.build,
                _ => Arc::new(self.row_build(
                    row,
                    message,
                    blocks,
                    (&palette, colors),
                    (&mut painted, &mut kept),
                    row_count,
                )),
            };
            // The resolved spans of a row that did not change stay cached
            // for when it does.
            for visible in &self.blocks[row.blocks.clone()] {
                if let Some(entry) = painted.remove(&visible.key) {
                    kept.insert(visible.key, entry);
                }
            }
            let links = links.clone();
            let subtree = build.clone();
            placed.push(Placed {
                rect: Rect {
                    x: 0.0,
                    y: row.top,
                    width,
                    height: row.height,
                },
                element: cached(row_cache_key(row.key), hash, move || {
                    RowElement::new(subtree, &links)
                })
                .into_any(),
            });
            kept_builds.insert(row.key, RowEntry { hash, build });
        }
        painted.clear();
        self.painted = kept;
        self.painted_spare = painted;
        builds.clear();
        self.row_builds = kept_builds;
        self.row_builds_spare = builds;

        let on_event: EventMap = Rc::new(on_event);
        let jump = self.unseen.then(|| {
            let (w, h) = (style.font_size * 10.0, style.font_size * 2.4);
            let action = on_event(TranscriptEvent::JumpToLatest);
            Placed {
                rect: Rect {
                    x: ((width - w) * 0.5).max(0.0),
                    y: (height - h - style.font_size).max(0.0),
                    width: w,
                    height: h,
                },
                element: div()
                    .w(w)
                    .h(h)
                    .rounded(h * 0.5)
                    .items_center()
                    .justify_center()
                    .bg(theme.colors.accent)
                    .hover_bg(theme.colors.accent_strong)
                    .accessibility_id("transcript.jump-to-latest")
                    .accessibility_role(AccessibilityRole::Button)
                    .accessibility_label("Jump to latest")
                    .on_click(action)
                    .child(
                        text("Jump to latest")
                            .size(style.font_size * 0.9)
                            .semibold()
                            .color(theme.colors.text_strong),
                    )
                    .into_any(),
            }
        });

        TranscriptElement {
            size: self.size,
            scroll: self.list.scroll_offset(),
            max_scroll: self.list.max_scroll_offset(),
            total_extent: self.list.rows().total_extent(),
            row_count,
            rows: placed,
            jump,
            on_event,
            on_link,
            label: Cow::Borrowed("Transcript"),
            animating: self.wants_frame(),
        }
    }

    /// Hash of everything [`Self::row_build`] reads for `row`.
    fn row_hash(
        &self,
        row: &VisibleRow,
        message: Option<&TranscriptMessage>,
        blocks: &[TranscriptBlock],
        theme_hash: u64,
        row_count: usize,
    ) -> u64 {
        let mut hasher = DefaultHasher::new();
        (row.index, row_count, row.role, theme_hash).hash(&mut hasher);
        (self.size.0.to_bits(), row.height.to_bits()).hash(&mut hasher);
        self.style.font_size.to_bits().hash(&mut hasher);
        message.map(|m| &*m.author).hash(&mut hasher);
        for visible in &self.blocks[row.blocks.clone()] {
            let Some(block) = blocks.get(visible.index).filter(|b| b.key == visible.key) else {
                continue;
            };
            (block.revision(), visible.offset_in_row.to_bits()).hash(&mut hasher);
            (visible.rect.height.to_bits(), visible.rect.width.to_bits()).hash(&mut hasher);
            self.block_selection(block.key, visible.text_len)
                .hash(&mut hasher);
            if let Some(find) = &self.find {
                find.hash_block(block.key, &mut hasher);
            }
        }
        hasher.finish()
    }

    /// The inputs of `row`'s subtree.
    fn row_build(
        &self,
        row: &VisibleRow,
        message: Option<&TranscriptMessage>,
        blocks: &[TranscriptBlock],
        (palette, colors): (&Palette, RowColors),
        (painted, kept): (
            &mut HashMap<BlockKey, PaintedSpans>,
            &mut HashMap<BlockKey, PaintedSpans>,
        ),
        row_count: usize,
    ) -> RowBuild {
        let style = &self.style;
        let width = self.size.0;
        let mut built = Vec::with_capacity(row.blocks.len());
        let mut highlights = Vec::new();
        let mut rects = Vec::new();
        for visible in &self.blocks[row.blocks.clone()] {
            let Some(block) = blocks.get(visible.index).filter(|b| b.key == visible.key) else {
                continue;
            };
            let rect = Rect {
                y: visible.offset_in_row,
                ..visible.rect
            };
            if let Some(find) = &self.find {
                for (range, current) in find.block_matches(block.key) {
                    rects.clear();
                    visible.geometry.range_rects(range, &mut rects);
                    highlights.extend(rects.iter().map(|r| (r.offset(rect.x, rect.y), current)));
                }
            }
            built.push(BlockBuild {
                block: block.clone(),
                spans: painted_spans(painted, kept, block, palette),
                rect,
                selection: self.block_selection(block.key, visible.text_len),
            });
        }
        RowBuild {
            key: row.key,
            position: row.index + 1,
            row_count,
            role: row.role,
            size: (width, row.height),
            author: message.map_or_else(|| Arc::from(""), |m| m.author.clone()),
            header: Rect {
                x: style.pad_x,
                y: style.pad_y,
                width: (width - style.pad_x * 2.0).max(1.0),
                height: style.header_height,
            },
            font_size: style.font_size,
            colors,
            blocks: built,
            highlights,
        }
    }
}

/// One row: its background, find highlights, header, and blocks, with the
/// row's list item node. Built inside the row's cached boundary, so it
/// paints relative to the row's top left.
struct RowElement {
    build: Arc<RowBuild>,
    header: AnyElement,
    children: Vec<Placed>,
}

impl RowElement {
    fn new(build: Arc<RowBuild>, links: &LinkHandler) -> Self {
        let header = text(build.author.to_string())
            .size(build.font_size * 0.85)
            .semibold()
            .into_any();
        let mut children = Vec::with_capacity(build.blocks.len() * 2);
        for b in &build.blocks {
            block_elements(
                &b.block,
                b.spans.clone(),
                BlockPaint {
                    rect: b.rect,
                    base_font_size: build.font_size,
                    selection: b.selection,
                    links,
                    colors: &build.colors,
                },
                &mut children,
            );
        }
        Self {
            build,
            header,
            children,
        }
    }
}

impl Element for RowElement {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let mut ids = Vec::with_capacity(self.children.len() + 1);
        let header = self.header.request_layout(engine, cx);
        ids.push(engine.request_layout(absolute(self.build.header), &[header]));
        for placed in &mut self.children {
            let child = placed.element.request_layout(engine, cx);
            ids.push(engine.request_layout(absolute(placed.rect), &[child]));
        }
        let (width, height) = self.build.size;
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(width),
                    height: taffy::Dimension::length(height),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &ids,
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        self.header.prepaint(engine, cx);
        for placed in &mut self.children {
            placed.element.prepaint(engine, cx);
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        let build = &*self.build;
        if build.role == TranscriptRole::User {
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                bounds,
                0.0,
                build.colors.row_background,
            ));
        }
        for &(rect, current) in &build.highlights {
            let color = if current {
                build.colors.current_highlight
            } else {
                build.colors.highlight
            };
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                rect.offset(bounds.x, bounds.y),
                2.0,
                color,
            ));
        }

        let mut item = SemanticNode::new(bounds);
        item.parent = cx.current_semantic_parent();
        item.role = Some(SemanticRole::ListItem);
        // The accessibility node carries the row's name. A semantic label
        // is a `String` that every replay of the row would clone.
        let item = cx.semantic.push(item);
        cx.push_accessibility_for_semantic(
            AccessibilityNode::new(
                format!("transcript.row:{}", build.key.0),
                AccessibilityRole::ListItem,
                bounds,
            )
            .label(build.author.clone())
            .position_in_set(build.position, build.row_count),
            item,
        );
        cx.push_semantic_parent(item);

        // The author is the item's name; keep the header out of the tree so
        // it is not read twice.
        cx.push_accessibility_text_hidden(true);
        cx.push_text_color(build.colors.muted);
        self.header.paint(engine, scene, cx);
        cx.pop_text_color();
        cx.pop_accessibility_text_hidden();

        for placed in &mut self.children {
            placed.element.paint(engine, scene, cx);
        }
        cx.pop_semantic_parent();
    }
}

impl IntoAnyElement for RowElement {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

/// Where and how one block is painted.
struct BlockPaint<'a> {
    rect: Rect,
    base_font_size: f32,
    selection: Option<(usize, usize)>,
    links: &'a LinkHandler,
    colors: &'a RowColors,
}

/// The elements of one block at `rect`: quote bars and the list marker in
/// the inset, then the content to the right of it. `spans` are the content
/// spans with theme colors resolved.
fn block_elements(
    block: &TranscriptBlock,
    spans: Option<Arc<[StyledSpan]>>,
    paint: BlockPaint,
    placed: &mut Vec<Placed>,
) {
    let BlockPaint {
        rect,
        base_font_size,
        selection,
        links,
        colors,
    } = paint;
    let style = &block.style;
    let font_size = base_font_size * style.scale;
    let inset = style.inset(base_font_size);
    let muted = colors.muted;
    for level in 0..style.quote_depth {
        let bar = Rect {
            x: rect.x + level as f32 * QUOTE_STEP * base_font_size,
            width: 3.0,
            ..rect
        };
        placed.push(Placed {
            rect: bar,
            element: div()
                .w(bar.width)
                .h(bar.height)
                .bg(colors.border)
                .into_any(),
        });
    }
    if let Some(marker) = &style.marker {
        let gutter = LIST_STEP * base_font_size;
        let line = SelectableText::line_height_for(font_size);
        let cell = Rect {
            x: rect.x + inset - gutter,
            y: rect.y,
            width: gutter,
            height: line,
        };
        placed.push(Placed {
            rect: cell,
            element: div()
                .w(cell.width)
                .h(cell.height)
                .flex_row()
                .items_center()
                .justify_end()
                .pr((base_font_size * 0.5).round())
                .child(text(marker.to_string()).size(font_size).color(muted))
                .into_any(),
        });
    }

    let content = Rect {
        x: rect.x + inset,
        width: (rect.width - inset).max(1.0),
        ..rect
    };
    let spans = spans.unwrap_or_else(|| Arc::from([]));
    let element = match &block.content {
        BlockContent::Prose(_) => {
            let mut el = selectable_rich_text(spans)
                .width(content.width)
                .size(font_size)
                .weight(style.weight)
                .source(block.key.0)
                .selection(selection)
                .link_handler(links.clone());
            if style.muted {
                el = el.color(muted);
            }
            el.into_any()
        }
        BlockContent::Code {
            line_count, label, ..
        } => code_block_joined(spans, *line_count)
            .label(label.clone())
            .width(content.width)
            .size(font_size)
            .source(block.key.0)
            .selection(selection)
            .into_any(),
        BlockContent::Rule => {
            let mut el = div()
                .w(content.width)
                .h(content.height)
                .flex_col()
                .justify_center()
                .child(div().w(content.width).h(1.0).bg(colors.border));
            if selection.is_some() {
                el = el.bg(colors.selection);
            }
            el.into_any()
        }
    };
    placed.push(Placed {
        rect: content,
        element,
    });
}

impl TranscriptElement {
    /// Action a link click in any block emits, given its URL. Defaults to
    /// [`LinkClicked`].
    pub fn on_link(self, f: impl Fn(&Arc<str>) -> Action + 'static) -> Self {
        *self.on_link.borrow_mut() = Some(Rc::new(f));
        self
    }

    /// Accessible name of the list.
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = label.into();
        self
    }

    fn placed_mut(&mut self) -> impl Iterator<Item = &mut Placed> {
        self.rows.iter_mut().chain(self.jump.iter_mut())
    }

    fn paint_scroll_thumb(&self, bounds: Bounds, scene: &mut Scene, cx: &ElementContext) {
        if self.total_extent <= bounds.height || self.max_scroll <= 0.0 {
            return;
        }
        let width = 4.0 * cx.scale_factor.max(1.0);
        let thumb = (bounds.height / self.total_extent * bounds.height)
            .max(24.0)
            .min(bounds.height);
        let y = (self.scroll / self.max_scroll).clamp(0.0, 1.0) * (bounds.height - thumb);
        scene.rounded_rect(RoundedRectPrimitive::uniform(
            Rect {
                x: bounds.x + bounds.width - width * 2.0,
                y: bounds.y + y,
                width,
                height: thumb,
            },
            width * 0.5,
            cx.theme.colors.scrollbar_thumb,
        ));
    }
}

fn absolute(rect: Rect) -> taffy::Style {
    taffy::Style {
        position: taffy::Position::Absolute,
        inset: taffy::Rect {
            left: taffy::LengthPercentageAuto::length(rect.x),
            top: taffy::LengthPercentageAuto::length(rect.y),
            right: taffy::LengthPercentageAuto::auto(),
            bottom: taffy::LengthPercentageAuto::auto(),
        },
        size: taffy::Size {
            width: taffy::Dimension::length(rect.width),
            height: taffy::Dimension::length(rect.height),
        },
        ..Default::default()
    }
}

impl Element for TranscriptElement {
    type LayoutState = ();
    type PrepaintState = HitId;

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let mut ids = Vec::with_capacity(self.rows.len() + 1);
        for placed in self.placed_mut() {
            let child = placed.element.request_layout(engine, cx);
            ids.push(engine.request_layout(absolute(placed.rect), &[child]));
        }
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(self.size.0),
                    height: taffy::Dimension::length(self.size.1),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &ids,
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) -> HitId {
        let hit = cx.insert_hit(bounds, HitFlags::DRAG | HitFlags::SCROLL, CursorHint::Text);
        cx.push_clip(bounds);
        for placed in self.placed_mut() {
            placed.element.prepaint(engine, cx);
        }
        cx.pop_clip();
        hit
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        hit: &mut HitId,
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        scene.clip(bounds);
        if self.animating {
            cx.request_frame_at_ms(cx.clock_ms + AUTOSCROLL_FRAME_MS);
        }

        // The list: one hit entry and semantic node owning drag and wheel.
        let mut node = SemanticNode::new(bounds);
        node.parent = cx.current_semantic_parent();
        node.role = Some(SemanticRole::ScrollArea);
        node.label = Some(self.label.to_string());
        node.actions = SemanticActions::default().scrollable().draggable();
        let list = cx.semantic.push(node);
        cx.bind_hit(*hit, list);

        let on_event = self.on_event.clone();
        let scroll_events = on_event.clone();
        let builder =
            ScrollActionBuilder::new(move |lines| scroll_events(TranscriptEvent::Wheel(lines)));
        cx.handlers.on_scroll(
            list,
            ScrollTarget {
                builder: builder.clone(),
                offset: self.scroll,
                max: Some(self.max_scroll),
            },
        );
        let origin = (bounds.x, bounds.y);
        cx.handlers.on_drag(
            list,
            DragStart::new(move |event| {
                Box::new(SelectDrag {
                    origin,
                    press: event,
                    on_event: on_event.clone(),
                })
            }),
        );
        let label: Arc<str> = match &self.label {
            Cow::Borrowed(label) => list_label(label),
            Cow::Owned(label) => Arc::from(label.as_str()),
        };
        cx.push_accessibility_for_semantic(
            AccessibilityNode::shared(list_key(), AccessibilityRole::List, bounds)
                .label(label)
                .set_size(self.row_count)
                .action(AccessibilityAction::Scroll(builder)),
            list,
        );
        cx.push_semantic_parent(list);

        for row in &mut self.rows {
            row.element.paint(engine, scene, cx);
        }

        self.paint_scroll_thumb(bounds, scene, cx);
        if let Some(jump) = &mut self.jump {
            jump.element.paint(engine, scene, cx);
        }

        cx.pop_semantic_parent();
        scene.pop_clip();
    }
}

impl IntoAnyElement for TranscriptElement {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

/// A drag-select gesture. The press is reported at press time so the
/// anchor lands on the text under the pointer before content streams in.
struct SelectDrag {
    origin: (f32, f32),
    press: ClickEvent,
    on_event: EventMap,
}

impl SelectDrag {
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        (x - self.origin.0, y - self.origin.1)
    }
}

impl DragHandler for SelectDrag {
    fn on_press(&mut self) -> Vec<Action> {
        let (x, y) = self.local(self.press.x, self.press.y);
        vec![(self.on_event)(TranscriptEvent::PointerDown { x, y })]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        let (x, y) = self.local(x, y);
        vec![(self.on_event)(TranscriptEvent::PointerDrag { x, y })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.on_event)(TranscriptEvent::PointerUp)],
        }
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Text
    }
}
