//! Application menus as data. Hand a `Vec<Menu>` to
//! [`EventContext::set_menus`]; picks arrive as [`AppEvent::Menu`] with the
//! item's id.
//!
//! - **macOS**: the global menu bar. The first menu becomes the
//!   application menu (its label is replaced by the app name), so start with
//!   [`Menu::app`].
//! - **Windows**: a menu bar in every window, including windows opened later.
//! - **Linux and the BSDs**: most desktops have no global menu bar, so
//!   nothing native is shown. The menus stay readable through
//!   [`EventContext::menus`], for an app that draws its own menu bar: show
//!   [`MenuAction::accelerator_label`] next to each item, call the app's own
//!   handler for [`MenuItem::Action`], and [`EventContext::perform_role`] for
//!   [`MenuItem::Role`]. [`native_menu_bar`] tells the cases apart.
//!
//! Accelerators are keymap [`Binding`]s, so a menu shows the same shortcut
//! the app's keymap handles. Where a native menu bar exists it takes the key
//! first: the press arrives as [`AppEvent::Menu`] instead of an
//! [`InputEvent::KeyPress`], so route both to the same command. Bindings a
//! menu cannot show (several strokes, Cmd off macOS, keys without a key code)
//! are left off the native item and still reach the app as key presses.
//!
//! [`EventContext::set_menus`]: crate::EventContext::set_menus
//! [`EventContext::menus`]: crate::EventContext::menus
//! [`EventContext::perform_role`]: crate::EventContext::perform_role
//! [`AppEvent::Menu`]: crate::AppEvent::Menu
//! [`InputEvent::KeyPress`]: crate::InputEvent::KeyPress

use quark_ui::element::Binding;

/// Whether [`crate::EventContext::set_menus`] shows a native menu bar on this
/// platform.
pub const fn native_menu_bar() -> bool {
    cfg!(any(target_os = "macos", windows))
}

/// A top-level menu or a submenu.
#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub label: String,
    pub enabled: bool,
    pub items: Vec<MenuItem>,
}

impl Menu {
    pub fn new(label: impl Into<String>, items: Vec<MenuItem>) -> Self {
        Self {
            label: label.into(),
            enabled: true,
            items,
        }
    }

    /// The standard application menu: About, then Services and the Hide
    /// items on macOS, then Quit. `name` labels the menu where the platform
    /// does not substitute the app name.
    pub fn app(name: impl Into<String>) -> Self {
        use MenuRole::*;
        Self::new(
            name,
            [
                About, Separator, Services, Separator, Hide, HideOthers, ShowAll, Separator, Quit,
            ]
            .map(MenuItem::from)
            .to_vec(),
        )
    }

    /// The standard Edit menu. Its items act on the focused window as if the
    /// user had typed their shortcuts.
    pub fn edit() -> Self {
        use MenuRole::*;
        Self::new(
            "Edit",
            [
                Undo, Redo, Separator, Cut, Copy, Paste, Separator, SelectAll,
            ]
            .map(MenuItem::from)
            .to_vec(),
        )
    }

    /// The standard Window menu.
    pub fn window() -> Self {
        use MenuRole::*;
        Self::new(
            "Window",
            [
                Minimize,
                Zoom,
                ToggleFullscreen,
                Separator,
                CloseWindow,
                BringAllToFront,
            ]
            .map(MenuItem::from)
            .to_vec(),
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MenuItem {
    Action(MenuAction),
    Submenu(Menu),
    Separator,
    Role(MenuRole),
}

impl From<MenuAction> for MenuItem {
    fn from(action: MenuAction) -> Self {
        Self::Action(action)
    }
}

impl From<Menu> for MenuItem {
    fn from(menu: Menu) -> Self {
        Self::Submenu(menu)
    }
}

impl From<MenuRole> for MenuItem {
    /// `MenuRole::Separator` is shorthand for [`MenuItem::Separator`] in
    /// role lists.
    fn from(role: MenuRole) -> Self {
        match role {
            MenuRole::Separator => Self::Separator,
            role => Self::Role(role),
        }
    }
}

/// An item the app handles: picking it sends [`crate::AppEvent::Menu`] with
/// `id`.
#[derive(Debug, Clone, PartialEq)]
pub struct MenuAction {
    pub id: String,
    pub label: String,
    pub accelerator: Option<Binding>,
    pub enabled: bool,
    /// `Some` draws a check box in that state.
    pub checked: Option<bool>,
}

impl MenuAction {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            accelerator: None,
            enabled: true,
            checked: None,
        }
    }

    pub fn accelerator(mut self, binding: Binding) -> Self {
        self.accelerator = Some(binding);
        self
    }

    /// The accelerator from a keymap binding string such as
    /// `"mod+shift+p"`, as [`crate::keymap::Keymap::active_bindings`]
    /// returns. Strings that do not parse as one stroke set none.
    pub fn shortcut(mut self, binding: &str) -> Self {
        self.accelerator = binding.parse().ok();
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = Some(checked);
        self
    }

    /// The accelerator for display, such as `Ctrl+Shift+P`.
    pub fn accelerator_label(&self) -> Option<String> {
        let binding = self.accelerator.as_ref()?;
        Some(crate::keymap::format_binding(&binding.to_string()))
    }
}

/// A standard item with platform behavior. Items for features a platform
/// lacks (Services or Hide on Windows) are left out there.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MenuRole {
    /// The standard About panel (macOS) or dialog (Windows).
    About,
    /// The macOS Services submenu.
    Services,
    Hide,
    HideOthers,
    ShowAll,
    /// Asks [`crate::App::close_requested`] for every window and exits once
    /// all of them agree.
    Quit,
    /// Asks [`crate::App::close_requested`] for the focused window.
    CloseWindow,
    Minimize,
    /// Maximize or restore the focused window.
    Zoom,
    ToggleFullscreen,
    BringAllToFront,
    /// The edit roles deliver their standard shortcut (`mod+z`,
    /// `mod+shift+z`, `mod+x`, `mod+c`, `mod+v`, `mod+a`) to the focused
    /// window as a key press, so text fields handle them as typed.
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// Shorthand for [`MenuItem::Separator`]; never stored as a role.
    Separator,
}

