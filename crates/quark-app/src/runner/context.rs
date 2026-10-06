use super::*;

/// Wakes the event loop from any thread and calls [`App::wake`].
#[derive(Debug, Clone)]
pub struct Waker(pub(super) EventLoopProxy<()>);

impl Waker {
    pub fn wake(&self) {
        let _ = self.0.send_event(());
    }

    /// The raw proxy, for code that speaks winit directly.
    pub fn proxy(&self) -> &EventLoopProxy<()> {
        &self.0
    }
}

/// Runner state that contexts mutate on the app's behalf.
#[derive(Debug, Default)]
pub(super) struct Flags {
    /// Windows to redraw on the next pass. Duplicates are harmless.
    pub(super) redraw: Vec<WindowHandle>,
    pub(super) redraw_all: bool,
    /// When reached, every window redraws.
    pub(super) next_frame_at: Option<Instant>,
    pub(super) exit_requested: bool,
    pub(super) keep_running_without_windows: bool,
    pub(super) close: Vec<WindowHandle>,
}

impl Flags {
    fn request_frame_at(&mut self, at: Instant) {
        self.next_frame_at = Some(self.next_frame_at.map_or(at, |next| next.min(at)));
    }

    fn request_redraw(&mut self, window: Option<WindowHandle>) {
        match window {
            Some(window) => self.redraw.push(window),
            None => self.redraw_all = true,
        }
    }
}

pub struct FrameContext<'a> {
    pub(super) window: WindowHandle,
    pub(super) size: PhysicalSize<u32>,
    pub(super) scale_factor: f64,
    pub(super) text_metrics: TextMetrics,
    pub(super) font_system: &'a mut FontSystem,
    pub(super) elapsed: Duration,
    pub(super) flags: &'a mut Flags,
    pub(super) waker: &'a Waker,
}

impl FrameContext<'_> {
    /// The window this frame is for.
    pub fn window_handle(&self) -> WindowHandle {
        self.window
    }

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

    /// Draw another frame of this window right after this one, for animation.
    pub fn request_frame(&mut self) {
        self.flags.redraw.push(self.window);
    }

    pub fn request_frame_at(&mut self, at: Instant) {
        self.flags.request_frame_at(at);
    }

    pub fn waker(&self) -> &Waker {
        self.waker
    }
}

pub struct EventContext<'a> {
    pub(super) windows: &'a mut WindowTable<WindowEntry>,
    pub(super) window: Option<WindowHandle>,
    pub(super) flags: &'a mut Flags,
    pub(super) clipboard: &'a mut Option<arboard::Clipboard>,
    /// Stands in for a renderer's `FontSystem` while no window is open.
    pub(super) fallback_fonts: &'a mut Option<FontSystem>,
    pub(super) fonts: &'a FontSettings,
    pub(super) waker: &'a Waker,
    pub(super) events: &'a EventSink,
    pub(super) theme: Option<Theme>,
    #[cfg(feature = "tray")]
    pub(super) tray: &'a mut Option<tray_icon::TrayIcon>,
}

