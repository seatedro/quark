//! Navigation policy and lifecycle, through the hooks quark's wry patch adds
//! to wry's own `WKNavigationDelegate` (see `vendor/wry/VENDORED.md`).
//!
//! WebKit asks for an action decision before every top-level and frame
//! navigation, redirect included, and a response decision before a
//! document is accepted. Both go to the policy. Lifecycle callbacks carry
//! the `WKNavigation` object, which correlates them with the quark
//! [`NavigationId`] issued at provisional start.

use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AllocAnyThread, DefinedClass, Message, define_class, msg_send, sel};
use objc2_foundation::{
    NSDictionary, NSError, NSHTTPURLResponse, NSKeyValueChangeKey, NSKeyValueObservingOptions,
    NSObjectNSKeyValueObserverRegistration, NSObjectProtocol, NSString, NSURL, NSURLRequest,
    ns_string,
};
use objc2_web_kit::{WKNavigation, WKNavigationAction, WKNavigationResponse, WKWebView};
use url::Url;

use crate::backend::NativeSink;
use crate::policy::{BlockReason, Decision, Target};
use crate::script::ErrorDetail;
use crate::{FailureStage, NavigationError, NavigationId};

/// Attempts tracked at once. WebKit drops superseded attempts without
/// always reporting them; the oldest beyond this are forgotten.
const MAX_ATTEMPTS: usize = 8;

/// One view's navigation bookkeeping, shared by its hooks and URL
/// observer. Borrowed only briefly, never across a call into WebKit.
#[derive(Default)]
pub(super) struct NavState {
    /// Started attempts, keyed by the `WKNavigation` WebKit passed (`None`
    /// when it passed nil). Retained so an address is not reused while
    /// listed.
    attempts: Vec<Attempt>,
    /// The main-frame response status, from the response decision until the
    /// commit that follows it.
    status: Option<u16>,
    /// Why the policy last refused a main-frame action or response; WebKit
    /// then fails the attempt as cancelled or interrupted.
    refused: Option<BlockReason>,
    /// The URL last reported, to tell same-document changes from loads.
    url: Option<Url>,
}

struct Attempt {
    native: Option<Retained<WKNavigation>>,
    id: NavigationId,
    status: Option<u16>,
}

impl NavState {
    fn find(&self, native: Option<&WKNavigation>) -> Option<usize> {
        self.attempts.iter().position(|attempt| {
            attempt.native.as_deref().map(|n| n as *const WKNavigation)
                == native.map(|n| n as *const WKNavigation)
        })
    }

    fn push(&mut self, native: Option<&WKNavigation>, id: NavigationId) {
        if let Some(index) = self.find(native) {
            self.attempts.remove(index);
        }
        if self.attempts.len() == MAX_ATTEMPTS {
            self.attempts.remove(0);
        }
        self.attempts.push(Attempt {
            native: native.map(|n| n.retain()),
            id,
            status: None,
        });
    }
}

/// The navigation hooks for one view.
pub(super) struct Hooks {
    pub(super) sink: NativeSink,
    pub(super) state: Rc<RefCell<NavState>>,
}

impl Hooks {
    /// The attempt `native` belongs to, starting one if WebKit skipped the
    /// provisional callback (so a commit is never reported unstarted).
    fn attempt(&self, webview: &WKWebView, native: Option<&WKNavigation>) -> NavigationId {
        if let Some(index) = self.state.borrow().find(native) {
            return self.state.borrow().attempts[index].id;
        }
        let url = page_url(webview);
        let id = self.sink.navigation_started(&url);
        self.state.borrow_mut().push(native, id);
        id
    }

    /// Forget `native`'s attempt, returning its id and response status.
    fn take(&self, native: Option<&WKNavigation>) -> Option<(NavigationId, Option<u16>)> {
        let mut state = self.state.borrow_mut();
        let index = state.find(native)?;
        let attempt = state.attempts.remove(index);
        Some((attempt.id, attempt.status))
    }
}

impl wry::NavigationHooks for Hooks {
    fn decide_action(&self, webview: &WKWebView, action: &WKNavigationAction) -> bool {
        let url = request_url(&*unsafe { action.request() });
        // `shouldPerformDownload` is macOS 11.3+.
        let download = action.respondsToSelector(sel!(shouldPerformDownload))
            && unsafe { action.shouldPerformDownload() };
        if download {
            self.sink
                .blocked(Url::parse(&url).ok(), BlockReason::Download);
            return false;
        }
        let target = match unsafe { action.targetFrame() } {
            None => Target::NewWindow,
            Some(frame) if unsafe { frame.isMainFrame() } => Target::MainFrame,
            Some(_) => Target::Subframe,
        };
        match self.sink.decide(&url, target) {
            Decision::Allow => true,
            Decision::LoadInCurrent(url) => {
                load_later(webview, &url);
                false
            }
            Decision::Block(reason) => {
                // A refused redirect fails the attempt it belongs to.
                if target == Target::MainFrame {
                    self.state.borrow_mut().refused = Some(reason);
                }
                false
            }
        }
    }

