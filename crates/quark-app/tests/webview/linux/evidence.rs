//! Measurements and screenshots for the Linux backend, not pass/fail
//! checks: `webview_smoke --linux-evidence` on an X11 display with a window
//! manager. Prints one `measure:` line per phase and saves screenshots to
//! `$QUARK_WEBVIEW_SHOTS` (ImageMagick `import`) when set.
//!
//! Phases, each a fixed number of seconds on the runner's clock:
//!
//! - `idle`: no webview and nothing waking the app. Main-thread CPU time
//!   and context switches show the runner's own idle cost.
//! - `idle-latency`: no webview; a thread wakes the app every 10 ms with a
//!   timestamp, and the app records how late it sees each one.
//! - `modal-idle`: a loaded, idle page and nothing else. The extra wakeups
//!   are the GLib service deadline's.
//! - `modal-busy`: the page animates and spins timers while the 10 ms
//!   wakes run. Their latency against `idle-latency` shows whether GLib
//!   servicing starves the winit loop.
//! - `eval`: round trips of a trivial evaluation, back to back, idle and
//!   busy.
//!
//! Then a real click (xdotool) on a `target=_blank` link, which must be
//! reported as a blocked popup, and the focus handoff on close.

use std::collections::VecDeque;
use std::process::{Command, ExitCode};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use quark::scene::Scene;
use quark_app::platform::webview::{
    AsyncScript, BlockReason, DocumentId, Origin, OriginGuard, Url, WebViewEvent, WebViewHandle,
    WebViewOptions, WebWindowOptions,
};
use quark_app::{App, AppEvent, EventContext, FrameContext, WindowOptions};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

use super::super::fixture::{Fixture, Site, leaf_der};

const TITLE: &str = "linux evidence sign in";
const PHASE: Duration = Duration::from_secs(5);
const ROUND_TRIPS: usize = 100;

/// The page work for `modal-busy`: a full-page repaint every frame plus a
/// zero-delay timer storm.
const BUSY: &str = "const box = document.createElement('div'); document.body.append(box); \
    let n = 0; const frame = () => { n++; box.style.cssText = `position:fixed;inset:0;background:hsl(${n % 360} 80% 50%)`; \
    box.textContent = 'busy ' + n; requestAnimationFrame(frame); }; requestAnimationFrame(frame); \
    const spin = () => { const end = performance.now() + 4; while (performance.now() < end) {} setTimeout(spin, 0); }; spin(); return true;";

