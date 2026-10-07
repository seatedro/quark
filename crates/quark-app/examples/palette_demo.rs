//! The command palette, undo toasts, context menu submenus, and hover cards
//! from quark-components, wired into a `UiApp`.
//!
//! Ctrl+K (Cmd+K on macOS) opens the palette over a notes field. Its
//! sections (commands, threads, files) come from providers this example
//! registers; quark knows nothing about threads or files. "Delete note"
//! shows a toast whose Undo button (or Ctrl+Z outside the field) brings
//! the note back for five seconds. Ctrl+Shift+O or "Options" opens a context
//! menu with a submenu, and resting on "Hover me" opens a hover card.

use std::time::Duration;

use accesskit::Role;
use quark::view;
use quark_app::quark_ui::element::{AnyElement, Binding, IntoAnyElement, div, text, text_input};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome, TextField};
use quark_app::quark_ui::{Action, FocusId};
use quark_app::{InputEvent, UiApp, UiContext, ViewContext, WindowOptions};
use quark_components::{
    CommandPalette, ContextMenuEntry, ContextMenuOutcome, ContextMenuState, HoverCardState,
    PALETTE_INPUT, PaletteEvent, PaletteItem, PaletteOutcome, PaletteProvider, Side, Toast,
    ToastQueue,
};
use quark_render::Rect;

