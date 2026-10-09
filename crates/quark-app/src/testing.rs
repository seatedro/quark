//! Drive a [`UiApp`] in tests with no window, event loop, or GPU.
//!
//! [`UiTestHarness`] runs the app through the same [`UiAdapter`] as
//! [`crate::run_ui`], so layout, hit testing, focus, key bindings, text
//! editing, and accessibility behave as they do on screen. Everything is
//! deterministic:
//!
//! - Time is a fake clock that only [`UiTestHarness::advance`] moves.
//!   Frames an app or animation asks for are drawn at the clock time they
//!   asked for; a frame asking to redraw right away gets the next 16 ms
//!   vsync.
//! - The clipboard is in memory.
//! - [`UiSender`] messages and wakes are delivered on the test's thread by
//!   [`UiTestHarness::run_until_idle`], which every input method calls, so
//!   after any call the app has handled the input and painted the result.
//!
//! Find nodes the way assistive tech or a user would, by accessible name,
//! role, accessibility id, or test id ([`By`]), then click them, read their
//! bounds, or check focus. Geometry is in logical points, the space the app
//! lays out in.
//!
//! ```ignore
//! let mut ui = UiTestHarness::new(app, (640.0, 400.0), 2.0);
//! ui.click_node(By::role_name(Role::TextInput, "Name"));
//! ui.type_text("Ada");
//! ui.key("mod+s");
//! assert!(ui.painted_text().contains("Saved Ada"));
//! ```

use std::fmt;
use std::time::Duration;

use accesskit::{ActionRequest, NodeId, Role, TreeUpdate};
use quark::Rect;
use quark::scene::{Primitive, Scene};
use quark_ui::FocusId;
use quark_ui::element::Binding;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, TouchPhase};
use winit::keyboard::{ModifiersState, NamedKey};

use crate::runner::{HeadlessRunner, VirtualWindow};
use crate::{
    App, AppEvent, CloseReason, DesktopPoint, InputEvent, KeyChord, KeyKind, MonitorInfo,
    PlatformCapabilities, UiAdapter, UiApp, UiSender, WindowHandle, WindowPlacement,
};

/// What the app asked of the window's IME, as the frames so far left it
/// ([`UiTestHarness::ime`]).
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct ImeState {
    pub allowed: bool,
    /// Where the candidate window was last placed, in points.
    pub cursor_area: Option<Rect>,
    /// Times a frame made the IME drop its composition.
    pub resets: u32,
}

impl ImeState {
    pub(crate) fn apply(&mut self, frame: crate::runner::FrameIme) {
        if frame.reset {
            self.resets += 1;
        }
        if let Some(allowed) = frame.allowed {
            self.allowed = allowed;
        }
        if let Some((x, y, width, height)) = frame.cursor_area {
            self.cursor_area = Some(Rect {
                x,
                y,
                width,
                height,
            });
        }
    }
}

/// Pointer moves [`UiTestHarness::drag`] makes between its press and
/// release, so drag handlers see motion rather than a jump.
const DRAG_STEPS: u32 = 4;

/// A [`UiApp`] in headless windows. See the [module docs](self).
pub struct UiTestHarness<U: UiApp> {
    adapter: UiAdapter<U>,
    runner: HeadlessRunner,
    sender: UiSender<U::Message>,
    #[cfg(feature = "headless-render")]
    renderer: Option<quark_render::Renderer>,
}

impl<U: UiApp> UiTestHarness<U> {
    /// Open `app` in a `size` point window at `scale_factor`, run
    /// [`UiApp::init`], and paint the first frame. The accessibility tree's
    /// window is named "Test"; the theme is the adapter's default dark one.
    pub fn new(app: U, size: (f32, f32), scale_factor: f32) -> Self {
        Self::with_adapter(UiAdapter::new(app, "Test"), size, scale_factor)
    }

    /// [`Self::new`] for an adapter already configured, such as with
    /// [`UiAdapter::with_theme`].
    pub fn with_adapter(adapter: UiAdapter<U>, size: (f32, f32), scale_factor: f32) -> Self {
        Self::start(adapter, size, scale_factor, None)
    }

    /// [`Self::new`] with the main window on `monitor` from the start, so
    /// [`UiApp::init`] sees it ([`crate::EventContext::monitors`]). Windows
    /// the app opens go on the main window's monitor.
    pub fn with_monitor(app: U, size: (f32, f32), scale_factor: f32, monitor: MonitorInfo) -> Self {
        Self::start(
            UiAdapter::new(app, "Test"),
            size,
            scale_factor,
            Some(monitor),
        )
    }

    fn start(
        adapter: UiAdapter<U>,
        size: (f32, f32),
        scale_factor: f32,
        monitor: Option<MonitorInfo>,
    ) -> Self {
        let mut harness = Self {
            sender: adapter.sender(),
            adapter,
            runner: HeadlessRunner::new(size, f64::from(scale_factor)),
            #[cfg(feature = "headless-render")]
            renderer: None,
        };
        let main = harness.runner.main_window();
        harness.runner.set_monitor(main, monitor);
        harness
            .runner
            .callback(&mut harness.adapter, |adapter, cx| {
                App::init(adapter, cx);
                App::app_event(adapter, AppEvent::WindowOpened(main), cx);
            });
        harness.runner.request_frame(main);
        harness.run_until_idle();
        harness
    }

    pub fn app(&self) -> &U {
        self.adapter.app()
    }

    /// The theme the app paints with.
    pub fn theme(&self) -> &quark_ui::theme::Theme {
        self.adapter.theme()
    }

    /// The app, to set up state directly. Changes show after the next frame:
    /// call [`Self::frame`] or ask for one from the app.
    pub fn app_mut(&mut self) -> &mut U {
        self.adapter.app_mut()
    }

    // ---- Windows ---------------------------------------------------------

    /// The window the harness opened with. Methods on the harness itself
    /// that name no window act on it.
    pub fn main_window(&self) -> WindowHandle {
        self.runner.main_window()
    }

    /// Every open window, the main one first unless it closed.
    pub fn windows(&self) -> Vec<WindowHandle> {
        self.runner.windows()
    }

