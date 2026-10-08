//! Transcript row composition: user turns, assistant prose, error cards,
//! and the chrome around them. Each model [`Row`] becomes one document row
//! keyed by its [`MessageId`]; tool rows are composed in `tool_card`.

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use quark::view;
use quark_app::quark_ui::Action;
use quark_app::quark_ui::document::{
    AdornmentAccessibility, AdornmentKey, AdornmentSlot, DocumentStyle, RowAdornment, RowChrome,
    RowDecorator,
};
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::{Color, Theme};
use quark_components::{Button, ButtonStyle};

use super::tool_card;
use crate::contracts::ToolId;
use crate::design::tokens;
use crate::model::{Role, Row};

/// Row kinds the decorator branches on.
pub const KIND_USER: u32 = 0;
pub const KIND_ASSISTANT: u32 = 1;
pub const KIND_TOOL: u32 = 2;
pub const KIND_ERROR: u32 = 3;

/// Height of the author line above a message.
pub const AUTHOR_LINE: f32 = 20.0;
/// Height of the streaming cursor line under a growing answer.
pub const CURSOR_LINE: f32 = 18.0;
/// Height of an error card's action bar.
pub const ERROR_ACTIONS: f32 = 32.0;

/// Adornment keys within a row.
pub const HEADER: AdornmentKey = AdornmentKey(1);
pub const CURSOR: AdornmentKey = AdornmentKey(2);
pub const ACTIONS: AdornmentKey = AdornmentKey(3);

/// The transcript's geometry: 14-point prose, 16-point insets, and 24
/// points between messages (12 above and below each row).
pub fn document_style() -> DocumentStyle {
    let (font_size, _) = tokens::TYPE_PROSE;
    DocumentStyle {
        font_size,
        pad_x: tokens::SPACE_16,
        pad_y: tokens::SPACE_12,
        block_gap: 10.0,
        line_scroll: 42.0,
        edge: 28.0,
        overscan: 320.0,
    }
}

/// What a model row shows as a document row.
pub struct RowContent {
    pub markdown: String,
    pub chrome: RowChrome,
    pub adornments: Vec<RowAdornment>,
}

/// Everything besides a prose row's markdown that [`content`] reads.
pub fn shape(row: &Row, expanded: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    (row.role as u8, &row.at, row.streaming, row.retry, expanded).hash(&mut hasher);
    if let Some(call) = &row.tool {
        (&call.verb, &call.target, &call.duration, call.status as u8).hash(&mut hasher);
    }
    hasher.finish()
}

/// `row` as a document row. `expanded` applies to tool rows only.
pub fn content(row: &Row, expanded: bool) -> RowContent {
    if let Some(call) = &row.tool {
        return tool_card::content(row, call, expanded);
    }
    let kind = match row.role {
        Role::User => KIND_USER,
        Role::Assistant | Role::Tool => KIND_ASSISTANT,
        Role::Error => KIND_ERROR,
    };
    let mut adornments = Vec::new();
    if row.streaming {
        adornments.push(cursor());
    }
    if row.role == Role::Error && row.retry {
        adornments.push(error_actions(ToolId(row.id.0)));
    }
    RowContent {
        markdown: row.markdown.clone(),
        chrome: RowChrome {
            header_height: AUTHOR_LINE,
            label: Some(format!("{}, {}", row.author(), row.at).into()),
            kind,
        },
        adornments,
    }
}

/// The blinking-free cursor line under an answer that is still streaming.
/// It goes away with the row's `streaming` flag, on Stop or completion.
fn cursor() -> RowAdornment {
    RowAdornment::new(CURSOR, AdornmentSlot::End, CURSOR_LINE, 0, |cx| {
        let colors = &cx.theme.colors;
        view! {
            <div w={cx.width} h={cx.height} class="flex-row items-center gap-[6]">
                <div w={7.0} h={15.0} class="rounded-[2]" bg={colors.accent} />
                <text size={tokens::TYPE_META.0} color={colors.text_muted}>"Generating"</text>
            </div>
        }
        .into_any()
    })
}

/// An error card's Retry. The accessible name stays "Retry" (e2e specs
/// select it by name).
fn error_actions(tool: ToolId) -> RowAdornment {
    RowAdornment::new(ACTIONS, AdornmentSlot::End, ERROR_ACTIONS, 0, move |cx| {
        let retry: Action = super::Action::Retry(tool).into();
        view! {
            <div w={cx.width} h={cx.height} class="flex-row items-center">
                {Button::new(retry).label("Retry").style(ButtonStyle::Subtle).into_any()}
            </div>
        }
        .into_any()
    })
    .accessibility(AdornmentAccessibility::Exposed)
}

/// Draws the author line above messages, a soft inset behind user turns,
/// and an error tint behind error cards. Tool rows draw their own card.
pub struct TimelineChrome;

impl RowDecorator for TimelineChrome {
    fn background(&self, chrome: &RowChrome, theme: &Theme) -> Option<Color> {
        let c = &theme.colors;
        match chrome.kind {
            KIND_USER => Some(c.element_background),
            KIND_ERROR => Some(c.status_error.with_alpha(20)),
            _ => None,
        }
    }

    fn header(&self, chrome: &RowChrome, width: f32, theme: &Theme) -> Option<AnyElement> {
        let label: &Arc<str> = chrome.label.as_ref()?;
        let (author, at) = label.split_once(", ").unwrap_or((label, ""));
        let c = &theme.colors;
        let author_color = if chrome.kind == KIND_ERROR {
            c.status_error
        } else {
            c.text_strong
        };
        Some(
            view! {
                <div w={width} h={AUTHOR_LINE} class="flex-row items-center gap-[8]">
                    <text size={tokens::TYPE_CONTROL.0} class="font-semibold" color={author_color}>
                        {author.to_owned()}
                    </text>
                    <text size={tokens::TYPE_META.0} color={c.text_muted}>{at.to_owned()}</text>
                </div>
            }
            .into_any(),
        )
    }
}
