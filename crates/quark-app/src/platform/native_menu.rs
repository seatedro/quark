//! The muda side of [`super::menu`] on macOS and Windows, and the one muda
//! menu event handler that the menu bar and the tray share.

use crate::runner::{AppEvent, EventSink, Posted};

/// Prefixes on native item ids, so menu bar picks are told apart from tray
/// menu picks (whose ids are the app's own) in muda's one global handler.
#[cfg(feature = "ui")]
const ACTION_PREFIX: &str = "quark.menu.action:";
#[cfg(feature = "ui")]
const ROLE_PREFIX: &str = "quark.menu.role:";

/// Route every muda menu event to `events`. muda takes one global handler,
/// so the menu bar and the tray both install this one.
pub(crate) fn install_event_handler(events: EventSink) {
    muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
        if let Some(posted) = route(event.id.0) {
            events.post(posted);
        }
    }));
}

fn route(id: String) -> Option<Posted> {
    #[cfg(feature = "ui")]
    {
        if let Some(action) = id.strip_prefix(ACTION_PREFIX) {
            return Some(Posted::App(AppEvent::Menu(action.to_owned())));
        }
        if let Some(role) = id.strip_prefix(ROLE_PREFIX) {
            return super::menu::MenuRole::from_name(role).map(Posted::Role);
        }
    }
    #[cfg(feature = "tray")]
    return Some(Posted::App(AppEvent::TrayMenu(id)));
    #[cfg(not(feature = "tray"))]
    {
        let _ = id;
        None
    }
}

#[cfg(feature = "ui")]
pub(crate) use bar::*;

#[cfg(feature = "ui")]
mod bar {
    use std::str::FromStr;

    use muda::accelerator::{Accelerator as NativeAccelerator, Code, Modifiers};
    use muda::{CheckMenuItem, IsMenuItem, MenuItem as NativeItem, PredefinedMenuItem, Submenu};
    #[cfg(windows)]
    use winit::window::Window;

    use super::{ACTION_PREFIX, ROLE_PREFIX};
    use crate::platform::menu::{Accelerator, Menu, MenuItem, MenuRole};

    // The accelerator table of the menu bar on screen, for the message hook.
    #[cfg(windows)]
    thread_local! {
        static ACCELERATORS: std::cell::Cell<isize> = const { std::cell::Cell::new(0) };
    }

    #[cfg(windows)]
    pub(crate) fn set_accelerators(haccel: isize) {
        ACCELERATORS.set(haccel);
    }

    /// winit's message hook: turn a key message matching a menu accelerator
    /// into the menu command, as a Win32 message loop's
    /// `TranslateAcceleratorW` call would. True when it was consumed.
    #[cfg(windows)]
    pub(crate) fn translate_accelerator(msg: *const std::ffi::c_void) -> bool {
        use windows::Win32::UI::WindowsAndMessaging::{HACCEL, MSG, TranslateAcceleratorW};

        let haccel = ACCELERATORS.get();
        if haccel == 0 || msg.is_null() {
            return false;
        }
        let msg = msg.cast::<MSG>();
        // SAFETY: winit passes a valid MSG, and the table lives as long as
        // the menu bar that set it.
        unsafe { TranslateAcceleratorW((*msg).hwnd, HACCEL(haccel as _), msg) != 0 }
    }

    /// The menu bar currently shown, kept alive for as long as it is.
    pub(crate) struct NativeMenuBar {
        menu: muda::Menu,
    }

    impl NativeMenuBar {
        pub(crate) fn build(menus: &[Menu]) -> Result<Self, muda::Error> {
            let menu = muda::Menu::new();
            for top in menus {
                menu.append(&submenu(top)?)?;
            }
            Ok(Self { menu })
        }

        /// Show this bar in place of the current one.
        #[cfg(target_os = "macos")]
        pub(crate) fn show(&self) {
            self.menu.init_for_nsapp();
        }

        #[cfg(target_os = "macos")]
        pub(crate) fn hide(&self) {
            self.menu.remove_for_nsapp();
        }

