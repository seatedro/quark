//! The terminal element: a scroll container over the whole scrollback
//! whose visible rows are one cache boundary each, keyed by their content,
//! so an unchanged frame replays without building and a scrolled or
//! updated frame rebuilds only rows whose cells changed. The cursor is a
//! boundary of its own beside them, so a blinking cursor never rebuilds
//! the grid.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use accesskit::Role;
use quark::Color;
use quark_render::scene::{BorderPrimitive, Rect, RectPrimitive, RichTextPrimitive, ShapedText};
use quark_render::{FontStyle, FontWeight, Scene};
use quark_text::{TextQuery, TextSpan};
use quark_ui::Action;
use quark_ui::accessibility::{AccessibilityNode, AccessibleText};
use quark_ui::element::{
    AnyElement, Bounds, CacheKey, ClickEvent, CursorHint, DragHandler, DragReleaseResult, Element,
    ElementContext, IntoAnyElement, LayoutEngine, LayoutId, cached, canvas, div, inputs_hash,
};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

use crate::grid::{CellStyle, CursorShape, Grid, GridRow, Rgb, Underline};
use crate::state::{Frame, Metrics, Palette, TerminalEvent, TerminalState, palette};

/// Cursor blink half-period.
const BLINK_MS: u64 = 600;

/// What the view needs from the app besides the state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct TerminalEnv {
    /// The terminal has keyboard focus: a solid, blinking cursor.
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
pub fn terminal_view(
    state: &mut TerminalState,
    theme: &Theme,
    env: TerminalEnv,
    on_event: fn(TerminalEvent) -> Action,
) -> AnyElement {
    let Some(frame) = state.frame() else {
        return div().into_any();
    };
    let grid = state.shared_grid();
    // Built only for a screen reader. Turning one on changes the hash, so
    // the current screen is published without new output.
    let screen = env.accessible.then(|| state.screen_text());
    let (width, height) = frame.viewport;
    let palette = palette(theme);
    let top = state.scroll_top();
    let hash = inputs_hash(&(
        frame.revision,
        screen.as_ref().map(|s| s.1),
        top.to_bits(),
        state.nonce(),
        palette,
        on_event as usize,
    ));
    let (grid_frame, rows_grid) = (frame.clone(), grid.clone());
    let content = cached(frame.id, hash, move || {
        build(&grid_frame, &rows_grid, screen, top, palette, on_event)
    })
    .w(width)
    .h(height);
    let m = frame.metrics;
    let mut layer = div().w(width).h(height).relative().child(content);
    if let Some(at) = grid.cursor.at {
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
    }
    .into_any()
}

fn build(
    frame: &Frame,
    grid: &Rc<Grid>,
    screen: Option<(Arc<str>, usize)>,
    top: f32,
    palette: Palette,
    on_event: fn(TerminalEvent) -> Action,
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
        .on_drag(move |press: ClickEvent| {
            Box::new(SelectDrag { press, on_event }) as Box<dyn DragHandler>
        });
    for (i, row) in grid.rows.iter().enumerate() {
        // Rows that repeat (blank lines) share a hash; the occurrence keeps
        // their cache keys apart.
        let nth = grid.rows[..i].iter().filter(|r| r.hash == row.hash).count() as u64;
        let key = CacheKey(row.hash ^ (nth + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let grid = grid.clone();
        let hash = inputs_hash(&(row.hash, grid.colors, m.cell_w.to_bits(), palette));
        rows = rows.child(
            cached(key, hash, move || {
                canvas(move |bounds, scene, cx| {
                    paint_row(bounds, scene, cx, &grid.rows[i], &m, palette)
                })
                .w(grid_w)
                .h(m.cell_h)
            })
            .w(grid_w)
            .h(m.cell_h),
        );
    }
    if let Some((text, caret)) = screen {
        let frame = frame.clone();
        let screen = canvas(move |bounds, _scene, cx| {
            let title: &str = if frame.title.is_empty() {
                "Terminal"
            } else {
                &frame.title
            };
            cx.push_accessibility(
                AccessibilityNode::new(format!("{}.screen", frame.id), Role::Terminal, bounds)
                    .label(title.to_owned())
                    .read_only(true)
                    .focus(frame.focus)
                    .text(AccessibleText::new(text).caret(caret)),
            );
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

/// Paints one row: backgrounds, selection, text runs, then decorations.
fn paint_row(
    bounds: Bounds,
    scene: &mut Scene,
    cx: &mut ElementContext,
    row: &GridRow,
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
    for run in &row.runs {
        let text = row.run_text(run);
        if text.trim().is_empty() {
            continue;
        }
        draw_text(
            scene,
            cx,
            m,
            text,
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

/// Lays out and draws `text` in `style` at `(x, y)`, `cols` cells wide.
/// `fg` overrides the style's color (the glyph under a block cursor).
#[allow(clippy::too_many_arguments)]
fn draw_text(
    scene: &mut Scene,
    cx: &mut ElementContext,
    m: &Metrics,
    text: &str,
    style: CellStyle,
    x: f32,
    y: f32,
    cols: u16,
    fg: Option<Color>,
) {
    let mut text_style = m.text_style();
    if style.bold {
        text_style = text_style.weight(FontWeight::Bold);
    }
    let italic = [TextSpan {
        range: 0..text.len(),
        weight: None,
        style: Some(FontStyle::Italic),
        kind: None,
    }];
    let query = TextQuery {
        spans: if style.italic { &italic } else { &[] },
        ..TextQuery::new(text, text_style)
    };
    let Some(layout) = cx.layout_text_query(&query) else {
        return;
    };
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
                if !text.trim().is_empty() {
                    let bg = color(grid.colors.background);
                    draw_text(
                        scene,
                        cx,
                        m,
                        text,
                        run.style,
                        rect.x,
                        rect.y,
                        cols,
                        Some(bg),
                    );
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
        _bounds: Bounds,
        _layout: &mut (),
        _prepaint: &mut (),
        engine: &LayoutEngine,
        scene: &mut Scene,
        cx: &mut ElementContext,
    ) {
        self.child.paint(engine, scene, cx);
    }
}

impl IntoAnyElement for BoundsProbe {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
