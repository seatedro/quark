//! Guarded scripts through `callAsyncJavaScript:arguments:inFrame:inContentWorld:`.
//!
//! The call awaits a returned Promise in the engine, so the wrapper's
//! `await` of the app's getter needs no polling. It runs with `inFrame:
//! nil`, the main frame, and in `WKContentWorld.pageWorld`: the app's
//! getter is a global the page itself defines, and an isolated client
//! world shares the DOM but not the page's JavaScript globals, so the
//! getter would be undefined there. The page world is the trust boundary
//! the design accepts: page scripts can tamper with globals the wrapper
//! uses, which the wrapper's second origin check and the strict decoder
//! bound, but cannot widen the evaluation allowlist.
//!
//! Natively, `undefined` and BigInt both come back as nil without an
//! error, so the wrapper always returns its JSON string envelope and
//! anything else is malformed.

use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use block2::RcBlock;
use objc2::runtime::AnyObject;
use objc2::{MainThreadMarker, sel};
use objc2_foundation::{NSDictionary, NSError, NSObjectProtocol, NSString};
use objc2_web_kit::{WKContentWorld, WKWebView};

use crate::backend::{EvalDispatch, NativeSink};
use crate::script::{INPUT_ARGUMENT, RawEvaluation};
use crate::{EvalError, EvaluationId, PlatformError};

/// `WKErrorJavaScriptExceptionOccurred`: the wrapper itself threw, since it
/// catches the app body's exceptions.
const JAVASCRIPT_EXCEPTION: isize = 4;
/// `WKErrorJavaScriptResultTypeIsUnsupported`.
const RESULT_TYPE_UNSUPPORTED: isize = 5;
/// `WKErrorWebContentProcessTerminated`.
const PROCESS_TERMINATED: isize = 2;
/// `WKErrorWebViewInvalidated`.
const WEBVIEW_INVALIDATED: isize = 3;

/// Evaluations dispatched and not yet settled or cancelled, per view.
pub(super) type InFlight = Rc<RefCell<HashSet<EvaluationId>>>;

/// Run `dispatch` in `webview`'s main frame, page world. Settles through
/// `sink` exactly once unless cancelled first.
pub(super) fn dispatch(
    mtm: MainThreadMarker,
    webview: &WKWebView,
    sink: &NativeSink,
    in_flight: &InFlight,
    dispatch: EvalDispatch,
) {
    let evaluation = dispatch.evaluation;
    // Permission was revoked synchronously if a navigation started since
    // the session accepted this request.
    if sink.current_document() != Some(dispatch.document) {
        sink.evaluation_settled(
            evaluation,
            RawEvaluation::Failed(EvalError::NavigationChanged),
        );
        return;
    }
    // macOS 11+.
    let selector = sel!(callAsyncJavaScript:arguments:inFrame:inContentWorld:completionHandler:);
    if !webview.respondsToSelector(selector) {
        sink.evaluation_settled(
            evaluation,
            RawEvaluation::Failed(EvalError::UnsupportedRuntime),
        );
        return;
    }
    in_flight.borrow_mut().insert(evaluation);

    let key = NSString::from_str(INPUT_ARGUMENT);
    let input = NSString::from_str(&dispatch.envelope.input);
    let arguments = NSDictionary::<NSString, AnyObject>::from_slices(&[&*key], &[&*input]);
    let sink = sink.clone();
    let in_flight = Rc::clone(in_flight);
    let completion = RcBlock::new(move |result: *mut AnyObject, error: *mut NSError| {
        // Settled once; nothing after a cancel.
        if !in_flight.borrow_mut().remove(&evaluation) {
            return;
        }
        // SAFETY: WebKit passes valid objects or nil for the call's duration.
        let raw = unsafe { raw_result(result.as_ref(), error.as_ref()) };
        sink.evaluation_settled(evaluation, raw);
    });
    unsafe {
        webview.callAsyncJavaScript_arguments_inFrame_inContentWorld_completionHandler(
            &NSString::from_str(&dispatch.envelope.body),
            Some(&arguments),
            None,
            &WKContentWorld::pageWorld(mtm),
            Some(&completion),
        );
    }
}

/// What WebKit's completion means for the decoder.
fn raw_result(result: Option<&AnyObject>, error: Option<&NSError>) -> RawEvaluation {
    if let Some(error) = error {
        let domain = error.domain().to_string();
        return raw_error(&domain, error.code());
    }
    match result.and_then(|value| value.downcast_ref::<NSString>()) {
        Some(text) => RawEvaluation::Envelope(text.to_string()),
        None => RawEvaluation::NotAString,
    }
}

fn raw_error(domain: &str, code: isize) -> RawEvaluation {
    if domain != "WKErrorDomain" {
        return platform(domain, code);
    }
    match code {
        JAVASCRIPT_EXCEPTION => RawEvaluation::WrapperFailed,
        RESULT_TYPE_UNSUPPORTED => RawEvaluation::NotAString,
        PROCESS_TERMINATED => RawEvaluation::Failed(EvalError::ProcessTerminated),
        WEBVIEW_INVALIDATED => RawEvaluation::Failed(EvalError::WindowClosed),
        _ => platform(domain, code),
    }
}

/// A platform failure naming only the error's domain and code; its
/// description may quote page text.
fn platform(domain: &str, code: isize) -> RawEvaluation {
    RawEvaluation::Failed(EvalError::Platform(PlatformError::new(
        "run the script",
        format!("{domain} {code}"),
    )))
}

/// Keep a cancelled evaluation from settling; WebKit cannot stop it.
pub(super) fn cancel(in_flight: &InFlight, evaluation: EvaluationId) {
    in_flight.borrow_mut().remove(&evaluation);
}
