//! The one conversion from an app's logical scene to the renderer's physical
//! pixels.

use quark::scene::Scene;

/// Convert every primitive of `scene` from logical points to physical
/// pixels at `scale`; see [`to_physical`]. Rect edges snap to whole pixels,
/// and chunks convert their primitives as they are drawn.
///
/// Text origins snap too. Glyphs are not resized here: a text layout must
/// already be shaped at `scale` (see `FrameContext::layout_text`), which puts
/// its glyphs in physical pixels.
///
/// [`to_physical`]: quark::scene::Primitive::to_physical
pub fn scene_to_physical(scene: &mut Scene, scale: f32) {
    for primitive in &mut scene.primitives {
        primitive.to_physical(scale);
    }
}

#[cfg(test)]
mod tests {
    use quark::scene::{
        BorderPrimitive, EffectQuadPrimitive, EffectType, LayerPrimitive, Primitive, RectPrimitive,
    };
    use quark::{Color, Rect, Transform2D};

    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn physical(primitive: Primitive, scale: f32) -> String {
        let mut scene = Scene::default();
        scene.push(primitive);
        scene_to_physical(&mut scene, scale);
        match &scene.primitives[0] {
            Primitive::Rect(p) => format!("{:?}", p.rect),
            Primitive::Border(p) => format!("{:?} widths={:?}", p.rect, p.widths),
            other => format!("{other:?}"),
        }
    }

    // Regression guard for blurry or seamed quads at fractional scales: both
    // edges land on whole pixels, and hairlines survive.
    #[test]
    fn quads_snap_to_whole_pixels() {
        let cases = [
            (
                rect(10.0, 5.0, 100.0, 50.0),
                2.0,
                rect(20.0, 10.0, 200.0, 100.0),
            ),
            (rect(0.3, 0.3, 10.2, 10.2), 1.5, rect(0.0, 0.0, 16.0, 16.0)),
            (rect(1.0, 1.0, 1.0, 1.0), 1.25, rect(1.0, 1.0, 2.0, 2.0)),
            (rect(2.0, 2.0, 0.2, 0.2), 1.0, rect(2.0, 2.0, 1.0, 1.0)),
            (rect(5.0, 5.0, 0.0, 3.0), 2.0, rect(10.0, 10.0, 0.0, 6.0)),
        ];
        for (logical, scale, expected) in cases {
            let quad = Primitive::Rect(RectPrimitive {
                rect: logical,
                color: Color::rgba(0, 0, 0, 255),
            });
            assert_eq!(
                physical(quad, scale),
                format!("{expected:?}"),
                "{logical:?} at {scale}"
            );
        }
    }

    #[test]
    fn border_widths_scale_and_keep_hairlines() {
        let border = Primitive::Border(BorderPrimitive {
            rect: rect(0.0, 0.0, 10.0, 10.0),
            widths: [1.0, 0.0, 0.25, 2.0],
            corner_radii: [0.0; 4],
            color: Color::rgba(0, 0, 0, 255),
        });
        assert_eq!(
            physical(border, 1.5),
            format!(
                "{:?} widths=[2.0, 0.0, 1.0, 3.0]",
                rect(0.0, 0.0, 15.0, 15.0)
            )
        );
    }

    // A layer must land where its logical content would, scaled: mapping a
    // point then scaling equals scaling then mapping with the physical map.
    #[test]
    fn layer_transform_maps_physical_points_like_logical_ones() {
        let logical = Transform2D::rotate(0.6)
            .then(Transform2D::scale(1.5, 0.75))
            .around(40.0, 25.0);
        let mut scene = Scene::default();
        scene.push(Primitive::LayerStart(LayerPrimitive {
            opacity: 1.0,
            transform: logical,
        }));
        scene_to_physical(&mut scene, 2.0);
        let Primitive::LayerStart(physical) = &scene.primitives[0] else {
            panic!("layer start");
        };
        for (x, y) in [(0.0, 0.0), (40.0, 25.0), (13.0, -8.0)] {
            let (lx, ly) = logical.apply(x, y);
            let (px, py) = physical.transform.apply(x * 2.0, y * 2.0);
            assert!((px - lx * 2.0).abs() < 1e-3 && (py - ly * 2.0).abs() < 1e-3);
        }
    }

    // Regression guard: noise sampled per physical pixel doubled its
    // frequency on a 2x display.
    #[test]
    fn noise_frequency_is_per_logical_point() {
        let mut scene = Scene::default();
        scene.effect_quad(EffectQuadPrimitive {
            rect: rect(0.0, 0.0, 10.0, 10.0),
            effect_type: EffectType::NoiseGradient,
            params: [0.02, 0.0],
            ..Default::default()
        });
        scene_to_physical(&mut scene, 2.0);
        let Primitive::EffectQuad(effect) = &scene.primitives[0] else {
            panic!("effect quad");
        };
        assert_eq!(effect.params, [0.01, 0.0]);
    }
}
