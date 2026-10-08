use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::accessibility::{AccessibilityFrame, dump_accessibility};
use crate::test_alloc;
use crate::theme::Theme;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Pressed(usize);

impl From<Pressed> for Action {
    fn from(value: Pressed) -> Self {
        Action::new(value)
    }
}

/// A window: text state, theme, and the element cache, kept across frames.
struct Window {
    text: TextSystem,
    layouts: LayoutCache,
    signals: SignalStore,
    theme: Theme,
    scale: f32,
    pointer: Option<(f32, f32)>,
    /// Assistive tech listens.
    accessibility: bool,
    cache: ElementCache,
}

struct Frame {
    scene: Scene,
    input: InputFrame,
    accessibility: AccessibilityFrame,
}

impl Window {
    fn new() -> Self {
        Self {
            text: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
            signals: SignalStore::new(),
            theme: Theme::default_dark(),
            scale: 1.0,
            pointer: None,
            accessibility: true,
            cache: ElementCache::new(),
        }
    }

    /// Paint `root` into a 400x300 window; `cache` false paints without
    /// the element cache, as the reference for what a build produces.
    fn paint_with(&mut self, mut root: AnyElement, cache: bool) -> Frame {
        self.layouts.begin_frame();
        let mut cx = ElementContext::new(
            &self.theme,
            self.scale,
            &mut self.text,
            &mut self.layouts,
            self.pointer,
            &self.signals,
        )
        .with_accessibility(self.accessibility);
        if cache {
            cx = cx.with_element_cache(&mut self.cache);
        }
        cx.accessibility = AccessibilityFrame::new(400.0, 300.0);
        cx.semantic = SemanticFrame::new(400.0, 300.0);
        let mut scene = Scene::default();
        render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
        Frame {
            scene,
            input: cx.take_input_frame(),
            accessibility: std::mem::take(&mut cx.accessibility),
        }
    }

    fn paint(&mut self, root: AnyElement) -> Frame {
        self.paint_with(root, true)
    }

    /// Paint `root` into `last`'s buffers, as a host does every frame, so
    /// what a frame allocates is the elements' and the cache's own.
    fn repaint(&mut self, mut root: AnyElement, last: Frame) -> Frame {
        let Frame {
            mut scene,
            input,
            mut accessibility,
        } = last;
        scene.primitives.clear();
        accessibility.reset(400.0, 300.0);
        self.layouts.begin_frame();
        let mut cx = ElementContext::new(
            &self.theme,
            self.scale,
            &mut self.text,
            &mut self.layouts,
            self.pointer,
            &self.signals,
        )
        .with_accessibility(self.accessibility)
        .with_element_cache(&mut self.cache)
        .with_input_frame(input);
        cx.semantic.reset(400.0, 300.0);
        cx.accessibility = accessibility;
        render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
        Frame {
            scene,
            input: cx.take_input_frame(),
            accessibility: std::mem::take(&mut cx.accessibility),
        }
    }
}

impl Frame {
    /// Actions a click at `(x, y)` delivers.
    fn click(self, x: f32, y: f32) -> Vec<Pressed> {
        let mut router = InputRouter::default();
        router.set_frame(self.input);
        let mut focus = None;
        let mut actions = router.pointer_down(x, y, &mut focus).actions;
        actions.extend(router.pointer_up().actions);
        actions
            .iter()
            .filter_map(|a| a.downcast_ref::<Pressed>().cloned())
            .collect()
    }

    /// Fill colors of the scene's rounded rects, in paint order.
    fn fills(&self) -> Vec<Color> {
        self.scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                quark_render::Primitive::RoundedRect(r) => Some(r.color),
                _ => None,
            })
            .collect()
    }
}

const BUTTON: Color = Color::rgba(10, 20, 30, 255);
const HOVER: Color = Color::rgba(200, 100, 50, 255);

