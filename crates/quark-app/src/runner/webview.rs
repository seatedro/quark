//! The runner's webview registry: the `quark-webview` service, which modal
//! belongs to which parent window, and the parent lock.
//!
//! The service is driven only from runner service points (`about_to_wait`,
//! wake delivery, and after each app callback), never from inside a
//! callback. Its events go to the app with the context bound to the
//! view's parent while the parent is open.

use std::time::Duration;

use quark_webview::service::{NativeParent, Service};
use quark_webview::{WebCloseReason, WebViewEvent, WebViewHandle};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};

use super::*;

pub(crate) struct WebViewRunner {
    service: Service,
    /// Modal views not yet closed, with their parents. One per parent.
    modals: Vec<(WebViewHandle, WindowHandle)>,
}

/// A webview event and the window it belongs to.
pub(super) type Routed = (Option<WindowHandle>, WebViewEvent);

impl WebViewRunner {
    pub(super) fn new(waker: &Waker) -> Self {
        // `Waker` holds a winit proxy, which is Send but not Sync everywhere.
        let waker = Mutex::new(waker.clone());
        Self {
            service: Service::new(move || {
                waker
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner())
                    .wake();
            }),
            modals: Vec::new(),
        }
    }

    pub(super) fn service_mut(&mut self) -> &mut Service {
        &mut self.service
    }

    /// The modal open over `parent`, if any.
    pub(super) fn modal_for(&self, parent: WindowHandle) -> Option<WebViewHandle> {
        self.modals
            .iter()
            .find(|&&(_, window)| window == parent)
            .map(|&(view, _)| view)
    }

    /// Whether `window` is a parent whose input the modal lock blocks.
    pub(super) fn blocks(&self, window: WindowHandle) -> bool {
        self.modal_for(window).is_some()
    }

    pub(super) fn add_modal(&mut self, view: WebViewHandle, parent: WindowHandle) {
        self.modals.push((view, parent));
    }

    /// Bring `parent`'s modal forward, for a click on the blocked parent.
    pub(super) fn focus_modal(&mut self, parent: WindowHandle) {
        if let Some(view) = self.modal_for(parent) {
            self.service.focus(view);
            self.service.flush();
        }
    }

    /// Run queued commands on the backend; after every app callback.
    pub(super) fn flush(&mut self) {
        self.service.flush();
    }

    /// Pump the backend, then collect its events with their parents.
    pub(super) fn service(&mut self, now: Duration) -> Vec<Routed> {
        self.service.service(now);
        self.take_events()
    }

    fn take_events(&mut self) -> Vec<Routed> {
        let events: Vec<WebViewEvent> = self.service.events().collect();
        events
            .into_iter()
            .map(|event| {
                let view = event.view();
                let parent = self
                    .modals
                    .iter()
                    .find(|&&(modal, _)| modal == view)
                    .map(|&(_, parent)| parent);
                if matches!(event, WebViewEvent::Closed { .. }) {
                    self.modals.retain(|&(modal, _)| modal != view);
                }
                (parent, event)
            })
            .collect()
    }

    /// When the runner must service next, on its monotonic clock.
    pub(super) fn next_deadline(&self, now: Duration) -> Option<Duration> {
        self.service.next_deadline(now)
    }

    /// `parent` is closing: close its modal first, while the parent's
    /// native window still exists.
    pub(super) fn close_children(&mut self, parent: WindowHandle) -> Vec<Routed> {
        let Some(view) = self.modal_for(parent) else {
            return Vec::new();
        };
        self.service.close(view, WebCloseReason::ParentClosed);
        self.service.flush();
        self.take_events()
    }

    /// The app is exiting: close every view and release the engine.
    pub(super) fn shutdown(&mut self) -> Vec<Routed> {
        self.service.shutdown();
        self.take_events()
    }
}

