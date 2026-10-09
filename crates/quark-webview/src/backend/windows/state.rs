//! One WebView2 view's navigation and evaluation bookkeeping, free of COM.
//! The native glue forwards each WebView2 event and CDP message here and
//! carries out what comes back, so the ordering rules are tested on every
//! platform.
//!
//! WebView2 reports navigations by a numeric id that overlaps across
//! attempts: `NavigationStarting` (also once per redirect),
//! `ContentLoading` when a document commits (possibly an error page),
//! `SourceChanged` for same-document moves, and `NavigationCompleted`
//! last, successful or not. Commit here means `ContentLoading` for a
//! real document, never the provisional URL.

use std::collections::HashMap;

use url::Url;

use super::cdp::{self, CallOutcome, MainContext, Unbound};
use crate::backend::{EvalDispatch, NativeClose, NativeSink};
use crate::policy::{BlockReason, Decision, Origin, Target};
use crate::script::RawEvaluation;
use crate::{
    DocumentId, EvalError, EvaluationId, FailureStage, NavigationError, NavigationId, PlatformError,
};

/// The parts of `NavigationCompleted`'s `WebErrorStatus` the session tells
/// apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebError {
    /// `UNKNOWN`: no network error, as for an HTTP error status.
    None,
    Tls,
    Transport,
    Cancelled,
    /// Any other status, by its raw value.
    Other(i32),
}

/// What to do with a `NewWindowRequested`. The request is always marked
/// handled, so WebView2 never opens a window itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NewWindow {
    Deny,
    /// Load this allowed URL in the current view instead.
    Navigate(Url),
}

/// A `Runtime.callFunctionOn` to send for an evaluation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CdpCall {
    pub(crate) evaluation: EvaluationId,
    pub(crate) params: String,
}

#[derive(Debug)]
struct Attempt {
    navigation: NavigationId,
    committed: bool,
    error_page: bool,
    /// A redirect the policy refused; the completion reports it.
    blocked: Option<BlockReason>,
}

pub(crate) struct ViewState {
    sink: NativeSink,
    contexts: MainContext,
    attempts: HashMap<u64, Attempt>,
    /// The committed document and its origin as a browser serializes it.
    document: Option<(DocumentId, String)>,
    /// Dispatched before CDP reported the document's main context.
    waiting: Vec<EvalDispatch>,
    /// Sent to CDP and not answered yet.
    in_flight: Vec<EvaluationId>,
}

impl ViewState {
    pub(crate) fn new(sink: NativeSink) -> Self {
        Self {
            sink,
            contexts: MainContext::default(),
            attempts: HashMap::new(),
            document: None,
            waiting: Vec::new(),
            in_flight: Vec::new(),
        }
    }

    pub(crate) fn sink(&self) -> &NativeSink {
        &self.sink
    }

    /// `NavigationStarting` for the main frame. Returns whether to let it
    /// proceed; on false the glue sets `Cancel`.
    pub(crate) fn navigation_starting(&mut self, native: u64, uri: &str, redirected: bool) -> bool {
        let decision = self.sink.decide(uri, Target::MainFrame);
        let attempt = self.attempts.get_mut(&native).filter(|_| redirected);
        match (decision, attempt) {
            (Decision::Allow, Some(attempt)) => {
                if let Ok(url) = Url::parse(uri) {
                    self.sink.navigation_redirected(attempt.navigation, &url);
                }
                true
            }
            (Decision::Allow, None) => {
                let Ok(url) = Url::parse(uri) else {
                    return false;
                };
                let navigation = self.sink.navigation_started(&url);
                self.contexts.navigation_started();
                self.document = None;
                for dispatch in self.waiting.drain(..) {
                    self.sink.evaluation_settled(
                        dispatch.evaluation,
                        RawEvaluation::Failed(EvalError::NavigationChanged),
                    );
                }
                self.attempts.insert(
                    native,
                    Attempt {
                        navigation,
                        committed: false,
                        error_page: false,
                        blocked: None,
                    },
                );
                true
            }
            (Decision::Block(reason), Some(attempt)) => {
                attempt.blocked = Some(reason);
                false
            }
            // Refused before it started: `decide` reported the block and
            // nothing else is known about it.
            (Decision::Block(_) | Decision::LoadInCurrent(_), _) => false,
        }
    }

    /// `FrameNavigationStarting`: whether a frame may load `uri`.
    pub(crate) fn frame_navigation_starting(&self, uri: &str) -> bool {
        self.sink.decide(uri, Target::Subframe) == Decision::Allow
    }

