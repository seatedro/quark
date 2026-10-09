//! Native webviews for Quark apps.
//!
//! V1 opens a modal sign-in window over a parent window: WebKitGTK on
//! Linux, WKWebView on macOS, WebView2 on Windows. Apps use it through
//! `quark-app`'s `webview` feature (`cx.window.webviews()`); this crate
//! holds the backend-independent types, the navigation policy, the script
//! wrapper, and the session state machine.
//!
//! - Every navigation is checked against an explicit, nonempty allowlist of
//!   exact `https` origins ([`Origin`]).
//! - Scripts run only when granted: in the main frame of a committed
//!   document whose origin is in the evaluation allowlist and matches the
//!   caller's [`OriginGuard`]. Any navigation in between fails them.
//! - Each view gets a fresh ephemeral data store unless the app names a
//!   persistent [`ProfileId`].

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};
use std::time::Duration;

pub use url::Url;

mod backend;
pub mod policy;
mod profile;
mod script;
mod session;

pub use policy::{BlockReason, Origin, OriginError, PopupPolicy};
pub use profile::{DataStore, ProfileError, ProfileId};
pub use script::{AsyncScript, ErrorDetail, InvalidResult, OriginGuard, ScriptValue};

/// The runner's side of the service. Not a stable API: `quark-app` drives
/// it, apps use `quark_app::platform::webview`.
#[doc(hidden)]
pub mod service {
    pub use crate::backend::{ACTIVE_SERVICE_INTERVAL, NativeParent, SERVICE_ITERATIONS};
    pub use crate::session::{Service, Serviced};
}

/// Identifies one webview. Stays valid until its [`WebViewEvent::Closed`];
/// afterwards nothing matches it, even a view that reuses its slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WebViewHandle {
    index: u32,
    generation: u32,
}

/// One top-level document. A new one starts with every committed
/// cross-document navigation, including back to the same URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct DocumentId(u64);

/// One attempted top-level navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NavigationId(u64);

/// One script evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EvaluationId(u64);

/// A URL a page navigated to. Dereferences to [`Url`]; `Debug` shows only
/// its origin, since query strings can carry codes and tokens.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct PageUrl(Url);

impl PageUrl {
    pub fn into_url(self) -> Url {
        self.0
    }
}

impl std::ops::Deref for PageUrl {
    type Target = Url;

    fn deref(&self) -> &Url {
        &self.0
    }
}

impl fmt::Debug for PageUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "PageUrl({}/…)", self.0.origin().ascii_serialization())
    }
}

/// Light or dark rendering of native chrome and the engine's
/// `prefers-color-scheme`. Pages still choose their own CSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[non_exhaustive]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// Bounds on script evaluation for one webview. Defaults are the framework
/// maximums; setters can only lower them (and never to zero).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EvaluationLimits {
    pub(crate) timeout: Duration,
    pub(crate) max_result_bytes: usize,
    pub(crate) max_depth: u32,
    pub(crate) max_in_flight: u32,
}

impl Default for EvaluationLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(10),
            max_result_bytes: 1 << 20,
            max_depth: 64,
            max_in_flight: 8,
        }
    }
}

impl EvaluationLimits {
    /// Fail an evaluation with [`EvalError::Timeout`] this long after it
    /// was submitted. At most 10 seconds.
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout.clamp(Duration::from_millis(1), Self::default().timeout);
        self
    }

    /// The largest encoded JSON result, in bytes. At most 1 MiB.
    pub fn max_result_bytes(mut self, bytes: usize) -> Self {
        self.max_result_bytes = bytes.clamp(1, Self::default().max_result_bytes);
        self
    }

    /// The deepest nesting of arrays and objects, the outermost value
    /// counting as 1. At most 64.
    pub fn max_depth(mut self, depth: u32) -> Self {
        self.max_depth = depth.clamp(1, Self::default().max_depth);
        self
    }

    /// Evaluations one view runs at once; more fail with
    /// [`EvalError::TooManyRequests`]. At most 8.
    pub fn max_in_flight(mut self, count: u32) -> Self {
        self.max_in_flight = count.clamp(1, Self::default().max_in_flight);
        self
    }
}

