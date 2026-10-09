//! The report: one tab-separated row per fixture, scale, and rasterizer,
//! and a comparison of two reports (baseline and candidate) against the
//! design's budgets.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::measure::{Ink, Summary, pixel_difference};

/// Measurements of one fixture at one scale through one rasterizer.
pub struct Row {
    pub fixture: &'static str,
    pub scale: f32,
    pub backend: &'static str,
    pub width: u32,
    pub height: u32,
    pub glyphs: usize,
    /// The first frame of a new renderer: glyphs rasterized, bitmap bytes
    /// uploaded, allocations, and wall time.
    pub cold_misses: u64,
    pub cold_upload_bytes: u64,
    pub cold_allocs: u64,
    pub cold_ms: f64,
    /// Summed over every repeated frame; both should be zero.
    pub warm_misses: u64,
    pub warm_upload_bytes: u64,
    /// Median allocations of a repeated frame, and of the same frame
    /// without text.
    pub warm_allocs: u64,
    pub control_allocs: u64,
    pub warm: Summary,
    pub control: Summary,
    pub evictions: u64,
    pub growths: u64,
    pub atlas_textures: u32,
    pub atlas_bytes: u64,
    pub ink: Ink,
}

impl Row {
    pub fn key(&self) -> String {
        format!("{}@{}/{}", self.fixture, self.scale, self.backend)
    }

    fn columns(&self) -> Vec<(&'static str, String)> {
        let bounds = match self.ink.bounds {
            Some([l, t, r, b]) => format!("{l},{t},{r},{b}"),
            None => "-".to_owned(),
        };
        vec![
            ("key", self.key()),
            ("size", format!("{}x{}", self.width, self.height)),
            ("glyphs", self.glyphs.to_string()),
            ("cold_misses", self.cold_misses.to_string()),
            ("cold_upload_bytes", self.cold_upload_bytes.to_string()),
            ("cold_allocs", self.cold_allocs.to_string()),
            ("cold_ms", format!("{:.3}", self.cold_ms)),
            ("warm_misses", self.warm_misses.to_string()),
            ("warm_upload_bytes", self.warm_upload_bytes.to_string()),
            ("warm_allocs", self.warm_allocs.to_string()),
            ("control_allocs", self.control_allocs.to_string()),
            ("warm_ms", format!("{:.3}", self.warm.median)),
            ("warm_ms_p95", format!("{:.3}", self.warm.p95)),
            ("warm_ms_p99", format!("{:.3}", self.warm.p99)),
            ("control_ms", format!("{:.3}", self.control.median)),
            ("evictions", self.evictions.to_string()),
            ("growths", self.growths.to_string()),
            ("atlas_textures", self.atlas_textures.to_string()),
            ("atlas_bytes", self.atlas_bytes.to_string()),
            ("ink_pixels", self.ink.pixels.to_string()),
            ("ink_partial", self.ink.partial.to_string()),
            ("ink_total", format!("{:.1}", self.ink.total)),
            ("ink_bounds", bounds),
        ]
    }
}

/// The report text: `# name: value` header lines, a column header, then
/// one row per measurement.
pub fn render(header: &[(&str, String)], rows: &[Row]) -> String {
    let mut out = String::new();
    for (name, value) in header {
        let _ = writeln!(out, "# {name}: {value}");
    }
    if let Some(first) = rows.first() {
        let names: Vec<_> = first.columns().into_iter().map(|(n, _)| n).collect();
        let _ = writeln!(out, "{}", names.join("\t"));
    }
    for row in rows {
        let values: Vec<_> = row.columns().into_iter().map(|(_, v)| v).collect();
        let _ = writeln!(out, "{}", values.join("\t"));
    }
    out
}

/// A parsed report: rows by key, each a map of column to value. Columns
/// either side lacks are skipped when comparing, so reports from before a
/// column existed still compare.
pub type Parsed = BTreeMap<String, BTreeMap<String, String>>;

pub fn parse(text: &str) -> Parsed {
    let mut lines = text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.is_empty());
    let Some(header) = lines.next() else {
        return Parsed::new();
    };
    let names: Vec<_> = header.split('\t').collect();
    lines
        .filter_map(|line| {
            let row: BTreeMap<_, _> = names
                .iter()
                .zip(line.split('\t'))
                .map(|(n, v)| ((*n).to_owned(), v.to_owned()))
                .collect();
            Some((row.get("key")?.clone(), row))
        })
        .collect()
}

/// Whether a warm frame time regressed past the design's promotion gate:
/// more than 5% or 0.1 ms slower, whichever is larger.
pub fn time_regressed(baseline_ms: f64, candidate_ms: f64) -> bool {
    candidate_ms - baseline_ms > (baseline_ms * 0.05).max(0.1)
}

