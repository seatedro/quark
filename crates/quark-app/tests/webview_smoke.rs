//! Webviews against the local HTTPS fixture in `tests/webview/`. First the
//! fixture itself over real TLS: its CA verifies both hosts, the bad
//! certificate site does not, redirects carry their target, and a held
//! response waits for its release. Then, with the `webview` feature, real
//! modal webviews run the scenarios in `browser`: an awaited getter, a
//! navigation racing an evaluation, rejections and bad results, blocked
//! redirects, frames, popups and certificates, deadlines, close, and
//! ephemeral storage. Any failure, or a step with no progress for 30 s,
//! exits non-zero.
//!
//! `cargo test -p quark-app --test webview_smoke -- --serve` instead runs
//! the fixture for another process; see `webview::fixture::serve_stdio` for
//! the line protocol.

#[path = "webview/mod.rs"]
mod webview;

use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

use webview::fixture::{Fixture, Site, client};

fn main() -> ExitCode {
    if std::env::args().any(|arg| arg == "--serve") {
        return match webview::fixture::serve_stdio() {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("webview_smoke: fixture: {e}");
                ExitCode::FAILURE
            }
        };
    }
    #[cfg(all(target_os = "linux", feature = "webview"))]
    if std::env::args().any(|arg| arg == "--linux") {
        return webview::linux::run();
    }
    let mut failures = Vec::new();
    fixture_checks(&mut failures);
    #[cfg(feature = "webview")]
    if failures.is_empty() {
        // One winit event loop per process: the Linux-specific checks get
        // their own.
        #[cfg(target_os = "linux")]
        match std::process::Command::new(std::env::current_exe().expect("own path"))
            .arg("--linux")
            .status()
        {
            Ok(status) if status.success() => {}
            other => failures.push(format!("linux checks: {other:?}")),
        }
        if cfg!(target_os = "linux")
            && std::env::var_os("DISPLAY").is_none()
            && std::env::var_os("WAYLAND_DISPLAY").is_none()
        {
            println!("webview_smoke: no display; browser scenarios skipped");
        } else {
            failures.extend(browser::run());
        }
    }
    if failures.is_empty() {
        println!("webview_smoke: ok");
        ExitCode::SUCCESS
    } else {
        for failure in &failures {
            eprintln!("webview_smoke: FAILED: {failure}");
        }
        ExitCode::FAILURE
    }
}

fn check(failures: &mut Vec<String>, what: &str, ok: bool, detail: impl std::fmt::Debug) {
    if ok {
        println!("webview_smoke: ok: {what}");
    } else {
        failures.push(format!("{what}: {detail:?}"));
    }
}

fn fixture_checks(failures: &mut Vec<String>) {
    let fixture = Fixture::start().expect("fixture binds its sites");

    let got = client::get(&fixture, Site::App, "/getter?token=t");
    check(
        failures,
        "the fixture CA verifies the 127.0.0.1 leaf",
        matches!(got, Ok((200, _))),
        &got,
    );

    let got = client::get(&fixture, Site::Idp, "/getter");
    check(
        failures,
        "the fixture CA verifies the localhost leaf",
        matches!(got, Ok((200, _))),
        &got,
    );

    let got = client::get(&fixture, Site::BadCert, "/getter");
    let unknown_issuer = matches!(&got, Err(e) if e.to_string().contains("UnknownIssuer"));
    check(
        failures,
        "the bad certificate site fails verification",
        unknown_issuer,
        &got,
    );

    let to = fixture.url(Site::Foreign, "/getter", &[]);
    let got = client::get(
        &fixture,
        Site::App,
        &format!("/redirect?to={}", webview::pages::encode(&to)),
    );
    let location = format!("Location: {to}");
    let redirected = matches!(&got, Ok((302, head)) if head.lines().any(|line| line == location));
    check(failures, "a redirect names its target", redirected, &got);

    let (tx, rx) = mpsc::channel();
    let port = fixture.port(Site::App);
    std::thread::scope(|scope| {
        let fixture = &fixture;
        scope.spawn(move || {
            let _ = tx.send(client::get(fixture, Site::App, "/slow?key=k1"));
        });
        let logged = fixture.wait_request(Site::App, "/slow", Duration::from_secs(10));
        // The server logged the request; with k1 unreleased it must not have
        // answered.
        let early = rx.try_recv();
        check(
            failures,
            "a held response waits for its key",
            logged.is_some() && early.is_err(),
            (port, &logged, &early),
        );
        fixture.release("k1");
        let got = rx.recv_timeout(Duration::from_secs(10));
        check(
            failures,
            "releasing the key answers the held response",
            matches!(got, Ok(Ok((200, _)))),
            &got,
        );
    });
}