/// A labeled, clickable, hoverable button.
fn button(id: usize) -> AnyElement {
    div()
        .w(120.0)
        .h(30.0)
        .bg(BUTTON)
        .hover_bg(HOVER)
        .accessibility_role(AccessibilityRole::Button)
        .accessibility_label(format!("Button {id}"))
        .on_click(Pressed(id))
        .child(text(format!("Button {id}")).size(12.0))
        .into_any()
}

/// A column with `top` points of space above a cached button. `builds`
/// counts how often the button's closure runs.
fn screen(top: f32, hash: u64, builds: &Rc<Cell<u32>>) -> AnyElement {
    let builds = builds.clone();
    div()
        .size_full()
        .flex_col()
        .child(div().h(top))
        .child(cached("button", hash, move || {
            builds.set(builds.get() + 1);
            button(7)
        }))
        .into_any()
}

/// The same column built without a cache boundary.
fn reference(top: f32) -> AnyElement {
    div()
        .size_full()
        .flex_col()
        .child(div().h(top))
        .child(button(7))
        .into_any()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn unchanged_inputs_replay_without_building() {
    let mut window = Window::new();
    let builds = Rc::new(Cell::new(0));
    let first = window.paint(screen(0.0, 1, &builds));
    let second = window.paint(screen(0.0, 1, &builds));
    assert_eq!(builds.get(), 1);
    assert_eq!(second.scene, first.scene);
}

#[test]
fn changed_inputs_hash_rebuilds() {
    let mut window = Window::new();
    let builds = Rc::new(Cell::new(0));
    window.paint(screen(0.0, 1, &builds));
    window.paint(screen(0.0, 2, &builds));
    assert_eq!(builds.get(), 2);
}

#[test]
fn scale_and_theme_changes_rebuild() {
    type Change = fn(&mut Window);
    let cases: &[(&str, Change)] = &[
        ("scale", |w| w.scale = 2.0),
        ("theme", |w| w.theme = Theme::default_light()),
    ];
    for (name, change) in cases {
        let mut window = Window::new();
        let builds = Rc::new(Cell::new(0));
        window.paint(screen(0.0, 1, &builds));
        change(&mut window);
        window.paint(screen(0.0, 1, &builds));
        assert_eq!(builds.get(), 2, "{name}");
    }
}

#[test]
fn replay_at_a_new_origin_moves_hits_and_accessibility() {
    let mut window = Window::new();
    let builds = Rc::new(Cell::new(0));
    window.paint(screen(0.0, 1, &builds));
    let moved = window.paint(screen(100.0, 1, &builds));
    let built = window.paint_with(reference(100.0), false);
    assert_eq!(builds.get(), 1, "the move replayed");

    assert_eq!(
        dump_accessibility(&moved.accessibility),
        dump_accessibility(&built.accessibility)
    );
    assert_eq!(moved.click(10.0, 110.0), vec![Pressed(7)]);
    assert_eq!(
        built.click(10.0, 10.0),
        vec![],
        "nothing left at the old origin"
    );
}

#[test]
fn replayed_hits_answer_clicks_only_inside_the_new_bounds() {
    let mut window = Window::new();
    let builds = Rc::new(Cell::new(0));
    window.paint(screen(0.0, 1, &builds));
    let moved = window.paint(screen(100.0, 1, &builds));
    assert_eq!(moved.click(10.0, 10.0), vec![]);
}

#[test]
fn assistive_tech_connecting_rebuilds_with_accessibility() {
    let mut window = Window::new();
    let builds = Rc::new(Cell::new(0));
    window.accessibility = false;
    window.paint(screen(0.0, 1, &builds));
    window.accessibility = true;
    let connected = window.paint(screen(0.0, 1, &builds));
    assert_eq!(
        dump_accessibility(&connected.accessibility),
        dump_accessibility(&window.paint_with(reference(0.0), false).accessibility)
    );
}

#[test]
fn hover_inside_a_cached_subtree_updates() {
    let mut window = Window::new();
    let builds = Rc::new(Cell::new(0));
    let mut fills = Vec::new();
    for pointer in [None, Some((10.0, 10.0)), Some((300.0, 200.0)), None] {
        window.pointer = pointer;
        fills.push(window.paint(screen(0.0, 1, &builds)).fills());
    }
    assert_eq!(
        fills,
        [vec![BUTTON], vec![HOVER], vec![BUTTON], vec![BUTTON]]
    );
    assert_eq!(
        builds.get(),
        3,
        "hover in and out rebuild; the last frame replays"
    );
}

#[test]
fn a_new_width_rebuilds_at_that_width() {
    let mut window = Window::new();
    let builds = Rc::new(Cell::new(0));
    let row = |width: f32, builds: &Rc<Cell<u32>>| {
        let builds = builds.clone();
        div()
            .w(width)
            .flex_col()
            .child(cached("row", 1, move || {
                builds.set(builds.get() + 1);
                div().w_full().h(10.0).bg(BUTTON)
            }))
            .into_any()
    };
    window.paint(row(100.0, &builds));
    let wide = window.paint(row(200.0, &builds));
    let rects: Vec<f32> = wide
        .scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            quark_render::Primitive::RoundedRect(r) => Some(r.rect.width),
            _ => None,
        })
        .collect();
    assert_eq!(rects, [200.0]);
    assert_eq!(builds.get(), 2);
}