    /// `NewWindowRequested` for `uri`.
    pub(crate) fn new_window(&self, uri: &str) -> NewWindow {
        match self.sink.decide(uri, Target::NewWindow) {
            Decision::LoadInCurrent(url) => NewWindow::Navigate(url),
            Decision::Allow | Decision::Block(_) => NewWindow::Deny,
        }
    }

    /// `ContentLoading`: a document for `native` is loading at `source`
    /// (the view's `Source` now). Returns false when the policy refused
    /// the committed URL; the session then closes the view and the glue
    /// stops loading.
    pub(crate) fn content_loading(&mut self, native: u64, source: &str, error_page: bool) -> bool {
        let Some(attempt) = self.attempts.get_mut(&native) else {
            return true;
        };
        if error_page {
            // WebView2's own error page for a failed load: never a commit.
            attempt.error_page = true;
            return true;
        }
        let committed = Url::parse(source).ok().and_then(|url| {
            Some((
                self.sink.navigation_committed(attempt.navigation, &url)?,
                url,
            ))
        });
        match committed {
            Some((document, url)) => {
                attempt.committed = true;
                let origin = Origin::of(&url)
                    .map(|origin| origin.to_string())
                    .unwrap_or_default();
                self.document = Some((document, origin));
                true
            }
            None => {
                self.document = None;
                false
            }
        }
    }

    /// `SourceChanged`; only same-document changes matter here.
    pub(crate) fn source_changed(&self, new_document: bool, source: &str) {
        if !new_document && let Ok(url) = Url::parse(source) {
            self.sink.location_changed(&url);
        }
    }

    /// `NavigationCompleted`.
    pub(crate) fn navigation_completed(
        &mut self,
        native: u64,
        success: bool,
        error: WebError,
        http_status: Option<u16>,
    ) {
        let Some(attempt) = self.attempts.remove(&native) else {
            return;
        };
        let stage = if attempt.committed {
            FailureStage::Committed
        } else {
            FailureStage::Provisional
        };
        let loaded = attempt.committed && !attempt.error_page;
        // An HTTP error status with a body loads that body as the page;
        // WebView2 still reports it as unsuccessful.
        let http_error_page =
            error == WebError::None && http_status.is_some_and(|status| status >= 400);
        if let Some(reason) = attempt.blocked {
            self.sink
                .navigation_failed(attempt.navigation, stage, NavigationError::Policy(reason));
        } else if loaded && (success || http_error_page) {
            self.sink.load_finished(attempt.navigation, http_status);
        } else {
            let error = match error {
                WebError::Tls => NavigationError::Tls,
                WebError::Transport => NavigationError::Transport,
                WebError::Cancelled => NavigationError::Cancelled,
                WebError::None | WebError::Other(_) => NavigationError::Other(
                    crate::ErrorDetail::new(format!("WebView2 web error {error:?}")),
                ),
            };
            let stage = if attempt.error_page {
                FailureStage::Provisional
            } else {
                stage
            };
            self.sink
                .navigation_failed(attempt.navigation, stage, error);
        }
    }

    /// The `Page.getFrameTree` response.
    pub(crate) fn frame_tree(&mut self, response: &str) -> Vec<CdpCall> {
        self.contexts.set_frame_tree(response);
        self.release_waiting()
    }

    /// A CDP context event; may make waiting evaluations runnable.
    pub(crate) fn cdp_event(&mut self, method: &str, params: &str) -> Vec<CdpCall> {
        self.contexts.on_event(method, params);
        self.release_waiting()
    }

    fn release_waiting(&mut self) -> Vec<CdpCall> {
        let waiting = std::mem::take(&mut self.waiting);
        waiting
            .into_iter()
            .filter_map(|dispatch| self.evaluate(dispatch))
            .collect()
    }

