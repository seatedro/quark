//! A file tree beside a file table, both virtualized.
//!
//! The tree loads folder contents lazily, reorders by drag, and supports
//! Shift and Cmd/Ctrl selection and type-ahead. The table has 100,000 rows
//! with sortable, resizable, and reorderable columns and a custom renderer
//! for the size column.

use std::borrow::Cow;
use std::cmp::Ordering;
use std::rc::Rc;
use std::sync::Arc;

use quark::view;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::{Action, FocusId};
use quark_app::winit::keyboard::NamedKey;
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::{
    CellStyle, CollectionEnv, SelectMods, TableData, TableEvent, TableOutcome, TableState,
    TreeEvent, TreeOutcome, TreeState, table_view, tree_view,
};

const TREE_FOCUS: FocusId = FocusId::from_key("demo.tree");
const TABLE_FOCUS: FocusId = FocusId::from_key("demo.table");
const TREE_WIDTH: f32 = 260.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Tree(TreeEvent),
    Table(TableEvent),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// File rows as columns: name, size in bytes, kind.
struct Files {
    names: Vec<Arc<str>>,
    sizes: Vec<u64>,
    kinds: Vec<&'static str>,
}

const NAME: usize = 0;
const SIZE: usize = 1;

impl Files {
    fn generated(rows: usize) -> Self {
        const KINDS: [&str; 4] = ["rust", "markdown", "toml", "image"];
        Self {
            names: (0..rows)
                .map(|i| Arc::from(format!("file-{i:06}")))
                .collect(),
            // A fixed scramble so sorting by size reorders the rows.
            sizes: (0..rows as u64).map(|i| (i * 7_919) % 100_003).collect(),
            kinds: (0..rows).map(|i| KINDS[i % KINDS.len()]).collect(),
        }
    }
}

impl TableData for Files {
    fn row_count(&self) -> usize {
        self.names.len()
    }

    fn cell_text(&self, row: usize, column: usize) -> Cow<'_, str> {
        match column {
            NAME => Cow::Borrowed(&self.names[row]),
            SIZE => Cow::Owned(self.sizes[row].to_string()),
            _ => Cow::Borrowed(self.kinds[row]),
        }
    }

    fn compare(&self, a: usize, b: usize, column: usize) -> Ordering {
        match column {
            SIZE => self.sizes[a].cmp(&self.sizes[b]),
            _ => self.cell_text(a, column).cmp(&self.cell_text(b, column)),
        }
    }

    /// Sizes right-aligned in kilobytes; the rest as plain text.
    fn render_cell(&self, row: usize, column: usize, style: &CellStyle) -> AnyElement {
        if column != SIZE {
            return view! {
                <text class="text-sm" color={style.text} class="truncate">
                    {self.cell_text(row, column).into_owned()}
                </text>
            };
        }
        let kb = self.sizes[row] as f64 / 1024.0;
        view! {
            <div class="w-full flex-row justify-end">
                <text class="text-sm" color={style.muted}>"{kb:.1} KB"</text>
            </div>
        }
    }
}

struct Demo {
    tree: TreeState,
    table: TableState,
    files: Rc<Files>,
}

impl Demo {
    fn new(tree: TreeState, rows: usize) -> Self {
        let mut table = TableState::new("demo.table", TABLE_FOCUS).with_label("Files");
        table.add_column("Name", 220.0, true);
        table.add_column("Size", 120.0, true);
        table.add_column("Kind", 140.0, true);
        Self {
            tree,
            table,
            files: Rc::new(Files::generated(rows)),
        }
    }

    /// A small project tree whose folders load their files on first expand.
    fn sample_tree() -> TreeState {
        let mut tree = TreeState::new("demo.tree", TREE_FOCUS).with_label("Project");
        for folder in ["crates", "docs", "examples"] {
            let node = tree.add(None, folder);
            tree.set_icon(node, Some(lucide::FOLDER));
            tree.set_lazy(node, true);
        }
        let readme = tree.add(None, "README.md");
        tree.set_icon(readme, Some(lucide::FILE));
        tree
    }

