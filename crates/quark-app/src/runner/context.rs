use super::*;

/// Wakes the event loop from any thread and calls [`App::wake`].
#[derive(Debug, Clone)]
pub struct Waker(WakeTarget);

#[derive(Debug, Clone)]
enum WakeTarget {
    EventLoop(EventLoopProxy<()>),
    /// No event loop: the headless test runner polls the flag instead.
    #[cfg(feature = "test-support")]
    Detached(Arc<std::sync::atomic::AtomicBool>),
}

impl Waker {
    pub(super) fn new(proxy: EventLoopProxy<()>) -> Self {
        Self(WakeTarget::EventLoop(proxy))
    }

    #[cfg(feature = "test-support")]
    pub(crate) fn detached() -> Self {
        Self(WakeTarget::Detached(Arc::default()))
    }

    /// Whether a detached waker was woken since the last call.
    #[cfg(feature = "test-support")]
    pub(crate) fn take_detached_wake(&self) -> bool {
        match &self.0 {
            WakeTarget::Detached(woken) => woken.swap(false, std::sync::atomic::Ordering::AcqRel),
            WakeTarget::EventLoop(_) => false,
        }
    }

    pub fn wake(&self) {
        match &self.0 {
            WakeTarget::EventLoop(proxy) => {
                let _ = proxy.send_event(());
            }
            #[cfg(feature = "test-support")]
            WakeTarget::Detached(woken) => woken.store(true, std::sync::atomic::Ordering::Release),
        }
    }

    /// The raw proxy, for code that speaks winit directly.
    ///
    /// # Panics
    ///
    /// Under the headless test harness, which has no event loop.
    pub fn proxy(&self) -> &EventLoopProxy<()> {
        match &self.0 {
            WakeTarget::EventLoop(proxy) => proxy,
            #[cfg(feature = "test-support")]
            WakeTarget::Detached(_) => panic!("a detached test waker has no event loop"),
        }
    }
}

/// Where [`EventContext`]'s clipboard calls go.
pub(super) enum Clipboard {
    /// The system clipboard, opened on first use.
    System(Option<arboard::Clipboard>),
    /// In memory, so headless tests neither read nor clobber the desktop's.
    #[cfg(feature = "test-support")]
    Memory(MemoryClipboard),
}

#[cfg(feature = "test-support")]
#[derive(Debug, Default)]
pub(crate) struct MemoryClipboard {
    pub(crate) text: Option<String>,
    #[cfg(feature = "clipboard-image")]
    pub(crate) image: Option<ClipboardImage>,
}

/// What an [`EventContext`] reports about its window when no native window
/// backs it: the headless test runner's pointer, modifiers, and scale.
#[cfg(feature = "test-support")]
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeadlessWindow {
    pub(crate) pointer: Option<(f32, f32)>,
    pub(crate) modifiers: ModifiersState,
    pub(crate) scale_factor: f64,
}

/// Runner state that contexts mutate on the app's behalf.
#[derive(Debug, Default)]
pub(super) struct Flags {
    /// Windows to redraw on the next pass. Duplicates are harmless.
    pub(super) redraw: Vec<WindowHandle>,
    pub(super) redraw_all: bool,
    /// Frames requested for a time: for one window, or for every window when
    /// the window is `None`. The runner moves them onto each window's frame
    /// clock before it next waits.
    pub(super) frame_at: Vec<(Option<WindowHandle>, Instant)>,
    pub(super) exit_requested: bool,
    pub(super) keep_running_without_windows: bool,
    pub(super) close: Vec<WindowHandle>,
    #[cfg(feature = "dialogs")]
    pub(super) next_dialog: u64,
}

impl Flags {
    fn request_frame_at(&mut self, window: Option<WindowHandle>, at: Instant) {
        self.frame_at.push((window, at));
    }

    fn request_redraw(&mut self, window: Option<WindowHandle>) {
        match window {
            Some(window) => self.redraw.push(window),
            None => self.redraw_all = true,
        }
    }
}

/// Straight RGBA8 pixels, `width * height * 4` bytes.
#[cfg(feature = "clipboard-image")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// What [`App::frame`] sees. Sizes and scene coordinates are logical points;
/// the runner multiplies the returned scene by [`Self::scale_factor`] once,
/// snapping quads to physical pixels, before the renderer draws it.
pub struct FrameContext<'a> {
    pub(super) window: WindowHandle,
    pub(super) size: PhysicalSize<u32>,
    pub(super) scale_factor: f64,
    pub(super) text_metrics: TextMetrics,
    pub(super) text: &'a mut AppText,
    pub(super) timing: FrameTiming,
    pub(super) flags: &'a mut Flags,
    pub(super) waker: &'a Waker,
    pub(super) ime: FrameIme,
    /// Assistive tech listens to this window.
    pub(super) accessibility_active: bool,
    #[cfg(feature = "devtools")]
    pub(super) last_render: quark_render::FrameStats,
}

