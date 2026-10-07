use super::*;
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
enum TestAction {
    OpenRepoPicker,
    Bootstrap,
    StartDeviceFlow,
    SelectFile(usize),
    OpenRefPicker,
}

impl From<TestAction> for Action {
    fn from(value: TestAction) -> Self {
        Action::new(value)
    }
}

const FOCUS_LIST: FocusId = FocusId::new(1);
const FOCUS_EDITOR: FocusId = FocusId::new(2);

/// Vendored fonts only, so measurements do not depend on the machine.
struct TestText {
    system: TextSystem,
    layouts: LayoutCache,
}

impl TestText {
    fn new() -> Self {
        Self {
            system: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
        }
    }
}

fn test_cx<'a>(ts: &'a mut TestText, store: &'a mut SignalStore) -> ElementContext<'a> {
    let theme = Box::leak(Box::new(Theme::default_dark()));
    ElementContext::new(theme, 1.0, &mut ts.system, &mut ts.layouts, None, store)
}

#[test]
fn div_with_fixed_children_lays_out() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = div()
        .w(400.0)
        .h(300.0)
        .flex_row()
        .gap(10.0)
        .child(div().w(100.0).h_full())
        .child(div().flex_1().h_full())
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);

    // If we got here without panicking, the layout engine worked.
    // The scene should have no primitives (no bg/border set).
    assert_eq!(scene.len(), 0);
}

#[test]
fn div_with_background_emits_rounded_rect() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = div()
        .w(200.0)
        .h(100.0)
        .bg(Color::rgba(255, 0, 0, 255))
        .rounded(8.0)
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 100.0);

    assert_eq!(scene.len(), 1); // one rounded rect
}

#[test]
fn nested_divs_resolve_absolute_positions() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);

    let mut engine = LayoutEngine::new();
    let inner_w = 50.0;
    let padding = 20.0;

    // Outer: 200x100 with 20px padding, inner: 50x50
    let mut outer = div()
        .w(200.0)
        .h(100.0)
        .p(padding)
        .child(div().w(inner_w).h(inner_w));

    let (root_id, _) = outer.request_layout(&mut engine, &mut cx);
    engine.compute_layout(root_id, 200.0, 100.0);

    // The inner div should be offset by the padding.
    // Get child layout id — it's the first child of root.
    let inner_id = *engine.tree.children(root_id).unwrap().first().unwrap();
    let inner_bounds = engine.layout_bounds(inner_id);

    assert!(
        (inner_bounds.x - padding).abs() < 1.0,
        "inner x={} should be near padding={}",
        inner_bounds.x,
        padding
    );
    assert!(
        (inner_bounds.y - padding).abs() < 1.0,
        "inner y={} should be near padding={}",
        inner_bounds.y,
        padding
    );
    assert!((inner_bounds.width - inner_w).abs() < 1.0);
}

#[test]
fn text_element_emits_text_primitive() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = div()
        .w(400.0)
        .h(50.0)
        .child(
            text("Hello world")
                .size(14.0)
                .color(Color::rgba(255, 255, 255, 255)),
        )
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 400.0, 50.0);

    // Should have exactly one text primitive.
    let text_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::TextRun(_)))
        .count();
    assert_eq!(text_count, 1);
}

#[test]
fn string_as_child_works() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = div().w(300.0).h(40.0).child("bare string child").into_any();

    render_element(&mut root, &mut scene, &mut cx, 300.0, 40.0);

    let text_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::TextRun(_)))
        .count();
    assert_eq!(text_count, 1);
}

#[test]
fn text_element_has_intrinsic_width() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);

    let mut engine = LayoutEngine::new();
    let mut txt = text("ABCDE").size(10.0);
    let (id, _) = txt.request_layout(&mut engine, &mut cx);
    engine.compute_layout(id, 999.0, 999.0);

    let bounds = engine.layout_bounds(id);
    // 5 chars * 10.0 * 0.55 = 27.5
    assert!(
        bounds.width > 20.0 && bounds.width < 40.0,
        "text width {} should be roughly 27.5",
        bounds.width
    );
    // line height = 10.0 * 1.5 = 15.0
    assert!(
        (bounds.height - 15.0).abs() < 1.0,
        "text height {} should be ~15.0",
        bounds.height
    );
}

