//! What the inspector records while a frame lays out and paints, and how it
//! picks the element under a point afterwards.

use std::time::Instant;

use quark::hit::{CursorHint, HitFlags, HitTable};
use quark::{Color, ElementStyle, Rect, UiKey};

use super::StyleOverrides;
use crate::element::{Element, ElementContext};

/// What an element reports about itself beyond its bounds.
#[derive(Debug, Clone, Default)]
pub struct InspectInfo {
    /// The key style overrides are stored under, when the element has one.
    pub key: Option<UiKey>,
    /// The element's own z-index; 0 inherits its parent's.
    pub z_index: i32,
    pub blocks_mouse: bool,
    pub style: Option<StyleSummary>,
}

/// The style properties the inspector shows and edits.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StyleSummary {
    /// Left, right, top, bottom, in points. Percentages read as 0.
    pub padding: [f32; 4],
    /// Column gap, row gap.
    pub gap: [f32; 2],
    pub background: Option<Color>,
    pub border_color: Option<Color>,
    pub border_widths: [f32; 4],
    pub corner_radii: [f32; 4],
    pub opacity: f32,
}

impl StyleSummary {
    pub fn of(style: &ElementStyle) -> Self {
        let padding = style.layout.padding;
        let gap = style.layout.gap;
        Self {
            padding: [padding.left, padding.right, padding.top, padding.bottom].map(points),
            gap: [gap.width, gap.height].map(points),
            background: style.background,
            border_color: style.border_color,
            border_widths: style.border_widths,
            corner_radii: style.corner_radii,
            opacity: style.opacity,
        }
    }
}

fn points(length: taffy::LengthPercentage) -> f32 {
    let raw = length.into_raw();
    if raw.tag() == taffy::CompactLength::LENGTH_TAG {
        raw.value()
    } else {
        0.0
    }
}

/// One element of a painted frame, in paint order.
#[derive(Debug, Clone)]
pub struct ElementRecord {
    /// The element's type name, such as `Div` or `TextElement`.
    pub kind: &'static str,
    pub bounds: Rect,
    /// Intersection of the ancestor clips, as the hit table stores it.
    pub clip: Rect,
    pub z: i32,
    pub key: Option<UiKey>,
    /// The semantic node the element pushed, when it pushed one.
    pub semantic: Option<usize>,
    pub style: Option<StyleSummary>,
    blocks_mouse: bool,
}

/// Wall time of the layout and paint phases of one frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct PhaseTimings {
    pub layout_us: u64,
    /// Prepaint, hit test, and paint.
    pub paint_us: u64,
}

/// Devtools state that rides along in [`ElementContext`] for one frame.
#[derive(Debug, Default)]
pub struct FrameProbe {
    /// Record every element's bounds, clip, and z. Off unless an overlay
    /// needs them, so the cost when devtools are idle is one branch per
    /// element.
    pub recording: bool,
    pub overrides: StyleOverrides,
    pub phases: PhaseTimings,
    records: Vec<ElementRecord>,
}

impl FrameProbe {
    pub fn record_phases(&mut self, started: Instant, laid_out: Instant) {
        self.phases = PhaseTimings {
            layout_us: laid_out.duration_since(started).as_micros() as u64,
            paint_us: laid_out.elapsed().as_micros() as u64,
        };
    }

    /// The recorded frame, ready for picking.
    pub fn finish(&mut self) -> InspectFrame {
        InspectFrame::new(std::mem::take(&mut self.records))
    }
}

/// Applies the frame's style override for `element`, if any, before it
/// lays out.
pub fn apply_override<E: Element>(element: &mut E, cx: &mut ElementContext) {
    if cx.devtools.overrides.is_empty() {
        return;
    }
    if let Some((key, style)) = element.inspect_style_mut() {
        cx.devtools.overrides.apply(&key, style);
    }
}

/// Records `element` at the current clip and z. Called before the element
/// prepaints, so parents come before their children, as in the hit table.
pub fn record_prepaint<E: Element>(
    element: &E,
    bounds: Rect,
    cx: &mut ElementContext,
) -> Option<usize> {
    if !cx.devtools.recording {
        return None;
    }
    let info = element.inspect();
    // A non-zero z-index replaces the inherited one, as Div's prepaint does.
    let z = if info.z_index != 0 {
        info.z_index
    } else {
        cx.current_z_index()
    };
    let record = ElementRecord {
        kind: short_type_name(std::any::type_name::<E>()),
        bounds,
        clip: cx.current_clip(),
        z,
        key: info.key,
        semantic: None,
        style: info.style,
        blocks_mouse: info.blocks_mouse,
    };
    cx.devtools.records.push(record);
    Some(cx.devtools.records.len() - 1)
}

/// Links record `index` to the semantic node its element pushed during
/// paint. An element pushes its own node before any child's, so the first
/// node pushed with the element's bounds is its own.
pub fn record_paint(
    index: Option<usize>,
    semantic_before: usize,
    bounds: Rect,
    cx: &mut ElementContext,
) {
    let Some(index) = index else {
        return;
    };
    let own = cx
        .semantic
        .nodes()
        .get(semantic_before)
        .filter(|node| node.bounds == bounds)
        .map(|_| semantic_before);
    if let Some(record) = cx.devtools.records.get_mut(index) {
        record.semantic = own;
    }
}

fn short_type_name(full: &'static str) -> &'static str {
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}

/// Every recorded element of one frame, with a hit table over them so
/// picking honors clips, z, and mouse blockers exactly as input does.
#[derive(Debug, Default)]
pub struct InspectFrame {
    records: Vec<ElementRecord>,
    hits: HitTable,
}

impl InspectFrame {
    fn new(records: Vec<ElementRecord>) -> Self {
        let mut hits = HitTable::default();
        for record in &records {
            let flags = if record.blocks_mouse {
                HitFlags::BLOCKS_MOUSE
            } else {
                HitFlags::NONE
            };
            hits.push(
                record.bounds,
                record.clip,
                record.z,
                flags,
                CursorHint::Default,
            );
        }
        Self { records, hits }
    }

    pub fn records(&self) -> &[ElementRecord] {
        &self.records
    }

    /// The topmost element under `(x, y)`.
    pub fn pick(&self, x: f32, y: f32) -> Option<usize> {
        self.hits
            .stack_at(x, y)
            .first()
            .and_then(|id| self.hits.row(*id))
    }

    /// The element that pushed semantic node `node`.
    pub fn for_semantic(&self, node: usize) -> Option<usize> {
        self.records.iter().position(|r| r.semantic == Some(node))
    }

    /// The first element with override key `key`.
    pub fn for_key(&self, key: &UiKey) -> Option<usize> {
        self.records
            .iter()
            .position(|r| r.key.as_ref() == Some(key))
    }

    /// The innermost element with exactly these bounds.
    pub fn for_bounds(&self, bounds: Rect) -> Option<usize> {
        self.records.iter().rposition(|r| r.bounds == bounds)
    }
}