/// A cached 100pt viewport over twenty 20pt rows, row `i` filled with
/// red `i`, scrolled by `handle`. The inputs hash never changes.
fn scroller(handle: &ScrollHandle) -> AnyElement {
    let handle = handle.clone();
    cached("scroller", 1, move || {
        div()
            .w(100.0)
            .h(100.0)
            .flex_col()
            .track_scroll(&handle)
            .overflow_y_scroll()
            .children((0..20u8).map(|i| {
                div()
                    .w_full()
                    .h(20.0)
                    .flex_shrink_0()
                    .bg(Color::rgba(i, 0, 0, 255))
                    .into_any()
            }))
    })
    .into_any()
}

impl Frame {
    /// The rect filled with `color`.
    fn rect_of(&self, color: Color) -> Option<Rect> {
        self.scene.primitives.iter().find_map(|p| match p {
            quark_render::Primitive::RoundedRect(r) if r.color == color => Some(r.rect),
            _ => None,
        })
    }
}

#[test]
fn a_moved_scroll_handle_repaints_its_cached_subtree() {
    type Move = fn(&ScrollHandle);
    let moves: &[(&str, Move)] = &[
        ("requested jump", |h| h.set_offset(0.0, 60.0)),
        ("wheel", |h| {
            h.scroll_by(Axis::Y, 60.0, 0);
        }),
    ];
    for (name, scroll) in moves {
        let mut window = Window::new();
        let handle = ScrollHandle::new();
        window.paint(scroller(&handle));
        scroll(&handle);
        let frame = window.paint(scroller(&handle));
        assert_eq!(
            frame.rect_of(Color::rgba(3, 0, 0, 255)).map(|r| r.y),
            Some(0.0),
            "{name}: row 3 at the top"
        );
    }
}

/// A label in a cached boundary, followed by a `BUTTON` swatch: the
/// swatch sits where the boundary's measured width ends.
fn label_then_swatch() -> AnyElement {
    div()
        .flex_row()
        .child(cached("label", 1, || text("Hello, wide world").size(14.0)))
        .child(div().w(10.0).h(10.0).bg(BUTTON))
        .into_any()
}

// Catches replaying geometry shaped with the old fonts: a replay looks up
// no text, so nothing but the cache's font dependency notices the change.
#[test]
fn a_font_change_lays_out_a_cached_boundary_again() {
    let inter = quark_text::FontSettings {
        ui_family: "Inter".into(),
        ..Default::default()
    };
    let mut window = Window::new();
    let geist = window.paint(label_then_swatch()).rect_of(BUTTON);
    window.text.set_font_settings(&inter);
    let changed = window.paint(label_then_swatch()).rect_of(BUTTON);

    let mut fresh = Window::new();
    fresh.text.set_font_settings(&inter);
    let expected = fresh.paint(label_then_swatch()).rect_of(BUTTON);
    assert_ne!(
        expected, geist,
        "the two fonts set the label at different widths"
    );
    assert_eq!(changed, expected);
}