#[test]
fn hover_bg_applies_when_mouse_inside() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    cx.mouse_position = Some((100.0, 25.0)); // inside the 200x50 div

    let mut scene = Scene::default();
    let red = Color::rgba(255, 0, 0, 255);
    let blue = Color::rgba(0, 0, 255, 255);

    let mut root = div()
        .w(200.0)
        .h(50.0)
        .bg(red)
        .hover_bg(blue)
        .on_click(TestAction::Bootstrap)
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 50.0);

    // Should have painted blue (hover) not red (default)
    let bg_prim = scene
        .primitives
        .iter()
        .find(|p| matches!(p, quark_render::Primitive::RoundedRect(_)));
    assert!(bg_prim.is_some());
    if let quark_render::Primitive::RoundedRect(rr) = bg_prim.unwrap() {
        assert_eq!(rr.color, blue, "hover bg should be blue");
    }
}

#[test]
fn hover_bg_applies_when_rendered_at_offset() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    // Global mouse inside a 200x50 div offset by (400, 300).
    cx.mouse_position = Some((500.0, 325.0));

    let mut scene = Scene::default();
    let red = Color::rgba(255, 0, 0, 255);
    let blue = Color::rgba(0, 0, 255, 255);

    let mut root = div()
        .w(200.0)
        .h(50.0)
        .bg(red)
        .hover_bg(blue)
        .on_click(TestAction::Bootstrap)
        .into_any();

    render_element_at(&mut root, &mut scene, &mut cx, 400.0, 300.0, 200.0, 50.0);

    let bg_prim = scene
        .primitives
        .iter()
        .find(|p| matches!(p, quark_render::Primitive::RoundedRect(_)))
        .expect("expected a rounded rect");
    if let quark_render::Primitive::RoundedRect(rr) = bg_prim {
        assert_eq!(rr.color, blue, "hover bg should apply at offset");
        assert_eq!(rr.rect.x, 400.0, "rect x should be globally offset");
        assert_eq!(rr.rect.y, 300.0, "rect y should be globally offset");
    }
    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    let clicked = router.pointer_down(401.0, 301.0, &mut None).actions;
    assert_eq!(clicked, vec![Action::from(TestAction::Bootstrap)]);
}

#[test]
fn hover_bg_does_not_apply_when_mouse_outside() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    cx.mouse_position = Some((999.0, 999.0)); // outside

    let mut scene = Scene::default();
    let red = Color::rgba(255, 0, 0, 255);
    let blue = Color::rgba(0, 0, 255, 255);

    let mut root = div()
        .w(200.0)
        .h(50.0)
        .bg(red)
        .hover_bg(blue)
        .on_click(TestAction::Bootstrap)
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 50.0);

    if let quark_render::Primitive::RoundedRect(rr) = &scene.primitives[0] {
        assert_eq!(rr.color, red, "should use normal bg when not hovered");
    }
}

#[test]
fn realistic_title_bar_layout() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let theme = cx.theme;
    let mut root = div()
        .flex_row()
        .items_center()
        .w(1200.0)
        .h(52.0)
        .px(20.0)
        .bg(theme.colors.title_bar_background)
        .child(text("diffy").text_lg().color(theme.colors.text_strong))
        .child(spacer())
        .child(
            div()
                .flex_row()
                .gap(8.0)
                .child(
                    div()
                        .px(14.0)
                        .py(6.0)
                        .rounded(7.0)
                        .bg(theme.colors.element_background)
                        .hover_bg(theme.colors.element_hover)
                        .on_click(TestAction::OpenRepoPicker)
                        .child(text("Compare").text_sm().color(theme.colors.text)),
                )
                .child(
                    div()
                        .px(14.0)
                        .py(6.0)
                        .rounded(7.0)
                        .hover_bg(theme.colors.ghost_element_hover)
                        .on_click(TestAction::StartDeviceFlow)
                        .child(text("Sign in").text_sm().color(theme.colors.text_muted)),
                ),
        )
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 1200.0, 52.0);

    // Should have: title bar bg + "Compare" button bg + 3 text primitives
    let rect_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::RoundedRect(_)))
        .count();
    assert!(
        rect_count >= 2,
        "should have title bar bg + button bg, got {}",
        rect_count
    );

    let text_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::TextRun(_)))
        .count();
    assert_eq!(text_count, 3, "should have 3 text labels");
}