/// What a webview may load and run, independent of how it is presented.
#[derive(Debug, Clone, PartialEq)]
pub struct WebViewOptions {
    pub(crate) navigation: Vec<Origin>,
    pub(crate) evaluation: Vec<Origin>,
    pub(crate) data_store: DataStore,
    pub(crate) user_agent: Option<String>,
    pub(crate) popups: PopupPolicy,
    pub(crate) appearance: Appearance,
    pub(crate) devtools: bool,
    pub(crate) limits: EvaluationLimits,
}

impl WebViewOptions {
    /// Allow top-level and frame documents only from `navigation_origins`,
    /// which must not be empty. Include every identity provider and login
    /// frame origin the flow visits.
    pub fn new(navigation_origins: impl IntoIterator<Item = Origin>) -> Self {
        Self {
            navigation: navigation_origins.into_iter().collect(),
            evaluation: Vec::new(),
            data_store: DataStore::default(),
            user_agent: None,
            popups: PopupPolicy::default(),
            appearance: Appearance::default(),
            devtools: false,
            limits: EvaluationLimits::default(),
        }
    }

    /// Origins scripts may run in; empty by default, so no script runs.
    /// Each must also be a navigation origin.
    pub fn evaluation_origins(mut self, origins: impl IntoIterator<Item = Origin>) -> Self {
        self.evaluation = origins.into_iter().collect();
        self
    }

    pub fn data_store(mut self, store: DataStore) -> Self {
        self.data_store = store;
        self
    }

    /// Replace the engine's user agent string.
    pub fn user_agent(mut self, user_agent: impl Into<String>) -> Self {
        self.user_agent = Some(user_agent.into());
        self
    }

    pub fn popups(mut self, popups: PopupPolicy) -> Self {
        self.popups = popups;
        self
    }

    pub fn appearance(mut self, appearance: Appearance) -> Self {
        self.appearance = appearance;
        self
    }

    /// The engine's developer tools, for debug builds with synthetic
    /// credentials only. Opening fails with
    /// [`OpenError::DevtoolsUnavailable`] in release builds.
    pub fn devtools(mut self, devtools: bool) -> Self {
        self.devtools = devtools;
        self
    }

    pub fn evaluation_limits(mut self, limits: EvaluationLimits) -> Self {
        self.limits = limits;
        self
    }
}

/// A webview in its own modal window over a parent window.
#[derive(Debug, Clone, PartialEq)]
pub struct WebWindowOptions {
    pub(crate) view: WebViewOptions,
    pub(crate) title: String,
    pub(crate) size: (f32, f32),
    pub(crate) min_size: (f32, f32),
    pub(crate) require_native_parent: bool,
}

impl WebWindowOptions {
    /// A resizable, decorated window titled "Sign in", 520 × 720 points
    /// and at least 360 × 480.
    pub fn new(view: WebViewOptions) -> Self {
        Self {
            view,
            title: "Sign in".to_owned(),
            size: (520.0, 720.0),
            min_size: (360.0, 480.0),
            require_native_parent: false,
        }
    }

    /// The window's title, which the page cannot change.
    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = title.into();
        self
    }

    /// Initial content size in logical points.
    pub fn size(mut self, width: f32, height: f32) -> Self {
        self.size = (width, height);
        self
    }

    /// Minimum content size in logical points.
    pub fn min_size(mut self, width: f32, height: f32) -> Self {
        self.min_size = (width, height);
        self
    }

    /// Fail with [`OpenError::UnsupportedParenting`] rather than fall back
    /// to a parent lock quark enforces itself (Wayland without
    /// xdg-foreign, for example).
    pub fn require_native_parent(mut self, require: bool) -> Self {
        self.require_native_parent = require;
        self
    }

    pub fn view(&self) -> &WebViewOptions {
        &self.view
    }

    /// Check the options and initial URL, returning the view's policy.
    pub(crate) fn validate(&self, url: &Url) -> Result<policy::NavigationPolicy, OpenError> {
        let view = &self.view;
        if view.navigation.is_empty() {
            return Err(OpenError::EmptyNavigationAllowlist);
        }
        if let Some(origin) = view
            .evaluation
            .iter()
            .find(|origin| !view.navigation.contains(origin))
        {
            return Err(OpenError::EvaluationOriginNotNavigable(origin.clone()));
        }
        if view.devtools && !cfg!(debug_assertions) {
            return Err(OpenError::DevtoolsUnavailable);
        }
        let sizes = [self.size.0, self.size.1, self.min_size.0, self.min_size.1];
        if sizes.iter().any(|size| !size.is_finite() || *size <= 0.0) {
            return Err(OpenError::InvalidSize);
        }
        if view
            .user_agent
            .as_deref()
            .is_some_and(|agent| agent.chars().any(char::is_control))
        {
            return Err(OpenError::InvalidUserAgent);
        }
        let policy = policy::NavigationPolicy::new(
            view.navigation.clone(),
            view.evaluation.clone(),
            view.popups,
        );
        policy.check(url).map_err(OpenError::UrlRejected)?;
        Ok(policy)
    }
}

