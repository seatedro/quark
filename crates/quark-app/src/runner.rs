//! A single-window app runner: owns the winit event loop, the wgpu renderer,
//! and the accesskit adapter, and asks an [`App`] for a [`Scene`] whenever the
//! window needs repainting.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use accesskit::{
    ActionHandler, ActionRequest, ActivationHandler, DeactivationHandler, Node, NodeId, Role, Tree,
    TreeId, TreeUpdate,
};
use accesskit_winit::Adapter as AccessibilityAdapter;
use glyphon::FontSystem;
use quark::scene::Scene;
use quark_render::fonts::FontSettings;
use quark_render::{RenderError, Renderer, TextMetrics};
use winit::application::ApplicationHandler;
use winit::dpi::{LogicalSize, PhysicalPosition, PhysicalSize};
use winit::error::{EventLoopError, OsError};
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Icon, Window, WindowAttributes, WindowId};

use crate::input::{InputEvent, InputNormalizer};

/// An application driven by [`run`]. Scene and pointer coordinates are
/// physical pixels; use [`FrameContext::scale_factor`] to size content.
pub trait App: 'static {
    /// Called once after the window and renderer exist.
    fn init(&mut self, _cx: &mut EventContext) {}

    /// Build the scene for the next frame.
    fn frame(&mut self, cx: &mut FrameContext) -> Scene;

    fn event(&mut self, _event: InputEvent, _cx: &mut EventContext) {}

    /// Called after any [`Waker::wake`]. Wakes coalesce and may be spurious.
    fn wake(&mut self, _cx: &mut EventContext) {}

    /// The accessibility tree to publish after each frame, if it changed.
    fn accessibility(&mut self) -> Option<TreeUpdate> {
        None
    }

    fn accessibility_action(&mut self, _request: ActionRequest, _cx: &mut EventContext) {}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WindowChrome {
    /// Native title bar and borders.
    #[default]
    System,
    /// The app draws its own title bar: a transparent full-size title bar on
    /// macOS (traffic lights stay), no decorations elsewhere.
    Custom,
}

/// Where macOS traffic lights sit, in logical points from the window's
/// top-left. Reapplied on resize and title changes, which reset them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrafficLights {
    pub left_margin: f32,
    pub center_y: f32,
}

#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub title: String,
    /// Initial inner size in logical points.
    pub size: (f64, f64),
    pub min_size: Option<(f64, f64)>,
    pub chrome: WindowChrome,
    pub icon: Option<Icon>,
    pub fonts: FontSettings,
    pub traffic_lights: Option<TrafficLights>,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Quark".to_owned(),
            size: (1024.0, 768.0),
            min_size: None,
            chrome: WindowChrome::System,
            icon: None,
            fonts: FontSettings::default(),
            traffic_lights: None,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("could not start the event loop: {0}")]
    EventLoop(#[from] EventLoopError),
    #[error("could not create the native window: {0}")]
    Window(#[from] OsError),
    #[error("could not initialize the GPU renderer: {0}")]
    Renderer(#[from] RenderError),
}

/// Wakes the event loop from any thread and calls [`App::wake`].
#[derive(Debug, Clone)]
pub struct Waker(EventLoopProxy<()>);

impl Waker {
    pub fn wake(&self) {
        let _ = self.0.send_event(());
    }

    /// The raw proxy, for code that speaks winit directly.
    pub fn proxy(&self) -> &EventLoopProxy<()> {
        &self.0
    }
}

/// Open a window and drive `app` until it exits or the window is closed.
pub fn run<A: App>(app: A, options: WindowOptions) -> Result<(), RunError> {
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

/// Runner state that contexts mutate on the app's behalf.
#[derive(Debug, Default)]
struct Flags {
    needs_redraw: bool,
    next_frame_at: Option<Instant>,
    exit_requested: bool,
}

impl Flags {
    fn request_frame_at(&mut self, at: Instant) {
        self.next_frame_at = Some(self.next_frame_at.map_or(at, |next| next.min(at)));
    }
}

pub struct FrameContext<'a> {
    size: PhysicalSize<u32>,
    scale_factor: f64,
    text_metrics: TextMetrics,
    font_system: &'a mut FontSystem,
    elapsed: Duration,
    flags: &'a mut Flags,
    waker: &'a Waker,
}

impl FrameContext<'_> {
    /// Drawable size in physical pixels.
    pub fn size(&self) -> (f32, f32) {
        (
            self.size.width.max(1) as f32,
            self.size.height.max(1) as f32,
        )
    }

    pub fn logical_size(&self) -> (f32, f32) {
        let (width, height) = self.size();
        let scale = self.scale_factor as f32;
        (width / scale, height / scale)
    }

    pub fn scale_factor(&self) -> f32 {
        self.scale_factor as f32
    }

    /// Default UI and monospace metrics at the current scale factor.
    pub fn text_metrics(&self) -> TextMetrics {
        self.text_metrics
    }

    pub fn font_system(&mut self) -> &mut FontSystem {
        self.font_system
    }

    /// Time since the runner started.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Draw another frame right after this one, for animation.
    pub fn request_frame(&mut self) {
        self.flags.needs_redraw = true;
    }

    pub fn request_frame_at(&mut self, at: Instant) {
        self.flags.request_frame_at(at);
    }

    pub fn waker(&self) -> &Waker {
        self.waker
    }
}

pub struct EventContext<'a> {
    window: &'a Window,
    renderer: &'a mut Renderer,
    flags: &'a mut Flags,
    clipboard: &'a mut Option<arboard::Clipboard>,
    input: &'a InputNormalizer,
    waker: &'a Waker,
    traffic_lights: Option<TrafficLights>,
}

