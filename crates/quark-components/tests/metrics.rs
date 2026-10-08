//! A theme's component recipe sizes controls and surfaces once: set in
//! points at 100% zoom, multiplied by the zoom exactly once, whatever the
//! zoom. Modal padding used to be scaled before its view scaled it again,
//! growing by the square of the zoom.

use std::rc::Rc;

use quark::reactive::SignalStore;
use quark::{Color, Rect};
use quark_components::{
    Button, Modal, SelectOption, SelectState, TooltipSide, popover_panel, select, tooltip_layer,
};
use quark_render::{Primitive, Scene};
use quark_ui::Action;
use quark_ui::element::{AnyElement, ElementContext, IntoAnyElement, div, render_element};
use quark_ui::style::Styled;
use quark_ui::theme::{ComponentMetrics, ControlMetrics, Elevation, SurfaceMetrics, Theme};

#[derive(Debug, Clone, PartialEq)]
struct Pick;

impl From<Pick> for Action {
    fn from(value: Pick) -> Self {
        Action::new(value)
    }
}

struct Painted {
    scene: Scene,
    frame: quark::SemanticFrame,
}

fn paint(theme: &Theme, root: AnyElement) -> Painted {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    // A 2x window: logical bounds must not see the device scale.
    let mut cx = ElementContext::new(theme, 2.0, &mut text, &mut layouts, None, &store);
    let mut root = root;
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 1200.0, 900.0);
    Painted {
        scene,
        frame: std::mem::take(&mut cx.semantic),
    }
}

fn bounds(painted: &Painted, test_id: &str) -> Rect {
    painted
        .frame
        .nodes()
        .iter()
        .find(|n| n.test_id.as_ref().is_some_and(|t| t.as_str() == test_id))
        .map(|n| n.bounds)
        .unwrap_or_else(|| panic!("no {test_id}"))
}

fn compact() -> ComponentMetrics {
    ComponentMetrics {
        button: ControlMetrics {
            height: Some(36.0),
            ..Default::default()
        },
        select: ControlMetrics {
            height: Some(32.0),
            ..Default::default()
        },
        modal: SurfaceMetrics {
            padding_x: Some(16.0),
            padding_y: Some(20.0),
            ..Default::default()
        },
        ..Default::default()
    }
}

#[test]
fn recipe_sizes_scale_once_at_every_zoom() {
    for zoom in [1.0, 1.25, 2.0] {
        let mut theme = Theme::default_light().with_ui_scale(zoom);
        theme.components = compact();
        let at = |points: f32| (points * zoom).round();

        let options: Rc<[SelectOption]> = Rc::from(vec![SelectOption::new("Opus")]);
        let row = div()
            .flex_row()
            .items_start()
            .child(Button::new(Pick).label("Send"))
            .child(select(&SelectState::new("model"), options, |_| Pick.into()));
        let painted = paint(&theme, row.into_any());
        assert_eq!(
            bounds(&painted, "button").height,
            at(36.0),
            "button at {zoom}"
        );
        assert_eq!(
            bounds(&painted, "select-trigger").height,
            at(32.0),
            "select at {zoom}"
        );

        let modal = Modal::new("Settings", "", "", 560.0, 1200.0, 900.0, Pick)
            .body_child(div().test_id("body").w(10.0).h(10.0));
        let painted = paint(&theme, modal.into_any());
        let (panel, body) = (bounds(&painted, "modal"), bounds(&painted, "body"));
        assert_eq!(body.x - panel.x, at(16.0), "modal inset at {zoom}");
        // Plus the panel's 1-point bottom border.
        assert_eq!(
            panel.y + panel.height - (body.y + body.height),
            at(20.0) + 1.0,
            "modal bottom padding at {zoom}"
        );
    }
}

/// The corner radii and shadows of the surfaces in `painted`.
fn surfaces(painted: &Painted) -> (Vec<f32>, Vec<(f32, f32, Color)>) {
    let mut radii = Vec::new();
    let mut shadows = Vec::new();
    for primitive in &painted.scene.primitives {
        match primitive {
            Primitive::RoundedRect(rr) => radii.push(rr.corner_radii[0]),
            Primitive::Shadow(s) => shadows.push((s.blur_radius, s.offset[1], s.color)),
            _ => {}
        }
    }
    (radii, shadows)
}

#[test]
fn surface_recipe_sets_radius_and_replaces_the_shadow_preset() {
    let menu = SurfaceMetrics {
        radius: Some(8.0),
        shadow: Some(Elevation {
            offset_y: 4.0,
            blur: 16.0,
            alpha: 31,
        }),
        ..Default::default()
    };
    let mut theme = Theme::default_dark();
    theme.components.popover = menu;
    theme.components.tooltip = menu;
    let expected = (vec![8.0], vec![(16.0, 4.0, Color::rgba(0, 0, 0, 31))]);

    let panel = popover_panel(&theme).w(100.0).h(40.0);
    assert_eq!(
        surfaces(&paint(&theme, panel.into_any())),
        expected,
        "popover"
    );
    let tip = tooltip_layer("Copy", 10.0, 10.0, TooltipSide::Bottom, &theme);
    assert_eq!(surfaces(&paint(&theme, tip)), expected, "tooltip");
}
