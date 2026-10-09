//! The private contract between the session state machine and the native
//! engines.
//!
//! Everything here runs on the UI thread. The session issues commands
//! through [`Backend`] after app callbacks return; backends answer through
//! [`NativeSink`], which updates a view's security state synchronously (so
//! a navigation revokes script permission before anything else runs) and
//! queues an event for the session's next drain. Backends never call the
//! app or the runner.
//!
//! Backend obligations:
//!
//! - Construct the engine without the remote URL, install policy and load
//!   handlers, set up the data store, then load [`OpenRequest::url`].
//! - Ask [`NativeSink::decide`] for every top-level, frame, redirect,
//!   popup, and response decision, and complete every native decision
//!   exactly once. Never hand a URL to the operating system.
//! - Report `navigation_started` before `navigation_committed`; report a
//!   failed attempt with `navigation_failed` and never as finished.
//! - Run [`EvalDispatch::envelope`] in the main frame's page world only if
//!   [`NativeSink::current_document`] still equals the dispatch's
//!   document, and answer every dispatched evaluation with
//!   `evaluation_settled` (after `cancel`, with `EvalError::Cancelled` or
//!   not at all; the session has already moved on).
//! - [`Backend::close`] is idempotent; report `destroyed` once the native
//!   view and window are gone, after which the profile is released.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use raw_window_handle::{RawDisplayHandle, RawWindowHandle};
use url::Url;

use crate::policy::{BlockReason, Decision, NavigationPolicy, Origin, Target};
use crate::profile::{ProfileError, ProfileId};
use crate::script::{RawEvaluation, ScriptEnvelope};
use crate::{
    Capabilities, DocumentId, EvaluationId, FailureStage, NavigationError, NavigationId, OpenError,
    PlatformError, WebViewHandle, WebWindowOptions,
};

#[cfg(all(feature = "native", target_os = "linux"))]
mod linux;
#[cfg(all(feature = "native", target_os = "macos"))]
mod macos;
// Built for tests everywhere: the WebView2 backend's CDP and navigation
// bookkeeping has no COM, only its glue does.
#[cfg(any(test, all(feature = "native", windows)))]
mod windows;

/// While any backend needs servicing, the runner waits at most this long
/// between backend service calls. An initial tuning value for GLib on
/// Linux, not a measured latency.
pub const ACTIVE_SERVICE_INTERVAL: Duration = Duration::from_millis(8);

/// Main-context iterations one backend service call may dispatch.
pub const SERVICE_ITERATIONS: u32 = 64;

/// Events queued between drains, beyond which informational events (titles,
/// locations, navigation progress) are dropped. Results, opens, and closes
/// are always kept; they are bounded by the views and in-flight limits.
const INBOX_CAPACITY: usize = 1024;

/// The parent window's native handles. The runner guarantees they outlive
/// the view: it closes a parent's webviews before destroying the parent.
#[derive(Debug, Clone, Copy)]
pub struct NativeParent {
    pub window: RawWindowHandle,
    pub display: RawDisplayHandle,
}

/// What one backend service call left behind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Serviced {
    /// No more ready work.
    Idle,
    /// The iteration budget ran out with work still ready; service again
    /// right away instead of waiting.
    Exhausted,
}

/// A native webview engine. Implemented per platform under
/// `backend/<platform>/`.
#[allow(dead_code)]
pub(crate) trait Backend {
    /// Create the window and engine for `request.view`. An error here, or
    /// a later [`NativeSink::open_failed`], fails the open; otherwise call
    /// [`NativeSink::opened`] once the engine exists.
    fn open(&mut self, request: OpenRequest) -> Result<(), OpenError>;

    /// Run a guarded script; answer with [`NativeSink::evaluation_settled`].
    fn evaluate(&mut self, view: WebViewHandle, dispatch: EvalDispatch);

    /// Stop waiting for an evaluation; cancel the native call where the
    /// engine can.
    fn cancel(&mut self, view: WebViewHandle, evaluation: EvaluationId);

    /// Tear the view and its window down; idempotent. Report
    /// [`NativeSink::destroyed`] when done.
    fn close(&mut self, view: WebViewHandle);

    /// Bring the view's window forward, for a click on its blocked parent.
    fn focus(&mut self, _view: WebViewHandle) {}