#[test]
fn truncate_text_to_fit_accounts_for_font_weight() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let text = "Open Compare With Repository";
    let font_size = 12.0;
    let max_width =
        cx.measure_text_width("Open Compare", font_size, FontKind::Ui, FontWeight::Medium);
    let full_width = cx.measure_text_width(text, font_size, FontKind::Ui, FontWeight::Medium);

    let (truncated, truncated_width) = truncate_text_to_fit(
        &mut cx,
        text,
        font_size,
        FontKind::Ui,
        FontWeight::Medium,
        full_width,
        max_width,
    );

    assert_ne!(
        truncated, text,
        "text should truncate when width is constrained"
    );
    assert!(
        truncated.ends_with('\u{2026}'),
        "truncated text should end with an ellipsis: {truncated:?}",
    );
    assert!(
        truncated_width <= max_width + 1.0,
        "truncated width {truncated_width} should fit max width {max_width}",
    );
}

#[test]
fn truncated_text_primitive_fits_bounds_for_medium_weight() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = div()
        .w(110.0)
        .h(28.0)
        .flex_row()
        .child(
            text("Open Compare With Repository")
                .text_sm()
                .medium()
                .truncate(),
        )
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 110.0, 28.0);

    let text = scene
        .primitives
        .iter()
        .find_map(|primitive| match primitive {
            quark_render::Primitive::TextRun(text) => Some(text.clone()),
            _ => None,
        })
        .expect("expected a text primitive");
    let layout = text
        .layout
        .downcast_ref::<TextLayout>()
        .expect("text layout");

    assert!(
        layout.text().ends_with('\u{2026}'),
        "rendered text should be truncated: {:?}",
        layout.text(),
    );
    let measured = layout.size().0;
    assert!(
        measured <= text.rect.width + 1.0,
        "measured width {measured} should fit rendered bounds {} for {:?}",
        text.rect.width,
        layout.text(),
    );
}

#[test]
fn realistic_file_list_with_scroll() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let theme = cx.theme;
    let files = ["src/main.rs", "src/lib.rs", "Cargo.toml", "README.md"];

    let mut root =
        div()
            .flex_col()
            .w(260.0)
            .h(400.0)
            .bg(theme.colors.sidebar_background)
            .child(
                div().px(12.0).py(12.0).child(
                    text(format!("Files  ·  {}", files.len()))
                        .text_sm()
                        .color(theme.colors.text_muted),
                ),
            )
            .child(div().flex_1().flex_col().scroll_y(0.0).children_from(
                files.iter().enumerate().map(|(i, path)| {
                    div()
                        .w_full()
                        .h(36.0)
                        .px(12.0)
                        .items_center()
                        .flex_row()
                        .rounded(7.0)
                        .hover_bg(theme.colors.sidebar_row_hover)
                        .on_click(TestAction::SelectFile(i))
                        .child(text(*path).text_sm().color(theme.colors.text))
                        .into_any()
                }),
            ))
            .into_any();

    render_element(&mut root, &mut scene, &mut cx, 260.0, 400.0);

    // Should have text for header + 4 files = 5 text primitives
    let text_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::TextRun(_)))
        .count();
    assert_eq!(text_count, 5);
}

#[test]
fn scroll_y_offsets_nested_descendant_text() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);

    let build_scene = |scroll_y: f32, cx: &mut ElementContext<'_>| {
        let mut scene = Scene::default();
        let mut root = div()
            .w(220.0)
            .h(80.0)
            .scroll_y(scroll_y)
            .child(
                div().w_full().h(36.0).child(
                    div()
                        .px(12.0)
                        .py(8.0)
                        .child(text("nested file row").text_sm()),
                ),
            )
            .into_any();

        render_element(&mut root, &mut scene, cx, 220.0, 80.0);

        scene
            .primitives
            .iter()
            .find_map(|primitive| match primitive {
                quark_render::Primitive::TextRun(text) => Some(text.rect.y),
                _ => None,
            })
            .expect("expected nested text primitive")
    };

    let unscrolled_y = build_scene(0.0, &mut cx);
    let scrolled_y = build_scene(20.0, &mut cx);

    assert!(
        (scrolled_y - (unscrolled_y - 20.0)).abs() < 1.0,
        "nested text did not move with scroll: unscrolled_y={unscrolled_y}, scrolled_y={scrolled_y}"
    );
}

#[test]
fn scroll_y_clips_and_offsets_children() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);

    // Container 100px tall, child 50px tall, scrolled down 20px.
    // Child should paint at y = -20 (shifted up), and be clipped.
    let mut root = div()
        .w(200.0)
        .h(100.0)
        .scroll_y(20.0)
        .child(div().w(200.0).h(50.0).bg(red))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 100.0);

    // Should have: ClipStart, RoundedRect (child bg), ClipEnd
    let clip_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::ClipStart(_)))
        .count();
    assert_eq!(clip_count, 1, "scroll container should clip");

    // The child's bg rect should be offset by -20 in y
    let bg = scene
        .primitives
        .iter()
        .find_map(|p| {
            if let quark_render::Primitive::RoundedRect(rr) = p {
                Some(rr)
            } else {
                None
            }
        })
        .expect("should have child bg");
    assert!(
        (bg.rect.y - (-20.0)).abs() < 1.0,
        "child y={} should be ~-20 (scrolled)",
        bg.rect.y
    );
}

