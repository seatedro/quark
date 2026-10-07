//! Keyboard shortcuts that fire while another app has focus, such as a
//! "show quick entry" key. Off by default (`global-shortcut` feature), and
//! each shortcut is registered by the app, ideally from a user setting:
//! a global shortcut steals that key combination from every other app.
//!
//! Shortcuts use the keymap syntax (`mod+shift+space`, where `mod` is Cmd on
//! macOS and Ctrl elsewhere). Presses arrive on a background thread, which
//! queues them and wakes the event loop; drain them in [`crate::App::wake`].
//!
//! ```no_run
//! # fn demo(cx: &mut quark_app::EventContext) -> Result<(), Box<dyn std::error::Error>> {
//! use quark_app::platform::global_shortcut::GlobalShortcuts;
//!
//! // In `App::init`, on the main thread:
//! let mut shortcuts = GlobalShortcuts::new(cx.waker().clone())?;
//! shortcuts.register("quick-entry", "mod+shift+space")?;
//! // In `App::wake`:
//! for name in shortcuts.drain() {
//!     if name == "quick-entry" { /* show the window */ }
//! }
//! # Ok(()) }
//! ```
//!
//! # Platform notes
//!
//! - **macOS** uses Carbon `RegisterEventHotKey`, which needs no
//!   accessibility permission. Create [`GlobalShortcuts`] on the main
//!   thread.
//! - **Windows** uses `RegisterHotKey`. A combination another app already
//!   holds fails to register.
//! - **Linux** grabs keys on the X11 root window. Under Wayland a client
//!   cannot grab keys: X11 grabs through XWayland fire only while an
//!   XWayland window has focus. [`GlobalShortcuts::new`] therefore returns
//!   [`GlobalShortcutError::Wayland`] in a Wayland session. The Wayland
//!   route is the `org.freedesktop.portal.GlobalShortcuts` portal (KDE
//!   Plasma 5.27+, GNOME 48+), where the compositor asks the user to
//!   confirm each binding; quark does not implement it yet.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use global_hotkey::hotkey::{Code, HotKey, Modifiers};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
use quark_ui::element::Binding;

use super::menu::Accelerator;
use crate::Waker;

#[derive(Debug, thiserror::Error)]
pub enum GlobalShortcutError {
    #[error("global shortcuts are unavailable in a Wayland session")]
    Wayland,
    #[error("{0:?} is not a shortcut a global key can use")]
    InvalidShortcut(String),
    #[error("a global shortcut named {0:?} is already registered")]
    DuplicateName(String),
    #[error(transparent)]
    Platform(#[from] global_hotkey::Error),
}

#[derive(Default)]
struct Shared {
    names: HashMap<u32, String>,
    pressed: Vec<String>,
}

/// Registered global shortcuts. Dropping it unregisters them.
pub struct GlobalShortcuts {
    manager: GlobalHotKeyManager,
    hotkeys: HashMap<String, HotKey>,
    shared: Arc<Mutex<Shared>>,
}

impl GlobalShortcuts {
    /// Start listening. `waker` wakes the event loop when a shortcut fires.
    /// Only one `GlobalShortcuts` should exist per process: the platform
    /// layer has one event handler.
    pub fn new(waker: Waker) -> Result<Self, GlobalShortcutError> {
        if cfg!(target_os = "linux")
            && is_wayland_session(
                std::env::var("XDG_SESSION_TYPE").ok().as_deref(),
                std::env::var_os("WAYLAND_DISPLAY").is_some(),
            )
        {
            return Err(GlobalShortcutError::Wayland);
        }
        let manager = GlobalHotKeyManager::new()?;
        let shared = Arc::new(Mutex::new(Shared::default()));
        let handler_shared = shared.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            if event.state != HotKeyState::Pressed {
                return;
            }
            let mut shared = handler_shared.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(name) = shared.names.get(&event.id).cloned() {
                shared.pressed.push(name);
                drop(shared);
                waker.wake();
            }
        }));
        Ok(Self {
            manager,
            hotkeys: HashMap::new(),
            shared,
        })
    }

    /// Register `shortcut` under `name`, the string [`Self::drain`] returns.
    pub fn register(&mut self, name: &str, shortcut: &str) -> Result<(), GlobalShortcutError> {
        if self.hotkeys.contains_key(name) {
            return Err(GlobalShortcutError::DuplicateName(name.to_owned()));
        }
        let hotkey = hotkey(shortcut, cfg!(target_os = "macos"))
            .ok_or_else(|| GlobalShortcutError::InvalidShortcut(shortcut.to_owned()))?;
        self.manager.register(hotkey)?;
        self.lock().names.insert(hotkey.id(), name.to_owned());
        self.hotkeys.insert(name.to_owned(), hotkey);
        Ok(())
    }

    pub fn unregister(&mut self, name: &str) -> Result<(), GlobalShortcutError> {
        if let Some(hotkey) = self.hotkeys.remove(name) {
            self.lock().names.remove(&hotkey.id());
            self.manager.unregister(hotkey)?;
        }
        Ok(())
    }

    /// Names of the shortcuts pressed since the last call, in order.
    pub fn drain(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().pressed)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl Drop for GlobalShortcuts {
    fn drop(&mut self) {
        let hotkeys: Vec<HotKey> = self.hotkeys.values().copied().collect();
        let _ = self.manager.unregister_all(&hotkeys);
        GlobalHotKeyEvent::set_event_handler(None::<fn(GlobalHotKeyEvent)>);
    }
}

