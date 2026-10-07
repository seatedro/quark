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
    /// URLs or paths to open: arguments another launch of the app forwarded
    /// through [`crate::platform::single_instance`] before exiting, or, on
    /// macOS, URLs of the app's schemes that the system delivered as an
    /// Apple Event (see [`crate::platform::deep_link`]).
    OpenUrls(Vec<String>),
    /// An item of the menus set with [`EventContext::set_menus`] was picked,
    /// from the native menu bar or with its accelerator; carries its id.
    Menu(String),
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
    /// its action buttons. macOS reports these only for bundled apps; see
    /// [`crate::platform::notification`].
    #[cfg(feature = "notifications")]
    NotificationAction { id: u64, action: String },
    /// A notification was closed without an action. Windows reports
    /// expiry as well as dismissal.
    #[cfg(feature = "notifications")]
    NotificationDismissed { id: u64 },
    /// The tray icon was clicked with the primary button.
    #[cfg(feature = "tray")]
    TrayClicked,
    /// A tray menu item was picked; carries its id.
    #[cfg(feature = "tray")]
    TrayMenu(String),
}

/// What other threads and native callbacks hand the runner.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Posted {
    App(AppEvent),
    /// A standard menu item the runner carries out itself.
    #[cfg(feature = "ui")]
    Role(crate::platform::menu::MenuRole),
}

/// Posts [`AppEvent`]s to the main thread from any thread.
#[derive(Debug, Clone)]
pub(crate) struct EventSink {
    sender: Sender<Posted>,
    waker: Waker,
}

impl EventSink {
    pub(super) fn new(waker: Waker) -> (Self, Receiver<Posted>) {
        let (sender, receiver) = mpsc::channel();
        (Self { sender, waker }, receiver)
    }

    /// Returns false once the runner is gone.
    pub(crate) fn send(&self, event: AppEvent) -> bool {
        self.post(Posted::App(event))
    }

    pub(crate) fn post(&self, posted: Posted) -> bool {
        if self.sender.send(posted).is_err() {
            return false;
        }
        self.waker.wake();
        true
    }
}
