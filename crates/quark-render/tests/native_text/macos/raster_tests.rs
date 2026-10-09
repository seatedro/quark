//! CoreText rasterizer tests (macOS only; compiled into
//! `text_engine::raster::coretext` as its test module). The reference
//! sheets in `refs/` are CoreText's own drawing of the same glyphs, from
//! `ct_ref.swift` on macOS 27 (see `run.sh`); swash is the independent
//! check for placement.

use std::collections::HashMap;
use std::path::PathBuf;

use quark::{FontKind, FontWeight};
use quark_text::cosmic_text::{
    CacheKey, CacheKeyFlags, PhysicalGlyph, SubpixelBin, SwashCache, SwashContent,
};
use quark_text::fonts::{FontRegistry, SYSTEM_UI, UI_MONOSPACE};
use quark_text::{FontSettings, TextParams, TextStyle, TextSystem};

use super::*;
use crate::text_engine::raster::{RasterOptions, SubpixelOffset};

const TEXT: &str = "Hamburgefonstiv quick 0123 llll";
const PAD: f32 = 8.0;

fn refs() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/native_text/macos/refs")
}

/// A text system whose UI (or mono) family is `family`: the platform's
/// fonts for `system-ui`/`ui-monospace`, else the bundled one.
fn text_system(family: &str, kind: FontKind) -> TextSystem {
    let mut settings = FontSettings::default();
    match kind {
        FontKind::Ui => settings.ui_family = family.to_owned(),
        FontKind::Mono => settings.mono_family = family.to_owned(),
    }
    if family == SYSTEM_UI || family == UI_MONOSPACE || family == "Helvetica" {
        TextSystem::with_settings(&settings)
    } else {
        TextSystem::vendored_only(&settings)
    }
}

/// `text`'s glyphs as the renderer places them with the layout's origin at
/// `origin` device pixels.
fn shape(
    system: &mut TextSystem,
    text: &str,
    kind: FontKind,
    size: f32,
    weight: u16,
    scale: f32,
    origin: (f32, f32),
) -> Vec<PhysicalGlyph> {
    let mut style = TextStyle::new(size);
    style.font_kind = kind;
    style.font_weight = FontWeight::Numeric(weight);
    let mut params = TextParams::new(text, style);
    params.scale_factor = scale;
    let layout = system.layout(&params).expect("layout");
    layout
        .glyph_runs()
        .flat_map(|run| layout.physical_run(&run, origin).collect::<Vec<_>>())
        .collect()
}

/// A drawn mask in device pixels: top-left corner and coverage rows.
#[derive(Debug, Clone)]
struct Mask {
    left: i32,
    top: i32,
    width: usize,
    height: usize,
    pixels: Vec<u8>,
}

impl Mask {
    /// Ink bounds (left, top, right, bottom), y down, of coverage > 0.
    fn ink(&self) -> Option<(i32, i32, i32, i32)> {
        let (x0, y0, x1, y1) = ink_bounds(&self.pixels, self.width, self.height, 1)?;
        Some((
            self.left + x0 as i32,
            self.top + y0 as i32,
            self.left + x1 as i32,
            self.top + y1 as i32,
        ))
    }

    fn total(&self) -> f64 {
        self.pixels.iter().map(|&v| f64::from(v)).sum::<f64>() / 255.0
    }

    /// Ink-weighted mean x of the pixel centers.
    fn centroid_x(&self) -> f64 {
        let mut sum = 0.0;
        for (i, &v) in self.pixels.iter().enumerate() {
            sum += f64::from(v) * (self.left as f64 + (i % self.width) as f64 + 0.5);
        }
        sum / (self.total() * 255.0)
    }
}