    fn load_children(&mut self, node: quark_components::NodeId) {
        let parent = self.tree.label(node).to_owned();
        for i in 1..=3 {
            let child = self.tree.add(Some(node), format!("{parent}-{i}.rs"));
            self.tree.set_icon(child, Some(lucide::FILE_CODE));
        }
        self.tree.finish_loading(node);
    }
}

fn select_mods(cx: &UiContext) -> SelectMods {
    let mods = cx.window.modifiers();
    SelectMods {
        extend: mods.shift_key(),
        toggle: mods.control_key() || mods.super_key(),
    }
}

impl UiApp for Demo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let accessible = cx.frame.accessibility_active();
        self.tree.set_viewport_height(height);
        self.table
            .set_viewport((width - TREE_WIDTH).max(0.0), height);
        let tree = tree_view(
            &mut self.tree,
            cx.theme,
            CollectionEnv {
                focused: cx.is_focused(TREE_FOCUS),
                accessible,
            },
            |event| Msg::Tree(event).into(),
        );
        let table = table_view(
            &mut self.table,
            &self.files,
            cx.theme,
            CollectionEnv {
                focused: cx.is_focused(TABLE_FOCUS),
                accessible,
            },
            |event| Msg::Table(event).into(),
        );
        let colors = &cx.theme.colors;
        view! {
            <div w={width} h={height} class="flex-row bg-[colors.background]">
                <div
                    w={TREE_WIDTH}
                    h={height}
                    class="shrink-0 bg-[colors.sidebar_background] border-r-[colors.border]"
                >
                    {tree}
                </div>
                {table}
            </div>
        }
    }

    /// Escape mid-drag drops nothing: a dragged row or column goes back.
    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let escape = matches!(event, InputEvent::KeyPress(chord)
            if chord.named() == Some(NamedKey::Escape));
        if escape && cx.is_dragging() {
            cx.cancel_drag();
            return true;
        }
        false
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        let mods = select_mods(cx);
        match msg {
            Msg::Tree(event) => {
                let now_ms = cx.window.elapsed().as_millis() as u64;
                match self.tree.handle(event, mods, now_ms) {
                    TreeOutcome::LoadChildren(node) => self.load_children(node),
                    TreeOutcome::Activated(node) => {
                        tracing::info!("opened {}", self.tree.label(node));
                    }
                    _ => {}
                }
            }
            Msg::Table(event) => {
                if let TableOutcome::Activated(row) = self.table.handle(&*self.files, event, mods) {
                    tracing::info!("opened {}", self.files.names[row]);
                }
            }
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        Demo::new(Demo::sample_tree(), 100_000),
        WindowOptions {
            title: "Tree and table".into(),
            size: (900.0, 600.0),
            ..WindowOptions::default()
        },
    )
}

/// The tree and table driven headlessly the way a user drives them:
/// clicks, drags, and keys on nodes found by role and name.
#[cfg(test)]
mod tests {
    use accesskit::{Role, SortDirection};
    use quark_app::InputEvent;
    use quark_app::quark_ui::test_alloc::{self, Counting};
    use quark_app::testing::{By, UiTestHarness};
    use quark_app::winit::keyboard::ModifiersState;

    use super::*;

    #[global_allocator]
    static ALLOCATOR: Counting = Counting;

    const SIZE: (f32, f32) = (900.0, 480.0);

    /// ```text
    /// src
    ///   main.rs
    ///   lib.rs
    /// docs
    ///   guide.md
    /// README.md
    /// ```
    fn project() -> TreeState {
        let mut tree = TreeState::new("demo.tree", TREE_FOCUS).with_label("Project");
        let src = tree.add(None, "src");
        tree.add(Some(src), "main.rs");
        tree.add(Some(src), "lib.rs");
        let docs = tree.add(None, "docs");
        tree.add(Some(docs), "guide.md");
        tree.add(None, "README.md");
        tree
    }

