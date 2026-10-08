//! Window verbs, the menu bar, and the badge: [`EventContext`] calls that
//! reach past the window into the desktop.

use winit::window::{Fullscreen, UserAttentionType, WindowLevel};

use super::*;

/// Runner state behind the menu and badge calls.
#[derive(Default)]
pub(super) struct PlatformState {
    #[cfg(feature = "ui")]
    menus: Vec<crate::platform::menu::Menu>,
    #[cfg(all(feature = "ui", any(target_os = "macos", windows)))]
    menu_bar: Option<crate::platform::native_menu::NativeMenuBar>,
    #[cfg(all(feature = "ui", any(target_os = "macos", windows)))]
    menu_events_installed: bool,
    #[cfg_attr(not(windows), allow(dead_code))]
    badge: Option<u32>,
}

impl PlatformState {
    /// Give a newly opened window the menu bar and badge (Windows keeps
    /// both per window).
    pub(super) fn window_opened(&self, window: &Window) {
        #[cfg(all(feature = "ui", windows))]
        if let Some(bar) = &self.menu_bar {
            bar.attach(window);
        }
        #[cfg(windows)]
        if self.badge.is_some() {
            crate::platform::badge::set_overlay(std::iter::once(window), self.badge);
        }
        let _ = window;
    }
}

/// The key press an edit role types: `key` with the platform's primary
/// modifier, plus Shift when `shift`.
#[cfg(feature = "ui")]
pub(super) fn edit_chord(key: &str, shift: bool) -> crate::input::KeyChord {
    use winit::keyboard::KeyCode;

    let physical = match key {
        "z" => Some(KeyCode::KeyZ),
        "x" => Some(KeyCode::KeyX),
        "c" => Some(KeyCode::KeyC),
        "v" => Some(KeyCode::KeyV),
        "a" => Some(KeyCode::KeyA),
        _ => None,
    };
    let mut modifiers = if cfg!(target_os = "macos") {
        ModifiersState::SUPER
    } else {
        ModifiersState::CONTROL
    };
    if shift {
        modifiers |= ModifiersState::SHIFT;
    }
    crate::input::KeyChord {
        logical: crate::input::KeyKind::Character(key.to_owned()),
        physical,
        modifiers,
        repeat: false,
    }
}

pub(super) fn toggle_maximized(window: &Window) {
    window.set_maximized(!window.is_maximized());
}

pub(super) fn toggle_fullscreen(window: &Window) {
    let next = match window.fullscreen() {
        Some(_) => None,
        None => Some(Fullscreen::Borderless(None)),
    };
    window.set_fullscreen(next);
}