pub fn run() -> ExitCode {
    if std::env::var_os("DISPLAY").is_none() {
        println!("webview_smoke linux evidence: no X11 display; skipped");
        return ExitCode::SUCCESS;
    }
    let fixture = Fixture::start().expect("fixture binds its sites");
    quark_webview::testing::trust_leaf(leaf_der(), &["127.0.0.1", "localhost"]);
    let result = quark_app::run(
        Evidence {
            fixture,
            phase: Phase::Idle,
            phase_start: None,
            view: None,
            document: None,
            stats: Stats::default(),
            ticks: Arc::new(Mutex::new(VecDeque::new())),
            ticking: Arc::new(AtomicBool::new(false)),
            round_trip: None,
            round_trips: Vec::new(),
            parent: None,
        },
        WindowOptions {
            title: "linux webview evidence".into(),
            size: (480.0, 320.0),
            ..WindowOptions::default()
        },
    );
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("webview_smoke linux evidence: could not run: {error}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    IdleLatency,
    Opening,
    ModalIdle,
    EvalIdle,
    Busy,
    EvalBusy,
    Popup,
    Closing,
}

#[derive(Default)]
struct Stats {
    cpu: Duration,
    switches: u64,
    latencies: Vec<Duration>,
}

struct Evidence {
    fixture: Fixture,
    phase: Phase,
    phase_start: Option<(Instant, Duration, u64)>,
    view: Option<WebViewHandle>,
    document: Option<DocumentId>,
    stats: Stats,
    /// Timestamps of wakes the tick thread sent, not yet seen by the app.
    ticks: Arc<Mutex<VecDeque<Instant>>>,
    ticking: Arc<AtomicBool>,
    round_trip: Option<Instant>,
    round_trips: Vec<Duration>,
    parent: Option<u32>,
}

impl Evidence {
    fn begin(&mut self, phase: Phase, cx: &mut EventContext) {
        self.phase = phase;
        self.stats = Stats::default();
        self.phase_start = Some((Instant::now(), thread_cpu(), context_switches()));
        if !matches!(
            phase,
            Phase::Idle | Phase::IdleLatency | Phase::ModalIdle | Phase::Busy
        ) {
            return;
        }
        // Idle phases wake the app only once, at their end.
        let tick = matches!(phase, Phase::IdleLatency | Phase::Busy);
        self.ticks.lock().unwrap().clear();
        self.ticking.store(true, Ordering::Release);
        let (ticks, ticking, waker) = (
            Arc::clone(&self.ticks),
            Arc::clone(&self.ticking),
            cx.waker().clone(),
        );
        std::thread::spawn(move || {
            let end = Instant::now() + PHASE;
            while tick && Instant::now() < end {
                ticks.lock().unwrap().push_back(Instant::now());
                waker.wake();
                std::thread::sleep(Duration::from_millis(10));
            }
            if let Some(left) = end.checked_duration_since(Instant::now()) {
                std::thread::sleep(left);
            }
            ticking.store(false, Ordering::Release);
            waker.wake();
        });
    }

    fn report(&mut self) {
        let Some((start, cpu, switches)) = self.phase_start.take() else {
            return;
        };
        let wall = start.elapsed();
        let cpu = thread_cpu().saturating_sub(cpu);
        let switches = context_switches().saturating_sub(switches);
        let latencies = summary(&mut self.stats.latencies);
        println!(
            "measure: phase={:?} wall_ms={} main_cpu_ms={} main_cpu_pct={:.2} wakeups_per_s={:.1} wake_latency_ms={latencies}",
            self.phase,
            wall.as_millis(),
            cpu.as_millis(),
            100.0 * cpu.as_secs_f64() / wall.as_secs_f64(),
            switches as f64 / wall.as_secs_f64(),
        );
    }

    fn open(&mut self, cx: &mut EventContext, path: &str) {
        let app = Origin::parse(&self.fixture.origin(Site::App)).unwrap();
        let url = Url::parse(&self.fixture.url(Site::App, path, &[("title", "evidence")])).unwrap();
        let options =
            WebWindowOptions::new(WebViewOptions::new([app.clone()]).evaluation_origins([app]))
                .title(TITLE);
        self.view = Some(cx.webviews().open(url, options).expect("the modal opens"));
    }

    fn evaluate(&mut self, cx: &mut EventContext, body: &'static str) {
        let (Some(view), Some(document)) = (self.view, self.document) else {
            return;
        };
        let guard = OriginGuard::new(
            Origin::parse(&self.fixture.origin(Site::App)).unwrap(),
            document,
        );
        self.round_trip = Some(Instant::now());
        cx.webviews()
            .evaluate_script_event(view, AsyncScript::new(body), guard)
            .expect("the evaluation is accepted");
    }

    fn shot(&self, name: &str) {
        if let Some(dir) = std::env::var_os("QUARK_WEBVIEW_SHOTS") {
            let path = std::path::Path::new(&dir).join(format!("{name}.png"));
            let _ = Command::new("import")
                .args(["-window", "root"])
                .arg(path)
                .status();
        }
    }

    fn webview_event(&mut self, event: WebViewEvent, cx: &mut EventContext) {
        match (self.phase, event) {
            (Phase::Opening, WebViewEvent::PageLoadFinished { document, .. }) => {
                self.document = Some(document);
                self.shot("modal-getter");
                self.begin(Phase::ModalIdle, cx);
            }
            (
                Phase::EvalIdle | Phase::EvalBusy,
                WebViewEvent::EvaluationFinished { result, .. },
            ) => {
                if let Some(at) = self.round_trip.take() {
                    self.round_trips.push(at.elapsed());
                }
                if let Err(error) = result {
                    println!("measure: evaluation failed: {error:?}");
                }
                if self.round_trips.len() < ROUND_TRIPS {
                    return self.evaluate(cx, "return 1;");
                }
                let latencies = summary(&mut self.round_trips);
                println!(
                    "measure: phase={:?} eval_round_trip_ms={latencies}",
                    self.phase
                );
                if self.phase == Phase::EvalIdle {
                    self.evaluate(cx, BUSY);
                    self.begin(Phase::Busy, cx);
                } else {
                    self.shot("modal-busy");
                    if let Some(view) = self.view {
                        cx.webviews().close(view);
                    }
                    self.phase = Phase::Popup;
                }
            }
            (Phase::Busy, WebViewEvent::EvaluationFinished { .. }) => {}
            (Phase::Popup, WebViewEvent::Closed { .. }) => {
                self.document = None;
                self.open(cx, "/popup");
            }
            (Phase::Popup, WebViewEvent::PageLoadFinished { document, .. }) => {
                self.document = Some(document);
                self.shot("modal-popup-page");
                self.evaluate(
                    cx,
                    "const r = document.getElementById('popup').getBoundingClientRect(); \
                     return [Math.round(r.x + 5), Math.round(r.y + r.height / 2)];",
                );
            }
            (
                Phase::Popup,
                WebViewEvent::EvaluationFinished {
                    result: Ok(value), ..
                },
            ) => {
                // The page's own `window.open` on load has no user gesture,
                // so WebKit's blocker drops it before any policy call. A real
                // click is a gesture: it must come back as a blocked popup.
                let point = value.into_json();
                let (x, y) = (
                    point[0].as_i64().unwrap_or(0),
                    point[1].as_i64().unwrap_or(0),
                );
                let clicked = Command::new("sh")
                    .arg("-c")
                    .arg(format!(
                        "w=$(xdotool search --name '^{TITLE}$' | head -1) && xdotool windowactivate --sync $w \
                         && eval $(xdotool getwindowgeometry --shell $w) && xdotool mousemove $((X+{x})) $((Y+{y})) click 1"
                    ))
                    .status();
                println!("measure: clicked the target=_blank link at ({x}, {y}): {clicked:?}");
            }
            (Phase::Popup, WebViewEvent::NavigationBlocked { reason, .. }) => {
                println!(
                    "measure: popup click reported as blocked popup: {}",
                    reason == BlockReason::Popup
                );
                self.phase = Phase::Closing;
                if let Some(view) = self.view {
                    cx.webviews().close(view);
                }
            }
            (Phase::Closing, WebViewEvent::Closed { .. }) => {
                // The backend hands focus back through _NET_ACTIVE_WINDOW;
                // the window manager answers asynchronously.
                let (parent, done, waker) =
                    (self.parent, Arc::clone(&self.ticking), cx.waker().clone());
                done.store(true, Ordering::Release);
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(500));
                    let active = active_window();
                    println!(
                        "measure: parent {parent:?} active after close: {}",
                        parent.is_some() && active == parent
                    );
                    done.store(false, Ordering::Release);
                    waker.wake();
                });
                self.view = None;
            }
            _ => {}
        }
    }
}