/// CoreText's mask for the glyph keyed `key` (at integer origin `x`,
/// baseline `y`), or `None` for a blank glyph.
fn coretext_mask(
    raster: &mut CoreTextRasterizer,
    registry: &mut FontRegistry,
    key: &CacheKey,
    scale: f32,
    (x, y): (i32, i32),
) -> Option<Mask> {
    let font = registry
        .prepare(key.font_id, key.font_weight, key.flags)
        .expect("prepare")
        .clone();
    let request = RasterRequest::from_cache_key(key, scale).unwrap();
    let mut scratch = RasterScratch::default();
    match raster
        .rasterize(&font, &request, &mut scratch)
        .expect("rasterize")
    {
        RasterOutcome::Empty => None,
        RasterOutcome::Bitmap(bitmap) => {
            assert_eq!(bitmap.content, BitmapContent::Mask);
            let p = bitmap.placement;
            let mut pixels = Vec::new();
            for row in scratch.bitmap(&bitmap).chunks(bitmap.stride as usize) {
                pixels.extend_from_slice(&row[..p.width as usize]);
            }
            Some(Mask {
                left: x + p.left,
                top: y - p.top,
                width: p.width as usize,
                height: p.height as usize,
                pixels,
            })
        }
    }
}

/// quark's swash mask for the same glyph, from a fresh scaler context.
fn swash_mask(system: &mut TextSystem, key: &CacheKey, (x, y): (i32, i32)) -> Option<Mask> {
    let image = SwashCache::new().get_image_uncached(system.raster_font_system(), *key)?;
    assert!(matches!(image.content, SwashContent::Mask));
    let p = image.placement;
    (p.width > 0).then(|| Mask {
        left: x + p.left,
        top: y - p.top,
        width: p.width as usize,
        height: p.height as usize,
        pixels: image.data.clone(),
    })
}

fn with_bin(key: &CacheKey, bin: SubpixelBin) -> CacheKey {
    CacheKey { x_bin: bin, ..*key }
}

const BINS: [SubpixelBin; 4] = [
    SubpixelBin::Zero,
    SubpixelBin::One,
    SubpixelBin::Two,
    SubpixelBin::Three,
];

#[test]
fn masks_land_where_swash_places_the_same_glyph() {
    // Ascender, x-height, and descender ink, so a flipped row order or a
    // misplaced baseline moves some edge by far more than a pixel.
    let mut system = text_system("Inter", FontKind::Ui);
    let mut registry = FontRegistry::new(system.font_snapshot());
    let mut raster = CoreTextRasterizer::new(SmoothingProfile::Disabled);
    for scale in [1.0, 1.5, 2.0] {
        for glyph in shape(
            &mut system,
            "Hgy",
            FontKind::Ui,
            13.0,
            400,
            scale,
            (0.0, 0.0),
        ) {
            for bin in BINS {
                let key = with_bin(&glyph.cache_key, bin);
                let at = (glyph.x, glyph.y);
                let ct = coretext_mask(&mut raster, &mut registry, &key, scale, at)
                    .unwrap()
                    .ink()
                    .unwrap();
                let sw = swash_mask(&mut system, &key, at).unwrap().ink().unwrap();
                let off = [ct.0 - sw.0, ct.1 - sw.1, ct.2 - sw.2, ct.3 - sw.3];
                assert!(
                    off.iter().all(|d| d.abs() <= 1),
                    "glyph {} at {scale}x, bin {bin:?}: CoreText ink {ct:?}, swash {sw:?}",
                    key.glyph_id
                );
            }
        }
    }
}

#[test]
fn subpixel_bins_move_ink_by_quarter_pixels() {
    let mut system = text_system("Inter", FontKind::Ui);
    let mut registry = FontRegistry::new(system.font_snapshot());
    let mut raster = CoreTextRasterizer::new(SmoothingProfile::Disabled);
    for scale in [1.0, 1.5, 2.0] {
        let glyph = shape(&mut system, "l", FontKind::Ui, 13.0, 400, scale, (0.0, 0.0)).remove(0);
        let centroid = |raster: &mut CoreTextRasterizer, registry: &mut FontRegistry, bin| {
            let key = with_bin(&glyph.cache_key, bin);
            coretext_mask(raster, registry, &key, scale, (0, 0))
                .unwrap()
                .centroid_x()
        };
        let zero = centroid(&mut raster, &mut registry, SubpixelBin::Zero);
        let moved: Vec<f64> = BINS
            .into_iter()
            .map(|bin| centroid(&mut raster, &mut registry, bin) - zero)
            .collect();
        // CoreGraphics' antialiasing is not exact area coverage, so the
        // centroid wobbles by a few hundredths; a bin ignored, doubled, or
        // reversed moves it by a quarter pixel or more.
        for (i, step) in moved.iter().enumerate() {
            assert!(
                (step - i as f64 * 0.25).abs() < 0.1,
                "{scale}x: ink moved {moved:?} px for bins 0 to 3"
            );
        }
    }
}

