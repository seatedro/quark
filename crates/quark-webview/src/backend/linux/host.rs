//! One view: a decorated GTK window, the WebKitGTK view wry builds in it,
//! and the WebKit signals that drive navigation policy and lifecycle.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use webkit2gtk::{
    LoadEvent, NavigationPolicyDecision, NavigationPolicyDecisionExt, NetworkError,
    PermissionRequestExt, PolicyDecision, PolicyDecisionExt, PolicyDecisionType, PolicyError,
    ResponsePolicyDecision, ResponsePolicyDecisionExt, ScriptDialogType, SettingsExt,
    TLSErrorsPolicy, URIRequestExt, URIResponseExt, WebContextExt, WebViewExt,
    WebsiteDataManagerExt,
};
use wry::{WebViewBuilderExtUnix, WebViewExtUnix};

use super::eval::{self, Pending};
use super::{Display, parent};
use crate::backend::{EvalDispatch, NativeClose, NativeSink, OpenRequest};
use crate::policy::{BlockReason, Decision, Origin, Target};
use crate::profile::DataStore;
use crate::{
    Appearance, Capabilities, DocumentId, ErrorDetail, EvaluationId, FailureStage, NavigationError,
    NavigationId, OpenError, PlatformError, ProfileMode, Url,
};

use super::profiles::Profiles;

pub(super) struct Host {
    window: gtk::Window,
    /// Dropping it destroys the WebKit widget.
    webview: Option<wry::WebView>,
    native: webkit2gtk::WebView,
    sink: NativeSink,
    nav: Rc<RefCell<Nav>>,
    pending: Pending,
    /// Set first in teardown so signals fired while GTK destroys the
    /// widgets report nothing.
    closing: Rc<Cell<bool>>,
    /// A download refusal on the context, which a persistent profile's
    /// later views share.
    download_handler: Option<(webkit2gtk::WebContext, glib::SignalHandlerId)>,
    link: Option<parent::Link>,
}

/// Main-frame navigation state, updated synchronously by WebKit signals.
#[derive(Default)]
struct Nav {
    current: Option<NavigationId>,
    phase: Phase,
    /// The committed document and its origin, while scripts may target it.
    document: Option<(DocumentId, Origin)>,
    /// The document URL last reported, to tell same-document changes from
    /// repeats.
    url: Option<Url>,
    status: Option<u16>,
    /// WebKit refused the current attempt's certificate.
    tls_failed: bool,
    /// Why quark refused part of the current attempt, for the failure that
    /// follows a refused redirect.
    blocked: Option<BlockReason>,
    /// A load quark asked for whose start WebKit has not reported; the URI
    /// it changes in the meantime is not a same-document move.
    requested: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Provisional,
    Committed,
    Finished,
    Failed,
}