impl Frame {
    /// Lines of every painted text, in paint order.
    fn text_lines(&self) -> Vec<Vec<String>> {
        self.scene
            .primitives
            .iter()
            .filter_map(|p| match p {
                quark_render::Primitive::TextRun(run) => {
                    let layout = run.layout.downcast_ref::<TextLayout>()?;
                    Some(
                        layout
                            .lines()
                            .map(|line| layout.text()[line.byte_range].to_owned())
                            .collect(),
                    )
                }
                _ => None,
            })
            .collect()
    }
}

/// Wrapping text in a boundary whose size its own style fixes, so its
/// parent places it without a measure query.
fn fixed_paragraph(width: f32) -> AnyElement {
    cached("paragraph", 1, || {
        div()
            .flex_col()
            .child(text("the quick brown fox jumps over the lazy dog").size(14.0))
    })
    .w(width)
    .h(100.0)
    .into_any()
}

// Catches replaying lines wrapped at the old width when a boundary's own
// style resizes it: the parent then asks it no sizing query, so only the
// final placement (a memo miss, or failing that the recorded size) tells
// the resized boundary apart.
#[test]
fn a_resized_fixed_boundary_rewraps_its_text() {
    let mut window = Window::new();
    window.paint(fixed_paragraph(300.0));
    let narrow = window.paint(fixed_paragraph(120.0)).text_lines();
    let fresh = Window::new().paint_with(fixed_paragraph(120.0), false);
    assert_eq!(narrow, fresh.text_lines());
    assert!(narrow[0].len() > 1, "{narrow:?}");
}

// ---------------------------------------------------------------------------
// Frame budget: the regression guard for per-frame waste
// ---------------------------------------------------------------------------

/// A cached, clickable 100x30 box whose canvas paints `color`, under
/// `key`. The boundary is the window's root and sized like a terminal
/// row, so layout comes from Taffy's caches and what a rebuild allocates
/// is the element tree's and the cache's own.
fn swatch(key: u64, color: Color, action: &Action) -> AnyElement {
    let action = action.clone();
    let hash = inputs_hash(&(color.r, color.g, color.b, color.a));
    cached(key, hash, move || {
        div().w(100.0).h(30.0).on_click(action).child(
            canvas(move |bounds, scene, _| {
                scene.rounded_rect(quark_render::RoundedRectPrimitive::uniform(
                    bounds, 0.0, color,
                ));
            })
            .w(100.0)
            .h(30.0),
        )
    })
    .w(100.0)
    .h(30.0)
    .into_any()
}

// Catches per-rebuild bookkeeping allocations: the recorded handler
// columns, the boundary's live state, and the canvas closure.
#[test]
fn rebuilding_a_boundary_with_new_content_allocates_nothing() {
    let mut window = Window::new();
    window.accessibility = false;
    let action = Action::from(Pressed(1));
    let unseen = Color::rgba(1, 2, 3, 255);
    let mut frame = window.paint(swatch(0, BUTTON, &action));
    frame = window.repaint(swatch(0, HOVER, &action), frame);

    let (frame, allocated) =
        test_alloc::count(|| window.repaint(swatch(0, unseen, &action), frame));

    assert_eq!(frame.fills(), [unseen]);
    assert_eq!(allocated, 0);
    assert_eq!(frame.click(10.0, 10.0), vec![Pressed(1)]);
}

