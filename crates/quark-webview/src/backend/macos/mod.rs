//! The WKWebView backend: wry builds the view inside a sheet on the parent
//! window; quark's wry patch adds the navigation hooks this needs.
//!
//! Everything runs on the main thread, inside AppKit's run loop, which
//! winit already drives; WebKit's callbacks arrive there and only update
//! the sink, so the backend never needs servicing.

mod eval;
mod host;
mod navigation;
mod store;

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::NSAutoresizingMaskOptions;
use objc2_web_kit::WKWebViewConfiguration;
use wry::{
    NewWindowResponse, PermissionResponse, Rect, WebViewBuilder, WebViewBuilderExtDarwin,
    WebViewBuilderExtMacos, WebViewExtMacOS, dpi,
};

use super::{Backend, ClearRequest, EvalDispatch, NativeClose, NativeSink, OpenRequest};
use crate::policy::{Decision, Target};
use crate::{
    Capabilities, EvaluationId, OpenError, ParentRelationship, PlatformError, WebViewHandle,
};
use host::Sheet;
use navigation::{Hooks, NavState, UrlObserver};

pub(super) fn backend() -> Option<Box<dyn Backend>> {
    let mtm = MainThreadMarker::new()?;
    Some(Box::new(MacBackend {
        mtm,
        views: HashMap::new(),
    }))
}

struct MacBackend {
    mtm: MainThreadMarker,
    views: HashMap<WebViewHandle, View>,
}

/// One open view.
struct View {
    sink: NativeSink,
    url_observer: Retained<UrlObserver>,
    webview: wry::WebView,
    sheet: Sheet,
    in_flight: eval::InFlight,
}

impl View {
    /// Tear down the view and its sheet; report `destroyed` unless the open
    /// itself is failing.
    fn destroy(self, report: bool) {
        let sink = self.sink.clone();
        // Drain what teardown autoreleases now, so the web view and its
        // data store are released before `destroyed` frees the profile.
        objc2::rc::autoreleasepool(|_| self.teardown());
        if report {
            sink.destroyed();
        }
    }

    fn teardown(self) {
        let webview = self.webview.webview();
        unsafe { webview.stopLoading() };
        self.url_observer.detach();
        self.sheet.dismiss();
        // wry removes the view from the sheet and releases its delegates.
        drop(self.webview);
        drop(webview);
        self.sheet.window.close();
        // Unsettled evaluations never complete now; the session already
        // failed them when it closed the view.
        self.in_flight.borrow_mut().clear();
    }
}

