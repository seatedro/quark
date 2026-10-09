//! `swash-dump OUT_DIR`: shapes the reference strings with quark-text,
//! rasterizes every glyph through quark's swash path, and writes:
//!
//! - `manifest.json`: sheets, cases (one text line each), the exact faces
//!   and variation coordinates swash used, and every glyph as
//!   `[font, glyph id, x, x bin, baseline y, advance, text]` in device
//!   pixels (integer x plus a quarter-pixel bin, integer baseline);
//! - `fonts/<hash>.font`: the bytes of each face's source, so the CoreText
//!   reference draws the same glyph ids from the same font data;
//! - `swash/<sheet>.png`: 8-bit coverage, glyphs placed as quark-render
//!   places them (the physical glyph's integer x and baseline, plus the
//!   swash bitmap's bearings).
//!
//! The positions are quark's, so ct_ref.swift draws the same quantized
//! offsets and the comparison isolates rasterization from shaping.

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use quark::{FontKind, FontWeight};
use quark_text::cosmic_text::{FontSystem, SwashCache, SwashContent, fontdb};
use quark_text::fonts::{SYSTEM_UI, UI_MONOSPACE};
use quark_text::{FontSettings, TextParams, TextStyle, TextSystem};

/// The line every case draws: round and straight strokes, figures, and a
/// run of `l` whose stems the comparison measures.
const TEXT: &str = "Hamburgefonstiv quick 0123 llll";
const SIZES: [f32; 5] = [11.0, 13.0, 17.0, 20.0, 28.0];
const WEIGHTS: [u16; 5] = [300, 400, 500, 600, 700];
const SCALES: [u32; 2] = [1, 2];
/// Horizontal and vertical padding around each line, in device pixels.
const PAD: i32 = 8;

struct FontSpec {
    id: &'static str,
    /// Loads installed fonts (and resolves the platform UI faces).
    system: bool,
    kind: FontKind,
    family: &'static str,
}

const FONTS: [FontSpec; 5] = [
    FontSpec {
        id: "sf-pro",
        system: true,
        kind: FontKind::Ui,
        family: SYSTEM_UI,
    },
    FontSpec {
        id: "sf-mono",
        system: true,
        kind: FontKind::Mono,
        family: UI_MONOSPACE,
    },
    FontSpec {
        id: "inter",
        system: false,
        kind: FontKind::Ui,
        family: "Inter",
    },
    FontSpec {
        id: "geist",
        system: false,
        kind: FontKind::Ui,
        family: "Geist",
    },
    FontSpec {
        id: "jetbrains-mono",
        system: false,
        kind: FontKind::Mono,
        family: "JetBrains Mono",
    },
];

/// The lines the on-screen panel draws (it must fit the 1024x768 display).
const PANEL: [(&str, f32, u16); 6] = [
    ("sf-pro", 11.0, 400),
    ("sf-pro", 13.0, 400),
    ("sf-pro", 17.0, 400),
    ("sf-pro", 20.0, 400),
    ("sf-pro", 13.0, 700),
    ("sf-mono", 13.0, 400),
];

/// The few lines the luminance sweep draws in many color pairs.
const SWEEP: [(&str, f32, u16); 4] = [
    ("sf-pro", 13.0, 400),
    ("sf-pro", 13.0, 700),
    ("sf-mono", 13.0, 400),
    ("inter", 13.0, 400),
];

struct Line {
    case: String,
    font: &'static str,
    size: f32,
    weight: u16,
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: swash-dump OUT_DIR"));
    std::fs::create_dir_all(out.join("fonts")).unwrap();
    std::fs::create_dir_all(out.join("swash")).unwrap();

    // One text system per font: settings pick the UI or mono family.
    let mut systems: HashMap<&'static str, TextSystem> = HashMap::new();
    for spec in &FONTS {
        let settings = FontSettings {
            ui_family: if spec.kind == FontKind::Ui { spec.family } else { SYSTEM_UI }.to_owned(),
            mono_family: if spec.kind == FontKind::Mono { spec.family } else { UI_MONOSPACE }
                .to_owned(),
            ..FontSettings::default()
        };
        let system = if spec.system {
            TextSystem::with_settings(&settings)
        } else {
            TextSystem::vendored_only(&settings)
        };
        systems.insert(spec.id, system);
    }