const NOTES_FIELD: FocusId = FocusId::from_key("palette_demo.notes");
const NOTE: &str = "Buy oat milk";
const UNDO_TIMEOUT_MS: u64 = 5_000;
/// The hover card's anchor, placed absolutely so pointer moves can be
/// tested against it.
const HOVER_ANCHOR: Rect = Rect {
    x: 40.0,
    y: 360.0,
    width: 140.0,
    height: 32.0,
};
/// Where Ctrl+Shift+O opens the menu: under the buttons. A click on
/// "Options" opens it at the pointer instead.
const MENU_AT: (f32, f32) = (40.0, 220.0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sort {
    Name,
    Date,
}

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Palette(PaletteEvent),
    NewThread,
    ToggleWrap,
    OpenThread(&'static str),
    OpenFile(&'static str),
    DeleteNote,
    RestoreNote(String),
    DismissToast(usize),
    ToastButton(u64, usize),
    OpenMenu,
    Rename,
    SortBy(Sort),
    OpenProfile,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

/// What the providers read: the app's threads and files.
struct Library {
    threads: Vec<&'static str>,
    files: Vec<&'static str>,
}

fn binding(text: &str) -> Binding {
    text.parse().expect("valid binding")
}

struct Commands;

impl PaletteProvider<Library> for Commands {
    fn title(&self) -> &str {
        "Commands"
    }

    fn collect(&self, _: &Library, out: &mut Vec<PaletteItem>) {
        out.push(PaletteItem::new(1, "New thread", Msg::NewThread).binding(binding("mod+n")));
        out.push(PaletteItem::new(2, "Delete note", Msg::DeleteNote));
        out.push(PaletteItem::new(3, "Toggle word wrap", Msg::ToggleWrap));
    }
}

struct Threads;

impl PaletteProvider<Library> for Threads {
    fn title(&self) -> &str {
        "Threads"
    }

    fn collect(&self, library: &Library, out: &mut Vec<PaletteItem>) {
        for (i, title) in library.threads.iter().enumerate() {
            out.push(PaletteItem::new(
                100 + i as u64,
                *title,
                Msg::OpenThread(title),
            ));
        }
    }
}

struct Files;

impl PaletteProvider<Library> for Files {
    fn title(&self) -> &str {
        "Files"
    }

    fn collect(&self, library: &Library, out: &mut Vec<PaletteItem>) {
        for (i, path) in library.files.iter().enumerate() {
            out.push(PaletteItem::new(200 + i as u64, *path, Msg::OpenFile(path)));
        }
    }
}

struct PaletteDemo {
    library: Library,
    palette: CommandPalette<Library>,
    toasts: ToastQueue,
    menu: ContextMenuState,
    card: HoverCardState,
    notes: TextField,
    note: Option<String>,
    status: String,
    sort: Sort,
    wrap: bool,
    /// The clock as of the last event or frame; `edit_text` gets no context.
    now_ms: u64,
    pointer: Option<(f32, f32)>,
}

impl PaletteDemo {
    fn new() -> Self {
        let mut palette = CommandPalette::new();
        palette.register(Commands);
        palette.register(Threads);
        palette.register(Files);
        Self {
            library: Library {
                threads: vec!["Fix flaky login test", "Refactor renderer"],
                files: vec!["README.md", "src/render.rs"],
            },
            palette,
            toasts: ToastQueue::new(),
            menu: ContextMenuState::default(),
            card: HoverCardState::new((220.0, 96.0), Side::Right),
            notes: TextField::new(""),
            note: Some(NOTE.to_owned()),
            status: "Ready".to_owned(),
            sort: Sort::Name,
            wrap: false,
            now_ms: 0,
            pointer: None,
        }
    }

    fn open_menu(&mut self, (x, y): (f32, f32)) {
        let entries = vec![
            ContextMenuEntry::item("Rename", Msg::Rename).binding(&binding("mod+r")),
            ContextMenuEntry::submenu(
                "Sort by",
                vec![
                    ContextMenuEntry::item("Name", Msg::SortBy(Sort::Name))
                        .checked(self.sort == Sort::Name),
                    ContextMenuEntry::item("Date", Msg::SortBy(Sort::Date))
                        .checked(self.sort == Sort::Date),
                ],
            ),
            ContextMenuEntry::item("Word wrap", Msg::ToggleWrap).checked(self.wrap),
            ContextMenuEntry::separator(),
            ContextMenuEntry::item("Delete", Msg::DeleteNote)
                .destructive()
                .disabled_if(self.note.is_none()),
        ];
        self.menu.open(entries, x, y);
    }

    fn run(&mut self, action: Action, cx: &mut UiContext) {
        if let Some(msg) = action.downcast_ref::<Msg>().cloned() {
            self.update(msg, cx);
        }
    }

    fn palette_outcome(&mut self, outcome: PaletteOutcome, cx: &mut UiContext) {
        match outcome {
            PaletteOutcome::Handled => {}
            PaletteOutcome::Run {
                action,
                restore_focus,
            } => {
                cx.set_focus(restore_focus);
                self.run(action, cx);
            }
            PaletteOutcome::Closed { restore_focus } => cx.set_focus(restore_focus),
        }
        cx.window.request_redraw();
    }

    fn button(id: &str, label: &str, msg: Msg, cx: &ViewContext) -> AnyElement {
        let colors = &cx.theme.colors;
        view! {
            <div accessibility_id={id} accessibility_role={Role::Button} aria-label={label}
                 on:click={msg}
                 class="px-[14] h-8 items-center justify-center rounded-[8]
                        bg-[colors.element_background]"
                 hover_bg={colors.element_hover}>
                <text color={colors.text}>{label}</text>
            </div>
        }
    }

    fn pointer_moved(&mut self, pointer: Option<(f32, f32)>, cx: &mut UiContext) {
        self.pointer = pointer;
        let mut changed = self.toasts.pointer_moved(pointer, self.now_ms);
        if let Some((x, y)) = pointer {
            changed |= self.menu.pointer_moved(x, y);
        }
        changed |= self
            .card
            .pointer_moved(pointer, &[(1, HOVER_ANCHOR)], self.now_ms);
        if changed {
            cx.window.request_redraw();
        }
    }

    fn key(&mut self, pressed: &Binding, cx: &mut UiContext) -> bool {
        if let Some(outcome) = self.palette.handle_key(pressed) {
            self.palette_outcome(outcome, cx);
            return true;
        }
        if let Some(outcome) = self.menu.handle_key(pressed) {
            if let ContextMenuOutcome::Activate(action) = outcome {
                self.run(action, cx);
            }
            cx.window.request_redraw();
            return true;
        }
        if binding("mod+k").matches(pressed) {
            let focus = self.palette.open(&self.library, cx.focus());
            cx.set_focus(Some(focus));
            cx.window.request_redraw();
            return true;
        }
        if binding("mod+shift+o").matches(pressed) {
            self.open_menu(MENU_AT);
            cx.window.request_redraw();
            return true;
        }
        let in_field = cx.focus() == Some(NOTES_FIELD);
        if let Some(action) = self.toasts.undo_shortcut(pressed, in_field, self.now_ms) {
            self.run(action, cx);
            return true;
        }
        false
    }
}

impl UiApp for PaletteDemo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let now_ms = cx.frame.elapsed().as_millis() as u64;
        self.now_ms = now_ms;
        let window = cx.frame.size();
        let scale = cx.theme.metrics.ui_scale();
        self.card.tick(now_ms);
        let toast_wake = self.toasts.tick(cx.animations(), now_ms);
        if let Some(at) = toast_wake.into_iter().chain(self.card.next_wake_ms()).min() {
            cx.frame
                .request_frame_in(Duration::from_millis(at.saturating_sub(now_ms)));
        }

        let toasts = self.toasts.stack(
            cx.animations(),
            window,
            scale,
            0.0,
            now_ms,
            &[],
            |index| Msg::DismissToast(index).into(),
            |id, button| Msg::ToastButton(id, button).into(),
        );
        let theme = cx.theme;
        let colors = &theme.colors;
        let note = self.note.clone().unwrap_or_else(|| "No note".to_owned());
        let card_content = view! {
            <div class="flex-col gap-2">
                <text class="font-semibold" color={colors.text_strong}>"Ada Lovelace"</text>
                {Self::button("demo.profile", "Open profile", Msg::OpenProfile, cx)}
            </div>
        };
        let focused = cx.is_focused(PALETTE_INPUT);
        view! {
            <div w={window.0} h={window.1} class="bg-[colors.background] flex-col p-10 gap-3">
                <text class="text-sm" wrap_width={window.0 - 80.0} color={colors.text_muted}>
                    "Ctrl+K opens the command palette. Delete note shows an undo toast. \
                     Options (or Ctrl+Shift+O) opens a menu with a submenu. Rest the \
                     pointer on Hover me for a hover card."
                </text>
                <text_input("Notes", "") field={&self.notes} placeholder="Type a note"
                            focus_target={NOTES_FIELD} focused={cx.is_focused(NOTES_FIELD)}
                            w={360.0} h={44.0} />
                <text color={colors.text}>{self.status.clone()}</text>
                <text color={colors.text_muted}>{note}</text>
                <div class="flex-row gap-2">
                    {Self::button("demo.delete", "Delete note", Msg::DeleteNote, cx)}
                    {Self::button("demo.options", "Options", Msg::OpenMenu, cx)}
                </div>
                <div class="absolute" left={HOVER_ANCHOR.x} top={HOVER_ANCHOR.y}
                     w={HOVER_ANCHOR.width} h={HOVER_ANCHOR.height}
                     class="items-center justify-center rounded-[6] border-[colors.border]">
                    <text color={colors.text}>"Hover me"</text>
                </div>
                {toasts}
                if let Some(menu) = self.menu.render(window, theme) {
                    {menu}
                }
                if let Some(card) = self.card.render(card_content, window, theme) {
                    {card}
                }
                if let Some(palette) =
                    self.palette.render(window, theme, focused, |event| Msg::Palette(event).into())
                {
                    {palette}
                }
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        self.now_ms = cx.window.elapsed().as_millis() as u64;
        // Any choice from the menu closes it.
        self.menu.close();
        match msg {
            Msg::Palette(event) => {
                if let Some(outcome) = self.palette.apply(event) {
                    self.palette_outcome(outcome, cx);
                }
            }
            Msg::NewThread => self.status = "New thread".into(),
            Msg::ToggleWrap => {
                self.wrap = !self.wrap;
                self.status = format!("Word wrap {}", if self.wrap { "on" } else { "off" });
            }
            Msg::OpenThread(title) => self.status = format!("Opened thread {title}"),
            Msg::OpenFile(path) => self.status = format!("Opened {path}"),
            Msg::DeleteNote => {
                if let Some(note) = self.note.take() {
                    let toast =
                        Toast::info("Note deleted").undo(Msg::RestoreNote(note), UNDO_TIMEOUT_MS);
                    self.toasts.push(toast, self.now_ms);
                }
            }
            Msg::RestoreNote(note) => {
                self.note = Some(note);
                self.status = "Note restored".into();
            }
            Msg::DismissToast(index) => self.toasts.dismiss_index(index),
            Msg::ToastButton(id, button) => {
                if let Some(action) = self.toasts.activate(id, button, self.now_ms) {
                    self.run(action, cx);
                }
            }
            Msg::OpenMenu => self.open_menu(self.pointer.unwrap_or(MENU_AT)),
            Msg::Rename => self.status = "Rename".into(),
            Msg::SortBy(sort) => {
                self.sort = sort;
                self.status = format!("Sorted by {sort:?}").to_lowercase();
            }
            Msg::OpenProfile => self.status = "Opened profile".into(),
        }
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        match target {
            PALETTE_INPUT => self.palette.edit(command, self.now_ms),
            NOTES_FIELD => self.notes.apply_at(command, self.now_ms),
            _ => TextEditOutcome::default(),
        }
    }

    fn set_preedit(&mut self, target: FocusId, text: String, cursor: Option<(usize, usize)>) {
        match target {
            PALETTE_INPUT => self.palette.set_preedit(text, cursor),
            NOTES_FIELD => self.notes.set_preedit(text, cursor),
            _ => {}
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        self.now_ms = cx.window.elapsed().as_millis() as u64;
        match event {
            InputEvent::PointerMoved { x, y } => {
                self.pointer_moved(Some((*x, *y)), cx);
                false
            }
            InputEvent::PointerLeft => {
                self.pointer_moved(None, cx);
                false
            }
            InputEvent::KeyPress(chord) => match chord.binding() {
                Some(pressed) => self.key(&pressed, cx),
                None => false,
            },
            _ => false,
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    quark_app::run_ui(
        PaletteDemo::new(),
        WindowOptions {
            title: "Quark palette demo".into(),
            size: (800.0, 600.0),
            ..WindowOptions::default()
        },
    )
}

#[cfg(test)]
mod tests {
    use quark_app::testing::{By, UiTestHarness};

    use super::*;

    fn demo() -> UiTestHarness<PaletteDemo> {
        UiTestHarness::new(PaletteDemo::new(), (800.0, 600.0), 1.0)
    }

    fn status(ui: &UiTestHarness<PaletteDemo>) -> String {
        ui.app().status.clone()
    }

    // Catches a palette that forgets the focus it took, or leaves focus in
    // its removed search field.
    #[test]
    fn closing_the_palette_restores_the_focus_it_took() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::TextInput, "Notes"));
        assert_eq!(ui.focus(), Some(NOTES_FIELD));

        ui.key("mod+k");
        assert_eq!(ui.focus(), Some(PALETTE_INPUT));
        ui.find(By::role_name(Role::Dialog, "Command palette"));

        ui.key("escape");
        assert_eq!(ui.focus(), Some(NOTES_FIELD));
        assert!(
            ui.try_find(By::role_name(Role::Dialog, "Command palette"))
                .is_none()
        );
    }

    // Catches quick select counting headings, ignoring the filter, or
    // picking from the unfiltered list.
    #[test]
    fn mod_number_runs_that_result_of_the_filtered_list() {
        let mut ui = demo();
        ui.key("mod+k");
        ui.type_text("render");
        // Results: "Refactor renderer" under Threads, "src/render.rs" under
        // Files.
        ui.key("mod+2");
        assert_eq!(status(&ui), "Opened src/render.rs");
        assert!(!ui.app().palette.is_open());
    }

    // Catches an Undo button that does nothing or stops working before its
    // timeout.
    #[test]
    fn undo_button_restores_within_the_timeout() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::Button, "Delete note"));
        assert_eq!(ui.app().note, None);

        ui.advance(UNDO_TIMEOUT_MS - 1_000);
        ui.click_node(By::role_name(Role::Button, "Undo"));
        assert_eq!(ui.app().note.as_deref(), Some(NOTE));
    }

    // Catches an undo toast that never leaves, or an undo that still runs
    // through the shortcut after the toast timed out.
    #[test]
    fn undo_expires_after_the_timeout() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::Button, "Delete note"));

        ui.advance(UNDO_TIMEOUT_MS + 500);
        assert!(ui.try_find(By::role_name(Role::Button, "Undo")).is_none());
        ui.key("mod+z");
        assert_eq!(ui.app().note, None);
    }

    // Catches the undo shortcut stealing mod+z from a focused text field.
    #[test]
    fn undo_shortcut_skips_text_fields() {
        let mut ui = demo();
        ui.click_node(By::role_name(Role::Button, "Delete note"));
        ui.click_node(By::role_name(Role::TextInput, "Notes"));

        ui.key("mod+z");
        assert_eq!(ui.app().note, None);

        // A click on the background clears focus; now the toast gets it.
        ui.click((700.0, 150.0));
        ui.key("mod+z");
        assert_eq!(ui.app().note.as_deref(), Some(NOTE));
    }

    fn open_menus(ui: &UiTestHarness<PaletteDemo>) -> usize {
        ui.find_all(By::role(Role::Menu)).len()
    }

    // Catches Down landing on the wrong row, Right not opening the
    // highlighted submenu, Left not closing it, or Enter not choosing the
    // submenu's highlighted item.
    #[test]
    fn submenus_open_and_close_from_the_keyboard() {
        let mut ui = demo();
        ui.key("mod+shift+o");
        // Rename, then Sort by.
        ui.key("arrowdown");
        ui.key("arrowdown");
        ui.key("arrowright");
        assert_eq!(open_menus(&ui), 2);

        ui.key("arrowleft");
        assert_eq!(open_menus(&ui), 1);

        // The submenu opens on Name; Down moves to Date.
        ui.key("arrowright");
        ui.key("arrowdown");
        ui.key("enter");
        assert_eq!(status(&ui), "sorted by date");
        assert_eq!(open_menus(&ui), 0);
    }

    fn card_open(ui: &UiTestHarness<PaletteDemo>) -> bool {
        ui.try_find(By::role(Role::Tooltip)).is_some()
    }

    fn anchor_center() -> (f32, f32) {
        (
            HOVER_ANCHOR.x + HOVER_ANCHOR.width / 2.0,
            HOVER_ANCHOR.y + HOVER_ANCHOR.height / 2.0,
        )
    }

    // Catches a card that opens at once or never opens.
    #[test]
    fn hover_card_opens_after_the_pointer_rests_for_its_delay() {
        let mut ui = demo();
        ui.pointer_move(anchor_center());
        ui.advance(400);
        assert!(!card_open(&ui));

        ui.advance(100);
        assert!(card_open(&ui));
    }

    // Catches a card that closes when the pointer moves onto it, which
    // would make its content unclickable.
    #[test]
    fn hover_card_stays_open_while_the_pointer_is_on_it() {
        let mut ui = demo();
        ui.pointer_move(anchor_center());
        ui.advance(500);
        let card = ui.find(By::role(Role::Tooltip));

        ui.pointer_move(card.center());
        ui.advance(1_000);
        ui.click_node(By::role_name(Role::Button, "Open profile"));
        assert_eq!(status(&ui), "Opened profile");
    }
}