/// A reference sheet's mode: the text and background grays CoreText drew
/// with.
struct Mode {
    name: &'static str,
    fg: f64,
    bg: f64,
}

/// Coverage CoreText drew into a reference pixel `c`.
fn recovered(mode: &Mode, c: u8) -> f64 {
    let (f, b) = ((mode.fg * 255.0).round(), (mode.bg * 255.0).round());
    ((f64::from(c) - b) / (f - b)).clamp(0.0, 1.0) * 255.0
}

/// How a line's per-glyph masks, composited over each other as CoreGraphics
/// draws, differ from the reference sheet's band. Pixels inked by exactly
/// one glyph compare directly; where glyphs overlap, compositing order and
/// rounding may differ by a code value.
#[derive(Debug, Default)]
struct BandDiff {
    max_single: f64,
    max_overlap: f64,
    mean: f64,
    ink_ratio: f64,
}

fn band_diff(
    sheet: &image::GrayImage,
    top: u32,
    height: u32,
    masks: &[Mask],
    mode: &Mode,
) -> BandDiff {
    let width = sheet.width() as usize;
    let mut ours = vec![0.0f64; width * height as usize];
    let mut owners = vec![0u8; width * height as usize];
    for m in masks {
        for row in 0..m.height {
            for col in 0..m.width {
                let (x, y) = (m.left + col as i32, m.top + row as i32 - top as i32);
                let a = f64::from(m.pixels[row * m.width + col]);
                if a == 0.0 || x < 0 || y < 0 || x as usize >= width || y as u32 >= height {
                    continue;
                }
                let i = y as usize * width + x as usize;
                ours[i] = a + ours[i] * (1.0 - a / 255.0);
                owners[i] = owners[i].saturating_add(1);
            }
        }
    }
    let mut diff = BandDiff::default();
    let (mut sum, mut n, mut ink_ours, mut ink_ref) = (0.0, 0usize, 0.0, 0.0);
    for y in 0..height as usize {
        for x in 0..width {
            let i = y * width + x;
            let theirs = recovered(mode, sheet.get_pixel(x as u32, top + y as u32)[0]);
            let d = (ours[i].round() - theirs.round()).abs();
            ink_ours += ours[i];
            ink_ref += theirs;
            if ours[i] > 0.0 || theirs > 0.0 {
                sum += d;
                n += 1;
            }
            if owners[i] <= 1 {
                diff.max_single = diff.max_single.max(d);
            } else {
                diff.max_overlap = diff.max_overlap.max(d);
            }
        }
    }
    diff.mean = sum / n.max(1) as f64;
    diff.ink_ratio = ink_ours / ink_ref;
    diff
}

