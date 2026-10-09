//! Linux (WebKitGTK) checks on a real X11 display, run as their own process
//! by `webview_smoke` (one winit event loop per process). Each step waits
//! for the event the previous one should produce; a failure, or no
//! progress within two minutes, exits non-zero.
//!
//! - The modal is a transient child of the parent's XID, and says so.
//! - A Promise-returning getter's value comes back through the async
//!   function API.
//! - A certificate failure stays a failure: WebKit's error page never
//!   commits as a document at the failed URL.
//! - A redirect to an origin outside the allowlist is refused before the
//!   request leaves, and the failure names the policy.
//! - A persistent profile keeps its storage across views until cleared.
//!
//! Needs an X11 display with a window manager (CI: Xvfb plus openbox) and
//! the `webview` feature; prints a skip line without a display.

use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use quark::scene::Scene;
use quark_app::platform::webview::{
    AsyncScript, BlockReason, DataStore, DocumentId, FailureStage, NavigationError, OpenError,
    Origin, OriginGuard, ParentRelationship, ProfileClear, ProfileId, Url, WebCloseReason,
    WebViewEvent, WebViewHandle, WebViewOptions, WebWindowOptions,
};
use quark_app::{App, AppEvent, EventContext, FrameContext, WindowOptions};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::fixture::{Fixture, Site, leaf_der};

pub mod evidence;

const TITLE: &str = "linux smoke sign in";

pub fn run() -> ExitCode {
    if std::env::var_os("DISPLAY").is_none() {
        println!("webview_smoke linux: no X11 display; skipped");
        return ExitCode::SUCCESS;
    }
    // The persistent profile lives under a data directory of the run's own.
    let data = std::env::temp_dir().join(format!("quark-webview-smoke-{}", std::process::id()));
    // SAFETY: no other thread exists yet.
    unsafe { std::env::set_var("XDG_DATA_HOME", &data) };
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(120));
        eprintln!("webview_smoke linux: no progress within 120 s");
        std::process::exit(1);
    });
    let fixture = Fixture::start().expect("fixture binds its sites");
    quark_webview::testing::trust_leaf(leaf_der(), &["127.0.0.1", "localhost"]);
    let result = quark_app::run(
        Smoke {
            fixture,
            step: Step::Start,
            view: None,
            failures: Vec::new(),
            clear: None,
            quiet: Arc::new(AtomicBool::new(false)),
        },
        WindowOptions {
            title: "linux webview smoke".into(),
            size: (480.0, 320.0),
            ..WindowOptions::default()
        },
    );
    let _ = std::fs::remove_dir_all(&data);
    if let Err(error) = result {
        eprintln!("webview_smoke linux: could not run: {error}");
        return ExitCode::FAILURE;
    }
    if PASSED.load(Ordering::Acquire) {
        println!("webview_smoke linux: passed");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

static PASSED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Start,
    /// The getter page is loading in a parented modal.
    Getter,
    /// Waiting for the getter's Promise.
    Token,
    /// A certificate failure; then a quiet spell in which nothing commits.
    BadCert,
    /// A redirect to a foreign origin.
    Redirect,
    /// A persistent profile: write, read back, clear, read empty.
    ProfileWrite,
    ProfileRead,
    Clearing,
    ProfileEmpty,
    Done,
}

struct Smoke {
    fixture: Fixture,
    step: Step,
    view: Option<WebViewHandle>,
    failures: Vec<String>,
    clear: Option<ProfileClear>,
    /// Set by a timer thread once the certificate failure had time to
    /// produce an error page.
    quiet: Arc<AtomicBool>,
}

