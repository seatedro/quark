//! The diff view's elements: one cache boundary for the whole view, and
//! one per gutter cell, text cell, and header row inside it, so an
//! unchanged frame replays without building or allocating and a scrolled
//! frame builds only the rows entering the window.

use std::cell::Cell;
use std::rc::Rc;
use std::sync::Arc;

use accesskit::Role;
use quark::view;
use quark_diff::{GapId, Mode, Reveal, RowKind, Side};
use quark_render::scene::{Rect, RichTextPrimitive, ShapedText};
use quark_render::{FontKind, RoundedRectPrimitive, Scene};
use quark_syntax::HighlightKind;
use quark_text::{TextParams, TextStyle};
use quark_ui::Action;
use quark_ui::design::Alpha;
use quark_ui::element::{
    AnyElement, Bounds, CacheKey, ClickEvent, CursorHint, DragHandler, DragReleaseResult, Element,
    ElementContext, IntoAnyElement, LayoutEngine, LayoutId, ScrollActionBuilder, cached, canvas,
    div, inputs_hash, svg_icon, text,
};
use quark_ui::icons::lucide;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme};

use super::{
    AUTOSCROLL_FRAME_MS, DiffEvent, DiffKey, DiffViewState, FrameRow, Metrics, REVEAL_STEP,
    ViewFrame,
};
use crate::tree::CollectionEnv;

const KEYS: &[(&str, DiffKey)] = &[
    ("n", DiffKey::NextHunk),
    ("p", DiffKey::PrevHunk),
    ("alt+arrowdown", DiffKey::NextHunk),
    ("alt+arrowup", DiffKey::PrevHunk),
    ("]", DiffKey::NextFile),
    ("[", DiffKey::PrevFile),
    ("arrowup", DiffKey::LineUp),
    ("arrowdown", DiffKey::LineDown),
    ("pageup", DiffKey::PageUp),
    ("pagedown", DiffKey::PageDown),
    ("home", DiffKey::Home),
    ("end", DiffKey::End),
    ("mod+c", DiffKey::Copy),
    ("mod+a", DiffKey::SelectAll),
];

/// Theme colors the view paints with, resolved once per build.
#[derive(Debug, Clone, Copy, PartialEq)]
struct DiffColors {
    surface: Color,
    filler: Color,
    text: Color,
    muted: Color,
    border: Color,
    gutter: Color,
    gutter_text: Color,
    add: Color,
    del: Color,
    add_word: Color,
    del_word: Color,
    add_text: Color,
    del_text: Color,
    file_header: Color,
    hunk_header: Color,
    hover: Color,
    selection: Color,
    syntax: [Color; 8],
}

impl DiffColors {
    fn of(theme: &Theme) -> Self {
        let c = &theme.colors;
        Self {
            surface: c.editor_surface,
            filler: c.background,
            text: c.text,
            muted: c.text_muted,
            border: c.border_variant,
            gutter: c.gutter_bg,
            gutter_text: c.gutter_text,
            add: c.line_add,
            del: c.line_del,
            add_word: c.line_add_word_bg,
            del_word: c.line_del_word_bg,
            add_text: c.line_add_text,
            del_text: c.line_del_text,
            file_header: c.file_header_bg,
            hunk_header: c.hunk_header_bg,
            hover: c.ghost_element_hover,
            selection: c.accent.with_alpha(Alpha::SOFT),
            syntax: [
                c.syntax_keyword,
                c.syntax_string,
                c.syntax_comment,
                c.syntax_function,
                c.syntax_type,
                c.syntax_number,
                c.syntax_property,
                c.syntax_operator,
            ],
        }
    }

    fn tone(&self, kind: HighlightKind) -> Color {
        let i = match kind {
            HighlightKind::Keyword | HighlightKind::Preprocessor => 0,
            HighlightKind::String => 1,
            HighlightKind::Comment => 2,
            HighlightKind::Function => 3,
            HighlightKind::Type | HighlightKind::Namespace => 4,
            HighlightKind::Number | HighlightKind::Constant | HighlightKind::Builtin => 5,
            HighlightKind::Property
            | HighlightKind::Attribute
            | HighlightKind::Tag
            | HighlightKind::Label => 6,
            HighlightKind::Operator => 7,
            HighlightKind::Normal | HighlightKind::Punctuation | HighlightKind::Variable => {
                return self.text;
            }
        };
        self.syntax[i]
    }