impl EventContext<'_> {
    /// Minimize the context's window.
    pub fn minimize(&mut self) {
        if let Some(window) = self.native() {
            window.set_minimized(true);
        }
    }

    /// Maximize the context's window, or restore it when it is maximized.
    pub fn toggle_maximized(&mut self) {
        if let Some(window) = self.native() {
            toggle_maximized(window);
        }
    }

    /// Make the context's window borderless fullscreen on its monitor, or
    /// leave fullscreen.
    pub fn toggle_fullscreen(&mut self) {
        if let Some(window) = self.native() {
            toggle_fullscreen(window);
        }
    }

    /// Keep the context's window above other apps' windows.
    pub fn set_always_on_top(&mut self, on_top: bool) {
        if let Some(window) = self.native() {
            window.set_window_level(if on_top {
                WindowLevel::AlwaysOnTop
            } else {
                WindowLevel::Normal
            });
        }
    }

    /// Restore the context's window if minimized, raise it, and give it
    /// keyboard focus. Desktops may refuse to take focus from another app
    /// and flash the window instead.
    pub fn focus_window(&mut self) {
        if let Some(window) = self.native() {
            window.set_minimized(false);
            window.focus_window();
        }
    }

    /// Ask for the user's attention while the app is in the background:
    /// bounce the Dock icon on macOS (once, or until the app is activated
    /// when `critical`), flash the taskbar button on Windows, and set the
    /// urgency hint on X11 (or request activation on Wayland). Ends when the
    /// window gets focus.
    pub fn request_attention(&mut self, critical: bool) {
        if let Some(window) = self.native() {
            window.request_user_attention(Some(if critical {
                UserAttentionType::Critical
            } else {
                UserAttentionType::Informational
            }));
        }
    }

    /// Show `count` on the app's Dock or taskbar icon; `None` or `Some(0)`
    /// clears it. See [`crate::platform::badge`].
    pub fn set_badge(&mut self, count: Option<u32>) {
        self.platform.badge = count;
        #[cfg(feature = "test-support")]
        if self.headless {
            return;
        }
        #[cfg(target_os = "macos")]
        crate::platform::badge::set_dock_badge(count);
        #[cfg(windows)]
        crate::platform::badge::set_overlay(
            self.windows
                .iter()
                .filter_map(|(_, entry)| entry.open())
                .map(|state| &*state.window),
            count,
        );
        #[cfg(target_os = "linux")]
        crate::platform::badge::set_launcher_badge(count);
    }

    /// Replace the app's menus. See [`crate::platform::menu`] for what each
    /// platform shows. An empty list removes the menu bar.
    #[cfg(feature = "ui")]
    pub fn set_menus(&mut self, menus: Vec<crate::platform::menu::Menu>) {
        #[cfg(any(target_os = "macos", windows))]
        self.show_native_menus(&menus);
        self.platform.menus = menus;
    }

    /// The menus last passed to [`Self::set_menus`], for apps that draw
    /// their own menu bar where the platform has none.
    #[cfg(feature = "ui")]
    pub fn menus(&self) -> &[crate::platform::menu::Menu] {
        &self.platform.menus
    }

    /// Carry out a standard menu item, as picking it from the native menu
    /// bar does: for an app's own drawn menu bar. Runs after the current
    /// callback returns.
    #[cfg(feature = "ui")]
    pub fn perform_role(&mut self, role: crate::platform::menu::MenuRole) {
        self.events.post(Posted::Role(role));
    }

    #[cfg(all(feature = "ui", any(target_os = "macos", windows)))]
    fn show_native_menus(&mut self, menus: &[crate::platform::menu::Menu]) {
        use crate::platform::native_menu::{self, NativeMenuBar};

        // muda menus are main thread only, and test threads are not it.
        #[cfg(feature = "test-support")]
        if self.headless {
            return;
        }
        if !self.platform.menu_events_installed {
            native_menu::install_event_handler(self.events.clone());
            self.platform.menu_events_installed = true;
        }
        let bar = if menus.is_empty() {
            None
        } else {
            match NativeMenuBar::build(menus) {
                Ok(bar) => Some(bar),
                Err(error) => {
                    tracing::warn!("could not build the menu bar: {error}");
                    return;
                }
            }
        };
        #[cfg(target_os = "macos")]
        match (&bar, &self.platform.menu_bar) {
            (Some(bar), _) => bar.show(),
            (None, Some(old)) => old.hide(),
            (None, None) => {}
        }
        #[cfg(windows)]
        {
            for (_, entry) in self.windows.iter() {
                let Some(state) = entry.open() else { continue };
                if let Some(old) = &self.platform.menu_bar {
                    old.detach(&state.window);
                }
                if let Some(bar) = &bar {
                    bar.attach(&state.window);
                }
            }
            native_menu::set_accelerators(bar.as_ref().map_or(0, NativeMenuBar::haccel));
        }
        self.platform.menu_bar = bar;
    }
}

#[cfg(all(test, feature = "ui"))]
mod tests {
    use quark_ui::element::Binding;

    use super::*;
    use crate::platform::menu::MenuRole;

    /// The key an edit role types must be the shortcut its menu item shows,
    /// or text fields would see a different command than the menu names.
    #[test]
    fn edit_roles_type_the_shortcut_they_show() {
        use MenuRole::*;
        for role in [Undo, Redo, Cut, Copy, Paste, SelectAll] {
            let (key, shift) = role.edit_key().unwrap();
            let typed: Binding = edit_chord(key, shift)
                .binding_string()
                .unwrap()
                .parse()
                .unwrap();
            let shown: Binding = role.shortcut().unwrap().parse().unwrap();
            assert!(shown.matches(&typed), "{role:?} types {typed}");
        }
    }
}