impl EventContext<'_> {
    pub fn request_redraw(&mut self) {
        self.flags.needs_redraw = true;
    }

    pub fn request_frame_at(&mut self, at: Instant) {
        self.flags.request_frame_at(at);
    }

    pub fn exit(&mut self) {
        self.flags.exit_requested = true;
    }

    /// Last pointer position in physical pixels, if the pointer is inside.
    pub fn pointer_position(&self) -> Option<(f32, f32)> {
        self.input.pointer_position()
    }

    pub fn modifiers(&self) -> ModifiersState {
        self.input.modifiers()
    }

    pub fn scale_factor(&self) -> f32 {
        self.window.scale_factor() as f32
    }

    pub fn font_system(&mut self) -> &mut FontSystem {
        self.renderer.font_system_mut()
    }

    pub fn set_cursor(&mut self, cursor: CursorIcon) {
        self.window.set_cursor(cursor);
    }

    /// winit leaves IME off by default; enable it while a text field has focus.
    pub fn set_ime_allowed(&mut self, allowed: bool) {
        self.window.set_ime_allowed(allowed);
    }

    /// Where the IME candidate window should appear, in physical pixels.
    pub fn set_ime_cursor_area(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.window.set_ime_cursor_area(
            PhysicalPosition::new(x as f64, y as f64),
            PhysicalSize::new(width as f64, height as f64),
        );
    }

    pub fn set_title(&mut self, title: &str) {
        self.window.set_title(title);
        position_traffic_lights(self.window, self.traffic_lights);
    }

    pub fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard()?.get_text().ok()
    }

    pub fn set_clipboard_text(&mut self, text: &str) {
        if let Some(clipboard) = self.clipboard() {
            let _ = clipboard.set_text(text);
        }
    }

    /// The native window, for operations the context doesn't wrap (drag,
    /// resize, minimize, maximize).
    pub fn window(&self) -> &Window {
        self.window
    }

    pub fn waker(&self) -> &Waker {
        self.waker
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            *self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }
}

struct Runner<A> {
    app: A,
    options: WindowOptions,
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    waker: Waker,
    input: InputNormalizer,
    clipboard: Option<arboard::Clipboard>,
    flags: Flags,
    launch_at: Instant,
    startup_failure: Option<RunError>,
    accessibility_adapter: Option<AccessibilityAdapter>,
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
            renderer: None,
            waker,
            input: InputNormalizer::default(),
            clipboard: None,
            flags: Flags {
                needs_redraw: true,
                ..Flags::default()
            },
            launch_at: Instant::now(),
            startup_failure: None,
            accessibility_adapter: None,
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
        let accessibility_adapter = AccessibilityAdapter::with_direct_handlers(
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
        renderer.resize(size.width, size.height, window.scale_factor());
        window.set_visible(true);
        position_traffic_lights(&window, self.options.traffic_lights);
        self.renderer = Some(renderer);
        self.accessibility_adapter = Some(accessibility_adapter);
        self.window = Some(window);
        Ok(())
    }