/// Whether this Mac runs the OS build the references were drawn on, so
/// CoreText's output must match them exactly.
fn reference_os() -> bool {
    let env = std::fs::read_to_string(refs().join("env.json")).unwrap();
    let build = std::process::Command::new("sysctl")
        .args(["-n", "kern.osversion"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();
    !build.is_empty() && env.contains(&format!("(Build {build})"))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Shapes each case of reference `sheet` again, checks the glyphs and
/// positions are the manifest's, rasterizes them with `profile` (or the
/// bundle's plane `plane`), and compares with the sheet drawn in `mode`.
/// `None` when the installed system font is not the one the sheet drew.
fn compare_sheet(
    manifest: &Json,
    sheet_id: &str,
    mode: &Mode,
    profile: SmoothingProfile,
    plane: Option<usize>,
) -> Option<Vec<(String, BandDiff)>> {
    let sheet = manifest["sheets"]
        .array()
        .iter()
        .find(|s| s["id"].str() == sheet_id)
        .unwrap();
    let scale = sheet["scale"].num() as f32;
    let png = refs().join(format!("{sheet_id}.{}.png", mode.name));
    let image = image::open(&png)
        .unwrap_or_else(|e| panic!("{}: {e}", png.display()))
        .to_luma8();
    // Registries snapshot the fonts after the first line is shaped: shaping
    // loads an installed face's file, and a snapshot taken before holds no
    // bytes for it.
    let mut systems: HashMap<String, (TextSystem, Option<FontRegistry>)> = HashMap::new();
    let mut raster = CoreTextRasterizer::new(profile);
    let mut out = Vec::new();
    for case in sheet["cases"].array() {
        let font = case["font"].str();
        let (family, kind) = match font {
            "sf-pro" => (SYSTEM_UI, FontKind::Ui),
            "sf-mono" => (UI_MONOSPACE, FontKind::Mono),
            "inter" => ("Inter", FontKind::Ui),
            "geist" => ("Geist", FontKind::Ui),
            "jetbrains-mono" => ("JetBrains Mono", FontKind::Mono),
            other => panic!("unknown font {other}"),
        };
        let (system, registry) = systems
            .entry(font.to_owned())
            .or_insert_with(|| (text_system(family, kind), None));
        let band = case["band"].array();
        let (top, height) = (band[0].num() as u32, band[1].num() as u32);
        let glyphs = shape(
            system,
            TEXT,
            kind,
            case["size"].num() as f32,
            case["weight"].num() as u16,
            scale,
            (PAD, top as f32 + PAD),
        );
        let registry = registry.get_or_insert_with(|| FontRegistry::new(system.font_snapshot()));
        let expected = case["glyphs"].array();
        assert_eq!(
            glyphs.len(),
            expected.len(),
            "{}: glyph count",
            case["id"].str()
        );
        let mut masks = Vec::new();
        for (g, e) in glyphs.iter().zip(expected) {
            let e = e.array();
            let key = g.cache_key;
            let prepared = registry
                .prepare(key.font_id, key.font_weight, key.flags)
                .unwrap()
                .clone();
            // The installed system font must be the bytes the sheet drew.
            let entry = &manifest["fonts"].array()[e[0].num() as usize];
            if format!("{:016x}", fnv1a(prepared.source().data())) != entry["fnv1a"].str() {
                return None;
            }
            assert_eq!(
                (
                    u32::from(key.glyph_id),
                    g.x,
                    f64::from(key.x_bin.as_float()),
                    g.y
                ),
                (
                    e[1].num() as u32,
                    e[2].num() as i32,
                    e[3].num(),
                    e[4].num() as i32
                ),
                "{}: glyph {:?} is not where the sheet drew it",
                case["id"].str(),
                e[6].str()
            );
            let mask = match plane {
                None => coretext_mask(&mut raster, registry, &key, scale, (g.x, g.y)),
                Some(plane) => {
                    let request = RasterRequest::from_cache_key(&key, scale).unwrap();
                    let mut scratch = RasterScratch::default();
                    raster
                        .rasterize_bundle(&prepared, &request, &mut scratch)
                        .unwrap()
                        .map(|b| Mask {
                            left: g.x + b.placement.left,
                            top: g.y - b.placement.top,
                            width: b.placement.width as usize,
                            height: b.placement.height as usize,
                            pixels: scratch.pixels[b.planes[plane].clone()].to_vec(),
                        })
                }
            };
            masks.extend(mask);
        }
        out.push((
            case["id"].str().to_owned(),
            band_diff(&image, top, height, &masks, mode),
        ));
    }
    Some(out)
}

/// One line per sheet for the calibration report (`--nocapture`): the
/// worst of its lines.
fn summarize(sheet: &str, mode: &str, lines: &[(String, BandDiff)]) {
    let worst = |f: fn(&BandDiff) -> f64| lines.iter().map(|(_, d)| f(d)).fold(0.0, f64::max);
    let ratio = lines
        .iter()
        .map(|(_, d)| (d.ink_ratio - 1.0).abs())
        .fold(0.0, f64::max);
    eprintln!(
        "parity {sheet} {mode}: {} lines, max single-glyph diff {}, max overlap diff {}, worst mean diff {:.3}, worst ink ratio error {:.4}",
        lines.len(),
        worst(|d| d.max_single),
        worst(|d| d.max_overlap),
        worst(|d| d.mean),
        ratio
    );
}

#[test]
fn masks_match_coretexts_reference_sheets() {
    let manifest = Json::parse(&std::fs::read_to_string(refs().join("manifest.json")).unwrap());
    let exact = reference_os();
    let light_plain = Mode {
        name: "light-plain",
        fg: 0.0,
        bg: 1.0,
    };
    let light_smooth = Mode {
        name: "light-smooth",
        fg: 0.0,
        bg: 1.0,
    };
    let dark_smooth = Mode {
        name: "dark-smooth",
        fg: 1.0,
        bg: 0.0,
    };
    let cases: &[(&str, &Mode, SmoothingProfile)] = &[
        ("inter@1x", &light_plain, SmoothingProfile::Disabled),
        ("inter@2x", &light_plain, SmoothingProfile::Disabled),
        ("geist@1x", &light_plain, SmoothingProfile::Disabled),
        (
            "jetbrains-mono@1x",
            &light_plain,
            SmoothingProfile::Disabled,
        ),
        ("sf-pro@1x", &light_plain, SmoothingProfile::Disabled),
        ("sf-pro@2x", &light_plain, SmoothingProfile::Disabled),
        ("sf-mono@1x", &light_plain, SmoothingProfile::Disabled),
        (
            "inter@1x",
            &light_smooth,
            SmoothingProfile::Fixed(Foreground::BLACK),
        ),
        (
            "inter@2x",
            &dark_smooth,
            SmoothingProfile::Fixed(Foreground::WHITE),
        ),
        (
            "sf-pro@1x",
            &light_smooth,
            SmoothingProfile::Fixed(Foreground::BLACK),
        ),
        (
            "sf-pro@1x",
            &dark_smooth,
            SmoothingProfile::Fixed(Foreground::WHITE),
        ),
        (
            "sf-mono@1x",
            &dark_smooth,
            SmoothingProfile::Fixed(Foreground::WHITE),
        ),
    ];
    for &(sheet, mode, profile) in cases {
        let Some(lines) = compare_sheet(&manifest, sheet, mode, profile, None) else {
            eprintln!("{sheet}: the installed system font is not the reference's; skipped");
            continue;
        };
        summarize(sheet, mode.name, &lines);
        for (case, d) in lines {
            if exact {
                assert!(
                    d.max_single == 0.0 && d.max_overlap <= 1.0,
                    "{case} {}: {d:?} (the references' OS build must match exactly)",
                    mode.name
                );
            } else {
                assert!(
                    d.mean <= 3.0 && (d.ink_ratio - 1.0).abs() <= 0.02,
                    "{case} {}: {d:?}",
                    mode.name
                );
            }
        }
    }
}

#[test]
fn system_bundle_planes_match_each_text_gray() {
    let manifest = Json::parse(&std::fs::read_to_string(refs().join("manifest.json")).unwrap());
    let exact = reference_os();
    // The sweep's color pairs the bundle's grays are recovered against.
    let modes = [
        Mode {
            name: "fg0-bg100-smooth",
            fg: 0.0,
            bg: 1.0,
        },
        Mode {
            name: "fg25-bg100-smooth",
            fg: 0.25,
            bg: 1.0,
        },
        Mode {
            name: "fg50-bg0-smooth",
            fg: 0.5,
            bg: 0.0,
        },
        Mode {
            name: "fg75-bg0-smooth",
            fg: 0.75,
            bg: 0.0,
        },
        Mode {
            name: "fg100-bg0-smooth",
            fg: 1.0,
            bg: 0.0,
        },
    ];
    for sheet in ["sweep@1x", "sweep@2x"] {
        for (plane, mode) in modes.iter().enumerate() {
            let Some(lines) = compare_sheet(
                &manifest,
                sheet,
                mode,
                SmoothingProfile::System,
                Some(plane),
            ) else {
                eprintln!("{sheet}: the installed system font is not the reference's; skipped");
                break;
            };
            // Recovering coverage from intermediate grays scales a code
            // value by up to 255 / 191.
            let slack = if plane == 0 || plane == 4 { 0.0 } else { 2.0 };
            summarize(sheet, mode.name, &lines);
            for (case, d) in lines {
                if exact {
                    assert!(
                        d.max_single <= slack && d.max_overlap <= 1.0 + slack,
                        "{case} {}: {d:?}",
                        mode.name
                    );
                } else {
                    assert!(
                        d.mean <= 3.0 + slack && (d.ink_ratio - 1.0).abs() <= 0.02,
                        "{case} {}: {d:?}",
                        mode.name
                    );
                }
            }
        }
    }
}

/// Rasterizes the first glyph of `text` in `system`'s font with `flags`.
fn outcome(
    raster: &mut CoreTextRasterizer,
    system: &mut TextSystem,
    text: &str,
    flags: CacheKeyFlags,
) -> (Result<RasterOutcome, RasterError>, RasterScratch) {
    let glyph = shape(system, text, FontKind::Ui, 32.0, 400, 1.0, (0.0, 0.0)).remove(0);
    let mut registry = FontRegistry::new(system.font_snapshot());
    let key = CacheKey {
        flags,
        ..glyph.cache_key
    };
    let font = registry
        .prepare(key.font_id, key.font_weight, key.flags)
        .unwrap()
        .clone();
    let request = RasterRequest::from_cache_key(&key, 1.0).unwrap();
    let mut scratch = RasterScratch::default();
    (raster.rasterize(&font, &request, &mut scratch), scratch)
}

#[test]
fn emoji_draw_as_color_or_fall_back() {
    let mut raster = CoreTextRasterizer::new(SmoothingProfile::Disabled);
    // Apple Color Emoji (sbix) from the system, then quark's bundled Noto
    // Color Emoji (CBDT), which CoreText may refuse (the whole font, or the
    // glyph's format): either way swash draws it, never a gray mask.
    for (mut system, may_fall_back) in [
        (TextSystem::with_settings(&FontSettings::system()), false),
        (text_system("Inter", FontKind::Ui), true),
    ] {
        let (result, scratch) = outcome(&mut raster, &mut system, "😀", CacheKeyFlags::empty());
        match result {
            Ok(RasterOutcome::Bitmap(bitmap)) => {
                assert_eq!(
                    bitmap.content,
                    BitmapContent::Color {
                        alpha: AlphaMode::Premultiplied
                    }
                );
                let pixels = scratch.bitmap(&bitmap);
                let px = pixels.chunks(4);
                assert!(
                    px.clone()
                        .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]),
                    "not premultiplied"
                );
                assert!(px.clone().any(|p| p[3] == 255), "no opaque pixel");
                assert!(
                    px.clone()
                        .any(|p| p[0].abs_diff(p[1]) > 40 || p[1].abs_diff(p[2]) > 40),
                    "no color"
                );
            }
            Err(RasterError::UnsupportedFont | RasterError::UnsupportedFormat) if may_fall_back => {
            }
            other => panic!("emoji drew {other:?}"),
        }
    }
}

#[test]
fn blank_and_unknown_glyphs() {
    let mut system = text_system("Inter", FontKind::Ui);
    let mut registry = FontRegistry::new(system.font_snapshot());
    let mut raster = CoreTextRasterizer::new(SmoothingProfile::Disabled);
    let space = shape(&mut system, " ", FontKind::Ui, 13.0, 400, 1.0, (0.0, 0.0)).remove(0);
    let font = registry
        .prepare(
            space.cache_key.font_id,
            space.cache_key.font_weight,
            space.cache_key.flags,
        )
        .unwrap()
        .clone();
    let past_end = u16::try_from(font.glyph_count()).unwrap();
    let mut scratch = RasterScratch::default();
    for (glyph, expected) in [
        (space.cache_key.glyph_id, Ok(RasterOutcome::Empty)),
        (past_end, Err(RasterError::InvalidGlyph)),
    ] {
        let request = RasterRequest::new(
            glyph,
            13.0,
            1.0,
            SubpixelOffset::ZERO,
            RasterOptions::default(),
        )
        .unwrap();
        assert_eq!(
            raster.rasterize(&font, &request, &mut scratch),
            expected,
            "glyph {glyph}"
        );
    }
}

#[test]
fn a_collection_member_draws_its_own_outlines() {
    // Helvetica Bold is a later member of Helvetica.ttc; drawing member 0
    // (Helvetica) with the bold glyph ids would carry far less ink.
    let mut system = text_system("Helvetica", FontKind::Ui);
    let mut raster = CoreTextRasterizer::new(SmoothingProfile::Disabled);
    let glyphs = shape(
        &mut system,
        "Hamburg",
        FontKind::Ui,
        20.0,
        700,
        1.0,
        (0.0, 0.0),
    );
    let mut registry = FontRegistry::new(system.font_snapshot());
    let key = glyphs[0].cache_key;
    let font = registry
        .prepare(key.font_id, key.font_weight, key.flags)
        .unwrap()
        .clone();
    assert!(
        font.index() > 0,
        "Helvetica Bold should not be the collection's first face"
    );
    let (mut ct, mut sw) = (0.0, 0.0);
    for g in &glyphs {
        ct += coretext_mask(&mut raster, &mut registry, &g.cache_key, 1.0, (g.x, g.y))
            .map_or(0.0, |m| m.total());
        sw += swash_mask(&mut system, &g.cache_key, (g.x, g.y)).map_or(0.0, |m| m.total());
    }
    assert!(
        (ct / sw - 1.0).abs() < 0.08,
        "CoreText ink {ct:.1}, swash {sw:.1}"
    );
}

#[test]
fn synthetic_styles_reach_the_outline_like_swash() {
    // Italic slants the top right; thickening widens every edge by about
    // a fiftieth of the em. Both must agree with swash's geometry.
    let mut system = text_system("Inter", FontKind::Ui);
    let mut registry = FontRegistry::new(system.font_snapshot());
    let mut raster = CoreTextRasterizer::new(SmoothingProfile::Disabled);
    let glyph = shape(&mut system, "l", FontKind::Ui, 40.0, 400, 1.0, (0.0, 0.0)).remove(0);
    let plain = coretext_mask(&mut raster, &mut registry, &glyph.cache_key, 1.0, (0, 0)).unwrap();
    for flags in [CacheKeyFlags::FAKE_ITALIC, CacheKeyFlags::THICKEN] {
        let key = CacheKey {
            flags,
            ..glyph.cache_key
        };
        let ct = coretext_mask(&mut raster, &mut registry, &key, 1.0, (0, 0)).unwrap();
        let sw = swash_mask(&mut system, &key, (0, 0)).unwrap();
        let (c, s) = (ct.ink().unwrap(), sw.ink().unwrap());
        assert!(
            [c.0 - s.0, c.1 - s.1, c.2 - s.2, c.3 - s.3]
                .iter()
                .all(|d| d.abs() <= 1),
            "{flags:?}: CoreText ink {c:?}, swash {s:?}"
        );
        assert!(
            ct.width > plain.width + 1,
            "{flags:?} did not change the outline"
        );
    }
}

#[test]
fn rasterizers_on_other_threads_draw_the_same_masks() {
    let mut system = text_system("Inter", FontKind::Ui);
    let glyphs = shape(
        &mut system,
        "Hamburg",
        FontKind::Ui,
        13.0,
        400,
        2.0,
        (0.0, 0.0),
    );
    let snapshot = system.font_snapshot();
    let draw = move |snapshot: quark_text::FontSnapshot, glyphs: Vec<PhysicalGlyph>| {
        let mut registry = FontRegistry::new(snapshot);
        let mut raster = CoreTextRasterizer::new(SmoothingProfile::Fixed(Foreground::BLACK));
        glyphs
            .iter()
            .map(|g| {
                coretext_mask(&mut raster, &mut registry, &g.cache_key, 2.0, (0, 0))
                    .map(|m| m.pixels)
            })
            .collect::<Vec<_>>()
    };
    let here = draw(snapshot.clone(), glyphs.clone());
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let (snapshot, glyphs) = (snapshot.clone(), glyphs.clone());
            std::thread::spawn(move || draw(snapshot, glyphs))
        })
        .collect();
    for thread in threads {
        assert_eq!(thread.join().unwrap(), here);
    }
}