    /// Background of a line of `kind` on `side`, if it has one.
    fn line(&self, kind: RowKind, side: Side) -> Option<Color> {
        match (kind, side) {
            (RowKind::Removed, _) | (RowKind::Modified, Side::Old) => Some(self.del),
            (RowKind::Added, _) | (RowKind::Modified, Side::New) => Some(self.add),
            _ => None,
        }
    }
}

/// The diff in `state` at its viewport size, as of the last
/// [`DiffViewState::prepare`]. `on_event` wraps input into the app's
/// action type.
pub fn diff_view(
    state: &mut DiffViewState,
    theme: &Theme,
    env: CollectionEnv,
    on_event: fn(DiffEvent) -> Action,
) -> AnyElement {
    let (width, height) = state.viewport;
    let Some(frame) = state.frame.clone() else {
        return view! { <div w={width} h={height} /> };
    };
    let colors = DiffColors::of(theme);
    // The cache watches the sideways scroll handles itself.
    let hash = inputs_hash(&(state.frame_id, env, on_event as usize));
    BoundsProbe {
        child: view! {
            <cached(state.id, hash, move || build(&frame, colors, env, on_event))
                    w={width} h={height} />
        },
        bounds: state.bounds.clone(),
        autoscrolling: state.wants_frame(),
    }
    .into_any()
}

fn build(
    frame: &Rc<ViewFrame>,
    colors: DiffColors,
    env: CollectionEnv,
    on_event: fn(DiffEvent) -> Action,
) -> AnyElement {
    let (width, height) = frame.viewport;
    let first_top = frame.rows.first().map_or(0.0, |r| r.top);
    let columns = frame
        .columns
        .sides
        .iter()
        .enumerate()
        .filter_map(|(slot, column)| {
            let column = column.as_ref()?;
            let side = if slot == 0 { Side::Old } else { Side::New };
            let content_w = if frame.wrap {
                column.text_w
            } else {
                frame.content_w[slot].max(column.text_w)
            };
            Some((slot, column, side, content_w))
        });
    view! {
        <div w={width} h={height} bg={colors.surface} track_focus={frame.focus}
             // The body below is moved back by the offset, so this only
             // feeds the wheel and the scrollbar.
             scroll_y={frame.scroll} scroll_total={frame.total}
             on:scroll={ScrollActionBuilder::new(move |lines| on_event(DiffEvent::Scroll(lines)))
                 .with_to_px(move |px| on_event(DiffEvent::ScrollTo(px as f32)))}
             @when {frame.scrollbar_auto_hide} {
                 scrollbar_visibility={&frame.scrollbar} class="scrollbar-auto-hide"
             }
             @for &(binding, key) in KEYS { on_key={(binding, on_event(DiffEvent::Key(key)))} }
             @when {env.accessible} {
                 accessibility_id={frame.id} accessibility_role={Role::List}
                 aria-label={frame.label}
             }>
            <div w={width} h={height} class="relative" translate={(0.0, frame.scroll)}
                 class="cursor-text"
                 on:drag={move |press: ClickEvent| {
                     Box::new(SelectDrag { press, on_event }) as Box<dyn DragHandler>
                 }}>
                for (slot, column, side, content_w) in columns {
                    <div class="absolute" left={column.gutter_x} class="top-0" w={column.gutter_w}
                         h={height} class="overflow-clip" bg={colors.gutter}>
                        <div w={column.gutter_w} class="flex-col" translate={(0.0, first_top)}>
                            for row in &frame.rows {
                                {gutter_cell(frame, row, side, column.gutter_w, colors)}
                            }
                        </div>
                    </div>
                    <div class="absolute" left={column.text_x} class="top-0" w={column.text_w}
                         h={height}
                         @when {frame.wrap} { class="overflow-clip" }
                         @when {!frame.wrap} {
                             track_scroll={&frame.hscroll[slot]} class="overflow-x-scroll"
                             scroll_total_x={content_w} class="scrollbar-auto-hide"
                         }>
                        <div w={content_w} class="flex-col" translate={(0.0, first_top)}>
                            for row in &frame.rows {
                                {text_cell(frame, row, side, content_w, colors, env)}
                            }
                        </div>
                    </div>
                }
                for row in frame.rows.iter().filter(|r| !r.paint.kind.is_line()) {
                    <div class="absolute left-0" top={row.top} w={width} h={row.height}>
                        {band(frame, row, colors, env, on_event)}
                    </div>
                }
            </div>
        </div>
    }
}