    /// Delete a persistent profile's data; no view uses it.
    fn clear_profile(&mut self, request: ClearRequest);

    /// Dispatch ready native work without blocking, at most `iterations`
    /// main-context iterations. Never called from inside an app callback.
    fn service(&mut self, _iterations: u32) -> Serviced {
        Serviced::Idle
    }

    /// Whether native work happens outside the winit loop's sight (GLib),
    /// so the runner must call [`Self::service`] every
    /// [`ACTIVE_SERVICE_INTERVAL`]. True while views or teardowns exist.
    fn needs_service(&self) -> bool {
        false
    }

    /// The app is exiting: release everything now.
    fn shutdown(&mut self) {}
}

/// Pick the backend built for this platform.
pub(crate) fn native() -> Option<Box<dyn Backend>> {
    #[cfg(all(feature = "native", target_os = "linux"))]
    return linux::backend();
    #[cfg(all(feature = "native", target_os = "macos"))]
    return macos::backend();
    #[cfg(all(feature = "native", windows))]
    return windows::backend();
    #[allow(unreachable_code)]
    None
}

/// A view to construct.
#[allow(dead_code)]
pub(crate) struct OpenRequest {
    pub(crate) view: WebViewHandle,
    /// The initial URL, already allowed by the policy.
    pub(crate) url: Url,
    pub(crate) options: WebWindowOptions,
    pub(crate) parent: NativeParent,
    pub(crate) sink: NativeSink,
}

/// A script to run in the main frame of `document`.
#[derive(Debug)]
#[allow(dead_code)]
pub(crate) struct EvalDispatch {
    pub(crate) evaluation: EvaluationId,
    pub(crate) document: DocumentId,
    pub(crate) envelope: ScriptEnvelope,
}

/// A profile to delete; answer through `sink`.
#[allow(dead_code)]
pub(crate) struct ClearRequest {
    pub(crate) profile: ProfileId,
    pub(crate) sink: ProfileSink,
}

/// Answers one [`ClearRequest`]. Dropping it unanswered reports a failure.
pub(crate) struct ProfileSink {
    id: u64,
    inbox: Arc<Inbox>,
    answered: bool,
}

impl ProfileSink {
    #[allow(dead_code)]
    pub(crate) fn finished(mut self, result: Result<(), ProfileError>) {
        self.answered = true;
        self.inbox.push(Inbound::ProfileCleared(self.id, result));
    }
}

impl Drop for ProfileSink {
    fn drop(&mut self) {
        if !self.answered {
            let error = PlatformError::new("clear the profile", "the backend dropped the request");
            self.inbox.push(Inbound::ProfileCleared(
                self.id,
                Err(ProfileError::Platform(error)),
            ));
        }
    }
}

/// Why the native side closed a view on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub(crate) enum NativeClose {
    /// The user closed the window.
    User,
    /// The web content process ended.
    ProcessTerminated,
}

/// What the native side reports, drained by the session.
#[derive(Debug)]
pub(crate) enum NativeEvent {
    Opened(Capabilities),
    OpenFailed(OpenError),
    NavigationStarted {
        navigation: NavigationId,
        url: Url,
    },
    NavigationRedirected {
        navigation: NavigationId,
        url: Url,
    },
    NavigationBlocked {
        url: Option<Url>,
        reason: BlockReason,
    },
    NavigationCommitted {
        navigation: NavigationId,
        document: DocumentId,
        url: Url,
        origin: Origin,
    },
    NavigationFailed {
        navigation: NavigationId,
        stage: FailureStage,
        error: NavigationError,
    },
    LoadFinished {
        navigation: NavigationId,
        document: DocumentId,
        http_status: Option<u16>,
    },
    LocationChanged {
        document: DocumentId,
        url: Url,
    },
    TitleChanged(String),
    EvaluationSettled {
        evaluation: EvaluationId,
        raw: RawEvaluation,
    },
    /// A document from a disallowed origin committed anyway.
    PolicyViolation,
    Closed(NativeClose),
    Destroyed,
}

impl NativeEvent {
    /// Kept even when the inbox is full.
    fn is_critical(&self) -> bool {
        matches!(
            self,
            Self::Opened(_)
                | Self::OpenFailed(_)
                | Self::EvaluationSettled { .. }
                | Self::PolicyViolation
                | Self::Closed(_)
                | Self::Destroyed
        )
    }

