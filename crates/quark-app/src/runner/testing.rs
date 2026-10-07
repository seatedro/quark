//! Contexts for driving an [`App`] in unit tests, with no event loop or
//! native window.

use super::*;

pub(crate) struct TestRunner {
    windows: WindowTable<WindowEntry>,
    text: AppText,
    flags: Flags,
    clipboard: Option<arboard::Clipboard>,
    waker: Waker,
    events: EventSink,
    _app_events: Receiver<AppEvent>,
    launch: Instant,
    /// Whether frames see assistive tech listening; on by default, since
    /// most tests read the tree a frame builds.
    pub(crate) accessibility_active: bool,
    #[cfg(feature = "tray")]
    tray: Option<tray_icon::TrayIcon>,
}

impl TestRunner {
    pub(crate) fn new() -> Self {
        let waker = Waker::detached();
        let (events, app_events) = EventSink::new(waker.clone());
        Self {
            windows: WindowTable::default(),
            // Vendored fonts only: fast, and the same on every host.
            text: AppText {
                system: TextSystem::vendored_only(&FontSettings::default()),
                layouts: LayoutCache::default(),
            },
            flags: Flags::default(),
            clipboard: None,
            waker,
            events,
            _app_events: app_events,
            launch: Instant::now(),
            accessibility_active: true,
            #[cfg(feature = "tray")]
            tray: None,
        }
    }

    /// A context for an app callback `elapsed_ms` after the runner started.
    pub(crate) fn event_cx(&mut self, elapsed_ms: u64) -> EventContext<'_> {
        EventContext {
            windows: &mut self.windows,
            window: None,
            text: &mut self.text,
            flags: &mut self.flags,
            clipboard: &mut self.clipboard,
            waker: &self.waker,
            events: &self.events,
            theme: None,
            elapsed: Duration::from_millis(elapsed_ms),
            #[cfg(feature = "tray")]
            tray: &mut self.tray,
        }
    }

    /// Draw a frame of `app` in a 400x300 point window at scale 1,
    /// `elapsed_ms` after the runner started.
    pub(crate) fn frame<A: App>(&mut self, app: &mut A, elapsed_ms: u64) -> Scene {
        self.text.layouts.begin_frame();
        let elapsed = Duration::from_millis(elapsed_ms);
        let mut cx = FrameContext {
            window: self.windows.insert(WindowEntry::Pending(Box::default())),
            size: PhysicalSize::new(400, 300),
            scale_factor: 1.0,
            text_metrics: TextMetrics::default(),
            text: &mut self.text,
            timing: FrameTiming {
                now: self.launch + elapsed,
                elapsed,
                delta: Duration::ZERO,
            },
            flags: &mut self.flags,
            waker: &self.waker,
            ime: FrameIme::default(),
            accessibility_active: self.accessibility_active,
            #[cfg(feature = "devtools")]
            last_render: Default::default(),
        };
        let scene = app.frame(&mut cx);
        let window = cx.window;
        self.windows.remove(window);
        scene
    }

    /// Whether a callback asked for a redraw since the last call.
    pub(crate) fn take_redraw(&mut self) -> bool {
        let redraw = !self.flags.redraw.is_empty() || self.flags.redraw_all;
        self.flags.redraw.clear();
        self.flags.redraw_all = false;
        redraw
    }
}