#[test]
fn overflow_hidden_clips_children() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);

    let mut root = div()
        .w(120.0)
        .h(40.0)
        .overflow_hidden()
        .child(div().w(220.0).h(40.0).bg(red))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 120.0, 40.0);

    let clip_starts = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::ClipStart(_)))
        .count();
    let clip_ends = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::ClipEnd))
        .count();

    assert_eq!(clip_starts, 1, "overflow-hidden should push a clip region");
    assert_eq!(clip_ends, 1, "overflow-hidden should pop its clip region");
}

#[test]
fn rounded_overflow_hidden_emits_rounded_clip() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);

    // Outer: rounded + overflow-hidden. Inner: square bg that extends to
    // the outer's right edge. Mirrors the compare-cluster layout.
    let mut root = div()
        .w(200.0)
        .h(40.0)
        .overflow_hidden()
        .rounded(8.0)
        .child(div().w(200.0).h(40.0).bg(red))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 40.0);

    // The outer's clip primitive should carry the rounded corner radii.
    let clip_start = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            quark_render::Primitive::ClipStart(c) => Some(*c),
            _ => None,
        })
        .expect("should emit a ClipStart");

    assert_eq!(
        clip_start.corner_radii, [8.0; 4],
        "overflow-hidden on a rounded div should emit a rounded clip",
    );
}

// -- New tests --

#[test]
fn canvas_element_emits_custom_primitives() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let green = Color::rgba(0, 255, 0, 255);

    let mut root = div()
        .w(400.0)
        .h(300.0)
        .child(
            canvas(move |bounds, scene, _cx| {
                // Draw a custom rect using the resolved bounds.
                scene.rounded_rect(RoundedRectPrimitive::uniform(bounds, 0.0, green));
            })
            .w(100.0)
            .h(50.0),
        )
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);

    // The canvas closure should have emitted exactly one rounded rect.
    let rr_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::RoundedRect(_)))
        .count();
    assert_eq!(rr_count, 1, "canvas should emit one rounded rect");

    if let quark_render::Primitive::RoundedRect(rr) = &scene.primitives[0] {
        assert_eq!(rr.color, green, "canvas rect should be green");
        assert!(
            (rr.rect.width - 100.0).abs() < 1.0,
            "canvas width should be ~100"
        );
        assert!(
            (rr.rect.height - 50.0).abs() < 1.0,
            "canvas height should be ~50"
        );
    } else {
        panic!("expected RoundedRect primitive from canvas");
    }
}

#[test]
fn render_once_component_renders_correctly() {
    // Define a simple component that produces a div with text.
    struct MyButton {
        label: String,
        color: Color,
    }

    impl RenderOnce for MyButton {
        fn render(self, _cx: &ElementContext) -> AnyElement {
            div()
                .w(120.0)
                .h(40.0)
                .bg(self.color)
                .child(text(self.label).size(14.0))
                .into_any()
        }
    }

    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let blue = Color::rgba(0, 0, 255, 255);

    let button = MyButton {
        label: "Click me".into(),
        color: blue,
    };

    // Use the RenderOnce component as a child via IntoAnyElement.
    let mut root = div().w(400.0).h(200.0).child(button.into_any()).into_any();

    render_element(&mut root, &mut scene, &mut cx, 400.0, 200.0);

    // Should have the button's background rect and text.
    let rr_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::RoundedRect(_)))
        .count();
    assert_eq!(rr_count, 1, "button should emit one background rect");

    let text_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::TextRun(_)))
        .count();
    assert_eq!(text_count, 1, "button should emit one text primitive");

    if let quark_render::Primitive::RoundedRect(rr) = &scene.primitives[0] {
        assert_eq!(rr.color, blue, "button bg should be blue");
    }
}