/// Cache key of one part of a row.
fn part_key(row: u64, part: u64) -> CacheKey {
    CacheKey(row ^ (part + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15))
}

/// The line `side`'s column shows for `row`: unified shows removed lines
/// from the old side and everything else from the new.
fn shown_side(mode: Mode, kind: RowKind, side: Side) -> Side {
    match mode {
        Mode::Split => side,
        Mode::Unified if kind == RowKind::Removed => Side::Old,
        Mode::Unified => Side::New,
    }
}

fn gutter_cell(
    frame: &ViewFrame,
    row: &FrameRow,
    side: Side,
    width: f32,
    colors: DiffColors,
) -> AnyElement {
    let (height, kind) = (row.height, row.paint.kind);
    if !kind.is_line() {
        return view! { <div w={width} h={height} class="shrink-0" /> };
    }
    let mode = frame.columns.mode;
    let numbers = match mode {
        Mode::Unified => row.paint.numbers,
        Mode::Split if side == Side::Old => [row.paint.numbers[0], 0],
        Mode::Split => [0, row.paint.numbers[1]],
    };
    let shown = shown_side(mode, kind, side);
    if mode == Mode::Split && numbers[side as usize] == 0 {
        return view! { <div w={width} h={height} class="shrink-0" bg={colors.filler} /> };
    }
    let m = frame.metrics;
    let hash = inputs_hash(&(
        row.paint.stamp,
        numbers,
        mode,
        height.to_bits(),
        width.to_bits(),
    ));
    let build = move || {
        view! {
            <div w={width} h={height} class="shrink-0"
                 @when {let Some(bg) = colors.line(kind, shown)} { bg={bg} }>
                <canvas(move |bounds, scene, cx| {
                    paint_numbers(bounds, scene, cx, &m, mode, numbers, kind, shown, colors);
                })
                    w={width} h={height} />
            </div>
        }
    };
    view! { <cached(part_key(row.key, side as u64), hash, build) w={width} h={height} /> }
}

#[allow(clippy::too_many_arguments)]
fn paint_numbers(
    bounds: Bounds,
    scene: &mut Scene,
    cx: &mut ElementContext,
    m: &Metrics,
    mode: Mode,
    numbers: [u32; 2],
    kind: RowKind,
    side: Side,
    colors: DiffColors,
) {
    let style = TextStyle::new(m.font_size)
        .kind(FontKind::Mono)
        .line_height(m.line_h);
    let columns = match mode {
        Mode::Unified => 2,
        Mode::Split => 1,
    };
    let mut x = bounds.x;
    for (i, &number) in numbers.iter().enumerate() {
        if mode == Mode::Split && number == 0 {
            continue;
        }
        if number != 0 {
            let label = number.to_string();
            if let Some(layout) = cx.layout_text(&TextParams::new(label, style)) {
                let w = layout.size().0;
                let right = x + m.number_w - m.char_w;
                scene.rich_text(RichTextPrimitive {
                    rect: Rect {
                        x: right - w,
                        y: bounds.y,
                        width: w + 1.0,
                        height: m.line_h,
                    },
                    layout: ShapedText::new(layout),
                    default_color: colors.gutter_text,
                    span_colors: Arc::from([]),
                });
            }
        }
        if columns == 2 || i == side as usize {
            x += m.number_w;
        }
    }
    let sign = match (kind, side) {
        (RowKind::Removed, _) | (RowKind::Modified, Side::Old) => Some(("-", colors.del_text)),
        (RowKind::Added, _) | (RowKind::Modified, Side::New) => Some(("+", colors.add_text)),
        _ => None,
    };
    if let Some((sign, color)) = sign
        && let Some(layout) = cx.layout_text(&TextParams::new(sign, style))
    {
        let x = bounds.x + bounds.width - m.sign_w + (m.sign_w - layout.size().0) / 2.0;
        scene.rich_text(RichTextPrimitive {
            rect: Rect {
                x,
                y: bounds.y,
                width: m.sign_w,
                height: m.line_h,
            },
            layout: ShapedText::new(layout),
            default_color: color,
            span_colors: Arc::from([]),
        });
    }
}