/// How the modal relates to its parent, in [`Capabilities`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ParentRelationship {
    /// The window system knows the parent: stacking, centering, and
    /// minimizing follow it.
    Native,
    /// Quark blocks the parent's input itself; the window manager treats
    /// the webview as an ordinary window.
    AppEnforced,
}

/// Which kind of data store a view got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ProfileMode {
    Ephemeral,
    Persistent,
}

/// What a webview actually got, in [`WebViewEvent::Opened`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct Capabilities {
    pub parent: ParentRelationship,
    pub profile: ProfileMode,
    /// The appearance in effect; [`Appearance::System`] when the engine
    /// cannot set one per view.
    pub appearance: Appearance,
}

impl Capabilities {
    #[allow(dead_code)]
    pub(crate) fn new(
        parent: ParentRelationship,
        profile: ProfileMode,
        appearance: Appearance,
    ) -> Self {
        Self {
            parent,
            profile,
            appearance,
        }
    }
}

/// When a navigation failed, in [`WebViewEvent::NavigationFailed`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum FailureStage {
    /// Before the document committed; the previous page stays.
    Provisional,
    /// After commit, while loading.
    Committed,
}

/// Why a navigation failed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum NavigationError {
    /// Certificate or TLS failure. Never overridden.
    Tls,
    /// DNS, connection, or other network failure.
    Transport,
    /// Stopped by a newer navigation or by the engine.
    Cancelled,
    /// Refused by the navigation policy after it started (a redirect or
    /// response to a disallowed origin).
    Policy(BlockReason),
    /// The web content process ended.
    ProcessTerminated,
    Other(ErrorDetail),
}

/// Why a webview closed, in [`WebViewEvent::Closed`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WebCloseReason {
    /// The user closed the window.
    User,
    /// The app called `close`.
    Program,
    /// Its parent window closed.
    ParentClosed,
    /// The app is exiting.
    Quit,
    /// The engine or window could not be created. The view never opened.
    OpenFailed(OpenError),
    /// The web content process ended. Open a fresh view to retry.
    ProcessTerminated,
    /// The engine let a document from a disallowed origin commit, so the
    /// view was closed before anything could run in it.
    PolicyViolation,
}

/// Events about webviews, as `AppEvent::WebView` in `quark-app`.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum WebViewEvent {
    /// The engine and window exist. The page is not loaded yet.
    Opened {
        view: WebViewHandle,
        capabilities: Capabilities,
    },
    /// An allowed top-level navigation started. Scripts can no longer run
    /// in the previous document.
    NavigationStarted {
        view: WebViewHandle,
        navigation: NavigationId,
        url: PageUrl,
    },
    /// A redirect the policy allowed.
    NavigationRedirected {
        view: WebViewHandle,
        navigation: NavigationId,
        url: PageUrl,
    },
    /// A navigation, frame load, or popup refused before it loaded.
    NavigationBlocked {
        view: WebViewHandle,
        /// `None` when the engine's URL did not parse.
        url: Option<PageUrl>,
        reason: BlockReason,
    },
    /// A new top-level document committed; pass `document` and `origin`
    /// to an [`OriginGuard`].
    NavigationCommitted {
        view: WebViewHandle,
        navigation: NavigationId,
        document: DocumentId,
        url: PageUrl,
        origin: Origin,
    },
    NavigationFailed {
        view: WebViewHandle,
        navigation: NavigationId,
        stage: FailureStage,
        error: NavigationError,
    },
    /// The document finished loading successfully. Says nothing about
    /// whether the page's own app state is ready.
    PageLoadFinished {
        view: WebViewHandle,
        navigation: NavigationId,
        document: DocumentId,
        /// The HTTP status where the engine reports it; an error status
        /// can still load a page.
        http_status: Option<u16>,
    },
    /// A same-document change (fragment, `history.pushState`). The
    /// document and its scripts' permission stay.
    LocationChanged {
        view: WebViewHandle,
        document: DocumentId,
        url: PageUrl,
    },
    /// The page's title. Untrusted; never an authorization signal.
    TitleChanged { view: WebViewHandle, title: String },
    /// The result of an `evaluate_script_event` call.
    EvaluationFinished {
        view: WebViewHandle,
        evaluation: EvaluationId,
        result: Result<ScriptValue, EvalError>,
    },
    /// The view closed. Every handle ends with exactly one of these.
    Closed {
        view: WebViewHandle,
        reason: WebCloseReason,
    },
}