    /// Input, queries, and window controls for one window, such as one the
    /// app opened with [`crate::EventContext::open_window`]. The app learns
    /// of new windows from [`AppEvent::WindowOpened`] as on the desktop;
    /// they open unfocused, at the main window's starting scale, at the
    /// options' position or (0, 0).
    ///
    /// # Panics
    ///
    /// When `window` is not open.
    #[track_caller]
    pub fn window(&mut self, window: WindowHandle) -> UiWindow<'_, U> {
        assert!(
            self.runner.window(window).is_some(),
            "UiTestHarness::window: {window:?} is not open"
        );
        UiWindow { ui: self, window }
    }

    fn main(&mut self) -> UiWindow<'_, U> {
        let window = self.main_window();
        UiWindow { ui: self, window }
    }

    /// Make the next window the app opens fail to open, as when the
    /// platform refuses one: the app gets [`AppEvent::WindowClosed`] with
    /// [`CloseReason::OpenFailed`]. Calls add up.
    pub fn fail_next_window_open(&mut self) {
        self.runner.fail_next_open();
    }

    /// Act as a desktop with `capabilities`, such as
    /// [`PlatformCapabilities::WAYLAND`]: without window positions,
    /// placements carry none and window moves go unreported. Starts as
    /// [`PlatformCapabilities::DESKTOP`].
    pub fn set_capabilities(&mut self, capabilities: PlatformCapabilities) {
        self.runner.set_capabilities(capabilities);
    }

    pub fn capabilities(&self) -> PlatformCapabilities {
        self.runner.capabilities()
    }

    /// Quit as the Quit menu item does: ask every window
    /// ([`UiApp::close_requested`]), close those that agree, and exit if
    /// all did. Returns whether all did.
    pub fn request_quit(&mut self) -> bool {
        let quit = self.runner.request_quit(&mut self.adapter);
        self.run_until_idle();
        quit
    }

    // ---- Desktop pointer -------------------------------------------------

    /// Move the pointer to `at` on the virtual desktop, across windows: the
    /// topmost window under it gets the motion in its own coordinates, with
    /// [`InputEvent::PointerLeft`] and [`InputEvent::PointerEntered`] as it
    /// crosses between windows. While the primary button pressed with
    /// [`Self::desktop_press`] is held, the window it was pressed in gets
    /// all motion, even outside it, as Windows, macOS, and X11 deliver a
    /// drag. Windows stack in the order they opened or took focus.
    ///
    /// The window-scoped pointer methods skip this simulation: they send
    /// one window input directly.
    pub fn desktop_move(&mut self, at: DesktopPoint) {
        self.runner.desktop_pointer_move(&mut self.adapter, at);
        self.run_until_idle();
    }

    /// Press the primary button at the desktop pointer.
    pub fn desktop_press(&mut self) {
        let state = ElementState::Pressed;
        self.runner
            .desktop_button(&mut self.adapter, MouseButton::Left, state);
        self.run_until_idle();
    }

    /// Release the primary button at the desktop pointer.
    pub fn desktop_release(&mut self) {
        let state = ElementState::Released;
        self.runner
            .desktop_button(&mut self.adapter, MouseButton::Left, state);
        self.run_until_idle();
    }

    // ---- Time and frames -------------------------------------------------

    /// The fake clock: time since the harness started.
    pub fn now(&self) -> Duration {
        Duration::from_millis(self.runner.now_ms())
    }

    /// Deliver pending [`UiSender`] messages and wakes, then draw the frames
    /// due now in any window, until neither is left. The clock does not
    /// move. Every input method ends with this.
    ///
    /// # Panics
    ///
    /// When the app never settles, such as by waking itself from every
    /// [`UiApp::wake`].
    pub fn run_until_idle(&mut self) {
        self.runner.run_until_idle(&mut self.adapter);
    }

    /// Move the clock forward `ms`, drawing every frame requested on the way
    /// at its own time: animations step, timers fire, and selection
    /// autoscroll runs as they would in real time.
    pub fn advance(&mut self, ms: u64) {
        self.runner.advance(&mut self.adapter, ms);
    }

    /// Draw a frame of the main window now whether or not one was asked
    /// for, and return it.
    ///
    /// It then runs [`Self::run_until_idle`], which can draw more frames
    /// before returning: those that delivered messages and wakes make due,
    /// and any requested for a clock time at or before now. A frame's own
    /// requests to draw again right away go to the next 16 ms vsync, which
    /// only [`Self::advance`] reaches. An allocation count around this call
    /// therefore covers the drawn frame plus that follow-up work, and the
    /// returned scene is the last frame drawn.
    pub fn frame(&mut self) -> &Scene {
        let main = self.main_window();
        self.window(main).frame();
        self.scene()
    }

    /// The main window's last painted frame, in points.
    pub fn scene(&self) -> &Scene {
        self.scene_in(self.main_window())
    }

    /// Whether frames see assistive tech listening (on by default). While
    /// off, frames skip building the accessibility tree, as they do when no
    /// screen reader is connected.
    pub fn set_accessibility_active(&mut self, active: bool) {
        self.runner.accessibility_active = active;
    }

    /// Frames the main window drew since the harness started, counting the
    /// first. Compare before and after an input to see whether it
    /// repainted.
    pub fn frame_count(&self) -> u64 {
        self.open(self.main_window()).frames_drawn
    }

    /// Whether a frame of any window is scheduled, for a running animation
    /// or timer. After [`Self::run_until_idle`] none is due now, so any is
    /// later.
    pub fn frame_requested(&self) -> bool {
        self.runner.next_frame_ms().is_some()
    }

    /// How long until the next frame any window scheduled.
    pub fn next_frame_in(&self) -> Option<Duration> {
        let now = self.runner.now_ms();
        self.runner
            .next_frame_ms()
            .map(|ms| Duration::from_millis(ms.saturating_sub(now)))
    }

    // ---- Window ----------------------------------------------------------

    /// Resize the main window, in points, and repaint.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.main().resize(width, height);
    }

    /// Move the main window to a display with another scale factor and
    /// repaint.
    pub fn set_scale(&mut self, scale_factor: f32) {
        self.main().set_scale(scale_factor);
    }

    /// The main window's size in points.
    pub fn size(&self) -> (f32, f32) {
        self.open(self.main_window()).size
    }

    /// The main window lost keyboard focus, as when the user switches apps.
    pub fn focus_loss(&mut self) {
        self.main().focus_loss();
    }

    /// The main window gained keyboard focus; any other focused window
    /// loses it first.
    pub fn focus_gain(&mut self) {
        self.main().focus();
    }

    /// Ask the app to close the main window, as its close button does, and
    /// close it if the app agrees. Returns [`UiApp::close_requested`]'s
    /// answer. Once the last window closes, [`Self::exit_requested`] is
    /// true.
    pub fn request_close(&mut self) -> bool {
        self.main().request_close()
    }

    /// Whether the app called `exit` on a context, quit, or closed its last
    /// window: when a desktop app would stop.
    pub fn exit_requested(&self) -> bool {
        self.runner.exit_requested()
    }

    // ---- App input -------------------------------------------------------

    /// Send `message` as a worker thread would through [`UiSender`], then
    /// deliver it.
    pub fn send_message(&mut self, message: U::Message) {
        self.sender.send(message);
        self.run_until_idle();
    }

    /// A sender for the app's messages, for handing to code under test.
    /// Its messages arrive at the next [`Self::run_until_idle`].
    pub fn sender(&self) -> UiSender<U::Message> {
        self.sender.clone()
    }

    /// Deliver an [`AppEvent`], such as a theme change or URLs from another
    /// launch, against the focused window as the event loop does.
    pub fn app_event(&mut self, event: AppEvent) {
        self.runner.app_event(&mut self.adapter, event);
        self.run_until_idle();
    }

    /// The in-memory clipboard's text.
    pub fn clipboard_text(&mut self) -> Option<String> {
        self.runner.clipboard().text.clone()
    }

    pub fn set_clipboard_text(&mut self, text: impl Into<String>) {
        self.runner.clipboard().text = Some(text.into());
    }

    // ---- Keyboard --------------------------------------------------------

    /// Press and release a key binding such as `"mod+s"`, `"shift+tab"`, or
    /// `"enter"`, in the syntax key bindings use, in the main window. `mod`
    /// is Cmd on macOS and Ctrl elsewhere. A printable key without Cmd or
    /// Ctrl also types its character, as a real key press does.
    ///
    /// # Panics
    ///
    /// When `binding` does not parse or names a key the runner cannot
    /// report, such as F13.
    #[track_caller]
    pub fn key(&mut self, binding: &str) {
        self.main().key(binding);
    }

    /// Type `text` one character at a time, each as a key press that
    /// inserts it; `\n` presses Enter.
    pub fn type_text(&mut self, text: &str) {
        self.main().type_text(text);
    }

    /// Show an IME composition in the focused field: `text` with the caret
    /// or selection at the byte range `cursor`. Empty text cancels it.
    pub fn ime_preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        self.main().ime_preedit(text, cursor);
    }

    /// What frames asked of the main window's IME so far: whether it is on,
    /// where its candidate window was placed, and how often it was reset.
    pub fn ime(&self) -> ImeState {
        self.open(self.main_window()).ime
    }

    /// End the composition by committing `text`, as an IME does: the
    /// preedit clears, then the text is inserted.
    pub fn ime_commit(&mut self, text: &str) {
        self.main().ime_commit(text);
    }

    // ---- Pointer ---------------------------------------------------------

    pub fn pointer_move(&mut self, at: (f32, f32)) {
        self.main().pointer_move(at);
    }

    /// The pointer left the main window.
    pub fn pointer_leave(&mut self) {
        self.main().pointer_leave();
    }

    /// Move to `at` if the pointer is elsewhere, then press the primary
    /// button.
    pub fn pointer_down(&mut self, at: (f32, f32)) {
        self.main().pointer_down(at);
    }

    /// Move to `at` if the pointer is elsewhere, then release the primary
    /// button.
    pub fn pointer_up(&mut self, at: (f32, f32)) {
        self.main().pointer_up(at);
    }

    /// Press and release the primary button at `at`.
    pub fn click(&mut self, at: (f32, f32)) {
        self.main().click(at);
    }

    /// Click the center of the one node `by` finds.
    ///
    /// # Panics
    ///
    /// When no node or more than one matches.
    #[track_caller]
    pub fn click_node(&mut self, by: impl Into<By>) {
        self.main().click_node(by);
    }

    /// Press and release the middle button at `at`.
    pub fn middle_click(&mut self, at: (f32, f32)) {
        self.main().middle_click(at);
    }

    /// Middle click the center of the one node `by` finds.
    ///
    /// # Panics
    ///
    /// When no node or more than one matches.
    #[track_caller]
    pub fn middle_click_node(&mut self, by: impl Into<By>) {
        self.main().middle_click_node(by);
    }

    /// Press at `from`, move to `to` in a few steps, and release there.
    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.main().drag(from, to);
    }

    /// Scroll the wheel at the pointer by points; positive values scroll
    /// content down and right. Move the pointer over the target first.
    pub fn wheel(&mut self, dx: f32, dy: f32) {
        self.main().wheel(dx, dy);
    }

    /// Deliver any input event to the main window, then run until idle.
    pub fn send_event(&mut self, event: InputEvent) {
        self.main().send_event(event);
    }

    /// Deliver `events` back to back, with no frame between them, as the
    /// platform queues input that arrives before the next redraw; then run
    /// until idle.
    pub fn send_events(&mut self, events: impl IntoIterator<Item = InputEvent>) {
        self.main().send_events(events);
    }

    /// Deliver `events`, each to its window, back to back with no frame
    /// between them, as the platform queues input for several windows
    /// that arrives before their next redraw; then run until idle.
    ///
    /// # Panics
    ///
    /// When a window is not open.
    #[track_caller]
    pub fn send_window_events(
        &mut self,
        events: impl IntoIterator<Item = (WindowHandle, InputEvent)>,
    ) {
        for (window, event) in events {
            self.open(window);
            self.dispatch(window, event);
        }
        self.run_until_idle();
    }

    // ---- Queries ---------------------------------------------------------

    /// The main window's last accessibility tree, one node per line,
    /// indented by depth: `Role "name"`, then `= "value"` when it adds to
    /// the name, `@test_id`, and `[focused]`. Accessibility ids are left
    /// out: most are generated from content and bounds.
    pub fn accessibility_tree(&self) -> String {
        self.accessibility_tree_in(self.main_window())
    }

    /// The last frame's accessibility tree as AccessKit receives it,
    /// announcements included, with bounds in points.
    pub fn accessibility_update(&self) -> TreeUpdate {
        self.accessibility_update_in(self.main_window())
    }

    /// Deliver a request from assistive tech (a click, focus, text
    /// selection, or value change) to the main window, then run until
    /// idle.
    pub fn accessibility_action(&mut self, request: ActionRequest) {
        self.main().accessibility_action(request);
    }

    /// The one node `by` finds in the main window.
    ///
    /// # Panics
    ///
    /// When no node or more than one matches; the message lists the tree
    /// or the matches.
    #[track_caller]
    pub fn find(&self, by: impl Into<By>) -> Node {
        self.find_in(self.main_window(), by.into())
    }

    /// The node `by` finds, if exactly one does.
    pub fn try_find(&self, by: impl Into<By>) -> Option<Node> {
        let mut matches = self.find_all(by);
        (matches.len() == 1).then(|| matches.remove(0))
    }

    /// Every node `by` finds, in tree order.
    pub fn find_all(&self, by: impl Into<By>) -> Vec<Node> {
        self.find_all_in(self.main_window(), &by.into())
    }

    /// Where the last frame's identified elements landed, as
    /// `UiContext::geometry` sees it.
    pub fn geometry(&self) -> &quark_ui::element::LayoutSnapshot {
        self.state(self.main_window()).geometry()
    }

    /// The node holding keyboard focus, if focus is on one.
    pub fn focused(&self) -> Option<Node> {
        self.nodes(self.main_window())
            .into_iter()
            .find(|node| node.focused)
    }

    /// The app's focus target, whether or not a node shows it.
    pub fn focus(&self) -> Option<FocusId> {
        self.state(self.main_window()).focus()
    }

    /// The innermost node under `at` that a click there would reach, by the
    /// last frame's hit regions. Regions without a node of their own report
    /// their nearest ancestor that has one.
    pub fn hit_test(&self, at: (f32, f32)) -> Option<Node> {
        self.hit_test_in(self.main_window(), at)
    }

    /// Every text run the main window's last frame painted, in paint order.
    pub fn painted_texts(&self) -> Vec<PaintedText> {
        painted_texts(self.scene())
    }

    /// The main window's last painted text, one run per line in paint
    /// order.
    pub fn painted_text(&self) -> String {
        painted_text(self.scene())
    }

    /// Render the main window's last frame on a headless GPU device and
    /// read it back. Fails with [`quark_render::RenderError::NoAdapter`] on
    /// hosts without one; skip the test then unless `QUARK_REQUIRE_GPU` is
    /// set.
    #[cfg(feature = "headless-render")]
    pub fn render_rgba(&mut self) -> Result<Pixels, quark_render::RenderError> {
        let main = self.open(self.main_window());
        let scale = main.scale_factor;
        let (width, height) = main.size;
        let options = main.renderer_options;
        let mut scene = main.scene.clone();
        let width = (f64::from(width) * scale).round() as u32;
        let height = (f64::from(height) * scale).round() as u32;
        let renderer = match &mut self.renderer {
            Some(renderer) => renderer,
            None => self
                .renderer
                .insert(quark_render::Renderer::new_headless(width, height, scale)?),
        };
        renderer.resize(width, height, scale);
        renderer.set_options(options);
        crate::scene_to_physical(&mut scene, scale as f32);
        let rgba =
            renderer.render_to_rgba(&scene, &mut self.runner.text().system, width, height)?;
        Ok(Pixels {
            width,
            height,
            rgba,
        })
    }

    // ---- Internals -------------------------------------------------------

    #[track_caller]
    fn open(&self, window: WindowHandle) -> &VirtualWindow {
        self.runner
            .window(window)
            .unwrap_or_else(|| panic!("UiTestHarness: {window:?} is not open"))
    }

    fn scene_in(&self, window: WindowHandle) -> &Scene {
        &self.open(window).scene
    }

    /// The UI adapter's state for `window`: its hit regions, focus, and
    /// accessibility as its last frame and input left them.
    #[track_caller]
    fn state(&self, window: WindowHandle) -> &crate::ui::WindowUiState {
        self.open(window);
        self.adapter
            .window_state(window)
            .unwrap_or_else(|| panic!("UiTestHarness: the app never heard of {window:?}"))
    }

    #[track_caller]
    fn accessibility_update_in(&self, window: WindowHandle) -> TreeUpdate {
        self.open(window);
        self.adapter
            .logical_accessibility_tree(window)
            .unwrap_or_else(|| panic!("UiTestHarness: the app never heard of {window:?}"))
    }

    /// Hand one event to `window`, keeping its pointer and modifiers in step
    /// as the platform layer does.
    #[track_caller]
    fn dispatch(&mut self, window: WindowHandle, event: InputEvent) {
        self.runner.input(&mut self.adapter, window, event);
    }

    #[track_caller]
    fn find_in(&self, window: WindowHandle, by: By) -> Node {
        let mut matches = self.find_all_in(window, &by);
        match matches.len() {
            1 => matches.remove(0),
            0 => panic!(
                "no node matches {by}; the accessibility tree is:\n{}",
                self.accessibility_tree_in(window)
            ),
            count => {
                let list: Vec<String> = matches
                    .iter()
                    .map(|node| {
                        let b = node.bounds;
                        format!("  {node} at {},{} {}x{}", b.x, b.y, b.width, b.height)
                    })
                    .collect();
                panic!(
                    "{count} nodes match {by}, expected one:\n{}",
                    list.join("\n")
                )
            }
        }
    }

    fn find_all_in(&self, window: WindowHandle, by: &By) -> Vec<Node> {
        self.nodes(window)
            .into_iter()
            .filter(|node| node.depth > 0 && by.matches(node))
            .collect()
    }

    fn accessibility_tree_in(&self, window: WindowHandle) -> String {
        let mut out = String::new();
        for node in &self.nodes(window) {
            out.push_str(&"  ".repeat(node.depth));
            out.push_str(&node.to_string());
            out.push('\n');
        }
        out
    }

    fn hit_test_in(&self, window: WindowHandle, (x, y): (f32, f32)) -> Option<Node> {
        let nodes = self.nodes(window);
        let state = self.state(window);
        let semantic = state.semantic_frame();
        let accessibility = state.accessibility_frame();
        let hit = state.semantic_node_at(x, y)?;
        semantic.ancestors_inclusive(hit).find_map(|index| {
            let owner = accessibility.semantic_owner(index);
            nodes
                .iter()
                .find(|node| {
                    owner.is_some_and(|owner| node.accessibility == Some(owner))
                        || node.semantic == Some(index)
                })
                .cloned()
        })
    }

    /// `window`'s last accessibility nodes in tree order, root first, with
    /// semantic nodes that have a test id but no accessibility node after.
    #[track_caller]
    fn nodes(&self, window: WindowHandle) -> Vec<Node> {
        let state = self.state(window);
        let tree = self.accessibility_update_in(window);
        let mut nodes = Vec::new();
        if let Some(root) = tree.tree.as_ref().map(|tree| tree.root) {
            let focused = state.focus().is_some().then_some(tree.focus);
            push_subtree(&tree, root, 0, focused, &mut nodes);
        }

        let accessibility = state.accessibility_frame();
        for (index, semantic) in state.semantic_frame().nodes().iter().enumerate() {
            let Some(test_id) = &semantic.test_id else {
                continue;
            };
            let test_id = test_id.as_str().to_owned();
            let owner = accessibility.semantic_owner(index);
            if let Some(node) = nodes
                .iter_mut()
                .find(|node| owner.is_some() && node.accessibility == owner)
            {
                node.test_id = Some(test_id);
                node.semantic = Some(index);
                continue;
            }
            nodes.push(Node {
                role: None,
                name: semantic.label.as_deref().map(str::to_owned),
                value: semantic.value.as_deref().map(str::to_owned),
                id: None,
                test_id: Some(test_id),
                bounds: semantic.bounds,
                focused: semantic.focus.is_some() && semantic.focus == state.focus(),
                depth: 1,
                accessibility: None,
                semantic: Some(index),
            });
        }
        nodes
    }
}

