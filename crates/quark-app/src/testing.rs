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

use crate::runner::HeadlessRunner;
use crate::{App, AppEvent, InputEvent, KeyChord, KeyKind, UiAdapter, UiApp, UiSender};

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

/// A [`UiApp`] in a headless window. See the [module docs](self).
pub struct UiTestHarness<U: UiApp> {
    adapter: UiAdapter<U>,
    runner: HeadlessRunner,
    sender: UiSender<U::Message>,
    /// The last painted frame, in points.
    scene: Scene,
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
        let mut harness = Self {
            sender: adapter.sender(),
            adapter,
            runner: HeadlessRunner::new(size, f64::from(scale_factor)),
            scene: Scene::default(),
            #[cfg(feature = "headless-render")]
            renderer: None,
        };
        harness.runner.callback(&mut harness.adapter, App::init);
        harness.runner.request_frame();
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

    // ---- Time and frames -------------------------------------------------

    /// The fake clock: time since the harness started.
    pub fn now(&self) -> Duration {
        Duration::from_millis(self.runner.now_ms())
    }

    /// Deliver pending [`UiSender`] messages and wakes, then draw the frame
    /// due now if one was asked for, until neither is left. The clock does
    /// not move. Every input method ends with this.
    ///
    /// # Panics
    ///
    /// When the app never settles, such as by waking itself from every
    /// [`UiApp::wake`].
    pub fn run_until_idle(&mut self) {
        if let Some(scene) = self.runner.run_until_idle(&mut self.adapter) {
            self.keep_scene(scene);
        }
    }

    /// Keep `scene` as the last frame and hand the one before back to the
    /// app, as the event loop does once a frame is rendered.
    fn keep_scene(&mut self, scene: Scene) {
        let previous = std::mem::replace(&mut self.scene, scene);
        self.adapter.recycle_scene(previous);
    }

    /// Move the clock forward `ms`, drawing every frame requested on the way
    /// at its own time: animations step, timers fire, and selection
    /// autoscroll runs as they would in real time.
    pub fn advance(&mut self, ms: u64) {
        if let Some(scene) = self.runner.advance(&mut self.adapter, ms) {
            self.keep_scene(scene);
        }
    }

    /// Draw a frame now whether or not one was asked for, and return it.
    ///
    /// It then runs [`Self::run_until_idle`], which can draw more frames
    /// before returning: those that delivered messages and wakes make due,
    /// and any requested for a clock time at or before now. A frame's own
    /// requests to draw again right away go to the next 16 ms vsync, which
    /// only [`Self::advance`] reaches. An allocation count around this call
    /// therefore covers the drawn frame plus that follow-up work, and the
    /// returned scene is the last frame drawn.
    pub fn frame(&mut self) -> &Scene {
        let scene = self.runner.draw(&mut self.adapter);
        self.keep_scene(scene);
        self.run_until_idle();
        &self.scene
    }

    /// The last painted frame, in points.
    pub fn scene(&self) -> &Scene {
        &self.scene
    }

    /// Frames drawn since the harness started, counting the first. Compare
    /// before and after an input to see whether it repainted.
    /// Whether frames see assistive tech listening (on by default). While
    /// off, frames skip building the accessibility tree, as they do when no
    /// screen reader is connected.
    pub fn set_accessibility_active(&mut self, active: bool) {
        self.runner.accessibility_active = active;
    }

    pub fn frame_count(&self) -> u64 {
        self.runner.frames_drawn()
    }

    /// Whether a frame is scheduled, for a running animation or timer.
    /// After [`Self::run_until_idle`] none is due now, so any is later.
    pub fn frame_requested(&self) -> bool {
        self.runner.next_frame_ms().is_some()
    }

    /// How long until the next scheduled frame.
    pub fn next_frame_in(&self) -> Option<Duration> {
        let now = self.runner.now_ms();
        self.runner
            .next_frame_ms()
            .map(|ms| Duration::from_millis(ms.saturating_sub(now)))
    }

    // ---- Window ----------------------------------------------------------

    /// Resize the window, in points, and repaint.
    pub fn resize(&mut self, width: f32, height: f32) {
        self.runner.resize((width, height));
        self.run_until_idle();
    }

    /// Move the window to a display with another scale factor and repaint.
    pub fn set_scale(&mut self, scale_factor: f32) {
        self.runner.set_scale_factor(f64::from(scale_factor));
        self.run_until_idle();
    }

    pub fn size(&self) -> (f32, f32) {
        self.runner.size()
    }

    /// The window lost keyboard focus, as when the user switches apps.
    pub fn focus_loss(&mut self) {
        self.send_event(InputEvent::Focused(false));
    }

    pub fn focus_gain(&mut self) {
        self.send_event(InputEvent::Focused(true));
    }

    /// Ask the app to close the window, as the close button does. Returns
    /// [`UiApp::close_requested`]'s answer.
    pub fn request_close(&mut self) -> bool {
        let close = self.runner.callback(&mut self.adapter, |adapter, cx| {
            App::close_requested(adapter, cx)
        });
        self.run_until_idle();
        close
    }

