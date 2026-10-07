//! A system tray icon with a menu, through tray-icon and muda. Clicks arrive
//! as [`AppEvent::TrayClicked`] and menu picks as [`AppEvent::TrayMenu`].
//!
//! Linux uses the StatusNotifierItem D-Bus protocol (KDE, and GNOME with the
//! AppIndicator extension); desktops without a StatusNotifierWatcher show
//! nothing. macOS and Windows use their native trays.
//!
//! Tray apps usually call
//! [`EventContext::set_exit_when_last_window_closes`]`(false)` so closing the
//! window leaves them running.
//!
//! [`AppEvent::TrayClicked`]: crate::AppEvent::TrayClicked
//! [`AppEvent::TrayMenu`]: crate::AppEvent::TrayMenu
//! [`EventContext::set_exit_when_last_window_closes`]: crate::EventContext::set_exit_when_last_window_closes

use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

use crate::runner::{AppEvent, EventSink};

/// The underlying crates, for what [`TrayOptions`] does not cover.
pub use tray_icon as native;

#[derive(Debug, Clone, Default)]
pub struct TrayOptions {
    pub tooltip: String,
    /// Straight RGBA8 pixels, `width * height * 4` bytes.
    pub icon_rgba: Vec<u8>,
    pub icon_width: u32,
    pub icon_height: u32,
    pub menu: Vec<TrayMenuItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrayMenuItem {
    /// `id` comes back in [`crate::AppEvent::TrayMenu`].
    Item {
        id: String,
        label: String,
        enabled: bool,
    },
    Separator,
}

#[derive(Debug, thiserror::Error)]
pub enum TrayError {
    #[error("invalid tray icon: {0}")]
    Icon(#[from] tray_icon::BadIcon),
    #[error("could not build the tray menu: {0}")]
    Menu(#[from] tray_icon::menu::Error),
    #[error("could not create the tray icon: {0}")]
    Tray(#[from] tray_icon::Error),
}

pub(crate) fn create(options: TrayOptions, events: &EventSink) -> Result<TrayIcon, TrayError> {
    let icon =
        tray_icon::Icon::from_rgba(options.icon_rgba, options.icon_width, options.icon_height)?;
    let menu = Menu::new();
    for item in &options.menu {
        match item {
            TrayMenuItem::Item { id, label, enabled } => {
                menu.append(&MenuItem::with_id(id.as_str(), label, *enabled, None))?
            }
            TrayMenuItem::Separator => menu.append(&PredefinedMenuItem::separator())?,
        }
    }
    install_handlers(events.clone());
    let tray = TrayIconBuilder::new()
        .with_tooltip(&options.tooltip)
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .build()?;
    Ok(tray)
}

/// The crates take one global handler each; reinstalling replaces it.
fn install_handlers(events: EventSink) {
    let tray_events = events.clone();
    TrayIconEvent::set_event_handler(Some(move |event| {
        if let TrayIconEvent::Click {
            button: MouseButton::Left,
            button_state: MouseButtonState::Up,
            ..
        } = event
        {
            tray_events.send(AppEvent::TrayClicked);
        }
    }));
    MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
        events.send(AppEvent::TrayMenu(event.id.0));
    }));
}
