//! The terminal element: a scroll container over the whole scrollback
//! whose visible rows are one cache boundary each, keyed by their content,
//! so an unchanged frame replays without building and a scrolled or
//! updated frame rebuilds only rows whose cells changed. The cursor is a
//! boundary of its own beside them, so a blinking cursor never rebuilds
//! the grid. An IME composition is painted over them every frame, outside
//! any cache, where the cursor is.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use accesskit::Role;
use quark::Color;
use quark_render::scene::{BorderPrimitive, Rect, RectPrimitive, RichTextPrimitive, ShapedText};
use quark_render::{FontStyle, FontWeight, Scene};
use quark_text::{TextBlock, TextLayout, TextQuery, TextSpan, TextStyle};
use quark_ui::accessibility::{AccessibilityNode, AccessibleText};
use quark_ui::element::{
    AnyElement, Bounds, CacheKey, ClickEvent, CursorHint, DragHandler, DragReleaseResult,
    DragStart, Element, ElementContext, IntoAnyElement, LayoutEngine, LayoutId, cached, canvas,
    div, inputs_hash,
};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;
use quark_ui::{Action, FocusId};

use crate::grid::{CellStyle, CursorShape, Grid, GridRow, Rgb, Underline};
use crate::state::{Frame, Metrics, Palette, Preedit, TerminalEvent, TerminalState, palette};
use crate::vt::timed;

/// Cursor blink half-period.
const BLINK_MS: u64 = 600;

/// What the view needs from the app besides the state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TerminalEnv {
    /// The terminal has keyboard focus: a solid, blinking cursor, and an
    /// IME composition kept (one built without focus drops it).
    pub focused: bool,
    /// A screen reader is listening: publish the screen's text.
    pub accessible: bool,
}

fn color(c: Rgb) -> Color {
    Color::rgba(c.r, c.g, c.b, 255)
}

/// The terminal in `state` at its viewport size, as of the last
/// [`TerminalState::prepare`]. `on_event` wraps selection input into the
/// app's action type.
///
/// The terminal registers itself as an IME target, so the host turns IME
/// on while it has focus and puts the candidate window at the cursor, or
/// at the composition's caret while one shows. A terminal built without
/// focus drops its composition unsent, so the composition cannot commit
/// later or show beside another element's.
pub fn terminal_view(
    state: &mut TerminalState,
    theme: &Theme,
    env: TerminalEnv,
    on_event: fn(TerminalEvent) -> Action,
) -> AnyElement {
    timed!(View);
    if !env.focused {
        state.cancel_preedit();
    }
    let Some(frame) = state.frame() else {
        return div().into_any();
    };
    let grid = state.shared_grid();
    let preedit = state.preedit().cloned();
    // Built only for a screen reader. Turning one on changes the hash, so
    // the current screen is published without new output.
    let screen = env.accessible.then(|| Screen {
        text: state.screen_text(),
        composition: preedit
            .as_ref()
            .zip(grid.cursor.cell)
            .map(|(p, at)| (p.clone(), at)),
    });
    let (width, height) = frame.viewport;
    let palette = palette(theme);
    let top = state.scroll_top();
    let hash = inputs_hash(&(
        frame.revision,
        screen.as_ref().map(|s| s.text.1),
        screen
            .as_ref()
            .and_then(|s| s.composition.as_ref())
            .map(|c| (c.0.revision, c.1)),
        top.to_bits(),
        state.nonce(),
        palette,
        on_event as usize,
    ));
    let drag = state.drag_start(on_event as usize, || {
        DragStart::new(move |press: ClickEvent| {
            Box::new(SelectDrag { press, on_event }) as Box<dyn DragHandler>
        })
    });
    let (grid_frame, rows_grid) = (frame.clone(), grid.clone());
    let row_text = state.row_text();
    let content = cached(frame.id, hash, move || {
        timed!(
            Build,
            build(
                &grid_frame,
                &rows_grid,
                &row_text,
                screen,
                top,
                palette,
                drag
            )
        )
    })
    .w(width)
    .h(height);
    let m = frame.metrics;
    let mut layer = div().w(width).h(height).relative().child(content);
    // A composition covers the cursor.
    if let Some(at) = grid.cursor.at.filter(|_| preedit.is_none()) {
        let blink = env.focused && grid.cursor.blinking;
        // Everything the cursor paints: its cell's row (the glyph under a
        // block), its style, and the cell size.
        let row_hash = grid.rows.get(at.1 as usize).map_or(0, |r| r.hash);
        let hash = inputs_hash(&(
            grid.cursor,
            row_hash,
            grid.colors,
            [m.font_size, m.cell_w, m.cell_h].map(f32::to_bits),
            env.focused,
            palette,
        ));
        let key = CacheKey(quark::stable_hash(frame.id) ^ 0x6375_7273_6f72);
        let cursor_grid = grid.clone();
        layer = layer.child(
            div()
                .absolute()
                .left(m.pad + f32::from(at.0) * m.cell_w)
                .top(m.pad + f32::from(at.1) * m.cell_h)
                .child(
                    cached(key, hash, move || {
                        cursor(cursor_grid, m, at, palette, env.focused, blink)
                    })
                    .w(m.cell_w * 2.0)
                    .h(m.cell_h),
                ),
        );
    }
    BoundsProbe {
        child: layer.into_any(),
        bounds: state.bounds_cell(),
        inset: (
            m.pad,
            f32::from(grid.cols) * m.cell_w,
            grid.rows.len() as f32 * m.cell_h,
        ),
        ime: Ime {
            focus: frame.focus,
            cell: grid.cursor.cell,
            wide: grid.cursor.wide,
            metrics: m,
            cols: grid.cols,
            viewport: frame.viewport,
            preedit,
            fg: color(grid.colors.foreground),
            bg: color(grid.colors.background),
            caret: grid.cursor.cursor_color(palette),
        },
    }
    .into_any()
}

