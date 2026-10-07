//! The desktop's light or dark preference on Linux, from the XDG desktop
//! portal's `org.freedesktop.appearance color-scheme` setting. winit 0.30
//! reports no theme on X11 and only its own decoration theme on Wayland, so
//! the runner reads the portal instead. Desktops without the portal (bare
//! window managers) report nothing, and apps keep their default.

use futures_lite::StreamExt;
use winit::window::Theme;
use zbus::zvariant::OwnedValue;

use crate::runner::{AppEvent, EventSink};

const NAMESPACE: &str = "org.freedesktop.appearance";
const KEY: &str = "color-scheme";

/// Report the current preference, then every change, from a thread.
pub(crate) fn watch(events: EventSink) {
    let spawned = std::thread::Builder::new()
        .name("quark-theme".to_owned())
        .spawn(move || {
            if let Err(error) = zbus::block_on(watch_portal(&events)) {
                tracing::debug!("no desktop theme from the settings portal: {error}");
            }
        });
    if let Err(error) = spawned {
        tracing::warn!("could not start the theme watcher: {error}");
    }
}

async fn watch_portal(events: &EventSink) -> zbus::Result<()> {
    let connection = zbus::Connection::session().await?;
    let settings = zbus::Proxy::new(
        &connection,
        "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.Settings",
    )
    .await?;
    // Subscribe before reading so a change in between is not lost.
    let mut changes = settings.receive_signal("SettingChanged").await?;

    let current: zbus::Result<OwnedValue> = settings.call("ReadOne", &(NAMESPACE, KEY)).await;
    let current = match current {
        Ok(value) => value,
        // Portals before version 2 only have `Read`, which wraps the value
        // in one more variant.
        Err(_) => {
            let wrapped: OwnedValue = settings.call("Read", &(NAMESPACE, KEY)).await?;
            unwrap_variant(wrapped)
        }
    };
    if let Some(theme) = color_scheme_theme(&current)
        && !events.send(AppEvent::ThemeChanged(theme))
    {
        return Ok(());
    }

    while let Some(message) = changes.next().await {
        let Ok((namespace, key, value)) =
            message.body().deserialize::<(String, String, OwnedValue)>()
        else {
            continue;
        };
        if namespace != NAMESPACE || key != KEY {
            continue;
        }
        if let Some(theme) = color_scheme_theme(&value)
            && !events.send(AppEvent::ThemeChanged(theme))
        {
            return Ok(());
        }
    }
    Ok(())
}

fn unwrap_variant(value: OwnedValue) -> OwnedValue {
    if let zbus::zvariant::Value::Value(inner) = &*value
        && let Ok(inner) = inner.try_to_owned()
    {
        return inner;
    }
    value
}

/// 0 is "no preference", which desktops render light.
fn color_scheme_theme(value: &OwnedValue) -> Option<Theme> {
    match u32::try_from(value).ok()? {
        1 => Some(Theme::Dark),
        0 | 2 => Some(Theme::Light),
        _ => None,
    }
}
