//! The thread sidebar: search, a Pinned section, one section per project,
//! and a virtualized row list that stays cheap with the 2,000-thread
//! stress fixture.
//!
//! Selection is the model's `ThreadId`, never a row index, so filtering,
//! collapsing sections, and threads arriving keep it on the same thread.

use quark::view;
use quark_app::ViewContext;
use quark_app::quark_ui::accessibility::AccessibilityRole;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::quark_ui::theme::Color;
use quark_app::quark_ui::virtual_list::virtual_list_window;
use quark_app::quark_ui::{Action as QAction, FocusId};
use quark_components::{Button, ButtonSize, search_field};
use std::rc::Rc;

use super::Action;
use crate::contracts::{ProjectId, SurfaceCx, ThreadId};
use crate::design::tokens;
use crate::model::{Model, ThreadStatus};

/// The sidebar's search field.
pub const SEARCH: FocusId = FocusId::from_key("sidebar.search");
/// The thread list, for focus and the accessibility tree.
const LIST: FocusId = FocusId::from_key("sidebar.list");
/// Height of the search area above the list.
const SEARCH_AREA: f32 = 48.0;
const ROW: f32 = tokens::SIDEBAR_ROW;
/// Rows built beyond each edge of the list's viewport.
const OVERSCAN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Pinned,
    Project(ProjectId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Row {
    Header(Section),
    /// A thread and its index in `Model::threads` as of the last rebuild.
    Thread(ThreadId, usize),
}

pub struct State {
    search: TextField,
    collapsed: Vec<Section>,
    scroll: f32,
    /// List viewport height as of the last frame, for clamping scrolls.
    viewport: f32,
    /// Rows as of the last frame; reused so steady frames do not allocate.
    rows: Vec<Row>,
    /// Row data of the rows on screen, reused across frames.
    visible: Vec<Rc<RowData>>,
    memo: RowMemo,
}

impl Default for State {
    fn default() -> Self {
        Self {
            search: TextField::new(""),
            collapsed: Vec::new(),
            scroll: 0.0,
            viewport: 0.0,
            rows: Vec::new(),
            visible: Vec::new(),
            memo: RowMemo::default(),
        }
    }
}

fn folded(s: &str) -> impl Iterator<Item = char> + '_ {
    s.chars().flat_map(char::to_lowercase)
}

/// Case-insensitive substring match that allocates nothing.
fn contains_folded(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    haystack.char_indices().any(|(at, _)| {
        let mut hay = folded(&haystack[at..]);
        folded(needle).all(|n| hay.next() == Some(n))
    })
}

impl State {
    pub fn query(&self) -> &str {
        self.search.text().trim()
    }

    pub fn clear_search(&mut self) {
        self.search.set_text("");
        self.scroll = 0.0;
    }

    pub fn edit_search(&mut self, command: TextEditCommand, now_ms: u64) -> TextEditOutcome {
        let outcome = self.search.apply_at(command, now_ms);
        // A new query starts from the top of its results.
        self.scroll = 0.0;
        outcome
    }

    pub fn toggle(&mut self, section: Section) {
        if let Some(at) = self.collapsed.iter().position(|s| *s == section) {
            self.collapsed.remove(at);
        } else {
            self.collapsed.push(section);
        }
    }

    pub fn is_collapsed(&self, section: Section) -> bool {
        self.collapsed.contains(&section)
    }

    fn max_scroll(&self) -> f32 {
        (self.rows.len() as f32 * ROW - self.viewport).max(0.0)
    }

    pub fn scroll_to(&mut self, offset: f32) {
        self.scroll = offset.clamp(0.0, self.max_scroll());
    }

    pub fn scroll_by_lines(&mut self, lines: i32) {
        self.scroll_to(self.scroll + lines as f32 * WHEEL_LINE_PX);
    }

    /// The thread `step` threads after `from` among the listed ones,
    /// clamped to the ends; the first listed when `from` is not listed.
    pub fn step(&self, _model: &Model, from: ThreadId, step: i32) -> Option<ThreadId> {
        let threads: Vec<ThreadId> = self
            .rows
            .iter()
            .filter_map(|r| match r {
                Row::Thread(id, _) => Some(*id),
                Row::Header(_) => None,
            })
            .collect();
        let last = threads.len().checked_sub(1)? as i64;
        let at = threads
            .iter()
            .position(|t| *t == from)
            .map_or(-1, |i| i as i64);
        let next = (at + i64::from(step)).clamp(0, last);
        threads.get(next as usize).copied()
    }

