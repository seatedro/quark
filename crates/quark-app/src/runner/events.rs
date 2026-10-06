use super::*;

/// Events about the app and its windows rather than input, delivered through
/// [`App::app_event`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AppEvent {
    /// A window closed, by the user or through [`EventContext::close_window`].
    /// Its handle is stale from here on.
    WindowClosed(WindowHandle),
    /// A window requested through [`EventContext::open_window`] could not be
    /// created. The handle is stale.
    WindowOpenFailed(WindowHandle),
    /// The user clicked a notification sent with [`EventContext::notify`]:
    /// its body ([`crate::platform::notification::DEFAULT_ACTION`]) or one of
    /// its action buttons. Linux and the BSDs only.
    #[cfg(feature = "notifications")]
    NotificationAction { id: u64, action: String },
    /// A notification was closed without an action. Linux and the BSDs only.
    #[cfg(feature = "notifications")]
    NotificationDismissed { id: u64 },
}

/// Posts [`AppEvent`]s to the main thread from any thread.
#[derive(Debug, Clone)]
pub(crate) struct EventSink {
    sender: Sender<AppEvent>,
    waker: Waker,
}

impl EventSink {
    pub(super) fn new(waker: Waker) -> (Self, Receiver<AppEvent>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender, waker }, receiver)
    }

    /// Returns false once the runner is gone.
    pub(crate) fn send(&self, event: AppEvent) -> bool {
        if self.sender.send(event).is_err() {
            return false;
        }
        self.waker.wake();
        true
    }
}