// Catches a new entry allocating fresh buffers while evicted rows' paint
// records and measure memos sit unused: a boundary keyed by its content
// gets a new key whenever the content changes.
#[test]
fn a_boundary_under_a_new_key_reuses_an_evicted_rows_buffers() {
    let mut window = Window::new();
    window.accessibility = false;
    let action = Action::from(Pressed(1));
    let mut frame = window.paint(swatch(0, BUTTON, &action));
    for key in 1..=EVICT_AFTER_PASSES + 2 {
        frame = window.repaint(swatch(key, BUTTON, &action), frame);
    }
    let unseen = EVICT_AFTER_PASSES + 3;

    let (frame, allocated) =
        test_alloc::count(|| window.repaint(swatch(unseen, HOVER, &action), frame));

    assert_eq!(frame.fills(), [HOVER]);
    assert_eq!(allocated, 0);
}

const ROWS: usize = 2_000;

fn list(builds: &Rc<Cell<u32>>, theme: &Theme) -> AnyElement {
    let surface = theme.colors.surface;
    div()
        .w(400.0)
        .h(300.0)
        .flex_col()
        .scroll_y(0.0)
        .children((0..ROWS).map(|i| {
            let builds = builds.clone();
            cached(i as u64, 1, move || {
                builds.set(builds.get() + 1);
                div()
                    .w_full()
                    .flex_col()
                    .p(4.0)
                    .bg(surface)
                    .child(text(format!("Author {i}")).size(12.0))
                    .child(text(format!("Message {i}")).size(14.0))
            })
            .into_any()
        }))
        .into_any()
}

#[test]
fn a_repeated_frame_of_cached_rows_stays_within_budget() {
    let mut window = Window::new();
    window.accessibility = false;
    let builds = Rc::new(Cell::new(0));
    let theme = window.theme.clone();
    window.paint(list(&builds, &theme));

    let (frame, allocated) = test_alloc::count(|| window.paint(list(&builds, &theme)));
    drop(frame);

    assert_eq!(builds.get(), ROWS as u32, "no row rebuilt");
    let nodes = window
        .cache
        .engine
        .as_ref()
        .map_or(0, LayoutEngine::node_count);
    assert!(nodes <= ROWS + 1, "{nodes} layout nodes");
    let budget = ROWS as u64 * 8;
    assert!(
        allocated <= budget,
        "{allocated} allocations, budget {budget}"
    );
}

/// The list without cache boundaries.
fn plain_list(rows: usize, theme: &Theme) -> AnyElement {
    let surface = theme.colors.surface;
    div()
        .w(400.0)
        .h(300.0)
        .flex_col()
        .scroll_y(0.0)
        .children((0..rows).map(|i| {
            div()
                .w_full()
                .flex_col()
                .p(4.0)
                .bg(surface)
                .child(text(format!("Author {i}")).size(12.0))
                .child(text(format!("Message {i}")).size(14.0))
                .into_any()
        }))
        .into_any()
}

/// Prints allocations per frame and the top call sites of a repeated
/// frame, cached and not. Run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement, prints a report"]
fn report_frame_allocations() {
    let mut window = Window::new();
    let theme = window.theme.clone();
    let builds = Rc::new(Cell::new(0));
    let modes = [
        ("plain", false, false),
        ("plain+a11y", false, true),
        ("cached", true, false),
        ("cached+a11y", true, true),
    ];
    for (name, cached, accessibility) in modes {
        window.accessibility = accessibility;
        let build = |cached: bool| {
            if cached {
                list(&builds, &theme)
            } else {
                plain_list(ROWS, &theme)
            }
        };
        for frame in 0..3 {
            let started = std::time::Instant::now();
            let (_, n) = test_alloc::count(|| window.paint(build(cached)));
            eprintln!(
                "{name} frame {frame}: {n} allocations, {:?}",
                started.elapsed()
            );
        }
        let (_, sites) = test_alloc::profile(|| window.paint(build(cached)));
        eprintln!("{name} top sites:");
        for (site, n) in sites.iter().take(25) {
            eprintln!("  {n:6}  {site}");
        }
    }
}
