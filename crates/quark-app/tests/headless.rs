//! Renders a scene of the primitives apps draw through quark-app (shadow,
//! rounded rect, text) offscreen. Skips when no wgpu adapter exists.

use quark::scene::{
    FontKind, FontWeight, RoundedRectPrimitive, Scene, ShadowPrimitive, TextPrimitive,
};
use quark::{Color, Rect};
use quark_render::fonts::FontSettings;
use quark_render::{RenderError, Renderer};

#[test]
fn renders_scene_offscreen() {
    let (width, height) = (200, 120);
    let mut renderer = match Renderer::new_headless(width, height, 1.0, &FontSettings::default()) {
        Ok(renderer) => renderer,
        Err(RenderError::NoAdapter) => {
            // A skip reports as a pass, so CI sets QUARK_REQUIRE_GPU to turn a
            // missing adapter into a failure instead of silent lost coverage.
            assert!(
                std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
            );
            eprintln!("skipping: no wgpu adapter available");
            return;
        }
        Err(error) => panic!("headless renderer failed: {error}"),
    };

    let card = Rect {
        x: 40.0,
        y: 30.0,
        width: 120.0,
        height: 60.0,
    };
    let mut scene = Scene::default();
    scene.shadow(ShadowPrimitive {
        rect: card,
        blur_radius: 12.0,
        corner_radius: 8.0,
        offset: [0.0, 4.0],
        color: Color::rgba(0, 0, 0, 160),
    });
    scene.rounded_rect(RoundedRectPrimitive::uniform(
        card,
        8.0,
        Color::rgba(255, 0, 0, 255),
    ));
    scene.text(TextPrimitive {
        rect: Rect {
            x: 4.0,
            y: 96.0,
            width: 192.0,
            height: 20.0,
        },
        text: "quark".into(),
        color: Color::rgba(255, 255, 255, 255),
        font_size: 14.0,
        font_kind: FontKind::Ui,
        font_weight: FontWeight::Normal,
    });

    // QUARK_HEADLESS_OUT keeps the rendered PNG for visual inspection.
    let keep = std::env::var_os("QUARK_HEADLESS_OUT").map(std::path::PathBuf::from);
    let dir = std::env::temp_dir().join(format!("quark-app-headless-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = keep.clone().unwrap_or_else(|| dir.join("scene.png"));
    renderer
        .render_to_png(&scene, width, height, 1.0, &path)
        .expect("offscreen render");

    let image = image::open(&path).expect("decode png").into_rgba8();
    assert_eq!(image.dimensions(), (width, height));
    let center = image.get_pixel(100, 60).0;
    assert!(
        center[0] > 200 && center[1] < 40 && center[2] < 40,
        "card center should be red, got {center:?}"
    );
    let text_band = (4..196).flat_map(|x| (96..116).map(move |y| (x, y)));
    assert!(
        text_band
            .into_iter()
            .any(|(x, y)| image.get_pixel(x, y).0[0] > 128),
        "text should draw light pixels"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