/// What a screen reader gets: the visible text with the cursor's byte in
/// it, and the composition with the cell it shows at.
struct Screen {
    text: (Arc<str>, usize),
    composition: Option<(Preedit, (u16, u16))>,
}

/// The text blocks each row lays its runs out with, by row id (see
/// [`GridRow::id`]), so a redrawn row lays out into storage it already
/// grew rather than into the shared layout cache, and nothing a row no
/// longer shows stays alive there. A block's layouts are still held for a
/// frame or two after its row redraws (by the scenes and recordings that
/// drew them), so a block with none released yet (a row's first change)
/// borrows released storage from another row's block.
#[derive(Default)]
pub(crate) struct RowText {
    rows: Vec<(u64, Vec<TextBlock>)>,
}

impl RowText {
    /// Drops the blocks of rows `grid` no longer has.
    pub(crate) fn retain(&mut self, grid: &Grid) {
        self.rows
            .retain(|(id, _)| grid.rows.iter().any(|row| row.id == *id));
    }

    /// The index of row `id`'s blocks.
    fn row(&mut self, id: u64) -> usize {
        match self.rows.iter().position(|(row, _)| *row == id) {
            Some(i) => i,
            None => {
                self.rows.push((id, Vec::new()));
                self.rows.len() - 1
            }
        }
    }

    /// Block `index` of row `row`, set to `text` and `spans`. When that
    /// changes them and every layout the block has is still held, it takes
    /// released storage from another block. A block's first text is laid
    /// out into new storage: a row that shows text needs storage of its
    /// own, and borrowing it would only move the allocation to the donor's
    /// next change.
    fn block(
        &mut self,
        row: usize,
        index: usize,
        text: &str,
        spans: &[TextSpan],
    ) -> &mut TextBlock {
        let blocks = &mut self.rows[row].1;
        if index == blocks.len() {
            blocks.push(TextBlock::new());
        }
        let block = &mut blocks[index];
        let changed = block.text() != text || block.spans() != spans;
        if changed && block.revision() > 0 && !block.keeps_spare() {
            let mut block = std::mem::take(block);
            for donor in self.rows.iter_mut().flat_map(|(_, blocks)| blocks) {
                if donor.give_spare(&mut block) {
                    break;
                }
            }
            self.rows[row].1[index] = block;
        }
        let block = &mut self.rows[row].1[index];
        block.set(text, spans);
        block
    }
}

