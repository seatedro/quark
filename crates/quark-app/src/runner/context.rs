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
    pub(super) needs_redraw: bool,
    pub(super) next_frame_at: Option<Instant>,
    pub(super) exit_requested: bool,
}

impl Flags {
    fn request_frame_at(&mut self, at: Instant) {
        self.next_frame_at = Some(self.next_frame_at.map_or(at, |next| next.min(at)));
    }
}

pub struct FrameContext<'a> {
    pub(super) size: PhysicalSize<u32>,
    pub(super) scale_factor: f64,
    pub(super) text_metrics: TextMetrics,
    pub(super) text: &'a mut AppText,
    pub(super) elapsed: Duration,
    pub(super) flags: &'a mut Flags,
    pub(super) waker: &'a Waker,
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
        self.text.system.font_system_mut()
    }

    /// The text system and layout cache this frame's scene must be shaped
    /// with.
    pub fn text(&mut self) -> &mut AppText {
        self.text
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
    pub(super) window: &'a Window,
    pub(super) text: &'a mut AppText,
    pub(super) flags: &'a mut Flags,
    pub(super) clipboard: &'a mut Option<arboard::Clipboard>,
    pub(super) input: &'a InputNormalizer,
    pub(super) waker: &'a Waker,
    pub(super) traffic_lights: Option<TrafficLights>,
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
        self.text.system.font_system_mut()
    }

    pub fn text(&mut self) -> &mut AppText {
        self.text
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