/// One window of a [`UiTestHarness`], from [`UiTestHarness::window`]: its
/// input, queries, and the controls a desktop user has over it. Each method
/// does what the harness method of the same name does for the main window.
/// Every window keeps its own hit regions, focus, IME, and accessibility
/// tree, so input and queries work for any window in any order.
pub struct UiWindow<'a, U: UiApp> {
    ui: &'a mut UiTestHarness<U>,
    window: WindowHandle,
}

impl<U: UiApp> UiWindow<'_, U> {
    pub fn handle(&self) -> WindowHandle {
        self.window
    }

    /// Where the window is and how big, as
    /// [`crate::EventContext::placement`] reports it to the app.
    pub fn placement(&self) -> WindowPlacement {
        let capabilities = self.ui.runner.capabilities();
        self.ui.open(self.window).placement(capabilities)
    }

    /// The title the window was opened with.
    pub fn title(&self) -> &str {
        &self.ui.open(self.window).title
    }

    /// How many native window moves were started on the window
    /// ([`crate::EventContext::start_window_drag`]), as from app-drawn
    /// title chrome.
    pub fn window_drags(&self) -> u32 {
        self.ui.open(self.window).window_drags
    }

    /// The native material regions the window's frames last asked for
    /// ([`crate::EventContext::set_material_regions`]).
    pub fn material_regions(&self) -> &[crate::platform::material::MaterialRect] {
        &self.ui.open(self.window).material_regions
    }

    /// How many title bar double-click actions were asked of the window
    /// ([`crate::EventContext::title_double_click`]).
    pub fn title_double_clicks(&self) -> u32 {
        self.ui.open(self.window).title_double_clicks
    }

    /// Draw a frame of this window now, whether or not one was asked for,
    /// then run until idle; see [`UiTestHarness::frame`].
    pub fn frame(&mut self) -> &Scene {
        self.ui.runner.draw(&mut self.ui.adapter, self.window);
        self.ui.run_until_idle();
        self.ui.scene_in(self.window)
    }

    /// The window's last painted frame, in points.
    pub fn scene(&self) -> &Scene {
        self.ui.scene_in(self.window)
    }

    pub fn frame_count(&self) -> u64 {
        self.ui.open(self.window).frames_drawn
    }

    pub fn painted_texts(&self) -> Vec<PaintedText> {
        painted_texts(self.scene())
    }

    pub fn painted_text(&self) -> String {
        painted_text(self.scene())
    }

    pub fn size(&self) -> (f32, f32) {
        self.ui.open(self.window).size
    }

    pub fn ime(&self) -> ImeState {
        self.ui.open(self.window).ime
    }

    // ---- Window controls -------------------------------------------------

    /// Resize the window, in points: the app gets
    /// [`AppEvent::WindowResized`] and the window repaints.
    pub fn resize(&mut self, width: f32, height: f32) {
        let ui = &mut *self.ui;
        ui.runner
            .resize(&mut ui.adapter, self.window, (width, height));
        ui.run_until_idle();
    }

    /// Move the window to a display with another scale factor: the app gets
    /// [`AppEvent::WindowScaleChanged`] and the window repaints.
    pub fn set_scale(&mut self, scale_factor: f32) {
        let ui = &mut *self.ui;
        ui.runner
            .set_scale_factor(&mut ui.adapter, self.window, f64::from(scale_factor));
        ui.run_until_idle();
    }

    /// Move the window's outer corner on the desktop, as dragging its title
    /// bar does. The app gets [`AppEvent::WindowMoved`] unless the
    /// capabilities say windows have no positions. The virtual desktop's
    /// units are physical pixels, as on Windows and X11.
    pub fn move_to(&mut self, position: DesktopPoint) {
        let ui = &mut *self.ui;
        ui.runner
            .move_window(&mut ui.adapter, self.window, position);
        ui.run_until_idle();
    }

    /// Give the window decorations: its content area then starts `offset`
    /// desktop units from its outer corner, as
    /// [`WindowPlacement::client_offset`] reports. None by default.
    pub fn set_client_offset(&mut self, offset: DesktopPoint) {
        self.ui.runner.set_client_offset(self.window, offset);
    }

    /// Put the window on `monitor`, as [`WindowPlacement::monitor`] then
    /// reports. The platform sends no event for it.
    pub fn set_monitor(&mut self, monitor: Option<MonitorInfo>) {
        self.ui.runner.set_monitor(self.window, monitor);
    }

    /// Give the window keyboard focus, as clicking it does: the focused
    /// window, if another, loses focus first.
    pub fn focus(&mut self) {
        let ui = &mut *self.ui;
        ui.runner.set_focus(&mut ui.adapter, self.window, true);
        ui.run_until_idle();
    }

    /// The window lost keyboard focus, as when the user switches apps.
    pub fn focus_loss(&mut self) {
        let ui = &mut *self.ui;
        ui.runner.set_focus(&mut ui.adapter, self.window, false);
        ui.run_until_idle();
    }

    /// Ask the app to close the window, as its close button does, and close
    /// it if the app agrees ([`CloseReason::User`]). Returns
    /// [`UiApp::close_requested`]'s answer.
    pub fn request_close(&mut self) -> bool {
        let ui = &mut *self.ui;
        let close = ui
            .runner
            .request_close(&mut ui.adapter, self.window, CloseReason::User);
        ui.run_until_idle();
        close
    }

    // ---- Input -----------------------------------------------------------

    #[track_caller]
    pub fn key(&mut self, binding: &str) {
        let parsed: Binding = binding
            .parse()
            .unwrap_or_else(|error| panic!("UiTestHarness::key: {error}"));
        let (chord, text) = key_press(&parsed)
            .unwrap_or_else(|| panic!("UiTestHarness::key: no key named {:?}", parsed.key));
        let modifiers = chord.modifiers;
        if !modifiers.is_empty() {
            self.dispatch(InputEvent::ModifiersChanged(modifiers));
        }
        self.dispatch(InputEvent::KeyPress(chord.clone()));
        if let Some(text) = text {
            self.dispatch(InputEvent::TextInput(text));
        }
        self.dispatch(InputEvent::KeyRelease(chord));
        if !modifiers.is_empty() {
            self.dispatch(InputEvent::ModifiersChanged(ModifiersState::empty()));
        }
        self.ui.run_until_idle();
    }

    pub fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            if ch == '\n' {
                self.key("enter");
                continue;
            }
            let chord = KeyChord {
                logical: KeyKind::Character(ch.to_string()),
                physical: None,
                modifiers: ModifiersState::empty(),
                repeat: false,
            };
            self.dispatch(InputEvent::KeyPress(chord.clone()));
            self.dispatch(InputEvent::TextInput(ch.to_string()));
            self.dispatch(InputEvent::KeyRelease(chord));
            self.ui.run_until_idle();
        }
    }

    pub fn ime_preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        self.send_event(InputEvent::ImePreedit(text.to_owned(), cursor));
    }

    pub fn ime_commit(&mut self, text: &str) {
        self.dispatch(InputEvent::ImePreedit(String::new(), None));
        self.send_event(InputEvent::ImeCommit(text.to_owned()));
    }

    pub fn pointer_move(&mut self, (x, y): (f32, f32)) {
        self.send_event(InputEvent::PointerMoved { x, y });
    }

    pub fn pointer_leave(&mut self) {
        self.send_event(InputEvent::PointerLeft);
    }

    pub fn pointer_down(&mut self, at: (f32, f32)) {
        self.move_to_point(at);
        self.send_event(button(ElementState::Pressed));
    }

    pub fn pointer_up(&mut self, at: (f32, f32)) {
        self.move_to_point(at);
        self.send_event(button(ElementState::Released));
    }

    pub fn click(&mut self, at: (f32, f32)) {
        self.pointer_down(at);
        self.pointer_up(at);
    }

    #[track_caller]
    pub fn click_node(&mut self, by: impl Into<By>) {
        let node = self.find(by);
        self.click(node.center());
    }

    pub fn middle_click(&mut self, at: (f32, f32)) {
        self.move_to_point(at);
        for state in [ElementState::Pressed, ElementState::Released] {
            self.send_event(InputEvent::PointerButton {
                button: MouseButton::Middle,
                state,
            });
        }
    }

    #[track_caller]
    pub fn middle_click_node(&mut self, by: impl Into<By>) {
        let node = self.find(by);
        self.middle_click(node.center());
    }

    pub fn drag(&mut self, from: (f32, f32), to: (f32, f32)) {
        self.pointer_down(from);
        for step in 1..=DRAG_STEPS {
            let t = step as f32 / DRAG_STEPS as f32;
            let x = from.0 + (to.0 - from.0) * t;
            let y = from.1 + (to.1 - from.1) * t;
            self.pointer_move((x, y));
        }
        self.pointer_up(to);
    }

    pub fn wheel(&mut self, dx: f32, dy: f32) {
        // Platforms report the finger's motion, the opposite way.
        let delta = PhysicalPosition::new(-f64::from(dx), -f64::from(dy));
        self.send_event(InputEvent::Wheel {
            delta: MouseScrollDelta::PixelDelta(delta),
            phase: TouchPhase::Moved,
        });
    }

    #[track_caller]
    pub fn send_event(&mut self, event: InputEvent) {
        self.dispatch(event);
        self.ui.run_until_idle();
    }

    #[track_caller]
    pub fn send_events(&mut self, events: impl IntoIterator<Item = InputEvent>) {
        for event in events {
            self.dispatch(event);
        }
        self.ui.run_until_idle();
    }

    #[track_caller]
    pub fn accessibility_action(&mut self, request: ActionRequest) {
        let window = self.window;
        let ui = &mut *self.ui;
        ui.runner
            .callback_in(window, &mut ui.adapter, |adapter, cx| {
                App::accessibility_action(adapter, request, cx)
            });
        ui.run_until_idle();
    }

    // ---- Queries ---------------------------------------------------------

    pub fn accessibility_tree(&self) -> String {
        self.ui.accessibility_tree_in(self.window)
    }

    pub fn accessibility_update(&self) -> TreeUpdate {
        self.ui.accessibility_update_in(self.window)
    }

    /// The window's focus target, whether or not a node shows it; see
    /// [`UiTestHarness::focus`]. (Not [`Self::focus`], which gives the
    /// native window keyboard focus.)
    pub fn focus_target(&self) -> Option<FocusId> {
        self.ui.state(self.window).focus()
    }

    pub fn geometry(&self) -> &quark_ui::element::LayoutSnapshot {
        self.ui.state(self.window).geometry()
    }

    #[track_caller]
    pub fn find(&self, by: impl Into<By>) -> Node {
        self.ui.find_in(self.window, by.into())
    }

    pub fn try_find(&self, by: impl Into<By>) -> Option<Node> {
        let mut matches = self.find_all(by);
        (matches.len() == 1).then(|| matches.remove(0))
    }

    pub fn find_all(&self, by: impl Into<By>) -> Vec<Node> {
        self.ui.find_all_in(self.window, &by.into())
    }

    pub fn focused(&self) -> Option<Node> {
        self.ui
            .nodes(self.window)
            .into_iter()
            .find(|node| node.focused)
    }

    pub fn hit_test(&self, at: (f32, f32)) -> Option<Node> {
        self.ui.hit_test_in(self.window, at)
    }

    // ---- Internals -------------------------------------------------------

    #[track_caller]
    fn dispatch(&mut self, event: InputEvent) {
        self.ui.dispatch(self.window, event);
    }

    fn move_to_point(&mut self, at: (f32, f32)) {
        if self.ui.open(self.window).pointer != Some(at) {
            self.pointer_move(at);
        }
    }
}

