use super::*;

// ---------------------------------------------------------------------------
// Text measurement
// ---------------------------------------------------------------------------

/// Measure text width using glyphon's real shaping — the same engine that
/// renders the text on the GPU.  This eliminates the mismatch between layout
/// estimates and actual rendered glyph widths.
pub(crate) fn measure_text_width(
    font_system: &mut glyphon::FontSystem,
    text: &str,
    font_size: f32,
    font_kind: FontKind,
    font_weight: FontWeight,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    // Ceil to avoid sub-pixel truncation. `line_w` is the line advance width, not the
    // visible ink bounds — bounding boxes (`glyph.x + glyph.w`) under-measure strings
    // and make glyphon clip button labels into garbled fragments in the GPU renderer.
    shaped_line_w(font_system, text, font_size, font_kind, font_weight).ceil()
}

/// Like `measure_text_width` but counts a trailing space's advance, which `line_w`
/// trims. Used to position adjacent styled pieces: a span that ends in a space must
/// advance the pen by that space, or the next piece abuts the previous word. Done by
/// appending a sentinel (so the trailing space becomes interior) and subtracting it.
pub(crate) fn measure_text_advance(
    font_system: &mut glyphon::FontSystem,
    text: &str,
    font_size: f32,
    font_kind: FontKind,
    font_weight: FontWeight,
) -> f32 {
    if text.is_empty() {
        return 0.0;
    }
    let probe = format!("{text}.");
    let with = shaped_line_w(font_system, &probe, font_size, font_kind, font_weight);
    let sentinel = shaped_line_w(font_system, ".", font_size, font_kind, font_weight);
    (with - sentinel).max(0.0).ceil()
}

/// Shapes `text` on a single unbounded line and returns its raw `line_w` advance.
fn shaped_line_w(
    font_system: &mut glyphon::FontSystem,
    text: &str,
    font_size: f32,
    font_kind: FontKind,
    font_weight: FontWeight,
) -> f32 {
    let metrics = glyphon::Metrics::new(font_size, font_size * 1.2);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);

    let family = match font_kind {
        FontKind::Ui => glyphon::Family::SansSerif,
        FontKind::Mono => glyphon::Family::Monospace,
    };
    let weight = glyphon_weight_for_font(font_kind, font_weight);
    let attrs = glyphon::Attrs::new().family(family).weight(weight);

    buffer.set_size(font_system, None, None);
    buffer.set_text(font_system, text, &attrs, glyphon::Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    buffer
        .layout_runs()
        .fold(0.0f32, |width, run| width.max(run.line_w))
}

/// Word-wrap `text` to fit within `max_width`. Returns the visual lines in
/// order, each as an owned string (sliced from the source by glyph byte
/// ranges). Honours `max_lines`: extra content collapses into the final line
/// so it can be truncated or ellipsised by the caller.
pub(crate) fn wrap_text_to_lines(
    font_system: &mut glyphon::FontSystem,
    text: &str,
    font_size: f32,
    font_kind: FontKind,
    font_weight: FontWeight,
    max_width: f32,
    max_lines: usize,
) -> Vec<String> {
    if text.is_empty() || max_lines == 0 {
        return Vec::new();
    }

    let metrics = glyphon::Metrics::new(font_size, font_size * 1.35);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);

    let family = match font_kind {
        FontKind::Ui => glyphon::Family::SansSerif,
        FontKind::Mono => glyphon::Family::Monospace,
    };
    let weight = glyphon_weight_for_font(font_kind, font_weight);
    let attrs = glyphon::Attrs::new().family(family).weight(weight);

    buffer.set_size(font_system, Some(max_width.max(1.0)), None);
    buffer.set_text(font_system, text, &attrs, glyphon::Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    let mut lines: Vec<String> = Vec::new();
    let runs: Vec<_> = buffer.layout_runs().collect();
    for (i, run) in runs.iter().enumerate() {
        if lines.len() + 1 >= max_lines && i + 1 < runs.len() {
            // Last allowed line: absorb the remaining runs so the caller can
            // truncate if desired.
            let start = run.glyphs.first().map(|g| g.start).unwrap_or(0);
            let end = runs
                .last()
                .and_then(|r| r.glyphs.last())
                .map(|g| g.end)
                .unwrap_or(run.text.len());
            lines.push(run.text[start..end.min(run.text.len())].to_owned());
            break;
        }
        let start = run.glyphs.first().map(|g| g.start).unwrap_or(0);
        let end = run
            .glyphs
            .last()
            .map(|g| g.end)
            .unwrap_or_else(|| run.text.len());
        lines.push(run.text[start..end.min(run.text.len())].to_owned());
    }

    if lines.is_empty() {
        lines.push(text.to_owned());
    }
    lines
}

/// One visual (wrapped) line of a shaped paragraph: the byte range `[start, end)`
/// into the source string and the y offset to the top of the line. Used by
/// `SelectableText` to render, hit-test, and highlight from a single shaping so
/// every consumer agrees on where each character is.
#[derive(Debug, Clone, Copy)]
pub struct WrappedRun {
    pub start: usize,
    pub end: usize,
    pub line_top: f32,
}