    fn harness(tree: TreeState, rows: usize) -> UiTestHarness<Demo> {
        UiTestHarness::new(Demo::new(tree, rows), SIZE, 1.0)
    }

    fn item(name: &str) -> By {
        By::role_name(Role::TreeItem, name)
    }

    fn rows(ui: &mut UiTestHarness<Demo>) -> String {
        ui.app_mut().tree.dump_rows()
    }

    /// Click with modifier keys held, as the platform reports them.
    fn click_with(ui: &mut UiTestHarness<Demo>, mods: ModifiersState, by: By) {
        let at = ui.find(by).center();
        ui.send_event(InputEvent::ModifiersChanged(mods));
        ui.click(at);
        ui.send_event(InputEvent::ModifiersChanged(ModifiersState::empty()));
    }

    /// One line per published node of `role`: its name, then the
    /// collection properties set on it.
    fn collection_lines(ui: &UiTestHarness<Demo>, role: Role) -> String {
        let update = ui.accessibility_update();
        let mut out = String::new();
        for (_, node) in update.nodes.iter().filter(|(_, n)| n.role() == role) {
            out.push_str(node.label().unwrap_or("-"));
            if let Some(level) = node.level() {
                out.push_str(&format!(" level={level}"));
            }
            if let (Some(pos), Some(size)) = (node.position_in_set(), node.size_of_set()) {
                out.push_str(&format!(" pos={}/{size}", pos + 1));
            }
            if let (Some(rows), Some(columns)) = (node.row_count(), node.column_count()) {
                out.push_str(&format!(" size={rows}x{columns}"));
            }
            if let Some(row) = node.row_index() {
                out.push_str(&format!(" row={row}"));
            }
            if let Some(column) = node.column_index() {
                out.push_str(&format!(" col={column}"));
            }
            match node.sort_direction() {
                Some(SortDirection::Ascending) => out.push_str(" sort=asc"),
                Some(SortDirection::Descending) => out.push_str(" sort=desc"),
                _ => {}
            }
            match node.is_expanded() {
                Some(true) => out.push_str(" expanded"),
                Some(false) => out.push_str(" collapsed"),
                None => {}
            }
            if node.is_selected() == Some(true) {
                out.push_str(" selected");
            }
            if node.is_multiselectable() {
                out.push_str(" multiselectable");
            }
            out.push('\n');
        }
        out
    }

    // ---- Tree ------------------------------------------------------------

    #[test]
    fn arrow_keys_expand_collapse_and_walk_the_tree() {
        let mut ui = harness(project(), 10);
        ui.click_node(item("src"));
        ui.key("arrowright");
        ui.key("arrowdown");
        ui.key("arrowdown");
        assert_eq!(
            rows(&mut ui),
            "v src\n  main.rs\n  lib.rs * <\n> docs\nREADME.md\n"
        );

        // Left goes to the parent, then collapses it.
        ui.key("arrowleft");
        ui.key("arrowleft");
        ui.key("end");
        assert_eq!(rows(&mut ui), "> src\n> docs\nREADME.md * <\n");
        ui.key("home");
        ui.key("arrowright");
        ui.key("arrowright");
        assert_eq!(
            rows(&mut ui),
            "v src\n  main.rs * <\n  lib.rs\n> docs\nREADME.md\n"
        );
    }

    #[test]
    fn type_ahead_selects_the_next_matching_row() {
        let mut ui = harness(project(), 10);
        ui.click_node(item("src"));
        ui.key("r");
        assert_eq!(rows(&mut ui), "> src\n> docs\nREADME.md * <\n");
        // A second letter soon after extends the prefix instead of jumping.
        ui.key("e");
        assert_eq!(rows(&mut ui), "> src\n> docs\nREADME.md * <\n");
        ui.advance(1_500);
        ui.key("d");
        assert_eq!(rows(&mut ui), "> src\n> docs * <\nREADME.md\n");
    }