fn text_cell(
    frame: &ViewFrame,
    row: &FrameRow,
    side: Side,
    width: f32,
    colors: DiffColors,
    env: CollectionEnv,
) -> AnyElement {
    let (height, kind) = (row.height, row.paint.kind);
    let mode = frame.columns.mode;
    let shown = shown_side(mode, kind, side);
    if !kind.is_line() {
        return view! { <div w={width} h={height} class="shrink-0" /> };
    }
    if row.paint.sides[shown as usize].is_none() {
        return view! { <div w={width} h={height} class="shrink-0" bg={colors.filler} /> };
    }
    let paint = row.paint.clone();
    let selected = row.selected[shown as usize];
    let pad = frame.metrics.text_pad;
    let font_size = frame.metrics.font_size;
    let position = (row.index + 1, frame.row_count);
    let hash = inputs_hash(&(
        paint.stamp,
        selected,
        width.to_bits(),
        height.to_bits(),
        env.accessible,
        mode,
        position,
    ));
    let build = move || {
        let line = paint.sides[shown as usize].as_ref().expect("shown line");
        let canvas_paint = paint.clone();
        view! {
            <div w={width} h={height} class="shrink-0"
                 @when {let Some(bg) = colors.line(kind, shown)} { bg={bg} }
                 @when {env.accessible} {
                     accessibility_role={Role::ListItem}
                     aria-label={line.layout.text().to_string()}
                     aria-description={kind.description()}
                     aria-valuetext={match (mode, paint.numbers) {
                         (Mode::Unified, [o, n]) if o != 0 && n != 0 => {
                             format!("old line {o}, new line {n}")
                         }
                         (_, [o, _]) if shown == Side::Old => format!("old line {o}"),
                         (_, [_, n]) => format!("new line {n}"),
                     }}
                 }>
                <canvas(move |bounds, scene, _cx| {
                    if let Some(line) = &canvas_paint.sides[shown as usize] {
                        paint_line(bounds, scene, line, shown, selected, pad, font_size, colors);
                    }
                })
                    w={width} h={height} />
            </div>
        }
    };
    view! { <cached(part_key(row.key, 2 + side as u64), hash, build) w={width} h={height} /> }
}

#[allow(clippy::too_many_arguments)]
fn paint_line(
    bounds: Bounds,
    scene: &mut Scene,
    line: &super::LinePaint,
    side: Side,
    selected: Option<(usize, usize)>,
    pad: f32,
    font_size: f32,
    colors: DiffColors,
) {
    let layout = &line.layout;
    let origin = (bounds.x + pad, bounds.y);
    let mut fill = |range: std::ops::Range<usize>, color: Color| {
        for r in layout.selection_rects(range) {
            scene.rounded_rect(RoundedRectPrimitive::uniform(
                Rect {
                    x: origin.0 + r.x,
                    y: origin.1 + r.y,
                    width: r.width.max(1.0),
                    height: r.height,
                },
                2.0,
                color,
            ));
        }
    };
    let word = if side == Side::Old {
        colors.del_word
    } else {
        colors.add_word
    };
    for range in &line.words {
        fill(range.clone(), word);
    }
    if let Some((lo, hi)) = selected {
        fill(lo..hi, colors.selection);
    }
    let (w, h) = layout.size();
    scene.rich_text(RichTextPrimitive {
        rect: Rect {
            x: origin.0,
            y: origin.1,
            // Italic and wide glyphs ink past their advance.
            width: w + font_size,
            height: h.max(1.0),
        },
        layout: ShapedText::new(layout.clone()),
        default_color: colors.text,
        span_colors: line.tones.iter().map(|&k| colors.tone(k)).collect(),
    });
}