impl MenuRole {
    pub fn label(self) -> &'static str {
        match self {
            Self::About => "About",
            Self::Services => "Services",
            Self::Hide => "Hide",
            Self::HideOthers => "Hide Others",
            Self::ShowAll => "Show All",
            Self::Quit => "Quit",
            Self::CloseWindow => "Close Window",
            Self::Minimize => "Minimize",
            Self::Zoom => "Zoom",
            Self::ToggleFullscreen => "Toggle Full Screen",
            Self::BringAllToFront => "Bring All to Front",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
            Self::Separator => "",
        }
    }

    /// The shortcut the role shows and answers to, as a keymap binding.
    pub fn shortcut(self) -> Option<&'static str> {
        Some(match self {
            Self::Quit => "mod+q",
            Self::CloseWindow => "mod+w",
            Self::Minimize if cfg!(target_os = "macos") => "cmd+m",
            Self::ToggleFullscreen if cfg!(target_os = "macos") => "cmd+ctrl+f",
            Self::ToggleFullscreen => "f11",
            Self::Undo => "mod+z",
            Self::Redo => "mod+shift+z",
            Self::Cut => "mod+x",
            Self::Copy => "mod+c",
            Self::Paste => "mod+v",
            Self::SelectAll => "mod+a",
            _ => return None,
        })
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    /// Roles only the macOS menu bar has.
    pub(crate) fn macos_only(self) -> bool {
        matches!(
            self,
            Self::Services | Self::Hide | Self::HideOthers | Self::ShowAll | Self::BringAllToFront
        )
    }

    /// The edit key the role types, as a lowercase character.
    pub(crate) fn edit_key(self) -> Option<(&'static str, bool)> {
        Some(match self {
            Self::Undo => ("z", false),
            Self::Redo => ("z", true),
            Self::Cut => ("x", false),
            Self::Copy => ("c", false),
            Self::Paste => ("v", false),
            Self::SelectAll => ("a", false),
            _ => return None,
        })
    }

    /// Stable names for native item ids.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::About => "about",
            Self::Services => "services",
            Self::Hide => "hide",
            Self::HideOthers => "hide-others",
            Self::ShowAll => "show-all",
            Self::Quit => "quit",
            Self::CloseWindow => "close-window",
            Self::Minimize => "minimize",
            Self::Zoom => "zoom",
            Self::ToggleFullscreen => "toggle-fullscreen",
            Self::BringAllToFront => "bring-all-to-front",
            Self::Undo => "undo",
            Self::Redo => "redo",
            Self::Cut => "cut",
            Self::Copy => "copy",
            Self::Paste => "paste",
            Self::SelectAll => "select-all",
            Self::Separator => "separator",
        }
    }

    #[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        use MenuRole::*;
        [
            About,
            Services,
            Hide,
            HideOthers,
            ShowAll,
            Quit,
            CloseWindow,
            Minimize,
            Zoom,
            ToggleFullscreen,
            BringAllToFront,
            Undo,
            Redo,
            Cut,
            Copy,
            Paste,
            SelectAll,
        ]
        .into_iter()
        .find(|role| role.name() == name)
    }
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
/// An accelerator as native menus take it: modifiers and a W3C
/// `KeyboardEvent.code` name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Accelerator {
    pub(crate) command: bool,
    pub(crate) control: bool,
    pub(crate) alt: bool,
    pub(crate) shift: bool,
    pub(crate) code: &'static str,
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
impl Accelerator {
    /// The accelerator for `binding` on macOS (`macos`) or elsewhere: `mod`
    /// becomes Cmd on macOS and Ctrl elsewhere. `None` when the menu cannot
    /// show it: Cmd (the Windows key) off macOS, or a key with no code.
    pub(crate) fn from_binding(binding: &Binding, macos: bool) -> Option<Self> {
        let mods = binding.mods;
        if mods.cmd && !macos {
            return None;
        }
        Some(Self {
            command: mods.cmd || (mods.primary && macos),
            control: mods.ctrl || (mods.primary && !macos),
            alt: mods.alt,
            shift: mods.shift,
            code: key_code(&binding.key)?,
        })
    }
}

