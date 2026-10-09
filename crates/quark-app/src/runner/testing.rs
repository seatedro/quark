//! A runner with no event loop or native window, for driving an [`App`] from
//! tests: virtual windows that open, close, move, resize, rescale, and take
//! focus only when told, each with its own frame schedule; a fake clock
//! that moves only when told; an in-memory clipboard; and a waker that
//! raises a flag instead of waking a loop. Window lifecycle events, close
//! reasons, and geometry notifications arrive as the event loop delivers
//! them. [`crate::testing`] builds its public harness on it.

use super::*;
use crate::testing::ImeState;

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

/// An open window of the [`HeadlessRunner`]: what a native window and its
/// input normalizer would report, set by the test instead of a desktop.
pub(crate) struct VirtualWindow {
    pub(crate) title: String,
    /// Content size in logical points.
    pub(crate) size: (f32, f32),
    pub(crate) scale_factor: f64,
    pub(crate) pointer: Option<(f32, f32)>,
    pub(crate) modifiers: ModifiersState,
    /// The window's IME as its frames left it.
    pub(crate) ime: ImeState,
    /// Whether this is the active window, holding keyboard focus.
    pub(crate) focused: bool,
    /// Outer top-left corner on the virtual desktop. The virtual desktop is
    /// laid out like Windows and X11: desktop units are physical pixels, so
    /// the content area spans `size * scale_factor` units.
    pub(crate) position: DesktopPoint,
    /// The content area's corner from the outer corner: the decorations.
    /// None by default.
    pub(crate) client_offset: DesktopPoint,
    pub(crate) monitor: Option<MonitorInfo>,
    /// The smallest content size in logical points, from the options.
    pub(crate) min_size: Option<(f64, f64)>,
    pub(crate) surface: SurfaceState,
    /// Window drags started on it ([`EventContext::start_window_drag`]).
    pub(crate) window_drags: u32,
    /// Times frames were requested for, in ms since launch, sorted and
    /// deduplicated. One frame serves every request due by then, as the
    /// real frame clock does. A vector so a frame that schedules the next
    /// one reuses its capacity instead of allocating tree nodes.
    frames: Vec<u64>,
    last_frame_ms: Option<u64>,
    pub(crate) frames_drawn: u64,
    /// The last frame drawn, in points. The one before it went back to the
    /// app through [`App::recycle_scene`], as the event loop does once a
    /// frame is rendered.
    pub(crate) scene: Scene,
}

impl VirtualWindow {
    fn new(title: String, size: (f32, f32), scale_factor: f64, position: DesktopPoint) -> Self {
        Self {
            title,
            size,
            scale_factor,
            pointer: None,
            modifiers: ModifiersState::empty(),
            ime: ImeState::default(),
            focused: false,
            position,
            client_offset: (0.0, 0.0),
            monitor: None,
            min_size: None,
            surface: SurfaceState::headless(Default::default(), Default::default()),
            window_drags: 0,
            frames: Vec::new(),
            last_frame_ms: None,
            frames_drawn: 0,
            scene: Scene::default(),
        }
    }

    pub(crate) fn placement(&self, capabilities: PlatformCapabilities) -> WindowPlacement {
        let positions = capabilities.window_positions;
        WindowPlacement {
            inner_position: positions.then(|| self.client_origin().0),
            outer_position: positions.then_some(self.position),
            size: self.size,
            scale_factor: self.scale_factor,
            desktop_scale: self.scale_factor,
            monitor: self.monitor.clone(),
            maximized: false,
            minimized: Some(false),
            focused: self.focused,
        }
    }

    /// The content area's corner on the desktop, and desktop units per
    /// point.
    pub(crate) fn client_origin(&self) -> (DesktopPoint, f64) {
        let (x, y) = self.position;
        let (dx, dy) = self.client_offset;
        ((x + dx, y + dy), self.scale_factor)
    }

    /// Whether the desktop point is over the content area.
    fn contains(&self, point: DesktopPoint) -> bool {
        let (x, y) = desktop_to_local(self.client_origin(), point);
        (0.0..self.size.0).contains(&x) && (0.0..self.size.1).contains(&y)
    }

    /// The frame due soonest, in ms since launch.
    fn next_frame_ms(&self) -> Option<u64> {
        self.frames.first().copied()
    }
}

pub(crate) struct HeadlessRunner {
    windows: WindowTable<WindowEntry>,
    /// The window the runner starts with, which single-window callers
    /// target.
    main: WindowHandle,
    /// The active window, which app events and wakes are delivered against
    /// as the event loop delivers them against the focused window.
    focused: Option<WindowHandle>,
    text: AppText,
    flags: Flags,
    clipboard: Clipboard,
    waker: Waker,
    events: EventSink,
    app_events: Receiver<Posted>,
    theme: Option<Theme>,
    capabilities: PlatformCapabilities,
    /// Window creations left to fail.
    open_failures: usize,
    /// The scale factor windows open at.
    open_scale: f64,
    /// Open windows from bottom to top. Opening and focusing raise.
    stack: Vec<WindowHandle>,
    /// Where the desktop pointer is, once a test moved it there.
    desktop_pointer: Option<DesktopPoint>,
    /// The window under the desktop pointer.
    hovered: Option<WindowHandle>,
    /// The window a desktop button press went to. Like the platforms with
    /// desktop pointers, it gets all pointer input until the release, even
    /// outside it.
    grab: Option<WindowHandle>,
    /// The base the fake clock's milliseconds count from. Frame contexts
    /// hand out `Instant`s, so it has to be a real one; nothing reads the
    /// wall clock after construction.
    launch: Instant,
    /// Whether frames see assistive tech listening; on by default, since
    /// most tests read the tree a frame builds.
    pub(crate) accessibility_active: bool,
    now_ms: u64,
    #[cfg(feature = "tray")]
    tray: Option<tray_icon::TrayIcon>,
    platform: PlatformState,
}