impl Host {
    pub(super) fn open(
        request: OpenRequest,
        display: Display,
        profiles: &mut Profiles,
    ) -> Result<Self, OpenError> {
        let OpenRequest {
            view: _,
            url,
            options,
            parent,
            sink,
        } = request;
        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title(&options.title);
        window.set_type_hint(gtk::gdk::WindowTypeHint::Dialog);
        // GTK grabs are per window group; a group of its own keeps one
        // modal from blocking input to another parent's modal.
        gtk::WindowGroup::new().add_window(&window);
        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        window.add(&container);
        window.realize();

        let (relationship, link) = parent::attach(&window, &parent, display);
        if relationship == crate::ParentRelationship::AppEnforced && options.require_native_parent {
            // SAFETY: an unmapped top-level this function owns.
            unsafe { window.destroy() };
            return Err(OpenError::UnsupportedParenting);
        }
        let size = fit(&window, link.as_ref(), options.size, options.min_size);
        window.set_default_size(size.0, size.1);
        let min = (
            options.min_size.0.round() as i32,
            options.min_size.1.round() as i32,
        );
        window.set_geometry_hints(
            None::<&gtk::Widget>,
            Some(&gtk::gdk::Geometry::new(
                min.0.min(size.0),
                min.1.min(size.1),
                0,
                0,
                0,
                0,
                0,
                0,
                0.0,
                0.0,
                gtk::gdk::Gravity::NorthWest,
            )),
            gtk::gdk::WindowHints::MIN_SIZE,
        );
        parent::center_on(&window, link.as_ref(), size);

        let built = match &options.view.data_store {
            DataStore::Persistent(profile) => {
                let context = match profiles.context(profile) {
                    Ok(context) => context,
                    Err(error) => {
                        // SAFETY: as above.
                        unsafe { window.destroy() };
                        return Err(OpenError::Profile(error));
                    }
                };
                configure(
                    wry::WebViewBuilder::new_with_web_context(context),
                    &options.view,
                )
                .build_gtk(&container)
            }
            _ => configure(
                wry::WebViewBuilder::new().with_incognito(true),
                &options.view,
            )
            .build_gtk(&container),
        };
        let webview = match built {
            Ok(webview) => webview,
            Err(error) => {
                // SAFETY: as above.
                unsafe { window.destroy() };
                return Err(OpenError::Platform(PlatformError::new(
                    "create the webview",
                    error.to_string(),
                )));
            }
        };
        let native = webview.webview();
        let profile = match &options.view.data_store {
            DataStore::Persistent(_) => ProfileMode::Persistent,
            _ => ProfileMode::Ephemeral,
        };
        let context = native.context();
        let manager = context
            .as_ref()
            .and_then(|context| context.website_data_manager());
        // Never let a view fall back to a shared default store.
        let isolated = match (&manager, profile) {
            (Some(manager), ProfileMode::Ephemeral) => {
                manager.is_ephemeral() && native.is_ephemeral()
            }
            (Some(manager), _) => !manager.is_ephemeral(),
            (None, _) => false,
        };
        let (Some(context), Some(manager), true) = (context, manager, isolated) else {
            drop(webview);
            // SAFETY: as above.
            unsafe { window.destroy() };
            return Err(OpenError::Platform(PlatformError::new(
                "create the webview",
                "the data store is not the one requested",
            )));
        };
        if let DataStore::Persistent(id) = &options.view.data_store {
            profiles.built(id, &context);
        }
        // WebKitGTK's default, stated so a changed default cannot let a bad
        // certificate through.
        manager.set_tls_errors_policy(TLSErrorsPolicy::Fail);
        trust_test_leaf(&context);
        harden(&native, options.view.devtools);

        let nav = Rc::new(RefCell::new(Nav::default()));
        let pending = Pending::default();
        let closing = Rc::new(Cell::new(false));
        connect(&window, &native, &sink, &nav, &pending, &closing);
        let download_handler = {
            let (sink, closing) = (sink.clone(), Rc::clone(&closing));
            // Only this view uses the context: an ephemeral one is its own,
            // and a persistent profile admits one live view.
            let id = context.connect_download_started(move |_, download| {
                webkit2gtk::DownloadExt::cancel(download);
                if !closing.get() {
                    sink.blocked(None, BlockReason::Download);
                }
            });
            Some((context, id))
        };

        sink.opened(Capabilities::new(relationship, profile, Appearance::System));
        window.show_all();
        window.present();
        native.grab_focus();
        nav.borrow_mut().requested = true;
        native.load_uri(url.as_str());
        Ok(Self {
            window,
            webview: Some(webview),
            native,
            sink,
            nav,
            pending,
            closing,
            download_handler,
            link,
        })
    }

    pub(super) fn evaluate(&self, dispatch: EvalDispatch) {
        // The document counts only while the engine still shows its origin.
        let shown = self
            .native
            .uri()
            .and_then(|uri| Url::parse(&uri).ok())
            .and_then(|url| Origin::of(&url).ok());
        let document = self
            .nav
            .borrow()
            .document
            .as_ref()
            .filter(|(_, origin)| shown.as_ref() == Some(origin))
            .map(|(document, _)| *document);
        eval::dispatch(&self.native, &self.sink, &self.pending, document, dispatch);
    }

    pub(super) fn cancel(&self, evaluation: EvaluationId) {
        self.pending.cancel(evaluation);
    }

    pub(super) fn present(&self) {
        self.window.present();
    }

    /// Destroy the view, then its window, then report once GLib has run
    /// what the teardown queued.
    pub(super) fn destroy(mut self, owed: &Rc<Cell<usize>>) {
        self.closing.set(true);
        self.pending.cancel_all();
        let had_focus = self.window.is_active();
        if let Some((context, id)) = self.download_handler.take() {
            context.disconnect(id);
        }
        drop(self.webview.take());
        // SAFETY: the top-level this host created; nothing else holds it.
        unsafe { self.window.destroy() };
        // Only hand focus back when the modal had it, so closing does not
        // pull the parent over whatever the user moved to.
        if had_focus && let Some(link) = &self.link {
            link.focus_parent();
        }
        drop(self.link.take());
        owed.set(owed.get() + 1);
        let owed = Rc::clone(owed);
        let sink = self.sink.clone();
        glib::idle_add_local_once(move || {
            owed.set(owed.get() - 1);
            sink.destroyed();
        });
    }
}

