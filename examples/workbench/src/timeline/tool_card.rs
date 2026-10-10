//! Tool cards: a 36-point header with chevron, icon, verb, target, and
//! outcome, over the tool's output as an ordinary code block, so the output
//! selects and copies with the rest of the transcript.
//!
//! The header is a row adornment. Collapsing the card empties the row's
//! markdown, which takes the output out of the document (and out of any
//! selection) while the model keeps it; the header, and so keyboard focus
//! on its disclosure, stays.

use std::hash::{DefaultHasher, Hash, Hasher};

use accesskit::Role as AccessibilityRole;
use quark::view;
use quark_app::quark_ui::document::{AdornmentSlot, RowAdornment, RowChrome};
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::{Action, FocusId};
use quark_components::{Button, ButtonSize, ButtonStyle};

use super::Action as TimelineAction;
use super::rows::{HEADER, KIND_TOOL, RowContent};
use crate::contracts::ToolId;
use crate::design::tokens;
use crate::model::{Row, ToolCall, ToolKind, ToolStatus};

/// Height of a tool card's header.
pub const TOOL_HEADER: f32 = 36.0;

/// Whether a card shows its output when the user has not chosen: running
/// and failed tools expand, finished ones collapse.
pub fn expanded_by_default(status: ToolStatus) -> bool {
    status != ToolStatus::Ok
}

/// The stable id of a card's disclosure, which keeps focus while the card
/// collapses and expands.
pub fn disclosure_id(tool: ToolId) -> String {
    format!("timeline.tool:{}", tool.0)
}

pub fn disclosure_focus(tool: ToolId) -> FocusId {
    FocusId::from_key(&disclosure_id(tool))
}

/// The status word a card's header and accessible name use.
pub fn outcome(call: &ToolCall) -> &'static str {
    match call.status {
        ToolStatus::Running => "running",
        ToolStatus::Ok => "succeeded",
        ToolStatus::Failed => "failed",
    }
}

/// The card's markdown: the output in a fence while expanded, nothing
/// while collapsed.
pub fn markdown(call: &ToolCall, expanded: bool) -> String {
    if !expanded || call.output.is_empty() {
        return String::new();
    }
    // A fence longer than any backtick run in the output, so output that
    // holds a fence of its own stays inside this one.
    let longest = call
        .output
        .split(|c| c != '`')
        .map(str::len)
        .max()
        .unwrap_or(0);
    let fence = "`".repeat(longest.max(2) + 1);
    format!(
        "{fence}output\n{}\n{fence}",
        call.output.trim_end_matches('\n')
    )
}

pub fn content(row: &Row, call: &ToolCall, expanded: bool) -> RowContent {
    let tool = ToolId(row.id.0);
    let name = format!("{} {}, {}", call.verb, call.target, outcome(call));
    RowContent {
        markdown: markdown(call, expanded),
        chrome: RowChrome {
            header_height: 0.0,
            label: Some(format!("Tool: {name}").into()),
            kind: KIND_TOOL,
            ..RowChrome::default()
        },
        adornments: vec![header(tool, call.clone(), expanded)],
    }
}

fn icon(kind: ToolKind) -> &'static str {
    match kind {
        ToolKind::Search => lucide::SEARCH,
        ToolKind::Read => lucide::FILE,
        ToolKind::Edit => lucide::FILE_CODE,
        ToolKind::Run => lucide::TERMINAL,
    }
}

/// The header: a disclosure covering chevron, icon, verb, target, and
/// outcome, then Retry on a failed tool.
fn header(tool: ToolId, call: ToolCall, expanded: bool) -> RowAdornment {
    // Everything the header shows, and nothing it does not (streamed
    // output), so output chunks do not rebuild it.
    let revision = {
        let mut hasher = DefaultHasher::new();
        (
            &call.verb,
            &call.target,
            &call.duration,
            call.status as u8,
            expanded,
        )
            .hash(&mut hasher);
        hasher.finish()
    };
    RowAdornment::new(
        HEADER,
        AdornmentSlot::Start,
        TOOL_HEADER,
        revision,
        move |cx| {
            let c = &cx.theme.colors;
            let (status_color, status_icon) = match call.status {
                ToolStatus::Running => (c.accent, lucide::LOADER),
                ToolStatus::Ok => (c.line_add_text, lucide::CHECK),
                ToolStatus::Failed => (c.status_error, lucide::ALERT_CIRCLE),
            };
            let chevron = if expanded {
                lucide::CHEVRON_DOWN
            } else {
                lucide::CHEVRON_RIGHT
            };
            let word = outcome(&call);
            let detail = match &call.duration {
                Some(d) => format!("{word} in {d}"),
                None => word.to_owned(),
            };
            let name = format!("{} {}, {word}", call.verb, call.target);
            let toggle: Action = TimelineAction::ToggleTool(tool).into();
            let retry = (call.status == ToolStatus::Failed).then(|| {
                let action: Action = TimelineAction::Retry(tool).into();
                Button::new(action)
                    .label("Retry")
                    .icon(lucide::REFRESH)
                    .style(ButtonStyle::Subtle)
                    .size(ButtonSize::Compact)
                    .into_any()
            });
            let control = (tokens::TYPE_CONTROL.0, tokens::ICON);
            view! {
                <div
                    w={cx.width}
                    h={cx.height}
                    class="flex-row items-center gap-[8] px-[4]"
                    bg={c.surface}
                    border={c.border_variant}
                    class="rounded-[8]"
                >
                    <div
                        id={disclosure_id(tool).as_str()}
                        class="flex-row items-center gap-[8] grow h-full px-[6] rounded-[6]"
                        hover_bg={c.element_hover}
                        on:click={toggle}
                        track_focus={disclosure_focus(tool)}
                        accessibility_role={AccessibilityRole::Button}
                        aria-label={name}
                        aria-expanded={expanded}
                    >
                        <icon svg={chevron} size={control.1} color={c.text_muted} />
                        <icon svg={icon(call.kind)} size={control.1} color={c.icon} />
                        <text size={control.0} class="font-medium" color={c.text_strong}>
                            {call.verb.clone()}
                        </text>
                        <div class="grow flex-row items-center overflow-hidden">
                            <text size={control.0} color={c.text_muted} class="truncate">
                                {call.target.clone()}
                            </text>
                        </div>
                        <icon svg={status_icon} size={control.1} color={status_color} />
                        <text size={tokens::TYPE_META.0} color={status_color}>{detail}</text>
                    </div>
                    {?retry}
                </div>
            }
            .into_any()
        },
    )
}