impl HeadlessRunner {
    /// A runner with one focused `size` point window at `scale_factor`, at
    /// time zero, on a desktop with [`PlatformCapabilities::DESKTOP`].
    /// Windows the app opens later open at the same scale.
    pub(crate) fn new(size: (f32, f32), scale_factor: f64) -> Self {
        let waker = Waker::detached();
        let (events, app_events) = EventSink::new(waker.clone());
        let mut windows = WindowTable::default();
        let mut main = VirtualWindow::new("Test".to_owned(), size, scale_factor, (0.0, 0.0));
        main.focused = true;
        let main = windows.insert(WindowEntry::Virtual(Box::new(main)));
        Self {
            windows,
            main,
            focused: Some(main),
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
            capabilities: PlatformCapabilities::DESKTOP,
            open_failures: 0,
            open_scale: scale_factor,
            stack: vec![main],
            desktop_pointer: None,
            hovered: None,
            grab: None,
            launch: Instant::now(),
            accessibility_active: true,
            now_ms: 0,
            #[cfg(feature = "tray")]
            tray: None,
            platform: PlatformState::default(),
        }
    }

    pub(crate) fn now_ms(&self) -> u64 {
        self.now_ms
    }

    /// The window the runner started with.
    pub(crate) fn main_window(&self) -> WindowHandle {
        self.main
    }

    /// Every open window, in table order.
    pub(crate) fn windows(&self) -> Vec<WindowHandle> {
        self.windows.handles()
    }

    /// An open window, or `None` once it closed.
    pub(crate) fn window(&self, window: WindowHandle) -> Option<&VirtualWindow> {
        match self.windows.get(window)? {
            WindowEntry::Virtual(virtual_window) => Some(virtual_window),
            _ => None,
        }
    }

    fn window_mut(&mut self, window: WindowHandle) -> Option<&mut VirtualWindow> {
        match self.windows.get_mut(window)? {
            WindowEntry::Virtual(virtual_window) => Some(virtual_window),
            _ => None,
        }
    }

    /// The earliest frame requested of any window, in ms since launch.
    pub(crate) fn next_frame_ms(&self) -> Option<u64> {
        self.windows
            .iter()
            .filter_map(|(_, entry)| match entry {
                WindowEntry::Virtual(virtual_window) => virtual_window.next_frame_ms(),
                _ => None,
            })
            .min()
    }

    /// Whether the app called `exit`, quit, or closed its last window
    /// without [`EventContext::set_exit_when_last_window_closes`] turned
    /// off: when the event loop would stop.
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

    pub(crate) fn capabilities(&self) -> PlatformCapabilities {
        self.capabilities
    }

    /// Act as a desktop with `capabilities`: without window positions,
    /// placements carry none and moves are not reported, as on Wayland.
    pub(crate) fn set_capabilities(&mut self, capabilities: PlatformCapabilities) {
        self.capabilities = capabilities;
    }

    /// Make the next window creation fail, as when the platform refuses a
    /// window: the app gets [`AppEvent::WindowClosed`] with
    /// [`CloseReason::OpenFailed`] instead of [`AppEvent::WindowOpened`].
    /// Calls add up.
    pub(crate) fn fail_next_open(&mut self) {
        self.open_failures += 1;
    }

    /// Put `window` on `monitor`, or on no known display. Reported through
    /// [`EventContext::placement`]; the platform sends no event for it.
    pub(crate) fn set_monitor(&mut self, window: WindowHandle, monitor: Option<MonitorInfo>) {
        if let Some(virtual_window) = self.window_mut(window) {
            virtual_window.monitor = monitor;
        }
    }

    /// Resize `window`, in points: report it and ask for a frame.
    pub(crate) fn resize<A: App>(&mut self, app: &mut A, window: WindowHandle, size: (f32, f32)) {
        let Some(virtual_window) = self.window_mut(window) else {
            return;
        };
        virtual_window.size = size;
        self.flags.redraw.push(window);
        self.callback_in(window, app, |app, cx| {
            app.app_event(AppEvent::WindowResized { window, size }, cx)
        });
    }

    /// Move `window` to a display with another scale: report it and ask
    /// for a frame. The size in points and the pointer's point stay.
    pub(crate) fn set_scale_factor<A: App>(
        &mut self,
        app: &mut A,
        window: WindowHandle,
        scale_factor: f64,
    ) {
        let Some(virtual_window) = self.window_mut(window) else {
            return;
        };
        virtual_window.scale_factor = scale_factor;
        self.flags.redraw.push(window);
        self.callback_in(window, app, |app, cx| {
            let event = AppEvent::WindowScaleChanged {
                window,
                scale_factor,
            };
            app.app_event(event, cx)
        });
    }

    /// Move `window` on the desktop, in physical pixels, as the user
    /// dragging its title bar does. Reported only where windows have
    /// positions.
    pub(crate) fn move_window<A: App>(
        &mut self,
        app: &mut A,
        window: WindowHandle,
        position: DesktopPoint,
    ) {
        let reported = self.capabilities.window_positions;
        let Some(virtual_window) = self.window_mut(window) else {
            return;
        };
        virtual_window.position = position;
        if reported {
            self.callback_in(window, app, |app, cx| {
                app.app_event(AppEvent::WindowMoved { window, position }, cx)
            });
        }
    }