/// IME requests made while building a frame. The runner applies them to
/// the frame's window once [`App::frame`] returns, so they use the caret
/// that frame painted.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct FrameIme {
    pub(super) allowed: Option<bool>,
    pub(super) cursor_area: Option<(f32, f32, f32, f32)>,
}

impl FrameContext<'_> {
    /// Whether assistive tech listens to this window. While it does not,
    /// the window's accessibility tree is not published, so a frame need
    /// not build one.
    pub fn accessibility_active(&self) -> bool {
        self.accessibility_active
    }

    /// Renderer stats of this window's previous frame.
    #[cfg(feature = "devtools")]
    pub fn last_render_stats(&self) -> quark_render::FrameStats {
        self.last_render
    }

    /// Turn IME on or off for this window once the frame is built; see
    /// [`EventContext::set_ime_allowed`].
    pub fn set_ime_allowed(&mut self, allowed: bool) {
        self.ime.allowed = Some(allowed);
    }

    /// Place this window's IME candidate window, in logical points, once
    /// the frame is built; see [`EventContext::set_ime_cursor_area`].
    pub fn set_ime_cursor_area(&mut self, x: f32, y: f32, width: f32, height: f32) {
        self.ime.cursor_area = Some((x, y, width, height));
    }

    /// The window this frame is for.
    pub fn window_handle(&self) -> WindowHandle {
        self.window
    }

    /// Drawable size in logical points, the space the scene is built in.
    pub fn size(&self) -> (f32, f32) {
        let (width, height) = self.physical_size();
        let scale = self.scale_factor as f32;
        (width / scale, height / scale)
    }

    /// Same as [`Self::size`].
    pub fn logical_size(&self) -> (f32, f32) {
        self.size()
    }

    /// Drawable size in physical pixels.
    pub fn physical_size(&self) -> (f32, f32) {
        (
            self.size.width.max(1) as f32,
            self.size.height.max(1) as f32,
        )
    }

    /// Physical pixels per logical point for this window.
    pub fn scale_factor(&self) -> f32 {
        self.scale_factor as f32
    }

    /// Default UI and monospace metrics in logical points.
    pub fn text_metrics(&self) -> TextMetrics {
        self.text_metrics
    }

    /// Shape `params` for this window: sizes stay logical, glyphs are shaped
    /// at this window's scale so they rasterize crisply. Layouts drawn in a
    /// scene must be shaped this way (or with the same scale factor).
    pub fn layout_text(&mut self, params: &TextParams) -> Result<Arc<TextLayout>, TextError> {
        let params = params.clone().scale_factor(self.scale_factor as f32);
        self.text.layouts.layout(&mut self.text.system, &params)
    }

    pub fn font_system(&mut self) -> &mut FontSystem {
        self.text.system.font_system_mut()
    }

    /// The text system and layout cache this frame's scene must be shaped
    /// with.
    pub fn text(&mut self) -> &mut AppText {
        self.text
    }

    /// This frame's time on the window's frame clock, measured from when the
    /// runner started. Constant for the whole frame; drive animations off it.
    pub fn elapsed(&self) -> Duration {
        self.timing.elapsed
    }

    /// Time since this window's previous frame, zero for its first frame.
    pub fn frame_delta(&self) -> Duration {
        self.timing.delta
    }

    /// Draw another frame of this window right after this one, for animation.
    pub fn request_frame(&mut self) {
        self.flags.redraw.push(self.window);
    }

    /// Draw this window again at `at`. Other windows are not redrawn.
    pub fn request_frame_at(&mut self, at: Instant) {
        self.flags.request_frame_at(Some(self.window), at);
    }

    /// Draw this window again `after` this frame's time.
    pub fn request_frame_in(&mut self, after: Duration) {
        let at = self.timing.now + after;
        self.flags.request_frame_at(Some(self.window), at);
    }

    pub fn waker(&self) -> &Waker {
        self.waker
    }
}

pub struct EventContext<'a> {
    pub(super) windows: &'a mut WindowTable<WindowEntry>,
    pub(super) window: Option<WindowHandle>,
    pub(super) text: &'a mut AppText,
    pub(super) flags: &'a mut Flags,
    pub(super) clipboard: &'a mut Clipboard,
    pub(super) waker: &'a Waker,
    pub(super) events: &'a EventSink,
    pub(super) theme: Option<Theme>,
    #[cfg(feature = "test-support")]
    pub(super) headless: Option<HeadlessWindow>,
    /// When the callback started, measured from the runner's start.
    pub(super) elapsed: Duration,
    #[cfg(feature = "tray")]
    pub(super) tray: &'a mut Option<tray_icon::TrayIcon>,
}