/// Shapes a styled paragraph via `set_rich_text` so
/// each span uses its own font, then returns the visual lines' byte ranges into
/// the concatenated plain string (`spans` joined). Mixed mono/UI runs wrap at the
/// correct points; the byte ranges line up with selection/copy, which operate on
/// that same concatenation.
pub(crate) fn wrap_rich_text_to_runs(
    font_system: &mut glyphon::FontSystem,
    spans: &[StyledSpan],
    font_size: f32,
    base_kind: FontKind,
    base_weight: FontWeight,
    max_width: f32,
    max_lines: usize,
) -> Vec<WrappedRun> {
    let total: usize = spans.iter().map(|s| s.text.len()).sum();
    if total == 0 || max_lines == 0 {
        return Vec::new();
    }

    let metrics = glyphon::Metrics::new(font_size, font_size * 1.35);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);

    let attr_list: Vec<(&str, glyphon::Attrs)> = spans
        .iter()
        .map(|s| {
            let family = match s.font_kind {
                FontKind::Ui => glyphon::Family::SansSerif,
                FontKind::Mono => glyphon::Family::Monospace,
            };
            let mut attrs = glyphon::Attrs::new()
                .family(family)
                .weight(glyphon_weight_for_font(s.font_kind, s.font_weight));
            if s.italic {
                attrs = attrs.style(glyphon::Style::Italic);
            }
            (s.text.as_str(), attrs)
        })
        .collect();

    let default_family = match base_kind {
        FontKind::Ui => glyphon::Family::SansSerif,
        FontKind::Mono => glyphon::Family::Monospace,
    };
    let default_attrs = glyphon::Attrs::new()
        .family(default_family)
        .weight(glyphon_weight_for_font(base_kind, base_weight));

    buffer.set_size(font_system, Some(max_width.max(1.0)), None);
    buffer.set_rich_text(
        font_system,
        attr_list.iter().map(|(t, a)| (*t, a.clone())),
        &default_attrs,
        glyphon::Shaping::Advanced,
        None,
    );
    buffer.shape_until_scroll(font_system, false);

    let mut out: Vec<WrappedRun> = Vec::new();
    let layout: Vec<_> = buffer.layout_runs().collect();
    for (i, run) in layout.iter().enumerate() {
        let start = run.glyphs.first().map(|g| g.start).unwrap_or(0);
        if out.len() + 1 >= max_lines && i + 1 < layout.len() {
            let end = layout
                .last()
                .and_then(|r| r.glyphs.last())
                .map(|g| g.end)
                .unwrap_or(total);
            out.push(WrappedRun {
                start,
                end: end.min(total),
                line_top: run.line_top,
            });
            break;
        }
        let end = run.glyphs.last().map(|g| g.end).unwrap_or(total);
        out.push(WrappedRun {
            start,
            end: end.min(total),
            line_top: run.line_top,
        });
    }
    if out.is_empty() {
        out.push(WrappedRun {
            start: 0,
            end: total,
            line_top: 0.0,
        });
    }
    out
}

fn glyphon_weight_for_font(font_kind: FontKind, font_weight: FontWeight) -> glyphon::Weight {
    match (font_kind, font_weight) {
        (FontKind::Ui, FontWeight::Normal) => glyphon::Weight(450),
        (_, FontWeight::Normal) => glyphon::Weight::NORMAL,
        (_, FontWeight::Medium) => glyphon::Weight(500),
        (_, FontWeight::Semibold) => glyphon::Weight(600),
        (_, FontWeight::Bold) => glyphon::Weight::BOLD,
    }
}

pub(super) fn font_kind_measure_tag(font_kind: FontKind) -> u8 {
    match font_kind {
        FontKind::Ui => 0,
        FontKind::Mono => 1,
    }
}

pub(super) fn font_weight_measure_tag(font_weight: FontWeight) -> u8 {
    match font_weight {
        FontWeight::Normal => 0,
        FontWeight::Medium => 1,
        FontWeight::Semibold => 2,
        FontWeight::Bold => 3,
    }
}