impl WebViewEvent {
    pub fn view(&self) -> WebViewHandle {
        match self {
            Self::Opened { view, .. }
            | Self::NavigationStarted { view, .. }
            | Self::NavigationRedirected { view, .. }
            | Self::NavigationBlocked { view, .. }
            | Self::NavigationCommitted { view, .. }
            | Self::NavigationFailed { view, .. }
            | Self::PageLoadFinished { view, .. }
            | Self::LocationChanged { view, .. }
            | Self::TitleChanged { view, .. }
            | Self::EvaluationFinished { view, .. }
            | Self::Closed { view, .. } => *view,
        }
    }
}

/// A native failure. `Debug` and `Display` redact the detail.
#[derive(Debug, Clone, PartialEq, Eq, Hash, thiserror::Error)]
#[error("{context}: {detail}")]
pub struct PlatformError {
    context: &'static str,
    detail: ErrorDetail,
}

impl PlatformError {
    #[allow(dead_code)]
    pub(crate) fn new(context: &'static str, detail: impl Into<String>) -> Self {
        Self {
            context,
            detail: ErrorDetail::new(detail),
        }
    }

    /// What was being done, such as `"create the WebKit view"`.
    pub fn context(&self) -> &'static str {
        self.context
    }

    pub fn detail(&self) -> &ErrorDetail {
        &self.detail
    }
}

/// Why a webview could not open.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum OpenError {
    /// No webview engine is built for this platform, or the feature is off.
    #[error("webviews are not supported in this build")]
    Unsupported,
    /// The calling context has no open window to parent the modal to.
    #[error("a modal webview needs an open parent window")]
    ParentRequired,
    /// The parent already has a modal webview.
    #[error("the parent window already has a modal webview")]
    ModalAlreadyOpen,
    #[error("the navigation allowlist is empty")]
    EmptyNavigationAllowlist,
    #[error("evaluation origin {0} is not a navigation origin")]
    EvaluationOriginNotNavigable(Origin),
    /// The initial URL fails the navigation policy.
    #[error("initial URL rejected: {0}")]
    UrlRejected(BlockReason),
    #[error("developer tools are unavailable in release builds")]
    DevtoolsUnavailable,
    #[error("window sizes must be finite and positive")]
    InvalidSize,
    #[error("the user agent has control characters")]
    InvalidUserAgent,
    /// [`WebWindowOptions::require_native_parent`] was set and the window
    /// system cannot parent the modal natively.
    #[error("native parenting is unsupported here")]
    UnsupportedParenting,
    #[error(transparent)]
    Profile(ProfileError),
    /// The engine reported a version or interface too old for the
    /// required APIs.
    #[error("the webview runtime is missing required features")]
    UnsupportedRuntime,
    #[error("could not open the webview: {0}")]
    Platform(PlatformError),
}

/// Why a script evaluation failed. Each accepted evaluation ends with
/// exactly one result.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EvalError {
    /// The view is closed or the handle is stale.
    #[error("no such webview")]
    InvalidHandle,
    /// No committed document: the view is opening or mid-navigation.
    #[error("no committed document")]
    FrameNotReady,
    /// The document's origin is not the guard's, or not an evaluation
    /// origin.
    #[error("wrong origin")]
    WrongOrigin,
    /// The guard's document is gone, or a navigation started while the
    /// script ran, even back to the same origin.
    #[error("the document changed")]
    NavigationChanged,
    #[error("timed out")]
    Timeout,
    #[error("cancelled")]
    Cancelled,
    #[error("the webview closed")]
    WindowClosed,
    #[error("the app is exiting")]
    AppExiting,
    /// The script threw or rejected.
    #[error("JavaScript exception: {0}")]
    JavaScriptException(ErrorDetail),
    #[error("invalid result: {0:?}")]
    InvalidResult(InvalidResult),
    #[error("result too large")]
    ResultTooLarge,
    /// More than [`EvaluationLimits::max_in_flight`] at once.
    #[error("too many evaluations in flight")]
    TooManyRequests,
    #[error("the web content process ended")]
    ProcessTerminated,
    /// The engine lacks the async evaluation API this needs.
    #[error("the webview runtime cannot run async scripts")]
    UnsupportedRuntime,
    #[error("evaluation failed: {0}")]
    Platform(PlatformError),
}

