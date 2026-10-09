//! A modal sign-in webview against a local HTTPS fixture, written with
//! `view!` (docs/guide/writing-views.md).
//!
//! "Sign in" opens a modal webview at the fixture's identity provider
//! origin, which redirects to the app origin. Once that page loads, the
//! app runs the page's async getter, which resolves to a synthetic token
//! after a short delay, and closes the window. The timeline shows each
//! webview event with only its origin, and the getter result only as its
//! type and length, as an app handling a real credential would.
//!
//! The fixture is the one the webview tests use (`tests/webview/`). It runs
//! in this process on loopback ports, and the demo trusts exactly its
//! certificate for exactly those hosts through the test-only
//! `quark_webview::testing` hook; nothing else's certificate is affected.
//! Needs the `webview` feature and a platform engine (WebKitGTK 4.1 on
//! Linux, WKWebView on macOS, WebView2 on Windows).

#[path = "../tests/webview/mod.rs"]
mod webview;

use quark::view;
use quark_app::platform::webview::{
    AsyncScript, EvalError, Origin, OriginGuard, PageUrl, ScriptValue, Url, WebViewEvent,
    WebViewHandle, WebViewOptions, WebWindowOptions,
};
use quark_app::quark_ui::Action;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div, text};
use quark_app::quark_ui::style::Styled;
use quark_app::{AppEvent, UiApp, UiContext, ViewContext, WindowOptions};
use webview::fixture::{Fixture, Site};

/// The fixture getter's synthetic credential. Never shown: the demo
/// displays only what kind of value came back.
const TOKEN: &str = "demo-token-0123456789";
/// The getter as Portal would call it: trusted code, no arguments.
const GETTER: &str = "return await window.fixture.getToken();";

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    SignIn,
    Cancel,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

struct WebviewDemo {
    fixture: Fixture,
    view: Option<WebViewHandle>,
    status: String,
    timeline: Vec<String>,
}

/// A URL as the timeline shows it: the origin only, since paths and
/// queries can carry codes and tokens.
fn origin_of(url: &PageUrl) -> String {
    url.origin().ascii_serialization()
}

/// A getter result as the timeline shows it: never the value.
fn redacted(result: &Result<ScriptValue, EvalError>) -> String {
    match result {
        Ok(value) => match value.as_json() {
            serde_json::Value::String(token) => {
                format!("received a string of {} characters", token.chars().count())
            }
            other => format!("received {}", kind(other)),
        },
        // `EvalError`'s Display redacts page-supplied details.
        Err(error) => format!("failed: {error}"),
    }
}

fn kind(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "a boolean",
        serde_json::Value::Number(_) => "a number",
        serde_json::Value::String(_) => "a string",
        serde_json::Value::Array(_) => "an array",
        serde_json::Value::Object(_) => "an object",
    }
}

impl WebviewDemo {
    fn new(fixture: Fixture) -> Self {
        Self {
            fixture,
            view: None,
            status: "Not signed in".into(),
            timeline: Vec::new(),
        }
    }

    fn origin(&self, site: Site) -> Origin {
        Origin::parse(&self.fixture.origin(site)).expect("fixture origins parse")
    }

    fn sign_in(&mut self, cx: &mut UiContext) {
        if self.view.is_some() {
            return;
        }
        // The identity provider redirects to the app's getter page.
        let page = self.fixture.url(
            Site::App,
            "/getter",
            &[("token", TOKEN), ("delay_ms", "400")],
        );
        let start = self.fixture.url(Site::Idp, "/redirect", &[("to", &page)]);
        let app = self.origin(Site::App);
        let options = WebWindowOptions::new(
            WebViewOptions::new([self.origin(Site::Idp), app.clone()]).evaluation_origins([app]),
        );
        let url = Url::parse(&start).expect("fixture URLs parse");
        self.timeline.clear();
        match cx.window.webviews().open(url, options) {
            Ok(view) => {
                self.view = Some(view);
                self.status = "Signing in…".into();
            }
            Err(error) => self.status = format!("Could not open the sign-in window: {error}"),
        }
    }

    fn button(id: &str, label: &str, msg: Msg, cx: &ViewContext) -> AnyElement {
        let colors = &cx.theme.colors;
        view! {
            <div
                accessibility_id={id}
                role="button"
                aria-label={label}
                on:click={msg}
                class="px-4 h-9 items-center justify-center rounded-[8]
                        bg-[colors.accent] hover:bg-[colors.accent_strong]"
            >
                <text class="font-semibold" color={colors.on_accent}>{label}</text>
            </div>
        }
    }