        /// Add this bar to `window`.
        #[cfg(windows)]
        pub(crate) fn attach(&self, window: &Window) {
            if let Some(hwnd) = hwnd(window) {
                // SAFETY: the handle comes from a live winit window.
                if let Err(error) = unsafe { self.menu.init_for_hwnd(hwnd) } {
                    tracing::warn!("could not add the menu bar to a window: {error}");
                }
            }
        }

        #[cfg(windows)]
        pub(crate) fn detach(&self, window: &Window) {
            if let Some(hwnd) = hwnd(window) {
                // SAFETY: as in `attach`.
                let _ = unsafe { self.menu.remove_for_hwnd(hwnd) };
            }
        }

        /// The accelerator table for `TranslateAcceleratorW`.
        #[cfg(windows)]
        pub(crate) fn haccel(&self) -> isize {
            self.menu.haccel()
        }
    }

    #[cfg(windows)]
    fn hwnd(window: &Window) -> Option<isize> {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
            _ => None,
        }
    }

    fn submenu(menu: &Menu) -> Result<Submenu, muda::Error> {
        let submenu = Submenu::new(&menu.label, menu.enabled);
        for item in &menu.items {
            match item {
                MenuItem::Action(action) => {
                    let id = format!("{ACTION_PREFIX}{}", action.id);
                    let accelerator = action.accelerator.as_ref().and_then(|binding| {
                        native_accelerator(Accelerator::from_binding(
                            binding,
                            cfg!(target_os = "macos"),
                        )?)
                    });
                    match action.checked {
                        Some(checked) => submenu.append(&CheckMenuItem::with_id(
                            id,
                            &action.label,
                            action.enabled,
                            checked,
                            accelerator,
                        ))?,
                        None => submenu.append(&NativeItem::with_id(
                            id,
                            &action.label,
                            action.enabled,
                            accelerator,
                        ))?,
                    }
                }
                MenuItem::Submenu(menu) => submenu.append(&self::submenu(menu)?)?,
                MenuItem::Separator => submenu.append(&PredefinedMenuItem::separator())?,
                MenuItem::Role(role) => {
                    if let Some(item) = role_item(*role) {
                        submenu.append(item.as_ref())?;
                    }
                }
            }
        }
        Ok(submenu)
    }

    /// The platform's own item where it acts on the app (About, Services,
    /// Hide), else a plain item the runner carries out, so every platform
    /// sees the same Quit, window, and edit behavior.
    fn role_item(role: MenuRole) -> Option<Box<dyn IsMenuItem>> {
        if role.macos_only() && !cfg!(target_os = "macos") {
            return None;
        }
        let native: Option<Box<dyn IsMenuItem>> = match role {
            MenuRole::About => Some(Box::new(PredefinedMenuItem::about(None, None))),
            MenuRole::Services => Some(Box::new(PredefinedMenuItem::services(None))),
            MenuRole::Hide => Some(Box::new(PredefinedMenuItem::hide(None))),
            MenuRole::HideOthers => Some(Box::new(PredefinedMenuItem::hide_others(None))),
            MenuRole::ShowAll => Some(Box::new(PredefinedMenuItem::show_all(None))),
            MenuRole::BringAllToFront => {
                Some(Box::new(PredefinedMenuItem::bring_all_to_front(None)))
            }
            MenuRole::Separator => Some(Box::new(PredefinedMenuItem::separator())),
            _ => None,
        };
        if native.is_some() {
            return native;
        }
        let accelerator = role
            .shortcut()
            .and_then(|binding| binding.parse().ok())
            .and_then(|binding| Accelerator::from_binding(&binding, cfg!(target_os = "macos")))
            .and_then(native_accelerator);
        Some(Box::new(NativeItem::with_id(
            format!("{ROLE_PREFIX}{}", role.name()),
            role.label(),
            true,
            accelerator,
        )))
    }

    fn native_accelerator(accelerator: Accelerator) -> Option<NativeAccelerator> {
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
        Some(NativeAccelerator::new(mods, code))
    }
}
