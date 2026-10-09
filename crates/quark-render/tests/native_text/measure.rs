//! Measurements of rendered frames: ink coverage of the pixels, frame time
//! summaries, and the atlas memory the renderer holds.

use quark_render::TextAtlasStats;

use crate::fixtures::Content;

/// Ink of an RGBA frame drawn on white: each pixel's ink is how far its
/// darkest channel falls below 255, so black text and colored emoji both
/// count.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Ink {
    /// `[left, top, right, bottom]`, exclusive, of every inked pixel.
    pub bounds: Option<[u32; 4]>,
    /// Pixels with any ink.
    pub pixels: u64,
    /// Pixels with ink but not full ink: the antialiased edge.
    pub partial: u64,
    /// Summed ink in whole-pixel units.
    pub total: f64,
}

pub fn ink(rgba: &[u8], width: u32) -> Ink {
    let mut out = Ink::default();
    let mut sum = 0u64;
    for (i, pixel) in rgba.as_chunks::<4>().0.iter().enumerate() {
        let darkest = pixel[0].min(pixel[1]).min(pixel[2]);
        let amount = 255 - darkest;
        if amount == 0 {
            continue;
        }
        let (x, y) = ((i as u32) % width, (i as u32) / width);
        out.pixels += 1;
        out.partial += u64::from(amount < 255);
        sum += u64::from(amount);
        let b = out.bounds.get_or_insert([x, y, x + 1, y + 1]);
        b[0] = b[0].min(x);
        b[1] = b[1].min(y);
        b[2] = b[2].max(x + 1);
        b[3] = b[3].max(y + 1);
    }
    out.total = sum as f64 / 255.0;
    out
}

/// How two frames of one size differ: pixels with any channel apart, and
/// the largest channel difference.
pub fn pixel_difference(a: &[u8], b: &[u8]) -> (u64, u8) {
    a.as_chunks::<4>()
        .0
        .iter()
        .zip(b.as_chunks::<4>().0)
        .fold((0, 0), |(count, max), (p, q)| {
            let delta = p.iter().zip(q).map(|(x, y)| x.abs_diff(*y)).max();
            let delta = delta.unwrap_or(0);
            (count + u64::from(delta > 0), max.max(delta))
        })
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Summary {
    pub median: f64,
    pub p95: f64,
    pub p99: f64,
}

pub fn summarize(samples: &mut [f64]) -> Summary {
    if samples.is_empty() {
        return Summary::default();
    }
    samples.sort_by(f64::total_cmp);
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    Summary {
        median: at(0.5),
        p95: at(0.95),
        p99: at(0.99),
    }
}

pub fn median(samples: &mut [u64]) -> u64 {
    samples.sort_unstable();
    samples.get(samples.len() / 2).copied().unwrap_or(0)
}

/// GPU memory of the glyph atlas: texture count and bytes.
///
/// The renderer on master (vendored glyphon) does not report its texture
/// sizes, so this derives them from its fixed policy: one mask (`R8`) and
/// one color (`RGBA8`) texture, each 256 pixels square at creation and
/// doubling per growth. A fixture keeps to one content kind, so its
/// renderer's growths all belong to that texture. Replace this with the
/// renderer's own page report once the paged atlas publishes one.
pub fn atlas_memory(stats: TextAtlasStats, content: Content) -> (u32, u64) {
    const INITIAL: u64 = 256;
    // Growth stops at the default device limit, 8192.
    let side = |growths: u64| INITIAL << growths.min(5);
    let (mask, color) = match content {
        Content::Mask => (side(stats.growths), INITIAL),
        Content::Color => (INITIAL, side(stats.growths)),
    };
    (2, mask * mask + 4 * color * color)
}
