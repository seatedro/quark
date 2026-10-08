//! The document's element: the materialized rows placed at the positions
//! [`Document::prepare`] computed, painted through `SelectableText` and
//! `CodeBlock`, with drag-select, wheel, and accessibility wiring.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use accesskit::Role as AccessibilityRole;
use quark::hit::{CursorHint, HitFlags, HitId};
use quark::{SemanticActions, SemanticNode, SemanticRole, Transform2D};
use quark_render::scene::Rect;
use quark_render::{ImagePrimitive, RoundedRectPrimitive, Scene};

use quark::selection::BlockKey;

use super::measure::{failed_image_spans, image_extent};
use super::{
    AdornmentAccessibility, AdornmentCx, Block, BlockContent, BlockGeometry, Decorator, Document,
    DocumentSource, ImageState, LIST_STEP, Palette, QUOTE_STEP, RowAdornment, RowChrome,
    VisibleRow,
};
use crate::accessibility::{AccessibilityAction, AccessibilityNode};
use crate::action::Action;
use crate::design::Alpha;
use crate::element::{
    AnyElement, Bounds, CacheKey, ClickEvent, ClickHandler, CodeBlock, CodeHeader, DragHandler,
    DragReleaseResult, DragStart, Element, ElementContext, IntoAnyElement, LayoutEngine, LayoutId,
    LinkClicked, LinkHandler, ScrollActionBuilder, ScrollAxes, ScrollHandle, ScrollSink,
    ScrollTarget, ScrollbarInput, ScrollbarVisibility, Scrollbars, SelectableText, StyledSpan,
    cached, code_block_joined, div, inputs_hash, selectable_rich_text, svg_icon, text,
};
use crate::icons::lucide;
use crate::style::Styled;
use crate::theme::Theme;
use crate::virtual_list::RowKey;
use quark::Color;

thread_local! {
    /// The list node's key, and the last static label it was given, shared
    /// so the per-frame list node allocates nothing.
    static LIST_KEY: Arc<str> = Arc::from("document");
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

/// Input from the document element, in coordinates relative to its top
/// left. Pass each to [`Document::handle`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DocumentEvent {
    PointerDown {
        x: f32,
        y: f32,
    },
    PointerDrag {
        x: f32,
        y: f32,
    },
    PointerUp,
    Wheel(i32),
    /// The scrollbar moved the view to this offset (`f32::MAX` for the
    /// end); landing at the bottom pins the view there.
    ScrollTo(f32),
    /// A code block's wrap toggle was pressed.
    SetCodeWrap {
        block: BlockKey,
        wrap: bool,
    },
}

/// A code block's Copy button was pressed. `text` is the block's whole
/// source, including lines and columns scrolled out of view; write it to
/// the clipboard. Emitted as an action of this type unless
/// [`DocumentElement::on_copy_code`] maps it.
#[derive(Debug, Clone, PartialEq)]
pub struct CopyCode {
    pub block: BlockKey,
    pub text: Arc<str>,
}

impl From<CopyCode> for Action {
    fn from(copy: CopyCode) -> Self {
        Action::new(copy)
    }
}

/// Frame interval requested while a drag autoscrolls.
const AUTOSCROLL_FRAME_MS: u64 = 16;

type EventMap = Rc<dyn Fn(DocumentEvent) -> Action>;

/// The link action of a built element. Its blocks are built before
/// [`DocumentElement::on_link`] can be called, so they share this slot.
type LinkSlot = Rc<RefCell<Option<Rc<dyn Fn(&Arc<str>) -> Action>>>>;

/// The copy-code action of a built element, shared like [`LinkSlot`].
type CopySlot = Rc<RefCell<Option<Rc<dyn Fn(CopyCode) -> Action>>>>;

/// What a code block's toolbar buttons emit through.
#[derive(Clone)]
struct CodeControls {
    on_event: EventMap,
    on_copy: CopySlot,
}

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
    block: &Block,
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
    build: Rc<RowBuild>,
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
    size: (f32, f32),
    chrome: RowChrome,
    decorator: Option<Decorator>,
    header: Rect,
    font_size: f32,
    colors: RowColors,
    blocks: Vec<BlockBuild>,
    /// Adornments with their bands relative to the row, in flow order.
    adornments: Vec<(RowAdornment, Rect)>,
    /// Find highlights: rectangles relative to the row, and whether each is
    /// the current match.
    highlights: Vec<(Rect, bool)>,
}