    #[test]
    fn expanding_a_lazy_folder_loads_its_children() {
        let mut ui = harness(Demo::sample_tree(), 10);
        ui.click_node(item("docs"));
        ui.key("arrowright");
        assert_eq!(
            rows(&mut ui),
            "> crates\nv docs * <\n  docs-1.rs\n  docs-2.rs\n  docs-3.rs\n> examples\nREADME.md\n"
        );
    }

    #[test]
    fn shift_click_selects_a_range_and_mod_click_toggles_one_row() {
        let mut ui = harness(project(), 10);
        ui.click_node(item("src"));
        ui.key("arrowright");
        click_with(&mut ui, ModifiersState::SHIFT, item("docs"));
        assert_eq!(
            rows(&mut ui),
            "v src *\n  main.rs *\n  lib.rs *\n> docs * <\nREADME.md\n"
        );
        click_with(&mut ui, ModifiersState::CONTROL, item("main.rs"));
        click_with(&mut ui, ModifiersState::CONTROL, item("README.md"));
        assert_eq!(
            rows(&mut ui),
            "v src *\n  main.rs\n  lib.rs *\n> docs *\nREADME.md * <\n"
        );
    }

    #[test]
    fn dropping_a_row_lands_by_the_band_under_its_center() {
        // (dragged, target, fraction of the target row's height, result)
        let cases = [
            (
                "README.md",
                "docs",
                0.5,
                "v src\n  main.rs\n  lib.rs\nv docs\n  guide.md\n  README.md * <\n",
            ),
            (
                "lib.rs",
                "main.rs",
                0.1,
                "v src\n  lib.rs * <\n  main.rs\n> docs\nREADME.md\n",
            ),
            (
                "main.rs",
                "docs",
                0.9,
                "v src\n  lib.rs\n> docs\nmain.rs * <\nREADME.md\n",
            ),
            // Into its own child: refused.
            (
                "src",
                "lib.rs",
                0.5,
                "v src * <\n  main.rs\n  lib.rs\n> docs\nREADME.md\n",
            ),
        ];
        for (dragged, target, fraction, expected) in cases {
            let mut ui = harness(project(), 10);
            ui.click_node(item("src"));
            ui.key("arrowright");
            let from = ui.find(item(dragged)).center();
            let to = ui.find(item(target)).bounds;
            ui.drag(from, (from.0, to.y + to.height * fraction));
            assert_eq!(
                rows(&mut ui),
                expected,
                "{dragged} onto {target} at {fraction}"
            );
            ui.app().tree.verify_integrity().unwrap();
        }
    }

    type Cancel = fn(&mut UiTestHarness<Demo>);
    const CANCELS: [(&str, Cancel); 2] = [
        ("focus loss", |ui| ui.focus_loss()),
        ("cancel_drag", |ui| ui.key("escape")),
    ];

    // Regression: a cancelled drag fell back to its release and dropped the
    // row on the target under the pointer.
    #[test]
    fn a_cancelled_row_drag_moves_nothing() {
        for (name, cancel) in CANCELS {
            let mut ui = harness(project(), 10);
            let from = ui.find(item("README.md")).center();
            let to = ui.find(item("docs")).center();
            ui.pointer_down(from);
            ui.pointer_move((from.0, to.1));
            let held = ui.app().tree.drop_target().is_some();
            cancel(&mut ui);
            ui.pointer_up((from.0, to.1));

            assert!(held, "{name}: README.md was over docs");
            assert_eq!(ui.app().tree.drop_target(), None, "{name}");
            assert_eq!(rows(&mut ui), "> src\n> docs\nREADME.md * <\n", "{name}");
        }
    }

