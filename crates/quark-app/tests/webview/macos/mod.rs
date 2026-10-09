//! macOS (WKWebView) checks against the fixture, run as their own process
//! by `webview_smoke` (one winit event loop per process). Each step waits
//! for the event the previous one should produce; a failure, or no
//! progress within two minutes, exits non-zero.
//!
//! - The modal is a sheet on the parent window, and says so.
//! - A Promise-returning page getter's value comes back: the script runs
//!   in the page's own world, where the page's globals are.
//! - A rejected Promise is a JavaScript exception.
//! - `history.pushState` is reported as a same-document location change.
//! - Command-V pastes into the page.
//! - A navigation while a script runs fails the script.
//! - A certificate failure is a TLS failure; nothing commits.
//! - A redirect to a foreign origin is refused before its request, and
//!   the failure names the policy.
//! - A popup is refused and reported.
//! - A dark view tells the page it is dark.
//! - Escape cancels the sheet as a user close and removes it.
//! - A persistent profile keeps its storage across views until cleared.
//!
//! The app must run as a bundle launched by LaunchServices (`open`) to get
//! windows; see `run-macos-smoke.sh` beside this file.

use std::future::Future;
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use quark::scene::Scene;
use quark_app::platform::webview::{
    Appearance, AsyncScript, BlockReason, DataStore, DocumentId, EvalError, FailureStage,
    NavigationError, OpenError, Origin, OriginGuard, ParentRelationship, ProfileClear,
    ProfileError, ProfileId, ProfileMode, Url, WebCloseReason, WebViewEvent, WebViewHandle,
    WebViewOptions, WebWindowOptions,
};
use quark_app::{App, AppEvent, EventContext, FrameContext, WindowOptions};
use serde_json::json;

use super::fixture::{Fixture, Site, leaf_der};

const TITLE: &str = "macOS smoke sign in";
const PASTED: &str = "pasted by command-v";

pub fn run() -> ExitCode {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(120));
        eprintln!("webview_smoke macos: no progress within 120 s");
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
            pasteboard: None,
            document: None,
            located: 0,
        },
        WindowOptions {
            title: "macos webview smoke".into(),
            size: (720.0, 820.0),
            ..WindowOptions::default()
        },
    );
    if let Err(error) = result {
        eprintln!("webview_smoke macos: could not run: {error}");
        return ExitCode::FAILURE;
    }
    if PASSED.load(Ordering::Acquire) {
        println!("webview_smoke macos: passed");
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

static PASSED: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Copy, PartialEq)]
enum Step {
    Start,
    /// The getter page is loading in a sheet.
    Getter,
    /// Waiting for the getter's Promise.
    Token,
    /// Waiting for a rejected Promise.
    Rejects,
    /// A `history.pushState` in the same document.
    Location,
    /// An input is focused; then Command-V and its value.
    PasteFocus,
    PasteRead,
    /// A script is pending when the page navigates away.
    NavigateDuring,
    BadCert,
    Redirect,
    Popup,
    Dark,
    /// Escape in the sheet.
    Escape,
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
    /// The user's pasteboard text, restored after the paste check.
    pasteboard: Option<Option<String>>,
    /// The last committed document.
    document: Option<DocumentId>,
    /// Events seen of the two the location step waits for.
    located: u8,
}

impl Smoke {
    fn check<T: PartialEq + std::fmt::Debug>(&mut self, what: &str, got: T, want: T) {
        if got == want {
            println!("webview_smoke macos: ok: {what}");
        } else {
            self.failures
                .push(format!("{what}: got {got:?}, want {want:?}"));
        }
    }

    fn origin(&self, site: Site) -> Origin {
        Origin::parse(&self.fixture.origin(site)).expect("fixture origins parse")
    }