/// Compare `candidate` against `baseline`, one line per row, and return
/// the lines plus every gate failure. Images are read from the two report
/// directories when both have the row's frame.
pub fn compare(
    baseline: &Parsed,
    candidate: &Parsed,
    baseline_dir: &Path,
    candidate_dir: &Path,
) -> (Vec<String>, Vec<String>) {
    let mut lines = Vec::new();
    let mut failures = Vec::new();
    for (key, cand) in candidate {
        let num = |row: &BTreeMap<String, String>, column: &str| -> Option<f64> {
            row.get(column)?.parse().ok()
        };
        for column in ["warm_misses", "warm_upload_bytes"] {
            if num(cand, column).is_some_and(|v| v > 0.0) {
                failures.push(format!("{key}: repeated frames have {column} > 0"));
            }
        }
        // A row compares against the same fixture and scale in the
        // baseline, from any rasterizer, so a native backend compares with
        // swash's baseline.
        let stem = key.split('/').next().unwrap_or(key);
        let Some((base_key, base)) = baseline.iter().find(|(k, _)| *k == key).or_else(|| {
            baseline
                .iter()
                .find(|(k, _)| k.split('/').next() == Some(stem))
        }) else {
            lines.push(format!("{key}: no baseline row"));
            continue;
        };
        let mut line = format!("{key} vs {base_key}:");
        let pair = |column: &str| Some((num(base, column)?, num(cand, column)?));
        // Text's share of a frame: the frame less the same frame without
        // text.
        let text_ms = |row| Some(num(row, "warm_ms")? - num(row, "control_ms")?);
        if let (Some((b, c)), Some(bt), Some(ct)) = (pair("warm_ms"), text_ms(base), text_ms(cand))
        {
            let _ = write!(line, " warm {b:.3}->{c:.3} ms (text {bt:.3}->{ct:.3})");
            if time_regressed(b, c) {
                failures.push(format!("{key}: warm frame {b:.3} -> {c:.3} ms"));
            }
        }
        let text_allocs = |row| Some(num(row, "warm_allocs")? - num(row, "control_allocs")?);
        if let (Some(b), Some(c)) = (text_allocs(base), text_allocs(cand)) {
            let _ = write!(line, "; text allocs {b}->{c}");
            if c > b {
                failures.push(format!("{key}: repeated frame text allocations {b} -> {c}"));
            }
        }
        for column in ["cold_misses", "cold_upload_bytes", "atlas_bytes"] {
            if let Some((b, c)) = pair(column) {
                let _ = write!(line, "; {column} {b}->{c}");
            }
        }
        if let Some((b, c)) = pair("ink_total") {
            let change = if b > 0.0 { (c - b) / b * 100.0 } else { 0.0 };
            let _ = write!(line, "; ink {b:.1}->{c:.1} ({change:+.2}%)");
        }
        if let (Some(b), Some(c)) = (base.get("ink_bounds"), cand.get("ink_bounds"))
            && b != c
        {
            let _ = write!(line, "; ink bounds {b}->{c}");
        }
        if let Some((count, max)) = image_difference(
            &baseline_dir.join(image_name(base_key)),
            &candidate_dir.join(image_name(key)),
        ) {
            let _ = write!(line, "; {count} pixels differ (max {max})");
        }
        lines.push(line);
    }
    (lines, failures)
}

/// Where a row's frame is saved in its report directory, by row key.
pub fn image_name(key: &str) -> String {
    format!("{}.png", key.replacen('/', "-", 1))
}

fn image_difference(a: &Path, b: &Path) -> Option<(u64, u8)> {
    let a = image::open(a).ok()?.into_rgba8();
    let b = image::open(b).ok()?.into_rgba8();
    (a.dimensions() == b.dimensions()).then(|| pixel_difference(a.as_raw(), b.as_raw()))
}

// The promotion gate tolerates 5% or 0.1 ms, whichever is larger: a tenth
// of a millisecond on fast frames, a twentieth on slow ones.
#[test]
fn a_warm_frame_regresses_past_five_percent_or_a_tenth_of_a_millisecond() {
    for (baseline, candidate, regressed) in [
        (1.0, 1.09, false),
        (1.0, 1.11, true),
        (10.0, 10.45, false),
        (10.0, 10.55, true),
        (10.0, 9.0, false),
    ] {
        assert_eq!(
            time_regressed(baseline, candidate),
            regressed,
            "{baseline} -> {candidate}"
        );
    }
}