    /// Scroll just enough to show `thread`'s row.
    pub fn reveal(&mut self, thread: ThreadId) {
        let Some(i) = self
            .rows
            .iter()
            .position(|r| matches!(r, Row::Thread(id, _) if *id == thread))
        else {
            return;
        };
        let (top, bottom) = (i as f32 * ROW, (i + 1) as f32 * ROW);
        if top < self.scroll {
            self.scroll_to(top);
        } else if bottom > self.scroll + self.viewport {
            self.scroll_to(bottom - self.viewport);
        }
    }

    /// Thread titles as listed (headers as `## name`), one per line; for
    /// tests and logs.
    pub fn dump(&self, model: &Model) -> String {
        let mut out = String::new();
        for row in &self.rows {
            match *row {
                Row::Header(section) => {
                    out.push_str("## ");
                    out.push_str(section_label(model, section));
                }
                Row::Thread(_, index) => out.push_str(&model.threads[index].title),
            }
            out.push('\n');
        }
        out
    }

    /// Rebuild the flat row list from the model, the query, and the
    /// collapsed sections. While searching every matching thread shows.
    pub fn rebuild(&mut self, model: &Model) {
        let query = self.search.text().trim();
        let searching = !query.is_empty();
        self.rows.clear();
        let sections = std::iter::once(Section::Pinned)
            .chain(model.projects.iter().map(|p| Section::Project(p.id)));
        for section in sections {
            let open = searching || !self.collapsed.contains(&section);
            let mut matched = false;
            for (index, t) in model.threads.iter().enumerate() {
                let member = match section {
                    Section::Pinned => t.pinned,
                    Section::Project(p) => !t.pinned && t.project == p,
                };
                if !member || !contains_folded(&t.title, query) {
                    continue;
                }
                if !matched {
                    self.rows.push(Row::Header(section));
                    matched = true;
                }
                if open {
                    self.rows.push(Row::Thread(t.id, index));
                }
            }
        }
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }
}

fn section_label(model: &Model, section: Section) -> &str {
    match section {
        Section::Pinned => "Pinned",
        Section::Project(id) => model.project(id).map_or("Project", |p| p.name.as_str()),
    }
}

pub fn view(state: &mut super::State, scx: &SurfaceCx, _vcx: &mut ViewContext) -> AnyElement {
    let sidebar = &mut state.sidebar;
    sidebar.rebuild(scx.model);
    let colors = &scx.theme.colors;
    let (width, height) = scx.size;
    let list_h = (height - SEARCH_AREA).max(0.0);
    sidebar.viewport = list_h;
    sidebar.scroll = sidebar.scroll.clamp(0.0, sidebar.max_scroll());

    let has_query = !sidebar.query().is_empty();
    let input = text_input("Search threads", "")
        .placeholder("Search threads")
        .focus_target(SEARCH)
        .focused(scx.is_focused(SEARCH))
        .field(&sidebar.search)
        .on_click(Action::FocusSearch)
        .search(true)
        .bare();
    let search = search_field(
        input,
        has_query,
        Some(Action::ClearSearch.into()),
        None,
        scx.theme,
    );

    let body = if sidebar.rows.is_empty() {
        no_results(sidebar.query(), scx)
    } else {
        list(sidebar, scx, width, list_h)
    };
    view! {
        <div w={width} h={height} class="flex-col" bg={colors.sidebar_background}
             test_id="sidebar">
            <div w={width} h={SEARCH_AREA} class="shrink-0 px-2 py-2">{search}</div>
            {body}
        </div>
    }
    .into_any()
}

fn no_results(query: &str, scx: &SurfaceCx) -> AnyElement {
    let colors = &scx.theme.colors;
    let message = format!("No threads match “{query}”");
    view! {
        <div class="flex-col items-center p-4 gap-[8]" role="status" aria-label={message.clone()}>
            <text size={13.0} color={colors.text_muted}>{message}</text>
            <Button on:click={Action::ClearSearch} size={ButtonSize::Compact}>"Clear search"</Button>
        </div>
    }
    .into_any()
}