#[allow(clippy::too_many_arguments)]
fn build(
    frame: &Frame,
    grid: &Rc<Grid>,
    row_text: &Rc<std::cell::RefCell<RowText>>,
    screen: Option<Screen>,
    top: f32,
    palette: Palette,
    drag: DragStart,
) -> AnyElement {
    let (width, height) = frame.viewport;
    let m = frame.metrics;
    let grid_w = f32::from(grid.cols) * m.cell_w;
    let grid_h = grid.rows.len() as f32 * m.cell_h;
    let mut rows = div()
        .absolute()
        .left(m.pad)
        // Pinned to the top of the viewport: the terminal, not the scroll
        // container, decides which rows show.
        .top(top + m.pad)
        .w(grid_w)
        .h(grid_h)
        .flex_col()
        .cursor(CursorHint::Text)
        .on_drag_start(drag);
    // Each row is a boundary keyed by its id, unique in the process (so
    // repeated rows and another terminal's rows stay apart), and redrawn
    // when its hash (content and selection) or the drawing settings change.
    // A row that scrolled keeps its id and replays where it now is.
    for (i, row) in grid.rows.iter().enumerate() {
        let key = CacheKey(row.id.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ ROW_KEY_SALT);
        let (grid, row_text) = (grid.clone(), row_text.clone());
        let hash = inputs_hash(&(row.hash, grid.colors, m.cell_w.to_bits(), palette));
        rows = rows.child(
            cached(key, hash, move || {
                canvas(move |bounds, scene, cx| {
                    let row = &grid.rows[i];
                    let mut row_text = row_text.borrow_mut();
                    paint_row(bounds, scene, cx, row, &mut row_text, &m, palette);
                })
                .w(grid_w)
                .h(m.cell_h)
            })
            .w(grid_w)
            .h(m.cell_h),
        );
    }
    if let Some(Screen {
        text: (text, caret),
        composition,
    }) = screen
    {
        let frame = frame.clone();
        let screen = canvas(move |bounds, _scene, cx| {
            let title: &str = if frame.title.is_empty() {
                "Terminal"
            } else {
                &frame.title
            };
            let terminal = cx.push_accessibility(
                AccessibilityNode::new(format!("{}.screen", frame.id), Role::Terminal, bounds)
                    .label(title.to_owned())
                    .read_only(true)
                    .focus(frame.focus)
                    .text(AccessibleText::new(text).caret(caret)),
            );
            // The composition is part of the focused terminal, as marked
            // text on the cursor's row, and no live region: it is read
            // where the user is, not announced as screen output.
            if let Some((preedit, (col, row))) = composition {
                let rect = Rect {
                    x: bounds.x + f32::from(col) * m.cell_w,
                    y: bounds.y + f32::from(row) * m.cell_h,
                    width: (bounds.width - f32::from(col) * m.cell_w).max(m.cell_w),
                    height: m.cell_h,
                };
                cx.push_accessibility_child(
                    AccessibilityNode::new(format!("{}.preedit", frame.id), Role::Mark, rect)
                        .label("Composition")
                        .value(preedit.text)
                        .read_only(true),
                    terminal,
                );
            }
        })
        .w(grid_w)
        .h(grid_h);
        rows = rows.child(div().absolute().left(0.0).top(0.0).child(screen));
    }
    div()
        .w(width)
        .h(height)
        .bg(color(grid.colors.background))
        .track_focus(frame.focus)
        .track_scroll(&frame.scroll)
        .overflow_y_scroll()
        .scrollbar_auto_hide()
        .child(
            div()
                .w(width)
                .h(frame.content_h.max(height))
                .relative()
                .child(rows),
        )
        .into_any()
}

static NO_SPAN_COLORS: LazyLock<Arc<[Color]>> = LazyLock::new(|| Arc::from(Vec::new()));

/// Keeps row cache keys apart from other elements' hashed keys.
const ROW_KEY_SALT: u64 = 0x7465_726d_2e72_6f77;