/// Wry options for `view`. Wry's portable navigation, load, title, and
/// new-window handlers stay unset: quark connects the WebKit signals.
fn configure<'a>(
    builder: wry::WebViewBuilder<'a>,
    view: &crate::WebViewOptions,
) -> wry::WebViewBuilder<'a> {
    let builder = builder
        .with_devtools(view.devtools)
        .with_clipboard(false)
        .with_back_forward_navigation_gestures(false)
        .with_hotkeys_zoom(false)
        .with_visible(true)
        .with_focused(true);
    match &view.user_agent {
        Some(user_agent) => builder.with_user_agent(user_agent),
        None => builder,
    }
}

/// Settings beyond wry's: no console echo of page messages (they can carry
/// credentials), no script-opened windows, no file-URL access.
fn harden(native: &webkit2gtk::WebView, devtools: bool) {
    if let Some(settings) = WebViewExt::settings(native) {
        settings.set_enable_write_console_messages_to_stdout(false);
        settings.set_javascript_can_open_windows_automatically(false);
        settings.set_allow_file_access_from_file_urls(false);
        settings.set_allow_universal_access_from_file_urls(false);
        settings.set_enable_developer_extras(devtools);
    }
    // Wry turns preedit off for every view; WebKit owns text input in this
    // window, so restore its default of drawing the composition inline.
    if let Some(input) = native.input_method_context() {
        webkit2gtk::InputMethodContextExt::set_enable_preedit(&input, true);
    }
}

/// The window size for `size` logical points, clamped to the work area of
/// the parent's monitor (or the primary one).
fn fit(
    window: &gtk::Window,
    link: Option<&parent::Link>,
    size: (f32, f32),
    min: (f32, f32),
) -> (i32, i32) {
    let display = WidgetExt::display(window);
    let monitor = match link {
        Some(parent::Link::X11(parent)) => display.monitor_at_window(parent),
        _ => None,
    }
    .or_else(|| display.primary_monitor())
    .or_else(|| display.monitor(0));
    let wanted = (
        size.0.max(min.0).round() as i32,
        size.1.max(min.1).round() as i32,
    );
    match monitor.map(|monitor| monitor.workarea()) {
        Some(area) if area.width() > 0 && area.height() > 0 => {
            (wanted.0.min(area.width()), wanted.1.min(area.height()))
        }
        _ => wanted,
    }
}

