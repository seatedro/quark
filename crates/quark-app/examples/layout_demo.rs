//! Layout and input: a CSS grid gallery, sticky section headers, a list
//! reordered by dragging or Alt+Up/Down, key bindings that depend on the
//! focused context, files dragged out of the window, and a drawn menu bar.
//!
//! - Tasks (left): drag a row by its handle, or focus it and press
//!   Alt+Up/Down. Ctrl/Cmd+D duplicates the focused task (a `task-list`
//!   binding); elsewhere Ctrl/Cmd+D toggles the gallery's density (a
//!   global one).
//! - Gallery (middle): `repeat(auto-fill, 120px)` columns, spans, and
//!   square thumbnails from `aspect_ratio`.
//! - Files (right): folders as sticky sections. On macOS and Windows, drag
//!   a file onto the desktop or a file manager.
//! - The menu bar is drawn where the platform has none (Linux), or
//!   everywhere with `QUARK_DRAWN_MENU=1`; F10 or a lone Alt opens it.

use quark_app::platform::drag_out::{self, DragOutError};
use quark_app::platform::drawn_menu::{DrawnMenuBar, MenuPick};
use quark_app::platform::menu::{Menu, MenuAction, MenuItem, MenuRole, native_menu_bar};
use quark_app::quark_ui::design::Ico;
use quark_app::quark_ui::element::{
    AnyElement, CursorHint, Div, DragHandler, DragReleaseResult, IntoAnyElement, ScrollHandle, div,
    sticky_section, svg_icon, text,
};
use quark_app::quark_ui::icons::lucide;
use quark_app::quark_ui::key_context::KeyBindings;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::style::track::{fr, minmax, px, repeat_fill};
use quark_app::quark_ui::virtual_list::{
    KEY_MOVE_DOWN, KEY_MOVE_UP, Reorder, ReorderMsg, UniformRows,
};
use quark_app::quark_ui::{Action, FocusId};
use quark_app::{InputEvent, UiAdapter, UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::ContextMenuOutcome;

const TASK_ROW: f32 = 36.0;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Reorder(ReorderMsg),
    Duplicate,
    ToggleDense,
    DragOut(&'static str),
    MenuTitle(usize),
    Menu(MenuPick),
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

const FOLDERS: &[(&str, &[&str])] = &[
    (
        "Documents",
        &["notes.md", "plan.txt", "budget.csv", "letter.txt"],
    ),
    ("Pictures", &["sunset.png", "cat.jpg", "map.png"]),
    (
        "Music",
        &["intro.ogg", "theme.ogg", "outro.ogg", "demo.ogg"],
    ),
];

struct Demo {
    tasks: Vec<String>,
    reorder: Reorder,
    task_scroll: ScrollHandle,
    files_scroll: ScrollHandle,
    dense: bool,
    status: String,
    menu: Option<DrawnMenuBar>,
}

fn menus(dense: bool) -> Vec<Menu> {
    vec![
        Menu::app("Layout Demo"),
        Menu::new(
            "File",
            vec![
                MenuAction::new("duplicate", "Duplicate Task")
                    .shortcut("mod+d")
                    .into(),
                MenuItem::Separator,
                MenuRole::CloseWindow.into(),
            ],
        ),
        Menu::edit(),
        Menu::new(
            "View",
            vec![
                MenuAction::new("dense", "Dense Gallery")
                    .checked(dense)
                    .into(),
            ],
        ),
    ]
}

impl Demo {
    fn new() -> Self {
        let drawn = !native_menu_bar() || std::env::var_os("QUARK_DRAWN_MENU").is_some();
        Self {
            tasks: [
                "Sketch the grid",
                "Pin folder headers",
                "Reorder by dragging",
                "Bind keys per context",
                "Drag files out",
                "Draw a menu bar",
                "Write the docs",
                "Ship it",
            ]
            .map(String::from)
            .to_vec(),
            reorder: Reorder::new(),
            task_scroll: ScrollHandle::new(),
            files_scroll: ScrollHandle::new(),
            dense: false,
            status: "Ready".into(),
            menu: drawn.then(|| DrawnMenuBar::new(&menus(false), Msg::Menu)),
        }
    }

    fn task_rows(&self) -> UniformRows {
        UniformRows {
            count: self.tasks.len(),
            extent: TASK_ROW,
            gap: 0.0,
        }
    }

    fn focused_task(cx: &UiContext, tasks: &[String]) -> Option<usize> {
        let focus = cx.focus()?;
        tasks
            .iter()
            .position(|task| FocusId::from_key(task) == focus)
    }

    fn task_list(&mut self, now_ms: u64, cx: &mut ViewContext) -> Div {
        let theme = cx.theme;
        let rows = self.task_rows();
        let viewport = 7.0 * TASK_ROW;
        if let Some(next) =
            self.reorder
                .autoscroll(now_ms, &rows, self.task_scroll.offset().1, viewport)
        {
            self.task_scroll.set_offset(0.0, next);
            cx.frame.request_frame();
        }
        let dragged = self.reorder.dragged();
        let shift = self.reorder.dragged_shift().unwrap_or(0.0);
        let mut list = div()
            .h(viewport)
            .flex_col()
            .track_scroll(&self.task_scroll)
            .overflow_y_scroll()
            .key_context("task-list")
            .children_from(self.tasks.iter().enumerate().map(|(i, task)| {
                let handle = div()
                    .w(18.0)
                    .h(TASK_ROW)
                    .flex_shrink_0()
                    .flex_row()
                    .items_center()
                    .justify_center()
                    .cursor(CursorHint::Grab)
                    .tooltip("Drag to reorder")
                    .child(svg_icon(lucide::GRIP_VERTICAL, Ico::SM).color(theme.colors.text_muted))
                    .on_drag(Reorder::drag_start(i, Msg::Reorder));
                div()
                    .key(task.as_str())
                    .flex_row()
                    .items_center()
                    .gap(6.0)
                    .px(8.0)
                    .h(TASK_ROW)
                    .flex_shrink_0()
                    .bg(theme.colors.surface)
                    .border_b(theme.colors.border_variant)
                    .focus_ring(FocusId::from_key(task))
                    .accessibility_role(accesskit::Role::ListItem)
                    .accessibility_label(task.as_str())
                    .on_key(
                        KEY_MOVE_UP,
                        Msg::Reorder(ReorderMsg::Step {
                            index: i,
                            delta: -1,
                        }),
                    )
                    .on_key(
                        KEY_MOVE_DOWN,
                        Msg::Reorder(ReorderMsg::Step { index: i, delta: 1 }),
                    )
                    .when(dragged == Some(i), |row| {
                        row.translate(0.0, shift).z_index(1).opacity(0.9)
                    })
                    .child(handle)
                    .child(text(task.as_str()).text_sm())
            }));
        if let Some(y) = self.reorder.drop_indicator(&rows) {
            list = list.child(
                div()
                    .absolute()
                    .top(y - 1.0)
                    .left(0.0)
                    .right(0.0)
                    .h(2.0)
                    .bg(theme.colors.text_accent),
            );
        }
        list
    }

    fn gallery(&self, cx: &ViewContext) -> Div {
        let theme = cx.theme;
        let gap = if self.dense { 4.0 } else { 12.0 };
        div()
            .grid_cols([repeat_fill([px(120.0)])])
            .gap(gap)
            .children_from((0..14).map(|i| {
                let wide = i % 5 == 0;
                div()
                    .flex_col()
                    .gap(4.0)
                    .p(6.0)
                    .rounded(6.0)
                    .bg(theme.colors.elevated_surface)
                    .when(wide, |card| card.col_span(2))
                    .child(
                        div()
                            .w_full()
                            .aspect_ratio(if wide { 2.0 } else { 1.0 })
                            .rounded(4.0)
                            .bg(theme.colors.border),
                    )
                    .child(text(format!("Card {i}")).text_xs())
            }))
    }

    fn files(&self, cx: &ViewContext) -> Div {
        let theme = cx.theme;
        div()
            .h_full()
            .flex_col()
            .track_scroll(&self.files_scroll)
            .overflow_y_scroll()
            .children_from(FOLDERS.iter().map(|(folder, files)| {
                let header = div()
                    .h(28.0)
                    .px(8.0)
                    .flex_row()
                    .items_center()
                    .bg(theme.colors.title_bar_background)
                    .border_b(theme.colors.border)
                    .child(text(*folder).text_sm());
                let body = div().flex_col().children_from(files.iter().map(|file| {
                    let row = div()
                        .h(44.0)
                        .px(16.0)
                        .flex_row()
                        .items_center()
                        .border_b(theme.colors.border_variant)
                        .child(text(*file).text_sm());
                    // Where there is no drag source (the BSDs, see
                    // `drag_out`), rows are plain.
                    if drag_out::supported() {
                        row.cursor(CursorHint::Grab)
                            .tooltip("Drag onto the desktop or a file manager")
                            .on_drag(move |_| Box::new(DragOutRow::new(file)))
                    } else {
                        row
                    }
                }));
                sticky_section(header, body).flex_shrink_0()
            }))
    }

    fn handle_menu(&mut self, pick: MenuPick, cx: &mut UiContext) {
        if let Some(menu) = &mut self.menu {
            menu.bar.close();
        }
        match pick.perform(cx.window).as_deref() {
            Some("duplicate") => self.update(Msg::Duplicate, cx),
            Some("dense") => self.update(Msg::ToggleDense, cx),
            _ => {}
        }
    }
}

/// Starts a drag out once the pointer has moved a few points.
struct DragOutRow {
    file: &'static str,
    sent: bool,
}

impl DragOutRow {
    fn new(file: &'static str) -> Self {
        Self { file, sent: false }
    }
}

impl DragHandler for DragOutRow {
    fn on_move(&mut self, _x: f32, _y: f32) -> Vec<Action> {
        if std::mem::replace(&mut self.sent, true) {
            return Vec::new();
        }
        vec![Msg::DragOut(self.file).into()]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult::empty()
    }
}

impl UiApp for Demo {
    type Action = Msg;
    type Message = ();

    fn init(&mut self, cx: &mut UiContext) {
        cx.window.set_menus(menus(self.dense));
    }

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let now_ms = cx.frame.elapsed().as_millis() as u64;
        let size = cx.frame.logical_size();
        let theme = cx.theme;
        let mut root = div().size_full().flex_col().bg(theme.colors.background);
        if let Some(menu) = &mut self.menu {
            root = root.child(menu.bar.render(size, theme, |i| Msg::MenuTitle(i).into()));
        }
        let column = |title: &str, body: Div| {
            div()
                .flex_col()
                .gap(8.0)
                .min_h(0.0)
                .child(
                    text(title.to_owned())
                        .text_sm()
                        .color(theme.colors.text_muted),
                )
                .child(body)
        };
        let tasks = self.task_list(now_ms, cx);
        let theme = cx.theme;
        let body = div()
            .flex_1()
            .min_h(0.0)
            .p(12.0)
            .grid_cols([px(260.0), fr(1.0), minmax(px(220.0), fr(1.0))])
            .grid_rows([fr(1.0)])
            .gap(12.0)
            .child(column("Tasks", tasks))
            .child(column("Gallery", self.gallery(cx)))
            .child(column("Files", self.files(cx).flex_1()));
        root.child(body)
            .child(
                div()
                    .h(24.0)
                    .px(12.0)
                    .flex_row()
                    .items_center()
                    .border_t(theme.colors.border)
                    .child(text(self.status.clone()).text_xs()),
            )
            .into_any()
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::Reorder(msg) => {
                let rows = self.task_rows();
                let scroll = self.task_scroll.offset().1;
                if let Some(event) = self.reorder.update(msg, &rows, scroll) {
                    let label = self.tasks[event.from].clone();
                    event.apply(&mut self.tasks);
                    let said = event.announcement(&label, self.tasks.len());
                    cx.announce(
                        said.clone(),
                        quark_app::quark_ui::accessibility::Politeness::Assertive,
                    );
                    self.status = said;
                }
            }
            Msg::Duplicate => {
                if let Some(i) = Self::focused_task(cx, &self.tasks) {
                    let copy = format!("{} (copy)", self.tasks[i]);
                    self.tasks.insert(i + 1, copy);
                    self.status = format!("Duplicated task {}", i + 1);
                }
            }
            Msg::ToggleDense => {
                self.dense = !self.dense;
                cx.window.set_menus(menus(self.dense));
                if let Some(menu) = &mut self.menu {
                    menu.set_menus(&menus(self.dense));
                }
                self.status = format!("Dense gallery {}", if self.dense { "on" } else { "off" });
            }
            Msg::DragOut(file) => {
                let path = std::env::temp_dir().join(file);
                // Real files to drop: the demo writes a stand-in on demand.
                let _ = std::fs::write(&path, format!("{file} from layout_demo\n"));
                self.status = match cx.start_drag_out([&path]) {
                    Ok(()) => format!("Dragging {}", path.display()),
                    Err(DragOutError::Unsupported) => {
                        "Dragging files out is not supported on this platform".into()
                    }
                    Err(error) => error.to_string(),
                };
            }
            Msg::MenuTitle(index) => {
                if let Some(menu) = &mut self.menu {
                    menu.bar.title_clicked(index);
                }
            }
            Msg::Menu(pick) => self.handle_menu(pick, cx),
        }
    }

    fn app_event(&mut self, event: quark_app::AppEvent, cx: &mut UiContext) {
        if let quark_app::AppEvent::Menu(id) = event {
            self.handle_menu(MenuPick::Item(id), cx);
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let Some(outcome) = self.menu.as_mut().and_then(|menu| menu.event(event)) else {
            return false;
        };
        if let ContextMenuOutcome::Activate(action) = outcome
            && let Some(Msg::Menu(pick)) = action.downcast_ref::<Msg>()
        {
            self.handle_menu(pick.clone(), cx);
        }
        cx.window.request_redraw();
        true
    }
}

fn main() -> Result<(), quark_app::RunError> {
    let mut keys = KeyBindings::new();
    keys.bind("mod+d", None, Msg::ToggleDense)
        .expect("binding parses");
    keys.bind("mod+d", Some("task-list"), Msg::Duplicate)
        .expect("binding parses");
    let adapter = UiAdapter::new(Demo::new(), "Layout Demo").with_key_bindings(keys);
    quark_app::run(
        adapter,
        WindowOptions {
            title: "Layout Demo".into(),
            size: (1100.0, 720.0),
            ..WindowOptions::default()
        },
    )
}