fn painted_texts(scene: &Scene) -> Vec<PaintedText> {
    scene
        .expanded()
        .iter()
        .filter_map(|primitive| {
            let (bounds, layout) = match primitive {
                Primitive::TextRun(run) => (run.rect, &run.layout),
                Primitive::RichTextRun(run) => (run.rect, &run.layout),
                _ => return None,
            };
            let layout = layout.downcast_ref::<quark_text::TextLayout>()?;
            Some(PaintedText {
                text: layout.text().to_string(),
                bounds,
            })
        })
        .collect()
}

fn painted_text(scene: &Scene) -> String {
    let runs: Vec<String> = painted_texts(scene)
        .into_iter()
        .map(|run| run.text)
        .collect();
    runs.join("\n")
}

fn push_subtree(
    tree: &TreeUpdate,
    id: NodeId,
    depth: usize,
    focused: Option<NodeId>,
    out: &mut Vec<Node>,
) {
    let Some((_, node)) = tree.nodes.iter().find(|(node_id, _)| *node_id == id) else {
        return;
    };
    let bounds = node.bounds().map_or(Rect::default(), |b| Rect {
        x: b.x0 as f32,
        y: b.y0 as f32,
        width: (b.x1 - b.x0) as f32,
        height: (b.y1 - b.y0) as f32,
    });
    out.push(Node {
        role: Some(node.role()),
        name: node.label().map(str::to_owned),
        value: node.value().map(str::to_owned),
        id: node.author_id().map(str::to_owned),
        test_id: None,
        bounds,
        // The tree reports the window as focused when nothing else is.
        focused: depth > 0 && focused == Some(id),
        depth,
        accessibility: Some(id),
        semantic: None,
    });
    for &child in node.children() {
        push_subtree(tree, child, depth + 1, focused, out);
    }
}

