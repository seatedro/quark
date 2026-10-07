use super::*;

/// A table slot: windows requested from an `EventContext` wait as `Pending`
/// until the runner gets back to the event loop and can create them.
pub(super) enum WindowEntry {
    Pending(Box<WindowOptions>),
    Open(Box<WindowState>),
}

impl WindowEntry {
    pub(super) fn open(&self) -> Option<&WindowState> {
        match self {
            Self::Open(state) => Some(state),
            Self::Pending(_) => None,
        }
    }

    pub(super) fn open_mut(&mut self) -> Option<&mut WindowState> {
        match self {
            Self::Open(state) => Some(state),
            Self::Pending(_) => None,
        }
    }
}

/// Everything the runner owns for one native window. Each window has its own
/// renderer (surface and frame state) on the runner's shared [`GpuContext`];
/// all of them borrow the runner's one [`AppText`] so layouts shaped for any
/// window rasterize in every window.
pub(super) struct WindowState {
    // Declared before `window` so the surface goes first on drop.
    pub(super) renderer: Renderer,
    pub(super) accessibility: AccessibilityAdapter,
    pub(super) accessibility_tree: Arc<Mutex<TreeUpdate>>,
    pub(super) window: Arc<Window>,
    pub(super) input: InputNormalizer,
    pub(super) scale_factor: f64,
    pub(super) surface_size: PhysicalSize<u32>,
    pub(super) frame_clock: FrameClock,
    pub(super) traffic_lights: Option<TrafficLights>,
    pub(super) persist_key: Option<String>,
}

impl WindowState {
    pub(super) fn id(&self) -> WindowId {
        self.window.id()
    }

    /// Save the window's geometry if it opted in. Failures only cost the
    /// saved placement, so they are logged and dropped.
    pub(super) fn persist(&self) {
        let Some(path) = self.persist_key.as_deref().and_then(state_path) else {
            return;
        };
        let size = self.window.inner_size();
        let geometry = WindowGeometry {
            position: self.window.outer_position().ok().map(|p| (p.x, p.y)),
            width: size.width,
            height: size.height,
            maximized: self.window.is_maximized(),
        };
        if let Err(error) = geometry.save_to(&path) {
            tracing::warn!("could not save window state to {}: {error}", path.display());
        }
    }

    /// Resize the surface to `size` and adopt `scale_factor`. Text layouts
    /// need no flush on a scale change: the layout cache keys every layout by
    /// its scale factor, so the next frame shapes at the new scale and the
    /// old layouts age out.
    pub(super) fn sync_metrics(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
        self.surface_size = size;
        self.scale_factor = scale_factor;
        self.input.set_scale_factor(scale_factor);
        self.renderer.resize(size.width, size.height, scale_factor);
        position_traffic_lights(&self.window, self.traffic_lights);
    }
}

/// The per-window frame clock: when the window last drew and when it must
/// draw next. Windows only redraw when something asked for a frame, so an
/// idle window costs nothing and an animating one does not wake the others.
#[derive(Debug, Default)]
pub(super) struct FrameClock {
    last_frame: Option<Instant>,
    next_frame_at: Option<Instant>,
}

/// One frame's reading of a window's [`FrameClock`].
#[derive(Debug, Clone, Copy)]
pub(super) struct FrameTiming {
    pub(super) now: Instant,
    /// `now` measured from the runner's start.
    pub(super) elapsed: Duration,
    /// Since the window's previous frame.
    pub(super) delta: Duration,
}

impl FrameClock {
    /// Ask for a frame at `at`; the earliest request wins.
    pub(super) fn schedule(&mut self, at: Instant) {
        self.next_frame_at = Some(self.next_frame_at.map_or(at, |next| next.min(at)));
    }

    /// The pending request, cleared when it is due at `now`.
    /// Returns `Ok(())` when a frame is due, else the time to wake for.
    pub(super) fn poll(&mut self, now: Instant) -> Result<(), Option<Instant>> {
        match self.next_frame_at {
            Some(at) if at <= now => {
                self.next_frame_at = None;
                Ok(())
            }
            next => Err(next),
        }
    }

    /// Start a frame at `now`.
    pub(super) fn tick(&mut self, now: Instant, launch: Instant) -> FrameTiming {
        let delta = self
            .last_frame
            .map_or(Duration::ZERO, |last| now.saturating_duration_since(last));
        self.last_frame = Some(now);
        FrameTiming {
            now,
            elapsed: now.saturating_duration_since(launch),
            delta,
        }
    }
}

pub(super) fn window_attributes(
    options: &WindowOptions,
    event_loop: &ActiveEventLoop,
) -> WindowAttributes {
    let (width, height) = options.size;
    let mut attrs = Window::default_attributes()
        .with_title(options.title.clone())
        .with_inner_size(LogicalSize::new(width, height))
        .with_window_icon(options.icon.clone())
        // Shown once the renderer exists, so the first paint isn't blank.
        .with_visible(false);
    if let Some(saved) = options
        .persist_key
        .as_deref()
        .and_then(state_path)
        .and_then(|path| WindowGeometry::load_from(&path))
    {
        let monitors: Vec<MonitorArea> = event_loop
            .available_monitors()
            .map(|monitor| {
                let position = monitor.position();
                let size = monitor.size();
                MonitorArea {
                    x: position.x,
                    y: position.y,
                    width: size.width,
                    height: size.height,
                }
            })
            .collect();
        let geometry = saved.clamped_to(&monitors);
        attrs = attrs
            .with_inner_size(PhysicalSize::new(geometry.width, geometry.height))
            .with_maximized(geometry.maximized);
        if let Some((x, y)) = geometry.position {
            attrs = attrs.with_position(PhysicalPosition::new(x, y));
        }
    }
    if let Some((width, height)) = options.min_size {
        attrs = attrs.with_min_inner_size(LogicalSize::new(width, height));
    }
    match options.chrome {
        WindowChrome::System => attrs,
        WindowChrome::Custom => custom_chrome(attrs),
    }
}

#[cfg(target_os = "macos")]
pub(super) fn custom_chrome(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS;
    attrs
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
        .with_movable_by_window_background(false)
}

#[cfg(not(target_os = "macos"))]
pub(super) fn custom_chrome(attrs: WindowAttributes) -> WindowAttributes {
    attrs.with_decorations(false)
}

#[cfg(target_os = "macos")]
pub(super) fn position_traffic_lights(window: &Window, lights: Option<TrafficLights>) {
    if let Some(lights) = lights {
        crate::macos_window::position_traffic_lights(window, lights.left_margin, lights.center_y);
    }
}

#[cfg(not(target_os = "macos"))]
pub(super) fn position_traffic_lights(_window: &Window, _lights: Option<TrafficLights>) {}