    fn app_options(&self) -> WebViewOptions {
        let app = self.origin(Site::App);
        WebViewOptions::new([app.clone()]).evaluation_origins([app])
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
        ProfileId::new("quark-webview-smoke", "macos").expect("a valid profile id")
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

    /// Open the view for the step after `self.step`'s view closed.
    fn next(&mut self, cx: &mut EventContext) {
        let app = self.origin(Site::App);
        let result = match self.step {
            Step::PasteRead => {
                let second = self
                    .fixture
                    .url(Site::App, "/getter", &[("title", "second")]);
                let url =
                    self.fixture
                        .url(Site::App, "/getter", &[("go_after", "k2"), ("go", &second)]);
                let options = self.app_options();
                self.open(cx, url, options, Step::NavigateDuring)
            }
            Step::NavigateDuring => {
                let bad = self.origin(Site::BadCert);
                let url = self.fixture.url(Site::BadCert, "/getter", &[]);
                self.open(cx, url, WebViewOptions::new([bad]), Step::BadCert)
            }
            Step::BadCert => {
                let to = self.fixture.url(Site::Foreign, "/getter", &[]);
                let url = self.fixture.url(Site::App, "/redirect", &[("to", &to)]);
                self.open(cx, url, WebViewOptions::new([app]), Step::Redirect)
            }
            Step::Redirect => {
                let to = self.fixture.url(Site::App, "/getter", &[]);
                let url = self.fixture.url(Site::App, "/popup", &[("to", &to)]);
                self.open(cx, url, WebViewOptions::new([app]), Step::Popup)
            }
            Step::Popup => {
                let url = self.fixture.url(Site::App, "/getter", &[]);
                let options = self.app_options().appearance(Appearance::Dark);
                self.open(cx, url, options, Step::Dark)
            }
            Step::Dark => {
                let url = self.fixture.url(Site::App, "/getter", &[]);
                self.open(cx, url, WebViewOptions::new([app]), Step::Escape)
            }
            Step::Escape | Step::ProfileWrite | Step::Clearing => {
                let step = match self.step {
                    Step::Escape => Step::ProfileWrite,
                    Step::ProfileWrite => Step::ProfileRead,
                    _ => Step::ProfileEmpty,
                };
                let url = self.fixture.url(Site::App, "/getter", &[]);
                let options = self
                    .app_options()
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
            // The detail is the native error's domain and code.
            let result = result.map_err(|error| match error {
                ProfileError::Platform(error) => error.detail().reveal().to_owned(),
                other => format!("{other:?}"),
            });
            self.check("clearing the profile", result, Ok(()));
            self.next(cx);
        }
    }

    fn finish(&mut self, cx: &mut EventContext) {
        self.step = Step::Done;
        cx.exit();
    }

    fn webview_event(&mut self, event: WebViewEvent, cx: &mut EventContext) {
        let parent = cx.window().and_then(native::ns_window);
        match (self.step, event) {
            (Step::Getter, WebViewEvent::Opened { capabilities, .. }) => {
                self.check(
                    "opened as a native sheet with a fresh store",
                    (
                        capabilities.parent,
                        capabilities.profile,
                        capabilities.appearance,
                    ),
                    (
                        ParentRelationship::Native,
                        ProfileMode::Ephemeral,
                        Appearance::System,
                    ),
                );
            }
            (Step::Getter, WebViewEvent::PageLoadFinished { view, document, .. }) => {
                self.check(
                    "the parent's attached sheet is the modal",
                    parent.as_deref().and_then(native::sheet_title),
                    Some(TITLE.to_owned()),
                );
                self.step = Step::Token;
                self.evaluate(
                    cx,
                    view,
                    document,
                    "return await window.fixture.getToken();",
                );
                self.fixture.release("h1");
            }
            (Step::Token, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "the page getter's Promise value",
                    result.map(|value| value.into_json()),
                    Ok(json!("macos-token")),
                );
                self.step = Step::Rejects;
                let document = self.document();
                self.evaluate(cx, view, document, "return await window.fixture.rejects();");
            }
            (Step::Rejects, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                let detail = match result {
                    Err(EvalError::JavaScriptException(detail)) => detail.reveal().to_owned(),
                    other => format!("{other:?}"),
                };
                self.check(
                    "a rejected Promise is an exception",
                    detail,
                    "Error: fixture rejection".to_owned(),
                );
                self.step = Step::Location;
                let document = self.document();
                self.evaluate(
                    cx,
                    view,
                    document,
                    "history.pushState(null, '', '/getter?pushed=1'); return true;",
                );
            }
            (Step::Location, WebViewEvent::LocationChanged { document, url, .. }) => {
                self.check(
                    "pushState is a location change in the same document",
                    (document, url.query()),
                    (self.document(), Some("pushed=1")),
                );
                self.location_step_done(cx);
            }
            (Step::Location, WebViewEvent::EvaluationFinished { result: Ok(_), .. }) => {
                self.location_step_done(cx);
            }
            (Step::PasteFocus, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "an input has focus",
                    result.map(|value| value.into_json()),
                    Ok(json!(true)),
                );
                self.pasteboard = Some(native::replace_pasteboard(PASTED));
                let sent = parent.as_deref().is_some_and(native::command_v_in_sheet);
                self.check("the sheet handles command-v", sent, true);
                self.step = Step::PasteRead;
                let document = self.document();
                self.evaluate(
                    cx,
                    view,
                    document,
                    "const input = document.getElementById('paste');\
                     for (let i = 0; i < 100 && !input.value; i++) {\
                       await new Promise(resolve => setTimeout(resolve, 20));\
                     }\
                     return input.value;",
                );
            }
            (Step::PasteRead, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                if let Some(saved) = self.pasteboard.take() {
                    native::restore_pasteboard(saved);
                }
                self.check(
                    "command-v pastes into the page",
                    result.map(|value| value.into_json()),
                    Ok(json!(PASTED)),
                );
                cx.webviews().close(view);
            }
            (Step::NavigateDuring, WebViewEvent::PageLoadFinished { view, document, .. }) => {
                self.evaluate(cx, view, document, "return await window.fixture.never();");
                self.fixture.release("k2");
            }
            (Step::NavigateDuring, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "a navigation fails the running script",
                    result.map(|value| value.into_json()),
                    Err(EvalError::NavigationChanged),
                );
                cx.webviews().close(view);
            }
            (
                Step::BadCert,
                WebViewEvent::NavigationFailed {
                    stage, error, view, ..
                },
            ) => {
                self.check(
                    "a bad certificate fails",
                    (stage, error),
                    (FailureStage::Provisional, NavigationError::Tls),
                );
                cx.webviews().close(view);
            }
            (Step::BadCert, WebViewEvent::NavigationCommitted { url, .. }) => {
                self.failures.push(format!(
                    "a document committed with a bad certificate: {url:?}"
                ));
            }
            (Step::Redirect, WebViewEvent::NavigationBlocked { reason, .. }) => {
                self.check(
                    "the foreign redirect is blocked",
                    reason,
                    BlockReason::OriginNotAllowed,
                );
            }
            (Step::Redirect, WebViewEvent::NavigationFailed { error, view, .. }) => {
                self.check(
                    "the redirect failure names the policy",
                    error,
                    NavigationError::Policy(BlockReason::OriginNotAllowed),
                );
                let foreign = self
                    .fixture
                    .requests()
                    .into_iter()
                    .filter(|request| request.site == Site::Foreign)
                    .count();
                self.check("requests that reached the foreign origin", foreign, 0);
                cx.webviews().close(view);
            }
            (Step::Popup, WebViewEvent::NavigationBlocked { reason, view, .. }) => {
                self.check("the popup is blocked", reason, BlockReason::Popup);
                cx.webviews().close(view);
            }
            (Step::Dark, WebViewEvent::Opened { capabilities, .. }) => {
                self.check(
                    "the dark appearance applies",
                    capabilities.appearance,
                    Appearance::Dark,
                );
            }
            (Step::Dark, WebViewEvent::PageLoadFinished { view, document, .. }) => self.evaluate(
                cx,
                view,
                document,
                "return matchMedia('(prefers-color-scheme: dark)').matches;",
            ),
            (Step::Dark, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "the page sees a dark color scheme",
                    result.map(|value| value.into_json()),
                    Ok(json!(true)),
                );
                cx.webviews().close(view);
            }
            (Step::Escape, WebViewEvent::PageLoadFinished { .. }) => {
                let sent = parent.as_deref().is_some_and(native::escape_in_sheet);
                self.check("escape reaches the sheet", sent, true);
            }
            (Step::ProfileWrite, WebViewEvent::PageLoadFinished { view, document, .. }) => self
                .evaluate(
                    cx,
                    view,
                    document,
                    "localStorage.setItem('k', 'v');\
                     document.cookie = 'c=1; Secure; Max-Age=3600';\
                     return true;",
                ),
            (
                Step::ProfileRead | Step::ProfileEmpty,
                WebViewEvent::PageLoadFinished { view, document, .. },
            ) => self.evaluate(
                cx,
                view,
                document,
                "return [localStorage.getItem('k'), document.cookie];",
            ),
            (Step::ProfileWrite, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "writing the profile",
                    result.map(|value| value.into_json()),
                    Ok(json!(true)),
                );
                cx.webviews().close(view);
            }
            (Step::ProfileRead, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "a later view of the profile reads its storage",
                    result.map(|value| value.into_json()),
                    Ok(json!(["v", "c=1"])),
                );
                cx.webviews().close(view);
            }
            (Step::ProfileEmpty, WebViewEvent::EvaluationFinished { view, result, .. }) => {
                self.check(
                    "a cleared profile reads empty",
                    result.map(|value| value.into_json()),
                    Ok(json!([null, ""])),
                );
                cx.webviews().close(view);
            }
            (Step::Escape, WebViewEvent::Closed { view, reason }) if Some(view) == self.view => {
                self.check("escape closes as the user", reason, WebCloseReason::User);
                self.check(
                    "the sheet is gone",
                    parent.as_deref().and_then(native::sheet_title),
                    None,
                );
                self.view = None;
                self.next(cx);
            }
            (_, WebViewEvent::Closed { view, reason }) if Some(view) == self.view => {
                if reason != WebCloseReason::Program {
                    self.failures
                        .push(format!("{:?}: closed by {reason:?}", self.step));
                    return self.finish(cx);
                }
                self.view = None;
                self.next(cx);
            }
            (
                _,
                WebViewEvent::EvaluationFinished {
                    result: Err(error), ..
                },
            ) => {
                self.failures
                    .push(format!("{:?}: evaluation failed: {error:?}", self.step));
                self.finish(cx);
            }
            _ => {}
        }
    }

    /// The location change and the script that caused it arrive in either
    /// order; focus an input once both have.
    fn location_step_done(&mut self, cx: &mut EventContext) {
        self.located += 1;
        if self.located < 2 {
            return;
        }
        let (Some(view), document) = (self.view, self.document()) else {
            return;
        };
        self.step = Step::PasteFocus;
        self.evaluate(
            cx,
            view,
            document,
            "const input = document.createElement('input');\
             input.id = 'paste';\
             document.body.append(input);\
             input.focus();\
             return document.activeElement === input;",
        );
    }

    fn document(&self) -> DocumentId {
        self.document
            .expect("a document committed before its load finished")
    }
}