impl EventContext<'_> {
    /// When this callback started, measured from when the runner started:
    /// the same clock as [`FrameContext::elapsed`], so event and frame
    /// times compare directly.
    pub fn elapsed(&self) -> Duration {
        self.elapsed
    }

    /// Redraw the context's window, or every window when the context has none.
    pub fn request_redraw(&mut self) {
        self.flags.request_redraw(self.window);
    }

    pub fn request_redraw_all(&mut self) {
        self.flags.redraw_all = true;
    }

    /// Redraw the context's window (or every window when the context has
    /// none) at `at`.
    pub fn request_frame_at(&mut self, at: Instant) {
        self.flags.request_frame_at(self.window, at);
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

    /// Last pointer position in logical points, if the pointer is inside the
    /// context's window.
    pub fn pointer_position(&self) -> Option<(f32, f32)> {
        #[cfg(feature = "test-support")]
        if let Some(headless) = self.headless {
            return headless.pointer;
        }
        self.state()?.input.pointer_position()
    }

    pub fn modifiers(&self) -> ModifiersState {
        #[cfg(feature = "test-support")]
        if let Some(headless) = self.headless {
            return headless.modifiers;
        }
        self.state()
            .map(|state| state.input.modifiers())
            .unwrap_or_default()
    }

    /// Physical pixels per logical point for the context's window.
    pub fn scale_factor(&self) -> f32 {
        #[cfg(feature = "test-support")]
        if let Some(headless) = self.headless {
            return headless.scale_factor as f32;
        }
        self.state().map_or(1.0, |state| state.scale_factor as f32)
    }

    pub fn font_system(&mut self) -> &mut FontSystem {
        self.text.system.font_system_mut()
    }

    pub fn text(&mut self) -> &mut AppText {
        self.text
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

    /// Where the IME candidate window should appear, in logical points.
    pub fn set_ime_cursor_area(&mut self, x: f32, y: f32, width: f32, height: f32) {
        if let Some(window) = self.native() {
            window.set_ime_cursor_area(
                LogicalPosition::new(x as f64, y as f64),
                LogicalSize::new(width as f64, height as f64),
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
        #[cfg(feature = "test-support")]
        if let Clipboard::Memory(memory) = self.clipboard {
            return memory.text.clone();
        }
        self.clipboard()?.get_text().ok()
    }

    pub fn set_clipboard_text(&mut self, text: &str) {
        #[cfg(feature = "test-support")]
        if let Clipboard::Memory(memory) = self.clipboard {
            memory.text = Some(text.to_owned());
            return;
        }
        if let Some(clipboard) = self.clipboard() {
            let _ = clipboard.set_text(text);
        }
    }

    /// The clipboard's image, if it holds one.
    #[cfg(feature = "clipboard-image")]
    pub fn clipboard_image(&mut self) -> Option<ClipboardImage> {
        #[cfg(feature = "test-support")]
        if let Clipboard::Memory(memory) = self.clipboard {
            return memory.image.clone();
        }
        let image = self.clipboard()?.get_image().ok()?;
        Some(ClipboardImage {
            width: image.width,
            height: image.height,
            rgba: image.bytes.into_owned(),
        })
    }

    /// Put an image on the clipboard. Returns false if the clipboard is
    /// unavailable or `rgba` does not match the size.
    #[cfg(feature = "clipboard-image")]
    pub fn set_clipboard_image(&mut self, image: &ClipboardImage) -> bool {
        if image.rgba.len() != image.width * image.height * 4 {
            return false;
        }
        #[cfg(feature = "test-support")]
        if let Clipboard::Memory(memory) = self.clipboard {
            memory.image = Some(image.clone());
            return true;
        }
        let Some(clipboard) = self.clipboard() else {
            return false;
        };
        clipboard
            .set_image(arboard::ImageData {
                width: image.width,
                height: image.height,
                bytes: std::borrow::Cow::Borrowed(&image.rgba),
            })
            .is_ok()
    }

    /// Show a native open or save dialog parented to the context's window.
    /// The result arrives as [`AppEvent::FileDialogClosed`] with the returned
    /// id.
    #[cfg(feature = "dialogs")]
    pub fn file_dialog(
        &mut self,
        dialog: crate::platform::dialog::FileDialog,
    ) -> crate::platform::dialog::DialogId {
        self.flags.next_dialog += 1;
        let id = crate::platform::dialog::DialogId(self.flags.next_dialog);
        let parent = self.native();
        crate::platform::dialog::open(dialog, id, parent, self.events.clone());
        id
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

    /// The system clipboard; `None` when it cannot be opened or the
    /// context uses an in-memory one.
    fn clipboard(&mut self) -> Option<&mut arboard::Clipboard> {
        match self.clipboard {
            Clipboard::System(clipboard) => {
                if clipboard.is_none() {
                    *clipboard = arboard::Clipboard::new().ok();
                }
                clipboard.as_mut()
            }
            #[cfg(feature = "test-support")]
            Clipboard::Memory(_) => None,
        }
    }
}