impl EventContext<'_> {
    /// Redraw the context's window, or every window when the context has none.
    pub fn request_redraw(&mut self) {
        self.flags.request_redraw(self.window);
    }

    pub fn request_redraw_all(&mut self) {
        self.flags.redraw_all = true;
    }

    pub fn request_frame_at(&mut self, at: Instant) {
        self.flags.request_frame_at(at);
    }

    pub fn exit(&mut self) {
        self.flags.exit_requested = true;
    }

    /// The window this event is for. `None` only for app events that arrive
    /// while no window is open.
    pub fn window_handle(&self) -> Option<WindowHandle> {
        self.window
    }

    /// Open another window. It is created when the current callback returns;
    /// until then the handle is valid but the window has no native surface.
    /// Failure arrives as [`AppEvent::WindowOpenFailed`].
    pub fn open_window(&mut self, options: WindowOptions) -> WindowHandle {
        self.windows.insert(WindowEntry::Pending(Box::new(options)))
    }

    /// Close a window when the current callback returns, without asking
    /// [`App::close_requested`]. Stale handles are ignored.
    pub fn close_window(&mut self, window: WindowHandle) {
        self.flags.close.push(window);
    }

    /// Every open or opening window.
    pub fn windows(&self) -> Vec<WindowHandle> {
        self.windows.handles()
    }

    /// By default the app exits when its last window closes. Tray and
    /// background apps turn that off and call [`EventContext::exit`] instead.
    pub fn set_exit_when_last_window_closes(&mut self, exit: bool) {
        self.flags.keep_running_without_windows = !exit;
    }

    /// Last pointer position in physical pixels, if the pointer is inside the
    /// context's window.
    pub fn pointer_position(&self) -> Option<(f32, f32)> {
        self.state()?.input.pointer_position()
    }

    pub fn modifiers(&self) -> ModifiersState {
        self.state()
            .map(|state| state.input.modifiers())
            .unwrap_or_default()
    }

    pub fn scale_factor(&self) -> f32 {
        self.state().map_or(1.0, |state| state.scale_factor as f32)
    }

    pub fn font_system(&mut self) -> &mut FontSystem {
        let fonts = self.fonts;
        let window = self.window;
        if let Some(state) = window
            .and_then(|window| self.windows.get_mut(window))
            .and_then(WindowEntry::open_mut)
        {
            return state.renderer.font_system_mut();
        }
        self.fallback_fonts
            .get_or_insert_with(|| quark_render::fonts::new_font_system_with_settings(fonts))
    }

    pub fn set_cursor(&mut self, cursor: CursorIcon) {
        if let Some(window) = self.native() {
            window.set_cursor(cursor);
        }
    }

    /// winit leaves IME off by default; enable it while a text field has focus.
    pub fn set_ime_allowed(&mut self, allowed: bool) {
        if let Some(window) = self.native() {
            window.set_ime_allowed(allowed);
        }
    }

    /// Where the IME candidate window should appear, in physical pixels.
    pub fn set_ime_cursor_area(&mut self, x: f32, y: f32, width: f32, height: f32) {
        if let Some(window) = self.native() {
            window.set_ime_cursor_area(
                PhysicalPosition::new(x as f64, y as f64),
                PhysicalSize::new(width as f64, height as f64),
            );
        }
    }

    pub fn set_title(&mut self, title: &str) {
        if let Some(state) = self.state() {
            state.window.set_title(title);
            position_traffic_lights(&state.window, state.traffic_lights);
        }
    }

    pub fn clipboard_text(&mut self) -> Option<String> {
        self.clipboard()?.get_text().ok()
    }

    pub fn set_clipboard_text(&mut self, text: &str) {
        if let Some(clipboard) = self.clipboard() {
            let _ = clipboard.set_text(text);
        }
    }

    /// The context's native window, for operations the context doesn't wrap
    /// (drag, resize, minimize, maximize).
    pub fn window(&self) -> Option<&Window> {
        self.native()
    }

    /// Another open window, by handle.
    pub fn window_by_handle(&self, window: WindowHandle) -> Option<&Window> {
        let state = self.windows.get(window)?.open()?;
        Some(&state.window)
    }

    pub fn waker(&self) -> &Waker {
        self.waker
    }

    /// The desktop's light or dark preference, if the platform reports one.
    /// Changes arrive as [`AppEvent::ThemeChanged`].
    pub fn theme(&self) -> Option<Theme> {
        self.theme
    }

    /// Show a desktop notification without blocking. Clicks come back as
    /// [`AppEvent::NotificationAction`] where the platform reports them.
    #[cfg(feature = "notifications")]
    pub fn notify(&mut self, notification: crate::platform::notification::Notification) {
        crate::platform::notification::show(notification, self.events.clone());
    }

    /// Deliver arguments from later launches of the app as
    /// [`AppEvent::OpenUrls`]. See [`crate::platform::single_instance`].
    pub fn listen_for_instances(
        &mut self,
        primary: crate::platform::single_instance::PrimaryInstance,
    ) {
        primary.spawn(self.events.clone());
    }

    /// Show a tray icon, replacing any previous one.
    #[cfg(feature = "tray")]
    pub fn set_tray(
        &mut self,
        options: crate::platform::tray::TrayOptions,
    ) -> Result<(), crate::platform::tray::TrayError> {
        *self.tray = None;
        *self.tray = Some(crate::platform::tray::create(options, self.events)?);
        Ok(())
    }

    #[cfg(feature = "tray")]
    pub fn remove_tray(&mut self) {
        *self.tray = None;
    }

    fn state(&self) -> Option<&WindowState> {
        self.windows.get(self.window?)?.open()
    }

    fn native(&self) -> Option<&Window> {
        self.state().map(|state| &*state.window)
    }

    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        if self.clipboard.is_none() {
            *self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()
    }
}