    let mut sheets: Vec<(String, &str, u32, Vec<Line>)> = Vec::new();
    for &scale in &SCALES {
        for spec in &FONTS {
            let lines = SIZES
                .iter()
                .flat_map(|&size| WEIGHTS.iter().map(move |&weight| (size, weight)))
                .map(|(size, weight)| Line {
                    case: format!("{}-{size}-{weight}@{scale}x", spec.id),
                    font: spec.id,
                    size,
                    weight,
                })
                .collect::<Vec<_>>();
            sheets.push((format!("{}@{scale}x", spec.id), "font", scale, lines));
        }
        for (kind, lines) in [("sweep", &SWEEP[..]), ("panel", &PANEL[..])] {
            let lines = lines
                .iter()
                .map(|&(font, size, weight)| Line {
                    case: format!("{kind}-{font}-{size}-{weight}@{scale}x"),
                    font,
                    size,
                    weight,
                })
                .collect();
            sheets.push((format!("{kind}@{scale}x"), kind, scale, lines));
        }
    }

    // SWASH_ONLY=a,b dumps only those sheets, in order.
    if let Ok(only) = std::env::var("SWASH_ONLY") {
        let only: Vec<&str> = only.split(',').collect();
        sheets.retain(|s| only.contains(&s.0.as_str()));
    }
    let mut fonts = FontTable::default();
    let mut swash = SwashCache::new();
    let mut json = String::from("{\n");
    writeln!(json, "  \"text\": {:?},", TEXT).unwrap();
    json.push_str("  \"sheets\": [\n");
    for (sheet_index, (sheet, kind, scale, lines)) in sheets.iter().enumerate() {
        let mut placed = Vec::new();
        let mut cases_json = Vec::new();
        let mut top = 0i32;
        let mut width = 0i32;
        for line in lines {
            let system = systems.get_mut(&line.font).unwrap();
            // A fresh scaler context per line: swash 0.2.10's hinting cache
            // keeps a stale instance when reconfiguring an evicted entry for
            // a face whose hinting fails (SF Pro after JetBrains Mono fills
            // it), so a shared context makes later lines order-dependent.
            // SWASH_SHARED=1 keeps one context, as glyphon does.
            if std::env::var_os("SWASH_SHARED").is_none() {
                swash = SwashCache::new();
            }
            let spec = FONTS.iter().find(|f| f.id == line.font).unwrap();
            let mut style = TextStyle::new(line.size);
            style.font_kind = spec.kind;
            style.font_weight = FontWeight::Numeric(line.weight);
            let mut params = TextParams::new(TEXT, style);
            params.scale_factor = *scale as f32;
            let layout = system.layout(&params).expect("layout");
            let band_top = top;
            let mut glyphs_json = Vec::new();
            let mut band_height = 0i32;
            let left = PAD as f32 + layout.buffer_x();
            let origin_y = (band_top + PAD) as f32;
            for run in layout.buffer().layout_runs() {
                band_height = band_height.max((run.line_top + run.line_height).ceil() as i32);
                let line_y = run.line_y.round() as i32;
                for glyph in run.glyphs {
                    // quark-render's placement: the buffer is already at
                    // physical size, so scale 1 here.
                    let physical = glyph.physical((left, origin_y), 1.0);
                    let key = physical.cache_key;
                    let baseline = physical.y + line_y;
                    let font = fonts.index(system.raster_font_system(), key.font_id, key.font_weight, &out);
                    let image = swash.get_image_uncached(system.raster_font_system(), key);
                    if let Some(image) = image {
                        placed.push((physical.x, baseline, image));
                    }
                    let ch = &TEXT[glyph.start..glyph.end];
                    glyphs_json.push(format!(
                        "[{font}, {}, {}, {}, {baseline}, {}, {:?}]",
                        key.glyph_id,
                        physical.x,
                        key.x_bin.as_float(),
                        glyph.w,
                        ch,
                    ));
                    width = width.max(physical.x + glyph.w.ceil() as i32 + PAD);
                }
            }
            let height = band_height + 2 * PAD;
            top += height;
            cases_json.push(format!(
                "        {{\"id\": {:?}, \"font\": {:?}, \"size\": {}, \"weight\": {}, \"band\": [{band_top}, {height}], \"glyphs\": [\n          {}\n        ]}}",
                line.case,
                line.font,
                line.size,
                line.weight,
                glyphs_json.join(",\n          "),
            ));
        }
        let (w, h) = (width as u32, top as u32);
        let mut coverage = vec![0u8; (w * h) as usize];
        for (x, baseline, image) in &placed {
            blit(&mut coverage, w, h, *x, *baseline, image);
        }
        write_png(&out.join("swash").join(format!("{sheet}.png")), w, h, &coverage);
        writeln!(
            json,
            "    {{\"id\": {sheet:?}, \"kind\": {kind:?}, \"scale\": {scale}, \"size\": [{w}, {h}], \"cases\": [\n{}\n    ]}}{}",
            cases_json.join(",\n"),
            if sheet_index + 1 < sheets.len() { "," } else { "" },
        )
        .unwrap();
    }
    json.push_str("  ],\n  \"fonts\": [\n");
    json.push_str(&fonts.json.join(",\n"));
    json.push_str("\n  ]\n}\n");
    std::fs::write(out.join("manifest.json"), json).unwrap();
}