fn button(state: ElementState) -> InputEvent {
    InputEvent::PointerButton {
        button: MouseButton::Left,
        state,
    }
}

/// The key press a platform would report for `binding`, and the text it
/// would type.
fn key_press(binding: &Binding) -> Option<(KeyChord, Option<String>)> {
    let mods = binding.mods;
    let primary_is_cmd = cfg!(target_os = "macos");
    let mut modifiers = ModifiersState::empty();
    modifiers.set(
        ModifiersState::SUPER,
        mods.cmd || (mods.primary && primary_is_cmd),
    );
    modifiers.set(
        ModifiersState::CONTROL,
        mods.ctrl || (mods.primary && !primary_is_cmd),
    );
    modifiers.set(ModifiersState::ALT, mods.alt);
    modifiers.set(ModifiersState::SHIFT, mods.shift);

    let named = match binding.key.as_str() {
        "enter" => Some(NamedKey::Enter),
        "tab" => Some(NamedKey::Tab),
        "escape" => Some(NamedKey::Escape),
        "space" => Some(NamedKey::Space),
        "arrowup" => Some(NamedKey::ArrowUp),
        "arrowdown" => Some(NamedKey::ArrowDown),
        "arrowleft" => Some(NamedKey::ArrowLeft),
        "arrowright" => Some(NamedKey::ArrowRight),
        "pageup" => Some(NamedKey::PageUp),
        "pagedown" => Some(NamedKey::PageDown),
        "home" => Some(NamedKey::Home),
        "end" => Some(NamedKey::End),
        "backspace" => Some(NamedKey::Backspace),
        "delete" => Some(NamedKey::Delete),
        "f1" => Some(NamedKey::F1),
        "f2" => Some(NamedKey::F2),
        "f3" => Some(NamedKey::F3),
        "f4" => Some(NamedKey::F4),
        "f5" => Some(NamedKey::F5),
        "f6" => Some(NamedKey::F6),
        "f7" => Some(NamedKey::F7),
        "f8" => Some(NamedKey::F8),
        "f9" => Some(NamedKey::F9),
        "f10" => Some(NamedKey::F10),
        "f11" => Some(NamedKey::F11),
        "f12" => Some(NamedKey::F12),
        _ => None,
    };
    let (logical, text) = match named {
        Some(NamedKey::Space) => (KeyKind::Named(NamedKey::Space), Some(" ".to_owned())),
        Some(named) => (KeyKind::Named(named), None),
        None => {
            let mut chars = binding.key.chars();
            let (Some(ch), None) = (chars.next(), chars.next()) else {
                return None;
            };
            let ch = if mods.shift {
                ch.to_ascii_uppercase()
            } else {
                ch
            };
            (KeyKind::Character(ch.to_string()), Some(ch.to_string()))
        }
    };
    let types = !(modifiers.control_key() || modifiers.super_key());
    let chord = KeyChord {
        logical,
        physical: None,
        modifiers,
        repeat: false,
    };
    Some((chord, text.filter(|_| types)))
}