/// Just enough JSON for the reference manifest.
#[derive(Debug)]
enum Json {
    Null,
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn parse(text: &str) -> Self {
        Self::value(&mut text.chars().peekable())
    }

    fn value(c: &mut std::iter::Peekable<std::str::Chars>) -> Self {
        while c.peek().is_some_and(|ch| ch.is_whitespace()) {
            c.next();
        }
        match c.peek().copied() {
            Some('{') => {
                c.next();
                let mut fields = Vec::new();
                loop {
                    match Self::value(c) {
                        Json::Str(key) => {
                            Self::expect(c, ':');
                            fields.push((key, Self::value(c)));
                        }
                        Json::Null => {}
                        other => panic!("object key {other:?}"),
                    }
                    if !Self::separator(c, '}') {
                        break Json::Obj(fields);
                    }
                }
            }
            Some('[') => {
                c.next();
                let mut items = Vec::new();
                loop {
                    match Self::value(c) {
                        Json::Null => {}
                        item => items.push(item),
                    }
                    if !Self::separator(c, ']') {
                        break Json::Arr(items);
                    }
                }
            }
            Some('"') => {
                c.next();
                let mut s = String::new();
                while let Some(ch) = c.next() {
                    match ch {
                        '"' => break,
                        '\\' => match c.next() {
                            Some('n') => s.push('\n'),
                            Some('u') => {
                                let hex: String = c.by_ref().take(4).collect();
                                s.push(
                                    char::from_u32(u32::from_str_radix(&hex, 16).unwrap())
                                        .unwrap_or('?'),
                                );
                            }
                            Some(other) => s.push(other),
                            None => break,
                        },
                        ch => s.push(ch),
                    }
                }
                Json::Str(s)
            }
            Some(ch) if ch == '-' || ch.is_ascii_digit() => {
                let mut s = String::new();
                while c
                    .peek()
                    .is_some_and(|ch| "-+.eE".contains(*ch) || ch.is_ascii_digit())
                {
                    s.push(c.next().unwrap());
                }
                Json::Num(s.parse().unwrap())
            }
            Some('n') | Some('t') | Some('f') => {
                let word: String =
                    std::iter::from_fn(|| c.next_if(|ch| ch.is_alphabetic())).collect();
                match word.as_str() {
                    "true" => Json::Num(1.0),
                    "false" => Json::Num(0.0),
                    _ => Json::Null,
                }
            }
            // An empty array or object: the closing bracket ends it.
            _ => Json::Null,
        }
    }

