use super::*;

/// Events about the app and its windows rather than input, delivered through
/// [`App::app_event`].
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AppEvent {
    /// The window's native window exists. Every window gets this once,
    /// before its first frame: windows from [`EventContext::open_window`]
    /// once they are created, the first window right after [`App::init`].
    /// The context is bound to the window.
    WindowOpened(WindowHandle),
    /// The window is closing, or never opened when `reason` is
    /// [`CloseReason::OpenFailed`]. Every handle the runner issues ends
    /// with exactly one of these, except windows requested while the app
    /// quits, which are never created. During this event a window that
    /// opened still answers [`EventContext::placement`], so its placement
    /// can be saved; afterwards its handle is stale. The context is bound
    /// to another window, if any is open.
    WindowClosed {
        window: WindowHandle,
        reason: CloseReason,
    },
    /// The window moved on the desktop: its outer position in physical
    /// desktop pixels. Never sent where windows have no readable position
    /// (see [`PlatformCapabilities::window_positions`]).
    WindowMoved {
        window: WindowHandle,
        position: (i32, i32),
    },
    /// The window's content area changed size, in logical points. The
    /// runner has already scheduled a frame at the new size.
    WindowResized {
        window: WindowHandle,
        size: (f32, f32),
    },
    /// The window moved to a display with another scale factor, or the
    /// display's scale changed. A [`Self::WindowResized`] follows when the
    /// physical size changes with it.
    WindowScaleChanged {
        window: WindowHandle,
        scale_factor: f64,
    },
    /// A token from [`EventContext::request_activation_token`], for handing
    /// to the desktop (or another process) to activate a window with.
    /// X11 and Wayland only.
    ActivationToken { window: WindowHandle, token: String },
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

/// Why a window closed, in [`AppEvent::WindowClosed`]. [`App::close_requested`]
/// is asked first for [`Self::User`] and [`Self::Quit`]; the window closes
/// only if it agrees.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CloseReason {
    /// The user closed the window: its close button, or the Close Window
    /// menu item.
    User,
    /// The app closed it with [`EventContext::close_window`], without
    /// asking [`App::close_requested`].
    Program,
    /// The app is quitting: the Quit menu item, which asks first, or
    /// [`EventContext::exit`] and other exits with windows still open, which
    /// do not ask. An app that closes some windows on [`Self::User`] (say,
    /// moving their contents back to the main window) can tell quitting
    /// apart and save them as they are instead.
    Quit,
    /// The native window could not be created. The window never opened and
    /// got no [`AppEvent::WindowOpened`].
    OpenFailed,
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
