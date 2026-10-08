//! The diff panel (stream E): the proposed patch from `fixtures/diff.patch`
//! in a `DiffViewState`, unified at the dock's default 400 points and side
//! by side once the panel is wide enough, with Apply and Undo against the
//! in-memory fixture file store.

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::{UiContext, ViewContext};
use quark_components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonStyle, CollectionEnv, DiffEvent, DiffOutcome,
    DiffViewState, SegmentedControl, SegmentedItem, diff_view,
};
use quark_diff::Mode;

use crate::contracts::{Effect, Effects, SurfaceCx};
use crate::design::tokens;
use crate::fixtures;
use crate::model::Model;

pub const FOCUS: FocusId = FocusId::from_key("workbench.diff");

/// Narrowest panel that offers side by side: two columns of about 32
/// characters of 13-point code beside their line numbers. A 1240-point
/// window with the sidebar docked can still widen the dock this far.
pub const SPLIT_MIN_WIDTH: f32 = 600.0;

const TOOLBAR_H: f32 = 40.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    View(DiffEvent),
    /// The mode the user picked; shown while the panel is wide enough.
    Mode(Mode),
    Apply,
    Undo,
}

pub struct State {
    view: DiffViewState,
    /// The user's mode. A narrow panel shows unified whatever this says,
    /// and goes back to it when widened.
    preferred: Mode,
    /// Files, additions, and deletions of the patch.
    totals: (usize, u32, u32),
}

impl State {
    pub fn new() -> Self {
        let doc =
            quark_diff::parse_unified(fixtures::DIFF_PATCH).expect("the fixture patch parses");
        let totals = doc.summaries().fold((0, 0, 0), |(n, a, d), s| {
            (n + 1, a + s.additions, d + s.deletions)
        });
        let mut view = DiffViewState::new("workbench.diff", FOCUS, doc)
            .with_label("Proposed changes")
            .with_scrollbar_auto_hide();
        if let Some(store) = crate::assets::grammar_store() {
            view.enable_syntax(store);
        }
        Self {
            view,
            preferred: Mode::Unified,
            totals,
        }
    }

    /// The mode on screen.
    pub fn mode(&self) -> Mode {
        self.view.mode()
    }

    /// Scroll to `path`'s file header.
    pub fn reveal(&mut self, path: &str) {
        let file = self.view.document().summaries().find(|s| &*s.path == path);
        if let Some(file) = file {
            let row = self.view.projection().file_rows[file.index as usize];
            self.view.scroll_to_row(row);
        }
    }
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> AnyElement {
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let wide = width >= SPLIT_MIN_WIDTH;
    let mode = if wide { state.preferred } else { Mode::Unified };
    if state.view.mode() != mode {
        state.view.set_mode(mode);
    }
    let body_h = (height - TOOLBAR_H).max(0.0);
    state.view.set_viewport(width, body_h);
    let scale = vcx.frame.scale_factor();
    let now_ms = vcx.frame.elapsed().as_millis() as u64;
    let text_cx = vcx.frame.text();
    state
        .view
        .prepare(&mut text_cx.system, &mut text_cx.layouts, scale, now_ms);
    let env = CollectionEnv {
        focused: vcx.is_focused(FOCUS),
        accessible: vcx.frame.accessibility_active(),
    };
    let body = diff_view(&mut state.view, scx.theme, env, |e| {
        super::Action::Diff(Action::View(e)).into()
    });

    let applied = scx.model.files.can_undo();
    let (badge, variant) = if applied {
        ("Applied", BadgeVariant::Success)
    } else {
        ("Proposed", BadgeVariant::Info)
    };
    let (files, adds, dels) = state.totals;
    let summary = format!("{files} file{} changed", if files == 1 { "" } else { "s" });
    let segmented = wide.then(|| {
        let item = |label: &str, m: Mode| {
            SegmentedItem::new(
                label,
                super::Action::Diff(Action::Mode(m)),
                state.preferred == m,
            )
        };
        SegmentedControl::new(vec![
            item("Unified", Mode::Unified),
            item("Split", Mode::Split),
        ])
        .id("workbench.diff.mode")
    });
    view! {
        <div w={width} h={height} class="flex-col" bg={colors.editor_surface} test_id="dock.diff">
            <div w={width} h={TOOLBAR_H} class="flex-row items-center shrink-0"
                 px={tokens::SPACE_8} gap={tokens::SPACE_8} border_b={colors.border}
                 bg={colors.panel} accessibility_role={accesskit::Role::Toolbar}
                 aria-label="Diff actions">
                <Badge label={badge} variant={variant} />
                <div class="flex-row items-center gap-[6] flex-1" min_w={0.0}
                     role="status" aria-label={format!("{summary}, {adds} additions, {dels} deletions")}>
                    <text size={12.0} class="truncate" color={colors.text_muted}>{summary}</text>
                    <text size={12.0} color={colors.line_add_text}>{format!("+{adds}")}</text>
                    <text size={12.0} color={colors.line_del_text}>{format!("-{dels}")}</text>
                </div>
                if let Some(segmented) = segmented {
                    {segmented}
                }
                <Button on:click={super::Action::Diff(Action::Apply)} label="Apply"
                        icon={lucide::CHECK} variant={ButtonStyle::Filled}
                        size={ButtonSize::Compact} disabled={applied}
                        tooltip="Apply the proposed changes to the demo files" />
                <Button on:click={super::Action::Diff(Action::Undo)} label="Undo"
                        icon={lucide::CORNER_UP_LEFT} size={ButtonSize::Compact} disabled={!applied}
                        tooltip="Restore the files from before Apply" />
            </div>
            {body}
        </div>
    }
    .into_any()
}

pub fn update(
    state: &mut State,
    action: Action,
    _model: &Model,
    fx: &mut Effects,
    cx: &mut UiContext,
) {
    match action {
        Action::View(event) => {
            if let DiffOutcome::Copy(text) = state.view.handle(event)
                && !text.is_empty()
            {
                cx.window.set_clipboard_text(&text);
            }
        }
        Action::Mode(mode) => state.preferred = mode,
        Action::Apply => fx.push(Effect::ApplyDiff),
        Action::Undo => fx.push(Effect::UndoDiff),
    }
}
