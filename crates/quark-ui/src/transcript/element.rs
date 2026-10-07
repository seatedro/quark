//! The transcript's element: the materialized rows placed at the positions
//! [`Transcript::prepare`] computed, painted through `SelectableText` and
//! `CodeBlock`, with drag-select, wheel, and accessibility wiring.

use std::ops::Range;
use std::rc::Rc;

use accesskit::Role as AccessibilityRole;
use quark::hit::{CursorHint, HitFlags, HitId};
use quark::{SemanticActions, SemanticNode, SemanticRole};
use quark_render::scene::Rect;
use quark_render::{RoundedRectPrimitive, Scene};

use super::{
    BlockContent, BlockGeometry, LIST_STEP, QUOTE_STEP, Transcript, TranscriptBlock,
    TranscriptRole, TranscriptSource,
};
use crate::accessibility::{AccessibilityAction, AccessibilityNode};
use crate::action::Action;
use crate::design::Alpha;
use crate::element::{
    AnyElement, Bounds, ClickEvent, DragHandler, DragReleaseResult, DragStart, Element,
    ElementContext, IntoAnyElement, LayoutEngine, LayoutId, ScrollActionBuilder, ScrollTarget,
    SelectableText, code_block, div, selectable_rich_text, text,
};
use crate::style::Styled;
use crate::theme::Theme;
use crate::virtual_list::RowKey;

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

struct Placed {
    rect: Rect,
    element: AnyElement,
}

struct RowPaint {
    key: RowKey,
    /// 1-based position among all rows.
    position: usize,
    role: TranscriptRole,
    rect: Rect,
    author: String,
    header: usize,
    blocks: Range<usize>,
}

/// Built by [`Transcript::element`].
pub struct TranscriptElement {
    size: (f32, f32),
    scroll: f32,
    max_scroll: f32,
    total_extent: f32,
    row_count: usize,
    rows: Vec<RowPaint>,
    placed: Vec<Placed>,
    jump: Option<Placed>,
    on_event: EventMap,
    label: String,
    /// A drag is autoscrolling; ask for the next frame.
    animating: bool,
}

impl<G: BlockGeometry> Transcript<G> {
    /// The element for the rows materialized by the last
    /// [`Transcript::prepare`]. `on_event` wraps input into the app's
    /// action type.
    pub fn element(
        &self,
        source: &impl TranscriptSource,
        theme: &Theme,
        on_event: impl Fn(TranscriptEvent) -> Action + 'static,
    ) -> TranscriptElement {
        let style = self.style;
        let (width, height) = self.size;
        let mut placed = Vec::with_capacity(self.rows.len() * 3);
        let mut rows = Vec::with_capacity(self.rows.len());
        for row in &self.rows {
            let author = source
                .message(row.key)
                .map_or_else(String::new, |m| m.author.to_string());
            let header = placed.len();
            placed.push(Placed {
                rect: Rect {
                    x: style.pad_x,
                    y: row.top + style.pad_y,
                    width: (width - style.pad_x * 2.0).max(1.0),
                    height: style.header_height,
                },
                element: text(author.clone())
                    .size(style.font_size * 0.85)
                    .semibold()
                    .into_any(),
            });
            let first = placed.len();
            let blocks = source.message(row.key).map_or(&[][..], |m| &m.blocks[..]);
            for visible in &self.blocks[row.blocks.clone()] {
                let Some(block) = blocks.iter().find(|b| b.key == visible.key) else {
                    continue;
                };
                let selection = self.block_selection(block.key, visible.text_len);
                block_elements(
                    block,
                    visible.rect,
                    style.font_size,
                    selection,
                    theme,
                    &mut placed,
                );
            }
            rows.push(RowPaint {
                key: row.key,
                position: row.index + 1,
                role: row.role,
                rect: Rect {
                    x: 0.0,
                    y: row.top,
                    width,
                    height: row.height,
                },
                author,
                header,
                blocks: first..placed.len(),
            });
        }

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
            row_count: self.list.rows().len(),
            rows,
            placed,
            jump,
            on_event,
            label: "Transcript".to_owned(),
            animating: self.wants_frame(),
        }
    }
}

/// The elements of one block at `rect`: quote bars and the list marker in
/// the inset, then the content to the right of it.
fn block_elements(
    block: &TranscriptBlock,
    rect: Rect,
    base_font_size: f32,
    selection: Option<(usize, usize)>,
    theme: &Theme,
    placed: &mut Vec<Placed>,
) {
    let style = &block.style;
    let font_size = base_font_size * style.scale;
    let inset = style.inset(base_font_size);
    let muted = theme.colors.text_muted;
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
                .bg(theme.colors.border)
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
    let element = match &block.content {
        BlockContent::Prose(spans) => {
            let mut el = selectable_rich_text(spans.to_vec())
                .width(content.width)
                .size(font_size)
                .weight(style.weight)
                .source(block.key.0)
                .selection(selection);
            if style.muted {
                el = el.color(muted);
            }
            el.into_any()
        }
        BlockContent::Code { lines, label } => code_block(lines.to_vec())
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
                .child(div().w(content.width).h(1.0).bg(theme.colors.border));
            if selection.is_some() {
                el = el.bg(theme.colors.accent.with_alpha(Alpha::SOFT));
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
    /// Accessible name of the list.
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    fn placed_mut(&mut self) -> impl Iterator<Item = &mut Placed> {
        self.placed.iter_mut().chain(self.jump.iter_mut())
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
        let mut ids = Vec::with_capacity(self.placed.len() + 1);
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
        node.label = Some(self.label.clone());
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
        cx.push_accessibility_for_semantic(
            AccessibilityNode::new("transcript", AccessibilityRole::List, bounds)
                .label(self.label.clone())
                .set_size(self.row_count)
                .action(AccessibilityAction::Scroll(builder)),
            list,
        );
        cx.push_semantic_parent(list);

        let row_bg = cx.theme.colors.surface;
        let muted = cx.theme.colors.text_muted;
        for row in &self.rows {
            let rect = Rect {
                x: bounds.x + row.rect.x,
                y: bounds.y + row.rect.y,
                ..row.rect
            };
            if row.role == TranscriptRole::User {
                scene.rounded_rect(RoundedRectPrimitive::uniform(
                    rect,
                    0.0,
                    row_bg.with_alpha(Alpha::SOFT),
                ));
            }

            let mut item = SemanticNode::new(rect);
            item.parent = Some(list);
            item.role = Some(SemanticRole::ListItem);
            item.label = Some(row.author.clone());
            let item = cx.semantic.push(item);
            cx.push_accessibility_for_semantic(
                AccessibilityNode::new(
                    format!("transcript.row:{}", row.key.0),
                    AccessibilityRole::ListItem,
                    rect,
                )
                .label(row.author.clone())
                .position_in_set(row.position, self.row_count),
                item,
            );
            cx.push_semantic_parent(item);

            // The author is the item's name; keep the header out of the
            // tree so it is not read twice.
            cx.push_accessibility_text_hidden(true);
            cx.push_text_color(muted);
            self.placed[row.header].element.paint(engine, scene, cx);
            cx.pop_text_color();
            cx.pop_accessibility_text_hidden();

            for placed in &mut self.placed[row.blocks.clone()] {
                placed.element.paint(engine, scene, cx);
            }
            cx.pop_semantic_parent();
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
