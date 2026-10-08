//! A runner with no event loop or native window, for driving an [`App`] from
//! tests: one window that never opens natively, a fake clock that moves only
//! when told, an in-memory clipboard, and a waker that raises a flag instead
//! of waking a loop. [`crate::testing`] builds its public harness on it.

use super::*;

/// How far a frame's own redraw request pushes the next frame: one 60 Hz
/// vsync, as on a real display. Drawing it at the same instant would loop
/// forever on any running animation.
pub(crate) const FRAME_INTERVAL_MS: u64 = 16;

/// Steps [`HeadlessRunner::run_until_idle`] takes before deciding the app
/// never settles.
const IDLE_STEP_LIMIT: usize = 10_000;

/// Add a frame at `ms` to the sorted, deduplicated `frames`.
fn schedule(frames: &mut Vec<u64>, ms: u64) {
    if let Err(at) = frames.binary_search(&ms) {
        frames.insert(at, ms);
    }
}

pub(crate) struct HeadlessRunner {
    windows: WindowTable<WindowEntry>,
    /// The one window. It stays pending: nothing natively opens it.
    window: WindowHandle,
    text: AppText,
    flags: Flags,
    clipboard: Clipboard,
    waker: Waker,
    events: EventSink,
    app_events: Receiver<Posted>,
    theme: Option<Theme>,
    /// The base the fake clock's milliseconds count from. Frame contexts
    /// hand out `Instant`s, so it has to be a real one; nothing reads the
    /// wall clock after construction.
    launch: Instant,
    /// Whether frames see assistive tech listening; on by default, since
    /// most tests read the tree a frame builds.
    pub(crate) accessibility_active: bool,
    now_ms: u64,
    last_frame_ms: Option<u64>,
    /// Times frames were requested for, in ms since launch, sorted and
    /// deduplicated. One frame serves every request due by then, as the
    /// real frame clock does. A vector so a frame that schedules the next
    /// one reuses its capacity instead of allocating tree nodes.
    frames: Vec<u64>,
    /// Window size in logical points.
    size: (f32, f32),
    pub(crate) input: HeadlessWindow,
    frames_drawn: u64,
    #[cfg(feature = "tray")]
    tray: Option<tray_icon::TrayIcon>,
    platform: PlatformState,
}

impl HeadlessRunner {
    /// A `size` point window at `scale_factor`, at time zero.
    pub(crate) fn new(size: (f32, f32), scale_factor: f64) -> Self {
        let waker = Waker::detached();
        let (events, app_events) = EventSink::new(waker.clone());
        let mut windows = WindowTable::default();
        let window = windows.insert(WindowEntry::Pending(Box::default()));
        Self {
            windows,
            window,
            // Vendored fonts only: fast, and the same on every host.
            text: AppText {
                system: TextSystem::vendored_only(&FontSettings::default()),
                layouts: LayoutCache::default(),
            },
            flags: Flags::default(),
            clipboard: Clipboard::Memory(MemoryClipboard::default()),
            waker,
            events,
            app_events,
            theme: None,
            launch: Instant::now(),
            accessibility_active: true,
            now_ms: 0,
            last_frame_ms: None,
            frames: Vec::new(),
            size,
            input: HeadlessWindow {
                pointer: None,
                modifiers: ModifiersState::empty(),
                scale_factor,
            },
            frames_drawn: 0,
            #[cfg(feature = "tray")]
            tray: None,
            platform: PlatformState::default(),
        }
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.now_ms
    }

    pub(crate) fn size(&self) -> (f32, f32) {
        self.size
    }

    pub(crate) fn frames_drawn(&self) -> u64 {
        self.frames_drawn
    }

    /// The earliest requested frame, in ms since launch.
    pub(crate) fn next_frame_ms(&self) -> Option<u64> {
        self.frames.first().copied()
    }

    pub(crate) fn exit_requested(&self) -> bool {
        self.flags.exit_requested
    }

    /// The text system frames are shaped with, for rendering them.
    #[cfg(feature = "headless-render")]
    pub(crate) fn text(&mut self) -> &mut AppText {
        &mut self.text
    }

    pub(crate) fn clipboard(&mut self) -> &mut MemoryClipboard {
        match &mut self.clipboard {
            Clipboard::Memory(memory) => memory,
            Clipboard::System(_) => unreachable!("the headless runner's clipboard is in memory"),
        }
    }

    /// Resize the window, in points, and ask for a frame.
    pub(crate) fn resize(&mut self, size: (f32, f32)) {
        self.size = size;
        schedule(&mut self.frames, self.now_ms);
    }

    /// Move the window to a display with another scale and ask for a frame.
    /// The pointer stays at the same point.
    pub(crate) fn set_scale_factor(&mut self, scale_factor: f64) {
        self.input.scale_factor = scale_factor;
        schedule(&mut self.frames, self.now_ms);
    }

