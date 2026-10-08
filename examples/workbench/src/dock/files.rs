//! The files panel (stream E): the fixture project in the existing tree
//! view, and the selected file in a read-only, selectable source view.
//! Files a run or an applied diff changed carry a diff icon in the tree
//! and a "Modified" badge over their source.

use std::collections::HashMap;

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::Editor;
use quark_app::quark_ui::text_input::{
    EditorMode, TextEditCommand, TextEditOutcome, text_editor_element,
};
use quark_app::{UiContext, ViewContext};
use quark_components::{
    Badge, BadgeVariant, CollectionEnv, NodeId, SelectMods, TreeEvent, TreeOutcome, TreeState,
    tree_view,
};

use crate::contracts::SurfaceCx;
use crate::design::tokens;
use crate::fixtures;
use crate::model::Model;

pub const TREE_FOCUS: FocusId = FocusId::from_key("workbench.files.tree");
pub const SOURCE_FOCUS: FocusId = FocusId::from_key("workbench.files.source");
/// The source view's accessible name.
pub const SOURCE_LABEL: &str = "File source";

/// The file shown before any is picked.
const FIRST_FILE: &str = "src/App.tsx";
const ROW_H: f32 = 26.0;
const HEADER_H: f32 = 32.0;
/// Panels at least this wide put the tree beside the source.
const SIDE_BY_SIDE_WIDTH: f32 = 640.0;
const SIDE_TREE_WIDTH: f32 = 220.0;

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Tree(TreeEvent),
    /// Wheel lines over the source view.
    SourceScroll(i32),
}

pub struct State {
    tree: TreeState,
    /// Each file node's path; directories have none.
    paths: HashMap<NodeId, String>,
    /// The file in the source view.
    open: String,
    source: Editor,
    /// Changed paths as of the last frame, to update tree icons only when
    /// they change.
    badged: Vec<String>,
}

impl State {
    pub fn new() -> Self {
        let mut tree = TreeState::new("workbench.files", TREE_FOCUS)
            .with_label("Files")
            .with_row_height(ROW_H)
            .with_scrollbar_auto_hide();
        let mut paths = HashMap::new();
        let mut dirs: HashMap<&str, NodeId> = HashMap::new();
        // Directories first, as file browsers list them.
        let mut sorted: Vec<&str> = fixtures::FILES.iter().map(|(p, _)| *p).collect();
        sorted.sort_by_key(|p| (!p.contains('/'), *p));
        for path in sorted {
            let (parent, name) = match path.rsplit_once('/') {
                Some((dir, name)) => {
                    let node = *dirs.entry(dir).or_insert_with(|| {
                        let node = tree.add(None, dir);
                        tree.set_icon(node, Some(lucide::FOLDER));
                        node
                    });
                    (Some(node), name)
                }
                None => (None, path),
            };
            let node = tree.add(parent, name);
            tree.set_icon(node, Some(lucide::FILE_CODE));
            paths.insert(node, path.to_owned());
        }
        for node in dirs.into_values() {
            tree.expand(node);
        }
        let mut source = Editor::new(EditorMode::DiffReadOnly);
        source.set_font_size(tokens::TYPE_CODE.0);
        let mut state = Self {
            tree,
            paths,
            open: String::new(),
            source,
            badged: Vec::new(),
        };
        state.open(FIRST_FILE);
        state
    }

    /// The path in the source view.
    pub fn open_path(&self) -> &str {
        &self.open
    }

    /// Select `path` in the tree and show it. Unknown paths change nothing.
    pub fn open(&mut self, path: &str) {
        let Some(node) = self.paths.iter().find(|(_, p)| *p == path).map(|(n, _)| *n) else {
            return;
        };
        if let Some(parent) = self.tree.parent(node) {
            self.tree.expand(parent);
        }
        self.tree.select_only(node);
        self.open = path.to_owned();
    }

    /// Show the selected file when the selection names one.
    fn follow_selection(&mut self) {
        let picked = self
            .tree
            .selected()
            .find_map(|n| self.paths.get(&n))
            .cloned();
        if let Some(path) = picked {
            self.open = path;
        }
    }