struct BlockBuild {
    block: Block,
    spans: Option<Arc<[StyledSpan]>>,
    rect: Rect,
    selection: Option<(usize, usize)>,
    /// The horizontal scroll and unwrapped width of content wider than
    /// the column.
    scroll: Option<(ScrollHandle, f32)>,
}

/// The theme colors a row paints with besides its spans' own.
#[derive(Clone, Copy, PartialEq)]
struct RowColors {
    muted: Color,
    border: Color,
    selection: Color,
    highlight: Color,
    current_highlight: Color,
    placeholder: Color,
}

impl RowColors {
    fn new(theme: &Theme) -> Self {
        let c = &theme.colors;
        Self {
            muted: c.text_muted,
            border: c.border,
            selection: c.accent.with_alpha(Alpha::SOFT),
            highlight: c.search_match_bg,
            current_highlight: c.search_match_active_bg,
            placeholder: c.element_background,
        }
    }

    fn hash_into(&self, hasher: &mut impl Hasher) {
        for color in [
            self.muted,
            self.border,
            self.selection,
            self.highlight,
            self.current_highlight,
            self.placeholder,
        ] {
            (color.r, color.g, color.b, color.a).hash(hasher);
        }
    }
}

/// Built by [`Document::element`].
pub struct DocumentElement {
    size: (f32, f32),
    scroll: f32,
    max_scroll: f32,
    total_extent: f32,
    row_count: usize,
    /// One cached row subtree per materialized row.
    rows: Vec<Placed>,
    on_event: EventMap,
    on_link: LinkSlot,
    on_copy: CopySlot,
    label: Cow<'static, str>,
    /// A drag is autoscrolling; ask for the next frame.
    animating: bool,
    scrollbar_auto_hide: bool,
    /// The document's, so the linger survives rebuilding the element.
    scrollbar_visibility: ScrollbarVisibility,
}

/// Cache keys of document rows, apart from other cached boundaries.
fn row_cache_key(row: RowKey) -> CacheKey {
    CacheKey(inputs_hash(&("quark.document.row", row.0)))
}

