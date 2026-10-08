//! Cell metrics in device pixels, derived from the monospace face the way
//! Ghostty's `font/Metrics.zig` does: the cell is the face's advance by its
//! line height (ascent, descent, and line gap), each rounded to whole
//! pixels, with the baseline centered in the rounded height and the
//! underline, strikethrough, and box-drawing strokes taken from the font's
//! own decoration metrics.

use std::fmt;
use std::str::FromStr;

/// A face's vertical and decoration metrics at a pixel size, with +Y up
/// and positions relative to the baseline (Ghostty's `FaceMetrics`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FaceMetrics {
    /// Advance of the widest ASCII glyph, in pixels.
    pub cell_width: f64,
    pub ascent: f64,
    /// Below the baseline, so usually negative.
    pub descent: f64,
    pub line_gap: f64,
    /// Top of the underline stroke.
    pub underline_position: Option<f64>,
    pub underline_thickness: Option<f64>,
    /// Top of the strikethrough stroke.
    pub strikethrough_position: Option<f64>,
    pub strikethrough_thickness: Option<f64>,
    pub cap_height: Option<f64>,
    pub ex_height: Option<f64>,
}

impl FaceMetrics {
    /// The face's metrics at `px_per_em` pixels, read from its tables as
    /// FreeType (and Ghostty) choose them: typo metrics when the font asks
    /// for them, hhea otherwise. A zero underline or strikethrough
    /// thickness marks broken metrics, which fall back to estimates.
    pub fn from_font(
        font: &quark_text::cosmic_text::Font,
        px_per_em: f64,
        cell_width: f64,
    ) -> Self {
        let m = font.metrics();
        let unit = px_per_em / f64::from(m.units_per_em.max(1));
        let px = |v: f32| f64::from(v) * unit;
        let decoration = |d: Option<quark_text::cosmic_text::skrifa::metrics::Decoration>| match d {
            Some(d) if d.thickness != 0.0 => (Some(px(d.offset)), Some(px(d.thickness))),
            Some(d) if d.offset != 0.0 => (Some(px(d.offset)), None),
            _ => (None, None),
        };
        let (underline_position, underline_thickness) = decoration(m.underline);
        let (strikethrough_position, strikethrough_thickness) = decoration(m.strikeout);
        Self {
            cell_width,
            ascent: px(m.ascent),
            descent: px(m.descent),
            line_gap: px(m.leading),
            underline_position,
            underline_thickness,
            strikethrough_position,
            strikethrough_thickness,
            cap_height: m.cap_height.filter(|v| *v > 0.0).map(px),
            ex_height: m.x_height.filter(|v| *v > 0.0).map(px),
        }
    }

    fn line_height(&self) -> f64 {
        self.ascent - self.descent + self.line_gap
    }

    fn cap_height(&self) -> f64 {
        self.cap_height.unwrap_or(0.75 * self.ascent)
    }

    fn ex_height(&self) -> f64 {
        self.ex_height.unwrap_or(0.75 * self.cap_height())
    }

    fn underline_thickness(&self) -> f64 {
        self.underline_thickness
            .filter(|v| *v > 0.0)
            .unwrap_or(0.15 * self.ex_height())
    }

    fn strikethrough_thickness(&self) -> f64 {
        self.strikethrough_thickness
            .filter(|v| *v > 0.0)
            .unwrap_or_else(|| self.underline_thickness())
    }

    fn underline_position(&self) -> f64 {
        self.underline_position
            .unwrap_or(-self.underline_thickness())
    }

    fn strikethrough_position(&self) -> f64 {
        self.strikethrough_position
            .unwrap_or((self.ex_height() + self.strikethrough_thickness()) * 0.5)
    }
}