impl Smoke {
    fn check<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: T, want: T) {
        if got == want {
            println!("webview_smoke linux: ok: {what}");
        } else {
            self.failures
                .push(format!("{what}: got {got:?}, want {want:?}"));
        }
    }

    fn origin(&self, site: Site) -> Origin {
        Origin::parse(&self.fixture.origin(site)).expect("fixture origins parse")
    }

    fn open(
        &mut self,
        cx: &mut EventContext,
        url: String,
        options: WebViewOptions,
        step: Step,
    ) -> Result<(), OpenError> {
        let url = Url::parse(&url).expect("fixture URLs parse");
        let view = cx
            .webviews()
            .open(url, WebWindowOptions::new(options).title(TITLE))?;
        self.view = Some(view);
        self.step = step;
        Ok(())
    }

    fn profile() -> ProfileId {
        ProfileId::new("quark-webview-smoke", "linux").expect("a valid profile id")
    }

    fn evaluate(
        &mut self,
        cx: &mut EventContext,
        view: WebViewHandle,
        document: DocumentId,
        body: &'static str,
    ) {
        let guard = OriginGuard::new(self.origin(Site::App), document);
        if let Err(error) = cx
            .webviews()
            .evaluate_script_event(view, AsyncScript::new(body), guard)
        {
            self.failures
                .push(format!("{:?}: evaluation refused: {error:?}", self.step));
            self.finish(cx);
        }
    }

    fn next(&mut self, cx: &mut EventContext) {
        let app = self.origin(Site::App);
        let result = match self.step {
            Step::Token => {
                let bad = self.origin(Site::BadCert);
                let url = self.fixture.url(Site::BadCert, "/getter", &[]);
                self.open(cx, url, WebViewOptions::new([bad]), Step::BadCert)
            }
            Step::BadCert => {
                let to = self.fixture.url(Site::Foreign, "/getter", &[]);
                let url = self.fixture.url(Site::App, "/redirect", &[("to", &to)]);
                self.open(cx, url, WebViewOptions::new([app]), Step::Redirect)
            }
            Step::Redirect | Step::ProfileWrite | Step::Clearing => {
                let step = match self.step {
                    Step::Redirect => Step::ProfileWrite,
                    Step::ProfileWrite => Step::ProfileRead,
                    _ => Step::ProfileEmpty,
                };
                let url = self.fixture.url(Site::App, "/getter", &[]);
                let options = WebViewOptions::new([app.clone()])
                    .evaluation_origins([app])
                    .data_store(DataStore::Persistent(Self::profile()));
                self.open(cx, url, options, step)
            }
            Step::ProfileRead => {
                self.step = Step::Clearing;
                match cx.webviews().clear_profile(&Self::profile()) {
                    Ok(clear) => {
                        self.clear = Some(clear);
                        self.poll_clear(cx);
                    }
                    Err(error) => {
                        self.failures.push(format!("clear_profile: {error:?}"));
                        self.finish(cx);
                    }
                }
                Ok(())
            }
            _ => {
                self.finish(cx);
                Ok(())
            }
        };
        if let Err(error) = result {
            self.failures
                .push(format!("{:?}: open failed: {error:?}", self.step));
            self.finish(cx);
        }
    }

    fn poll_clear(&mut self, cx: &mut EventContext) {
        let Some(clear) = self.clear.as_mut() else {
            return;
        };
        if let Poll::Ready(result) = Pin::new(clear).poll(&mut Context::from_waker(Waker::noop())) {
            self.clear = None;
            self.check("clearing the profile", result, Ok(()));
            self.next(cx);
        }
    }

    fn finish(&mut self, cx: &mut EventContext) {
        self.step = Step::Done;
        cx.exit();
    }

    fn webview_event(&mut self, event: WebViewEvent, cx: &mut EventContext) {
        if std::env::var_os("QUARK_WEBVIEW_SMOKE_TRACE").is_some() {
            eprintln!("webview_smoke linux: {:?}: {event:?}", self.step);
        }
        match (self.step, event) {
            (Step::Getter, WebViewEvent::Opened { capabilities, .. }) => {
                self.check("X11 parent relationship", capabilities.parent, ParentRelationship::Native);
            }
            (Step::Getter, WebViewEvent::PageLoadFinished { view, document, .. }) => {
                let parent = cx.window().and_then(|window| match window.window_handle().ok()?.as_raw() {
                    RawWindowHandle::Xlib(handle) => Some(handle.window as u32),
                    RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
                    _ => None,
                });
                match parent {
                    Some(parent) => self.check(
                        "the modal's WM_TRANSIENT_FOR is the parent",
                        x11::transient_for_window_named(TITLE),
                        Some(parent),
                    ),
                    None => self.failures.push("the parent has no X11 window".into()),
                }
                self.step = Step::Token;
                self.evaluate(cx, view, document, "return await window.fixture.getToken();");
                self.fixture.release("h1");
            }
            (Step::Token, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "the getter's Promise value",
                    result.map(|value| value.into_json()),
                    Ok(serde_json::json!("linux-token")),
                );
                cx.webviews().close(view);
            }
            (Step::BadCert, WebViewEvent::NavigationFailed { stage, error, .. }) => {
                self.check("a bad certificate fails", (stage, error), (FailureStage::Provisional, NavigationError::Tls));
                // WebKit loads its error page right after the failure,
                // unless the backend stops it. Give it time to, then look.
                let (quiet, waker) = (Arc::clone(&self.quiet), cx.waker().clone());
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(1500));
                    quiet.store(true, Ordering::Release);
                    waker.wake();
                });
            }
            (Step::BadCert, WebViewEvent::NavigationCommitted { url, .. }) => {
                self.failures.push(format!("a document committed after the certificate failure: {url:?}"));
            }
            (Step::Redirect, WebViewEvent::NavigationBlocked { reason, .. }) => {
                self.check("the foreign redirect is blocked", reason, BlockReason::OriginNotAllowed);
            }
            (Step::Redirect, WebViewEvent::NavigationFailed { error, view, .. }) => {
                self.check(
                    "the redirect failure names the policy",
                    error,
                    NavigationError::Policy(BlockReason::OriginNotAllowed),
                );
                let foreign = self.fixture.requests().into_iter().filter(|r| r.site == Site::Foreign).count();
                self.check("requests that reached the foreign origin", foreign, 0);
                cx.webviews().close(view);
            }
            (Step::ProfileWrite, WebViewEvent::PageLoadFinished { view, document, .. }) => self.evaluate(
                cx,
                view,
                document,
                "localStorage.setItem('k', 'v'); document.cookie = 'c=1; Secure; Max-Age=3600'; return true;",
            ),
            (Step::ProfileRead | Step::ProfileEmpty, WebViewEvent::PageLoadFinished { view, document, .. }) => {
                self.evaluate(cx, view, document, "return [localStorage.getItem('k'), document.cookie];")
            }
            (Step::ProfileWrite, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check("writing the profile", result.map(|value| value.into_json()), Ok(serde_json::json!(true)));
                cx.webviews().close(view);
            }
            (Step::ProfileRead, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "a later view of the profile reads its storage",
                    result.map(|value| value.into_json()),
                    Ok(serde_json::json!(["v", "c=1"])),
                );
                cx.webviews().close(view);
            }
            (Step::ProfileEmpty, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "a cleared profile reads empty",
                    result.map(|value| value.into_json()),
                    Ok(serde_json::json!([null, ""])),
                );
                cx.webviews().close(view);
            }
            (_, WebViewEvent::Closed { view, reason }) if Some(view) == self.view => {
                if !matches!(reason, WebCloseReason::Program) {
                    self.failures.push(format!("{:?}: closed by {reason:?}", self.step));
                    return self.finish(cx);
                }
                self.view = None;
                self.next(cx);
            }
            (_, WebViewEvent::EvaluationFinished { result: Err(error), .. }) => {
                self.failures.push(format!("{:?}: evaluation failed: {error:?}", self.step));
                self.finish(cx);
            }
            _ => {}
        }
    }
}