impl<G: BlockGeometry> Document<G> {
    /// The element for the rows materialized by the last
    /// [`Document::prepare`]. `on_event` wraps input into the app's
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
        source: &impl DocumentSource,
        theme: &Theme,
        on_event: impl Fn(DocumentEvent) -> Action + 'static,
    ) -> DocumentElement {
        self.elements_built += 1;
        let width = self.size.0;
        let palette = Palette::new(theme);
        let colors = RowColors::new(theme);
        let theme_hash = {
            let mut hasher = DefaultHasher::new();
            (theme.mode == crate::theme::ThemeMode::Dark).hash(&mut hasher);
            palette.hash_into(&mut hasher);
            colors.hash_into(&mut hasher);
            hasher.finish()
        };
        let on_event: EventMap = Rc::new(on_event);
        let controls = CodeControls {
            on_event: on_event.clone(),
            on_copy: Rc::default(),
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
            let content = source.row(row.key);
            let blocks = content.map_or(&[][..], |r| &r.blocks[..]);
            let adornments = content.map_or(&[][..], |r| &r.adornments[..]);
            let chrome = content.map(|r| &r.chrome);
            let hash = self.row_hash(row, chrome, (blocks, adornments), theme_hash, row_count);
            let build = match builds.remove(&row.key) {
                Some(entry) if entry.hash == hash => entry.build,
                _ => Rc::new(self.row_build(
                    row,
                    chrome,
                    (blocks, adornments),
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
            let controls = controls.clone();
            let subtree = build.clone();
            placed.push(Placed {
                rect: Rect {
                    x: 0.0,
                    y: row.top,
                    width,
                    height: row.height,
                },
                element: cached(row_cache_key(row.key), hash, move || {
                    RowElement::new(subtree, &links, &controls)
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

        DocumentElement {
            size: self.size,
            scroll: self.list.scroll_offset(),
            max_scroll: self.list.max_scroll_offset(),
            total_extent: self.list.rows().total_extent(),
            row_count,
            rows: placed,
            on_event,
            on_link,
            on_copy: controls.on_copy,
            label: Cow::Borrowed("Document"),
            animating: self.wants_frame(),
            scrollbar_auto_hide: false,
            scrollbar_visibility: self.scrollbar.clone(),
        }
    }

    /// Hash of everything [`Self::row_build`] reads for `row`.
    fn row_hash(
        &self,
        row: &VisibleRow,
        chrome: Option<&RowChrome>,
        (blocks, adornments): (&[Block], &[RowAdornment]),
        theme_hash: u64,
        row_count: usize,
    ) -> u64 {
        let mut hasher = DefaultHasher::new();
        (row.index, row_count, theme_hash).hash(&mut hasher);
        (self.size.0.to_bits(), row.height.to_bits()).hash(&mut hasher);
        self.style.font_size.to_bits().hash(&mut hasher);
        chrome.hash(&mut hasher);
        // Another decorator draws other chrome.
        self.decorator
            .as_ref()
            .map(|d| Rc::as_ptr(&d.0).cast::<()>())
            .hash(&mut hasher);
        for visible in &self.blocks[row.blocks.clone()] {
            let Some(block) = blocks.get(visible.index).filter(|b| b.key == visible.key) else {
                continue;
            };
            (block.revision(), visible.offset_in_row.to_bits()).hash(&mut hasher);
            if let BlockContent::Code { .. } = block.content {
                (self.code_toolbar, self.is_code_wrapped(block.key)).hash(&mut hasher);
            }
            self.scroll_x(block.key).to_bits().hash(&mut hasher);
            if self.scroll_unsettled(block.key) {
                // The offset resolves while the row paints; a replay would
                // skip that.
                self.elements_built.hash(&mut hasher);
            }
            (visible.rect.height.to_bits(), visible.rect.width.to_bits()).hash(&mut hasher);
            self.block_selection(block.key, visible.text_len)
                .hash(&mut hasher);
            if let Some(find) = &self.find {
                find.hash_block(block.key, &mut hasher);
            }
        }
        for visible in self.visible_row_adornments(row) {
            if let Some(adornment) = adornments
                .get(visible.index)
                .filter(|a| a.key == visible.key)
            {
                (adornment, visible.offset_in_row.to_bits()).hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    fn visible_row_adornments(&self, row: &VisibleRow) -> &[super::VisibleAdornment] {
        &self.adornments[row.adornments.clone()]
    }

    /// The inputs of `row`'s subtree.
    fn row_build(
        &self,
        row: &VisibleRow,
        chrome: Option<&RowChrome>,
        (blocks, adornments): (&[Block], &[RowAdornment]),
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
            let scroll = self
                .scroll_handles
                .get(&block.key)
                .and_then(|handle| Some((handle.clone(), visible.geometry.natural_width()?)));
            let scroll_x = self.scroll_x(block.key);
            if let Some(find) = &self.find {
                for (range, current) in find.block_matches(block.key) {
                    rects.clear();
                    visible.geometry.range_rects(range, &mut rects);
                    // Matches scrolled out of a wide block's column hide.
                    highlights.extend(rects.iter().filter_map(|r| {
                        let r = r.offset(rect.x - scroll_x, rect.y);
                        let left = r.x.max(rect.x);
                        let right = (r.x + r.width).min(rect.x + rect.width);
                        (left < right).then_some((
                            Rect {
                                x: left,
                                width: right - left,
                                ..r
                            },
                            current,
                        ))
                    }));
                }
            }
            built.push(BlockBuild {
                block: self.code_presentation().present(block).into_owned(),
                spans: painted_spans(painted, kept, block, palette),
                rect,
                selection: self.block_selection(block.key, visible.text_len),
                scroll,
            });
        }
        RowBuild {
            key: row.key,
            position: row.index + 1,
            row_count,
            size: (width, row.height),
            header: Rect {
                x: style.pad_x,
                y: style.pad_y,
                width: (width - style.pad_x * 2.0).max(1.0),
                height: chrome.map_or(0.0, |c| c.header_height),
            },
            chrome: chrome.cloned().unwrap_or_default(),
            decorator: self.decorator.clone(),
            font_size: style.font_size,
            colors,
            blocks: built,
            adornments: self
                .visible_row_adornments(row)
                .iter()
                .filter_map(|visible| {
                    let adornment = adornments.get(visible.index)?;
                    let rect = Rect {
                        y: visible.offset_in_row,
                        ..visible.rect
                    };
                    (adornment.key == visible.key).then(|| (adornment.clone(), rect))
                })
                .collect(),
            highlights,
        }
    }
}

/// One row: its chrome background, find highlights, chrome header,
/// blocks, and adornments, with the row's list item node. Built inside the
/// row's cached boundary, so it paints relative to the row's top left.
struct RowElement {
    build: Rc<RowBuild>,
    /// The decorator's header, built at layout, where the theme is known.
    header: Option<AnyElement>,
    /// Block elements and adornment slots in flow order, so assistive tech
    /// reads them as they appear.
    children: Vec<RowChild>,
}

enum RowChild {
    Placed(Placed),
    /// The adornment at this index of the build, and its element once
    /// built at layout.
    Adornment(usize, Option<AnyElement>),
}

impl RowElement {
    fn new(build: Rc<RowBuild>, links: &LinkHandler, controls: &CodeControls) -> Self {
        let mut children = Vec::with_capacity(build.blocks.len() * 2 + build.adornments.len());
        let mut placed = Vec::new();
        let mut adornments = build.adornments.iter().enumerate().peekable();
        for b in &build.blocks {
            while let Some((i, _)) = adornments.next_if(|(_, (_, rect))| rect.y <= b.rect.y) {
                children.push(RowChild::Adornment(i, None));
            }
            block_elements(
                &b.block,
                b.spans.clone(),
                BlockPaint {
                    rect: b.rect,
                    base_font_size: build.font_size,
                    selection: b.selection,
                    links,
                    controls,
                    colors: &build.colors,
                    scroll: b.scroll.as_ref(),
                },
                &mut placed,
            );
            children.extend(placed.drain(..).map(RowChild::Placed));
        }
        children.extend(adornments.map(|(i, _)| RowChild::Adornment(i, None)));
        Self {
            build,
            header: None,
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
        let build = &*self.build;
        if build.header.height > 0.0 {
            self.header = build
                .decorator
                .as_ref()
                .and_then(|d| d.0.header(&build.chrome, build.header.width, cx.theme));
        }
        if let Some(header) = &mut self.header {
            let header = header.request_layout(engine, cx);
            ids.push(engine.request_layout(absolute(build.header), &[header]));
        }
        for child in &mut self.children {
            let (rect, element) = match child {
                RowChild::Placed(placed) => (placed.rect, &mut placed.element),
                RowChild::Adornment(i, element) => {
                    let (adornment, rect) = &build.adornments[*i];
                    let element = element.get_or_insert_with(|| {
                        adornment.build(&AdornmentCx {
                            row: build.key,
                            key: adornment.key,
                            width: rect.width,
                            height: rect.height,
                            theme: cx.theme,
                        })
                    });
                    (*rect, element)
                }
            };
            let id = element.request_layout(engine, cx);
            ids.push(engine.request_layout(absolute(rect), &[id]));
        }
        let (width, height) = build.size;
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
        if let Some(header) = &mut self.header {
            header.prepaint(engine, cx);
        }
        for child in &mut self.children {
            match child {
                RowChild::Placed(placed) => placed.element.prepaint(engine, cx),
                RowChild::Adornment(_, Some(element)) => element.prepaint(engine, cx),
                RowChild::Adornment(_, None) => {}
            }
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
        let background = build
            .decorator
            .as_ref()
            .and_then(|d| d.0.background(&build.chrome, cx.theme));
        if let Some(color) = background {
            scene.rounded_rect(RoundedRectPrimitive::uniform(bounds, 0.0, color));
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
        let mut node = AccessibilityNode::new(
            format!("document.row:{}", build.key.0),
            AccessibilityRole::ListItem,
            bounds,
        )
        .position_in_set(build.position, build.row_count);
        if let Some(label) = &build.chrome.label {
            node = node.label(label.clone());
        }
        cx.push_accessibility_for_semantic(node, item);
        cx.push_semantic_parent(item);

        if let Some(header) = &mut self.header {
            // A labelled row is named by its label; keep the header's text
            // out of the tree so it is not read twice. Controls with their
            // own labels stay.
            cx.push_accessibility_text_hidden(build.chrome.label.is_some());
            cx.push_text_color(build.colors.muted);
            header.paint(engine, scene, cx);
            cx.pop_text_color();
            cx.pop_accessibility_text_hidden();
        }

        for child in &mut self.children {
            match child {
                RowChild::Placed(placed) => placed.element.paint(engine, scene, cx),
                RowChild::Adornment(i, Some(element)) => {
                    let policy = build.adornments[*i].0.accessibility;
                    cx.push_accessibility_text_hidden(
                        policy == AdornmentAccessibility::ControlsOnly,
                    );
                    element.paint(engine, scene, cx);
                    cx.pop_accessibility_text_hidden();
                }
                RowChild::Adornment(_, None) => {}
            }
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
    controls: &'a CodeControls,
    colors: &'a RowColors,
    scroll: Option<&'a (ScrollHandle, f32)>,
}

/// The elements of one block at `rect`: quote bars and the list marker in
/// the inset, then the content to the right of it. `spans` are the content
/// spans with theme colors resolved.
fn block_elements(
    block: &Block,
    spans: Option<Arc<[StyledSpan]>>,
    paint: BlockPaint,
    placed: &mut Vec<Placed>,
) {
    let BlockPaint {
        rect,
        base_font_size,
        selection,
        links,
        controls,
        colors,
        scroll,
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
    let mut overlay = None;
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
            line_count,
            label,
            toolbar,
            wrap,
            ..
        } => {
            if *toolbar {
                // Over the panel the code paints, and outside its sideways
                // scroll, so the buttons stay put.
                let height = CodeHeader::Toolbar.height(font_size);
                let rect = Rect { height, ..content };
                let toolbar = CodeToolbar {
                    block,
                    label: label.as_deref(),
                    wrap: *wrap,
                    font_size,
                    colors,
                    controls,
                };
                overlay = Some(Placed {
                    rect,
                    element: toolbar.element(rect),
                });
            }
            let code = |width: f32| {
                code_block_joined(spans.clone(), *line_count)
                    .label(label.clone())
                    .toolbar(*toolbar)
                    .wrap(*wrap)
                    .width(width)
                    .size(font_size)
                    .source(block.key.0)
                    .selection(selection)
            };
            match scroll {
                // Wider than the column: the block scrolls sideways under
                // its own scrollbar.
                // A tab stop, so arrow keys can bring hidden columns in.
                Some((handle, natural)) => div()
                    .id(format!("document.code:{}:lines", block.key.0).as_str())
                    .tab_stop(quark::focus::TabStop::new(0))
                    .w(content.width)
                    .h(content.height)
                    .track_scroll(handle)
                    .overflow_x_scroll()
                    .scrollbar_auto_hide()
                    .child(
                        div()
                            .w(natural.max(content.width))
                            .flex_shrink_0()
                            .child(code(natural.max(content.width))),
                    )
                    .into_any(),
                None => code(content.width).into_any(),
            }
        }
        BlockContent::Image {
            state: ImageState::Failed,
            ..
        } => selectable_rich_text(failed_image_spans(&block.text))
            .width(content.width)
            .size(font_size)
            .weight(style.weight)
            .source(block.key.0)
            .selection(selection)
            .color(muted)
            .into_any(),
        BlockContent::Image { state, .. } => {
            let (width, height) = match state.size() {
                Some(size) => image_extent(size, content.width),
                None => (content.width, content.height),
            };
            ImageBox {
                key: block.key,
                state: state.clone(),
                alt: block.text.clone(),
                size: (width, height),
                font_size,
                colors: *colors,
                selected: selection.is_some(),
            }
            .into_any()
        }
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
    placed.extend(overlay);
}

/// A code block's toolbar: its label, then Copy and the wrap toggle at
/// the right end.
struct CodeToolbar<'a> {
    block: &'a Block,
    label: Option<&'a str>,
    wrap: bool,
    font_size: f32,
    colors: &'a RowColors,
    controls: &'a CodeControls,
}

impl CodeToolbar<'_> {
    fn element(&self, rect: Rect) -> AnyElement {
        let key = self.block.key;
        let pad = CodeBlock::header_metrics(self.font_size, 1, CodeHeader::Toolbar)
            .text_origin
            .0;
        let icon = (self.font_size * 1.15).round();
        let button = |id: &str, svg: &'static str, name: &'static str| {
            div()
                .id(format!("document.code:{}:{id}", key.0).as_str())
                .w(rect.height)
                .h(rect.height)
                .flex_row()
                .items_center()
                .justify_center()
                .rounded(4.0)
                .hover_bg(self.colors.placeholder)
                .accessibility_role(AccessibilityRole::Button)
                .accessibility_label(name)
                .tooltip(name)
                .child(svg_icon(svg, icon).color(self.colors.muted))
        };
        let copy = {
            let text = self.block.text.clone();
            let slot = self.controls.on_copy.clone();
            button("copy", lucide::COPY, "Copy code").on_click_handler(ClickHandler::new(
                move |_| {
                    let copy = CopyCode {
                        block: key,
                        text: text.clone(),
                    };
                    vec![match &*slot.borrow() {
                        Some(f) => f(copy),
                        None => copy.into(),
                    }]
                },
            ))
        };
        let wrap = {
            let (on_event, wrap) = (self.controls.on_event.clone(), !self.wrap);
            button("wrap", lucide::WRAP_TEXT, "Wrap lines")
                .accessibility_toggled(self.wrap)
                .on_click_handler(ClickHandler::new(move |_| {
                    vec![on_event(DocumentEvent::SetCodeWrap { block: key, wrap })]
                }))
        };
        let label = self.label.unwrap_or("");
        div()
            .w(rect.width)
            .h(rect.height)
            .flex_row()
            .items_center()
            .justify_between()
            .pl(pad)
            .child(
                text(label.to_owned())
                    .size(CodeHeader::label_size(self.font_size))
                    .color(self.colors.muted),
            )
            .child(div().flex_row().child(copy).child(wrap))
            .into_any()
    }
}

/// An image block's content: the pixels scaled to `size`, a placeholder
/// while they load, or the alt text when they failed. Assistive tech reads
/// the alt text as the image's name.
struct ImageBox {
    key: BlockKey,
    state: ImageState,
    alt: Arc<str>,
    size: (f32, f32),
    font_size: f32,
    colors: RowColors,
    selected: bool,
}

impl Element for ImageBox {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        _cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let id = engine.request_layout(
            taffy::Style {
                size: taffy::Size {
                    width: taffy::Dimension::length(self.size.0),
                    height: taffy::Dimension::length(self.size.1),
                },
                flex_shrink: 0.0,
                ..Default::default()
            },
            &[],
        );
        (id, ())
    }

    fn prepaint(
        &mut self,
        _bounds: Bounds,
        _layout_state: &mut (),
        _engine: &LayoutEngine,
        _cx: &mut ElementContext,
    ) {
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        _prepaint_state: &mut (),
        _engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        match &self.state {
            ImageState::Ready(image) => scene.image(ImagePrimitive {
                rect: bounds,
                width: image.width,
                height: image.height,
                rgba: image.rgba.clone(),
                cache_key: image.cache_key,
            }),
            ImageState::Pending { .. } => scene.rounded_rect(RoundedRectPrimitive::uniform(
                bounds,
                self.font_size * 0.4,
                self.colors.placeholder,
            )),
            // Failed images are painted as their alt text instead.
            ImageState::Failed => {}
        }
        if self.selected {
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                bounds,
                0.0,
                self.colors.selection,
            ));
        }
        if cx.accessibility_enabled() && !self.alt.is_empty() {
            cx.push_accessibility(
                AccessibilityNode::new(
                    format!("document.image:{}", self.key.0),
                    AccessibilityRole::Image,
                    bounds,
                )
                .label(self.alt.clone()),
            );
        }
    }
}

impl IntoAnyElement for ImageBox {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

impl DocumentElement {
    /// Action a link click in any block emits, given its URL. Defaults to
    /// [`LinkClicked`].
    pub fn on_link(self, f: impl Fn(&Arc<str>) -> Action + 'static) -> Self {
        *self.on_link.borrow_mut() = Some(Rc::new(f));
        self
    }

    /// Action a code block's Copy button emits. Defaults to the
    /// [`CopyCode`] itself.
    pub fn on_copy_code(self, f: impl Fn(CopyCode) -> Action + 'static) -> Self {
        *self.on_copy.borrow_mut() = Some(Rc::new(f));
        self
    }

    /// Accessible name of the list.
    pub fn label(mut self, label: impl Into<Cow<'static, str>>) -> Self {
        self.label = label.into();
        self
    }

    /// Show the list's scrollbar only while the pointer is over the list,
    /// its thumb is held, or for
    /// [`SCROLLBAR_LINGER_MS`](crate::element::SCROLLBAR_LINGER_MS) after
    /// the list scrolls.
    pub fn scrollbar_auto_hide(mut self) -> Self {
        self.scrollbar_auto_hide = true;
        self
    }

    /// Input from the wheel and the scrollbar.
    fn scroll_builder(&self) -> ScrollActionBuilder {
        let lines = self.on_event.clone();
        let to = self.on_event.clone();
        ScrollActionBuilder::new(move |n| lines(DocumentEvent::Wheel(n)))
            .with_to_px(move |px| to(DocumentEvent::ScrollTo(px as f32)))
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

/// The document's prepaint state: the list's hit entry and its scrollbar.
pub struct DocumentPrepaint {
    hit: HitId,
    builder: ScrollActionBuilder,
    scrollbars: Scrollbars,
}

impl Element for DocumentElement {
    type LayoutState = ();
    type PrepaintState = DocumentPrepaint;

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        let mut ids = Vec::with_capacity(self.rows.len() + 1);
        for placed in &mut self.rows {
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
    ) -> DocumentPrepaint {
        let hit = cx.insert_hit(bounds, HitFlags::DRAG | HitFlags::SCROLL, CursorHint::Text);
        cx.push_clip(bounds);
        for placed in &mut self.rows {
            placed.element.prepaint(engine, cx);
        }
        // The list owns its offset (through the app), so the bar's input
        // becomes document events like the wheel's.
        let builder = self.scroll_builder();
        let scrollbars = Scrollbars::prepaint(
            ScrollbarInput {
                bounds,
                content: (bounds.width, self.total_extent),
                offset: (0.0, self.scroll),
                axes: ScrollAxes { x: false, y: true },
                sinks: [None, Some(ScrollSink::Builder(builder.clone()))],
                auto_hide: self.scrollbar_auto_hide,
                visibility: Some(self.scrollbar_visibility.clone()),
                focused: false,
            },
            cx,
        );
        cx.pop_clip();
        DocumentPrepaint {
            hit,
            builder,
            scrollbars,
        }
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout_state: &mut (),
        prepaint: &mut DocumentPrepaint,
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
        node.label = Some(Arc::from(&*self.label));
        node.actions = SemanticActions::default().scrollable().draggable();
        let list = cx.semantic.push(node);
        cx.bind_hit(prepaint.hit, list);

        let on_event = self.on_event.clone();
        let builder = prepaint.builder.clone();
        cx.handlers.on_scroll(
            list,
            ScrollTarget {
                builder: builder.clone(),
                offset: self.scroll,
                max: Some(self.max_scroll),
            },
        );
        let origin = (bounds.x, bounds.y);
        // Under a transform that flattens the list nothing can be pressed,
        // so it registers no drag.
        if let Some(to_layout) = cx.current_transform().invert() {
            cx.handlers.on_drag(
                list,
                DragStart::new(move |event| {
                    Box::new(SelectDrag {
                        to_layout,
                        origin,
                        press: event,
                        on_event: on_event.clone(),
                    })
                }),
            );
        }
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
        prepaint.scrollbars.register(list, cx);

        for row in &mut self.rows {
            row.element.paint(engine, scene, cx);
        }

        prepaint.scrollbars.paint(scene, cx);

        cx.pop_semantic_parent();
        scene.pop_clip();
    }
}

impl IntoAnyElement for DocumentElement {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}

/// A drag-select gesture. The press is reported at press time so the
/// anchor lands on the text under the pointer before content streams in.
struct SelectDrag {
    /// Window coordinates to the list's layout coordinates, through the
    /// transforms of its ancestors.
    to_layout: Transform2D,
    /// The list's top left in layout coordinates.
    origin: (f32, f32),
    press: ClickEvent,
    on_event: EventMap,
}

impl SelectDrag {
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let (x, y) = self.to_layout.apply(x, y);
        (x - self.origin.0, y - self.origin.1)
    }
}

impl DragHandler for SelectDrag {
    fn on_press(&mut self) -> Vec<Action> {
        let (x, y) = self.local(self.press.x, self.press.y);
        vec![(self.on_event)(DocumentEvent::PointerDown { x, y })]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        let (x, y) = self.local(x, y);
        vec![(self.on_event)(DocumentEvent::PointerDrag { x, y })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.on_event)(DocumentEvent::PointerUp)],
        }
    }

    /// The selection made so far stays; the release only ends the drag.
    fn on_cancel(&mut self) -> Vec<Action> {
        self.on_release().actions
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Text
    }
}