impl MacBackend {
    fn build(&mut self, request: OpenRequest) -> Result<View, OpenError> {
        let mtm = self.mtm;
        let OpenRequest {
            view: _,
            url,
            options,
            parent,
            sink,
        } = request;
        let parent = host::parent_window(parent.window).ok_or_else(|| {
            OpenError::Platform(PlatformError::new(
                "find the parent window",
                "the parent handle has no AppKit window",
            ))
        })?;
        let view_options = &options.view;
        let (data_store, profile) =
            store::data_store(mtm, &view_options.data_store).map_err(OpenError::Profile)?;
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe { configuration.setWebsiteDataStore(&data_store) };
        // Let every `window.open` reach the UI delegate, which denies it and
        // reports it. With WebKit's default, one without a user gesture is
        // dropped silently, and the app never learns the flow wanted a popup.
        unsafe {
            configuration
                .preferences()
                .setJavaScriptCanOpenWindowsAutomatically(true)
        };

        let user_sink = sink.clone();
        let sheet = Sheet::new(
            mtm,
            &parent,
            &options.title,
            options.size,
            options.min_size,
            view_options.appearance,
            Box::new(move || user_sink.closed(NativeClose::User)),
        );

        let state = Rc::new(RefCell::new(NavState::default()));
        let hooks = Hooks {
            sink: sink.clone(),
            state: Rc::clone(&state),
        };
        let popup_sink = sink.clone();
        let title_sink = sink.clone();
        let terminate_sink = sink.clone();
        let frame = sheet.content.frame();
        let mut builder = WebViewBuilder::new()
            .with_webview_configuration(configuration)
            .with_navigation_hooks(Box::new(hooks))
            .with_new_window_req_handler(move |url, features| {
                // `window.open` reaches here without an action decision.
                if let Decision::LoadInCurrent(url) = popup_sink.decide(&url, Target::NewWindow) {
                    navigation::load_later(&features.opener.webview, &url);
                }
                NewWindowResponse::Deny
            })
            .with_permission_handler(|_| PermissionResponse::Deny)
            .with_document_title_changed_handler(move |title| title_sink.title_changed(&title))
            .with_on_web_content_process_terminate_handler(move || {
                terminate_sink.closed(NativeClose::ProcessTerminated)
            })
            .with_devtools(view_options.devtools)
            .with_accept_first_mouse(true)
            .with_bounds(Rect {
                position: dpi::LogicalPosition::new(0.0, 0.0).into(),
                size: dpi::LogicalSize::new(frame.size.width, frame.size.height).into(),
            });
        if let Some(agent) = &view_options.user_agent {
            builder = builder.with_user_agent(agent);
        }
        let webview = builder
            .build_as_child(&sheet.content_handle())
            .map_err(|error| {
                OpenError::Platform(PlatformError::new(
                    "create the WebKit view",
                    error.to_string(),
                ))
            })?;
        let native = webview.webview();
        // wry pins a child webview's size; fill the sheet as it resizes.
        native.setAutoresizingMask(
            NSAutoresizingMaskOptions::ViewWidthSizable
                | NSAutoresizingMaskOptions::ViewHeightSizable,
        );
        let url_observer =
            UrlObserver::new(Retained::into_super(native.clone()), sink.clone(), state);

        let view = View {
            sink,
            url_observer,
            webview,
            sheet,
            in_flight: Rc::default(),
        };
        view.sheet.present(&parent);
        view.sheet.window.makeFirstResponder(Some(&native));
        // Handlers and the data store are in place; only now load.
        if let Err(error) = view.webview.load_url(url.as_str()) {
            view.destroy(false);
            return Err(OpenError::Platform(PlatformError::new(
                "load the initial URL",
                error.to_string(),
            )));
        }
        view.sink.opened(Capabilities::new(
            ParentRelationship::Native,
            profile,
            view_options.appearance,
        ));
        Ok(view)
    }
}

impl Backend for MacBackend {
    fn open(&mut self, request: OpenRequest) -> Result<(), OpenError> {
        let handle = request.view;
        let view = self.build(request)?;
        self.views.insert(handle, view);
        Ok(())
    }

    fn evaluate(&mut self, view: WebViewHandle, dispatch: EvalDispatch) {
        let Some(view) = self.views.get(&view) else {
            // Closed: the session failed the evaluation already.
            return;
        };
        let webview = view.webview.webview();
        eval::dispatch(self.mtm, &webview, &view.sink, &view.in_flight, dispatch);
    }

    fn cancel(&mut self, view: WebViewHandle, evaluation: EvaluationId) {
        if let Some(view) = self.views.get(&view) {
            eval::cancel(&view.in_flight, evaluation);
        }
    }

    fn close(&mut self, view: WebViewHandle) {
        if let Some(view) = self.views.remove(&view) {
            view.destroy(true);
        }
    }

    fn focus(&mut self, view: WebViewHandle) {
        if let Some(view) = self.views.get(&view) {
            view.sheet.window.makeKeyAndOrderFront(None);
        }
    }

    fn clear_profile(&mut self, request: ClearRequest) {
        store::clear(self.mtm, &request.profile, request.sink);
    }

    fn shutdown(&mut self) {
        for (_, view) in self.views.drain() {
            view.destroy(true);
        }
    }
}