/// How [`UiTestHarness::find`] picks nodes.
#[derive(Debug, Clone, PartialEq)]
pub enum By {
    /// The accessible name (label), exactly.
    Name(String),
    Role(Role),
    RoleAndName(Role, String),
    /// The accessibility id an element was given with `accessibility_id`.
    Id(String),
    /// The test id an element was given with `test_id`.
    TestId(String),
}

impl By {
    pub fn name(name: impl Into<String>) -> Self {
        Self::Name(name.into())
    }

    pub fn role(role: Role) -> Self {
        Self::Role(role)
    }

    pub fn role_name(role: Role, name: impl Into<String>) -> Self {
        Self::RoleAndName(role, name.into())
    }

    pub fn id(id: impl Into<String>) -> Self {
        Self::Id(id.into())
    }

    pub fn test_id(id: impl Into<String>) -> Self {
        Self::TestId(id.into())
    }

    fn matches(&self, node: &Node) -> bool {
        match self {
            Self::Name(name) => node.name.as_deref() == Some(name),
            Self::Role(role) => node.role == Some(*role),
            Self::RoleAndName(role, name) => {
                node.role == Some(*role) && node.name.as_deref() == Some(name)
            }
            Self::Id(id) => node.id.as_deref() == Some(id),
            Self::TestId(id) => node.test_id.as_deref() == Some(id),
        }
    }
}

