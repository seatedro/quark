use super::*;

/// Open a window and drive `app` until it exits or the window is closed.
pub fn run<A: App>(app: A, options: WindowOptions) -> Result<(), RunError> {
    if options.panic_hook {
        crate::panic_hook::install(&options.title);
    }
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let waker = Waker(event_loop.create_proxy());

    let mut runner = Runner::new(app, options, waker);

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
    options: WindowOptions,
    window: Option<WindowState>,
    waker: Waker,
    input: InputNormalizer,
    clipboard: Option<arboard::Clipboard>,
    flags: Flags,
    launch_at: Instant,
    startup_failure: Option<RunError>,
    accessibility_latest_tree: Arc<Mutex<TreeUpdate>>,
    accessibility_action_sender: Sender<ActionRequest>,
    accessibility_actions: Receiver<ActionRequest>,
    #[cfg(feature = "hot-reload")]
    hot_reload_pending: Option<Arc<std::sync::atomic::AtomicBool>>,
}

impl<A: App> Runner<A> {
    fn new(app: A, options: WindowOptions, waker: Waker) -> Self {
        let (accessibility_action_sender, accessibility_actions) = mpsc::channel();
        Self {
            app,
            options,
            window: None,
            waker,
            input: InputNormalizer::default(),
            clipboard: None,
            flags: Flags {
                needs_redraw: true,
                ..Flags::default()
            },
            launch_at: Instant::now(),
            startup_failure: None,
            accessibility_latest_tree: Arc::new(Mutex::new(empty_tree_update())),
            accessibility_action_sender,
            accessibility_actions,
            #[cfg(feature = "hot-reload")]
            hot_reload_pending: None,
        }
    }

    fn window_attributes(&self) -> WindowAttributes {
        let (width, height) = self.options.size;
        let mut attrs = Window::default_attributes()
            .with_title(self.options.title.clone())
            .with_inner_size(LogicalSize::new(width, height))
            .with_window_icon(self.options.icon.clone())
            // Shown once the renderer exists, so the first paint isn't blank.
            .with_visible(false);
        if let Some((width, height)) = self.options.min_size {
            attrs = attrs.with_min_inner_size(LogicalSize::new(width, height));
        }
        match self.options.chrome {
            WindowChrome::System => attrs,
            WindowChrome::Custom => custom_chrome(attrs),
        }
    }

    fn create_window(&mut self, event_loop: &ActiveEventLoop) -> Result<(), RunError> {
        let window = Arc::new(event_loop.create_window(self.window_attributes())?);
        let size = window.inner_size();
        let scale_factor = window.scale_factor();
        let accessibility = AccessibilityAdapter::with_direct_handlers(
            event_loop,
            &window,
            AccessibilityActivation {
                latest_tree: Arc::clone(&self.accessibility_latest_tree),
            },
            AccessibilityActions {
                sender: self.accessibility_action_sender.clone(),
                waker: self.waker.clone(),
            },
            AccessibilityDeactivation,
        );
        let mut renderer = Renderer::new(window.clone(), &self.options.fonts)?;
        renderer.resize(size.width, size.height, scale_factor);
        window.set_visible(true);
        position_traffic_lights(&window, self.options.traffic_lights);
        self.window = Some(WindowState {
            window,
            renderer,
            accessibility,
            scale_factor,
            surface_size: size,
            chrome: self.options.chrome,
            traffic_lights: self.options.traffic_lights,
        });
        Ok(())
    }

    fn with_event_cx(&mut self, f: impl FnOnce(&mut A, &mut EventContext)) {
        let Some(state) = self.window.as_mut() else {
            return;
        };
        let mut cx = EventContext {
            window: &state.window,
            renderer: &mut state.renderer,
            flags: &mut self.flags,
            clipboard: &mut self.clipboard,
            input: &self.input,
            waker: &self.waker,
            traffic_lights: state.traffic_lights,
        };
        f(&mut self.app, &mut cx);
    }

    fn sync_window_metrics(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
        if let Some(state) = self.window.as_mut() {
            state.sync_metrics(size, scale_factor);
        }
        self.flags.needs_redraw = true;
    }

    fn redraw(&mut self) {
        let Some(state) = self.window.as_mut() else {
            return;
        };
        let renderer = &mut state.renderer;
        self.flags.needs_redraw = false;
        let elapsed = self.launch_at.elapsed();
        let text_metrics = renderer.text_metrics();
        let mut cx = FrameContext {
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
            if let Ok(mut latest) = self.accessibility_latest_tree.lock() {
                *latest = update.clone();
            }
            state.accessibility.update_if_active(|| update);
        }
    }

    fn process_accessibility_actions(&mut self) {
        let requests: Vec<_> = self.accessibility_actions.try_iter().collect();
        for request in requests {
            self.with_event_cx(|app, cx| app.accessibility_action(request, cx));
        }
    }
}

impl<A: App> ApplicationHandler for Runner<A> {
    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: ()) {
        self.process_accessibility_actions();
        self.with_event_cx(|app, cx| app.wake(cx));
    }

    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        if let Err(error) = self.create_window(event_loop) {
            tracing::error!("startup failed: {error}");
            self.startup_failure = Some(error);
            event_loop.exit();
            return;
        }
        self.with_event_cx(|app, cx| app.init(cx));
        self.flags.needs_redraw = true;
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = self.window.as_mut() else {
            return;
        };
        if state.id() != window_id {
            return;
        }
        state.accessibility.process_event(&state.window, &event);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let scale_factor = state.window.scale_factor();
                self.sync_window_metrics(size, scale_factor);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = state.window.inner_size();
                self.sync_window_metrics(size, scale_factor);
            }
            WindowEvent::RedrawRequested => self.redraw(),
            event => {
                for event in self.input.normalize(event) {
                    self.with_event_cx(|app, cx| app.event(event, cx));
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.process_accessibility_actions();

        #[cfg(feature = "hot-reload")]
        if let Some(pending) = &self.hot_reload_pending
            && pending.swap(false, std::sync::atomic::Ordering::AcqRel)
        {
            self.flags.needs_redraw = true;
        }

        if self.flags.exit_requested {
            event_loop.exit();
            return;
        }

        let now = Instant::now();
        if self.flags.next_frame_at.is_some_and(|at| at <= now) {
            self.flags.next_frame_at = None;
            self.flags.needs_redraw = true;
        }
        event_loop.set_control_flow(match self.flags.next_frame_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });

        if self.flags.needs_redraw
            && let Some(state) = self.window.as_ref()
        {
            state.window.request_redraw();
        }
    }
}