/// Paints one row: backgrounds, selection, text runs, then decorations.
fn paint_row(
    bounds: Bounds,
    scene: &mut Scene,
    cx: &mut ElementContext,
    row: &GridRow,
    row_text: &mut RowText,
    m: &Metrics,
    palette: Palette,
) {
    let x_of = |col: u16| bounds.x + f32::from(col) * m.cell_w;
    for run in &row.runs {
        if let Some(bg) = run.style.bg {
            scene.rect(RectPrimitive {
                rect: Rect {
                    x: x_of(run.col),
                    y: bounds.y,
                    width: f32::from(run.cols) * m.cell_w,
                    height: m.cell_h,
                },
                color: color(bg),
            });
        }
    }
    if let Some((from, to)) = row.selection {
        scene.rect(RectPrimitive {
            rect: Rect {
                x: x_of(from),
                y: bounds.y,
                width: f32::from(to.saturating_sub(from) + 1) * m.cell_w,
                height: m.cell_h,
            },
            color: palette.selection,
        });
    }
    let at = row_text.row(row.id);
    let mut drawn = 0;
    for run in &row.runs {
        let text = row.run_text(run);
        if text.trim().is_empty() {
            continue;
        }
        let (style, italic) = text_style(m, run.style, text.len());
        let block = row_text.block(at, drawn, text, italic.as_slice());
        drawn += 1;
        let Ok(layout) = block.layout(cx.text, style, None, cx.scale_factor) else {
            continue;
        };
        draw_layout(
            scene,
            m,
            layout,
            run.style,
            x_of(run.col),
            bounds.y,
            run.cols,
            None,
        );
    }
    for run in &row.runs {
        decorate(
            scene,
            m,
            run.style,
            x_of(run.col),
            bounds.y,
            run.cols,
            palette,
        );
    }
}

/// The text style of a run in `style`, and its italic span over `len`
/// bytes when italic.
fn text_style(m: &Metrics, style: CellStyle, len: usize) -> (TextStyle, Option<TextSpan>) {
    let mut text_style = m.text_style();
    if style.bold {
        text_style = text_style.weight(FontWeight::Bold);
    }
    let italic = style.italic.then_some(TextSpan {
        range: 0..len,
        weight: None,
        style: Some(FontStyle::Italic),
        kind: None,
    });
    (text_style, italic)
}

/// Draws `layout`, text in `style`, at `(x, y)`, `cols` cells wide. `fg`
/// overrides the style's color (the glyph under a block cursor).
#[allow(clippy::too_many_arguments)]
fn draw_layout(
    scene: &mut Scene,
    m: &Metrics,
    layout: Arc<TextLayout>,
    style: CellStyle,
    x: f32,
    y: f32,
    cols: u16,
    fg: Option<Color>,
) {
    let mut default_color = fg.unwrap_or(color(style.fg));
    if style.faint && fg.is_none() {
        default_color = default_color.with_alpha(140);
    }
    scene.rich_text(RichTextPrimitive {
        rect: Rect {
            x,
            y,
            // Italic and wide glyphs ink past their cells.
            width: f32::from(cols) * m.cell_w + m.font_size,
            height: m.cell_h,
        },
        layout: ShapedText::new(layout),
        default_color,
        span_colors: NO_SPAN_COLORS.clone(),
    });
}

/// Underlines, strikethrough, overline, and the link underline.
fn decorate(
    scene: &mut Scene,
    m: &Metrics,
    style: CellStyle,
    x: f32,
    y: f32,
    cols: u16,
    palette: Palette,
) {
    let width = f32::from(cols) * m.cell_w;
    let thick = (m.font_size / 13.0).round().max(1.0);
    let fg = color(style.fg);
    let mut line = |y: f32, from: f32, w: f32, c: Color| {
        scene.rect(RectPrimitive {
            rect: Rect {
                x: from,
                y,
                width: w,
                height: thick,
            },
            color: c,
        });
    };
    let base = y + m.cell_h - thick * 2.0;
    let under = style.underline_color.map_or(fg, color);
    match style.underline {
        Underline::None if style.hyperlink => line(base, x, width, palette.link.with_alpha(160)),
        Underline::None => {}
        Underline::Single => line(base, x, width, under),
        Underline::Double => {
            line(base, x, width, under);
            line(base - thick * 2.0, x, width, under);
        }
        Underline::Dotted | Underline::Dashed | Underline::Curly => {
            let (on, step) = match style.underline {
                Underline::Dotted => (thick, thick * 2.0),
                Underline::Dashed => (thick * 3.0, thick * 5.0),
                _ => (thick * 2.0, thick * 2.0),
            };
            let mut at = x;
            let mut up = false;
            while at < x + width {
                // A curly underline alternates between two heights.
                let dy = if style.underline == Underline::Curly && up {
                    -thick
                } else {
                    0.0
                };
                line(base + dy, at, on.min(x + width - at), under);
                at += step;
                up = !up;
            }
        }
    }
    if style.strikethrough {
        line(y + (m.cell_h / 2.0).round(), x, width, fg);
    }
    if style.overline {
        line(y, x, width, fg);
    }
}