#[test]
fn hover_style_override_changes_border() {
    let mut ts = TestText::new();
    let store = SignalStore::new();
    let mut cx = ElementContext::new(
        Box::leak(Box::new(Theme::default_dark())),
        1.0,
        &mut ts.system,
        &mut ts.layouts,
        Some((100.0, 25.0)), // inside
        &store,
    );
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);
    let blue = Color::rgba(0, 0, 255, 255);
    let green = Color::rgba(0, 255, 0, 255);

    let mut root = div()
        .w(200.0)
        .h(50.0)
        .bg(red)
        .border_b(blue)
        .hover(|s| s.bg(green).border_color(green))
        .on_click(TestAction::Bootstrap)
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 50.0);

    // Should use green bg and green border (hover override)
    let bg = scene
        .primitives
        .iter()
        .find_map(|p| {
            if let quark_render::Primitive::RoundedRect(rr) = p {
                Some(rr)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(bg.color, green, "hover should override bg to green");

    let border = scene
        .primitives
        .iter()
        .find_map(|p| {
            if let quark_render::Primitive::Border(b) = p {
                Some(b)
            } else {
                None
            }
        })
        .unwrap();
    assert_eq!(border.color, green, "hover should override border to green");
}

#[test]
fn when_conditional_applies() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);
    let blue = Color::rgba(0, 0, 255, 255);

    // .when(true, ...) should apply
    let mut root = div()
        .w(100.0)
        .h(50.0)
        .bg(red)
        .when(true, |d| d.bg(blue))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 100.0, 50.0);

    if let quark_render::Primitive::RoundedRect(rr) = &scene.primitives[0] {
        assert_eq!(rr.color, blue, "when(true) should apply bg override");
    }
}

#[test]
fn focus_tracking_query() {
    let mut ts = TestText::new();
    let store = SignalStore::new();
    let cx = ElementContext::new(
        Box::leak(Box::new(Theme::default_dark())),
        1.0,
        &mut ts.system,
        &mut ts.layouts,
        None,
        &store,
    )
    .with_focus(Some(FOCUS_LIST));

    assert!(cx.is_focused(FOCUS_LIST));
    assert!(!cx.is_focused(FOCUS_EDITOR));
}

#[test]
fn text_input_renders_label_and_value() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = text_input("Branch", "main")
        .w(200.0)
        .h(56.0)
        .on_click(TestAction::OpenRefPicker)
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 56.0);

    // Should have: bg rect + border + 2 text primitives (label + value)
    let text_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::TextRun(_)))
        .count();
    assert_eq!(text_count, 2, "should have label + value text");

    let mut router = InputRouter::default();
    router.set_frame(cx.take_input_frame());
    assert_eq!(router.cursor_at(100.0, 28.0), CursorHint::Text);
}

#[test]
fn when_conditional_skips() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);
    let blue = Color::rgba(0, 0, 255, 255);

    // .when(false, ...) should NOT apply
    let mut root = div()
        .w(100.0)
        .h(50.0)
        .bg(red)
        .when(false, |d| d.bg(blue))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 100.0, 50.0);

    if let quark_render::Primitive::RoundedRect(rr) = &scene.primitives[0] {
        assert_eq!(rr.color, red, "when(false) should keep original bg");
    }
}

#[test]
fn bg_effect_noise_gradient_emits_effect_quad() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let a = Color::rgba(255, 0, 0, 255);
    let b = Color::rgba(0, 0, 255, 255);

    let mut root = div()
        .w(300.0)
        .h(200.0)
        .rounded(10.0)
        .bg_effect(noise_gradient(0.02, a, b))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 300.0, 200.0);

    let effect_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::EffectQuad(_)))
        .count();
    assert_eq!(effect_count, 1, "should emit one effect quad");

    // Should NOT emit a RoundedRect bg (effect replaces it).
    let rr_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::RoundedRect(_)))
        .count();
    assert_eq!(rr_count, 0, "effect should replace solid bg");

    if let quark_render::Primitive::EffectQuad(eq) = &scene.primitives[0] {
        assert_eq!(eq.effect_type, quark_render::EffectType::NoiseGradient);
        assert_eq!(eq.color_a, a);
        assert_eq!(eq.color_b, b);
        assert!((eq.params[0] - 0.02).abs() < 0.001);
        assert!((eq.corner_radius - 10.0).abs() < 0.1);
    } else {
        panic!("expected EffectQuad primitive");
    }
}