/// The terminal grid's metrics in whole device pixels (Ghostty's
/// `font.Metrics`). Positions marked "from the top" count down from the
/// cell's top edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct CellMetrics {
    pub cell_width: u32,
    pub cell_height: u32,
    /// From the bottom of the cell up to the baseline.
    pub cell_baseline: u32,
    /// Top of the underline, from the top.
    pub underline_position: u32,
    pub underline_thickness: u32,
    /// Top of the strikethrough, from the top.
    pub strikethrough_position: u32,
    pub strikethrough_thickness: u32,
    /// Top of the overline, from the top.
    pub overline_position: i32,
    pub overline_thickness: u32,
    /// Light box-drawing stroke width.
    pub box_thickness: u32,
    pub cursor_thickness: u32,
    pub cursor_height: u32,
}

impl CellMetrics {
    /// Ghostty's `Metrics.calc`, followed by the cell height adjustment.
    pub fn new(face: &FaceMetrics, adjust_cell_height: Option<MetricModifier>) -> Self {
        let face_height = face.line_height();
        let cell_width = face.cell_width.round();
        let cell_height = face_height.round();
        // Half the line gap goes above the text and half below; the face is
        // then centered in the rounded height.
        let face_baseline = face.line_gap / 2.0 - face.descent;
        let cell_baseline = (face_baseline - (cell_height - face_height) / 2.0).round();
        let face_y = cell_baseline - face_baseline;
        let top_to_baseline = cell_height - cell_baseline;
        let underline_thickness = face.underline_thickness().ceil().max(1.0);
        let strikethrough_thickness = face.strikethrough_thickness().ceil().max(1.0);
        let underline_position = (top_to_baseline - face.underline_position()).round();
        let strikethrough_position = (top_to_baseline - face.strikethrough_position()).round();
        let px = |v: f64| v.max(0.0) as u32;
        let mut m = Self {
            cell_width: px(cell_width).max(1),
            cell_height: px(cell_height).max(1),
            cell_baseline: px(cell_baseline),
            underline_position: px(underline_position),
            underline_thickness: px(underline_thickness),
            strikethrough_position: px(strikethrough_position),
            strikethrough_thickness: px(strikethrough_thickness),
            overline_position: 0,
            overline_thickness: px(underline_thickness),
            box_thickness: px(underline_thickness),
            cursor_thickness: 1,
            cursor_height: px(cell_height).max(1),
        };
        if let Some(adjust) = adjust_cell_height {
            m.adjust_cell_height(adjust, face_y, face_height);
        }
        m
    }

    /// Changes the cell height and keeps the text centered in it, putting
    /// an odd pixel where the face sits furthest from center (Ghostty's
    /// `Metrics.apply` for `cell_height`). The cursor keeps the face's
    /// height.
    fn adjust_cell_height(&mut self, adjust: MetricModifier, face_y: f64, face_height: f64) {
        let original = self.cell_height;
        let new = adjust.apply(original).max(1);
        if new == original {
            return;
        }
        let (original, new_f) = (f64::from(original), f64::from(new));
        let half = (new_f - original) / 2.0;
        let off_center = face_y - (original - face_height) / 2.0;
        let (top, bottom) = if off_center > 0.0 {
            (half.ceil(), half.floor())
        } else {
            (half.floor(), half.ceil())
        };
        let add = |v: u32, d: f64| (f64::from(v) + d).max(0.0) as u32;
        self.cell_height = new;
        self.cell_baseline = add(self.cell_baseline, bottom);
        self.underline_position = add(self.underline_position, top);
        self.strikethrough_position = add(self.strikethrough_position, top);
        self.overline_position += top as i32;
    }

    /// From the top of the cell down to the baseline.
    pub fn baseline_from_top(&self) -> u32 {
        self.cell_height.saturating_sub(self.cell_baseline)
    }
}

/// A change to a metric, as Ghostty's `adjust-*` options write it: `20%`
/// grows it by a fifth, `-2` shrinks it by two pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MetricModifier {
    /// Multiplies the metric (`1.2` for `20%`).
    Percent(f64),
    /// Adds whole pixels.
    Absolute(i32),
}

impl MetricModifier {
    pub(crate) fn apply(self, v: u32) -> u32 {
        match self {
            Self::Percent(p) => (f64::from(v) * p.max(0.0)).round() as u32,
            Self::Absolute(a) => (i64::from(v) + i64::from(a)).clamp(0, i64::from(u32::MAX)) as u32,
        }
    }
}