    fn decide_response(&self, _webview: &WKWebView, response: &WKNavigationResponse) -> bool {
        let main = unsafe { response.isForMainFrame() };
        let native = unsafe { response.response() };
        let url = native
            .URL()
            .and_then(|url| url.absoluteString())
            .map(|url| url.to_string())
            .unwrap_or_default();
        let refuse = |reason| {
            if main {
                self.state.borrow_mut().refused = Some(reason);
            }
            false
        };
        // What WebKit cannot show it would download, which v1 never does.
        if !unsafe { response.canShowMIMEType() } {
            self.sink
                .blocked(Url::parse(&url).ok(), BlockReason::Download);
            return refuse(BlockReason::Download);
        }
        let target = if main {
            Target::MainFrame
        } else {
            Target::Subframe
        };
        match self.sink.decide(&url, target) {
            Decision::Allow => {
                if main {
                    let status = native
                        .downcast_ref::<NSHTTPURLResponse>()
                        .and_then(|http| u16::try_from(http.statusCode()).ok());
                    let mut state = self.state.borrow_mut();
                    state.status = status;
                    state.refused = None;
                }
                true
            }
            Decision::Block(reason) => refuse(reason),
            // Only new windows load elsewhere.
            Decision::LoadInCurrent(_) => refuse(BlockReason::OriginNotAllowed),
        }
    }

    fn provisional_started(&self, webview: &WKWebView, native: Option<&WKNavigation>) {
        let url = page_url(webview);
        let id = self.sink.navigation_started(&url);
        let mut state = self.state.borrow_mut();
        state.push(native, id);
        state.status = None;
        state.refused = None;
    }

    fn server_redirect(&self, webview: &WKWebView, native: Option<&WKNavigation>) {
        let id = self.attempt(webview, native);
        self.sink.navigation_redirected(id, &page_url(webview));
    }

    fn provisional_failed(
        &self,
        _webview: &WKWebView,
        native: Option<&WKNavigation>,
        error: &NSError,
    ) {
        self.failed_at(FailureStage::Provisional, native, error);
    }

    fn committed(&self, webview: &WKWebView, native: Option<&WKNavigation>) {
        let id = self.attempt(webview, native);
        let url = page_url(webview);
        {
            let mut state = self.state.borrow_mut();
            let status = state.status.take();
            if let Some(index) = state.find(native) {
                state.attempts[index].status = status;
            }
            state.url = Some(url.clone());
        }
        if self.sink.navigation_committed(id, &url).is_none() {
            // A document the policy refuses got through: the session closes
            // the view; stop it from loading anything more meanwhile.
            unsafe { webview.stopLoading() };
        }
    }

    fn finished(&self, _webview: &WKWebView, native: Option<&WKNavigation>) {
        if let Some((id, status)) = self.take(native) {
            self.sink.load_finished(id, status);
        }
    }

    fn failed(&self, _webview: &WKWebView, native: Option<&WKNavigation>, error: &NSError) {
        self.failed_at(FailureStage::Committed, native, error);
    }
}

impl Hooks {
    fn failed_at(&self, stage: FailureStage, native: Option<&WKNavigation>, error: &NSError) {
        let Some((id, _)) = self.take(native) else {
            return;
        };
        let refused = self.state.borrow_mut().refused.take();
        let error = navigation_error(&error.domain().to_string(), error.code(), refused);
        self.sink.navigation_failed(id, stage, error);
    }
}

/// `NSURLErrorDomain` codes for certificate and TLS failures.
const TLS_ERRORS: std::ops::RangeInclusive<isize> = -1206..=-1200;
/// `NSURLErrorCancelled`.
const URL_CANCELLED: isize = -999;
/// `WebKitErrorFrameLoadInterruptedByPolicyChange`: a cancelled response.
const POLICY_INTERRUPTED: isize = 102;
/// `WKErrorWebContentProcessTerminated`.
const PROCESS_TERMINATED: isize = 2;