    /// Give `window` keyboard focus, raising it, or take focus away.
    /// Focusing a window first blurs the one that had focus, as desktops
    /// do.
    pub(crate) fn set_focus<A: App>(&mut self, app: &mut A, window: WindowHandle, focused: bool) {
        self.shift_focus(app, window, focused);
        self.settle(app);
    }

    /// [`Self::set_focus`] without applying what the callbacks asked for.
    fn shift_focus<A: App>(&mut self, app: &mut A, window: WindowHandle, focused: bool) {
        if self.window(window).is_none() {
            return;
        }
        if focused {
            let blurred = self.windows.iter().find_map(|(handle, entry)| match entry {
                WindowEntry::Virtual(other) if other.focused && handle != window => Some(handle),
                _ => None,
            });
            if let Some(blurred) = blurred {
                self.deliver_input(app, blurred, InputEvent::Focused(false));
            }
            self.focused = Some(window);
            self.stack.retain(|&other| other != window);
            self.stack.push(window);
        }
        self.deliver_input(app, window, InputEvent::Focused(focused));
    }

    /// Put the decorations' size on `window`: its content area then starts
    /// `offset` desktop units from its outer corner.
    pub(crate) fn set_client_offset(&mut self, window: WindowHandle, offset: DesktopPoint) {
        if let Some(virtual_window) = self.window_mut(window) {
            virtual_window.client_offset = offset;
        }
    }

    /// The topmost window whose content area is under `point`.
    fn window_at(&self, point: DesktopPoint) -> Option<WindowHandle> {
        self.stack
            .iter()
            .rev()
            .copied()
            .find(|&window| self.window(window).is_some_and(|w| w.contains(point)))
    }

    /// Move the desktop pointer to `at`, as a mouse does across windows:
    /// the window under it gets the motion in its own coordinates, with
    /// [`InputEvent::PointerLeft`] and [`InputEvent::PointerEntered`] as it
    /// crosses between windows. While a button pressed in a window is held,
    /// that window alone gets the motion, even outside it, as Windows,
    /// macOS, and X11 deliver it.
    pub(crate) fn desktop_pointer_move<A: App>(&mut self, app: &mut A, at: DesktopPoint) {
        self.desktop_pointer = Some(at);
        if self.grab.is_none() {
            self.update_hover(app, at);
        }
        if let Some(window) = self.grab.or(self.hovered)
            && let Some(origin) = self.window(window).map(VirtualWindow::client_origin)
        {
            let (x, y) = desktop_to_local(origin, at);
            self.deliver_input(app, window, InputEvent::PointerMoved { x, y });
        }
        self.settle(app);
    }

    /// Press or release `button` at the desktop pointer. A press goes to the
    /// window under the pointer, which keeps the pointer until the release.
    pub(crate) fn desktop_button<A: App>(
        &mut self,
        app: &mut A,
        button: winit::event::MouseButton,
        state: winit::event::ElementState,
    ) {
        let pressed = state == winit::event::ElementState::Pressed;
        let target = self.grab.or(self.hovered);
        if let Some(window) = target {
            self.deliver_input(app, window, InputEvent::PointerButton { button, state });
        }
        if pressed {
            self.grab = self.grab.or(target);
        } else {
            self.grab = None;
            if let Some(at) = self.desktop_pointer {
                self.update_hover(app, at);
            }
        }
        self.settle(app);
    }

    /// Track which window the desktop pointer is over, telling the one it
    /// left and the one it entered.
    fn update_hover<A: App>(&mut self, app: &mut A, at: DesktopPoint) {
        let under = self.window_at(at);
        if under == self.hovered {
            return;
        }
        if let Some(left) = std::mem::replace(&mut self.hovered, under) {
            self.deliver_input(app, left, InputEvent::PointerLeft);
        }
        if let Some(entered) = under {
            self.deliver_input(app, entered, InputEvent::PointerEntered);
        }
    }

    /// Ask the app to close `window` for `reason`, as its close button
    /// ([`CloseReason::User`]) does, and close it if the app agrees.
    /// Returns [`App::close_requested`]'s answer.
    pub(crate) fn request_close<A: App>(
        &mut self,
        app: &mut A,
        window: WindowHandle,
        reason: CloseReason,
    ) -> bool {
        if self.window(window).is_none() {
            return false;
        }
        self.callback_in(window, app, |app, cx| {
            let close = app.close_requested(reason, cx);
            if close {
                cx.flags.close.push((window, reason));
            }
            close
        })
    }

    /// Quit as the Quit menu item does: ask every window, and if all agree
    /// close them all and exit; one refusal cancels the quit. Returns
    /// whether all agreed.
    pub(crate) fn request_quit<A: App>(&mut self, app: &mut A) -> bool {
        let windows = self.windows();
        let mut all = true;
        for &window in &windows {
            all &= app.close_requested(CloseReason::Quit, &mut self.event_cx(Some(window)));
        }
        if all {
            self.flags.close.extend(
                windows
                    .into_iter()
                    .map(|window| (window, CloseReason::Quit)),
            );
            self.flags.exit_requested = true;
        }
        self.settle(app);
        all
    }

    /// Ask for a frame of `window` now, as the platform does when a window
    /// is exposed.
    pub(crate) fn request_frame(&mut self, window: WindowHandle) {
        let now = self.now_ms;
        if let Some(virtual_window) = self.window_mut(window) {
            schedule(&mut virtual_window.frames, now);
        }
    }

    /// Hand `event` to the app for `window`, keeping the window's pointer
    /// and modifiers in step as the platform layer does.
    pub(crate) fn input<A: App>(&mut self, app: &mut A, window: WindowHandle, event: InputEvent) {
        self.deliver_input(app, window, event);
        self.settle(app);
    }