/// Real webviews against the fixture, as an [`App`] stepping through
/// scenarios on its webview events. Each scenario opens one view and ends
/// by closing it.
#[cfg(feature = "webview")]
mod browser {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    use quark::scene::Scene;
    use quark_app::platform::webview::{
        AsyncScript, BlockReason, DocumentId, EvalError, EvaluationId, EvaluationLimits,
        InvalidResult, NavigationError, Origin, OriginGuard, Url, WebCloseReason, WebViewEvent,
        WebViewHandle, WebViewOptions, WebWindowOptions,
    };
    use quark_app::{App, AppEvent, EventContext, FrameContext, WindowOptions};
    use serde_json::{Value, json};

    use super::webview::fixture::{Fixture, Site, leaf_der};

    /// What an evaluation should end with.
    enum Want {
        Value(Value),
        Error(&'static str, fn(&EvalError) -> bool),
    }

    enum Step {
        /// Open a new view; the scenario's name is printed with its checks.
        Open(&'static str, String, WebWindowOptions),
        /// The next `PageLoadFinished`, whose document must be on `Site`.
        Load(Site),
        /// Evaluate with an app-origin guard on the loaded document and
        /// wait for the result.
        Eval(&'static str, Want),
        /// Evaluate without waiting; `Finish` waits.
        Start(&'static str),
        Finish(Want),
        Release(&'static str),
        /// Some event of this view so far matches.
        Saw(&'static str, fn(&WebViewEvent) -> bool),
        /// The fixture never received this path on this site.
        NoRequest(Site, &'static str),
        Close,
    }

    struct Smoke {
        fixture: Fixture,
        steps: Vec<Step>,
        next: usize,
        progress: Arc<AtomicUsize>,
        scenario: &'static str,
        view: Option<WebViewHandle>,
        /// The view's events so far, for `Saw`.
        seen: Vec<WebViewEvent>,
        origins: HashMap<DocumentId, Origin>,
        document: Option<DocumentId>,
        /// The evaluation a step waits on.
        pending: Option<EvaluationId>,
        failures: Arc<Mutex<Vec<String>>>,
    }

    fn origin(fixture: &Fixture, site: Site) -> Origin {
        Origin::parse(&fixture.origin(site)).unwrap()
    }

    fn options(fixture: &Fixture, sites: &[Site]) -> WebWindowOptions {
        let navigation = sites.iter().map(|&site| origin(fixture, site));
        WebWindowOptions::new(
            WebViewOptions::new(navigation).evaluation_origins([origin(fixture, Site::App)]),
        )
        .size(480.0, 360.0)
    }

    fn scenarios(f: &Fixture) -> Vec<Step> {
        use Step::*;
        let app_idp = options(f, &[Site::App, Site::Idp]);
        let getter = |token: &str| f.url(Site::App, "/getter", &[("token", token)]);
        let get_token = "return await window.fixture.getToken();";
        let error = |what, is: fn(&EvalError) -> bool| Want::Error(what, is);
        vec![
            Open(
                "awaited getter",
                f.url(
                    Site::App,
                    "/getter",
                    &[("token", "tok-1"), ("delay_ms", "300")],
                ),
                app_idp.clone(),
            ),
            Load(Site::App),
            Eval(get_token, Want::Value(json!("tok-1"))),
            Eval("return null;", Want::Value(Value::Null)),
            Eval(
                "return undefined;",
                error("InvalidResult(Undefined)", |e| {
                    matches!(e, EvalError::InvalidResult(InvalidResult::Undefined))
                }),
            ),
            Eval(
                "return await window.fixture.rejects();",
                error("JavaScriptException", |e| {
                    matches!(e, EvalError::JavaScriptException(_))
                }),
            ),
            Eval(
                "return window.fixture.whereami().main;",
                Want::Value(json!(true)),
            ),
            Close,
            Open(
                "navigation during a held evaluation",
                f.url(
                    Site::App,
                    "/getter",
                    &[
                        ("token", "first"),
                        ("hold", "h1"),
                        ("go_after", "go1"),
                        ("go", &getter("second")),
                    ],
                ),
                app_idp.clone(),
            ),
            Load(Site::App),
            Start(get_token),
            Release("go1"),
            Finish(error("NavigationChanged", |e| {
                matches!(e, EvalError::NavigationChanged)
            })),
            Load(Site::App),
            Release("h1"),
            Eval(get_token, Want::Value(json!("second"))),
            Close,
            Open(
                "a page whose script threw",
                f.url(Site::App, "/throws", &[]),
                app_idp.clone(),
            ),
            Load(Site::App),
            Eval(
                get_token,
                error("JavaScriptException", |e| {
                    matches!(e, EvalError::JavaScriptException(_))
                }),
            ),
            Close,
            Open(
                "redirect to a disallowed origin",
                f.url(
                    Site::App,
                    "/redirect",
                    &[("to", &f.url(Site::Foreign, "/getter", &[]))],
                ),
                app_idp.clone(),
            ),
            Saw("blocked OriginNotAllowed", |e| {
                matches!(
                    e,
                    WebViewEvent::NavigationBlocked {
                        reason: BlockReason::OriginNotAllowed,
                        ..
                    }
                )
            }),
            Saw("failed by policy", |e| {
                matches!(
                    e,
                    WebViewEvent::NavigationFailed {
                        error: NavigationError::Policy(_),
                        ..
                    }
                )
            }),
            NoRequest(Site::Foreign, "/getter"),
            Close,
            Open(
                "redirect to the identity provider",
                f.url(
                    Site::App,
                    "/redirect",
                    &[("to", &f.url(Site::Idp, "/getter", &[("token", "idp")]))],
                ),
                app_idp.clone(),
            ),
            Saw("redirected", |e| {
                matches!(e, WebViewEvent::NavigationRedirected { .. })
            }),
            Load(Site::Idp),
            Eval(
                get_token,
                error("WrongOrigin", |e| matches!(e, EvalError::WrongOrigin)),
            ),
            Close,
            Open(
                "main frame only",
                f.url(
                    Site::App,
                    "/frame",
                    &[
                        ("token", "main"),
                        ("src", &f.url(Site::Idp, "/getter", &[("token", "frame")])),
                    ],
                ),
                app_idp.clone(),
            ),
            Load(Site::App),
            Eval(
                "await window.fixture.frameLoaded(); return await window.fixture.getToken();",
                Want::Value(json!("main")),
            ),
            Close,
            Open(
                "a frame from a disallowed origin",
                f.url(
                    Site::App,
                    "/frame",
                    &[("src", &f.url(Site::Foreign, "/frame", &[]))],
                ),
                app_idp.clone(),
            ),
            Load(Site::App),
            Saw("frame blocked", |e| {
                matches!(
                    e,
                    WebViewEvent::NavigationBlocked {
                        reason: BlockReason::OriginNotAllowed,
                        ..
                    }
                )
            }),
            NoRequest(Site::Foreign, "/frame"),
            Close,
            Open("evaluation deadline", getter("t"), {
                let limits = EvaluationLimits::default().timeout(Duration::from_secs(1));
                let view = options(f, &[Site::App])
                    .view()
                    .clone()
                    .evaluation_limits(limits);
                WebWindowOptions::new(view).size(480.0, 360.0)
            }),
            Load(Site::App),
            Eval(
                "return await window.fixture.never();",
                error("Timeout", |e| matches!(e, EvalError::Timeout)),
            ),
            Close,
            Open(
                "closing ends a pending evaluation",
                f.url(Site::App, "/getter", &[("hold", "h2")]),
                app_idp.clone(),
            ),
            Load(Site::App),
            Start(get_token),
            Close,
            Finish(error("WindowClosed", |e| {
                matches!(e, EvalError::WindowClosed)
            })),
            Release("h2"),
            Open(
                "ephemeral storage, first view",
                getter("t"),
                app_idp.clone(),
            ),
            Load(Site::App),
            Eval(
                "localStorage.setItem('k', 'v'); \
                 document.cookie = 'c=1; Secure; SameSite=Strict; Max-Age=600'; \
                 return [localStorage.getItem('k'), document.cookie];",
                Want::Value(json!(["v", "c=1"])),
            ),
            Close,
            Open(
                "ephemeral storage, second view",
                getter("t"),
                app_idp.clone(),
            ),
            Load(Site::App),
            Eval(
                "return [localStorage.getItem('k'), document.cookie];",
                Want::Value(json!([null, ""])),
            ),
            Close,
            Open(
                "an unknown certificate authority",
                f.url(Site::BadCert, "/getter", &[]),
                options(f, &[Site::App, Site::BadCert]),
            ),
            Saw("failed with TLS", |e| {
                matches!(
                    e,
                    WebViewEvent::NavigationFailed {
                        error: NavigationError::Tls,
                        ..
                    }
                )
            }),
            NoRequest(Site::BadCert, "/getter"),
            Close,
            Open(
                "popups are denied",
                f.url(Site::App, "/popup", &[("to", &getter("popup"))]),
                app_idp,
            ),
            Load(Site::App),
            // Denied: `window.open` hands the page no window.
            Eval(
                "return window.fixture.openPopup();",
                Want::Value(json!(false)),
            ),
            Saw("popup blocked", |e| {
                matches!(
                    e,
                    WebViewEvent::NavigationBlocked {
                        reason: BlockReason::Popup,
                        ..
                    }
                )
            }),
            Close,
        ]
    }

    impl Smoke {
        fn fail(&self, what: String) {
            eprintln!("webview_smoke: FAILED: {}: {what}", self.scenario);
            self.failures
                .lock()
                .unwrap()
                .push(format!("{}: {what}", self.scenario));
        }

        fn ok(&self, what: &str) {
            println!("webview_smoke: ok: {}: {what}", self.scenario);
        }

        fn guard(&self) -> Option<OriginGuard> {
            Some(OriginGuard::new(
                origin(&self.fixture, Site::App),
                self.document?,
            ))
        }

        fn advance(&mut self) {
            self.next += 1;
            self.progress.store(self.next, Ordering::SeqCst);
        }

        /// Run steps until one has to wait for an event.
        fn run(&mut self, cx: &mut EventContext) {
            while let Some(step) = self.steps.get(self.next) {
                match step {
                    Step::Open(name, url, options) => {
                        self.scenario = name;
                        self.seen.clear();
                        self.document = None;
                        let url = Url::parse(url).unwrap();
                        match cx.webviews().open(url, options.clone()) {
                            Ok(view) => self.view = Some(view),
                            Err(error) => {
                                self.fail(format!("open: {error}"));
                                return cx.exit();
                            }
                        }
                    }
                    // Already sent; `EvaluationFinished` advances.
                    Step::Eval(..) if self.pending.is_some() => return,
                    Step::Eval(body, _) | Step::Start(body) => {
                        let (Some(view), Some(guard)) = (self.view, self.guard()) else {
                            self.fail("no loaded document to evaluate in".into());
                            return cx.exit();
                        };
                        let waits = matches!(step, Step::Eval(..));
                        match cx.webviews().evaluate_script_event(
                            view,
                            AsyncScript::new(*body),
                            guard,
                        ) {
                            Ok(id) => {
                                self.pending = Some(id);
                                if waits {
                                    return; // `EvaluationFinished` checks it.
                                }
                            }
                            Err(error) => {
                                if waits {
                                    // Advances past this step itself.
                                    self.check_result(Err(error));
                                    continue;
                                } else {
                                    self.fail(format!("start: {error:?}"));
                                }
                            }
                        }
                    }
                    Step::Release(key) => self.fixture.release(key),
                    Step::NoRequest(site, path) => {
                        let hit = self
                            .fixture
                            .requests()
                            .iter()
                            .any(|r| r.site == *site && r.path == *path);
                        if hit {
                            self.fail(format!("{site:?} received {path}"));
                        } else {
                            self.ok(&format!("{site:?} never received {path}"));
                        }
                    }
                    Step::Saw(what, matches) => {
                        if !self.seen.iter().any(matches) {
                            return;
                        }
                        self.ok(what);
                    }
                    Step::Close => {
                        if let Some(view) = self.view {
                            cx.webviews().close(view);
                        }
                        // Wait for `Closed` unless an evaluation result is
                        // expected first.
                        if !matches!(self.steps.get(self.next + 1), Some(Step::Finish(_))) {
                            return;
                        }
                    }
                    Step::Load(_) | Step::Finish(_) => return,
                }
                self.advance();
            }
            println!("webview_smoke: browser scenarios done");
            cx.exit();
        }

        fn check_result(
            &mut self,
            result: Result<quark_app::platform::webview::ScriptValue, EvalError>,
        ) {
            let want = match self.steps.get(self.next) {
                Some(Step::Eval(_, want) | Step::Finish(want)) => want,
                _ => return,
            };
            let failure = match (want, result) {
                (Want::Value(want), Ok(got)) if *want == got.as_json().clone() => None,
                (Want::Error(_, is), Err(error)) if is(&error) => None,
                (Want::Value(want), got) => Some(format!("want {want}, got {got:?}")),
                (Want::Error(name, _), got) => Some(format!("want {name}, got {got:?}")),
            };
            match failure {
                None => {
                    let what = match want {
                        Want::Value(value) => format!("result {value}"),
                        Want::Error(name, _) => format!("error {name}"),
                    };
                    self.ok(&what);
                }
                Some(failure) => self.fail(failure),
            }
            self.pending = None;
            self.advance();
        }
    }

    impl App for Smoke {
        fn init(&mut self, cx: &mut EventContext) {
            self.run(cx);
        }

        fn frame(&mut self, _cx: &mut FrameContext) -> Scene {
            Scene::default()
        }

        fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
            let AppEvent::WebView(event) = event else {
                return;
            };
            if Some(event.view()) != self.view {
                return;
            }
            self.seen.push(event.clone());
            match (&self.steps.get(self.next), event) {
                (
                    _,
                    WebViewEvent::NavigationCommitted {
                        document, origin, ..
                    },
                ) => {
                    self.origins.insert(document, origin);
                }
                (Some(Step::Load(site)), WebViewEvent::PageLoadFinished { document, .. }) => {
                    let want = origin(&self.fixture, *site);
                    match self.origins.get(&document) {
                        Some(got) if *got == want => self.ok(&format!("loaded on {site:?}")),
                        got => self.fail(format!("loaded on {got:?}, want {want:?}")),
                    }
                    self.document = Some(document);
                    self.advance();
                }
                (
                    Some(Step::Eval(..) | Step::Finish(_)),
                    WebViewEvent::EvaluationFinished {
                        evaluation, result, ..
                    },
                ) if Some(evaluation) == self.pending => {
                    self.check_result(result);
                }
                (Some(Step::Close), WebViewEvent::Closed { reason, .. }) => {
                    if reason == WebCloseReason::Program {
                        self.ok("closed");
                    } else {
                        self.fail(format!("closed with {reason:?}"));
                    }
                    self.advance();
                }
                (_, WebViewEvent::Closed { reason, .. }) => {
                    // A close after `Finish` consumed the `Close` step.
                    if reason != WebCloseReason::Program {
                        self.fail(format!("closed early with {reason:?}"));
                        return cx.exit();
                    }
                }
                _ => {}
            }
            self.run(cx);
        }
    }

    /// Run every scenario on a real window; returns the failures.
    pub(super) fn run() -> Vec<String> {
        let fixture = Fixture::start().expect("fixture binds its sites");
        quark_webview::testing::trust_leaf(leaf_der(), &["127.0.0.1", "localhost"]);
        let steps = scenarios(&fixture);
        let progress = Arc::new(AtomicUsize::new(0));
        let failures = Arc::new(Mutex::new(Vec::new()));
        let watched = Arc::clone(&progress);
        let progress_after = Arc::clone(&progress);
        std::thread::spawn(move || {
            let mut last = usize::MAX;
            loop {
                std::thread::sleep(Duration::from_secs(30));
                let now = watched.load(Ordering::SeqCst);
                if now == last {
                    eprintln!("webview_smoke: no progress for 30 s at step {now}");
                    std::process::exit(1);
                }
                last = now;
            }
        });
        let smoke = Smoke {
            fixture,
            steps,
            next: 0,
            progress,
            scenario: "",
            view: None,
            seen: Vec::new(),
            origins: HashMap::new(),
            document: None,
            pending: None,
            failures: Arc::clone(&failures),
        };
        let total = smoke.steps.len();
        let result = quark_app::run(
            smoke,
            WindowOptions {
                title: "webview smoke".into(),
                size: (320.0, 200.0),
                ..WindowOptions::default()
            },
        );
        let mut failures = failures.lock().unwrap().clone();
        if let Err(error) = result {
            failures.push(format!("could not run: {error}"));
        }
        // The runner drops the app before returning; `progress` then holds
        // the last step reached.
        let reached = progress_after.load(Ordering::SeqCst);
        if reached < total {
            failures.push(format!("stopped at step {reached} of {total}"));
        }
        failures
    }
}