/// A bare string finds by accessible name.
impl From<&str> for By {
    fn from(name: &str) -> Self {
        Self::name(name)
    }
}

impl From<Role> for By {
    fn from(role: Role) -> Self {
        Self::Role(role)
    }
}

impl fmt::Display for By {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => write!(f, "name {name:?}"),
            Self::Role(role) => write!(f, "role {role:?}"),
            Self::RoleAndName(role, name) => write!(f, "role {role:?} named {name:?}"),
            Self::Id(id) => write!(f, "accessibility id {id:?}"),
            Self::TestId(id) => write!(f, "test id {id:?}"),
        }
    }
}

/// A node of the last frame, as assistive tech sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// `None` for a node only a test id names, which has no accessibility
    /// node.
    pub role: Option<Role>,
    pub name: Option<String>,
    pub value: Option<String>,
    /// The accessibility id.
    pub id: Option<String>,
    pub test_id: Option<String>,
    /// In points, relative to the window.
    pub bounds: Rect,
    pub focused: bool,
    depth: usize,
    accessibility: Option<NodeId>,
    semantic: Option<usize>,
}

impl Node {
    pub fn center(&self) -> (f32, f32) {
        let b = self.bounds;
        (b.x + b.width / 2.0, b.y + b.height / 2.0)
    }
}

