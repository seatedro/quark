//! The one conversion from an app's logical scene to the renderer's physical
//! pixels.

use quark::Rect;
use quark::scene::{Primitive, Scene};

/// Multiply every coordinate in `scene` by `scale`, turning logical points
/// into physical pixels. Rect edges snap to whole pixels so quads stay sharp
/// and neighbours tile without seams; a non-empty rect or border never
/// snaps away to nothing. Radii, blur, and shadow offsets scale unsnapped.
///
/// Text origins snap too. Glyphs are not resized here: a text layout must
/// already be shaped at `scale` (see `FrameContext::layout_text`), which puts
/// its glyphs in physical pixels.
pub fn scene_to_physical(scene: &mut Scene, scale: f32) {
    for primitive in &mut scene.primitives {
        scale_primitive(primitive, scale);
    }
}

fn scale_primitive(primitive: &mut Primitive, s: f32) {
    match primitive {
        Primitive::Rect(p) => p.rect = snap(p.rect, s),
        Primitive::RoundedRect(p) => {
            p.rect = snap(p.rect, s);
            p.corner_radii = p.corner_radii.map(|r| r * s);
        }
        Primitive::Border(p) => {
            p.rect = snap(p.rect, s);
            p.widths = p.widths.map(|w| snap_length(w, s));
            p.corner_radii = p.corner_radii.map(|r| r * s);
        }
        Primitive::Shadow(p) => {
            p.rect = snap(p.rect, s);
            p.blur_radius *= s;
            p.corner_radius *= s;
            p.offset = p.offset.map(|o| o * s);
        }
        Primitive::TextRun(p) => p.rect = snap(p.rect, s),
        Primitive::RichTextRun(p) => p.rect = snap(p.rect, s),
        Primitive::Icon(p) => p.rect = snap(p.rect, s),
        Primitive::Image(p) => p.rect = snap(p.rect, s),
        Primitive::EffectQuad(p) => {
            p.rect = snap(p.rect, s);
            p.corner_radius *= s;
        }
        Primitive::BlurRegion(p) => {
            p.rect = snap(p.rect, s);
            p.blur_radius *= s;
            p.corner_radius *= s;
        }
        Primitive::ClipStart(p) => {
            p.rect = snap(p.rect, s);
            p.corner_radii = p.corner_radii.map(|r| r * s);
        }
        Primitive::ClipEnd | Primitive::ZIndexPush(_) | Primitive::ZIndexPop => {}
        Primitive::LayerBoundary => {}
    }
}

/// Scale and round both edges, so adjacent rects share a pixel edge.
fn snap(rect: Rect, s: f32) -> Rect {
    let x0 = (rect.x * s).round();
    let y0 = (rect.y * s).round();
    let mut x1 = ((rect.x + rect.width) * s).round();
    let mut y1 = ((rect.y + rect.height) * s).round();
    if rect.width > 0.0 && x1 <= x0 {
        x1 = x0 + 1.0;
    }
    if rect.height > 0.0 && y1 <= y0 {
        y1 = y0 + 1.0;
    }
    Rect {
        x: x0,
        y: y0,
        width: x1 - x0,
        height: y1 - y0,
    }
}

/// A stroke width in whole pixels, at least one when it is drawn at all.
fn snap_length(length: f32, s: f32) -> f32 {
    if length > 0.0 {
        (length * s).round().max(1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use quark::Color;
    use quark::scene::{BorderPrimitive, RectPrimitive};

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
}