#[test]
fn bg_effect_linear_gradient_emits_effect_quad() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let a = Color::rgba(0, 255, 0, 255);
    let b = Color::rgba(255, 255, 0, 255);
    let angle = std::f32::consts::FRAC_PI_2;

    let mut root = div()
        .w(200.0)
        .h(100.0)
        .bg_effect(linear_gradient(angle, a, b))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 100.0);

    let effect_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::EffectQuad(_)))
        .count();
    assert_eq!(effect_count, 1);

    if let quark_render::Primitive::EffectQuad(eq) = &scene.primitives[0] {
        assert_eq!(eq.effect_type, quark_render::EffectType::LinearGradient);
        assert!((eq.params[0] - angle).abs() < 0.001);
    } else {
        panic!("expected EffectQuad primitive");
    }
}

#[test]
fn bg_effect_replaces_solid_bg() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);
    let blue = Color::rgba(0, 0, 255, 255);

    // Setting both bg() and bg_effect() — effect should win.
    let mut root = div()
        .w(100.0)
        .h(100.0)
        .bg(red)
        .bg_effect(linear_gradient(0.0, red, blue))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 100.0, 100.0);

    let effect_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::EffectQuad(_)))
        .count();
    let rr_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::RoundedRect(_)))
        .count();

    assert_eq!(effect_count, 1, "effect should be emitted");
    assert_eq!(
        rr_count, 0,
        "solid bg should not be emitted when effect is set"
    );
}

#[test]
fn blur_emits_blur_region_primitive() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);

    let mut root = div()
        .w(400.0)
        .h(300.0)
        .blur(12.0)
        .bg(red)
        .rounded(14.0)
        .child(text("Frosted glass"))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);

    // Should have a BlurRegion primitive before the background.
    let blur_count = scene
        .primitives
        .iter()
        .filter(|p| matches!(p, quark_render::Primitive::BlurRegion(_)))
        .count();
    assert_eq!(blur_count, 1, "should emit one blur region");

    // The BlurRegion should come before the RoundedRect (background).
    let blur_idx = scene
        .primitives
        .iter()
        .position(|p| matches!(p, quark_render::Primitive::BlurRegion(_)))
        .unwrap();
    let bg_idx = scene
        .primitives
        .iter()
        .position(|p| matches!(p, quark_render::Primitive::RoundedRect(_)))
        .unwrap();
    assert!(blur_idx < bg_idx, "blur should precede background");

    if let quark_render::Primitive::BlurRegion(br) = &scene.primitives[blur_idx] {
        assert!((br.blur_radius - 12.0).abs() < 0.1);
        assert!((br.corner_radius - 14.0).abs() < 0.1);
        assert!((br.rect.width - 400.0).abs() < 1.0);
    } else {
        panic!("expected BlurRegion");
    }
}

#[test]
fn radial_gradient_emits_correct_effect_type() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let a = Color::rgba(255, 255, 255, 255);
    let b = Color::rgba(0, 0, 0, 255);

    let mut root = div()
        .w(200.0)
        .h(200.0)
        .bg_effect(radial_gradient(a, b))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 200.0);

    if let quark_render::Primitive::EffectQuad(eq) = &scene.primitives[0] {
        assert_eq!(eq.effect_type, quark_render::EffectType::RadialGradient);
    } else {
        panic!("expected EffectQuad");
    }
}

#[test]
fn shimmer_emits_correct_effect_type() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let base = Color::rgba(40, 40, 40, 255);
    let highlight = Color::rgba(60, 60, 60, 255);

    let mut root = div()
        .w(300.0)
        .h(20.0)
        .bg_effect(shimmer(base, highlight, 2.0))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 300.0, 20.0);

    if let quark_render::Primitive::EffectQuad(eq) = &scene.primitives[0] {
        assert_eq!(eq.effect_type, quark_render::EffectType::Shimmer);
        assert!((eq.params[0] - 2.0).abs() < 0.01, "speed should be 2.0");
    } else {
        panic!("expected EffectQuad");
    }
}

#[test]
fn vignette_emits_correct_effect_type() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let dark = Color::rgba(0, 0, 0, 128);

    let mut root = div()
        .w(800.0)
        .h(600.0)
        .bg_effect(vignette(dark, 0.5))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 800.0, 600.0);

    if let quark_render::Primitive::EffectQuad(eq) = &scene.primitives[0] {
        assert_eq!(eq.effect_type, quark_render::EffectType::Vignette);
        assert!((eq.params[0] - 0.5).abs() < 0.01, "intensity should be 0.5");
    } else {
        panic!("expected EffectQuad");
    }
}

