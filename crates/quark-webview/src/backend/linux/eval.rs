//! Guarded script evaluation through
//! `webkit_web_view_call_async_javascript_function` (WebKitGTK 2.40+). It
//! runs the wrapper as an async function body in the main frame of the
//! page's own world (`world_name = NULL`, so page globals and getters are
//! visible), awaits a returned Promise, and passes the wrapper's input as a
//! native string argument, never as source text.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gio::prelude::CancellableExt;
use gtk::glib;
use gtk::glib::ToVariant;
use javascriptcore::ValueExt;
use webkit2gtk::WebViewExt;

use crate::backend::{EvalDispatch, NativeSink};
use crate::script::{INPUT_ARGUMENT, RawEvaluation};
use crate::{DocumentId, EvalError, EvaluationId, PlatformError};

/// Native calls still running, cancelled when the document they target
/// goes away so WebKit drops their callbacks.
#[derive(Clone, Default)]
pub(super) struct Pending(Rc<RefCell<HashMap<EvaluationId, gio::Cancellable>>>);

impl Pending {
    fn insert(&self, evaluation: EvaluationId, cancellable: gio::Cancellable) {
        self.0.borrow_mut().insert(evaluation, cancellable);
    }

    fn remove(&self, evaluation: EvaluationId) -> bool {
        self.0.borrow_mut().remove(&evaluation).is_some()
    }

    pub(super) fn cancel(&self, evaluation: EvaluationId) {
        if let Some(cancellable) = self.0.borrow_mut().remove(&evaluation) {
            cancellable.cancel();
        }
    }

    /// Cancel every call, for a navigation or teardown. Their callbacks
    /// still run once, with a cancellation the session no longer waits for.
    pub(super) fn cancel_all(&self) {
        let pending: Vec<_> = self
            .0
            .borrow_mut()
            .drain()
            .map(|(_, cancellable)| cancellable)
            .collect();
        for cancellable in pending {
            cancellable.cancel();
        }
    }
}

/// Run `dispatch` in `webview`'s main frame if `document` is still the
/// committed one there.
pub(super) fn dispatch(
    webview: &webkit2gtk::WebView,
    sink: &NativeSink,
    pending: &Pending,
    document: Option<DocumentId>,
    dispatch: EvalDispatch,
) {
    let EvalDispatch {
        evaluation,
        document: target,
        envelope,
    } = dispatch;
    // The session checked at submission; a navigation may have started
    // since, and load-changed has already revoked the document if so.
    if sink.current_document() != Some(target) || document != Some(target) {
        sink.evaluation_settled(
            evaluation,
            RawEvaluation::Failed(EvalError::NavigationChanged),
        );
        return;
    }
    let arguments = glib::VariantDict::new(None);
    arguments.insert_value(INPUT_ARGUMENT, &envelope.input.to_variant());
    let cancellable = gio::Cancellable::new();
    pending.insert(evaluation, cancellable.clone());
    let (sink, pending) = (sink.clone(), pending.clone());
    webview.call_async_javascript_function(
        &envelope.body,
        Some(&arguments.end()),
        None,
        None,
        Some(&cancellable),
        move |result| {
            let live = pending.remove(evaluation);
            let raw = if !live || sink.current_document() != Some(target) {
                // Cancelled, or the document changed while it ran: whatever
                // came back belongs to a document the caller did not name.
                RawEvaluation::Failed(EvalError::NavigationChanged)
            } else {
                raw(result)
            };
            sink.evaluation_settled(evaluation, raw);
        },
    );
}

/// Map WebKit's answer. The wrapper catches the app body's exceptions
/// itself, so a JavaScript error here means the wrapper broke.
fn raw(result: Result<javascriptcore::Value, glib::Error>) -> RawEvaluation {
    match result {
        Ok(value) if value.is_string() => RawEvaluation::Envelope(value.to_str().into()),
        Ok(_) => RawEvaluation::NotAString,
        Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {
            RawEvaluation::Failed(EvalError::Cancelled)
        }
        Err(error) if error.kind::<webkit2gtk::JavascriptError>().is_some() => {
            RawEvaluation::WrapperFailed
        }
        // Only the domain: WebKit's messages can quote page data.
        Err(error) => RawEvaluation::Failed(EvalError::Platform(PlatformError::new(
            "evaluate a script",
            error.domain().as_str().to_owned(),
        ))),
    }
}