/// Composites a swash image's coverage (a color glyph's alpha) over the
/// sheet: `a + o * (1 - a)`, the source-over CoreGraphics also applies.
fn blit(
    sheet: &mut [u8],
    w: u32,
    h: u32,
    x: i32,
    baseline: i32,
    image: &quark_text::cosmic_text::SwashImage,
) {
    let p = image.placement;
    let (stride, channel) = match image.content {
        SwashContent::Mask => (1usize, 0usize),
        SwashContent::Color => (4, 3),
        SwashContent::SubpixelMask => panic!("swash returned an LCD mask"),
    };
    for row in 0..p.height as i32 {
        for col in 0..p.width as i32 {
            let (sx, sy) = (x + p.left + col, baseline - p.top + row);
            if sx < 0 || sy < 0 || sx >= w as i32 || sy >= h as i32 {
                continue;
            }
            let a = image.data[(row as usize * p.width as usize + col as usize) * stride + channel] as u32;
            let o = &mut sheet[(sy as u32 * w + sx as u32) as usize];
            *o = (a + *o as u32 - (a * *o as u32 + 127) / 255) as u8;
        }
    }
}

fn write_png(path: &Path, w: u32, h: u32, data: &[u8]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, w, h);
    encoder.set_color(png::ColorType::Grayscale);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(data).unwrap();
}

/// Each distinct face (and the weight its variable `wght` axis was set
/// to) once, with its source bytes written to `fonts/`.
#[derive(Default)]
struct FontTable {
    index: HashMap<(fontdb::ID, u16), usize>,
    written: HashMap<u64, ()>,
    json: Vec<String>,
}

impl FontTable {
    fn index(
        &mut self,
        system: &mut FontSystem,
        id: fontdb::ID,
        weight: fontdb::Weight,
        out: &Path,
    ) -> usize {
        if let Some(&i) = self.index.get(&(id, weight.0)) {
            return i;
        }
        let (bytes, face_index) = system
            .db()
            .with_face_data(id, |data, index| (data.to_vec(), index))
            .expect("face data");
        let hash = fnv1a(&bytes);
        let file = format!("fonts/{hash:016x}.font");
        if self.written.insert(hash, ()).is_none() {
            std::fs::write(out.join(&file), &bytes).unwrap();
        }
        let face = system.db().face(id).expect("face");
        let source = match &face.source {
            fontdb::Source::File(path) | fontdb::Source::SharedFile(path, _) => {
                path.display().to_string()
            }
            fontdb::Source::Binary(_) => "bundled".to_owned(),
        };
        let post_script = face.post_script_name.clone();
        // The coordinates swash rasterizes with: `wght` from the cache key
        // clamped to the axis, every other axis at its default.
        let font = system.get_font(id, weight).expect("font");
        let axes: Vec<String> = font
            .as_swash()
            .variations()
            .map(|axis| {
                let tag = axis.tag().to_be_bytes();
                let tag = String::from_utf8_lossy(&tag).into_owned();
                let value = if tag == "wght" {
                    f32::from(weight.0).clamp(axis.min_value(), axis.max_value())
                } else {
                    axis.default_value()
                };
                format!(
                    "{{\"tag\": {tag:?}, \"min\": {}, \"default\": {}, \"max\": {}, \"value\": {value}}}",
                    axis.min_value(),
                    axis.default_value(),
                    axis.max_value(),
                )
            })
            .collect();
        let i = self.json.len();
        self.json.push(format!(
            "    {{\"file\": {file:?}, \"index\": {face_index}, \"bytes\": {}, \"fnv1a\": \"{hash:016x}\", \"source\": {source:?}, \"post_script\": {post_script:?}, \"weight\": {}, \"axes\": [{}]}}",
            bytes.len(),
            weight.0,
            axes.join(", "),
        ));
        self.index.insert((id, weight.0), i);
        i
    }
}

fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    })
}