    /// Only the newest of these per view matters.
    fn coalesces_with(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::TitleChanged(_), Self::TitleChanged(_))
                | (Self::LocationChanged { .. }, Self::LocationChanged { .. })
        )
    }
}

#[derive(Debug)]
pub(crate) enum Inbound {
    View(WebViewHandle, NativeEvent),
    ProfileCleared(u64, Result<(), ProfileError>),
}

#[derive(Default)]
struct Queue {
    events: VecDeque<Inbound>,
    /// Evaluations whose futures were dropped.
    cancels: Vec<EvaluationId>,
}

/// Shared between the session, every sink, and every pending
/// [`crate::Evaluation`]: queued native events and cancellations, plus the
/// runner's wake.
pub(crate) struct Inbox {
    queue: Mutex<Queue>,
    wake: Box<dyn Fn() + Send + Sync>,
    /// Navigation and document ids, unique across views.
    next_id: AtomicU64,
}

impl Inbox {
    pub(crate) fn new(wake: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            queue: Mutex::default(),
            wake,
            next_id: AtomicU64::new(1),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Queue> {
        self.queue
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn next_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn push(&self, inbound: Inbound) {
        {
            let mut queue = self.lock();
            if let Inbound::View(view, event) = &inbound {
                let same = |queued: &Inbound| matches!(queued, Inbound::View(other, queued) if other == view && event.coalesces_with(queued));
                if let Some(index) = queue.events.iter().position(same) {
                    queue.events[index] = inbound;
                    return;
                }
                if !event.is_critical() && queue.events.len() >= INBOX_CAPACITY {
                    return;
                }
            }
            queue.events.push_back(inbound);
        }
        (self.wake)();
    }

    pub(crate) fn cancel(&self, evaluation: EvaluationId) {
        self.lock().cancels.push(evaluation);
        (self.wake)();
    }

    pub(crate) fn take(&self) -> (VecDeque<Inbound>, Vec<EvaluationId>) {
        let mut queue = self.lock();
        (
            std::mem::take(&mut queue.events),
            std::mem::take(&mut queue.cancels),
        )
    }

    pub(crate) fn profile_sink(self: &Arc<Self>, id: u64) -> ProfileSink {
        ProfileSink {
            id,
            inbox: Arc::clone(self),
            answered: false,
        }
    }
}

/// A view's script authority, updated synchronously by native callbacks.
#[derive(Debug, Default)]
pub(crate) struct Live {
    /// The committed document scripts may target, if any.
    pub(crate) document: Option<(DocumentId, Origin)>,
    pub(crate) closed: bool,
}

pub(crate) type SharedLive = Arc<Mutex<Live>>;

pub(crate) fn lock_live(live: &SharedLive) -> MutexGuard<'_, Live> {
    live.lock().unwrap_or_else(|poison| poison.into_inner())
}

/// A backend's handle for reporting on one view. Cheap to clone into
/// native callbacks.
#[derive(Clone)]
pub(crate) struct NativeSink {
    view: WebViewHandle,
    inbox: Arc<Inbox>,
    live: SharedLive,
    policy: Arc<NavigationPolicy>,
}

#[allow(dead_code)]
impl NativeSink {
    pub(crate) fn new(
        view: WebViewHandle,
        inbox: Arc<Inbox>,
        live: SharedLive,
        policy: Arc<NavigationPolicy>,
    ) -> Self {
        Self {
            view,
            inbox,
            live,
            policy,
        }
    }

    pub(crate) fn view(&self) -> WebViewHandle {
        self.view
    }

    fn send(&self, event: NativeEvent) {
        self.inbox.push(Inbound::View(self.view, event));
    }

    /// The policy's answer for a navigation to `url`. Blocks are reported
    /// as [`crate::WebViewEvent::NavigationBlocked`] here; the backend only
    /// completes the native decision.
    pub(crate) fn decide(&self, url: &str, target: Target) -> Decision {
        let decision = self.policy.decide(url, target);
        if let Decision::Block(reason) = decision {
            self.send(NativeEvent::NavigationBlocked {
                url: Url::parse(url).ok(),
                reason,
            });
        }
        decision
    }

    /// Report a refusal the policy did not decide: a response the engine
    /// would download, a download it started, a custom protocol launch.
    pub(crate) fn blocked(&self, url: Option<Url>, reason: BlockReason) {
        self.send(NativeEvent::NavigationBlocked { url, reason });
    }