    fn expect(c: &mut std::iter::Peekable<std::str::Chars>, ch: char) {
        while c.peek().is_some_and(|ch| ch.is_whitespace()) {
            c.next();
        }
        assert_eq!(c.next(), Some(ch));
    }

    /// Consumes a `,` (true: more follows) or `close` (false).
    fn separator(c: &mut std::iter::Peekable<std::str::Chars>, close: char) -> bool {
        while c.peek().is_some_and(|ch| ch.is_whitespace()) {
            c.next();
        }
        match c.next() {
            Some(',') => true,
            Some(ch) if ch == close => false,
            other => panic!("expected , or {close}, got {other:?}"),
        }
    }

    fn array(&self) -> &[Json] {
        match self {
            Json::Arr(items) => items,
            other => panic!("not an array: {other:?}"),
        }
    }

    fn str(&self) -> &str {
        match self {
            Json::Str(s) => s,
            other => panic!("not a string: {other:?}"),
        }
    }

    fn num(&self) -> f64 {
        match self {
            Json::Num(n) => *n,
            other => panic!("not a number: {other:?}"),
        }
    }
}

impl std::ops::Index<&str> for Json {
    type Output = Json;

    fn index(&self, key: &str) -> &Json {
        match self {
            Json::Obj(fields) => {
                &fields
                    .iter()
                    .find(|(k, _)| k == key)
                    .unwrap_or_else(|| panic!("no {key}"))
                    .1
            }
            other => panic!("not an object: {other:?}"),
        }
    }
}