    /// Ask for a frame now, as the platform does when a window is exposed.
    pub(crate) fn request_frame(&mut self) {
        schedule(&mut self.frames, self.now_ms);
    }

    fn event_cx(&mut self) -> EventContext<'_> {
        EventContext {
            windows: &mut self.windows,
            window: Some(self.window),
            text: &mut self.text,
            flags: &mut self.flags,
            clipboard: &mut self.clipboard,
            waker: &self.waker,
            events: &self.events,
            theme: self.theme,
            headless: Some(self.input),
            elapsed: Duration::from_millis(self.now_ms),
            #[cfg(feature = "tray")]
            tray: &mut self.tray,
            platform: &mut self.platform,
        }
    }

    /// Run an app callback with an event context, then queue the frames it
    /// asked for.
    pub(crate) fn callback<A: App, R>(
        &mut self,
        app: &mut A,
        f: impl FnOnce(&mut A, &mut EventContext) -> R,
    ) -> R {
        let result = f(app, &mut self.event_cx());
        self.queue_requested_frames(false);
        result
    }

    /// Move the contexts' redraw and frame requests onto the clock. A frame
    /// asking to be redrawn right away gets the next vsync instead.
    fn queue_requested_frames(&mut self, from_frame: bool) {
        let now = self.now_ms;
        let soonest = |ms: u64| {
            if from_frame && ms <= now {
                now + FRAME_INTERVAL_MS
            } else {
                ms.max(now)
            }
        };
        let flags = &mut self.flags;
        if !flags.redraw.is_empty() || std::mem::take(&mut flags.redraw_all) {
            flags.redraw.clear();
            schedule(&mut self.frames, soonest(now));
        }
        // Drain, not take: the request vector keeps its capacity.
        for (_, at) in flags.frame_at.drain(..) {
            // Round up: a frame drawn early would see its animation unfinished.
            let since = at.saturating_duration_since(self.launch);
            let ms = since.as_nanos().div_ceil(1_000_000) as u64;
            schedule(&mut self.frames, soonest(ms));
        }
        // The one window cannot close or open others; drop such requests.
        flags.close.clear();
    }

    /// Draw a frame now, whether or not one was requested.
    pub(crate) fn draw<A: App>(&mut self, app: &mut A) -> Scene {
        let now = self.now_ms;
        self.frames.retain(|&ms| ms > now);
        let delta = self
            .last_frame_ms
            .map_or(Duration::ZERO, |last| Duration::from_millis(now - last));
        self.last_frame_ms = Some(now);
        self.text.layouts.begin_frame_for(self.window.scope_id());
        let scale = self.input.scale_factor;
        let (width, height) = self.size;
        let elapsed = Duration::from_millis(now);
        let mut cx = FrameContext {
            window: self.window,
            size: PhysicalSize::new(
                (f64::from(width) * scale).round() as u32,
                (f64::from(height) * scale).round() as u32,
            ),
            scale_factor: scale,
            text_metrics: TextMetrics::default(),
            text: &mut self.text,
            timing: FrameTiming {
                now: self.launch + elapsed,
                elapsed,
                delta,
            },
            flags: &mut self.flags,
            waker: &self.waker,
            ime: FrameIme::default(),
            accessibility_active: self.accessibility_active,
            #[cfg(feature = "devtools")]
            last_render: Default::default(),
        };
        let scene = app.frame(&mut cx);
        self.frames_drawn += 1;
        self.queue_requested_frames(true);
        scene
    }

    /// Deliver an app event, dropping a theme change to the theme the app
    /// already has, as the event loop does.
    pub(crate) fn app_event<A: App>(&mut self, app: &mut A, event: AppEvent) {
        if let AppEvent::ThemeChanged(theme) = event {
            if self.theme == Some(theme) {
                return;
            }
            self.theme = Some(theme);
        }
        self.callback(app, |app, cx| app.app_event(event, cx));
    }

    /// If something woke the runner, deliver queued app events and then
    /// [`App::wake`], as the event loop does. Returns whether it was woken.
    fn deliver_wake<A: App>(&mut self, app: &mut A) -> bool {
        if !self.waker.take_detached_wake() {
            return false;
        }
        let posted: Vec<_> = self.app_events.try_iter().collect();
        for posted in posted {
            match posted {
                Posted::App(event) => self.app_event(app, event),
                // The one window cannot minimize or close; edit roles type
                // their keys as on the desktop.
                #[cfg(feature = "ui")]
                Posted::Role(role) => {
                    if let Some((key, shift)) = role.edit_key() {
                        let chord = platform::edit_chord(key, shift);
                        self.callback(app, |app, cx| {
                            app.event(InputEvent::KeyPress(chord.clone()), cx);
                            app.event(InputEvent::KeyRelease(chord), cx);
                        });
                    } else if role == crate::platform::menu::MenuRole::Quit {
                        self.flags.exit_requested = true;
                    }
                }
            }
        }
        self.callback(app, |app, cx| app.wake(cx));
        true
    }

    /// Without moving the clock: deliver wakes until none are pending, and
    /// draw any frame due now, repeating until neither is left. Wakes come
    /// before frames so a frame shows every message sent before it. Returns
    /// the last frame drawn.
    ///
    /// # Panics
    ///
    /// When the app never settles, for example by waking itself from every
    /// [`App::wake`].
    pub(crate) fn run_until_idle<A: App>(&mut self, app: &mut A) -> Option<Scene> {
        let mut last = None;
        for _ in 0..IDLE_STEP_LIMIT {
            if self.deliver_wake(app) {
                continue;
            }
            if self.next_frame_ms().is_some_and(|ms| ms <= self.now_ms) {
                last = Some(self.draw(app));
                continue;
            }
            return last;
        }
        panic!(
            "the app did not go idle in {IDLE_STEP_LIMIT} steps at {} ms: it keeps waking itself",
            self.now_ms
        );
    }

    /// Move the clock forward `ms`, stopping at each requested frame on the
    /// way to draw it, so animations and timers run in order at their own
    /// times. Returns the last frame drawn.
    pub(crate) fn advance<A: App>(&mut self, app: &mut A, ms: u64) -> Option<Scene> {
        let target = self.now_ms + ms;
        let mut last = self.run_until_idle(app);
        while let Some(next) = self.next_frame_ms().filter(|&next| next <= target) {
            self.now_ms = next;
            last = self.run_until_idle(app).or(last);
        }
        self.now_ms = target;
        self.run_until_idle(app).or(last)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app that logs its frames and wakes, and asks for frames as told.
    #[derive(Default)]
    struct Recorder {
        log: Vec<String>,
        /// Frames to request after each frame, as `request_frame_in` delays;
        /// `None` asks for one right away.
        after_frame: Vec<Option<u64>>,
        /// Redraw from the next wake.
        redraw_on_wake: bool,
    }

    impl App for Recorder {
        fn frame(&mut self, cx: &mut FrameContext) -> Scene {
            self.log.push(format!("frame {}", cx.elapsed().as_millis()));
            for delay in std::mem::take(&mut self.after_frame) {
                match delay {
                    Some(ms) => cx.request_frame_in(Duration::from_millis(ms)),
                    None => cx.request_frame(),
                }
            }
            Scene::default()
        }

        fn wake(&mut self, cx: &mut EventContext) {
            self.log.push(format!("wake {}", cx.elapsed().as_millis()));
            if std::mem::take(&mut self.redraw_on_wake) {
                cx.request_redraw();
            }
        }
    }

    // Timers must fire at their own times and in order, or animations and
    // debounces in tests run at the wrong moments.
    #[test]
    fn advance_draws_each_requested_frame_at_its_time_in_order() {
        let mut runner = HeadlessRunner::new((100.0, 100.0), 1.0);
        let mut app = Recorder {
            after_frame: vec![Some(50), Some(20), None],
            ..Recorder::default()
        };
        runner.draw(&mut app);
        app.log.clear();

        runner.advance(&mut app, 100);

        assert_eq!(app.log, ["frame 16", "frame 20", "frame 50"]);
        assert_eq!(runner.now_ms(), 100);
    }

    // A frame asking for another right away must not be redrawn at the
    // same instant, or a running animation never lets the runner idle.
    #[test]
    fn immediate_redraw_from_a_frame_waits_for_the_next_vsync() {
        let mut runner = HeadlessRunner::new((100.0, 100.0), 1.0);
        let mut app = Recorder {
            after_frame: vec![None],
            ..Recorder::default()
        };
        runner.request_frame();

        runner.run_until_idle(&mut app);

        assert_eq!(app.log, ["frame 0"]);
        assert_eq!(runner.next_frame_ms(), Some(FRAME_INTERVAL_MS));
    }

    // Messages sent before a frame must be in it: wakes drain first, and
    // the redraw a wake asks for is drawn in the same idle run.
    #[test]
    fn run_until_idle_delivers_wakes_before_drawing() {
        let mut runner = HeadlessRunner::new((100.0, 100.0), 1.0);
        let mut app = Recorder {
            redraw_on_wake: true,
            ..Recorder::default()
        };
        runner.request_frame();
        runner.waker.wake();

        runner.run_until_idle(&mut app);

        assert_eq!(app.log, ["wake 0", "frame 0"]);
    }
}