/// The platform hot key for a keymap shortcut, through the same mapping
/// native menus use.
fn hotkey(shortcut: &str, macos: bool) -> Option<HotKey> {
    let binding = Binding::from_str(shortcut).ok()?;
    let accelerator = Accelerator::from_binding(&binding, macos)?;
    let mut mods = Modifiers::empty();
    for (held, flag) in [
        (accelerator.command, Modifiers::META),
        (accelerator.control, Modifiers::CONTROL),
        (accelerator.alt, Modifiers::ALT),
        (accelerator.shift, Modifiers::SHIFT),
    ] {
        if held {
            mods |= flag;
        }
    }
    let code = Code::from_str(accelerator.code).ok()?;
    Some(HotKey::new(Some(mods), code))
}

/// `XDG_SESSION_TYPE` decides when set; otherwise a `WAYLAND_DISPLAY`
/// means Wayland.
fn is_wayland_session(session_type: Option<&str>, wayland_display: bool) -> bool {
    match session_type {
        Some("wayland") => true,
        Some("x11") => false,
        _ => wayland_display,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_keymap_shortcuts_to_hot_keys() {
        let ctrl_shift_space =
            HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space);
        let meta_shift_space = HotKey::new(Some(Modifiers::META | Modifiers::SHIFT), Code::Space);
        for (shortcut, macos, expected) in [
            ("mod+shift+space", false, Some(ctrl_shift_space)),
            ("mod+shift+space", true, Some(meta_shift_space)),
            (
                "alt+f5",
                false,
                Some(HotKey::new(Some(Modifiers::ALT), Code::F5)),
            ),
            // Cmd is the Windows key off macOS, which hot keys may not use.
            ("cmd+k", false, None),
            ("mod+", false, None),
        ] {
            assert_eq!(
                hotkey(shortcut, macos),
                expected,
                "{shortcut} macos={macos}"
            );
        }
    }

    #[test]
    fn detects_wayland_sessions() {
        for (session, display, expected) in [
            (Some("wayland"), false, true),
            (Some("x11"), true, false),
            (Some("tty"), true, true),
            (None, false, false),
        ] {
            assert_eq!(
                is_wayland_session(session, display),
                expected,
                "{session:?} {display}"
            );
        }
    }
}