/// The quark error for a failed navigation's `NSError`. `refused` is the
/// policy's reason when it cancelled the response itself.
pub(super) fn navigation_error(
    domain: &str,
    code: isize,
    refused: Option<BlockReason>,
) -> NavigationError {
    match (domain, code) {
        ("NSURLErrorDomain", URL_CANCELLED) | ("WebKitErrorDomain", POLICY_INTERRUPTED) => {
            match refused {
                Some(reason) => NavigationError::Policy(reason),
                // Stopped by a newer navigation.
                None => NavigationError::Cancelled,
            }
        }
        ("NSURLErrorDomain", code) if TLS_ERRORS.contains(&code) => NavigationError::Tls,
        ("NSURLErrorDomain", _) => NavigationError::Transport,
        ("WKErrorDomain", PROCESS_TERMINATED) => NavigationError::ProcessTerminated,
        // The code identifies the failure; the description can quote the URL.
        (domain, code) => NavigationError::Other(ErrorDetail::new(format!("{domain} {code}"))),
    }
}

/// The view's current URL as WebKit reports it, or `about:blank`, which the
/// policy refuses.
fn page_url(webview: &WKWebView) -> Url {
    unsafe { webview.URL() }
        .and_then(|url| url.absoluteString())
        .and_then(|url| Url::parse(&url.to_string()).ok())
        .unwrap_or_else(|| Url::parse("about:blank").expect("a valid URL"))
}

fn request_url(request: &NSURLRequest) -> String {
    request
        .URL()
        .and_then(|url| url.absoluteString())
        .map(|url| url.to_string())
        .unwrap_or_default()
}

/// Load `url` in `webview` on the next run loop pass, outside the policy
/// callback that asked for it.
pub(super) fn load_later(webview: &WKWebView, url: &Url) {
    let Some(native) = NSURL::URLWithString(&NSString::from_str(url.as_str())) else {
        return;
    };
    let request = NSURLRequest::requestWithURL(&native);
    unsafe {
        let _: () = msg_send![
            webview,
            performSelector: sel!(loadRequest:),
            withObject: &*request,
            afterDelay: 0.0f64
        ];
    }
}

pub(super) struct UrlObserverIvars {
    webview: Retained<WKWebView>,
    sink: NativeSink,
    state: Rc<RefCell<NavState>>,
}

define_class!(
    /// Observes `WKWebView.URL` for same-document changes (fragments,
    /// `history.pushState`), which reach no navigation callback.
    #[unsafe(super(NSObject))]
    #[ivars = UrlObserverIvars]
    pub(super) struct UrlObserver;

    impl UrlObserver {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: Option<&NSDictionary<NSKeyValueChangeKey, AnyObject>>,
            _context: *mut c_void,
        ) {
            let ivars = self.ivars();
            let url = page_url(&ivars.webview);
            {
                let mut state = ivars.state.borrow_mut();
                if state.url.as_ref() == Some(&url) {
                    return;
                }
                state.url = Some(url.clone());
            }
            // The sink ignores this unless a document is committed and the
            // URL stays on its origin; a provisional URL never is.
            ivars.sink.location_changed(&url);
        }
    }

    unsafe impl NSObjectProtocol for UrlObserver {}
);

impl UrlObserver {
    pub(super) fn new(
        webview: Retained<WKWebView>,
        sink: NativeSink,
        state: Rc<RefCell<NavState>>,
    ) -> Retained<Self> {
        let observer = Self::alloc().set_ivars(UrlObserverIvars {
            webview,
            sink,
            state,
        });
        let observer: Retained<Self> = unsafe { msg_send![super(observer), init] };
        unsafe {
            observer
                .ivars()
                .webview
                .addObserver_forKeyPath_options_context(
                    &observer,
                    ns_string!("URL"),
                    NSKeyValueObservingOptions::New,
                    std::ptr::null_mut(),
                );
        }
        observer
    }

    /// Stop observing; call before the webview goes away.
    pub(super) fn detach(&self) {
        unsafe {
            self.ivars()
                .webview
                .removeObserver_forKeyPath(self, ns_string!("URL"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_map_to_their_kind() {
        let refused = Some(BlockReason::OriginNotAllowed);
        for (domain, code, refused, expected) in [
            ("NSURLErrorDomain", -1202, None, NavigationError::Tls),
            ("NSURLErrorDomain", -1200, None, NavigationError::Tls),
            ("NSURLErrorDomain", -1206, None, NavigationError::Tls),
            ("NSURLErrorDomain", -1004, None, NavigationError::Transport),
            ("NSURLErrorDomain", -999, None, NavigationError::Cancelled),
            (
                "NSURLErrorDomain",
                -999,
                refused,
                NavigationError::Policy(BlockReason::OriginNotAllowed),
            ),
            (
                "WebKitErrorDomain",
                102,
                refused,
                NavigationError::Policy(BlockReason::OriginNotAllowed),
            ),
            ("WebKitErrorDomain", 102, None, NavigationError::Cancelled),
            ("WKErrorDomain", 2, None, NavigationError::ProcessTerminated),
        ] {
            assert_eq!(
                navigation_error(domain, code, refused),
                expected,
                "{domain} {code}"
            );
        }
    }
}