pub(super) fn truncate_text_to_fit(
    cx: &mut ElementContext<'_>,
    text: &str,
    font_size: f32,
    font_kind: FontKind,
    font_weight: FontWeight,
    full_width: f32,
    max_width: f32,
) -> (String, f32) {
    const ELLIPSIS: &str = "\u{2026}";

    if text.is_empty() || max_width <= 0.0 {
        return (String::new(), 0.0);
    }

    if full_width <= max_width {
        return (text.to_owned(), full_width);
    }

    let ellipsis_width = cx.measure_text_width(ELLIPSIS, font_size, font_kind, font_weight);
    if ellipsis_width > max_width {
        return (String::new(), 0.0);
    }

    let mut boundaries = Vec::with_capacity(text.chars().count() + 1);
    boundaries.push(0);
    boundaries.extend(text.char_indices().skip(1).map(|(idx, _)| idx));
    boundaries.push(text.len());

    let char_count = boundaries.len() - 1;
    let mut low = 0usize;
    let mut high = char_count;
    let mut best_width = ellipsis_width;

    while low < high {
        let mid = (low + high + 1) / 2;
        let prefix = &text[..boundaries[mid]];
        let mut candidate = String::with_capacity(prefix.len() + ELLIPSIS.len());
        candidate.push_str(prefix);
        candidate.push_str(ELLIPSIS);
        let candidate_width = cx.measure_text_width(&candidate, font_size, font_kind, font_weight);
        if candidate_width <= max_width {
            low = mid;
            best_width = candidate_width;
        } else {
            high = mid - 1;
        }
    }

    let prefix = &text[..boundaries[low]];
    let mut truncated = String::with_capacity(prefix.len() + ELLIPSIS.len());
    truncated.push_str(prefix);
    truncated.push_str(ELLIPSIS);
    let truncated_width = if low == 0 { ellipsis_width } else { best_width };
    (truncated, truncated_width)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_text_width_matches_layout_run_width() {
        let mut font_system = glyphon::FontSystem::new();
        let text = "Open Compare";
        let font_size = 12.0;

        let measured = measure_text_width(
            &mut font_system,
            text,
            font_size,
            FontKind::Ui,
            FontWeight::Normal,
        );

        let metrics = glyphon::Metrics::new(font_size, font_size * 1.2);
        let mut buffer = glyphon::Buffer::new(&mut font_system, metrics);
        let attrs = glyphon::Attrs::new()
            .family(glyphon::Family::SansSerif)
            .weight(glyphon::Weight(450));
        buffer.set_size(&mut font_system, None, None);
        buffer.set_text(
            &mut font_system,
            text,
            &attrs,
            glyphon::Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);

        let line_width = buffer
            .layout_runs()
            .fold(0.0f32, |width, run| width.max(run.line_w))
            .ceil();

        assert!(
            (measured - line_width).abs() < 1.0,
            "measured width {measured} should track glyphon line width {line_width}",
        );
    }

    #[test]
    fn measure_text_width_accounts_for_font_weight() {
        let mut font_system = glyphon::FontSystem::new();
        quark_render::fonts::configure_font_system(&mut font_system);
        let text = "Open Compare";
        let font_size = 12.0;

        let measured = measure_text_width(
            &mut font_system,
            text,
            font_size,
            FontKind::Ui,
            FontWeight::Medium,
        );

        let metrics = glyphon::Metrics::new(font_size, font_size * 1.2);
        let mut buffer = glyphon::Buffer::new(&mut font_system, metrics);
        let attrs = glyphon::Attrs::new()
            .family(glyphon::Family::SansSerif)
            .weight(glyphon::Weight(500));
        buffer.set_size(&mut font_system, None, None);
        buffer.set_text(
            &mut font_system,
            text,
            &attrs,
            glyphon::Shaping::Advanced,
            None,
        );
        buffer.shape_until_scroll(&mut font_system, false);

        let line_width = buffer
            .layout_runs()
            .fold(0.0f32, |width, run| width.max(run.line_w))
            .ceil();

        assert!(
            (measured - line_width).abs() < 1.0,
            "measured width {measured} should track medium-weight glyphon line width {line_width}",
        );
    }

    #[test]
    fn wrap_text_to_runs_covers_source_in_order() {
        let mut font_system = glyphon::FontSystem::new();
        let text = "the quick brown fox jumps over the lazy dog repeatedly today";
        // Narrow width forces several wrapped runs.
        let runs = wrap_rich_text_to_runs(
            &mut font_system,
            &[StyledSpan::plain(text)],
            13.0,
            FontKind::Ui,
            FontWeight::Normal,
            80.0,
            64,
        );
        assert!(runs.len() > 1, "narrow width should wrap to multiple runs");
        // Byte ranges are valid, ordered, and non-overlapping; line tops ascend.
        let mut prev_end = 0usize;
        let mut prev_top = -1.0f32;
        for run in &runs {
            assert!(run.start <= run.end && run.end <= text.len());
            assert!(text.is_char_boundary(run.start) && text.is_char_boundary(run.end));
            assert!(run.start >= prev_end, "runs must not overlap");
            assert!(run.line_top > prev_top, "line tops must ascend");
            prev_end = run.end;
            prev_top = run.line_top;
        }
    }

    #[test]
    fn wrap_text_to_runs_single_line_spans_full_text() {
        let mut font_system = glyphon::FontSystem::new();
        let text = "short";
        let runs = wrap_rich_text_to_runs(
            &mut font_system,
            &[StyledSpan::plain(text)],
            13.0,
            FontKind::Ui,
            FontWeight::Normal,
            10_000.0,
            64,
        );
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start, 0);
        assert_eq!(runs[0].end, text.len());
    }
}