fn list(sidebar: &mut State, scx: &SurfaceCx, width: f32, list_h: f32) -> AnyElement {
    let window = virtual_list_window(
        sidebar.rows.len(),
        sidebar.scroll,
        list_h,
        ROW,
        0.0,
        OVERSCAN,
    );
    let scroll = ScrollActionBuilder::new(|lines| Action::ScrollLines(lines).into())
        .with_to_px(|px| Action::ScrollTo(px).into());
    let colors = RowColors::new(scx);
    let searching = !sidebar.query().is_empty();
    let mut rows = std::mem::take(&mut sidebar.visible);
    rows.clear();
    for i in window.range.clone() {
        let row = sidebar.rows[i];
        let data = match row {
            Row::Header(section) => {
                let open = searching || !sidebar.is_collapsed(section);
                let label = section_label(scx.model, section);
                sidebar.memo.header(row, section, label, open)
            }
            Row::Thread(id, index) => sidebar.memo.thread(row, scx, id, &scx.model.threads[index]),
        };
        rows.push(data);
    }
    let element = view! {
        <div w={width} h={list_h} class="flex-col px-2" scroll_y={sidebar.scroll}
             scroll_total={window.total_extent} on:scroll={scroll} track_focus={LIST}
             focus_ring={LIST}
             on_key={("up", QAction::from(Action::Step(-1)))}
             on_key={("down", QAction::from(Action::Step(1)))}
             on_key={("home", QAction::from(Action::Step(i32::MIN)))}
             on_key={("end", QAction::from(Action::Step(i32::MAX)))}
             accessibility_role={AccessibilityRole::List} aria-label="Threads">
            <div class="w-full shrink-0" h={window.top_spacer} />
            for data in rows.iter() {
                {row_view(data, colors)}
            }
            <div class="w-full shrink-0" h={window.bottom_spacer} />
        </div>
    }
    .into_any();
    sidebar.visible = rows;
    element
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Status {
    Running,
    Failed,
}

/// Everything one row's view reads, shared with its cached build closure
/// so a replayed row costs no allocation.
#[derive(Debug, PartialEq, Eq, Hash)]
struct RowData {
    row: RowKind,
    label: String,
    /// The accessible name: the label plus its states in words.
    name: String,
    detail: String,
    selected: bool,
    unread: bool,
    status: Option<Status>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum RowKind {
    Header { section: Section, open: bool },
    Thread(ThreadId),
}

fn status_of(scx: &SurfaceCx, id: ThreadId, thread: &crate::model::Thread) -> Option<Status> {
    if thread.status == ThreadStatus::Running || scx.model.run(id).is_some() {
        Some(Status::Running)
    } else if thread.status == ThreadStatus::Failed {
        Some(Status::Failed)
    } else {
        None
    }
}

/// Row data per row, reused while what it shows is unchanged, so steady
/// frames clone no strings.
#[derive(Default)]
struct RowMemo {
    rows: std::collections::HashMap<Row, Rc<RowData>>,
}

impl RowMemo {
    fn header(&mut self, row: Row, section: Section, label: &str, open: bool) -> Rc<RowData> {
        let kind = RowKind::Header { section, open };
        if let Some(old) = self.rows.get(&row)
            && old.row == kind
            && old.label == label
        {
            return old.clone();
        }
        self.store(
            row,
            RowData {
                row: kind,
                label: label.to_owned(),
                name: label.to_owned(),
                detail: String::new(),
                selected: false,
                unread: false,
                status: None,
            },
        )
    }

    fn thread(
        &mut self,
        row: Row,
        scx: &SurfaceCx,
        id: ThreadId,
        thread: &crate::model::Thread,
    ) -> Rc<RowData> {
        let status = status_of(scx, id, thread);
        let selected = id == scx.model.selected;
        if let Some(old) = self.rows.get(&row)
            && old.label == thread.title
            && old.detail == thread.updated
            && (old.selected, old.unread, old.status) == (selected, thread.unread, status)
        {
            return old.clone();
        }
        let mut name = thread.title.clone();
        match status {
            Some(Status::Running) => name.push_str(", running"),
            Some(Status::Failed) => name.push_str(", failed"),
            None => {}
        }
        if thread.unread {
            name.push_str(", unread");
        }
        self.store(
            row,
            RowData {
                row: RowKind::Thread(id),
                label: thread.title.clone(),
                name,
                detail: thread.updated.clone(),
                selected,
                unread: thread.unread,
                status,
            },
        )
    }

    fn store(&mut self, row: Row, data: RowData) -> Rc<RowData> {
        let data = Rc::new(data);
        self.rows.insert(row, data.clone());
        data
    }
}

#[derive(Clone, Copy)]
struct RowColors {
    background: Color,
    hover: Color,
    selected: Color,
    text: Color,
    strong: Color,
    muted: Color,
    accent: Color,
    error: Color,
}

impl RowColors {
    fn new(scx: &SurfaceCx) -> Self {
        let c = &scx.theme.colors;
        Self {
            background: c.sidebar_background,
            hover: c.sidebar_row_hover,
            selected: c.sidebar_row_selected,
            text: c.text,
            strong: c.text_strong,
            muted: c.text_muted,
            accent: c.accent,
            error: c.status_error,
        }
    }
}

fn row_view(data: &Rc<RowData>, colors: RowColors) -> AnyElement {
    let key = match data.row {
        RowKind::Header { section, .. } => inputs_hash(&("sidebar.header", section)),
        RowKind::Thread(id) => inputs_hash(&("sidebar.thread", id)),
    };
    let data = data.clone();
    // The data is the full input; colors come from the theme, which the
    // cache already compares.
    let hash = inputs_hash(&*data);
    cached(key, hash, move || build_row(&data, colors))
        .w_full()
        .h(ROW)
        .flex_shrink_0()
        .into_any()
}

fn build_row(data: &RowData, colors: RowColors) -> AnyElement {
    match data.row {
        RowKind::Header { section, open } => view! {
            <div class="w-full flex-row items-center gap-[6] px-2 rounded-[6]" h={ROW}
                 hover_bg={colors.hover}
                 on:click={Action::ToggleSection(section)}
                 accessibility_role={AccessibilityRole::Button} aria-label={data.label.clone()}
                 accessibility_expanded={open}>
                <icon svg={if open { lucide::CHEVRON_DOWN } else { lucide::CHEVRON_RIGHT }}
                      size={12.0} color={colors.muted} />
                <text size={11.0} class="font-semibold" color={colors.muted}>{data.label.clone()}</text>
            </div>
        }
        .into_any(),
        RowKind::Thread(id) => {
            let selected = data.selected;
            let bg = if selected { colors.selected } else { colors.background };
            let title_color = if selected { colors.strong } else { colors.text };
            let status = data.status.map(|s| match s {
                Status::Running => (lucide::LOADER, colors.accent),
                Status::Failed => (lucide::ALERT_CIRCLE, colors.error),
            });
            view! {
                <div class="w-full flex-row items-center gap-[8] px-2 rounded-[6]" h={ROW}
                     bg={bg} hover_bg={if selected { bg } else { colors.hover }}
                     on:click={Action::SelectThread(id)}
                     accessibility_role={AccessibilityRole::ListItem} aria-label={data.name.clone()}
                     accessibility_selected={selected} tooltip={data.label.clone()}>
                    <div class="w-[6] h-[6] rounded-[3] shrink-0"
                         bg={if data.unread { colors.accent } else { bg }} />
                    <div class="flex-1" min-w={0.0}>
                        if selected {
                            <text size={13.0} class="truncate" color={title_color} semibold>
                                {data.label.clone()}
                            </text>
                        } else {
                            <text size={13.0} class="truncate" color={title_color}>
                                {data.label.clone()}
                            </text>
                        }
                    </div>
                    if let Some((svg, color)) = status {
                        <icon svg={svg} size={14.0} color={color} />
                    }
                    <text size={11.0} color={colors.muted}>{data.detail.clone()}</text>
                </div>
            }
            .into_any()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::contains_folded;

    // Catches the allocation-free matcher missing case-folded Unicode
    // matches or matching past the end of the haystack.
    #[test]
    fn folded_match_agrees_with_lowercase_contains() {
        let cases = [
            ("Add keyboard shortcuts", "SHORT", true),
            ("Add keyboard shortcuts", "", true),
            ("ЙЦУКЕН layout", "йцу", true),
            ("Trip export to GPX", "gpx!", false),
            ("ab", "abc", false),
        ];
        for (hay, needle, want) in cases {
            assert_eq!(contains_folded(hay, needle), want, "{hay:?} / {needle:?}");
        }
    }
}
