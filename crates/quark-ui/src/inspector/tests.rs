use quark::reactive::SignalStore;
use quark::scene::{Primitive, Scene};
use quark::{Color, Rect, UiKey};
use quark_text::{LayoutCache, TextSystem};

use super::*;
use crate::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element};
use crate::style::Styled;
use crate::theme::Theme;

const RED: Color = Color::rgba(250, 10, 10, 255);

/// Paints `root` into a 400x300 window with the inspector recording, and
/// returns the scene. The recorded frame is left in `devtools`.
fn paint(devtools: &mut Devtools, mut root: AnyElement) -> Scene {
    let mut text = TextSystem::vendored_only(&Default::default());
    let mut layouts = LayoutCache::default();
    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    devtools.begin_frame(&mut cx.devtools);
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
    devtools.end_frame(&mut cx.devtools);
    scene
}

fn inspecting() -> Devtools {
    Devtools {
        inspector: true,
        ..Devtools::default()
    }
}

fn picked_key(devtools: &Devtools, x: f32, y: f32) -> Option<String> {
    let frame = devtools.frame();
    let index = frame.pick(x, y)?;
    frame.records()[index].key.as_ref().map(|k| k.to_string())
}

fn bounds_of(devtools: &Devtools, key: &str) -> Rect {
    let frame = devtools.frame();
    let index = frame.for_key(&UiKey::from(key)).expect(key);
    frame.records()[index].bounds
}

fn painted_in(scene: &Scene, color: Color) -> Vec<Rect> {
    scene
        .primitives
        .iter()
        .filter_map(|primitive| match primitive {
            Primitive::RoundedRect(p) if p.color == color => Some(p.rect),
            Primitive::Rect(p) if p.color == color => Some(p.rect),
            _ => None,
        })
        .collect()
}

fn boxed(key: &str, x: f32, y: f32, w: f32, h: f32) -> crate::element::Div {
    div().key(key).absolute().left(x).top(y).w(w).h(h)
}

// Regression: the inspector picked an element its ancestor clips away at
// that point, or a lower-z element just because it was painted later.
#[test]
fn pick_returns_topmost_element_honoring_clip_and_z() {
    let mut devtools = inspecting();
    let root = div()
        .key("root")
        .w(400.0)
        .h(300.0)
        .child(
            boxed("clipper", 0.0, 0.0, 100.0, 100.0)
                .overflow_hidden()
                .child(boxed("spill", 50.0, 50.0, 200.0, 200.0)),
        )
        .child(boxed("raised", 200.0, 0.0, 100.0, 100.0).z_index(5))
        .child(boxed("later", 250.0, 0.0, 100.0, 100.0))
        .into_any();
    paint(&mut devtools, root);

    let cases = [
        ((20.0, 20.0), "clipper"),
        ((75.0, 75.0), "spill"),
        // Inside spill's bounds but outside clipper's clip.
        ((150.0, 150.0), "root"),
        // raised and later overlap; raised has the higher z.
        ((275.0, 50.0), "raised"),
        ((325.0, 50.0), "later"),
    ];
    for ((x, y), expected) in cases {
        assert_eq!(
            picked_key(&devtools, x, y).as_deref(),
            Some(expected),
            "at ({x}, {y})"
        );
    }
}

// Regression: an inspector edit leaked to other elements, or stuck after
// the override was cleared.
#[test]
fn style_override_applies_to_keyed_element_until_cleared() {
    let tree = || {
        let card = |key: &str| {
            div()
                .key(key)
                .p(4.0)
                .child(div().key(format!("{key}.child")).w(10.0).h(10.0))
        };
        div()
            .w(400.0)
            .h(300.0)
            .flex_col()
            .child(card("a"))
            .child(card("b"))
            .into_any()
    };
    let inset = |devtools: &Devtools, key: &str| {
        let (outer, inner) = (
            bounds_of(devtools, key),
            bounds_of(devtools, &format!("{key}.child")),
        );
        (inner.x - outer.x, inner.y - outer.y)
    };
    let mut devtools = inspecting();
    devtools.overrides_mut().edit(UiKey::from("a"), |edit| {
        edit.padding = Some(20.0);
        edit.background = Some(RED);
    });

    let scene = paint(&mut devtools, tree());
    assert_eq!(inset(&devtools, "a"), (20.0, 20.0));
    assert_eq!(inset(&devtools, "b"), (4.0, 4.0));
    assert_eq!(painted_in(&scene, RED), vec![bounds_of(&devtools, "a")]);

    devtools.overrides_mut().clear(&UiKey::from("a"));
    let scene = paint(&mut devtools, tree());
    assert_eq!(inset(&devtools, "a"), (4.0, 4.0));
    assert_eq!(painted_in(&scene, RED), Vec::<Rect>::new());
}