/// The cursor at cell `at`, over the grid.
fn cursor(
    grid: Rc<Grid>,
    m: Metrics,
    at: (u16, u16),
    palette: Palette,
    focused: bool,
    blink: bool,
) -> AnyElement {
    canvas(move |bounds, scene, cx| {
        if blink {
            let phase = cx.clock_ms / BLINK_MS;
            cx.request_frame_at_ms((phase + 1) * BLINK_MS);
            if phase % 2 == 1 {
                return;
            }
        }
        paint_cursor(bounds, scene, cx, &grid, at, &m, palette, focused);
    })
    .w(m.cell_w * 2.0)
    .h(m.cell_h)
    .into_any()
}

#[allow(clippy::too_many_arguments)]
fn paint_cursor(
    bounds: Bounds,
    scene: &mut Scene,
    cx: &mut ElementContext,
    grid: &Grid,
    (col, row): (u16, u16),
    m: &Metrics,
    palette: Palette,
    focused: bool,
) {
    let c = grid.cursor;
    let cursor_color = c.cursor_color(palette);
    let cells = if c.wide { 2.0 } else { 1.0 };
    let rect = Rect {
        x: bounds.x,
        y: bounds.y,
        width: m.cell_w * cells,
        height: m.cell_h,
    };
    let bar = (m.font_size / 7.0).round().max(1.0);
    let shape = if focused {
        c.shape
    } else {
        CursorShape::BlockHollow
    };
    match shape {
        CursorShape::Block => {
            scene.rect(RectPrimitive {
                rect,
                color: cursor_color,
            });
            // The glyph under it, in the background color.
            if let Some(line) = grid.rows.get(row as usize)
                && let Some(run) = line.run_at(col)
            {
                let text = line.run_text(run);
                let ascii = text.len() as u16 == run.cols;
                let (text, cols) = if ascii {
                    let i = (col - run.col) as usize;
                    (&text[i..i + 1], 1)
                } else {
                    (text, run.cols)
                };
                let (style, italic) = text_style(m, run.style, text.len());
                let query = TextQuery {
                    spans: italic.as_slice(),
                    ..TextQuery::new(text, style)
                };
                if !text.trim().is_empty()
                    && let Some(layout) = cx.layout_text_query(&query)
                {
                    let bg = color(grid.colors.background);
                    draw_layout(scene, m, layout, run.style, rect.x, rect.y, cols, Some(bg));
                }
            }
        }
        CursorShape::BlockHollow => {
            scene.border(BorderPrimitive::uniform(
                rect,
                bar.min(2.0),
                0.0,
                cursor_color,
            ));
        }
        CursorShape::Bar => scene.rect(RectPrimitive {
            rect: Rect { width: bar, ..rect },
            color: cursor_color,
        }),
        CursorShape::Underline => scene.rect(RectPrimitive {
            rect: Rect {
                y: rect.y + rect.height - bar,
                height: bar,
                ..rect
            },
            color: cursor_color,
        }),
    }
}

impl crate::grid::Cursor {
    fn cursor_color(&self, palette: Palette) -> Color {
        self.color.map_or(palette.cursor, color)
    }
}

/// A drag that selects cells. The press is reported at press time so the
/// anchor lands on what was under the pointer.
struct SelectDrag {
    press: ClickEvent,
    on_event: fn(TerminalEvent) -> Action,
}

impl DragHandler for SelectDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.on_event)(TerminalEvent::Press {
            x: self.press.x,
            y: self.press.y,
        })]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        vec![(self.on_event)(TerminalEvent::Drag { x, y })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.on_event)(TerminalEvent::Release)],
        }
    }

    fn on_cancel(&mut self) -> Vec<Action> {
        vec![(self.on_event)(TerminalEvent::Cancel)]
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Text
    }
}

/// Records the grid's window bounds every frame, replayed or not, so
/// pointer input maps onto cells: the child's bounds inset by the padding,
/// at the grid's size.
struct BoundsProbe {
    child: AnyElement,
    bounds: Rc<Cell<Rect>>,
    /// Padding, grid width, grid height.
    inset: (f32, f32, f32),
    ime: Ime,
}