/// A file header, hunk header, or collapsed gap row across the view.
fn band(
    frame: &ViewFrame,
    row: &FrameRow,
    colors: DiffColors,
    env: CollectionEnv,
    on_event: fn(DiffEvent) -> Action,
) -> AnyElement {
    let (width, height) = (frame.viewport.0, row.height);
    let paint = row.paint.clone();
    let pad = frame.metrics.char_w * 2.0;
    let font_size = frame.metrics.font_size;
    let id = frame.id;
    let hash = inputs_hash(&(
        paint.stamp,
        width.to_bits(),
        height.to_bits(),
        env,
        on_event as usize,
    ));
    let build = move || {
        let (adds, dels) = paint.stats;
        let status = paint.status.name();
        let gap = (paint.kind == RowKind::Gap).then(|| paint.gap.expect("gap row"));
        let header = paint.kind == RowKind::FileHeader;
        view! {
            <div w={width} h={height} class="flex-row items-center" gap={pad * 0.5} px={pad}
                 border_b={colors.border}
                 bg={if header { colors.file_header } else { colors.hunk_header }}
                 @when {header && env.accessible} {
                     accessibility_id={format!("{id}.file.{}", paint.file)}
                     accessibility_role={Role::Heading}
                     aria-label={format!("{}, {status}, {adds} added, {dels} removed", paint.title)}
                 }>
                if header {
                    <text size={font_size * 0.85} color={colors.muted}>{status}</text>
                    <text size={font_size} class="font-semibold" color={colors.text}>
                        {&*paint.title}
                    </text>
                    <div class="flex-1" />
                    if paint.binary {
                        <text size={font_size * 0.85} color={colors.muted}>"binary"</text>
                    }
                    <text size={font_size} color={colors.add_text}>"+{adds}"</text>
                    <text size={font_size} color={colors.del_text}>"-{dels}"</text>
                } else {
                    if let Some(gap) = gap {
                        for (reveal, icon, label) in gap_controls(gap) {
                            {expand_button(gap, reveal, icon, label, height, colors, env, on_event)}
                        }
                    }
                    <text size={font_size * 0.9} class="font-mono" color={colors.muted}>
                        {&*paint.title}
                    </text>
                }
            </div>
        }
    };
    view! { <cached(part_key(row.key, 4), hash, build) w={width} h={height} /> }
}

/// The expand controls of a gap: up and down between hunks, down only
/// after the last hunk, and all.
fn gap_controls(gap: GapId) -> Vec<(Reveal, &'static str, String)> {
    let mut controls = Vec::with_capacity(3);
    if gap.hunk.is_some() {
        controls.push((
            Reveal::Up,
            lucide::CHEVRON_UP,
            format!("Show {REVEAL_STEP} more lines above the next change"),
        ));
    }
    controls.push((
        Reveal::Down,
        lucide::CHEVRON_DOWN,
        format!("Show {REVEAL_STEP} more lines below the previous change"),
    ));
    controls.push((
        Reveal::All,
        lucide::LIST,
        "Show all unchanged lines".to_owned(),
    ));
    controls
}

#[allow(clippy::too_many_arguments)]
fn expand_button(
    gap: GapId,
    reveal: Reveal,
    icon: &'static str,
    label: String,
    height: f32,
    colors: DiffColors,
    env: CollectionEnv,
    on_event: fn(DiffEvent) -> Action,
) -> AnyElement {
    let size = (height - 6.0).max(12.0);
    view! {
        <div w={size} h={size} rounded={4.0} class="flex-row items-center justify-center"
             hover_bg={colors.hover} on:click={on_event(DiffEvent::Expand(gap, reveal))}
             @when {env.accessible} { accessibility_role={Role::Button} aria-label={label} }>
            <icon svg={icon} size={(size * 0.7).round()} color={colors.muted} />
        </div>
    }
}

/// A drag that selects text. The press is reported at press time so the
/// anchor lands on what was under the pointer.
struct SelectDrag {
    press: ClickEvent,
    on_event: fn(DiffEvent) -> Action,
}

impl DragHandler for SelectDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.on_event)(DiffEvent::Press {
            x: self.press.x,
            y: self.press.y,
        })]
    }

    fn on_move(&mut self, x: f32, y: f32) -> Vec<Action> {
        vec![(self.on_event)(DiffEvent::Drag { x, y })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.on_event)(DiffEvent::Release)],
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

/// Records its child's bounds every frame, replayed or not, so pointer
/// events map into the view.
struct BoundsProbe {
    child: AnyElement,
    bounds: Rc<Cell<Rect>>,
    /// A drag autoscrolls: ask for the next frame.
    autoscrolling: bool,
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
        self.bounds.set(bounds);
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
        if self.autoscrolling {
            cx.request_frame_at_ms(cx.clock_ms + AUTOSCROLL_FRAME_MS);
        }
        self.child.paint(engine, scene, cx);
    }
}

impl IntoAnyElement for BoundsProbe {
    fn into_any(self) -> AnyElement {
        AnyElement::new(self)
    }
}