    fn with_event_cx(&mut self, f: impl FnOnce(&mut A, &mut EventContext)) {
        let (Some(window), Some(renderer)) = (self.window.as_deref(), self.renderer.as_mut())
        else {
            return;
        };
        let mut cx = EventContext {
            window,
            renderer,
            flags: &mut self.flags,
            clipboard: &mut self.clipboard,
            input: &self.input,
            waker: &self.waker,
            traffic_lights: self.options.traffic_lights,
        };
        f(&mut self.app, &mut cx);
    }

    fn sync_window_metrics(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.resize(size.width, size.height, scale_factor);
        }
        if let Some(window) = self.window.as_deref() {
            position_traffic_lights(window, self.options.traffic_lights);
        }
        self.flags.needs_redraw = true;
    }

    fn redraw(&mut self) {
        let (Some(window), Some(renderer)) = (self.window.as_deref(), self.renderer.as_mut())
        else {
            return;
        };
        self.flags.needs_redraw = false;
        let elapsed = self.launch_at.elapsed();
        let text_metrics = renderer.text_metrics();
        let mut cx = FrameContext {
            size: window.inner_size(),
            scale_factor: renderer.scale_factor(),
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
            if let Some(adapter) = self.accessibility_adapter.as_mut() {
                adapter.update_if_active(|| update);
            }
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
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }
        if let Some(adapter) = self.accessibility_adapter.as_mut() {
            adapter.process_event(window, &event);
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                let scale_factor = window.scale_factor();
                self.sync_window_metrics(size, scale_factor);
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = window.inner_size();
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
            && let Some(window) = self.window.as_ref()
        {
            window.request_redraw();
        }
    }
}

#[cfg(target_os = "macos")]
fn custom_chrome(attrs: WindowAttributes) -> WindowAttributes {
    use winit::platform::macos::WindowAttributesExtMacOS;
    attrs
        .with_titlebar_transparent(true)
        .with_fullsize_content_view(true)
        .with_title_hidden(true)
        .with_movable_by_window_background(false)
}

#[cfg(not(target_os = "macos"))]
fn custom_chrome(attrs: WindowAttributes) -> WindowAttributes {
    attrs.with_decorations(false)
}

#[cfg(target_os = "macos")]
fn position_traffic_lights(window: &Window, lights: Option<TrafficLights>) {
    if let Some(lights) = lights {
        crate::macos_window::position_traffic_lights(window, lights.left_margin, lights.center_y);
    }
}

#[cfg(not(target_os = "macos"))]
fn position_traffic_lights(_window: &Window, _lights: Option<TrafficLights>) {}

/// accesskit requires a tree before the app has produced one.
fn empty_tree_update() -> TreeUpdate {
    let root = NodeId(0);
    TreeUpdate {
        nodes: vec![(root, Node::new(Role::Window))],
        tree: Some(Tree::new(root)),
        tree_id: TreeId::ROOT,
        focus: root,
    }
}

struct AccessibilityActivation {
    latest_tree: Arc<Mutex<TreeUpdate>>,
}

impl ActivationHandler for AccessibilityActivation {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.latest_tree.lock().ok().map(|tree| tree.clone())
    }
}

/// accesskit calls this off the main thread; queue the request and wake the
/// loop so the app handles it with an `EventContext`.
struct AccessibilityActions {
    sender: Sender<ActionRequest>,
    waker: Waker,
}

impl ActionHandler for AccessibilityActions {
    fn do_action(&mut self, request: ActionRequest) {
        if self.sender.send(request).is_ok() {
            self.waker.wake();
        }
    }
}

struct AccessibilityDeactivation;

impl DeactivationHandler for AccessibilityDeactivation {
    fn deactivate_accessibility(&mut self) {}
}