impl App for Smoke {
    fn init(&mut self, cx: &mut EventContext) {
        let app = self.origin(Site::App);
        let url = self.fixture.url(
            Site::App,
            "/getter",
            &[("token", "linux-token"), ("hold", "h1")],
        );
        let options = WebViewOptions::new([app.clone()]).evaluation_origins([app]);
        if let Err(error) = self.open(cx, url, options, Step::Getter) {
            self.failures.push(format!("open: {error:?}"));
            self.finish(cx);
        }
    }

    fn frame(&mut self, _cx: &mut FrameContext) -> Scene {
        Scene::default()
    }

    fn wake(&mut self, cx: &mut EventContext) {
        if self.step == Step::BadCert
            && self.quiet.swap(false, Ordering::AcqRel)
            && let Some(view) = self.view
        {
            cx.webviews().close(view);
        }
        if self.step == Step::Clearing {
            self.poll_clear(cx);
        }
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
        if let AppEvent::WebView(event) = event {
            self.webview_event(event, cx);
        }
    }
}

impl Drop for Smoke {
    fn drop(&mut self) {
        if self.step != Step::Done {
            self.failures.push(format!("stopped at {:?}", self.step));
        }
        for failure in &self.failures {
            eprintln!("webview_smoke linux: FAIL: {failure}");
        }
        PASSED.store(self.failures.is_empty(), Ordering::Release);
    }
}

mod x11 {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, Window};

    /// WM_TRANSIENT_FOR of the window titled `title`, searched among the
    /// root's children and theirs (a window manager reparents clients into
    /// frames).
    pub fn transient_for_window_named(title: &str) -> Option<u32> {
        let (conn, screen) = x11rb::connect(None).ok()?;
        let root = conn.setup().roots[screen].root;
        let mut stack = vec![(root, 0)];
        while let Some((window, depth)) = stack.pop() {
            if name(&conn, window).as_deref() == Some(title) {
                return transient_for(&conn, window);
            }
            if depth < 2 {
                let tree = conn.query_tree(window).ok()?.reply().ok()?;
                stack.extend(tree.children.into_iter().map(|child| (child, depth + 1)));
            }
        }
        None
    }

    fn name(conn: &impl Connection, window: Window) -> Option<String> {
        let reply = conn
            .get_property(false, window, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 256)
            .ok()?
            .reply()
            .ok()?;
        String::from_utf8(reply.value).ok()
    }

    fn transient_for(conn: &impl Connection, window: Window) -> Option<u32> {
        let reply = conn
            .get_property(
                false,
                window,
                AtomEnum::WM_TRANSIENT_FOR,
                AtomEnum::WINDOW,
                0,
                1,
            )
            .ok()?
            .reply()
            .ok()?;
        reply.value32()?.next()
    }
}