#[cfg_attr(not(any(target_os = "macos", windows)), allow(dead_code))]
/// The physical key code for a [`Binding`] key name, assuming a US layout as
/// menu accelerators do.
fn key_code(key: &str) -> Option<&'static str> {
    const LETTERS: [&str; 26] = [
        "KeyA", "KeyB", "KeyC", "KeyD", "KeyE", "KeyF", "KeyG", "KeyH", "KeyI", "KeyJ", "KeyK",
        "KeyL", "KeyM", "KeyN", "KeyO", "KeyP", "KeyQ", "KeyR", "KeyS", "KeyT", "KeyU", "KeyV",
        "KeyW", "KeyX", "KeyY", "KeyZ",
    ];
    const DIGITS: [&str; 10] = [
        "Digit0", "Digit1", "Digit2", "Digit3", "Digit4", "Digit5", "Digit6", "Digit7", "Digit8",
        "Digit9",
    ];
    const FUNCTION: [&str; 24] = [
        "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "F13", "F14",
        "F15", "F16", "F17", "F18", "F19", "F20", "F21", "F22", "F23", "F24",
    ];
    let mut chars = key.chars();
    if let (Some(ch), None) = (chars.next(), chars.next()) {
        return Some(match ch {
            'a'..='z' => LETTERS[(ch as u8 - b'a') as usize],
            '0'..='9' => DIGITS[(ch as u8 - b'0') as usize],
            ',' => "Comma",
            '.' => "Period",
            '/' => "Slash",
            ';' => "Semicolon",
            '\'' => "Quote",
            '[' => "BracketLeft",
            ']' => "BracketRight",
            '\\' => "Backslash",
            '-' => "Minus",
            '=' => "Equal",
            '`' => "Backquote",
            _ => return None,
        });
    }
    if let Some(n) = key.strip_prefix('f').and_then(|n| n.parse::<usize>().ok()) {
        return FUNCTION.get(n.checked_sub(1)?).copied();
    }
    Some(match key {
        "enter" => "Enter",
        "tab" => "Tab",
        "escape" => "Escape",
        "space" => "Space",
        "backspace" => "Backspace",
        "delete" => "Delete",
        "insert" => "Insert",
        "home" => "Home",
        "end" => "End",
        "pageup" => "PageUp",
        "pagedown" => "PageDown",
        "arrowup" => "ArrowUp",
        "arrowdown" => "ArrowDown",
        "arrowleft" => "ArrowLeft",
        "arrowright" => "ArrowRight",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `binding` as a native accelerator on macOS or elsewhere, printed as
    /// `Cmd+Ctrl+Alt+Shift+Code`, or `-` when it has none.
    fn accelerator(binding: &str, macos: bool) -> String {
        let binding: Binding = binding.parse().unwrap();
        let Some(acc) = Accelerator::from_binding(&binding, macos) else {
            return "-".into();
        };
        let mut out = String::new();
        for (held, name) in [
            (acc.command, "Cmd+"),
            (acc.control, "Ctrl+"),
            (acc.alt, "Alt+"),
            (acc.shift, "Shift+"),
        ] {
            if held {
                out.push_str(name);
            }
        }
        out + acc.code
    }

    #[test]
    fn keymap_bindings_map_to_native_accelerators() {
        // (binding, macOS, Windows)
        let cases = [
            ("mod+s", "Cmd+KeyS", "Ctrl+KeyS"),
            ("mod+shift+p", "Cmd+Shift+KeyP", "Ctrl+Shift+KeyP"),
            ("ctrl+alt+1", "Ctrl+Alt+Digit1", "Ctrl+Alt+Digit1"),
            ("cmd+,", "Cmd+Comma", "-"),
            ("mod+enter", "Cmd+Enter", "Ctrl+Enter"),
            ("shift+arrowup", "Shift+ArrowUp", "Shift+ArrowUp"),
            ("f11", "F11", "F11"),
            ("mod+[", "Cmd+BracketLeft", "Ctrl+BracketLeft"),
            // No key code: left to the keymap.
            ("ctrl++", "-", "-"),
            ("f25", "-", "-"),
            ("mod+é", "-", "-"),
        ];
        for (binding, macos, windows) in cases {
            assert_eq!(accelerator(binding, true), macos, "{binding} on macOS");
            assert_eq!(accelerator(binding, false), windows, "{binding} on Windows");
        }
    }

    #[test]
    fn multi_stroke_keymap_strings_set_no_accelerator() {
        let action = MenuAction::new("go", "Go to top").shortcut("g g");
        assert_eq!(action.accelerator, None);
        assert_eq!(action.accelerator_label(), None);
    }

    #[test]
    fn role_names_round_trip() {
        let roles = Menu::app("App")
            .items
            .into_iter()
            .chain(Menu::edit().items)
            .chain(Menu::window().items)
            .filter_map(|item| match item {
                MenuItem::Role(role) => Some(role),
                _ => None,
            });
        for role in roles {
            assert_eq!(MenuRole::from_name(role.name()), Some(role));
        }
    }
}