/// The native handles of an open window, for parenting a webview to it.
pub(super) fn native_parent(window: &Window) -> Option<NativeParent> {
    Some(NativeParent {
        window: window.window_handle().ok()?.as_raw(),
        display: window.display_handle().ok()?.as_raw(),
    })
}

/// Webviews for the context's window, from [`EventContext::webviews`].
/// Answers arrive as [`AppEvent::WebView`].
pub struct WebViews<'c, 'w> {
    pub(super) cx: &'c mut EventContext<'w>,
}

impl WebViews<'_, '_> {
    /// Open `url` in a modal window over the context's window. The options
    /// are checked now and the handle reserved; the engine is built once
    /// the current callback returns. `Opened` or `Closed` with
    /// [`WebCloseReason::OpenFailed`] follows.
    pub fn open(
        &mut self,
        url: quark_webview::Url,
        options: quark_webview::WebWindowOptions,
    ) -> Result<WebViewHandle, quark_webview::OpenError> {
        let parent = self
            .cx
            .window
            .ok_or(quark_webview::OpenError::ParentRequired)?;
        self.open_with_parent(parent, url, options)
    }

    /// [`Self::open`] over `parent` instead of the context's window.
    pub fn open_with_parent(
        &mut self,
        parent: WindowHandle,
        url: quark_webview::Url,
        options: quark_webview::WebWindowOptions,
    ) -> Result<WebViewHandle, quark_webview::OpenError> {
        use quark_webview::OpenError;

        let native = self
            .cx
            .windows
            .get(parent)
            .and_then(WindowEntry::open)
            .and_then(|state| native_parent(&state.window))
            .ok_or(OpenError::ParentRequired)?;
        if self.cx.webviews.modal_for(parent).is_some() {
            return Err(OpenError::ModalAlreadyOpen);
        }
        let view = self.cx.webviews.service_mut().open(url, options, native)?;
        self.cx.webviews.add_modal(view, parent);
        Ok(view)
    }

    /// Run `script` in `view` if `guard` still matches its committed
    /// document. The future resolves on any executor; dropping it cancels.
    pub fn evaluate_script(
        &mut self,
        view: WebViewHandle,
        script: quark_webview::AsyncScript,
        guard: quark_webview::OriginGuard,
    ) -> Result<quark_webview::Evaluation, quark_webview::EvalError> {
        let now = self.cx.elapsed;
        self.cx
            .webviews
            .service_mut()
            .evaluate_script(view, &script, &guard, now)
    }

    /// [`Self::evaluate_script`] with the result delivered as
    /// [`WebViewEvent::EvaluationFinished`] instead of a future.
    pub fn evaluate_script_event(
        &mut self,
        view: WebViewHandle,
        script: quark_webview::AsyncScript,
        guard: quark_webview::OriginGuard,
    ) -> Result<quark_webview::EvaluationId, quark_webview::EvalError> {
        let now = self.cx.elapsed;
        self.cx
            .webviews
            .service_mut()
            .evaluate_script_event(view, &script, &guard, now)
    }

    /// Stop waiting for an evaluation; it ends with
    /// [`quark_webview::EvalError::Cancelled`]. JavaScript already running
    /// is not interrupted.
    pub fn cancel_evaluation(&mut self, evaluation: quark_webview::EvaluationId) {
        self.cx.webviews.service_mut().cancel_evaluation(evaluation);
    }

    /// Close `view` with [`WebCloseReason::Program`]. Idempotent; stale
    /// handles are ignored.
    pub fn close(&mut self, view: WebViewHandle) {
        self.cx
            .webviews
            .service_mut()
            .close(view, WebCloseReason::Program);
    }

    /// Delete a persistent profile's stored data. Fails with
    /// [`quark_webview::ProfileError::InUse`] while a view uses it.
    pub fn clear_profile(
        &mut self,
        profile: &quark_webview::ProfileId,
    ) -> Result<quark_webview::ProfileClear, quark_webview::ProfileError> {
        self.cx.webviews.service_mut().clear_profile(profile)
    }
}