fn connect(
    window: &gtk::Window,
    native: &webkit2gtk::WebView,
    sink: &NativeSink,
    nav: &Rc<RefCell<Nav>>,
    pending: &Pending,
    closing: &Rc<Cell<bool>>,
) {
    {
        let (sink, nav, closing) = (sink.clone(), Rc::clone(nav), Rc::clone(closing));
        native.connect_decide_policy(move |webview, decision, kind| {
            // Every decision is completed here, before returning: WebKit
            // would otherwise apply its own default.
            if closing.get() {
                decision.ignore();
            } else {
                decide(webview, decision, kind, &sink, &nav);
            }
            true
        });
    }
    {
        let (sink, nav, pending, closing) = (
            sink.clone(),
            Rc::clone(nav),
            pending.clone(),
            Rc::clone(closing),
        );
        native.connect_load_changed(move |webview, event| {
            if !closing.get() {
                load_changed(webview, event, &sink, &nav, &pending);
            }
        });
    }
    {
        let (nav, closing) = (Rc::clone(nav), Rc::clone(closing));
        native.connect_load_failed_with_tls_errors(move |_, _, _, _| {
            if !closing.get() {
                nav.borrow_mut().tls_failed = true;
            }
            // load-failed follows and reports it.
            false
        });
    }
    {
        let (sink, nav, closing) = (sink.clone(), Rc::clone(nav), Rc::clone(closing));
        native.connect_load_failed(move |_, event, _, error| {
            if !closing.get() {
                load_failed(event, error, &sink, &nav);
            }
            // Handled: WebKit's own error page would commit a document at
            // the failed URL.
            true
        });
    }
    {
        let (sink, nav, closing) = (sink.clone(), Rc::clone(nav), Rc::clone(closing));
        native.connect_uri_notify(move |webview| {
            if closing.get() {
                return;
            }
            let mut nav = nav.borrow_mut();
            if nav.requested || !matches!(nav.phase, Phase::Committed | Phase::Finished) {
                return;
            }
            let Some(url) = webview.uri().and_then(|uri| Url::parse(&uri).ok()) else {
                return;
            };
            if nav.url.as_ref() != Some(&url) {
                nav.url = Some(url.clone());
                drop(nav);
                sink.location_changed(&url);
            }
        });
    }
    {
        let (sink, closing) = (sink.clone(), Rc::clone(closing));
        native.connect_title_notify(move |webview| {
            if !closing.get() {
                sink.title_changed(webview.title().as_deref().unwrap_or(""));
            }
        });
    }
    // A backstop: new-window actions are refused in decide-policy already.
    native.connect_create(|_, _| None);
    {
        let (sink, pending, closing) = (sink.clone(), pending.clone(), Rc::clone(closing));
        native.connect_web_process_terminated(move |_, _| {
            if !closing.get() {
                pending.cancel_all();
                sink.closed(NativeClose::ProcessTerminated);
            }
        });
    }
    {
        // `window.close()` from the page; wry destroys the widget too.
        let (sink, closing) = (sink.clone(), Rc::clone(closing));
        native.connect_close(move |_| {
            if !closing.get() {
                sink.closed(NativeClose::User);
            }
        });
    }
    // Camera, microphone, location, notifications, pointer lock, and every
    // other permission: refused without a prompt.
    native.connect_permission_request(|_, request| {
        request.deny();
        true
    });
    // No page-driven dialogs. Leaving a page stays possible; confirm and
    // prompt answer "no".
    native.connect_script_dialog(|_, dialog| {
        let dialog = dialog.clone();
        if dialog.dialog_type() == ScriptDialogType::BeforeUnloadConfirm {
            dialog.confirm_set_confirmed(true);
        }
        true
    });
    native.connect_run_file_chooser(|_, request| {
        webkit2gtk::FileChooserRequestExt::cancel(request);
        true
    });
    {
        let (sink, closing) = (sink.clone(), Rc::clone(closing));
        window.connect_delete_event(move |_, _| {
            if !closing.get() {
                sink.closed(NativeClose::User);
            }
            // The session answers with `close`, which destroys the window.
            glib::Propagation::Stop
        });
    }
}

fn decide(
    webview: &webkit2gtk::WebView,
    decision: &PolicyDecision,
    kind: PolicyDecisionType,
    sink: &NativeSink,
    nav: &Rc<RefCell<Nav>>,
) {
    match kind {
        PolicyDecisionType::NavigationAction | PolicyDecisionType::NewWindowAction => {
            let uri = decision
                .downcast_ref::<NavigationPolicyDecision>()
                .and_then(|decision| decision.navigation_action())
                .and_then(|action| action.request())
                .and_then(|request| request.uri());
            let Some(uri) = uri else {
                decision.ignore();
                return;
            };
            // WebKitGTK does not say which frame a navigation action is
            // for. The empty documents frames start from are allowed as
            // frame loads; a main frame that commits one is refused at
            // commit.
            let target = if kind == PolicyDecisionType::NewWindowAction {
                Target::NewWindow
            } else if matches!(uri.as_str(), "about:blank" | "about:srcdoc") {
                Target::Subframe
            } else {
                Target::MainFrame
            };
            match sink.decide(&uri, target) {
                Decision::Allow => decision.use_(),
                Decision::LoadInCurrent(url) => {
                    decision.ignore();
                    // Not from inside WebKit's policy callback.
                    let (webview, nav) = (webview.clone(), Rc::clone(nav));
                    glib::idle_add_local_once(move || {
                        nav.borrow_mut().requested = true;
                        webview.load_uri(url.as_str());
                    });
                }
                Decision::Block(reason) => {
                    nav.borrow_mut().blocked = Some(reason);
                    decision.ignore();
                }
            }
        }
        PolicyDecisionType::Response => {
            let Some(response) = decision.downcast_ref::<ResponsePolicyDecision>() else {
                decision.ignore();
                return;
            };
            let main = response.is_main_frame_main_resource();
            let uri = response.request().and_then(|request| request.uri());
            let Some(uri) = uri else {
                decision.ignore();
                return;
            };
            // Something WebKit cannot show would become a download.
            if !response.is_mime_type_supported() {
                nav.borrow_mut().blocked = Some(BlockReason::Download);
                sink.blocked(Url::parse(&uri).ok(), BlockReason::Download);
                decision.ignore();
                return;
            }
            let target = if main {
                Target::MainFrame
            } else {
                Target::Subframe
            };
            match sink.decide(&uri, target) {
                Decision::Allow => {
                    if main {
                        nav.borrow_mut().status = response
                            .response()
                            .map(|response| response.status_code())
                            .and_then(|code| u16::try_from(code).ok());
                    }
                    decision.use_();
                }
                Decision::LoadInCurrent(_) | Decision::Block(_) => {
                    nav.borrow_mut().blocked = Some(BlockReason::OriginNotAllowed);
                    decision.ignore();
                }
            }
        }
        _ => decision.ignore(),
    }
}

