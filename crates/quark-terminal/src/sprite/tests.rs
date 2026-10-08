use proptest::prelude::*;

use super::*;
use crate::metrics::FaceMetrics;

/// Cell metrics for a `width` by `ascent + descent` cell whose strokes are
/// `thickness` pixels, as Ghostty's sprite tests build them.
fn metrics(width: u32, ascent: u32, descent: u32, thickness: u32) -> CellMetrics {
    CellMetrics::new(
        &FaceMetrics {
            cell_width: f64::from(width),
            ascent: f64::from(ascent),
            descent: -f64::from(descent),
            line_gap: 0.0,
            underline_position: None,
            underline_thickness: Some(f64::from(thickness)),
            strikethrough_position: None,
            strikethrough_thickness: Some(f64::from(thickness)),
            cap_height: None,
            ex_height: None,
        },
        None,
    )
}

/// Every codepoint with a sprite.
fn sprite_codepoints() -> impl Iterator<Item = u32> {
    RANGES.iter().flat_map(|&(lo, hi, _)| lo..=hi)
}

/// Paints `rects` onto a cleared copy of the canvas's cell and margin.
fn repaint(canvas: &Canvas, rects: &[SpriteRect]) -> Canvas {
    let mut out = Canvas::new(canvas.width, canvas.height);
    for r in rects {
        out.rect(
            canvas::Rect {
                x: r.x,
                y: r.y,
                width: i32::from(r.width),
                height: i32::from(r.height),
            },
            r.alpha,
        );
    }
    out
}

proptest! {
    /// The rectangles a sprite is painted with cover exactly its
    /// coverage, antialiased edges and margin included, without overlap.
    #[test]
    fn sprite_rects_reproduce_the_coverage(
        index in 0..sprite_codepoints().count(),
        width in 5u32..24,
        height in 10u32..40,
        thickness in 1u32..4,
    ) {
        let cp = sprite_codepoints().nth(index).unwrap();
        let m = metrics(width, height - height / 4, height / 4, thickness);
        let canvas = render_for_test(cp, &m).unwrap();
        let rects = canvas.to_rects();
        let area: u64 = rects.iter().map(|r| u64::from(r.width) * u64::from(r.height)).sum();
        let covered = canvas.pixels_for_test().iter().filter(|&&a| a != 0).count() as u64;
        prop_assert_eq!(area, covered, "rectangles overlap");
        prop_assert!(
            repaint(&canvas, &rects).pixels_for_test() == canvas.pixels_for_test(),
            "U+{:X} repainted differently",
            cp
        );
    }
}

/// Compares every sprite with Ghostty's reference atlases
/// (`src/font/sprite/testdata` in the Ghostty tree), which draw each block
/// of 256 codepoints on a 16 by 16 grid of padded cells. Set
/// `GHOSTTY_SPRITE_TESTDATA` to that directory. Antialiasing differs
/// between z2d and tiny-skia, so only pixels that are fully on in one and
/// fully off in the other count as wrong.
#[test]
#[ignore = "needs Ghostty's reference images; prints a report"]
fn sprites_match_ghostty_reference_images() {
    let Ok(dir) = std::env::var("GHOSTTY_SPRITE_TESTDATA") else {
        panic!("set GHOSTTY_SPRITE_TESTDATA to Ghostty's src/font/sprite/testdata");
    };
    let sizes = [
        (18, 30, 6, 4),
        (12, 20, 4, 3),
        (11, 19, 2, 2),
        (9, 15, 2, 1),
    ];
    let mut wrong_total = 0;
    let mut soft_total = 0;
    for (width, ascent, descent, thickness) in sizes {
        let m = metrics(width, ascent, descent, thickness);
        let height = ascent + descent;
        let (pad_x, pad_y) = (width / 4, height / 4);
        let (stride_x, stride_y) = (width + 2 * pad_x, height + 2 * pad_y);
        let mut pages: Vec<u32> = sprite_codepoints().map(|cp| cp & !0xff).collect();
        pages.dedup();
        for page in pages {
            let path = format!(
                "{dir}/U+{page:X}...U+{:X}-{width}x{height}+{thickness}.png",
                page + 0xff
            );
            let Ok(reference) = tiny_skia::Pixmap::load_png(&path) else {
                println!("missing {path}");
                continue;
            };
            for cp in sprite_codepoints().filter(|cp| cp & !0xff == page) {
                let canvas = render_for_test(cp, &m).unwrap();
                let pixels = canvas.pixels_for_test();
                let (ox, oy) = (stride_x * ((cp - page) % 16), stride_y * ((cp - page) / 16));
                let (mut wrong, mut soft) = (0, 0);
                for y in 0..stride_y {
                    for x in 0..stride_x {
                        let r = reference.pixel(ox + x, oy + y).map_or(0, |p| p.red());
                        let a = pixels[(y * stride_x + x) as usize];
                        if (r == 0 && a == 255) || (r == 255 && a == 0) {
                            wrong += 1;
                        } else if r.abs_diff(a) > 96 {
                            soft += 1;
                        }
                    }
                }
                if wrong > 0 || soft > 2 {
                    println!("U+{cp:X} {width}x{height}+{thickness}: {wrong} wrong, {soft} soft");
                }
                wrong_total += wrong;
                soft_total += soft;
            }
        }
    }
    println!("{wrong_total} wrong pixels, {soft_total} soft");
    assert_eq!(wrong_total, 0);
}