/// The terminal as an IME target: where its cursor is and the composition
/// to paint there.
struct Ime {
    focus: FocusId,
    /// The cursor's cell, shown or not.
    cell: Option<(u16, u16)>,
    wide: bool,
    metrics: Metrics,
    cols: u16,
    viewport: (f32, f32),
    preedit: Option<Preedit>,
    fg: Color,
    bg: Color,
    caret: Color,
}

impl Ime {
    /// Paints the composition over the cursor's cell, clipped to the
    /// viewport, and returns the caret the candidate window should follow
    /// (the cursor's cell when nothing is composing). `bounds` is the
    /// terminal's, padding included.
    fn paint(&self, bounds: Bounds, scene: &mut Scene, cx: &mut ElementContext) -> Option<Rect> {
        let m = &self.metrics;
        let (col, row) = self.cell?;
        let grid_left = bounds.x + m.pad;
        let at_x = grid_left + f32::from(col) * m.cell_w;
        let y = bounds.y + m.pad + f32::from(row) * m.cell_h;
        let cursor = Rect {
            x: at_x,
            y,
            width: m.cell_w * if self.wide { 2.0 } else { 1.0 },
            height: m.cell_h,
        };
        let Some(preedit) = &self.preedit else {
            return Some(cursor);
        };
        let Some(layout) = cx.layout_text_query(&TextQuery::new(&preedit.text, m.text_style()))
        else {
            return Some(cursor);
        };
        let width = layout.size().0;
        // A composition too long for the rest of the row shifts left to
        // stay on the grid.
        let grid_right = grid_left + f32::from(self.cols) * m.cell_w;
        let x = at_x.min(grid_right - width).max(grid_left);
        let viewport = Rect {
            x: bounds.x,
            y: bounds.y,
            width: self.viewport.0,
            height: self.viewport.1,
        };
        scene.clip(viewport);
        cx.push_paint_clip(viewport);
        scene.rect(RectPrimitive {
            rect: Rect {
                x,
                y,
                width,
                height: m.cell_h,
            },
            color: self.bg,
        });
        scene.rich_text(RichTextPrimitive {
            rect: Rect {
                x,
                y,
                width,
                height: m.cell_h,
            },
            layout: ShapedText::new(layout.clone()),
            default_color: self.fg,
            span_colors: NO_SPAN_COLORS.clone(),
        });
        // A thin underline under the composition and a thick one under
        // the clause the IME is converting, as text fields draw them.
        let thin = (m.font_size / 13.0).round().max(1.0);
        let mut line = |x: f32, width: f32, height: f32| {
            scene.rect(RectPrimitive {
                rect: Rect {
                    x,
                    y: y + m.cell_h - height,
                    width,
                    height,
                },
                color: self.fg,
            });
        };
        line(x, width, thin);
        let clause = preedit.selection.clone().filter(|s| !s.is_empty());
        for r in clause.into_iter().flat_map(|s| layout.selection_rects(s)) {
            line(x + r.x, r.width, thin * 2.0);
        }
        let bar = (m.font_size / 7.0).round().max(1.0);
        let caret = Rect {
            x: x + preedit
                .selection
                .as_ref()
                .map_or(0.0, |s| layout.caret(s.end).x),
            y,
            width: bar,
            height: m.cell_h,
        };
        if preedit.selection.is_some() {
            scene.rect(RectPrimitive {
                rect: caret,
                color: self.caret,
            });
        }
        cx.pop_paint_clip();
        scene.pop_clip();
        Some(caret)
    }
}

impl Element for BoundsProbe {
    type LayoutState = ();
    type PrepaintState = ();

    fn request_layout(
        &mut self,
        engine: &mut LayoutEngine,
        cx: &mut ElementContext,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(engine, cx), ())
    }

    fn prepaint(
        &mut self,
        bounds: Bounds,
        _layout: &mut (),
        engine: &LayoutEngine,
        cx: &mut ElementContext,
    ) {
        let (pad, width, height) = self.inset;
        self.bounds.set(Rect {
            x: bounds.x + pad,
            y: bounds.y + pad,
            width,
            height,
        });
        self.child.prepaint(engine, cx);
    }

    fn paint(
        &mut self,
        bounds: Bounds,
        _layout: &mut (),
        _prepaint: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        self.child.paint(engine, scene, cx);
        let caret = self.ime.paint(bounds, scene, cx);
        cx.register_ime_target(self.ime.focus, caret);
    }
}

impl IntoAnyElement for BoundsProbe {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
