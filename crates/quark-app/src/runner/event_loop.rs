use super::*;

/// Open a window and drive `app` until it exits or its last window closes.
pub fn run<A: App>(app: A, options: WindowOptions) -> Result<(), RunError> {
    if options.panic_hook {
        crate::panic_hook::install(&options.title);
    }
    #[cfg(any(feature = "profile-puffin", feature = "profile-tracy"))]
    crate::profile::start();
    let event_loop = event_loop()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let waker = Waker::new(event_loop.create_proxy());

    let mut runner = Runner::new(app, options, waker);
    #[cfg(target_os = "linux")]
    crate::platform::theme::watch(runner.events.clone());
    // Before the app finishes launching, so the URL or notification click
    // that launched it is caught.
    #[cfg(target_os = "macos")]
    crate::platform::deep_link::listen(&runner.events);
    #[cfg(all(target_os = "macos", feature = "notifications"))]
    crate::platform::notification::install(&runner.events);

    #[cfg(feature = "hot-reload")]
    {
        let pending = Arc::new(std::sync::atomic::AtomicBool::new(false));
        crate::hot_reload::connect(runner.waker.proxy().clone(), pending.clone());
        runner.hot_reload_pending = Some(pending);
    }

    event_loop.run_app(&mut runner)?;
    match runner.startup_failure.take() {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn event_loop() -> Result<EventLoop<()>, EventLoopError> {
    #[cfg_attr(not(all(windows, feature = "ui")), allow(unused_mut))]
    let mut builder = EventLoop::builder();
    // Win32 menu accelerators need TranslateAcceleratorW in the message
    // loop, which winit runs; this hook is its way in.
    #[cfg(all(windows, feature = "ui"))]
    {
        use winit::platform::windows::EventLoopBuilderExtWindows;
        builder.with_msg_hook(crate::platform::native_menu::translate_accelerator);
    }
    builder.build()
}

struct Runner<A> {
    app: A,
    /// The first window's options until `resumed` opens it.
    first_window: Option<WindowOptions>,
    windows: WindowTable<WindowEntry>,
    /// Created with the first window and shared by every later one.
    gpu: Option<GpuContext>,
    focused: Option<WindowHandle>,
    theme: Option<Theme>,
    capabilities: PlatformCapabilities,
    started: bool,
    text: AppText,
    waker: Waker,
    events: EventSink,
    app_events: Receiver<Posted>,
    clipboard: Clipboard,
    #[cfg(feature = "tray")]
    tray: Option<tray_icon::TrayIcon>,
    platform: PlatformState,
    flags: Flags,
    launch_at: Instant,
    startup_failure: Option<RunError>,
    accessibility_action_sender: Sender<(WindowId, ActionRequest)>,
    accessibility_actions: Receiver<(WindowId, ActionRequest)>,
    #[cfg(feature = "hot-reload")]
    hot_reload_pending: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// Presented frames left before exiting, from `QUARK_EXIT_AFTER_FRAMES`.
    /// Lets CI smoke runs start an example, draw, and quit on their own.
    /// Debug builds only, so a stray variable cannot close a shipped app.
    #[cfg(debug_assertions)]
    exit_after_frames: Option<u64>,
}

impl<A: App> Runner<A> {
    fn new(app: A, options: WindowOptions, waker: Waker) -> Self {
        let (accessibility_action_sender, accessibility_actions) = mpsc::channel();
        let text = AppText::new(&options.fonts);
        let (events, app_events) = EventSink::new(waker.clone());
        Self {
            app,
            first_window: Some(options),
            windows: WindowTable::default(),
            gpu: None,
            focused: None,
            theme: None,
            capabilities: PlatformCapabilities::default(),
            started: false,
            text,
            waker,
            events,
            app_events,
            clipboard: Clipboard::System(None),
            #[cfg(feature = "tray")]
            tray: None,
            platform: PlatformState::default(),
            flags: Flags::default(),
            launch_at: Instant::now(),
            startup_failure: None,
            accessibility_action_sender,
            accessibility_actions,
            #[cfg(feature = "hot-reload")]
            hot_reload_pending: None,
            #[cfg(debug_assertions)]
            exit_after_frames: std::env::var("QUARK_EXIT_AFTER_FRAMES")
                .ok()
                .and_then(|frames| frames.parse().ok()),
        }
    }

    fn create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        options: &WindowOptions,
    ) -> Result<WindowState, RunError> {
        let window = Arc::new(event_loop.create_window(window_attributes(options, event_loop))?);
        let size = window.inner_size();
        let scale_factor = window.scale_factor();
        let accessibility_state = Arc::new(AccessibilityState::default());
        let accessibility = AccessibilityAdapter::with_direct_handlers(
            event_loop,
            &window,
            AccessibilityActivation {
                state: Arc::clone(&accessibility_state),
                waker: self.waker.clone(),
            },
            AccessibilityActions {
                window: window.id(),
                sender: self.accessibility_action_sender.clone(),
                waker: self.waker.clone(),
            },
            AccessibilityDeactivation {
                state: Arc::clone(&accessibility_state),
            },
        );
        let mut renderer = match &self.gpu {
            Some(gpu) => Renderer::with_gpu(gpu, window.clone())?,
            None => {
                let renderer = Renderer::new(window.clone())?;
                self.gpu = Some(renderer.gpu().clone());
                renderer
            }
        };
        renderer.resize(size.width, size.height, scale_factor);
        #[cfg(target_os = "linux")]
        crate::platform::drag_out::window_created(&window);
        window.set_visible(true);
        position_traffic_lights(&window, options.traffic_lights);
        Ok(WindowState {
            renderer,
            accessibility,
            accessibility_state,
            window,
            input: InputNormalizer::new(scale_factor),
            scale_factor,
            surface_size: size,
            frame_clock: FrameClock::default(),
            #[cfg(feature = "devtools")]
            last_render: Default::default(),
            traffic_lights: options.traffic_lights,
            persist_key: options.persist_key.clone(),
            position: Default::default(),
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

    /// [`Self::default_window`], but never `closing`.
    fn default_window_except(&self, closing: WindowHandle) -> Option<WindowHandle> {
        self.focused
            .filter(|&handle| handle != closing)
            .filter(|&handle| self.windows.get(handle).is_some_and(|e| e.open().is_some()))
            .or_else(|| {
                self.windows
                    .iter()
                    .find(|&(handle, entry)| handle != closing && entry.open().is_some())
                    .map(|(handle, _)| handle)
            })
    }

    fn with_event_cx(
        &mut self,
        event_loop: &ActiveEventLoop,
        window: Option<WindowHandle>,
        f: impl FnOnce(&mut A, &mut EventContext),
    ) {
        self.call_app(window, f);
        self.apply_window_changes(event_loop);
    }

    /// Run an app callback without then applying the window changes it
    /// queued: for callbacks made while applying them.
    fn call_app(
        &mut self,
        window: Option<WindowHandle>,
        f: impl FnOnce(&mut A, &mut EventContext),
    ) {
        let mut cx = EventContext {
            windows: &mut self.windows,
            window,
            text: &mut self.text,
            flags: &mut self.flags,
            clipboard: &mut self.clipboard,
            waker: &self.waker,
            events: &self.events,
            theme: self.theme,
            capabilities: self.capabilities,
            #[cfg(feature = "test-support")]
            headless: false,
            elapsed: self.launch_at.elapsed(),
            #[cfg(feature = "tray")]
            tray: &mut self.tray,
            platform: &mut self.platform,
        };
        f(&mut self.app, &mut cx);
    }

    /// Open pending windows and close requested ones. Runs after every app
    /// callback, since contexts can only queue these. Takes one change at a
    /// time, telling the app of each, because those callbacks can queue
    /// more.
    fn apply_window_changes(&mut self, event_loop: &ActiveEventLoop) {
        loop {
            let pending = self
                .windows
                .iter()
                .find_map(|(handle, entry)| Some((handle, entry.pending()?.clone())));
            if let Some((handle, options)) = pending {
                self.open_pending(event_loop, handle, &options);
                if self.startup_failure.is_some() {
                    return;
                }
                continue;
            }
            // In request order: a quit closes windows in table order.
            if self.flags.close.is_empty() {
                break;
            }
            let (handle, reason) = self.flags.close.remove(0);
            self.close(handle, reason);
        }

        if self.started && self.windows.is_empty() && !self.flags.keep_running_without_windows {
            event_loop.exit();
        }
    }

    /// Tell the app `handle` closes for `reason` while its placement is
    /// still readable, then drop it. Stale handles are ignored.
    fn close(&mut self, handle: WindowHandle, reason: CloseReason) {
        let Some(entry) = self.windows.get(handle) else {
            return;
        };
        if let Some(state) = entry.open() {
            state.persist();
        }
        if self.focused == Some(handle) {
            self.focused = None;
        }
        let window = self.default_window_except(handle);
        self.call_app(window, |app, cx| {
            app.app_event(
                AppEvent::WindowClosed {
                    window: handle,
                    reason,
                },
                cx,
            )
        });
        let Some(entry) = self.windows.remove(handle) else {
            return;
        };
        self.text.layouts.remove_scope(handle.scope_id());
        #[cfg(target_os = "linux")]
        if let Some(state) = entry.open() {
            crate::platform::drag_out::window_destroyed(&state.window);
        }
        drop(entry);
    }

    fn open_pending(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: WindowHandle,
        options: &WindowOptions,
    ) {
        let error = match self.create_window(event_loop, options) {
            Ok(mut state) => {
                self.platform.window_opened(&state.window);
                state.refresh_position();
                if let Some(entry) = self.windows.get_mut(handle) {
                    *entry = WindowEntry::Open(Box::new(state));
                }
                self.flags.redraw.push(handle);
                // The first window's comes after `init`.
                if self.started {
                    self.call_app(Some(handle), |app, cx| {
                        app.app_event(AppEvent::WindowOpened(handle), cx)
                    });
                }
                return;
            }
            Err(error) => error,
        };
        tracing::error!("could not open a window: {error}");
        if !self.started {
            self.windows.remove(handle);
            self.startup_failure = Some(error);
            event_loop.exit();
            return;
        }
        self.close(handle, CloseReason::OpenFailed);
    }

    fn redraw(&mut self, handle: WindowHandle) {
        let Some(state) = self.windows.get_mut(handle).and_then(WindowEntry::open_mut) else {
            return;
        };
        let renderer = &mut state.renderer;
        let timing = state.frame_clock.tick(Instant::now(), self.launch_at);
        self.text.begin_frame(handle.scope_id());
        let scale = state.scale_factor as f32;
        let text_metrics = logical_metrics(renderer.text_metrics(&mut self.text.system), scale);
        let mut cx = FrameContext {
            window: handle,
            size: state.surface_size,
            scale_factor: state.scale_factor,
            text_metrics,
            text: &mut self.text,
            timing,
            flags: &mut self.flags,
            waker: &self.waker,
            ime: FrameIme::default(),
            accessibility_active: state.accessibility_state.is_active(),
            #[cfg(feature = "devtools")]
            last_render: state.last_render,
        };

        // Through subsecond, a hot patch to the app's frame code takes effect
        // on the next frame.
        #[cfg(feature = "hot-reload")]
        let scene = subsecond::call(|| self.app.frame(&mut cx));
        #[cfg(not(feature = "hot-reload"))]
        let scene = {
            profile_scope!("frame");
            self.app.frame(&mut cx)
        };
        let mut scene = scene;
        let ime = cx.ime;
        if ime.reset {
            state.window.set_ime_allowed(false);
        }
        if let Some(allowed) = ime.allowed.or(ime.reset.then_some(true)) {
            state.window.set_ime_allowed(allowed);
        }
        if let Some((x, y, width, height)) = ime.cursor_area {
            state.window.set_ime_cursor_area(
                LogicalPosition::new(f64::from(x), f64::from(y)),
                LogicalSize::new(f64::from(width), f64::from(height)),
            );
        }

        scene_to_physical(&mut scene, scale);
        let time = timing.elapsed.as_secs_f32();
        let rendered = {
            profile_scope!("render");
            renderer.render(&scene, &mut self.text.system, time)
        };
        self.app.recycle_scene(handle, scene);
        #[cfg(debug_assertions)]
        if rendered.is_ok()
            && let Some(left) = &mut self.exit_after_frames
        {
            *left = left.saturating_sub(1);
            if *left == 0 {
                // stderr rather than tracing: smoke runs install no subscriber.
                eprintln!("QUARK_EXIT_AFTER_FRAMES reached; exiting");
                self.flags.exit_requested = true;
            }
        }
        match rendered {
            #[cfg(feature = "devtools")]
            Ok(stats) => state.last_render = stats,
            #[cfg(not(feature = "devtools"))]
            Ok(_) => {}
            // Nothing reached the screen; try again on the next pass.
            Err(RenderError::SurfaceReconfigured) => {
                tracing::debug!("surface reconfigured; redrawing");
                self.flags.redraw.push(handle);
            }
            Err(RenderError::SurfaceTimeout) => {
                tracing::debug!("surface acquire timed out; skipping a frame");
                self.flags.redraw.push(handle);
            }
            Err(RenderError::OutOfMemory) => {
                tracing::error!("the GPU is out of memory; exiting");
                self.flags.exit_requested = true;
            }
            Err(error) => tracing::error!("render failed: {error}"),
        }
        self.text.end_frame();
        #[cfg(any(feature = "profile-puffin", feature = "profile-tracy"))]
        crate::profile::finish_frame();

        let app = &mut self.app;
        if let Some(update) = state
            .accessibility_state
            .publish(|| app.accessibility(handle))
        {
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

    /// Ask the app whether `handle` may close for `reason`, and queue the
    /// close if so.
    fn ask_to_close(
        &mut self,
        event_loop: &ActiveEventLoop,
        handle: WindowHandle,
        reason: CloseReason,
    ) -> bool {
        let close = self.ask(handle, reason);
        if close {
            self.flags.close.push((handle, reason));
        }
        self.apply_window_changes(event_loop);
        close
    }

    /// [`App::close_requested`] for `handle`, without applying anything.
    fn ask(&mut self, handle: WindowHandle, reason: CloseReason) -> bool {
        let mut close = false;
        self.call_app(Some(handle), |app, cx| {
            close = app.close_requested(reason, cx);
        });
        close
    }

    /// Carry out a standard menu item on the focused window.
    #[cfg(feature = "ui")]
    fn perform_role(
        &mut self,
        event_loop: &ActiveEventLoop,
        role: crate::platform::menu::MenuRole,
    ) {
        use crate::platform::menu::MenuRole;

        if role == MenuRole::Quit {
            let open: Vec<WindowHandle> = self
                .windows
                .iter()
                .filter(|(_, entry)| entry.open().is_some())
                .map(|(handle, _)| handle)
                .collect();
            // Every window is asked before any closes, and one refusal
            // cancels the quit for all, so a quit never leaves some windows
            // closed and others open.
            let mut all = true;
            for &handle in &open {
                all &= self.ask(handle, CloseReason::Quit);
            }
            if all {
                self.flags
                    .close
                    .extend(open.into_iter().map(|handle| (handle, CloseReason::Quit)));
                self.flags.exit_requested = true;
            }
            self.apply_window_changes(event_loop);
            return;
        }
        let Some(handle) = self.default_window() else {
            return;
        };
        if let Some((key, shift)) = role.edit_key() {
            let chord = platform::edit_chord(key, shift);
            self.with_event_cx(event_loop, Some(handle), |app, cx| {
                app.event(InputEvent::KeyPress(chord.clone()), cx);
                app.event(InputEvent::KeyRelease(chord), cx);
            });
            return;
        }
        if role == MenuRole::CloseWindow {
            self.ask_to_close(event_loop, handle, CloseReason::User);
            return;
        }
        let Some(state) = self.windows.get(handle).and_then(WindowEntry::open) else {
            return;
        };
        match role {
            MenuRole::Minimize => state.window.set_minimized(true),
            MenuRole::Zoom => platform::toggle_maximized(&state.window),
            MenuRole::ToggleFullscreen => platform::toggle_fullscreen(&state.window),
            _ => {}
        }
    }

    fn process_app_events(&mut self, event_loop: &ActiveEventLoop) {
        // Hold events that beat the first window until `init` has run.
        if !self.started {
            return;
        }
        let posted: Vec<_> = self.app_events.try_iter().collect();
        for posted in posted {
            let event = match posted {
                Posted::App(event) => event,
                #[cfg(feature = "ui")]
                Posted::Role(role) => {
                    self.perform_role(event_loop, role);
                    continue;
                }
            };
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
        // A worker thread can wake the loop before the first window exists;
        // the app has not seen `init` yet, so it is not told.
        if !self.started {
            return;
        }
        let window = self.default_window();
        self.with_event_cx(event_loop, window, |app, cx| app.wake(cx));
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        let Some(options) = self.first_window.take() else {
            return;
        };
        // Again now that AppKit has installed its own Apple Event handlers.
        #[cfg(target_os = "macos")]
        crate::platform::deep_link::listen(&self.events);
        self.capabilities = PlatformCapabilities::detect(event_loop);
        let handle = self.windows.insert(WindowEntry::Pending(Box::new(options)));
        self.apply_window_changes(event_loop);
        if self.startup_failure.is_some() {
            return;
        }
        self.started = true;
        self.focused = Some(handle);
        self.with_event_cx(event_loop, Some(handle), |app, cx| {
            app.init(cx);
            app.app_event(AppEvent::WindowOpened(handle), cx);
        });
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
                self.ask_to_close(event_loop, handle, CloseReason::User);
            }
            WindowEvent::Resized(size) => {
                let scale_factor = state.window.scale_factor();
                state.sync_metrics(size, scale_factor);
                self.flags.redraw.push(handle);
                let size = size.to_logical::<f32>(scale_factor);
                let event = AppEvent::WindowResized {
                    window: handle,
                    size: (size.width, size.height),
                };
                self.with_event_cx(event_loop, Some(handle), |app, cx| app.app_event(event, cx));
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = state.window.inner_size();
                state.sync_metrics(size, scale_factor);
                self.flags.redraw.push(handle);
                let event = AppEvent::WindowScaleChanged {
                    window: handle,
                    scale_factor,
                };
                self.with_event_cx(event_loop, Some(handle), |app, cx| app.app_event(event, cx));
            }
            WindowEvent::Moved(position) => {
                state.refresh_position();
                let event = AppEvent::WindowMoved {
                    window: handle,
                    position: state.desktop_position(position),
                };
                self.with_event_cx(event_loop, Some(handle), |app, cx| app.app_event(event, cx));
            }
            WindowEvent::ActivationTokenDone { token, .. } => {
                let event = AppEvent::ActivationToken {
                    window: handle,
                    token: token.into_raw(),
                };
                self.with_event_cx(event_loop, Some(handle), |app, cx| app.app_event(event, cx));
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
        // Windows still open close for the quit, each told while its
        // placement is readable. No window changes apply from here on, so
        // windows these callbacks open are never created.
        let open: Vec<WindowHandle> = self
            .windows
            .iter()
            .filter(|(_, entry)| entry.open().is_some())
            .map(|(handle, _)| handle)
            .collect();
        for handle in open {
            if self.started {
                self.close(handle, CloseReason::Quit);
            } else if let Some(state) = self.windows.get(handle).and_then(WindowEntry::open) {
                state.persist();
            }
        }
        // Release the windows and the GPU while the display connection is
        // still open: `run_app` closes it on return, and wgpu's GL backend
        // then segfaults in eglTerminate talking to the closed wl_display.
        self.windows = WindowTable::default();
        self.gpu = None;
        #[cfg(target_os = "linux")]
        crate::platform::drag_out::shutdown();
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

        // Assistive tech just connected: draw so the frame publishes a tree.
        for (_, entry) in self.windows.iter() {
            if let Some(state) = entry.open()
                && state.accessibility_state.take_activation()
            {
                state.window.request_redraw();
            }
        }

        // Drain, not take: the request vectors keep their capacity.
        for (target, at) in self.flags.frame_at.drain(..) {
            for (handle, entry) in self.windows.iter_mut() {
                if let WindowEntry::Open(state) = entry
                    && target.is_none_or(|target| target == handle)
                {
                    state.frame_clock.schedule(at);
                }
            }
        }
        let now = Instant::now();
        let mut wake_at: Option<Instant> = None;
        for (_, entry) in self.windows.iter_mut() {
            let WindowEntry::Open(state) = entry else {
                continue;
            };
            match state.frame_clock.poll(now) {
                Ok(()) => state.window.request_redraw(),
                Err(Some(at)) => wake_at = Some(wake_at.map_or(at, |wake| wake.min(at))),
                Err(None) => {}
            }
        }
        event_loop.set_control_flow(match wake_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });

        let redraw = self.flags.redraw.drain(..);
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

/// The renderer measures in physical pixels; apps size text in points.
fn logical_metrics(metrics: TextMetrics, scale: f32) -> TextMetrics {
    TextMetrics {
        ui_font_size_px: metrics.ui_font_size_px / scale,
        ui_line_height_px: metrics.ui_line_height_px / scale,
        mono_font_size_px: metrics.mono_font_size_px / scale,
        mono_line_height_px: metrics.mono_line_height_px / scale,
        mono_char_width_px: metrics.mono_char_width_px / scale,
    }
}
