// Byte slicing of strings lives in `quark_text::offset`.
#![deny(clippy::string_slice, clippy::indexing_slicing)]

use super::*;
use quark_text::TextOffset;
use quark_text::offset;

// ---------------------------------------------------------------------------
// Text measurement helpers (layouts come from `ElementContext::layout_text`)
// ---------------------------------------------------------------------------

pub(super) fn truncate_text_to_fit(
    cx: &mut ElementContext<'_>,
    text: &str,
    font_size: f32,
    font_kind: FontKind,
    font_weight: FontWeight,
    full_width: f32,
    max_width: f32,
) -> (String, f32) {
    let style = TextStyle::new(font_size)
        .kind(font_kind)
        .weight(font_weight);
    truncate_text_to_fit_styled(cx, text, style, full_width, max_width)
}

/// The longest grapheme prefix of `text` that fits `max_width` with an
/// ellipsis, measured in `style`, and its width; `text` itself when its
/// `full_width` fits.
pub(super) fn truncate_text_to_fit_styled(
    cx: &mut ElementContext<'_>,
    text: &str,
    style: TextStyle,
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

    let ellipsis_width = cx.measure_text_width_styled(ELLIPSIS, style);
    if ellipsis_width > max_width {
        return (String::new(), 0.0);
    }

    // Cut only between graphemes, so an accent or a joined emoji is never
    // split from its base.
    let boundaries: Vec<TextOffset> = offset::grapheme_boundaries(text).collect();
    let with_ellipsis = |end: TextOffset| {
        let prefix = offset::prefix(text, end);
        let mut candidate = String::with_capacity(prefix.len() + ELLIPSIS.len());
        candidate.push_str(prefix);
        candidate.push_str(ELLIPSIS);
        candidate
    };

    // Largest number of graphemes whose prefix plus the ellipsis fits.
    let mut low = 0usize;
    let mut high = boundaries.len().saturating_sub(1);
    let mut best_width = ellipsis_width;
    while low < high {
        let mid = (low + high).div_ceil(2);
        let end = boundaries.get(mid).copied().unwrap_or_default();
        let candidate_width = cx.measure_text_width_styled(&with_ellipsis(end), style);
        if candidate_width <= max_width {
            low = mid;
            best_width = candidate_width;
        } else {
            high = mid - 1;
        }
    }

    let truncated = with_ellipsis(boundaries.get(low).copied().unwrap_or_default());
    let truncated_width = if low == 0 { ellipsis_width } else { best_width };
    (truncated, truncated_width)
}

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_segmentation::UnicodeSegmentation;

    // Regression: truncation cut between chars, so a narrow column showed
    // the first person of a ZWJ family emoji followed by the ellipsis.
    #[test]
    fn truncation_keeps_whole_graphemes() {
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut text_system = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx =
            ElementContext::new(&theme, 1.0, &mut text_system, &mut layouts, None, &signals);
        let family = "\u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467}";
        let text = format!("ab{family}{family}e\u{301}e\u{301}");
        let measure = |cx: &mut ElementContext, s: &str| {
            cx.measure_text_width(s, 14.0, FontKind::Ui, FontWeight::Normal)
        };
        let full = measure(&mut cx, &text);
        let boundaries: Vec<usize> = text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain([text.len()])
            .collect();
        for step in 1..80 {
            let max_width = full * step as f32 / 80.0;
            let (truncated, _) = truncate_text_to_fit(
                &mut cx,
                &text,
                14.0,
                FontKind::Ui,
                FontWeight::Normal,
                full,
                max_width,
            );
            let kept = truncated.strip_suffix('\u{2026}').unwrap_or(&truncated);
            assert!(
                text.starts_with(kept) && boundaries.contains(&kept.len()),
                "{max_width}: {truncated:?}"
            );
        }
    }

    // Regression: truncation measured without tracking, so tracked text
    // cut to "fit" overflowed its column once painted with the tracking.
    #[test]
    fn truncation_fits_with_the_style_letter_spacing() {
        let theme = Theme::default_dark();
        let signals = SignalStore::new();
        let mut text_system = quark_text::TextSystem::vendored_only(&Default::default());
        let mut layouts = quark_text::LayoutCache::default();
        let mut cx =
            ElementContext::new(&theme, 1.0, &mut text_system, &mut layouts, None, &signals);
        let tracked = TextStyle::new(14.0).letter_spacing(0.3);
        let text = "Workspace settings";
        let full = cx.measure_text_width_styled(text, tracked);
        let max_width = full * 0.6;

        let (truncated, width) =
            truncate_text_to_fit_styled(&mut cx, text, tracked, full, max_width);

        assert!(truncated.ends_with('\u{2026}'), "{truncated:?}");
        assert!(width <= max_width, "{width} > {max_width}");
        assert_eq!(cx.measure_text_width_styled(&truncated, tracked), width);
    }
}
