use super::*;

/// Open a window and drive `app` until it exits or its last window closes.
pub fn run<A: App>(app: A, options: WindowOptions) -> Result<(), RunError> {
    if options.panic_hook {
        crate::panic_hook::install(&options.title);
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let waker = Waker(event_loop.create_proxy());

    let mut runner = Runner::new(app, options, waker);
    #[cfg(target_os = "linux")]
    crate::platform::theme::watch(runner.events.clone());

    #[cfg(feature = "hot-reload")]
    {
        let pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::hot_reload::connect(runner.waker.0.clone(), pending.clone());
        runner.hot_reload_pending = Some(pending);
    }

    event_loop.run_app(&mut runner)?;
    match runner.startup_failure.take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

struct Runner<A> {
    app: A,
    /// The first window's options until `resumed` opens it.
    first_window: Option<WindowOptions>,
    fonts: FontSettings,
    windows: WindowTable<WindowEntry>,
    focused: Option<WindowHandle>,
    theme: Option<Theme>,
    started: bool,
    waker: Waker,
    events: EventSink,
    app_events: Receiver<AppEvent>,
    clipboard: Option<arboard::Clipboard>,
    fallback_fonts: Option<FontSystem>,
    #[cfg(feature = "tray")]
    tray: Option<tray_icon::TrayIcon>,
    flags: Flags,
    launch_at: Instant,
    startup_failure: Option<RunError>,
    accessibility_action_sender: Sender<(WindowId, ActionRequest)>,
    accessibility_actions: Receiver<(WindowId, ActionRequest)>,
    #[cfg(feature = "hot-reload")]
    hot_reload_pending: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl<A: App> Runner<A> {
    fn new(app: A, options: WindowOptions, waker: Waker) -> Self {
        let (accessibility_action_sender, accessibility_actions) = mpsc::channel();
        let (events, app_events) = EventSink::new(waker.clone());
        Self {
            app,
            fonts: options.fonts.clone(),
            first_window: Some(options),
            windows: WindowTable::default(),
            focused: None,
            theme: None,
            started: false,
            waker,
            events,
            app_events,
            clipboard: None,
            fallback_fonts: None,
            #[cfg(feature = "tray")]
            tray: None,
            flags: Flags::default(),
            launch_at: Instant::now(),
            startup_failure: None,
            accessibility_action_sender,
            accessibility_actions,
            #[cfg(feature = "hot-reload")]
            hot_reload_pending: None,
        }
    }

    fn create_window(
        &self,
        event_loop: &ActiveEventLoop,
        options: &WindowOptions,
    ) -> Result<WindowState, RunError> {
        let window = Arc::new(event_loop.create_window(window_attributes(options, event_loop))?);
        let size = window.inner_size();
        let scale_factor = window.scale_factor();
        let accessibility_tree = Arc::new(Mutex::new(empty_tree_update()));
        let accessibility = AccessibilityAdapter::with_direct_handlers(
            event_loop,
            &window,
            AccessibilityActivation {
                latest_tree: Arc::clone(&accessibility_tree),
            },
            AccessibilityActions {
                window: window.id(),
                sender: self.accessibility_action_sender.clone(),
                waker: self.waker.clone(),
            },
            AccessibilityDeactivation,
        );
        let mut renderer = Renderer::new(window.clone(), &options.fonts)?;
        renderer.resize(size.width, size.height, scale_factor);
        window.set_visible(true);
        position_traffic_lights(&window, options.traffic_lights);
        Ok(WindowState {
            renderer,
            accessibility,
            accessibility_tree,
            window,
            input: InputNormalizer::default(),
            scale_factor,
            surface_size: size,
            traffic_lights: options.traffic_lights,
            persist_key: options.persist_key.clone(),
        })
    }

    fn handle_for(&self, id: WindowId) -> Option<WindowHandle> {
        self.windows
            .iter()
            .find(|(_, entry)| entry.open().is_some_and(|state| state.id() == id))
            .map(|(handle, _)| handle)
    }

    /// The window app-level events are delivered against: the focused one,
    /// else any open one.
    fn default_window(&self) -> Option<WindowHandle> {
        self.focused
            .filter(|&handle| self.windows.get(handle).is_some_and(|e| e.open().is_some()))
            .or_else(|| {
                self.windows
                    .iter()
                    .find(|(_, entry)| entry.open().is_some())
                    .map(|(handle, _)| handle)
            })
    }

    fn with_event_cx(
        &mut self,
        event_loop: &ActiveEventLoop,
        window: Option<WindowHandle>,
        f: impl FnOnce(&mut A, &mut EventContext),
    ) {
        let mut cx = EventContext {
            windows: &mut self.windows,
            window,
            flags: &mut self.flags,
            clipboard: &mut self.clipboard,
            fallback_fonts: &mut self.fallback_fonts,
            fonts: &self.fonts,
            waker: &self.waker,
            events: &self.events,
            theme: self.theme,
            #[cfg(feature = "tray")]
            tray: &mut self.tray,
        };
        f(&mut self.app, &mut cx);
        self.apply_window_changes(event_loop);
    }

    /// Open pending windows and close requested ones. Runs after every app
    /// callback, since contexts can only queue these. Takes one change at a
    /// time because the callbacks it makes can queue more.
    fn apply_window_changes(&mut self, event_loop: &ActiveEventLoop) {
        loop {
            let pending = self.windows.iter().find_map(|(handle, entry)| match entry {
                WindowEntry::Pending(options) => Some((handle, (**options).clone())),
                WindowEntry::Open(_) => None,
            });
            if let Some((handle, options)) = pending {
                self.open_pending(event_loop, handle, &options);
                if self.startup_failure.is_some() {
                    return;
                }
                continue;
            }
            let Some(handle) = self.flags.close.pop() else {
                break;
            };
            let Some(entry) = self.windows.remove(handle) else {
                continue;
            };
            if let Some(state) = entry.open() {
                state.persist();
            }
            if self.focused == Some(handle) {
                self.focused = None;
            }
            let window = self.default_window();
            self.with_event_cx(event_loop, window, |app, cx| {
                app.app_event(AppEvent::WindowClosed(handle), cx)
            });
        }

        if self.started && self.windows.is_empty() && !self.flags.keep_running_without_windows {
            event_loop.exit();
        }
    }

    fn open_pending(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: WindowHandle,
        options: &WindowOptions,
    ) {
        let error = match self.create_window(event_loop, options) {
            Ok(state) => {
                if let Some(entry) = self.windows.get_mut(handle) {
                    *entry = WindowEntry::Open(Box::new(state));
                }
                self.flags.redraw.push(handle);
                return;
            }
            Err(error) => error,
        };
        tracing::error!("could not open a window: {error}");
        self.windows.remove(handle);
        if !self.started {
            self.startup_failure = Some(error);
            event_loop.exit();
            return;
        }
        let window = self.default_window();
        self.with_event_cx(event_loop, window, |app, cx| {
            app.app_event(AppEvent::WindowOpenFailed(handle), cx)
        });
    }

    fn redraw(&mut self, handle: WindowHandle) {
        let Some(state) = self.windows.get_mut(handle).and_then(WindowEntry::open_mut) else {
            return;
        };
        let renderer = &mut state.renderer;
        let elapsed = self.launch_at.elapsed();
        let text_metrics = renderer.text_metrics();
        let mut cx = FrameContext {
            window: handle,
            size: state.surface_size,
            scale_factor: state.scale_factor,
            text_metrics,
            font_system: renderer.font_system_mut(),
            elapsed,
            flags: &mut self.flags,
            waker: &self.waker,
        };

        // Through subsecond, a hot patch to the app's frame code takes effect
        // on the next frame.
        #[cfg(feature = "hot-reload")]
        let scene = subsecond::call(|| self.app.frame(&mut cx));
        #[cfg(not(feature = "hot-reload"))]
        let scene = self.app.frame(&mut cx);

        if let Err(error) = renderer.render(&scene, elapsed.as_secs_f32()) {
            tracing::error!("render failed: {error}");
        }

        if let Some(update) = self.app.accessibility() {
            if let Ok(mut latest) = state.accessibility_tree.lock() {
                *latest = update.clone();
            }
            state.accessibility.update_if_active(|| update);
        }
    }

    fn process_accessibility_actions(&mut self, event_loop: &ActiveEventLoop) {
        let requests: Vec<_> = self.accessibility_actions.try_iter().collect();
        for (id, request) in requests {
            let Some(handle) = self.handle_for(id) else {
                continue;
            };
            self.with_event_cx(event_loop, Some(handle), |app, cx| {
                app.accessibility_action(request, cx)
            });
        }
    }

    /// Deliver a theme change once, however many windows or sources report
    /// it.
    fn theme_changed(
        &mut self,
        event_loop: &ActiveEventLoop,
        window: Option<WindowHandle>,
        theme: Theme,
    ) {
        if self.theme == Some(theme) {
            return;
        }
        self.theme = Some(theme);
        let window = window.or_else(|| self.default_window());
        self.with_event_cx(event_loop, window, |app, cx| {
            app.app_event(AppEvent::ThemeChanged(theme), cx)
        });
    }

    fn process_app_events(&mut self, event_loop: &ActiveEventLoop) {
        // Hold events that beat the first window until `init` has run.
        if !self.started {
            return;
        }
        let events: Vec<_> = self.app_events.try_iter().collect();
        for event in events {
            if let AppEvent::ThemeChanged(theme) = event {
                self.theme_changed(event_loop, None, theme);
                continue;
            }
            let window = self.default_window();
            self.with_event_cx(event_loop, window, |app, cx| app.app_event(event, cx));
        }
    }
}

impl<A: App> ApplicationHandler for Runner<A> {
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _event: ()) {
        self.process_accessibility_actions(event_loop);
        self.process_app_events(event_loop);
        let window = self.default_window();
        self.with_event_cx(event_loop, window, |app, cx| app.wake(cx));
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(options) = self.first_window.take() else {
            return;
        };
        let handle = self.windows.insert(WindowEntry::Pending(Box::new(options)));
        self.apply_window_changes(event_loop);
        if self.startup_failure.is_some() {
            return;
        }
        self.started = true;
        self.focused = Some(handle);
        self.with_event_cx(event_loop, Some(handle), |app, cx| app.init(cx));
        // macOS and Windows report the theme through the window; on Linux it
        // comes from the settings portal.
        if let Some(theme) = self
            .windows
            .get(handle)
            .and_then(WindowEntry::open)
            .and_then(|state| state.window.theme())
        {
            self.theme_changed(event_loop, Some(handle), theme);
        }
        self.flags.redraw_all = true;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(handle) = self.handle_for(window_id) else {
            return;
        };
        let Some(state) = self.windows.get_mut(handle).and_then(WindowEntry::open_mut) else {
            return;
        };
        state.accessibility.process_event(&state.window, &event);

        match event {
            WindowEvent::CloseRequested => {
                let mut close = false;
                self.with_event_cx(event_loop, Some(handle), |app, cx| {
                    close = app.close_requested(cx);
                });
                if close {
                    self.flags.close.push(handle);
                    self.apply_window_changes(event_loop);
                }
            }
            WindowEvent::Resized(size) => {
                let scale_factor = state.window.scale_factor();
                state.sync_metrics(size, scale_factor);
                self.flags.redraw.push(handle);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = state.window.inner_size();
                state.sync_metrics(size, scale_factor);
                self.flags.redraw.push(handle);
            }
            WindowEvent::RedrawRequested => self.redraw(handle),
            WindowEvent::ThemeChanged(theme) => {
                self.theme_changed(event_loop, Some(handle), theme);
            }
            event => {
                if let WindowEvent::Focused(true) = event {
                    self.focused = Some(handle);
                }
                for event in state.input.normalize(event) {
                    self.with_event_cx(event_loop, Some(handle), |app, cx| app.event(event, cx));
                }
            }
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        for (_, entry) in self.windows.iter() {
            if let Some(state) = entry.open() {
                state.persist();
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.process_accessibility_actions(event_loop);
        self.process_app_events(event_loop);

        #[cfg(feature = "hot-reload")]
        if let Some(pending) = &self.hot_reload_pending
            && pending.swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            self.flags.redraw_all = true;
        }

        if self.flags.exit_requested {
            event_loop.exit();
            return;
        }

        let now = Instant::now();
        if self.flags.next_frame_at.is_some_and(|at| at <= now) {
            self.flags.next_frame_at = None;
            self.flags.redraw_all = true;
        }
        event_loop.set_control_flow(match self.flags.next_frame_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });

        let redraw = std::mem::take(&mut self.flags.redraw);
        if std::mem::take(&mut self.flags.redraw_all) {
            for (_, entry) in self.windows.iter() {
                if let Some(state) = entry.open() {
                    state.window.request_redraw();
                }
            }
        } else {
            for handle in redraw {
                if let Some(state) = self.windows.get(handle).and_then(WindowEntry::open) {
                    state.window.request_redraw();
                }
            }
        }
    }
}