    #[test]
    fn tree_items_publish_level_position_and_state() {
        let mut ui = harness(project(), 10);
        ui.click_node(item("src"));
        ui.key("arrowright");
        assert_eq!(
            collection_lines(&ui, Role::Tree),
            "Project multiselectable\n"
        );
        assert_eq!(
            collection_lines(&ui, Role::TreeItem),
            "src level=1 pos=1/3 expanded selected\n\
             main.rs level=2 pos=1/2\n\
             lib.rs level=2 pos=2/2\n\
             docs level=1 pos=2/3 collapsed\n\
             README.md level=1 pos=3/3\n"
        );
    }

    #[test]
    fn a_hundred_thousand_rows_build_only_the_window() {
        let mut tree = TreeState::new("demo.tree", TREE_FOCUS).with_row_height(24.0);
        tree.extend(None, (0..100_000).map(|i| format!("Item {i}")));
        let mut ui = harness(tree, 10);
        // 480 points of 24-point rows: 20 visible plus 2 overscan below.
        assert_eq!(ui.find_all(By::role(Role::TreeItem)).len(), 22);

        ui.click_node(item("Item 0"));
        ui.key("end");
        let items = ui.find_all(By::role(Role::TreeItem));
        assert_eq!(items.len(), 22);
        assert_eq!(items.last().unwrap().name.as_deref(), Some("Item 99999"));
        assert!(
            collection_lines(&ui, Role::TreeItem)
                .contains("Item 99999 level=1 pos=100000/100000 selected")
        );
    }

    // ---- Table -----------------------------------------------------------

    fn header(name: &str) -> By {
        By::role_name(Role::ColumnHeader, name)
    }

    fn table(ui: &UiTestHarness<Demo>) -> String {
        let app = ui.app();
        app.table.dump(&*app.files)
    }

    /// The first `n` lines of the table dump: the header and `n - 1` rows.
    fn table_head(ui: &UiTestHarness<Demo>, n: usize) -> String {
        table(ui).lines().take(n).collect::<Vec<_>>().join("\n")
    }

    #[test]
    fn clicking_a_header_sorts_then_reverses() {
        let mut ui = harness(project(), 5);
        // Sizes of rows 0..5 are 0, 7919, 15838, 23757, 31676.
        ui.click_node(header("Size"));
        assert_eq!(
            table_head(&ui, 3),
            "Name | Size ^ | Kind\nfile-000000 | 0 | rust\nfile-000001 | 7919 | markdown"
        );
        ui.click_node(header("Size"));
        assert_eq!(
            table_head(&ui, 3),
            "Name | Size v | Kind\nfile-000004 | 31676 | rust\nfile-000003 | 23757 | image"
        );
        assert!(collection_lines(&ui, Role::ColumnHeader).contains("Size row=0 col=1 sort=desc"));
    }

    #[test]
    fn dragging_a_resize_handle_sets_the_width_down_to_the_minimum() {
        // (drag distance, resulting width of the 220-point Name column)
        for (dx, width) in [(40.0, 260.0), (-300.0, 32.0)] {
            let mut ui = harness(project(), 5);
            let name = ui.find(header("Name")).bounds;
            let edge = (name.x + name.width - 2.0, name.y + name.height / 2.0);
            ui.drag(edge, (edge.0 + dx, edge.1));
            assert_eq!(ui.find(header("Name")).bounds.width, width, "drag {dx}");
        }
    }

    #[test]
    fn dragging_a_header_onto_another_moves_the_column() {
        let mut ui = harness(project(), 5);
        let from = ui.find(header("Name")).center();
        let to = ui.find(header("Kind")).center();
        ui.drag(from, to);
        assert_eq!(table_head(&ui, 1), "Size | Kind | Name");
        // A move is not a click: nothing got sorted.
        assert_eq!(ui.app().table.sort(), None);
    }

