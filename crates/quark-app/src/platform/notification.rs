//! Desktop notifications. Each one is shown from its own thread because the
//! platform calls block (on Linux, until the notification is closed).
//!
//! Clicks on the body or an action button arrive as
//! [`AppEvent::NotificationAction`], and closes without an action as
//! [`AppEvent::NotificationDismissed`]:
//!
//! - **Linux and the BSDs**: through the freedesktop notification server.
//! - **Windows**: through WinRT toast activation, while the app runs.
//! - **macOS**: through `UNUserNotificationCenter`, which works only in an
//!   app bundle with a bundle identifier. The first notification asks the
//!   user for permission. A notification with the same id as an earlier one
//!   replaces it. Unbundled binaries (`cargo run`) fall back to the older
//!   notification API, which shows the notification but reports nothing.
//!
//! [`AppEvent::NotificationAction`]: crate::AppEvent::NotificationAction
//! [`AppEvent::NotificationDismissed`]: crate::AppEvent::NotificationDismissed

#[cfg(any(target_os = "macos", test))]
use crate::runner::AppEvent;
use crate::runner::EventSink;

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
    /// Reported back in [`crate::AppEvent::NotificationAction`].
    pub id: String,
    pub label: String,
}

pub(crate) fn show(notification: Notification, events: EventSink) {
    #[cfg(target_os = "macos")]
    if macos::show(&notification) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("quark-notification".to_owned())
        .spawn(move || show_blocking(notification, events));
    if let Err(error) = spawned {
        tracing::warn!("could not start the notification thread: {error}");
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
fn show_blocking(notification: Notification, events: EventSink) {
    use crate::runner::AppEvent;

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

#[cfg(windows)]
fn show_blocking(notification: Notification, events: EventSink) {
    use notify_rust::NotificationResponse;

    use crate::runner::AppEvent;

    let handle = match native(&notification).show() {
        Ok(handle) => handle,
        Err(error) => {
            tracing::warn!("could not show a notification: {error}");
            return;
        }
    };
    let id = notification.id;
    let result = handle.wait_for_response(|response: &NotificationResponse| {
        let event = match response {
            NotificationResponse::Default => AppEvent::NotificationAction {
                id,
                action: DEFAULT_ACTION.to_owned(),
            },
            NotificationResponse::Action(action) | NotificationResponse::Reply(action) => {
                AppEvent::NotificationAction {
                    id,
                    action: action.clone(),
                }
            }
            NotificationResponse::Closed(_) => AppEvent::NotificationDismissed { id },
        };
        events.send(event);
    });
    if let Err(error) = result {
        tracing::debug!("no response to a notification: {error}");
    }
}

/// macOS without a bundle: shown through the older API, which reports
/// nothing back.
#[cfg(target_os = "macos")]
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

/// The macOS request identifier for the app's notification `id`.
#[cfg(any(target_os = "macos", test))]
fn request_identifier(id: u64) -> String {
    format!("quark.notification.{id}")
}

/// The event for a `UNNotificationResponse`: `request` is the notification's
/// request identifier and `action` the response's action identifier. `None`
/// for notifications this module did not post.
#[cfg(any(target_os = "macos", test))]
fn response_event(request: &str, action: &str) -> Option<AppEvent> {
    // The values of UNNotificationDefaultActionIdentifier and
    // UNNotificationDismissActionIdentifier.
    const DEFAULT: &str = "com.apple.UNNotificationDefaultActionIdentifier";
    const DISMISS: &str = "com.apple.UNNotificationDismissActionIdentifier";
    let id = request.strip_prefix("quark.notification.")?.parse().ok()?;
    Some(match action {
        DISMISS => AppEvent::NotificationDismissed { id },
        DEFAULT => AppEvent::NotificationAction {
            id,
            action: DEFAULT_ACTION.to_owned(),
        },
        action => AppEvent::NotificationAction {
            id,
            action: action.to_owned(),
        },
    })
}

#[cfg(target_os = "macos")]
pub(crate) use macos::install;

#[cfg(target_os = "macos")]
mod macos {
    use std::cell::RefCell;

    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, NSObject, NSObjectProtocol, ProtocolObject};
    use objc2::{AllocAnyThread, DefinedClass, define_class, msg_send};
    use objc2_foundation::{NSArray, NSBundle, NSError, NSSet, NSString};
    use objc2_user_notifications::{
        UNAuthorizationOptions, UNMutableNotificationContent, UNNotification, UNNotificationAction,
        UNNotificationActionOptions, UNNotificationCategory, UNNotificationCategoryOptions,
        UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
        UNUserNotificationCenter, UNUserNotificationCenterDelegate,
    };

    use super::{Notification, request_identifier, response_event};
    use crate::runner::EventSink;

    define_class!(
        #[unsafe(super(NSObject))]
        #[name = "QuarkNotificationDelegate"]
        #[ivars = EventSink]
        struct Delegate;

        unsafe impl NSObjectProtocol for Delegate {}

        unsafe impl UNUserNotificationCenterDelegate for Delegate {
            /// Show notifications while the app is frontmost too.
            #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
            fn will_present(
                &self,
                _center: &UNUserNotificationCenter,
                _notification: &UNNotification,
                completion: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
            ) {
                completion.call((UNNotificationPresentationOptions::Banner
                    | UNNotificationPresentationOptions::List
                    | UNNotificationPresentationOptions::Sound,));
            }

            #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
            fn did_receive(
                &self,
                _center: &UNUserNotificationCenter,
                response: &UNNotificationResponse,
                completion: &block2::DynBlock<dyn Fn()>,
            ) {
                let request = response.notification().request().identifier().to_string();
                let action = response.actionIdentifier().to_string();
                if let Some(event) = response_event(&request, &action) {
                    self.ivars().send(event);
                }
                completion.call(());
            }
        }
    );

    struct Center {
        center: Retained<UNUserNotificationCenter>,
        /// Held here because the center keeps its delegate weakly.
        _delegate: Retained<Delegate>,
        /// One category per distinct set of actions, all registered at once
        /// because registering replaces the previous set.
        categories: Vec<(String, Retained<UNNotificationCategory>)>,
    }

    thread_local! {
        /// Set on the main thread by `install` in bundled apps only.
        static CENTER: RefCell<Option<Center>> = const { RefCell::new(None) };
    }

    /// Become the notification center's delegate, before the app finishes
    /// launching so a click that launched it is reported. Does nothing
    /// outside an app bundle, where the center raises an exception.
    pub(crate) fn install(events: &EventSink) {
        if !bundled() {
            return;
        }
        let center = UNUserNotificationCenter::currentNotificationCenter();
        let delegate = Delegate::alloc().set_ivars(events.clone());
        let delegate: Retained<Delegate> = unsafe { msg_send![super(delegate), init] };
        center.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        CENTER.with_borrow_mut(|slot| {
            *slot = Some(Center {
                center,
                _delegate: delegate,
                categories: Vec::new(),
            });
        });
    }

    fn bundled() -> bool {
        let bundle = NSBundle::mainBundle();
        bundle.bundleIdentifier().is_some() && bundle.bundlePath().to_string().ends_with(".app")
    }

    /// Post through the notification center. False when it is unavailable,
    /// so the caller falls back to the older API.
    pub(super) fn show(notification: &Notification) -> bool {
        CENTER.with_borrow_mut(|center| {
            let Some(center) = center else {
                return false;
            };
            post(center, notification);
            true
        })
    }

    fn post(center: &mut Center, notification: &Notification) {
        let content = UNMutableNotificationContent::new();
        content.setTitle(&NSString::from_str(&notification.title));
        content.setBody(&NSString::from_str(&notification.body));
        content.setCategoryIdentifier(&NSString::from_str(&category(center, notification)));
        let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
            &NSString::from_str(&request_identifier(notification.id)),
            &content,
            None,
        );

        // Asking again after the user has answered returns at once without
        // a prompt, so every post asks and posts from the answer.
        let poster = center.center.clone();
        let authorized = RcBlock::new(move |granted: Bool, error: *mut NSError| {
            if !granted.as_bool() {
                // SAFETY: the framework passes a valid error or null.
                let error = unsafe { error.as_ref() }.map(|e| e.localizedDescription());
                tracing::warn!("notifications are not allowed: {error:?}");
                return;
            }
            let posted = RcBlock::new(|error: *mut NSError| {
                // SAFETY: as above.
                if let Some(error) = unsafe { error.as_ref() } {
                    tracing::warn!(
                        "could not post a notification: {}",
                        error.localizedDescription()
                    );
                }
            });
            poster.addNotificationRequest_withCompletionHandler(&request, Some(&posted));
        });
        center
            .center
            .requestAuthorizationWithOptions_completionHandler(
                UNAuthorizationOptions::Alert | UNAuthorizationOptions::Sound,
                &authorized,
            );
    }

    /// The category carrying `notification`'s action buttons, registered on
    /// first use. Every category asks to hear about dismissals.
    fn category(center: &mut Center, notification: &Notification) -> String {
        let id: String = std::iter::once("quark.actions".to_owned())
            .chain(
                notification
                    .actions
                    .iter()
                    .map(|action| format!("{}={}", action.id, action.label)),
            )
            .collect::<Vec<_>>()
            .join("\u{1f}");
        if center.categories.iter().any(|(known, _)| *known == id) {
            return id;
        }
        let actions: Vec<Retained<UNNotificationAction>> = notification
            .actions
            .iter()
            .map(|action| {
                UNNotificationAction::actionWithIdentifier_title_options(
                    &NSString::from_str(&action.id),
                    &NSString::from_str(&action.label),
                    UNNotificationActionOptions::Foreground,
                )
            })
            .collect();
        let category =
            UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
                &NSString::from_str(&id),
                &NSArray::from_retained_slice(&actions),
                &NSArray::new(),
                UNNotificationCategoryOptions::CustomDismissAction,
            );
        center.categories.push((id.clone(), category));
        let all: Vec<Retained<UNNotificationCategory>> = center
            .categories
            .iter()
            .map(|(_, category)| category.clone())
            .collect();
        center
            .center
            .setNotificationCategories(&NSSet::from_retained_slice(&all));
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_responses_map_to_notification_events() {
        let request = request_identifier(42);
        let cases = [
            (
                "com.apple.UNNotificationDefaultActionIdentifier",
                Some(AppEvent::NotificationAction {
                    id: 42,
                    action: DEFAULT_ACTION.to_owned(),
                }),
            ),
            (
                "com.apple.UNNotificationDismissActionIdentifier",
                Some(AppEvent::NotificationDismissed { id: 42 }),
            ),
            (
                "again",
                Some(AppEvent::NotificationAction {
                    id: 42,
                    action: "again".to_owned(),
                }),
            ),
        ];
        for (action, expected) in cases {
            assert_eq!(response_event(&request, action), expected, "{action}");
        }
        // Another component's notification in the same process.
        assert_eq!(response_event("com.example.reminder", "again"), None);
    }
}