#[test]
fn color_tint_emits_correct_effect_type() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let tint = Color::rgba(0, 100, 255, 80);

    let mut root = div()
        .w(400.0)
        .h(300.0)
        .bg_effect(color_tint(tint))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);

    if let quark_render::Primitive::EffectQuad(eq) = &scene.primitives[0] {
        assert_eq!(eq.effect_type, quark_render::EffectType::ColorTint);
        assert_eq!(eq.color_a, tint);
    } else {
        panic!("expected EffectQuad");
    }
}

#[test]
fn glow_adds_shadow_with_zero_offset() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let accent = Color::rgba(0, 128, 255, 200);

    let mut root = div()
        .w(100.0)
        .h(40.0)
        .rounded(8.0)
        .bg(Color::rgba(30, 30, 30, 255))
        .glow(accent, 10.0)
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 100.0, 40.0);

    // Glow should produce a ShadowPrimitive with offset [0, 0].
    let shadow = scene.primitives.iter().find_map(|p| {
        if let quark_render::Primitive::Shadow(s) = p {
            Some(s)
        } else {
            None
        }
    });
    assert!(shadow.is_some(), "glow should produce a shadow");
    let s = shadow.unwrap();
    assert_eq!(s.color, accent);
    assert!((s.offset[0]).abs() < 0.01, "glow x offset should be 0");
    assert!((s.offset[1]).abs() < 0.01, "glow y offset should be 0");
    assert!(
        (s.blur_radius - 10.0).abs() < 0.1,
        "blur radius should be 10"
    );
}

#[test]
fn z_index_emits_push_pop_primitives() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let red = Color::rgba(255, 0, 0, 255);

    let mut root = div().w(200.0).h(100.0).z_index(10).bg(red).into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 100.0);

    let has_push = scene
        .primitives
        .iter()
        .any(|p| matches!(p, quark_render::Primitive::ZIndexPush(10)));
    let has_pop = scene
        .primitives
        .iter()
        .any(|p| matches!(p, quark_render::Primitive::ZIndexPop));
    assert!(has_push, "z_index(10) should emit ZIndexPush(10)");
    assert!(has_pop, "z_index(10) should emit ZIndexPop");
}

#[test]
fn z_index_zero_emits_no_push_pop() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = div()
        .w(200.0)
        .h(100.0)
        .bg(Color::rgba(255, 0, 0, 255))
        .into_any();

    render_element(&mut root, &mut scene, &mut cx, 200.0, 100.0);

    let has_push = scene
        .primitives
        .iter()
        .any(|p| matches!(p, quark_render::Primitive::ZIndexPush(_)));
    assert!(!has_push, "z_index 0 should not emit ZIndexPush");
}

#[test]
fn focus_tree_registers_focus_ring_and_text_input_targets() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();
    const SEARCH: FocusId = FocusId::from_key("search");

    let mut root = div()
        .w(400.0)
        .h(300.0)
        .flex_col()
        .child(div().w(100.0).h(20.0).focus_ring(FOCUS_LIST).tab_stop(1))
        .child(
            text_input("Search", "")
                .focus_target(SEARCH)
                .w(200.0)
                .h(40.0),
        )
        .into_any();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);

    let tree = cx.semantic.focus_tree();
    let order: Vec<_> = tree.tab_order(None).into_iter().map(|n| n.id).collect();
    assert!(order.contains(&FOCUS_LIST), "{order:?}");
    assert!(order.contains(&SEARCH), "{order:?}");
}

#[test]
fn accessibility_tree_nests_buttons_under_their_dialog() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let button = |id: &str, label: &str| {
        div()
            .w(80.0)
            .h(30.0)
            .accessibility_id(id)
            .accessibility_role(AccessibilityRole::Button)
            .accessibility_label(label)
            .child(text(label))
    };
    let mut root = div()
        .w(400.0)
        .h(300.0)
        .flex_col()
        .child(
            div()
                .w(300.0)
                .h(200.0)
                .accessibility_id("dialog")
                .accessibility_role(AccessibilityRole::Dialog)
                .accessibility_label("Confirm")
                // A role-less group between dialog and buttons must not break nesting.
                .child(
                    div()
                        .flex_row()
                        .focus_scope("dialog")
                        .child(button("ok", "OK"))
                        .child(button("cancel", "Cancel")),
                ),
        )
        .into_any();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);

    assert_eq!(
        crate::accessibility::dump_accessibility_tree(&cx.accessibility),
        "dialog | Dialog | Confirm\n  ok | Button | OK\n  cancel | Button | Cancel\n"
    );
}