    /// Start an evaluation: a call to send now, or `None` when it settled
    /// at once or waits for the document's main context.
    pub(crate) fn evaluate(&mut self, dispatch: EvalDispatch) -> Option<CdpCall> {
        let origin = match &self.document {
            Some((document, origin))
                if *document == dispatch.document
                    && self.sink.current_document() == Some(*document) =>
            {
                origin.clone()
            }
            _ => {
                self.settle(
                    dispatch.evaluation,
                    RawEvaluation::Failed(EvalError::NavigationChanged),
                );
                return None;
            }
        };
        let failed = match self.contexts.bind(&origin) {
            Ok(context) => {
                let params = cdp::call_function_on(
                    &dispatch.envelope.function_declaration(),
                    &dispatch.envelope.input,
                    context,
                );
                self.in_flight.push(dispatch.evaluation);
                return Some(CdpCall {
                    evaluation: dispatch.evaluation,
                    params,
                });
            }
            Err(Unbound::NotReady) => {
                self.waiting.push(dispatch);
                return None;
            }
            Err(Unbound::WrongOrigin) => EvalError::WrongOrigin,
            Err(Unbound::NoUniqueId) => EvalError::UnsupportedRuntime,
        };
        self.settle(dispatch.evaluation, RawEvaluation::Failed(failed));
        None
    }

    /// `CallDevToolsProtocolMethod` completed for `evaluation`.
    pub(crate) fn call_finished(&mut self, evaluation: EvaluationId, succeeded: bool, json: &str) {
        let Some(index) = self.in_flight.iter().position(|id| *id == evaluation) else {
            return; // Cancelled meanwhile.
        };
        self.in_flight.swap_remove(index);
        let raw = match cdp::call_outcome(succeeded, json) {
            CallOutcome::Envelope(text) => RawEvaluation::Envelope(text),
            CallOutcome::NotAString => RawEvaluation::NotAString,
            CallOutcome::WrapperFailed => RawEvaluation::WrapperFailed,
            CallOutcome::ContextGone => RawEvaluation::Failed(EvalError::NavigationChanged),
            CallOutcome::Protocol(message) => RawEvaluation::Failed(EvalError::Platform(
                PlatformError::new("run the script", message),
            )),
        };
        self.settle(evaluation, raw);
    }

    /// Stop tracking `evaluation`; a late answer is dropped.
    pub(crate) fn cancel(&mut self, evaluation: EvaluationId) {
        self.waiting
            .retain(|dispatch| dispatch.evaluation != evaluation);
        self.in_flight.retain(|id| *id != evaluation);
    }

    /// The main content process or the browser process ended.
    pub(crate) fn process_gone(&mut self) {
        let pending = self
            .waiting
            .drain(..)
            .map(|dispatch| dispatch.evaluation)
            .chain(self.in_flight.drain(..));
        for evaluation in pending.collect::<Vec<_>>() {
            self.sink.evaluation_settled(
                evaluation,
                RawEvaluation::Failed(EvalError::ProcessTerminated),
            );
        }
        self.document = None;
        self.sink.closed(NativeClose::ProcessTerminated);
    }

    /// Forget every pending evaluation; the session already failed them as
    /// `WindowClosed`.
    pub(crate) fn close(&mut self) {
        self.waiting.clear();
        self.in_flight.clear();
        self.document = None;
    }

    fn settle(&self, evaluation: EvaluationId, raw: RawEvaluation) {
        self.sink.evaluation_settled(evaluation, raw);
    }
}

/// Whether a server certificate WebView2 rejected is the one test-only
/// certificate trusted for `host` (`crate::test_trust`). WebView2 hands
/// the certificate over as PEM; it matches only if its body is exactly the
/// base64 of the trusted DER.
pub(crate) fn test_trusted(host: &str, pem: &str, trust: Option<(&[u8], &[String])>) -> bool {
    let Some((der, hosts)) = trust else {
        return false;
    };
    if !hosts.iter().any(|trusted| trusted == host) {
        return false;
    }
    let body: String = pem
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with("-----"))
        .collect();
    let blocks = pem.matches("-----BEGIN CERTIFICATE-----").count();
    blocks == 1 && body == base64(der)
}

fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, byte)| n | u32::from(*byte) << (16 - 8 * i));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::backend::{Inbound, Inbox, NativeEvent, SharedLive};
    use crate::policy::{NavigationPolicy, PopupPolicy};
    use crate::script::{self, AsyncScript};
    use crate::{EvaluationLimits, WebViewHandle};

    const APP: &str = "https://app.test";
    const IDP: &str = "https://idp.test";

    struct Harness {
        state: ViewState,
        inbox: Arc<Inbox>,
    }

    fn harness(popups: PopupPolicy) -> Harness {
        let inbox = Arc::new(Inbox::new(Box::new(|| {})));
        let origins = vec![Origin::parse(APP).unwrap(), Origin::parse(IDP).unwrap()];
        let policy = NavigationPolicy::new(origins, vec![Origin::parse(APP).unwrap()], popups);
        let view = WebViewHandle {
            index: 0,
            generation: 0,
        };
        let sink = NativeSink::new(
            view,
            Arc::clone(&inbox),
            SharedLive::default(),
            Arc::new(policy),
        );
        Harness {
            state: ViewState::new(sink),
            inbox,
        }
    }

    impl Harness {
        /// Events reported since the last call, one line each. Ids are
        /// left out; tests that need them read `document` instead.
        fn events(&self) -> Vec<String> {
            let (events, _) = self.inbox.take();
            events
                .into_iter()
                .map(|inbound| match inbound {
                    Inbound::View(_, event) => match event {
                        NativeEvent::NavigationStarted { url, .. } => format!("started {url}"),
                        NativeEvent::NavigationRedirected { url, .. } => {
                            format!("redirected {url}")
                        }
                        NativeEvent::NavigationBlocked { url, reason } => {
                            format!(
                                "blocked {} {reason:?}",
                                url.map(|u| u.to_string()).unwrap_or_default()
                            )
                        }
                        NativeEvent::NavigationCommitted { url, .. } => format!("committed {url}"),
                        NativeEvent::NavigationFailed { stage, error, .. } => {
                            format!("failed {stage:?} {error:?}")
                        }
                        NativeEvent::LoadFinished { http_status, .. } => {
                            format!("finished {http_status:?}")
                        }
                        NativeEvent::LocationChanged { url, .. } => format!("location {url}"),
                        NativeEvent::EvaluationSettled { raw, .. } => format!("settled {raw:?}"),
                        NativeEvent::Closed(reason) => format!("closed {reason:?}"),
                        other => format!("{other:?}"),
                    },
                    Inbound::ProfileCleared(..) => "profile cleared".to_owned(),
                })
                .collect()
        }

        /// Starts, commits, and finishes a navigation to `url`, with the
        /// main context `unique` created for it; returns its document.
        fn load(&mut self, native: u64, url: &str, unique: &str) -> DocumentId {
            assert!(self.state.navigation_starting(native, url, false));
            assert!(self.state.content_loading(native, url, false));
            self.context(unique, url);
            self.state
                .navigation_completed(native, true, WebError::None, Some(200));
            self.events();
            self.state.document.as_ref().unwrap().0
        }

        fn context(&mut self, unique: &str, url: &str) -> Vec<CdpCall> {
            let origin = Origin::of(&Url::parse(url).unwrap()).unwrap().to_string();
            let params = json!({ "context": {
                "id": 1, "uniqueId": unique, "origin": origin, "name": "",
                "auxData": { "isDefault": true, "type": "default", "frameId": "MAIN" },
            }});
            self.state
                .cdp_event("Runtime.executionContextCreated", &params.to_string())
        }

        fn dispatch(&self, evaluation: u64, document: DocumentId) -> EvalDispatch {
            let script = AsyncScript::new("return 1;");
            EvalDispatch {
                evaluation: EvaluationId(evaluation),
                document,
                envelope: script::envelope(
                    &script,
                    &Origin::parse(APP).unwrap(),
                    &EvaluationLimits::default(),
                ),
            }
        }
    }

    type Drive = Box<dyn Fn(&mut Harness)>;

    fn opened() -> Harness {
        let mut h = harness(PopupPolicy::Deny);
        assert!(
            h.state
                .frame_tree(r#"{"frameTree":{"frame":{"id":"MAIN"}}}"#)
                .is_empty()
        );
        h
    }

    #[test]
    fn navigations_report_in_webview2_order() {
        let ok = |h: &mut Harness| {
            h.state.navigation_starting(1, "https://app.test/a", false);
            h.state.content_loading(1, "https://app.test/a", false);
        };
        let cases: Vec<(&str, Drive, Vec<&str>)> = vec![
            (
                "allowed redirect then load",
                Box::new(|h| {
                    h.state
                        .navigation_starting(1, "https://app.test/login", false);
                    h.state
                        .navigation_starting(1, "https://idp.test/auth", true);
                    h.state.content_loading(1, "https://idp.test/auth", false);
                    h.state
                        .navigation_completed(1, true, WebError::None, Some(200));
                }),
                vec![
                    "started https://app.test/login",
                    "redirected https://idp.test/auth",
                    "committed https://idp.test/auth",
                    "finished Some(200)",
                ],
            ),
            (
                "redirect to a disallowed origin fails by policy",
                Box::new(|h| {
                    h.state
                        .navigation_starting(1, "https://app.test/login", false);
                    assert!(!h.state.navigation_starting(1, "https://evil.test/", true));
                    h.state
                        .navigation_completed(1, false, WebError::Cancelled, None);
                }),
                vec![
                    "started https://app.test/login",
                    "blocked https://evil.test/ OriginNotAllowed",
                    "failed Provisional Policy(OriginNotAllowed)",
                ],
            ),
            (
                "a disallowed start is only blocked",
                Box::new(|h| {
                    assert!(!h.state.navigation_starting(1, "http://app.test/", false));
                    h.state
                        .navigation_completed(1, false, WebError::Cancelled, None);
                }),
                vec!["blocked http://app.test/ Scheme"],
            ),
            (
                "an error page never commits",
                Box::new(|h| {
                    h.state.navigation_starting(1, "https://app.test/", false);
                    h.state.content_loading(1, "https://app.test/", true);
                    h.state.navigation_completed(1, false, WebError::Tls, None);
                }),
                vec!["started https://app.test/", "failed Provisional Tls"],
            ),
            (
                "an HTTP error with a body finishes with its status",
                Box::new(move |h| {
                    ok(h);
                    h.state
                        .navigation_completed(1, false, WebError::None, Some(404));
                }),
                vec![
                    "started https://app.test/a",
                    "committed https://app.test/a",
                    "finished Some(404)",
                ],
            ),
            (
                "a load cut off after commit fails committed",
                Box::new(move |h| {
                    ok(h);
                    h.state
                        .navigation_completed(1, false, WebError::Transport, None);
                }),
                vec![
                    "started https://app.test/a",
                    "committed https://app.test/a",
                    "failed Committed Transport",
                ],
            ),
            (
                "a superseded attempt fails without finishing the newer one",
                Box::new(|h| {
                    h.state
                        .navigation_starting(1, "https://app.test/old", false);
                    h.state
                        .navigation_starting(2, "https://app.test/new", false);
                    h.state
                        .navigation_completed(1, false, WebError::Cancelled, None);
                    h.state.content_loading(2, "https://app.test/new", false);
                }),
                vec![
                    "started https://app.test/old",
                    "started https://app.test/new",
                    "failed Provisional Cancelled",
                    "committed https://app.test/new",
                ],
            ),
            (
                "same-document moves report a location",
                Box::new(move |h| {
                    ok(h);
                    h.state.source_changed(false, "https://app.test/a#done");
                    h.state.source_changed(true, "https://app.test/b");
                }),
                vec![
                    "started https://app.test/a",
                    "committed https://app.test/a",
                    "location https://app.test/a#done",
                ],
            ),
        ];
        for (name, drive, expected) in cases {
            let mut h = opened();
            drive(&mut h);
            assert_eq!(h.events(), expected, "{name}");
        }
    }

    #[test]
    fn a_committed_disallowed_document_stops_the_view() {
        let mut h = opened();
        h.state.navigation_starting(1, "https://app.test/", false);
        assert!(!h.state.content_loading(1, "https://evil.test/", false));
        assert_eq!(h.events(), ["started https://app.test/", "PolicyViolation"]);
    }

    #[test]
    fn popups_are_denied_or_loaded_in_place() {
        let h = opened();
        assert_eq!(h.state.new_window("https://app.test/pop"), NewWindow::Deny);
        assert_eq!(h.events(), ["blocked https://app.test/pop Popup"]);
        let h = harness(PopupPolicy::NavigateCurrent);
        assert_eq!(
            h.state.new_window("https://app.test/pop"),
            NewWindow::Navigate(Url::parse("https://app.test/pop").unwrap())
        );
        assert_eq!(h.state.new_window("https://evil.test/"), NewWindow::Deny);
        assert_eq!(h.events(), ["blocked https://evil.test/ OriginNotAllowed"]);
    }

    #[test]
    fn frames_load_only_allowed_origins() {
        let h = opened();
        assert!(h.state.frame_navigation_starting("https://idp.test/frame"));
        assert!(h.state.frame_navigation_starting("about:blank"));
        assert!(!h.state.frame_navigation_starting("https://evil.test/frame"));
    }

    #[test]
    fn an_evaluation_runs_in_the_documents_main_context() {
        let mut h = opened();
        let document = h.load(1, "https://app.test/", "u1");
        let call = h
            .state
            .evaluate(h.dispatch(7, document))
            .expect("sent at once");
        let params: serde_json::Value = serde_json::from_str(&call.params).unwrap();
        assert_eq!(params["uniqueContextId"], "u1");
        h.state.call_finished(
            EvaluationId(7),
            true,
            r#"{"result":{"type":"string","value":"E"}}"#,
        );
        assert_eq!(h.events(), [r#"settled Envelope("E")"#]);
    }

    /// A committed document whose main context CDP has not reported yet.
    fn committed_without_context(h: &mut Harness) -> DocumentId {
        h.state.navigation_starting(1, "https://app.test/", false);
        h.state.content_loading(1, "https://app.test/", false);
        h.events();
        h.state.document.as_ref().unwrap().0
    }

    #[test]
    fn an_evaluation_waits_for_the_main_context() {
        let mut h = opened();
        let document = committed_without_context(&mut h);
        assert_eq!(h.state.evaluate(h.dispatch(1, document)), None);
        let sent = h.context("u1", "https://app.test/");
        assert_eq!(
            sent.iter().map(|c| c.evaluation).collect::<Vec<_>>(),
            [EvaluationId(1)]
        );
    }

    #[test]
    fn a_waiting_evaluation_fails_when_a_navigation_starts() {
        let mut h = opened();
        let document = committed_without_context(&mut h);
        assert_eq!(h.state.evaluate(h.dispatch(1, document)), None);
        h.state
            .navigation_starting(2, "https://app.test/next", false);
        assert_eq!(
            h.events(),
            [
                "started https://app.test/next",
                "settled Failed(NavigationChanged)"
            ]
        );
        assert!(h.context("u2", "https://app.test/next").is_empty());
    }

    #[test]
    fn an_evaluation_for_a_replaced_document_never_runs() {
        let mut h = opened();
        let old = h.load(1, "https://app.test/", "u1");
        h.load(2, "https://app.test/", "u2");
        assert_eq!(h.state.evaluate(h.dispatch(1, old)), None);
        assert_eq!(h.events(), ["settled Failed(NavigationChanged)"]);
    }

    #[test]
    fn a_cancelled_evaluation_drops_its_late_answer() {
        let mut h = opened();
        let document = h.load(1, "https://app.test/", "u1");
        h.state.evaluate(h.dispatch(1, document)).unwrap();
        h.state.cancel(EvaluationId(1));
        h.state.call_finished(
            EvaluationId(1),
            true,
            r#"{"result":{"type":"string","value":"E"}}"#,
        );
        assert!(h.events().is_empty());
    }

    #[test]
    fn a_runtime_without_unique_context_ids_is_unsupported() {
        let mut h = opened();
        h.state.navigation_starting(1, "https://app.test/", false);
        h.state.content_loading(1, "https://app.test/", false);
        let params = json!({ "context": {
            "id": 1, "origin": "https://app.test", "name": "",
            "auxData": { "isDefault": true, "type": "default", "frameId": "MAIN" },
        }});
        h.state
            .cdp_event("Runtime.executionContextCreated", &params.to_string());
        let document = h.state.document.as_ref().unwrap().0;
        h.events();
        assert_eq!(h.state.evaluate(h.dispatch(1, document)), None);
        assert_eq!(h.events(), ["settled Failed(UnsupportedRuntime)"]);
    }

    #[test]
    fn a_crash_fails_pending_evaluations_and_closes() {
        let mut h = opened();
        let document = h.load(1, "https://app.test/", "u1");
        h.state.evaluate(h.dispatch(1, document)).unwrap();
        h.state.process_gone();
        assert_eq!(
            h.events(),
            [
                "settled Failed(ProcessTerminated)",
                "closed ProcessTerminated"
            ]
        );
    }

    #[test]
    fn only_the_trusted_certificate_for_its_host_passes() {
        let der: &[u8] = b"Many hands";
        let hosts = ["127.0.0.1".to_owned()];
        let trust = Some((der, &hosts[..]));
        let pem =
            "-----BEGIN CERTIFICATE-----\r\nTWFueSBo\r\nYW5kcw==\r\n-----END CERTIFICATE-----\r\n";
        for (host, pem, trust, expected) in [
            ("127.0.0.1", pem, trust, true),
            ("localhost", pem, trust, false),
            ("127.0.0.1", pem, None, false),
            (
                "127.0.0.1",
                "-----BEGIN CERTIFICATE-----\nTWFueSBoYW5kcx==\n-----END CERTIFICATE-----",
                trust,
                false,
            ),
            ("127.0.0.1", &format!("{pem}{pem}"), trust, false),
        ] {
            assert_eq!(test_trusted(host, pem, trust), expected, "{host} {pem:?}");
        }
    }
}