    pub(crate) fn opened(&self, capabilities: Capabilities) {
        self.send(NativeEvent::Opened(capabilities));
    }

    /// The open failed after [`Backend::open`] returned; the backend has
    /// released the view and reports nothing more for it.
    pub(crate) fn open_failed(&self, error: OpenError) {
        lock_live(&self.live).closed = true;
        self.send(NativeEvent::OpenFailed(error));
    }

    /// A top-level navigation the policy allowed is starting. Revokes
    /// script permission for the current document at once.
    pub(crate) fn navigation_started(&self, url: &Url) -> NavigationId {
        let navigation = NavigationId(self.inbox.next_id());
        lock_live(&self.live).document = None;
        self.send(NativeEvent::NavigationStarted {
            navigation,
            url: url.clone(),
        });
        navigation
    }

    pub(crate) fn navigation_redirected(&self, navigation: NavigationId, url: &Url) {
        self.send(NativeEvent::NavigationRedirected {
            navigation,
            url: url.clone(),
        });
    }

    /// A new top-level document committed at `url`. Returns its id, or
    /// `None` when the policy refuses `url`: the view is then closed, and
    /// the backend should stop loading.
    pub(crate) fn navigation_committed(
        &self,
        navigation: NavigationId,
        url: &Url,
    ) -> Option<DocumentId> {
        let mut live = lock_live(&self.live);
        live.document = None;
        if live.closed {
            return None;
        }
        let origin = match self.policy.check(url) {
            Ok(origin) => origin,
            Err(_) => {
                live.closed = true;
                drop(live);
                self.send(NativeEvent::PolicyViolation);
                return None;
            }
        };
        let document = DocumentId(self.inbox.next_id());
        live.document = Some((document, origin.clone()));
        drop(live);
        self.send(NativeEvent::NavigationCommitted {
            navigation,
            document,
            url: url.clone(),
            origin,
        });
        Some(document)
    }

    pub(crate) fn navigation_failed(
        &self,
        navigation: NavigationId,
        stage: FailureStage,
        error: NavigationError,
    ) {
        let mut live = lock_live(&self.live);
        // A failure after commit leaves an error page or a broken document.
        live.document = None;
        drop(live);
        self.send(NativeEvent::NavigationFailed {
            navigation,
            stage,
            error,
        });
    }

    /// The navigation's document finished loading. Ignored once the
    /// document is gone, as after a failure.
    pub(crate) fn load_finished(&self, navigation: NavigationId, http_status: Option<u16>) {
        let live = lock_live(&self.live);
        let Some((document, _)) = &live.document else {
            return;
        };
        let document = *document;
        drop(live);
        self.send(NativeEvent::LoadFinished {
            navigation,
            document,
            http_status,
        });
    }

    /// A same-document URL change. Ignored unless it stays on the
    /// committed document's origin.
    pub(crate) fn location_changed(&self, url: &Url) {
        let live = lock_live(&self.live);
        let Some((document, origin)) = &live.document else {
            return;
        };
        if Origin::of(url).as_ref() != Ok(origin) {
            return;
        }
        let document = *document;
        drop(live);
        self.send(NativeEvent::LocationChanged {
            document,
            url: url.clone(),
        });
    }

    pub(crate) fn title_changed(&self, title: &str) {
        self.send(NativeEvent::TitleChanged(title.to_owned()));
    }

    /// The committed document a dispatch may run in right now.
    pub(crate) fn current_document(&self) -> Option<DocumentId> {
        let live = lock_live(&self.live);
        live.document.as_ref().map(|(document, _)| *document)
    }

    pub(crate) fn evaluation_settled(&self, evaluation: EvaluationId, raw: RawEvaluation) {
        self.send(NativeEvent::EvaluationSettled { evaluation, raw });
    }

    /// The window closed or the content process died on its own.
    pub(crate) fn closed(&self, reason: NativeClose) {
        let mut live = lock_live(&self.live);
        live.closed = true;
        live.document = None;
        drop(live);
        self.send(NativeEvent::Closed(reason));
    }

    /// Native teardown finished.
    pub(crate) fn destroyed(&self) {
        self.send(NativeEvent::Destroyed);
    }
}
