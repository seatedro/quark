//! The drawn menu bar over an app's platform menus: keyboard navigation
//! (F10, Alt taps, arrows, submenus), pointer use, and shown shortcuts.

use accesskit::Role;
use quark_components::ContextMenuOutcome;
use quark_ui::element::{AnyElement, IntoAnyElement, div};
use quark_ui::style::Styled;
use winit::keyboard::{ModifiersState, NamedKey};

use crate::InputEvent;
use crate::input::{KeyChord, KeyKind};
use crate::platform::drawn_menu::{DrawnMenuBar, MenuPick};
use crate::platform::menu::{Menu, MenuAction, MenuItem, MenuRole};
use crate::testing::{By, UiTestHarness};
use crate::ui::{UiApp, UiContext, ViewContext};

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Title(usize),
    Pick(MenuPick),
}

impl From<Msg> for quark_ui::Action {
    fn from(msg: Msg) -> Self {
        quark_ui::Action::new(msg)
    }
}

fn menus() -> Vec<Menu> {
    vec![
        Menu::new(
            "File",
            vec![
                MenuAction::new("new", "New").shortcut("mod+n").into(),
                Menu::new(
                    "Open Recent",
                    vec![
                        MenuAction::new("recent-a", "a.txt").into(),
                        MenuAction::new("recent-b", "b.txt").into(),
                    ],
                )
                .into(),
                MenuItem::Separator,
                MenuRole::Quit.into(),
            ],
        ),
        Menu::new(
            "Edit",
            vec![
                MenuAction::new("undo", "Undo").enabled(false).into(),
                MenuAction::new("find", "Find").into(),
            ],
        ),
        Menu::new(
            "View",
            vec![MenuAction::new("wrap", "Word Wrap").checked(true).into()],
        ),
    ]
}

struct Editor {
    bar: DrawnMenuBar,
    /// Picked item ids, and `role:` plus the role for standard items.
    picks: Vec<String>,
}

impl Editor {
    fn pick(&mut self, pick: &MenuPick) {
        self.picks.push(match pick {
            MenuPick::Item(id) => id.clone(),
            MenuPick::Role(role) => format!("role:{role:?}"),
        });
    }
}

impl UiApp for Editor {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let size = cx.frame.logical_size();
        div()
            .size_full()
            .flex_col()
            .child(
                self.bar
                    .bar
                    .render(size, cx.theme, |i| Msg::Title(i).into()),
            )
            .child(div().flex_1())
            .into_any()
    }

    fn update(&mut self, msg: Msg, _cx: &mut UiContext) {
        match msg {
            Msg::Title(index) => self.bar.bar.title_clicked(index),
            Msg::Pick(pick) => {
                self.bar.bar.close();
                self.pick(&pick);
            }
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let Some(outcome) = self.bar.event(event) else {
            return false;
        };
        if let ContextMenuOutcome::Activate(action) = outcome
            && let Some(Msg::Pick(pick)) = action.downcast_ref::<Msg>()
        {
            self.pick(&pick.clone());
        }
        cx.window.request_redraw();
        true
    }
}

fn harness() -> UiTestHarness<Editor> {
    let app = Editor {
        bar: DrawnMenuBar::new(&menus(), Msg::Pick),
        picks: Vec::new(),
    };
    let mut ui = UiTestHarness::new(app, (400.0, 300.0), 1.0);
    ui.frame();
    ui
}

fn alt_tap(ui: &mut UiTestHarness<Editor>) {
    let alt = |modifiers| KeyChord {
        logical: KeyKind::Named(NamedKey::Alt),
        physical: None,
        modifiers,
        repeat: false,
    };
    ui.send_event(InputEvent::KeyPress(alt(ModifiersState::ALT)));
    ui.send_event(InputEvent::KeyRelease(alt(ModifiersState::empty())));
}

#[test]
fn keyboard_walks_the_bar_menus_and_submenus() {
    const ALT: &str = "<alt tap>";
    let cases: [(&[&str], &str); 8] = [
        (&["f10", "enter"], "new"),
        (
            &["f10", "arrowdown", "arrowright", "arrowdown", "enter"],
            "recent-b",
        ),
        // Right on a plain row moves to the next menu, whose disabled
        // first row is skipped.
        (&["f10", "arrowright", "enter"], "find"),
        (&["f10", "arrowleft", "space"], "wrap"),
        (&["f10", "arrowup", "enter"], "role:Quit"),
        (&[ALT, "arrowright", "arrowdown", "enter"], "find"),
        // Escape backs out a level at a time, then leaves the bar.
        (&["f10", "escape", "escape", "enter"], "-"),
        (&["f10", "f10", "enter"], "-"),
    ];
    for (keys, expected) in cases {
        let mut ui = harness();
        for key in keys {
            if *key == ALT {
                alt_tap(&mut ui);
            } else {
                ui.key(key);
            }
        }
        let picked = ui.app().picks.last().map_or("-", String::as_str);
        assert_eq!(picked, expected, "{keys:?}");
        assert!(
            !ui.app().bar.bar.is_active(),
            "{keys:?} left the bar active"
        );
    }
}

#[test]
fn pointer_opens_titles_follows_hover_and_shows_shortcuts() {
    let mut ui = harness();
    let file = ui.find(By::role_name(Role::MenuItem, "File")).center();
    ui.click(file);
    let open_text = ui.painted_text();
    // With a menu open, moving onto another title switches to it.
    let edit = ui.find(By::role_name(Role::MenuItem, "Edit")).center();
    ui.pointer_move(edit);
    let find = ui.find(By::role_name(Role::MenuItem, "Find")).center();
    ui.click(find);

    let shortcut = crate::keymap::format_binding("mod+n");
    assert!(
        open_text.contains("New") && open_text.contains(&shortcut),
        "{open_text}"
    );
    assert_eq!(ui.app().picks, ["find"]);
    assert_eq!(ui.app().bar.bar.open_menu(), None);
}

// Catches an open menu that only a title or an item can close, so a click
// elsewhere in the window leaves it hanging.
#[test]
fn a_press_outside_the_open_menu_closes_it_without_a_pick() {
    let mut ui = harness();
    let file = ui.find(By::role_name(Role::MenuItem, "File")).center();
    ui.click(file);
    assert_eq!(ui.app().bar.bar.open_menu(), Some(0));

    ui.click((350.0, 280.0));

    assert_eq!(ui.app().bar.bar.open_menu(), None);
    assert!(ui.app().picks.is_empty(), "{:?}", ui.app().picks);
}