    /// Keep tree icons and the source text in step with the file store.
    fn sync(&mut self, model: &Model) {
        let changed = model.files.changed();
        if self.badged != changed {
            for (node, path) in &self.paths {
                let icon = if changed.contains(path) {
                    lucide::FILE_DIFF
                } else {
                    lucide::FILE_CODE
                };
                self.tree.set_icon(*node, Some(icon));
            }
            self.badged = changed.to_vec();
        }
        let text = model.files.get(&self.open).unwrap_or_default();
        if self.source.text() != text {
            self.source.set_text(text);
            // The caret lands at the end, and the editor would scroll it
            // into view: a file opens at its first line.
            self.source.apply(TextEditCommand::SetTextCursor(0));
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
    state.sync(scx.model);

    let side = width >= SIDE_BY_SIDE_WIDTH;
    let rows = state.tree.row_count() as f32;
    let (tree_w, tree_h) = if side {
        (SIDE_TREE_WIDTH, height)
    } else {
        // Every row when it fits, else two fifths of the panel.
        (width, (rows * ROW_H + tokens::SPACE_8).min(height * 0.4))
    };
    let (source_w, source_h) = if side {
        (width - tree_w, height - HEADER_H)
    } else {
        (width, (height - tree_h - HEADER_H).max(0.0))
    };
    state.tree.set_viewport_height(tree_h);
    let env = CollectionEnv {
        focused: vcx.is_focused(TREE_FOCUS),
        accessible: vcx.frame.accessibility_active(),
    };
    let tree = tree_view(&mut state.tree, scx.theme, env, |e| {
        super::Action::Files(Action::Tree(e)).into()
    });

    state
        .source
        .set_clock(vcx.frame.elapsed().as_millis() as u64);
    state.source.sync_size(source_w, source_h);
    state.source.flush(&mut vcx.frame.text().system);
    let modified = scx.model.files.changed().contains(&state.open);
    let lines = state.source.line_count();
    let source = view! {
        <text_editor_element(
            SOURCE_FOCUS,
            ScrollActionBuilder::new(|lines| super::Action::Files(Action::SourceScroll(lines)).into()),
        )
            editor_snapshot={&state.source}
            placeholder={SOURCE_LABEL}
            focused={vcx.is_focused(SOURCE_FOCUS)}
            text_color={colors.text}
            w={source_w}
            h={source_h} />
    };
    let header = view! {
        <div w={source_w} h={HEADER_H} class="flex-row items-center shrink-0"
             px={tokens::SPACE_8} gap={tokens::SPACE_8} border_b={colors.border_variant}
             bg={colors.panel} role="status"
             aria-label={format!("{}{}", state.open, if modified { ", modified" } else { "" })}>
            <icon svg={lucide::FILE_CODE} size={tokens::ICON} color={colors.icon} />
            <div class="flex-1" min_w={0.0}>
                <text size={12.0} class="truncate font-medium" color={colors.text_strong}>
                    {state.open.clone()}
                </text>
            </div>
            if modified {
                <Badge label="Modified" variant={BadgeVariant::Warning} />
            }
            <text size={11.0} color={colors.text_muted}>{format!("{lines} lines")}</text>
        </div>
    };
    let tree_box = view! {
        <div w={tree_w} h={tree_h} class="shrink-0 overflow-clip"
             @when {side} { border_r={colors.border_variant} }
             @when {!side} { border_b={colors.border_variant} }>
            {tree}
        </div>
    };
    view! {
        <div w={width} h={height} bg={colors.editor_surface} test_id="dock.files"
             @when {side} { class="flex-row" }
             @when {!side} { class="flex-col" }>
            {tree_box}
            <div w={source_w} class="flex-col flex-1" min_h={0.0}>
                {header}
                {source}
            </div>
        </div>
    }
    .into_any()
}

pub fn update(state: &mut State, action: Action, cx: &mut UiContext) {
    match action {
        Action::Tree(event) => {
            let mods = cx.window.modifiers();
            let mods = SelectMods {
                extend: mods.shift_key(),
                toggle: mods.control_key() || mods.super_key(),
            };
            let now_ms = cx.window.elapsed().as_millis() as u64;
            match state.tree.handle(event, mods, now_ms) {
                TreeOutcome::Changed | TreeOutcome::Activated(_) => state.follow_selection(),
                _ => {}
            }
        }
        Action::SourceScroll(lines) => {
            let step = state.source.scroll_line_height_px();
            state.source.scroll(lines as f32 * step);
        }
    }
}

pub fn edit_text(
    state: &mut State,
    target: FocusId,
    command: TextEditCommand,
    now_ms: u64,
) -> Option<TextEditOutcome> {
    (target == SOURCE_FOCUS).then(|| state.source.apply_at(command, now_ms))
}