/// The receiving half of a one-shot result shared with the service.
pub(crate) struct Oneshot<T> {
    value: Option<T>,
    waker: Option<std::task::Waker>,
}

pub(crate) type Shared<T> = Arc<Mutex<Oneshot<T>>>;

pub(crate) fn oneshot<T>() -> Shared<T> {
    Arc::new(Mutex::new(Oneshot {
        value: None,
        waker: None,
    }))
}

/// Complete `shared` and wake whoever awaits it.
pub(crate) fn complete<T>(shared: &Shared<T>, value: T) {
    let waker = {
        let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
        slot.value = Some(value);
        slot.waker.take()
    };
    if let Some(waker) = waker {
        waker.wake();
    }
}

fn poll_shared<T>(shared: &Shared<T>, cx: &mut Context<'_>) -> Poll<T> {
    let mut slot = shared.lock().unwrap_or_else(|poison| poison.into_inner());
    match slot.value.take() {
        Some(value) => Poll::Ready(value),
        None => {
            slot.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}

/// A pending script result from `evaluate_script`. Resolves on any thread's
/// executor once the UI thread has validated the result; dropping it first
/// cancels the evaluation.
#[must_use = "dropping an Evaluation cancels it"]
pub struct Evaluation {
    id: EvaluationId,
    shared: Shared<Result<ScriptValue, EvalError>>,
    inbox: Arc<backend::Inbox>,
    done: bool,
}

impl Evaluation {
    pub fn id(&self) -> EvaluationId {
        self.id
    }
}

impl Future for Evaluation {
    type Output = Result<ScriptValue, EvalError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let poll = poll_shared(&self.shared, cx);
        if poll.is_ready() {
            self.done = true;
        }
        poll
    }
}

impl Drop for Evaluation {
    fn drop(&mut self) {
        if !self.done {
            self.inbox.cancel(self.id);
        }
    }
}

impl fmt::Debug for Evaluation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Evaluation").field("id", &self.id).finish()
    }
}

/// A pending profile removal from `clear_profile`. Resolves once the
/// engine has deleted every kind of stored data, or failed to.
#[must_use = "a ProfileClear reports whether the data is gone"]
pub struct ProfileClear {
    shared: Shared<Result<(), ProfileError>>,
}

impl Future for ProfileClear {
    type Output = Result<(), ProfileError>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        poll_shared(&self.shared, cx)
    }
}

impl fmt::Debug for ProfileClear {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ProfileClear")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(navigation: &[&str], evaluation: &[&str]) -> WebWindowOptions {
        let parse = |list: &[&str]| {
            list.iter()
                .map(|o| Origin::parse(o).unwrap())
                .collect::<Vec<_>>()
        };
        WebWindowOptions::new(
            WebViewOptions::new(parse(navigation)).evaluation_origins(parse(evaluation)),
        )
    }

    #[test]
    fn open_options_are_checked_against_the_initial_url() {
        let url = Url::parse("https://app.example/login").unwrap();
        let app = "https://app.example";
        let idp = "https://idp.example";
        for (options, expected) in [
            (options(&[app, idp], &[app]), Ok(())),
            (options(&[], &[]), Err(OpenError::EmptyNavigationAllowlist)),
            (
                options(&[app], &[idp]),
                Err(OpenError::EvaluationOriginNotNavigable(
                    Origin::parse(idp).unwrap(),
                )),
            ),
            (
                options(&[idp], &[]),
                Err(OpenError::UrlRejected(BlockReason::OriginNotAllowed)),
            ),
            (
                options(&[app], &[]).size(f32::NAN, 10.0),
                Err(OpenError::InvalidSize),
            ),
            (
                WebWindowOptions::new(options(&[app], &[]).view.user_agent("x\r\nCookie: a")),
                Err(OpenError::InvalidUserAgent),
            ),
        ] {
            assert_eq!(options.validate(&url).map(|_| ()), expected, "{options:?}");
        }
    }
}