impl fmt::Display for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.role {
            Some(role) => write!(f, "{role:?}")?,
            None => f.write_str("-")?,
        }
        if let Some(name) = &self.name {
            write!(f, " {name:?}")?;
        }
        // Static text repeats its name as its value; only say it once.
        if let Some(value) = self
            .value
            .as_ref()
            .filter(|v| Some(*v) != self.name.as_ref())
        {
            write!(f, " = {value:?}")?;
        }
        if let Some(id) = &self.test_id {
            write!(f, " @{id}")?;
        }
        if self.focused {
            f.write_str(" [focused]")?;
        }
        Ok(())
    }
}

/// One text run of a painted frame.
#[derive(Debug, Clone, PartialEq)]
pub struct PaintedText {
    pub text: String,
    /// Where the run was drawn, in points.
    pub bounds: Rect,
}

/// A frame rendered by [`UiTestHarness::render_rgba`]: sRGB RGBA8 rows in
/// physical pixels.
#[cfg(feature = "headless-render")]
#[derive(Debug, Clone)]
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[cfg(feature = "headless-render")]
impl Pixels {
    /// The pixel at physical `(x, y)`.
    ///
    /// # Panics
    ///
    /// When the point is outside the image.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is outside {}x{}",
            self.width,
            self.height
        );
        let at = ((y * self.width + x) * 4) as usize;
        [
            self.rgba[at],
            self.rgba[at + 1],
            self.rgba[at + 2],
            self.rgba[at + 3],
        ]
    }
}

#[cfg(test)]
mod tests {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    use quark_ui::Action;
    use quark_ui::element::{AnyElement, IntoAnyElement, div, text};
    use quark_ui::style::Styled;

    use super::*;
    use crate::{UiContext, ViewContext};

    #[derive(Debug, Clone, PartialEq)]
    struct Save;

    impl From<Save> for Action {
        fn from(save: Save) -> Self {
            Action::new(save)
        }
    }

    /// Two buttons both named "Save" (one showing text), and a mod+s
    /// binding, counting saves.
    #[derive(Default)]
    struct Saver {
        saves: usize,
    }

    impl UiApp for Saver {
        type Action = Save;
        type Message = ();

        fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
            let button = || {
                div()
                    .w(80.0)
                    .h(30.0)
                    .accessibility_role(Role::Button)
                    .accessibility_label("Save")
                    .on_click(Save)
            };
            div()
                .w(400.0)
                .h(300.0)
                .flex_col()
                .on_key("mod+s", Save)
                .child(button().child(text("Save now")))
                .child(button())
                .into_any()
        }

        fn update(&mut self, Save: Save, _cx: &mut UiContext) {
            self.saves += 1;
        }
    }

    fn saver() -> UiTestHarness<Saver> {
        UiTestHarness::new(Saver::default(), (400.0, 300.0), 1.0)
    }

    fn panic_message(f: impl FnOnce()) -> String {
        let payload = catch_unwind(AssertUnwindSafe(f)).expect_err("expected a panic");
        payload
            .downcast_ref::<String>()
            .cloned()
            .expect("a formatted panic message")
    }

    // A failed lookup must say what was asked for and what exists, or a
    // test author has to add prints to see why it failed.
    #[test]
    fn find_failures_name_the_selector_and_what_is_there() {
        let ui = saver();
        let cases = [
            (
                By::name("Open"),
                "no node matches name \"Open\"; the accessibility tree is:\n\
                 Window \"Test\"\n\
                 \x20 Button \"Save\"\n\
                 \x20 Button \"Save\"\n",
            ),
            (
                By::role_name(Role::Button, "Save"),
                "2 nodes match role Button named \"Save\", expected one:\n\
                 \x20 Button \"Save\" at 0,0 80x30\n\
                 \x20 Button \"Save\" at 0,30 80x30",
            ),
        ];
        for (by, expected) in cases {
            assert_eq!(panic_message(|| drop(ui.find(by))), expected);
        }
    }

    // Clicks land on the control, not the text drawn on it: a hit on a
    // button's text reports the button.
    #[test]
    fn hit_test_on_a_buttons_text_reports_the_button() {
        let ui = saver();
        let text = ui.painted_texts().remove(0);
        let b = text.bounds;

        let hit = ui.hit_test((b.x + b.width / 2.0, b.y + b.height / 2.0));
        let hit = hit.expect("a node under the text");

        assert_eq!((hit.role, hit.bounds.y), (Some(Role::Button), 0.0));
    }

    // Pixels are read back at physical size: a box at logical (10, 20)
    // covers physical (20, 40) and up at scale 2, and nothing before it.
    #[test]
    fn render_rgba_draws_the_frame_at_physical_size() {
        const BOX: quark::Color = quark::Color::rgba(200, 40, 40, 255);
        struct Boxed;
        impl UiApp for Boxed {
            type Action = Save;
            type Message = ();
            fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
                let boxed = div()
                    .absolute()
                    .left(10.0)
                    .top(20.0)
                    .w(100.0)
                    .h(50.0)
                    .bg(BOX);
                div()
                    .w(200.0)
                    .h(100.0)
                    .bg(quark::Color::rgba(0, 0, 0, 255))
                    .child(boxed)
                    .into_any()
            }
            fn update(&mut self, Save: Save, _cx: &mut UiContext) {}
        }
        let mut ui = UiTestHarness::new(Boxed, (200.0, 100.0), 2.0);

        let pixels = match ui.render_rgba() {
            Ok(pixels) => pixels,
            Err(quark_render::RenderError::NoAdapter) => {
                assert!(
                    std::env::var_os("QUARK_REQUIRE_GPU").is_none(),
                    "QUARK_REQUIRE_GPU is set but no wgpu adapter is available"
                );
                eprintln!("skipping: no wgpu adapter available");
                return;
            }
            Err(error) => panic!("headless render failed: {error}"),
        };

        assert_eq!((pixels.width, pixels.height), (400, 200));
        let probes = [(21, 41), (218, 138), (19, 41), (21, 39)].map(|(x, y)| pixels.pixel(x, y));
        assert_eq!(
            probes,
            [
                [200, 40, 40, 255],
                [200, 40, 40, 255],
                [0, 0, 0, 255],
                [0, 0, 0, 255]
            ]
        );
    }

    // `mod` must become the platform's own modifier, or `mod+` bindings
    // never fire from tests on one of the platforms.
    #[test]
    fn mod_key_presses_fire_mod_bindings() {
        let mut ui = saver();

        ui.key("mod+s");

        assert_eq!(ui.app().saves, 1);
    }
}