    /// Whether the app called `exit` on a context.
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
    /// launch.
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
    /// `"enter"`, in the syntax key bindings use. `mod` is Cmd on macOS and
    /// Ctrl elsewhere. A printable key without Cmd or Ctrl also types its
    /// character, as a real key press does.
    ///
    /// # Panics
    ///
    /// When `binding` does not parse or names a key the runner cannot
    /// report, such as F13.
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
        self.run_until_idle();
    }

    /// Type `text` one character at a time, each as a key press that
    /// inserts it; `\n` presses Enter.
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
            self.run_until_idle();
        }
    }

    /// Show an IME composition in the focused field: `text` with the caret
    /// or selection at the byte range `cursor`. Empty text cancels it.
    pub fn ime_preedit(&mut self, text: &str, cursor: Option<(usize, usize)>) {
        self.send_event(InputEvent::ImePreedit(text.to_owned(), cursor));
    }

    /// What frames asked of the window's IME so far: whether it is on,
    /// where its candidate window was placed, and how often it was reset.
    pub fn ime(&self) -> ImeState {
        self.runner.ime
    }

    /// End the composition by committing `text`, as an IME does: the
    /// preedit clears, then the text is inserted.
    pub fn ime_commit(&mut self, text: &str) {
        self.dispatch(InputEvent::ImePreedit(String::new(), None));
        self.send_event(InputEvent::ImeCommit(text.to_owned()));
    }

    // ---- Pointer ---------------------------------------------------------

    pub fn pointer_move(&mut self, (x, y): (f32, f32)) {
        self.send_event(InputEvent::PointerMoved { x, y });
    }

    /// The pointer left the window.
    pub fn pointer_leave(&mut self) {
        self.send_event(InputEvent::PointerLeft);
    }

    /// Move to `at` if the pointer is elsewhere, then press the primary
    /// button.
    pub fn pointer_down(&mut self, at: (f32, f32)) {
        self.move_to(at);
        self.send_event(button(ElementState::Pressed));
    }

    /// Move to `at` if the pointer is elsewhere, then release the primary
    /// button.
    pub fn pointer_up(&mut self, at: (f32, f32)) {
        self.move_to(at);
        self.send_event(button(ElementState::Released));
    }

    /// Press and release the primary button at `at`.
    pub fn click(&mut self, at: (f32, f32)) {
        self.pointer_down(at);
        self.pointer_up(at);
    }

    /// Click the center of the one node `by` finds.
    ///
    /// # Panics
    ///
    /// When no node or more than one matches.
    #[track_caller]
    pub fn click_node(&mut self, by: impl Into<By>) {
        let node = self.find(by);
        self.click(node.center());
    }

    /// Press and release the middle button at `at`.
    pub fn middle_click(&mut self, at: (f32, f32)) {
        self.move_to(at);
        for state in [ElementState::Pressed, ElementState::Released] {
            self.send_event(InputEvent::PointerButton {
                button: MouseButton::Middle,
                state,
            });
        }
    }

    /// Middle click the center of the one node `by` finds.
    ///
    /// # Panics
    ///
    /// When no node or more than one matches.
    #[track_caller]
    pub fn middle_click_node(&mut self, by: impl Into<By>) {
        let node = self.find(by);
        self.middle_click(node.center());
    }

    /// Press at `from`, move to `to` in a few steps, and release there.
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

    /// Scroll the wheel at the pointer by points; positive values scroll
    /// content down and right. Move the pointer over the target first.
    pub fn wheel(&mut self, dx: f32, dy: f32) {
        // Platforms report the finger's motion, the opposite way.
        let delta = PhysicalPosition::new(-f64::from(dx), -f64::from(dy));
        self.send_event(InputEvent::Wheel {
            delta: MouseScrollDelta::PixelDelta(delta),
            phase: TouchPhase::Moved,
        });
    }

    /// Deliver any input event, then run until idle.
    pub fn send_event(&mut self, event: InputEvent) {
        self.dispatch(event);
        self.run_until_idle();
    }

    /// Deliver `events` back to back, with no frame between them, as the
    /// platform queues input that arrives before the next redraw; then run
    /// until idle.
    pub fn send_events(&mut self, events: impl IntoIterator<Item = InputEvent>) {
        for event in events {
            self.dispatch(event);
        }
        self.run_until_idle();
    }

    // ---- Queries ---------------------------------------------------------

    /// The last frame's accessibility tree, one node per line, indented by
    /// depth: `Role "name"`, then `= "value"` when it adds to the name,
    /// `@test_id`, and `[focused]`. Accessibility ids are left out: most
    /// are generated from content and bounds.
    pub fn accessibility_tree(&self) -> String {
        let nodes = self.nodes();
        let mut out = String::new();
        for node in &nodes {
            out.push_str(&"  ".repeat(node.depth));
            out.push_str(&node.to_string());
            out.push('\n');
        }
        out
    }

    /// The last frame's accessibility tree as AccessKit receives it,
    /// announcements included, with bounds in points.
    pub fn accessibility_update(&self) -> TreeUpdate {
        self.adapter.logical_accessibility_tree()
    }

    /// Deliver a request from assistive tech (a click, focus, text
    /// selection, or value change), then run until idle.
    pub fn accessibility_action(&mut self, request: ActionRequest) {
        self.runner.callback(&mut self.adapter, |adapter, cx| {
            App::accessibility_action(adapter, request, cx)
        });
        self.run_until_idle();
    }

    /// The one node `by` finds.
    ///
    /// # Panics
    ///
    /// When no node or more than one matches; the message lists the tree
    /// or the matches.
    #[track_caller]
    pub fn find(&self, by: impl Into<By>) -> Node {
        let by = by.into();
        let mut matches = self.find_all(by.clone());
        match matches.len() {
            1 => matches.remove(0),
            0 => panic!(
                "no node matches {by}; the accessibility tree is:\n{}",
                self.accessibility_tree()
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

    /// The node `by` finds, if exactly one does.
    pub fn try_find(&self, by: impl Into<By>) -> Option<Node> {
        let mut matches = self.find_all(by);
        (matches.len() == 1).then(|| matches.remove(0))
    }

    /// Every node `by` finds, in tree order.
    pub fn find_all(&self, by: impl Into<By>) -> Vec<Node> {
        let by = by.into();
        self.nodes()
            .into_iter()
            .filter(|node| node.depth > 0 && by.matches(node))
            .collect()
    }

    /// Where the last frame's identified elements landed, as
    /// `UiContext::geometry` sees it.
    pub fn geometry(&self) -> &quark_ui::element::LayoutSnapshot {
        self.adapter.geometry()
    }

    /// The node holding keyboard focus, if focus is on one.
    pub fn focused(&self) -> Option<Node> {
        self.nodes().into_iter().find(|node| node.focused)
    }

    /// The app's focus target, whether or not a node shows it.
    pub fn focus(&self) -> Option<FocusId> {
        self.adapter.focus()
    }

    /// The innermost node under `at` that a click there would reach, by the
    /// last frame's hit regions. Regions without a node of their own report
    /// their nearest ancestor that has one.
    pub fn hit_test(&self, (x, y): (f32, f32)) -> Option<Node> {
        let semantic = self.adapter.semantic_frame();
        let accessibility = self.adapter.accessibility_frame();
        let hit = self.adapter.semantic_node_at(x, y)?;
        let nodes = self.nodes();
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

    /// Every text run the last frame painted, in paint order.
    pub fn painted_texts(&self) -> Vec<PaintedText> {
        self.scene
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

    /// The last frame's painted text, one run per line in paint order.
    pub fn painted_text(&self) -> String {
        let runs: Vec<String> = self
            .painted_texts()
            .into_iter()
            .map(|run| run.text)
            .collect();
        runs.join("\n")
    }

    /// Render the last frame on a headless GPU device and read it back.
    /// Fails with [`quark_render::RenderError::NoAdapter`] on hosts
    /// without one; skip the test then unless `QUARK_REQUIRE_GPU` is set.
    #[cfg(feature = "headless-render")]
    pub fn render_rgba(&mut self) -> Result<Pixels, quark_render::RenderError> {
        let scale = self.runner.input.scale_factor;
        let (width, height) = self.runner.size();
        let width = (f64::from(width) * scale).round() as u32;
        let height = (f64::from(height) * scale).round() as u32;
        let renderer = match &mut self.renderer {
            Some(renderer) => renderer,
            None => self
                .renderer
                .insert(quark_render::Renderer::new_headless(width, height, scale)?),
        };
        renderer.resize(width, height, scale);
        let mut scene = self.scene.clone();
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

    /// Hand one event to the app, keeping the window's pointer and
    /// modifiers in step as the platform layer does.
    fn dispatch(&mut self, event: InputEvent) {
        match &event {
            InputEvent::PointerMoved { x, y } => self.runner.input.pointer = Some((*x, *y)),
            InputEvent::PointerLeft => self.runner.input.pointer = None,
            InputEvent::ModifiersChanged(modifiers) => self.runner.input.modifiers = *modifiers,
            _ => {}
        }
        self.runner.callback(&mut self.adapter, |adapter, cx| {
            App::event(adapter, event, cx)
        });
    }

    fn move_to(&mut self, at: (f32, f32)) {
        if self.runner.input.pointer != Some(at) {
            self.pointer_move(at);
        }
    }

    /// The last frame's accessibility nodes in tree order, root first, with
    /// semantic nodes that have a test id but no accessibility node after.
    fn nodes(&self) -> Vec<Node> {
        let tree = self.adapter.logical_accessibility_tree();
        let mut nodes = Vec::new();
        if let Some(root) = tree.tree.as_ref().map(|tree| tree.root) {
            let focused = self.adapter.focus().is_some().then_some(tree.focus);
            push_subtree(&tree, root, 0, focused, &mut nodes);
        }

        let accessibility = self.adapter.accessibility_frame();
        for (index, semantic) in self.adapter.semantic_frame().nodes().iter().enumerate() {
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
                focused: semantic.focus.is_some() && semantic.focus == self.adapter.focus(),
                depth: 1,
                accessibility: None,
                semantic: Some(index),
            });
        }
        nodes
    }
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