// Regression: element style stored one corner radius and Div paint passed
// [r; 4], so per-corner rounding could not reach the scene.
#[test]
fn div_rounded_corners_paints_background_with_per_corner_radii() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let mut scene = Scene::default();

    let mut root = div()
        .w(100.0)
        .h(50.0)
        .rounded_corners([8.0, 0.0, 4.0, 0.0])
        .bg(Color::rgba(255, 0, 0, 255))
        .into_any();
    render_element(&mut root, &mut scene, &mut cx, 100.0, 50.0);

    let radii = scene.primitives.iter().find_map(|p| match p {
        quark_render::Primitive::RoundedRect(rr) => Some(rr.corner_radii),
        _ => None,
    });
    assert_eq!(radii, Some([8.0, 0.0, 4.0, 0.0]));
}

fn rich_text_runs(scene: &Scene) -> Vec<RichTextPrimitive> {
    scene
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            quark_render::Primitive::RichTextRun(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

fn paint_one(cx: &mut ElementContext, element: impl IntoAnyElement) -> Scene {
    let mut scene = Scene::default();
    let mut root = div().w(400.0).h(3000.0).child(element).into_any();
    render_element(&mut root, &mut scene, cx, 400.0, 3000.0);
    scene
}

fn bold(text: &str) -> StyledSpan {
    StyledSpan {
        font_weight: FontWeight::Bold,
        ..StyledSpan::plain(text)
    }
}

// Regression: SelectableText capped its layout at 64 lines and folded the
// rest into the last line, so long comments lost lines and hit the wrong row.
#[test]
fn selectable_text_lays_out_every_line_of_long_text() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let body = (0..100)
        .map(|i| format!("line {i}"))
        .collect::<Vec<_>>()
        .join("\n");
    paint_one(
        &mut cx,
        selectable_text(body.clone()).width(300.0).size(14.0),
    );

    let region = &cx.selectable_text_runs[0];
    assert_eq!(region.layout.line_count(), 100);
    let bottom = region.bounds.y + region.bounds.height - 1.0;
    let hit = region.hit(region.bounds.x + 1.0, bottom);
    assert_eq!(&body[hit.get()..], "line 99");
}

// Regression: pointer hit-testing re-shaped selectable text as plain sans, so
// inside bold and code spans the byte under the pointer drifted from the
// glyph painted there.
#[test]
fn selectable_text_hit_inside_bold_span_lands_on_painted_glyph() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let code = StyledSpan {
        font_kind: FontKind::Mono,
        ..StyledSpan::plain("fn main()")
    };
    let spans = vec![
        StyledSpan::plain("Call "),
        code,
        StyledSpan::plain(" then "),
        bold("WWWWWWWWWWWW"),
    ];
    let target = "Call fn main() then WWWWWWWW".len();
    let scene = paint_one(&mut cx, selectable_rich_text(spans).width(390.0).size(14.0));

    let painted = &rich_text_runs(&scene)[0];
    let layout = painted.layout.downcast_ref::<TextLayout>().expect("layout");
    let g = layout.glyphs();
    let i = (0..g.len())
        .find(|&i| g.byte_start[i] as usize == target)
        .expect("glyph at target byte");
    let line = layout.line(0).expect("line");
    let x = painted.rect.x + g.x[i] + g.advance[i] * 0.25;
    let y = painted.rect.y + line.top + line.height * 0.5;

    let region = &cx.selectable_text_runs[0];
    assert_eq!(region.hit(x, y), target);
    let caret_x = region.text_origin.0 + region.layout.caret(target).x;
    assert!((caret_x - (painted.rect.x + g.x[i])).abs() < 0.01);
}

// Colors are applied at paint, so recoloring a block (hover, theme switch)
// must reuse its cached layout instead of shaping it again.
#[test]
fn selectable_text_color_change_reuses_layout() {
    let mut ts = TestText::new();
    let mut store = SignalStore::new();
    let mut cx = test_cx(&mut ts, &mut store);
    let block = |color| {
        selectable_rich_text(vec![
            StyledSpan::plain("see "),
            StyledSpan {
                color: Some(color),
                ..StyledSpan::plain("the link")
            },
        ])
        .width(300.0)
        .size(14.0)
    };
    let red = rich_text_runs(&paint_one(&mut cx, block(Color::rgba(255, 0, 0, 255))));
    cx.layouts.begin_frame();
    let blue = rich_text_runs(&paint_one(&mut cx, block(Color::rgba(0, 0, 255, 255))));

    assert_eq!(red[0].layout, blue[0].layout, "recolor shaped a new layout");
    assert_ne!(red[0].span_colors, blue[0].span_colors);
}
