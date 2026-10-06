//! Desktop notifications. Each one is shown from its own thread because the
//! platform calls block (on Linux, until the notification is closed).
//!
//! Click and action callbacks arrive as [`AppEvent::NotificationAction`] and
//! [`AppEvent::NotificationDismissed`] on Linux and the BSDs, where the
//! freedesktop notification spec reports them. macOS and Windows show the
//! notification but report nothing back.

use crate::runner::{AppEvent, EventSink};

/// The action id reported when the user clicks the notification body.
pub const DEFAULT_ACTION: &str = "default";

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notification {
    pub title: String,
    pub body: String,
    /// Chosen by the app and echoed back in the events for this notification.
    pub id: u64,
    /// Buttons, where the notification server supports them.
    pub actions: Vec<NotificationAction>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NotificationAction {
    /// Reported back in [`AppEvent::NotificationAction`].
    pub id: String,
    pub label: String,
}

pub(crate) fn show(notification: Notification, events: EventSink) {
    let spawned = std::thread::Builder::new()
        .name("quark-notification".to_owned())
        .spawn(move || show_blocking(notification, events));
    if let Err(error) = spawned {
        tracing::warn!("could not start the notification thread: {error}");
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show_blocking(notification: Notification, events: EventSink) {
    let mut native = native(&notification);
    // Servers invoke the "default" action when the body is clicked; most do
    // not draw it as a button.
    native.action(DEFAULT_ACTION, "Open");
    let handle = match native.show() {
        Ok(handle) => handle,
        Err(error) => {
            tracing::warn!("could not show a notification: {error}");
            return;
        }
    };
    let id = notification.id;
    handle.wait_for_action(|action| {
        let event = match action {
            "__closed" => AppEvent::NotificationDismissed { id },
            action => AppEvent::NotificationAction {
                id,
                action: action.to_owned(),
            },
        };
        events.send(event);
    });
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn show_blocking(notification: Notification, _events: EventSink) {
    if let Err(error) = native(&notification).show() {
        tracing::warn!("could not show a notification: {error}");
    }
}

fn native(notification: &Notification) -> notify_rust::Notification {
    let mut native = notify_rust::Notification::new();
    native.summary(&notification.title).body(&notification.body);
    for action in &notification.actions {
        native.action(&action.id, &action.label);
    }
    native
}