impl App for Smoke {
    fn init(&mut self, cx: &mut EventContext) {
        let url = self.fixture.url(
            Site::App,
            "/getter",
            &[("token", "macos-token"), ("hold", "h1")],
        );
        let options = self.app_options();
        if let Err(error) = self.open(cx, url, options, Step::Getter) {
            self.failures.push(format!("open: {error:?}"));
            self.finish(cx);
        }
    }

    fn frame(&mut self, _cx: &mut FrameContext) -> Scene {
        Scene::default()
    }

    fn wake(&mut self, cx: &mut EventContext) {
        if self.step == Step::Clearing {
            self.poll_clear(cx);
        }
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
        if let AppEvent::WebView(event) = event {
            if let WebViewEvent::NavigationCommitted { document, .. } = &event {
                self.document = Some(*document);
            }
            self.webview_event(event, cx);
        }
    }
}

impl Drop for Smoke {
    fn drop(&mut self) {
        if let Some(saved) = self.pasteboard.take() {
            native::restore_pasteboard(saved);
        }
        if self.step != Step::Done {
            self.failures.push(format!("stopped at {:?}", self.step));
        }
        for failure in &self.failures {
            eprintln!("webview_smoke macos: FAIL: {failure}");
        }
        PASSED.store(self.failures.is_empty(), Ordering::Release);
    }
}