    /// One timeline line per webview event; `None` for events not shown.
    fn describe(event: &WebViewEvent) -> Option<String> {
        Some(match event {
            WebViewEvent::Opened { capabilities, .. } => {
                format!("opened ({:?} parent)", capabilities.parent)
            }
            WebViewEvent::NavigationStarted { url, .. } => {
                format!("navigation started: {}", origin_of(url))
            }
            WebViewEvent::NavigationRedirected { url, .. } => {
                format!("redirected: {}", origin_of(url))
            }
            WebViewEvent::NavigationBlocked { url, reason, .. } => {
                let origin = url.as_ref().map(origin_of).unwrap_or_default();
                format!("blocked: {origin} ({reason})")
            }
            WebViewEvent::NavigationCommitted { origin, .. } => format!("committed: {origin}"),
            WebViewEvent::NavigationFailed { stage, error, .. } => {
                format!("navigation failed: {stage:?} {error:?}")
            }
            WebViewEvent::PageLoadFinished { http_status, .. } => match http_status {
                Some(status) => format!("page loaded (HTTP {status})"),
                None => "page loaded".into(),
            },
            WebViewEvent::EvaluationFinished { result, .. } => {
                format!("getter {}", redacted(result))
            }
            WebViewEvent::Closed { reason, .. } => format!("closed: {reason:?}"),
            _ => return None,
        })
    }
}

impl UiApp for WebviewDemo {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let colors = &cx.theme.colors;
        let signing_in = self.view.is_some();
        view! {
            <div w={width} h={height} class="items-center justify-center bg-[colors.background]">
                <div
                    accessibility_id="webview_demo.panel"
                    role="group"
                    aria-label="Webview demo"
                    class="w-[480px] p-6 gap-4 flex-col rounded-[16] bg-[colors.surface]"
                >
                    <text class="text-lg font-bold">"Sign in with a modal webview"</text>
                    <text color={colors.text}>
                        "Opens a local HTTPS fixture in a modal window and reads its getter."
                    </text>
                    <div class="flex-row gap-2">
                        if signing_in {
                            {Self::button("webview_demo.cancel", "Cancel", Msg::Cancel, cx)}
                        } else {
                            {Self::button("webview_demo.sign_in", "Sign in", Msg::SignIn, cx)}
                        }
                    </div>
                    <div
                        accessibility_id="webview_demo.status"
                        role="status"
                        aria-label={self.status.clone()}
                    >
                        <text class="font-semibold">{self.status.clone()}</text>
                    </div>
                    <div
                        accessibility_id="webview_demo.timeline"
                        role="list"
                        aria-label="Webview events"
                        class="flex-col gap-1"
                    >
                        for line in &self.timeline {
                            <div role="listitem" aria-label={line.clone()}>
                                <text class="text-sm font-mono" color={colors.text}>
                                    {line.clone()}
                                </text>
                            </div>
                        }
                    </div>
                </div>
            </div>
        }
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        match msg {
            Msg::SignIn => self.sign_in(cx),
            Msg::Cancel => {
                if let Some(view) = self.view {
                    cx.window.webviews().close(view);
                }
            }
        }
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut UiContext) {
        let AppEvent::WebView(event) = event else {
            return;
        };
        if Some(event.view()) != self.view {
            return;
        }
        if let Some(line) = Self::describe(&event) {
            self.timeline.push(line);
        }
        match event {
            // The page loaded on the app origin: run its getter there.
            WebViewEvent::PageLoadFinished { view, document, .. } => {
                let guard = OriginGuard::new(self.origin(Site::App), document);
                if let Err(error) = cx.window.webviews().evaluate_script_event(
                    view,
                    AsyncScript::new(GETTER),
                    guard,
                ) {
                    // Not the app origin (still on the identity provider).
                    self.timeline.push(format!("getter not run: {error}"));
                }
            }
            WebViewEvent::EvaluationFinished { view, result, .. } => {
                self.status = match &result {
                    Ok(_) => format!("Signed in: {}", redacted(&result)),
                    Err(_) => format!("Sign-in {}", redacted(&result)),
                };
                // A real app stores the credential first, then closes.
                cx.window.webviews().close(view);
            }
            WebViewEvent::Closed { reason, .. } => {
                self.view = None;
                if !self.status.starts_with("Signed in") {
                    self.status = format!("Sign-in window closed ({reason:?})");
                }
            }
            _ => {}
        }
    }
}

fn main() -> Result<(), quark_app::RunError> {
    let fixture = Fixture::start().expect("the fixture binds loopback ports");
    quark_webview::testing::trust_leaf(webview::fixture::leaf_der(), &["127.0.0.1", "localhost"]);
    quark_app::run_ui(
        WebviewDemo::new(fixture),
        WindowOptions {
            title: "Webview demo".into(),
            size: (640.0, 560.0),
            ..WindowOptions::default()
        },
    )
}
