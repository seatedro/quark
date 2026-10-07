use super::*;

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