    /// [`Self::input`] without applying what the callback asked for.
    fn deliver_input<A: App>(&mut self, app: &mut A, window: WindowHandle, event: InputEvent) {
        let Some(virtual_window) = self.window_mut(window) else {
            return;
        };
        match &event {
            InputEvent::PointerMoved { x, y } => virtual_window.pointer = Some((*x, *y)),
            InputEvent::PointerLeft => virtual_window.pointer = None,
            InputEvent::ModifiersChanged(modifiers) => virtual_window.modifiers = *modifiers,
            InputEvent::Focused(focused) => virtual_window.focused = *focused,
            _ => {}
        }
        app.event(event, &mut self.event_cx(Some(window)));
    }

    fn event_cx(&mut self, window: Option<WindowHandle>) -> EventContext<'_> {
        EventContext {
            windows: &mut self.windows,
            window,
            text: &mut self.text,
            flags: &mut self.flags,
            clipboard: &mut self.clipboard,
            waker: &self.waker,
            events: &self.events,
            theme: self.theme,
            capabilities: self.capabilities,
            headless: true,
            elapsed: Duration::from_millis(self.now_ms),
            #[cfg(feature = "tray")]
            tray: &mut self.tray,
            platform: &mut self.platform,
        }
    }

    /// Run an app callback with an event context for the main window.
    pub(crate) fn callback<A: App, R>(
        &mut self,
        app: &mut A,
        f: impl FnOnce(&mut A, &mut EventContext) -> R,
    ) -> R {
        self.callback_in(self.main, app, f)
    }

    /// Run an app callback with an event context for `window`, then open
    /// and close the windows it asked for and queue the frames it asked
    /// for, as the event loop does after every callback.
    pub(crate) fn callback_in<A: App, R>(
        &mut self,
        window: WindowHandle,
        app: &mut A,
        f: impl FnOnce(&mut A, &mut EventContext) -> R,
    ) -> R {
        let result = f(app, &mut self.event_cx(Some(window)));
        self.settle(app);
        result
    }

    /// Open and close the windows callbacks asked for and queue the frames
    /// they asked for, as the event loop does after every callback.
    fn settle<A: App>(&mut self, app: &mut A) {
        self.apply_window_changes(app);
        self.queue_requested_frames(false);
    }

    /// The window app events are delivered against: the focused one, else
    /// the first open one.
    fn default_window(&self) -> Option<WindowHandle> {
        self.focused
            .filter(|&window| self.window(window).is_some())
            .or_else(|| self.windows.iter().map(|(handle, _)| handle).next())
    }

    /// Open pending windows and close requested ones, one change at a time
    /// as the event loop does, telling the app of each.
    fn apply_window_changes<A: App>(&mut self, app: &mut A) {
        loop {
            let pending = self
                .windows
                .iter()
                .find_map(|(handle, entry)| Some((handle, entry.pending()?.clone())));
            if let Some((window, options)) = pending {
                if self.open_failures > 0 {
                    self.open_failures -= 1;
                    self.close(app, window, CloseReason::OpenFailed);
                    continue;
                }
                let (width, height) = options.size;
                let position = options.position.unwrap_or((0.0, 0.0));
                let mut opened = VirtualWindow::new(
                    options.title,
                    (width as f32, height as f32),
                    self.open_scale,
                    position,
                );
                // On the main window's display, as a window placed by the
                // window manager would be.
                opened.monitor = self.window(self.main).and_then(|w| w.monitor.clone());
                opened.min_size = options.min_size;
                opened.surface = SurfaceState::headless(options.background, options.corners);
                if let Some(entry) = self.windows.get_mut(window) {
                    *entry = WindowEntry::Virtual(Box::new(opened));
                }
                self.flags.redraw.push(window);
                self.stack.push(window);
                let mut cx = self.event_cx(Some(window));
                app.app_event(AppEvent::WindowOpened(window), &mut cx);
                if options.active {
                    self.shift_focus(app, window, true);
                }
                continue;
            }
            if !self.flags.moved.is_empty() {
                let window = self.flags.moved.remove(0);
                if let Some(position) = self.window(window).map(|w| w.position) {
                    let mut cx = self.event_cx(Some(window));
                    app.app_event(AppEvent::WindowMoved { window, position }, &mut cx);
                }
                continue;
            }
            // In request order: a quit closes windows in table order.
            if self.flags.close.is_empty() {
                break;
            }
            let (window, reason) = self.flags.close.remove(0);
            self.close(app, window, reason);
        }
        if self.windows.is_empty() && !self.flags.keep_running_without_windows {
            self.flags.exit_requested = true;
        }
    }

    /// Tell the app `window` closes for `reason` while its placement is
    /// still readable, then drop it. Stale handles are ignored.
    fn close<A: App>(&mut self, app: &mut A, window: WindowHandle, reason: CloseReason) {
        if self.windows.get(window).is_none() {
            return;
        }
        if self.focused == Some(window) {
            self.focused = None;
        }
        for held in [&mut self.hovered, &mut self.grab] {
            if *held == Some(window) {
                *held = None;
            }
        }
        self.stack.retain(|&other| other != window);
        let bound = self
            .default_window()
            .filter(|&other| other != window)
            .or_else(|| self.windows().into_iter().find(|&other| other != window));
        let mut cx = self.event_cx(bound);
        app.app_event(AppEvent::WindowClosed { window, reason }, &mut cx);
        self.windows.remove(window);
        self.text.layouts.remove_scope(window.scope_id());
    }

    /// Move the contexts' redraw and frame requests onto the windows'
    /// schedules. A frame asking to be redrawn right away gets the next
    /// vsync instead.
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
        let redraw_all = std::mem::take(&mut flags.redraw_all);
        // Drain, not take: the request vectors keep their capacity.
        for window in flags.redraw.drain(..) {
            if let Some(WindowEntry::Virtual(virtual_window)) = self.windows.get_mut(window) {
                schedule(&mut virtual_window.frames, soonest(now));
            }
        }
        for (window, entry) in self.windows.iter_mut() {
            let WindowEntry::Virtual(virtual_window) = entry else {
                continue;
            };
            if redraw_all {
                schedule(&mut virtual_window.frames, soonest(now));
            }
            for &(target, at) in &flags.frame_at {
                if target.is_none_or(|target| target == window) {
                    // Round up: a frame drawn early would see its animation
                    // unfinished.
                    let since = at.saturating_duration_since(self.launch);
                    let ms = since.as_nanos().div_ceil(1_000_000) as u64;
                    schedule(&mut virtual_window.frames, soonest(ms));
                }
            }
        }
        flags.frame_at.clear();
    }

    /// Draw a frame of `window` now, whether or not one was requested.
    /// Returns false when the window is not open.
    pub(crate) fn draw<A: App>(&mut self, app: &mut A, window: WindowHandle) -> bool {
        let now = self.now_ms;
        let Some(WindowEntry::Virtual(virtual_window)) = self.windows.get_mut(window) else {
            return false;
        };
        virtual_window.frames.retain(|&ms| ms > now);
        let delta = virtual_window
            .last_frame_ms
            .map_or(Duration::ZERO, |last| Duration::from_millis(now - last));
        virtual_window.last_frame_ms = Some(now);
        self.text.begin_frame(window.scope_id());
        let scale = virtual_window.scale_factor;
        let (width, height) = virtual_window.size;
        let elapsed = Duration::from_millis(now);
        let mut cx = FrameContext {
            window,
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
            last_render: Default::default(),
        };
        let scene = app.frame(&mut cx);
        let ime = cx.ime;
        virtual_window.ime.apply(ime);
        self.text.end_frame();
        virtual_window.frames_drawn += 1;
        let previous = std::mem::replace(&mut virtual_window.scene, scene);
        app.recycle_scene(window, previous);
        self.queue_requested_frames(true);
        true
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
        let Some(window) = self.default_window() else {
            return;
        };
        self.callback_in(window, app, |app, cx| app.app_event(event, cx));
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
                // Quit and Close Window ask first, as on the desktop; edit
                // roles type their keys. Virtual windows do not minimize,
                // zoom, or go fullscreen.
                #[cfg(feature = "ui")]
                Posted::Role(role) => {
                    use crate::platform::menu::MenuRole;
                    let Some(window) = self.default_window() else {
                        continue;
                    };
                    if let Some((key, shift)) = role.edit_key() {
                        let chord = platform::edit_chord(key, shift);
                        self.callback_in(window, app, |app, cx| {
                            app.event(InputEvent::KeyPress(chord.clone()), cx);
                            app.event(InputEvent::KeyRelease(chord), cx);
                        });
                    } else if role == MenuRole::Quit {
                        self.request_quit(app);
                    } else if role == MenuRole::CloseWindow {
                        self.request_close(app, window, CloseReason::User);
                    }
                }
            }
        }
        if let Some(window) = self.default_window() {
            self.callback_in(window, app, |app, cx| app.wake(cx));
        }
        true
    }

    /// The window with the earliest frame due by now; the first in table
    /// order on a tie.
    fn due_window(&self) -> Option<WindowHandle> {
        let mut due: Option<(u64, WindowHandle)> = None;
        for (window, entry) in self.windows.iter() {
            let WindowEntry::Virtual(virtual_window) = entry else {
                continue;
            };
            if let Some(ms) = virtual_window
                .next_frame_ms()
                .filter(|&ms| ms <= self.now_ms)
                && due.is_none_or(|(soonest, _)| ms < soonest)
            {
                due = Some((ms, window));
            }
        }
        due.map(|(_, window)| window)
    }

    /// Without moving the clock: deliver wakes until none are pending, and
    /// draw any window's frame due now, repeating until neither is left.
    /// Wakes come before frames so a frame shows every message sent before
    /// it.
    ///
    /// # Panics
    ///
    /// When the app never settles, for example by waking itself from every
    /// [`App::wake`].
    pub(crate) fn run_until_idle<A: App>(&mut self, app: &mut A) {
        for _ in 0..IDLE_STEP_LIMIT {
            if self.deliver_wake(app) {
                continue;
            }
            if let Some(window) = self.due_window() {
                self.draw(app, window);
                continue;
            }
            return;
        }
        panic!(
            "the app did not go idle in {IDLE_STEP_LIMIT} steps at {} ms: it keeps waking itself",
            self.now_ms
        );
    }

    /// Move the clock forward `ms`, stopping at each requested frame on the
    /// way to draw it, so animations and timers run in order at their own
    /// times, every window on its own schedule.
    pub(crate) fn advance<A: App>(&mut self, app: &mut A, ms: u64) {
        let target = self.now_ms + ms;
        self.run_until_idle(app);
        while let Some(next) = self.next_frame_ms().filter(|&next| next <= target) {
            self.now_ms = next;
            self.run_until_idle(app);
        }
        self.now_ms = target;
        self.run_until_idle(app);
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use winit::event::{ElementState, MouseButton};

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
        runner.draw(&mut app, runner.main_window());
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
        runner.request_frame(runner.main_window());

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
        runner.request_frame(runner.main_window());
        runner.waker.wake();

        runner.run_until_idle(&mut app);

        assert_eq!(app.log, ["wake 0", "frame 0"]);
    }

    /// A multi-window app that logs, by window name, the lifecycle, input,
    /// and frames it sees, and answers close requests as told.
    #[derive(Default)]
    struct Shell {
        log: Vec<String>,
        names: HashMap<WindowHandle, String>,
        /// Windows whose close requests it refuses, by name.
        refuse_close: Vec<&'static str>,
        /// Frames each window asks for after drawing, as delays in ms.
        frame_every: HashMap<String, u64>,
        /// Scenes handed back: the window each came back with, and the
        /// width of its one rect, which is the length of the name of the
        /// window it was built for.
        recycled: Vec<String>,
    }

    impl Shell {
        fn name(&self, window: Option<WindowHandle>) -> String {
            window
                .and_then(|window| self.names.get(&window))
                .map_or("-".to_owned(), Clone::clone)
        }

        fn log_in(&mut self, cx: &EventContext, line: String) {
            let window = self.name(cx.window_handle());
            self.log.push(format!("{window}: {line}"));
        }
    }

    impl App for Shell {
        fn frame(&mut self, cx: &mut FrameContext) -> Scene {
            let name = self.name(Some(cx.window_handle()));
            self.log
                .push(format!("{name}: frame {}", cx.elapsed().as_millis()));
            if let Some(&every) = self.frame_every.get(&name) {
                cx.request_frame_in(Duration::from_millis(every));
            }
            // Mark the scene with its window, to see where it comes back.
            let mut scene = Scene::default();
            scene.rect(quark::scene::RectPrimitive {
                rect: quark::Rect {
                    x: 0.0,
                    y: 0.0,
                    width: name.len() as f32,
                    height: 1.0,
                },
                color: quark::Color::rgba(0, 0, 0, 255),
            });
            scene
        }

        fn recycle_scene(&mut self, window: WindowHandle, scene: Scene) {
            let built_for = match scene.expanded().first() {
                Some(quark::scene::Primitive::Rect(rect)) => rect.rect.width,
                _ => 0.0,
            };
            let name = self.name(Some(window));
            self.recycled.push(format!("{name} <- {built_for}"));
        }

        fn event(&mut self, event: InputEvent, cx: &mut EventContext) {
            let line = match event {
                InputEvent::Focused(focused) => format!("focused {focused}"),
                InputEvent::PointerMoved { x, y } => {
                    let main = cx.pointer_position();
                    format!("pointer {x},{y} (context sees {main:?})")
                }
                other => format!("{other:?}"),
            };
            self.log_in(cx, line);
        }

        fn app_event(&mut self, event: AppEvent, cx: &mut EventContext) {
            let line = match event {
                AppEvent::WindowOpened(window) => format!("opened {}", self.name(Some(window))),
                AppEvent::WindowClosed { window, reason } => {
                    let placed = cx.placement(window).map(|placement| placement.size);
                    let name = self.name(Some(window));
                    format!("closed {name} {reason:?}, placement {placed:?}")
                }
                AppEvent::WindowMoved { window, position } => {
                    format!("moved {} to {position:?}", self.name(Some(window)))
                }
                AppEvent::WindowResized { window, size } => {
                    format!("resized {} to {size:?}", self.name(Some(window)))
                }
                AppEvent::WindowScaleChanged {
                    window,
                    scale_factor,
                } => format!("rescaled {} to {scale_factor}", self.name(Some(window))),
                other => format!("{other:?}"),
            };
            self.log_in(cx, line);
        }

        fn close_requested(&mut self, reason: CloseReason, cx: &mut EventContext) -> bool {
            let name = self.name(cx.window_handle());
            self.log
                .push(format!("{name}: close requested, {reason:?}"));
            !self.refuse_close.contains(&name.as_str())
        }
    }

    /// A runner with a main window named "main" and the app's first frame
    /// drawn, with the log cleared.
    fn shell() -> (HeadlessRunner, Shell) {
        let mut runner = HeadlessRunner::new((400.0, 300.0), 1.0);
        let mut app = Shell::default();
        app.names.insert(runner.main_window(), "main".to_owned());
        runner.request_frame(runner.main_window());
        runner.run_until_idle(&mut app);
        app.log.clear();
        (runner, app)
    }

    /// Open a 200x100 point window named `name` from a callback on the main
    /// window, without focus, beside the main window.
    fn open(runner: &mut HeadlessRunner, app: &mut Shell, name: &str) -> WindowHandle {
        runner.callback(app, |app, cx| {
            let window = cx.open_window(WindowOptions {
                title: name.to_owned(),
                size: (200.0, 100.0),
                position: Some((500.0, 0.0)),
                active: false,
                ..WindowOptions::default()
            });
            app.names.insert(window, name.to_owned());
            window
        })
    }

    // Docking reserves a window and moves panels in only once it exists:
    // the acknowledgement must come, bound to the new window, before the
    // window's first frame.
    #[test]
    fn an_opened_window_is_acknowledged_before_its_first_frame() {
        let (mut runner, mut app) = shell();

        open(&mut runner, &mut app, "tools");
        runner.run_until_idle(&mut app);

        assert_eq!(app.log, ["tools: opened tools", "tools: frame 0"]);
    }

    // A window the platform refuses must end its handle with a failure the
    // app can tell from a close, and never draw or report opening.
    #[test]
    fn a_failed_window_reports_open_failed_and_never_opens() {
        let (mut runner, mut app) = shell();
        runner.fail_next_open();

        let tools = open(&mut runner, &mut app, "tools");
        runner.run_until_idle(&mut app);

        assert_eq!(app.log, ["main: closed tools OpenFailed, placement None"]);
        assert!(!runner.windows().contains(&tools));
    }

    // Each way a window closes must carry its own reason, and only user and
    // quit closes may ask the app first: a floating window re-docks on a
    // user close but must keep its layout when the app quits. A refused
    // quit closes nothing, so no window is lost to a cancelled quit.
    #[test]
    fn closes_report_their_reason_and_ask_only_for_user_and_quit() {
        type Close = fn(&mut HeadlessRunner, &mut Shell, WindowHandle);
        let cases: [(&str, Close, &[&str]); 5] = [
            (
                "close button",
                |runner, app, tools| {
                    runner.request_close(app, tools, CloseReason::User);
                },
                &[
                    "tools: close requested, User",
                    "main: closed tools User, placement Some((200.0, 100.0))",
                ],
            ),
            (
                "close button refused",
                |runner, app, tools| {
                    app.refuse_close.push("tools");
                    runner.request_close(app, tools, CloseReason::User);
                },
                &["tools: close requested, User"],
            ),
            (
                "close_window",
                |runner, app, tools| runner.callback(app, |_, cx| cx.close_window(tools)),
                &["main: closed tools Program, placement Some((200.0, 100.0))"],
            ),
            (
                "quit",
                |runner, app, _| {
                    runner.request_quit(app);
                },
                &[
                    "main: close requested, Quit",
                    "tools: close requested, Quit",
                    "tools: closed main Quit, placement Some((400.0, 300.0))",
                    "-: closed tools Quit, placement Some((200.0, 100.0))",
                ],
            ),
            (
                "quit refused by one window",
                |runner, app, _| {
                    app.refuse_close.push("tools");
                    runner.request_quit(app);
                },
                &[
                    "main: close requested, Quit",
                    "tools: close requested, Quit",
                ],
            ),
        ];
        for (name, close, expected) in cases {
            let (mut runner, mut app) = shell();
            let tools = open(&mut runner, &mut app, "tools");
            runner.run_until_idle(&mut app);
            app.log.clear();

            close(&mut runner, &mut app, tools);

            assert_eq!(app.log, expected, "{name}");
        }
    }

    // A closed window's handle must be stale once its close was reported,
    // or a dock could keep sending panels to a window that is gone.
    #[test]
    fn a_closed_window_has_no_placement_after_its_close() {
        let (mut runner, mut app) = shell();
        let tools = open(&mut runner, &mut app, "tools");

        runner.request_close(&mut app, tools, CloseReason::User);

        let placement = runner.callback(&mut app, |_, cx| cx.placement(tools));
        assert_eq!(placement, None);
    }

    // An animating window must not wake an idle one: each window draws on
    // its own schedule.
    #[test]
    fn each_window_draws_on_its_own_schedule() {
        let (mut runner, mut app) = shell();
        open(&mut runner, &mut app, "tools");
        app.frame_every.insert("tools".to_owned(), 30);
        runner.run_until_idle(&mut app);
        app.log.clear();

        runner.advance(&mut app, 70);

        assert_eq!(app.log, ["tools: frame 30", "tools: frame 60"]);
    }

    // Scenes must come back to the app tagged with the window they were
    // built for, whatever window drew in between, so per-window buffers
    // are reused by the right window.
    #[test]
    fn recycled_scenes_name_the_window_they_were_built_for() {
        let (mut runner, mut app) = shell();
        let main = runner.main_window();
        let tools = open(&mut runner, &mut app, "tools");
        runner.run_until_idle(&mut app);
        app.recycled.clear();

        runner.draw(&mut app, tools);
        runner.draw(&mut app, main);

        assert_eq!(app.recycled, ["tools <- 5", "main <- 4"]);
    }

    // Activation moves between windows the way desktops do it: the old
    // window blurs before the new one gains focus, and placements say which
    // window is active.
    #[test]
    fn focusing_a_window_blurs_the_focused_one_first() {
        let (mut runner, mut app) = shell();
        let main = runner.main_window();
        let tools = open(&mut runner, &mut app, "tools");
        app.log.clear();

        runner.set_focus(&mut app, tools, true);

        assert_eq!(app.log, ["main: focused false", "tools: focused true"]);
        let focused = runner.callback(&mut app, |_, cx| {
            [main, tools].map(|w| cx.placement(w).unwrap().focused)
        });
        assert_eq!(focused, [false, true]);
    }

    // Docking rebuilds its drop targets when a window moves, resizes, or
    // changes scale, so each change must be reported for the window it
    // happened to.
    #[test]
    fn geometry_changes_are_reported_for_their_window() {
        type Change = fn(&mut HeadlessRunner, &mut Shell, WindowHandle);
        let cases: [(&str, Change, &str); 3] = [
            (
                "move",
                |runner, app, tools| runner.move_window(app, tools, (500.0, 60.0)),
                "tools: moved tools to (500.0, 60.0)",
            ),
            (
                "resize",
                |runner, app, tools| runner.resize(app, tools, (320.0, 240.0)),
                "tools: resized tools to (320.0, 240.0)",
            ),
            (
                "rescale",
                |runner, app, tools| runner.set_scale_factor(app, tools, 2.0),
                "tools: rescaled tools to 2",
            ),
        ];
        for (name, change, expected) in cases {
            let (mut runner, mut app) = shell();
            let tools = open(&mut runner, &mut app, "tools");
            runner.run_until_idle(&mut app);
            app.log.clear();

            change(&mut runner, &mut app, tools);

            assert_eq!(app.log, [expected], "{name}");
        }
    }

    // Where windows have no desktop position (Wayland), placements and
    // desktop pointer coordinates must not invent one and moves must go
    // unreported, or docking would place drop targets at made-up
    // coordinates.
    #[test]
    fn without_window_positions_placement_has_none_and_moves_are_silent() {
        let (mut runner, mut app) = shell();
        runner.set_capabilities(PlatformCapabilities::WAYLAND);
        let tools = open(&mut runner, &mut app, "tools");
        app.log.clear();

        runner.move_window(&mut app, tools, (500.0, 60.0));

        assert_eq!(app.log, Vec::<String>::new());
        let (placement, desktop) = runner.callback(&mut app, |_, cx| {
            (
                cx.placement(tools).unwrap(),
                cx.to_desktop(tools, (1.0, 1.0)),
            )
        });
        assert_eq!(desktop, None);
        assert_eq!(
            (placement.inner_position, placement.outer_position),
            (None, None)
        );
    }

    // Input sent to one window must reach the app bound to that window and
    // move only that window's pointer.
    #[test]
    fn input_reaches_only_the_window_it_is_sent_to() {
        let (mut runner, mut app) = shell();
        let main = runner.main_window();
        let tools = open(&mut runner, &mut app, "tools");
        app.log.clear();

        runner.input(&mut app, tools, InputEvent::PointerMoved { x: 5.0, y: 6.0 });
        let main_pointer = runner.window(main).unwrap().pointer;

        assert_eq!(
            app.log,
            ["tools: pointer 5,6 (context sees Some((5.0, 6.0)))"]
        );
        assert_eq!(main_pointer, None);
    }

    // A window opened mid-drag must not take focus from the window the
    // drag started in; an ordinary window does take it.
    #[test]
    fn only_a_window_opened_active_takes_focus() {
        for (active, expected) in [
            (
                true,
                &[
                    "tools: opened tools",
                    "main: focused false",
                    "tools: focused true",
                ][..],
            ),
            (false, &["tools: opened tools"][..]),
        ] {
            let (mut runner, mut app) = shell();

            runner.callback(&mut app, |app, cx| {
                let tools = cx.open_window(WindowOptions {
                    active,
                    ..WindowOptions::default()
                });
                app.names.insert(tools, "tools".to_owned());
            });

            assert_eq!(app.log, expected, "active {active}");
        }
    }

    // Desktop motion must reach the topmost window under the pointer, in
    // that window's coordinates, with leave and enter as it crosses: what
    // a dock needs to find the window under a dragged tab.
    #[test]
    fn desktop_motion_reaches_the_window_under_it_in_its_coordinates() {
        let (mut runner, mut app) = shell();
        let tools = open(&mut runner, &mut app, "tools");
        // Over the main window's right part, so the windows overlap.
        runner.move_window(&mut app, tools, (300.0, 200.0));
        app.log.clear();

        runner.desktop_pointer_move(&mut app, (50.0, 50.0));
        runner.desktop_pointer_move(&mut app, (350.0, 250.0));

        assert_eq!(
            app.log,
            [
                "main: PointerEntered",
                "main: pointer 50,50 (context sees Some((50.0, 50.0)))",
                "main: PointerLeft",
                "tools: PointerEntered",
                "tools: pointer 50,50 (context sees Some((50.0, 50.0)))",
            ]
        );
    }

    // During a drag the pressed window keeps the pointer outside its
    // bounds, as desktops deliver it, so a tab dragged out of a window can
    // be followed; the window under the pointer takes over at release.
    #[test]
    fn a_pressed_window_keeps_the_pointer_until_release() {
        let (mut runner, mut app) = shell();
        open(&mut runner, &mut app, "tools");
        runner.desktop_pointer_move(&mut app, (50.0, 50.0));
        runner.desktop_button(&mut app, MouseButton::Left, ElementState::Pressed);
        app.log.clear();

        runner.desktop_pointer_move(&mut app, (550.0, 50.0));
        runner.desktop_button(&mut app, MouseButton::Left, ElementState::Released);

        assert_eq!(
            app.log,
            [
                "main: pointer 550,50 (context sees Some((550.0, 50.0)))",
                "main: PointerButton { button: Left, state: Released }",
                "main: PointerLeft",
                "tools: PointerEntered",
            ]
        );
    }

    // A point in one window must land on the desktop by that window's
    // content corner (decorations included) and its own scale, and come
    // back unchanged, or cross-window drops land in the wrong place.
    #[test]
    fn desktop_points_follow_the_windows_content_corner_and_scale() {
        let (mut runner, mut app) = shell();
        let tools = open(&mut runner, &mut app, "tools");
        runner.move_window(&mut app, tools, (100.0, 50.0));
        runner.set_client_offset(tools, (0.0, 20.0));
        runner.set_scale_factor(&mut app, tools, 2.0);

        let (desktop, back, offset) = runner.callback(&mut app, |_, cx| {
            let desktop = cx.to_desktop(tools, (10.0, 5.0));
            let offset = cx.placement(tools).unwrap().client_offset();
            (desktop, cx.from_desktop(tools, desktop.unwrap()), offset)
        });

        assert_eq!(desktop, Some((120.0, 80.0)));
        assert_eq!(back, Some((10.0, 5.0)));
        assert_eq!(offset, Some((0.0, 20.0)));
    }

    // A torn-off window follows the pointer by its grab point: aligning
    // must account for decorations and scale, and the move must be
    // reported like any other.
    #[test]
    fn align_window_puts_the_hotspot_on_the_desktop_point() {
        let (mut runner, mut app) = shell();
        let tools = open(&mut runner, &mut app, "tools");
        runner.set_client_offset(tools, (4.0, 20.0));
        runner.set_scale_factor(&mut app, tools, 2.0);
        app.log.clear();

        let hotspot = runner.callback(&mut app, |_, cx| {
            cx.align_window(tools, (10.0, 5.0), (500.0, 300.0));
            cx.to_desktop(tools, (10.0, 5.0))
        });

        assert_eq!(hotspot, Some((500.0, 300.0)));
        assert_eq!(app.log, ["tools: moved tools to (476.0, 270.0)"]);
    }
}
