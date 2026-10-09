//! The diff panel (stream E): the proposed patch from `fixtures/diff.patch`
//! in quark's shared diff viewer (`DiffViewState`, review presentation),
//! unified at the dock's default 400 points and, once the panel is wide
//! enough, automatic or the user's choice of unified and split. The
//! toolbar applies and undoes the patch against the in-memory fixture file
//! store, copies it, finds text in it, wraps lines, and expands every
//! collapsed region.

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::quark_ui::theme::ThemeColors;
use quark_app::{UiContext, ViewContext};
use quark_components::diff_view::presentation::{DiffLayout, DiffPresentation};
use quark_components::{
    Badge, BadgeVariant, Button, ButtonSize, ButtonStyle, CollectionEnv, CopyContent, DiffEvent,
    DiffOutcome, DiffStyle, DiffViewState, FindOptions, SearchDirection, SegmentedControl,
    SegmentedItem, diff_view,
};

use crate::contracts::{Effect, Effects, SurfaceCx};
use crate::design::tokens;
use crate::fixtures;
use crate::model::Model;

pub const FOCUS: FocusId = FocusId::from_key("workbench.diff");
pub const FIND_FIELD: FocusId = FocusId::from_key("workbench.diff.find");

/// Narrowest panel that offers side by side: two columns of about 32
/// characters of 13-point code beside their line numbers. A 1240-point
/// window with the sidebar docked can still widen the dock this far.
pub const SPLIT_MIN_WIDTH: f32 = 600.0;

const TOOLBAR_H: f32 = 40.0;
/// The row of view controls under the toolbar.
const TOOLS_H: f32 = 34.0;
const HEADER_H: f32 = TOOLBAR_H + TOOLS_H;
const FIND_H: f32 = 34.0;

/// The layouts the panel offers once it is wide enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Split while each side has room for its code, else unified.
    Auto,
    Unified,
    Split,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    View(DiffEvent),
    /// The layout the user picked; offered while the panel is wide enough.
    Layout(Layout),
    Apply,
    Undo,
    /// Puts the patch on the clipboard.
    Copy,
    Wrap,
    ExpandAll,
    /// Opens or closes the find bar.
    Find(bool),
    /// Next (`true`) or previous match.
    FindStep(bool),
}

pub struct State {
    view: DiffViewState,
    /// The user's layout. A narrow panel shows unified whatever this says,
    /// and goes back to it when widened.
    preferred: Layout,
    /// The find field while the find bar is open.
    find: Option<TextField>,
    /// Files, additions, and deletions of the patch.
    totals: (usize, u32, u32),
}

impl State {
    pub fn new() -> Self {
        let doc =
            quark_diff::parse_unified(fixtures::DIFF_PATCH).expect("the fixture patch parses");
        let doc = hydrated(doc);
        let totals = doc.summaries().fold((0, 0, 0), |(n, a, d), s| {
            (n + 1, a + s.additions, d + s.deletions)
        });
        let mut view = DiffViewState::new("workbench.diff", FOCUS, doc)
            .with_label("Proposed changes")
            .with_scrollbar_auto_hide();
        view.set_presentation(DiffPresentation {
            layout: DiffLayout::Unified,
            sticky_headers: true,
            ..DiffPresentation::review()
        });
        if let Some(store) = crate::assets::grammar_store() {
            view.enable_syntax(store);
        }
        Self {
            view,
            preferred: Layout::Auto,
            find: None,
            totals,
        }
    }