/// AppKit observations and synthetic keys, on the main thread.
mod native {
    use objc2::MainThreadMarker;
    use objc2::rc::Retained;
    use objc2_app_kit::{
        NSEvent, NSEventModifierFlags, NSEventType, NSPasteboard, NSPasteboardTypeString, NSView,
        NSWindow,
    };
    use objc2_foundation::{NSPoint, NSString};
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use winit::window::Window;

    pub fn ns_window(window: &Window) -> Option<Retained<NSWindow>> {
        let RawWindowHandle::AppKit(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        // SAFETY: winit's live view, on the main thread.
        let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
        view.window()
    }

    /// The title of the sheet attached to `parent`, if any.
    pub fn sheet_title(parent: &NSWindow) -> Option<String> {
        parent
            .attachedSheet()
            .map(|sheet| sheet.title().to_string())
    }

    fn key(
        sheet: &NSWindow,
        flags: NSEventModifierFlags,
        chars: &str,
        code: u16,
    ) -> Retained<NSEvent> {
        let chars = NSString::from_str(chars);
        NSEvent::keyEventWithType_location_modifierFlags_timestamp_windowNumber_context_characters_charactersIgnoringModifiers_isARepeat_keyCode(
            NSEventType::KeyDown,
            NSPoint::new(0.0, 0.0),
            flags,
            0.0,
            sheet.windowNumber(),
            None,
            &chars,
            &chars,
            false,
            code,
        )
        .expect("a key event")
    }

    /// Command-V as the app delivers it: the key window's key equivalents
    /// first. Whether the sheet handled it.
    pub fn command_v_in_sheet(parent: &NSWindow) -> bool {
        let Some(sheet) = parent.attachedSheet() else {
            return false;
        };
        sheet.performKeyEquivalent(&key(&sheet, NSEventModifierFlags::Command, "v", 9))
    }

    /// Escape as a key press in the sheet. Whether a sheet was there.
    pub fn escape_in_sheet(parent: &NSWindow) -> bool {
        let Some(sheet) = parent.attachedSheet() else {
            return false;
        };
        sheet.sendEvent(&key(&sheet, NSEventModifierFlags::empty(), "\u{1b}", 53));
        true
    }

    /// Put `text` on the general pasteboard, returning what was there.
    pub fn replace_pasteboard(text: &str) -> Option<String> {
        let _ = MainThreadMarker::new();
        let pasteboard = NSPasteboard::generalPasteboard();
        let kind = unsafe { NSPasteboardTypeString };
        let saved = pasteboard.stringForType(kind).map(|text| text.to_string());
        pasteboard.clearContents();
        pasteboard.setString_forType(&NSString::from_str(text), kind);
        saved
    }

    pub fn restore_pasteboard(saved: Option<String>) {
        let pasteboard = NSPasteboard::generalPasteboard();
        pasteboard.clearContents();
        if let Some(text) = saved {
            pasteboard.setString_forType(&NSString::from_str(&text), unsafe {
                NSPasteboardTypeString
            });
        }
    }
}