    // Regression: a cancelled header press fell back to its release, so it
    // moved the column dragged, or sorted by the column pressed.
    #[test]
    fn a_cancelled_header_drag_neither_moves_nor_sorts() {
        for (name, cancel) in CANCELS {
            for dragged in [true, false] {
                let mut ui = harness(project(), 5);
                let from = ui.find(header("Name")).center();
                let to = ui.find(header("Kind")).center();
                ui.pointer_down(from);
                ui.pointer_move(if dragged { to } else { from });
                cancel(&mut ui);
                ui.pointer_up(to);
                let after_cancel = table_head(&ui, 1);
                // A click after the cancel only sorts: no column move left
                // over from the cancelled drag.
                ui.click_node(header("Size"));

                let case = format!("{name}, dragged {dragged}");
                assert_eq!(after_cancel, "Name | Size | Kind", "{case}");
                assert_eq!(table_head(&ui, 1), "Name | Size ^ | Kind", "{case}");
            }
        }
    }

    #[test]
    fn table_keys_move_the_cursor_and_extend_the_selection() {
        let mut ui = harness(project(), 5);
        let first = ui.find(By::role_name(Role::Cell, "file-000001")).center();
        ui.click(first);
        ui.key("arrowdown");
        ui.key("shift+arrowdown");
        ui.key("shift+arrowdown");
        assert_eq!(
            table(&ui),
            "Name | Size | Kind\n\
             file-000000 | 0 | rust\n\
             file-000001 | 7919 | markdown\n\
             * file-000002 | 15838 | toml\n\
             * file-000003 | 23757 | image\n\
             * file-000004 | 31676 | rust\n"
        );
    }

    #[test]
    fn a_hundred_thousand_row_table_builds_only_the_window() {
        let mut ui = harness(project(), 100_000);
        // 450 points of body at 28 points a row: 17 visible plus 2 below.
        assert_eq!(ui.find_all(By::role(Role::Row)).len(), 1 + 19);
        assert_eq!(
            collection_lines(&ui, Role::Table),
            "Files size=100001x3 multiselectable\n"
        );

        // Narrow enough that only the first two columns are in view.
        ui.resize(TREE_WIDTH + 300.0, SIZE.1);
        assert_eq!(ui.find_all(By::role(Role::ColumnHeader)).len(), 2);
        assert_eq!(ui.find_all(By::role(Role::Cell)).len(), 19 * 2);
    }

    // Catches the table's columns not scrolling under a horizontal thumb
    // drag, or the body and header moving apart: dragging the thumb to the
    // end shows the last column flush with the table's right edge, its
    // cells under its header.
    #[test]
    fn dragging_the_tables_horizontal_thumb_scrolls_body_and_header_together() {
        let mut ui = harness(project(), 100);
        // 300 points of table over 480 of columns.
        let right = TREE_WIDTH + 300.0;
        ui.resize(right, SIZE.1);
        let thumb = ui
            .scene()
            .primitives
            .iter()
            .find_map(|p| match p {
                // The bottom bar's thumb; its track is the faint one.
                quark::scene::Primitive::RoundedRect(r)
                    if r.rect.x > TREE_WIDTH && r.rect.y > SIZE.1 - 8.0 && r.color.a != 10 =>
                {
                    Some((
                        r.rect.x + r.rect.width / 2.0,
                        r.rect.y + r.rect.height / 2.0,
                    ))
                }
                _ => None,
            })
            .expect("a horizontal thumb");
        ui.drag(thumb, (right + 40.0, thumb.1));

        let header = ui.find(By::role_name(Role::ColumnHeader, "Kind")).bounds;
        let cell = ui.find_all(By::role_name(Role::Cell, "rust"))[0].bounds;
        assert_eq!(
            (
                ui.app().table.scroll_offset().0,
                header.x + header.width,
                cell.x
            ),
            (180.0, right, header.x)
        );
    }

    // ---- Frames ----------------------------------------------------------

    /// A frame that repeats the last one, with no screen reader connected,
    /// replays both views from the element cache.
    #[test]
    fn a_repeated_frame_allocates_nothing() {
        let mut ui = harness(project(), 100_000);
        ui.set_accessibility_active(false);
        for _ in 0..3 {
            ui.frame();
        }
        let ((), allocated) = test_alloc::count(|| {
            ui.frame();
        });
        assert_eq!(allocated, 0);
    }
}