    /// Edits the find field; a new query searches and moves to its first
    /// match.
    pub fn edit_find(&mut self, command: TextEditCommand, now_ms: u64) -> Option<TextEditOutcome> {
        let field = self.find.as_mut()?;
        let outcome = field.apply_at(command, now_ms);
        if outcome.text_changed {
            let query = field.text().to_owned();
            self.view.set_find_query(&query, FindOptions::default());
            self.view.next_match(SearchDirection::Forward);
        }
        Some(outcome)
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

/// The patch with each file's exact old and new text from the fixture
/// store, so syntax colors whole files and unchanged context can expand.
/// A file that does not hydrate stays as the patch shows it.
fn hydrated(doc: quark_diff::DiffDocument) -> quark_diff::DiffDocument {
    use quark_diff::{FileSources, TextStore};
    (0..doc.file_count()).fold(doc, |doc, file| {
        let path = doc.path(file).to_owned();
        let Some(&(_, old)) = fixtures::FILES.iter().find(|f| f.0 == path) else {
            return doc;
        };
        let Ok(new) = quark_diff::apply(&doc, file, old) else {
            return doc;
        };
        let sources = FileSources {
            old: Some(TextStore::new(old)),
            new: Some(TextStore::new(new)),
        };
        match doc.hydrate_file(file, sources) {
            Ok(full) => full,
            Err(_) => doc,
        }
    })
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
    let layout = match (wide, state.preferred) {
        (false, _) | (true, Layout::Unified) => DiffLayout::Unified,
        (true, Layout::Split) => DiffLayout::Split,
        (true, Layout::Auto) => DiffLayout::AUTO,
    };
    let presentation = state.view.presentation();
    if presentation.layout != layout {
        state.view.set_presentation(DiffPresentation {
            layout,
            ..presentation
        });
    }
    let find = state.find.as_ref().map(|field| {
        let summary = state.view.search_summary();
        let count = match (summary.matches, summary.active) {
            (0, _) if field.text().is_empty() => String::new(),
            (0, _) => "No results".to_owned(),
            (n, Some(i)) => format!("{} of {n}", i + 1),
            (n, None) => format!("{n} results"),
        };
        find_bar(field, count, scx.is_focused(FIND_FIELD), width, colors)
    });
    let find_h = if find.is_some() { FIND_H } else { 0.0 };
    let body_h = (height - HEADER_H - find_h).max(0.0);
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

    let header = Header {
        applied: scx.model.files.can_undo(),
        totals: state.totals,
        wide,
        layout: state.preferred,
        wrap: state.view.style().wrap,
        finding: state.find.is_some(),
        width,
    };
    let hash = inputs_hash(&(
        (header.applied, header.totals, header.wide),
        (header.layout as u8, header.wrap, header.finding),
        width.to_bits(),
    ));
    let header_colors = *colors;
    let header = cached("dock.diff.header", hash, move || {
        header.build(header_colors)
    })
    .w(width)
    .h(HEADER_H)
    .flex_shrink_0();
    view! {
        <div w={width} h={height} class="flex-col" bg={colors.editor_surface} test_id="dock.diff">
            {header}
            {?find}
            {body}
        </div>
    }
    .into_any()
}

/// The find bar under the toolbar: the field, the match count, previous,
/// next, and close.
fn find_bar(
    field: &TextField,
    count: String,
    focused: bool,
    width: f32,
    colors: &ThemeColors,
) -> AnyElement {
    let step = |icon: &'static str, label: &str, action: Action| {
        view! {
            <Button
                on:click={super::Action::Diff(action)}
                icon={icon}
                tooltip={label}
                size={ButtonSize::Compact}
            />
        }
    };
    view! {
        <div
            w={width}
            h={FIND_H}
            class="flex-row items-center shrink-0"
            px={tokens::SPACE_8}
            gap={tokens::SPACE_8}
            border_b={colors.border_variant}
            bg={colors.panel}
            accessibility_role={accesskit::Role::Search}
            aria-label="Find in diff"
        >
            <text_input("Find in diff", "")
                placeholder="Find in diff"
                focus_target={FIND_FIELD}
                focused={focused}
                field={field}
                class="flex-1 h-6"
                min_w={0.0}
            />
            <div role="status" aria-label={count.clone()}>
                <text size={12.0} color={colors.text_muted}>{count}</text>
            </div>
            {step(
                lucide::CHEVRON_UP,
                "Previous match",
                Action::FindStep(false)
            )}
            {step(lucide::CHEVRON_DOWN, "Next match", Action::FindStep(true))}
            {step(lucide::X, "Close find", Action::Find(false))}
        </div>
    }
}

/// The toolbars over the diff: status with Apply and Undo, then the
/// layout choice with find, wrap, expand, and copy. They replay from the
/// element cache until one of these fields changes.
struct Header {
    applied: bool,
    totals: (usize, u32, u32),
    wide: bool,
    layout: Layout,
    wrap: bool,
    finding: bool,
    width: f32,
}

impl Header {
    fn build(self, colors: ThemeColors) -> AnyElement {
        let Self {
            applied,
            totals: (files, adds, dels),
            wide,
            layout,
            wrap,
            finding,
            width,
        } = self;
        let (badge, variant) = if applied {
            ("Applied", BadgeVariant::Success)
        } else {
            ("Proposed", BadgeVariant::Info)
        };
        let summary = format!("{files} file{} changed", if files == 1 { "" } else { "s" });
        let segmented = wide.then(|| {
            let item = |label: &str, l: Layout| {
                SegmentedItem::new(label, super::Action::Diff(Action::Layout(l)), layout == l)
            };
            SegmentedControl::new(vec![
                item("Auto", Layout::Auto),
                item("Unified", Layout::Unified),
                item("Split", Layout::Split),
            ])
            .id("workbench.diff.mode")
        });
        let tool = |icon: &'static str, tip: &str, action: Action, on: bool| {
            view! {
                <Button
                    on:click={super::Action::Diff(action)}
                    icon={icon}
                    tooltip={tip}
                    size={ButtonSize::Compact}
                    active={on}
                />
            }
        };
        view! {
            <div
                w={width}
                h={HEADER_H}
                class="flex-col shrink-0"
                border_b={colors.border_variant}
                bg={colors.panel}
            >
                <div
                    w={width}
                    h={TOOLBAR_H}
                    class="flex-row items-center shrink-0"
                    px={tokens::SPACE_8}
                    gap={tokens::SPACE_8}
                    accessibility_role={accesskit::Role::Toolbar}
                    aria-label="Diff actions"
                >
                    <Badge label={badge} variant={variant} />
                    <div
                        class="flex-row items-center gap-[6] flex-1"
                        min_w={0.0}
                        role="status"
                        aria-label={format!("{summary}, {adds} additions, {dels} deletions")}
                    >
                        <text size={12.0} class="truncate" color={colors.text_muted}>
                            {summary}
                        </text>
                        <text size={12.0} color={colors.line_add_text}>{format!("+{adds}")}</text>
                        <text size={12.0} color={colors.line_del_text}>{format!("-{dels}")}</text>
                    </div>
                    <Button
                        on:click={super::Action::Diff(Action::Apply)}
                        label="Apply"
                        icon={lucide::CHECK}
                        variant={ButtonStyle::Filled}
                        size={ButtonSize::Compact}
                        disabled={applied}
                        tooltip="Apply the proposed changes to the demo files"
                    />
                    <Button
                        on:click={super::Action::Diff(Action::Undo)}
                        label="Undo"
                        icon={lucide::CORNER_UP_LEFT}
                        size={ButtonSize::Compact}
                        disabled={!applied}
                        tooltip="Restore the files from before Apply"
                    />
                </div>
                <div
                    w={width}
                    h={TOOLS_H}
                    class="flex-row items-center shrink-0"
                    px={tokens::SPACE_8}
                    gap={tokens::SPACE_4}
                    accessibility_role={accesskit::Role::Toolbar}
                    aria-label="Diff view"
                >
                    if let Some(segmented) = segmented {
                        {segmented}
                    }
                    <div class="flex-1" />
                    {tool(
                        lucide::SEARCH,
                        "Find in diff",
                        Action::Find(!finding),
                        finding
                    )}
                    {tool(lucide::WRAP_TEXT, "Wrap lines", Action::Wrap, wrap)}
                    {tool(
                        lucide::LIST,
                        "Expand all unchanged lines",
                        Action::ExpandAll,
                        false
                    )}
                    {tool(lucide::COPY, "Copy patch", Action::Copy, false)}
                </div>
            </div>
        }
        .into_any()
    }
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
        Action::Layout(layout) => state.preferred = layout,
        Action::Apply => fx.push(Effect::ApplyDiff),
        Action::Undo => fx.push(Effect::UndoDiff),
        Action::Copy => {
            if let Some(patch) = state.view.copy(CopyContent::Patch) {
                cx.window.set_clipboard_text(&patch);
            }
        }
        Action::Wrap => {
            let style = state.view.style();
            state.view.set_style(DiffStyle {
                wrap: !style.wrap,
                ..style
            });
        }
        Action::ExpandAll => {
            state.view.expand_all();
        }
        Action::Find(true) => {
            state.find.get_or_insert_with(|| TextField::new(""));
            fx.push(Effect::Focus(Some(FIND_FIELD)));
        }
        Action::Find(false) => {
            state.find = None;
            state.view.set_find_query("", FindOptions::default());
            fx.push(Effect::Focus(Some(FOCUS)));
        }
        Action::FindStep(forward) => {
            let direction = if forward {
                SearchDirection::Forward
            } else {
                SearchDirection::Backward
            };
            state.view.next_match(direction);
        }
    }
}