impl std::hash::Hash for MetricModifier {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        match *self {
            Self::Percent(p) => (0u8, p.to_bits()).hash(state),
            Self::Absolute(a) => (1u8, a as u64).hash(state),
        }
    }
}

/// The modifier could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidModifier(pub String);

impl fmt::Display for InvalidModifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid metric modifier {:?}", self.0)
    }
}

impl std::error::Error for InvalidModifier {}

impl FromStr for MetricModifier {
    type Err = InvalidModifier;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        let bad = || InvalidModifier(s.to_owned());
        if let Some(percent) = s.strip_suffix('%') {
            let p: f64 = percent.trim().parse().map_err(|_| bad())?;
            if !p.is_finite() {
                return Err(bad());
            }
            return Ok(Self::Percent((1.0 + p / 100.0).max(0.0)));
        }
        s.parse().map(Self::Absolute).map_err(|_| bad())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1000-unit face at 26 px per em (13pt on a 2x display): ascent
    /// 1020, descent -300, no line gap, a 50-unit underline at -150 and
    /// strikeout at 320.
    fn jetbrains_26px() -> FaceMetrics {
        let unit = 26.0 / 1000.0;
        FaceMetrics {
            cell_width: 600.0 * unit,
            ascent: 1020.0 * unit,
            descent: -300.0 * unit,
            line_gap: 0.0,
            underline_position: Some(-150.0 * unit),
            underline_thickness: Some(50.0 * unit),
            strikethrough_position: Some(320.0 * unit),
            strikethrough_thickness: Some(50.0 * unit),
            cap_height: Some(730.0 * unit),
            ex_height: Some(550.0 * unit),
        }
    }

    fn dump(m: &CellMetrics) -> String {
        format!(
            "cell {}x{} baseline {} underline {}+{} strike {}+{} box {}",
            m.cell_width,
            m.cell_height,
            m.cell_baseline,
            m.underline_position,
            m.underline_thickness,
            m.strikethrough_position,
            m.strikethrough_thickness,
            m.box_thickness
        )
    }

    /// The numbers Ghostty's `Metrics.calc` gives for the same face: the
    /// cell is the rounded advance by the rounded line height, and the
    /// strokes are the font's decoration thickness rounded up.
    #[test]
    fn cell_metrics_come_from_the_face() {
        let m = CellMetrics::new(&jetbrains_26px(), None);
        assert_eq!(
            dump(&m),
            "cell 16x34 baseline 8 underline 30+2 strike 18+2 box 2"
        );
    }

    /// `adjust-cell-height` resizes the cell and keeps the text centered:
    /// an even change splits evenly above and below the baseline, and the
    /// odd pixel of 50% goes on top, where this face sits higher.
    #[test]
    fn adjusting_the_cell_height_keeps_the_text_centered() {
        let face = jetbrains_26px();
        let table = [
            (
                "4",
                "cell 16x38 baseline 10 underline 32+2 strike 20+2 box 2",
            ),
            (
                "-2",
                "cell 16x32 baseline 7 underline 29+2 strike 17+2 box 2",
            ),
            (
                "50%",
                "cell 16x51 baseline 16 underline 39+2 strike 27+2 box 2",
            ),
        ];
        for (adjust, expected) in table {
            let m = CellMetrics::new(&face, Some(adjust.parse().unwrap()));
            assert_eq!(dump(&m), expected, "adjust-cell-height = {adjust}");
        }
    }

    #[test]
    fn modifiers_parse_like_ghostty() {
        let table = [
            ("20%", Ok(MetricModifier::Percent(1.2))),
            ("-150%", Ok(MetricModifier::Percent(0.0))),
            ("3", Ok(MetricModifier::Absolute(3))),
            (" -1 ", Ok(MetricModifier::Absolute(-1))),
            ("x", Err(())),
            ("", Err(())),
        ];
        for (input, expected) in table {
            assert_eq!(
                input.parse::<MetricModifier>().map_err(|_| ()),
                expected,
                "{input:?}"
            );
        }
    }
}