fn load_changed(
    webview: &webkit2gtk::WebView,
    event: LoadEvent,
    sink: &NativeSink,
    nav: &Rc<RefCell<Nav>>,
    pending: &Pending,
) {
    let url = webview
        .uri()
        .and_then(|uri| Url::parse(&uri).ok())
        .unwrap_or_else(|| Url::parse("about:blank").expect("a valid URL"));
    match event {
        LoadEvent::Started => {
            // Revoke first: nothing may run in the old document from here.
            pending.cancel_all();
            let navigation = sink.navigation_started(&url);
            *nav.borrow_mut() = Nav {
                current: Some(navigation),
                phase: Phase::Provisional,
                ..Nav::default()
            };
        }
        LoadEvent::Redirected => {
            if let Some(navigation) = nav.borrow().current {
                sink.navigation_redirected(navigation, &url);
            }
        }
        LoadEvent::Committed => {
            let Some(navigation) = nav.borrow().current else {
                return;
            };
            match sink.navigation_committed(navigation, &url) {
                Some(document) => {
                    let mut state = nav.borrow_mut();
                    state.phase = Phase::Committed;
                    state.document = Origin::of(&url).ok().map(|origin| (document, origin));
                    state.url = Some(url);
                }
                None => {
                    nav.borrow_mut().phase = Phase::Failed;
                    webview.stop_loading();
                }
            }
        }
        LoadEvent::Finished => {
            let mut state = nav.borrow_mut();
            // FINISHED also follows a failure; only a committed load
            // finishes successfully.
            if state.phase != Phase::Committed {
                return;
            }
            state.phase = Phase::Finished;
            let (navigation, status) = (state.current, state.status);
            drop(state);
            if let Some(navigation) = navigation {
                sink.load_finished(navigation, status);
            }
        }
        _ => {}
    }
}

fn load_failed(event: LoadEvent, error: &glib::Error, sink: &NativeSink, nav: &Rc<RefCell<Nav>>) {
    let mut state = nav.borrow_mut();
    let Some(navigation) = state.current else {
        return;
    };
    let stage = if event == LoadEvent::Committed || state.phase == Phase::Committed {
        FailureStage::Committed
    } else {
        FailureStage::Provisional
    };
    let blocked = state.blocked.take();
    let reason = if state.tls_failed {
        NavigationError::Tls
    } else if let (Some(reason), Some(PolicyError::FrameLoadInterruptedByPolicyChange)) =
        (blocked, error.kind::<PolicyError>())
    {
        NavigationError::Policy(reason)
    } else if error.matches(NetworkError::Cancelled) || error.matches(gio::IOErrorEnum::Cancelled) {
        NavigationError::Cancelled
    } else if error.kind::<NetworkError>().is_some() {
        NavigationError::Transport
    } else {
        // The domain only; WebKit's messages can carry the URL.
        NavigationError::Other(ErrorDetail::new(error.domain().as_str().to_owned()))
    };
    state.phase = Phase::Failed;
    state.document = None;
    drop(state);
    sink.navigation_failed(navigation, stage, reason);
}

/// Accept the smoke tests' fixture certificate for exactly its loopback
/// hosts, and nothing else. A no-op unless quark-webview's `test-trust`
/// feature set one.
fn trust_test_leaf(context: &webkit2gtk::WebContext) {
    let Some((leaf, hosts)) = crate::test_trust() else {
        return;
    };
    let pem = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        glib::base64_encode(leaf)
    );
    let Ok(certificate) = gio::TlsCertificate::from_pem(&pem) else {
        return;
    };
    for host in hosts {
        context.allow_tls_certificate_for_host(&certificate, host);
    }
}
