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
    /// Another launch of the app forwarded its command line arguments (deep
    /// link URLs, file paths) through
    /// [`crate::platform::single_instance`], then exited.
    OpenUrls(Vec<String>),
    /// The desktop's light or dark preference, once when it is first known
    /// and again whenever it changes. See [`EventContext::theme`].
    ThemeChanged(Theme),
    /// A dialog from [`EventContext::file_dialog`] closed. `paths` is empty
    /// when the user cancelled.
    #[cfg(feature = "dialogs")]
    FileDialogClosed {
        id: crate::platform::dialog::DialogId,
        paths: Vec<std::path::PathBuf>,
    },
    /// The user clicked a notification sent with [`EventContext::notify`]:
    /// its body ([`crate::platform::notification::DEFAULT_ACTION`]) or one of
    /// its action buttons. Linux and the BSDs only.
    #[cfg(feature = "notifications")]
    NotificationAction { id: u64, action: String },
    /// A notification was closed without an action. Linux and the BSDs only.
    #[cfg(feature = "notifications")]
    NotificationDismissed { id: u64 },
    /// The tray icon was clicked with the primary button.
    #[cfg(feature = "tray")]
    TrayClicked,
    /// A tray menu item was picked; carries its id.
    #[cfg(feature = "tray")]
    TrayMenu(String),
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