impl App for Evidence {
    fn init(&mut self, cx: &mut EventContext) {
        self.parent = cx
            .window()
            .and_then(|window| match window.window_handle().ok()?.as_raw() {
                RawWindowHandle::Xlib(handle) => Some(handle.window as u32),
                RawWindowHandle::Xcb(handle) => Some(handle.window.get()),
                _ => None,
            });
        self.begin(Phase::Idle, cx);
    }

    fn frame(&mut self, _cx: &mut FrameContext) -> Scene {
        Scene::default()
    }

    fn wake(&mut self, cx: &mut EventContext) {
        let now = Instant::now();
        let seen: Vec<Instant> = self.ticks.lock().unwrap().drain(..).collect();
        self.stats
            .latencies
            .extend(seen.into_iter().map(|at| now - at));
        let ticking = self.ticking.load(Ordering::Acquire);
        match self.phase {
            Phase::Idle if !ticking => {
                self.report();
                self.begin(Phase::IdleLatency, cx);
            }
            Phase::IdleLatency if !ticking => {
                self.report();
                self.phase = Phase::Opening;
                self.open(cx, "/getter");
            }
            Phase::ModalIdle if !ticking => {
                self.report();
                self.begin(Phase::EvalIdle, cx);
                self.evaluate(cx, "return 1;");
            }
            Phase::Busy if !ticking => {
                self.report();
                self.round_trips.clear();
                self.begin(Phase::EvalBusy, cx);
                self.evaluate(cx, "return 1;");
            }
            Phase::Closing if self.view.is_none() && !ticking => cx.exit(),
            _ => {}
        }
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
        if let AppEvent::WebView(event) = event {
            self.webview_event(event, cx);
        }
    }
}

/// `p50/p99/max` of `samples` in milliseconds.
fn summary(samples: &mut [Duration]) -> String {
    if samples.is_empty() {
        return "none".into();
    }
    samples.sort();
    let at =
        |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize].as_secs_f64() * 1000.0;
    format!(
        "{:.2}/{:.2}/{:.2} (n={})",
        at(0.5),
        at(0.99),
        at(1.0),
        samples.len()
    )
}

/// CPU time of the calling (main) thread.
fn thread_cpu() -> Duration {
    let mut time = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: a valid out pointer for one timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, &mut time) };
    Duration::new(time.tv_sec as u64, time.tv_nsec as u32)
}

/// Voluntary plus involuntary context switches of the main thread: each
/// sleep-and-wake of the event loop is at least one.
fn context_switches() -> u64 {
    let status = std::fs::read_to_string(format!("/proc/self/task/{}/status", std::process::id()))
        .unwrap_or_default();
    status
        .lines()
        .filter(|line| line.contains("ctxt_switches"))
        .filter_map(|line| line.split_whitespace().last()?.parse::<u64>().ok())
        .sum()
}

/// `_NET_ACTIVE_WINDOW` on the root window.
fn active_window() -> Option<u32> {
    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};
    let (conn, screen) = x11rb::connect(None).ok()?;
    let root = conn.setup().roots[screen].root;
    let atom = conn
        .intern_atom(false, b"_NET_ACTIVE_WINDOW")
        .ok()?
        .reply()
        .ok()?
        .atom;
    let reply = conn
        .get_property(false, root, atom, AtomEnum::WINDOW, 0, 1)
        .ok()?
        .reply()
        .ok()?;
    reply.value32()?.next()
}
