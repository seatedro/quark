use quark::{SemanticRole, view};

use quark_ui::Action;
use quark_ui::design::{Ico, Rad, Sp};
use quark_ui::element::CursorHint;
use quark_ui::element::*;
use quark_ui::style::Styled;
use quark_ui::theme::{Color, Theme, ThemeColors};
use quark_ui::virtual_list::virtual_list_total_extent;

/// A row the picker can show. Only `label` and `detail` are required.
pub trait PickerItem {
    fn label(&self) -> &str;
    fn detail(&self) -> Option<&str>;
    fn label_style(&self) -> PickerLabelStyle {
        PickerLabelStyle::Default
    }
    /// Byte ranges of `label` drawn in the accent color (fuzzy-match hits).
    fn highlight_ranges(&self) -> &[(usize, usize)] {
        &[]
    }
    fn icon_svg(&self) -> Option<&'static str> {
        None
    }
    fn is_section_header(&self) -> bool {
        false
    }
    fn rhs(&self) -> Option<&str> {
        None
    }
    fn is_disabled(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PickerLabelStyle {
    #[default]
    Default,
    /// Identifier label whose first `prefix_len` bytes (the unique short
    /// prefix) are bold and colored; `emphasized` brightens the prefix.
    IdPrefix { prefix_len: usize, emphasized: bool },
}

pub fn picker_list<T: PickerItem>(
    entries: &[T],
    selected_index: usize,
    scroll_top_px: f32,
    max_visible: usize,
    theme: &Theme,
    on_select: impl Fn(usize) -> Action,
    on_scroll: ScrollActionBuilder,
) -> AnyElement {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    let row_h = theme.metrics.ui_row_height.round();
    let gap = (Sp::XS * scale).round();
    let icon_size = Ico::XS;
    let visible_count = entries.len().min(max_visible);
    let list_h = virtual_list_total_extent(visible_count, row_h, gap);
    let total_h = virtual_list_total_extent(entries.len(), row_h, gap);
    let scroll = scroll_top_px.min((total_h - list_h).max(0.0));

    view! { scale,
        <div class="w-full flex-col" gap={Sp::XS} h={list_h}
             id="picker-list"
             test-id="picker-list"
             semantic_role={SemanticRole::ScrollArea}
             accessibility_role={accesskit::Role::ListBox}
             accessibility_id={"picker-list"}
             overflow_hidden scroll_y={scroll} scroll_total={total_h}
             on_scroll={on_scroll}
             hide_scrollbar>
            for (i, entry) in entries.iter().enumerate() {
                if entry.is_section_header() {
                    <div class="w-full flex-row items-center" h={row_h} px={Sp::MD}>
                        <text class="text-xs truncate" color={tc.text_muted}>{entry.label()}</text>
                    </div>
                } else {
                    {picker_row(i, entry, selected_index, row_h, icon_size, theme, on_select(i))}
                }
            }
        </div>
    }
}

fn picker_row<T: PickerItem>(
    i: usize,
    entry: &T,
    selected_index: usize,
    row_h: f32,
    icon_size: f32,
    theme: &Theme,
    on_select: Action,
) -> AnyElement {
    let tc = &theme.colors;
    let scale = theme.metrics.ui_scale();
    let selected = i == selected_index;
    let row_bg = if selected {
        tc.sidebar_row_selected
    } else {
        Color::TRANSPARENT
    };
    let disabled = entry.is_disabled();
    view! { scale,
        <div class="w-full shrink-0 flex-row items-center"
             id={format!("picker-row:{i}:{}", entry.label())}
             key={format!("{i}:{}", entry.label())}
             test-id="picker-row"
             role="option"
             h={row_h} gap={Sp::SM} px={Sp::MD} rounded={Rad::MD}
             bg={row_bg}
             @when {!selected && !disabled} { hover_bg={tc.sidebar_row_hover} }
             on:click={on_select}
             hit_identity={HitIdentity::OverlayEntry(i)}
             accessibility_id={format!("picker-row:{i}:{}", entry.label())}
             aria-label={entry.label()}
             aria-selected={selected}
             aria-disabled={disabled}
             cursor={CursorHint::Pointer}>
            if let Some(svg) = entry.icon_svg() {
                <icon svg={svg} size={icon_size} color={tc.icon} />
            }
            {picker_label(
                entry.label(),
                entry.label_style(),
                entry.highlight_ranges(),
                selected,
                theme
            )}
            if let Some(detail) = entry.detail().filter(|d| !d.is_empty()) {
                <text class="text-xs" color={tc.text_muted} class="truncate">{detail}</text>
            }
            if let Some(rhs) = entry.rhs().filter(|d| !d.is_empty()) {
                <text class="text-xs" color={tc.text_muted}>{rhs}</text>
            }
        </div>
    }
}

fn picker_label(
    label_text: &str,
    label_style: PickerLabelStyle,
    highlights: &[(usize, usize)],
    selected: bool,
    theme: &Theme,
) -> AnyElement {
    let tc = &theme.colors;
    let base_color = if selected { tc.text_strong } else { tc.text };

    if let PickerLabelStyle::IdPrefix {
        prefix_len,
        emphasized,
    } = label_style
    {
        return id_prefix_label(label_text, prefix_len, emphasized, highlights, tc);
    }

    if highlights.is_empty() {
        return view! {
            <div class="flex-1 overflow-hidden">
                <text class="text-sm truncate" color={base_color}>{label_text}</text>
            </div>
        };
    }

    let mut spans = Vec::new();
    let mut cursor = 0;
    for &(start, end) in highlights {
        if start >= end || end > label_text.len() {
            continue;
        }
        if cursor < start {
            spans.push((&label_text[cursor..start], base_color));
        }
        spans.push((&label_text[start..end], tc.accent));
        cursor = end;
    }
    if cursor < label_text.len() {
        spans.push((&label_text[cursor..], base_color));
    }

    view! {
        <div class="flex-1 overflow-hidden">
            <div class="flex-row overflow-hidden">
                for (segment, color) in spans {
                    <text class="text-sm" color={color}>{segment}</text>
                }
            </div>
        </div>
    }
}

fn id_prefix_label(
    label_text: &str,
    prefix_len: usize,
    emphasized: bool,
    highlights: &[(usize, usize)],
    tc: &ThemeColors,
) -> AnyElement {
    let split = prefix_len.min(label_text.len());
    let split = if label_text.is_char_boundary(split) {
        split
    } else {
        0
    };
    let (prefix, rest) = label_text.split_at(split);
    let prefix_color = if emphasized {
        tc.syntax_keyword.lerp(tc.text_strong, 0.28)
    } else {
        tc.syntax_keyword
    };

    if highlights.is_empty() {
        return view! {
            <div class="flex-1 overflow-hidden">
                <div class="flex-row overflow-hidden">
                    <text class="text-sm font-bold" color={prefix_color}>{prefix}</text>
                    <text class="text-sm" color={tc.text_muted}>{rest}</text>
                </div>
            </div>
        };
    }

    let mut spans: Vec<AnyElement> = Vec::new();
    let mut cursor = 0;
    let mut push_segment = |start: usize, end: usize, highlighted: bool| {
        if start >= end {
            return;
        }
        let mut segment_start = start;
        let color = if highlighted { tc.accent } else { prefix_color };
        if segment_start < split {
            let segment_end = end.min(split);
            spans.push(view! {
                <text class="text-sm font-bold" color={color}>
                    {&label_text[segment_start..segment_end]}
                </text>
            });
            segment_start = segment_end;
        }
        if segment_start < end {
            let color = if highlighted {
                tc.accent
            } else {
                tc.text_muted
            };
            spans.push(view! {
                <text class="text-sm" color={color}>{&label_text[segment_start..end]}</text>
            });
        }
    };

    for &(start, end) in highlights {
        if start >= end || end > label_text.len() {
            continue;
        }
        if cursor < start {
            push_segment(cursor, start, false);
        }
        push_segment(start, end, true);
        cursor = end;
    }
    if cursor < label_text.len() {
        push_segment(cursor, label_text.len(), false);
    }

    view! {
        <div class="flex-1 overflow-hidden">
            <div class="flex-row overflow-hidden">
                for span in spans {
                    {span}
                }
            </div>
        </div>
    }
}
