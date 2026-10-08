//! `UiApp` hooks and module dispatch. Builds each surface's context,
//! routes actions to their surface, applies the effects surfaces return,
//! and plays the scenario. No surface logic lives here.

use std::time::Duration;

use quark::view;
use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::accessibility::Politeness;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::quark_ui::theme::Theme;
use quark_app::{
    AppEvent, CloseReason, InputEvent, UiApp, UiContext, UiSender, ViewContext, WindowHandle,
};
use quark_components::{Dock, HostId, PanelId};

use crate::contracts::{
    CommandId, EditCx, Effect, Effects, Msg, Options, ProjectId, SurfaceCx, Toast, ToastKind,
    WidthPolicy, panel_title, panels,
};
use crate::design::tokens;
use crate::model::Model;
use crate::perf::Marks;
use crate::scenario::{DemoClock, Event, EventKind, SCENARIO_TIME, Scenario};
use crate::{composer, design, dock, fixtures, overlays, settings, shell, timeline};

/// A surface context for the window `cx` is bound to, borrowing only
/// `self.model` so the surfaces' own states stay free to borrow mutably.
macro_rules! surface_cx {
    ($self:ident, $theme:expr, $cx:expr) => {
        SurfaceCx {
            model: &$self.model,
            theme: $theme,
            size: $self.main_size,
            scale: 1.0,
            window: $cx.window_handle(),
            host: $cx
                .window_handle()
                .and_then(|w| $self.dock.windows.host(w))
                .unwrap_or(HostId::MAIN),
            width_policy: $self.policy,
            focus: $cx.focus(),
            now_ms: $self.clock.now($self.runner_ms),
            manual_clock: $self.clock.is_manual(),
        }
    };
}

/// Values the app sends itself.
#[derive(Debug)]
pub enum Message {
    /// Announce what playback queued during the last frame (views have no
    /// `UiContext` to announce through).
    Flush,
}

pub struct Workbench {
    pub options: Options,
    pub model: Model,
    pub scenario: Scenario,
    pub clock: DemoClock,
    pub shell: shell::State,
    pub timeline: timeline::State,
    pub composer: composer::State,
    pub dock: dock::State,
    pub overlays: overlays::State,
    pub settings: settings::State,
    pub marks: Marks,
    fx: Effects,
    events: Vec<Event>,
    sender: Option<UiSender<Message>>,
    /// Announcements playback queued for the next `Message::Flush`.
    announcements: Vec<String>,
    /// Runner time of the latest callback, for text edits (which get no
    /// context).
    runner_ms: u64,
    /// The main window's size and width policy as of its last frame.
    main_size: (f32, f32),
    policy: WidthPolicy,
}

impl Workbench {
    pub fn new(options: Options) -> Self {
        let (model, script) = fixtures::load(&options);
        let clock = if options.manual_clock {
            DemoClock::manual()
        } else {
            DemoClock::live()
        };
        let mut dock = dock::new_state(&options);
        if let Some(dir) = &options.state_dir {
            let windows = std::mem::replace(
                &mut dock.windows,
                quark_app::dock_windows::DockWindows::new(|id| panel_title(id).to_owned()),
            );
            dock.windows = windows.save_to(dir.join("dock.json"));
            dock.windows.restore(&mut dock.layout, dock::known_panel);
        }
        Self {
            model,
            scenario: Scenario::new(script),
            clock,
            shell: shell::new_state(),
            timeline: timeline::new_state(),
            composer: composer::new_state(),
            dock,
            overlays: overlays::new_state(),
            settings: settings::new_state(),
            marks: Marks::new(options.perf_out.is_some()),
            fx: Effects::default(),
            events: Vec::new(),
            sender: None,
            announcements: Vec::new(),
            runner_ms: 0,
            main_size: (0.0, 0.0),
            policy: WidthPolicy {
                sidebar_docked: true,
                right_dock_shown: true,
            },
            options,
        }
    }

    /// The scenario's current time.
    pub fn now_ms(&self) -> u64 {
        self.clock.now(self.runner_ms)
    }

    /// Apply scenario events due by now and adopt one batch of queued
    /// history. Runs at the top of each main-window frame, so a frame
    /// drawn at an event's due time shows it. Returns whether anything
    /// changed.
    fn pump(&mut self) -> bool {
        let now = self.now_ms();
        self.scenario.advance_to(now, &mut self.events);
        let mut changed = false;
        for event in self.events.drain(..) {
            if !self.model.apply(&event) {
                continue;
            }
            changed = true;
            // Completions are announced once; streamed chunks never are.
            match &event.kind {
                EventKind::ToolDone { status, .. } => {
                    let word = match status {
                        crate::model::ToolStatus::Failed => "Tool failed",
                        _ => "Tool finished",
                    };
                    self.announcements.push(word.to_owned());
                }
                EventKind::Done { summary } => {
                    self.announcements.push(format!("Run complete. {summary}"));
                }
                _ => {}
            }
        }
        if self.model.history_pending() > 0 {
            let thread = self.model.selected;
            if self.model.load_history_batch(thread) == 0 {
                self.marks.history_ready(self.runner_ms);
            }
            changed = true;
        }
        if !self.announcements.is_empty()
            && let Some(sender) = &self.sender
        {
            sender.send(Message::Flush);
        }
        changed
    }

    /// Ask for the frame that plays the next due event, and for the next
    /// history batch.
    fn schedule(&mut self, vcx: &mut ViewContext) {
        if self.model.history_pending() > 0 {
            vcx.frame.request_frame();
        }
        if self.clock.is_manual() {
            return;
        }
        if let Some(due) = self.scenario.next_due() {
            let wait = due.saturating_sub(self.now_ms());
            vcx.frame.request_frame_in(Duration::from_millis(wait));
        }
    }

    fn apply_effects(&mut self, cx: &mut UiContext) {
        // Effects may queue more effects; keep going until none are left.
        let mut batch = Vec::new();
        while !self.fx.is_empty() {
            batch.extend(self.fx.drain());
            for effect in batch.drain(..) {
                self.apply_effect(effect, cx);
            }
        }
        cx.window.request_redraw_all();
    }

    fn apply_effect(&mut self, effect: Effect, cx: &mut UiContext) {
        match effect {
            Effect::Command(id) => self.run_command(id, cx),
            Effect::SelectThread(id) => {
                self.model.select(id);
            }
            Effect::SendPrompt { thread, text, .. } => self.send_prompt(thread, &text, cx),
            Effect::StopRun(thread) => {
                if let Some(generation) = self.model.stop_run(thread) {
                    self.scenario.cancel(generation);
                    cx.announce("Run stopped", Politeness::Polite);
                }
            }
            Effect::RetryTool { thread, .. } => {
                self.send_prompt(thread, "Retry the last step.", cx);
            }
            Effect::OpenFile(path) => {
                self.shell.dock_wanted = true;
                dock::open_file(&mut self.dock, &path);
            }
            Effect::RevealDiff(path) => {
                self.shell.dock_wanted = true;
                dock::reveal_diff(&mut self.dock, path.as_deref());
            }
            Effect::ApplyDiff => self.apply_diff(),
            Effect::UndoDiff => {
                let text = if self.model.files.undo() {
                    "Changes undone"
                } else {
                    "Nothing to undo"
                };
                self.fx.push(Effect::Toast(Toast {
                    kind: ToastKind::Info,
                    text: text.to_owned(),
                    undo: None,
                }));
            }
            Effect::Toast(toast) => {
                let scx = surface_cx!(self, cx.theme(), cx);
                overlays::toast(&mut self.overlays, toast, &scx);
            }
            Effect::Focus(target) => cx.set_focus(target),
            Effect::Announce(text) => cx.announce(text, Politeness::Polite),
            Effect::CopyText(text) => cx.window.set_clipboard_text(&text),
            Effect::SetTheme(choice) => {
                let (light, dark) = design::themes_for(choice);
                cx.set_themes(light, dark);
            }
            Effect::SetRegionVisible(region, visible) => {
                dock::set_region_visible(&mut self.dock, region, visible);
            }
        }
    }

    fn send_prompt(&mut self, thread: crate::contracts::ThreadId, text: &str, cx: &mut UiContext) {
        // A new prompt replaces a run still going on that thread.
        if let Some(old) = self.model.stop_run(thread) {
            self.scenario.cancel(old);
        }
        let row = self.scenario.allocate_id();
        let Some(generation) = self.model.begin_run(thread, row, text, SCENARIO_TIME) else {
            return;
        };
        self.scenario.start(thread, generation, self.now_ms());
        cx.announce("Prompt sent", Politeness::Polite);
    }

    fn apply_diff(&mut self) {
        let result = quark_diff::parse_unified(fixtures::DIFF_PATCH)
            .map_err(|e| e.to_string())
            .and_then(|patch| self.model.files.apply(&patch));
        let toast = match result {
            Ok(()) => Toast {
                kind: ToastKind::Success,
                text: "Applied changes to 2 files".to_owned(),
                undo: Some(CommandId::UndoDiff),
            },
            Err(e) => Toast {
                kind: ToastKind::Error,
                text: format!("Could not apply the diff: {e}"),
                undo: None,
            },
        };
        self.fx.push(Effect::Toast(toast));
    }

    /// Offer `id` to the surfaces that own commands, then handle the
    /// app-level rest.
    fn run_command(&mut self, id: CommandId, cx: &mut UiContext) {
        let handled = {
            let scx = surface_cx!(self, cx.theme(), cx);
            let fx = &mut self.fx;
            overlays::command(&mut self.overlays, id, &scx, fx)
                || settings::command(&mut self.settings, id, &scx, fx)
                || shell::command(&mut self.shell, id, &scx, fx)
                || timeline::command(&mut self.timeline, id, &scx, fx)
                || composer::command(&mut self.composer, id, &scx, fx)
                || dock::command(&mut self.dock, id, &scx, fx)
        };
        if handled {
            return;
        }
        match id {
            CommandId::NewThread => {
                let project = self
                    .model
                    .selected_thread()
                    .map_or(ProjectId(1), |t| t.project);
                self.model.new_thread(project);
                self.fx.push(Effect::Focus(Some(composer::INPUT)));
            }
            CommandId::ThemeSystem => self
                .fx
                .push(Effect::SetTheme(crate::contracts::ThemeChoice::System)),
            CommandId::ThemeLight => self
                .fx
                .push(Effect::SetTheme(crate::contracts::ThemeChoice::Light)),
            CommandId::ThemeDark => self
                .fx
                .push(Effect::SetTheme(crate::contracts::ThemeChoice::Dark)),
            CommandId::AdvanceDemoStep => {
                if let Some(due) = self.scenario.next_due() {
                    self.clock.set(due);
                }
            }
            CommandId::ResetDemo => {
                let options = self.options.clone();
                let (model, script) = fixtures::load(&options);
                self.model = model;
                self.scenario = Scenario::new(script);
                if self.clock.is_manual() {
                    self.clock = DemoClock::manual();
                }
                self.timeline = timeline::new_state();
                self.composer = composer::new_state();
            }
            CommandId::ShowDiff | CommandId::ShowFiles | CommandId::ShowPreview => {
                self.fx.push(Effect::RevealDiff(None));
            }
            CommandId::ApplyDiff => self.fx.push(Effect::ApplyDiff),
            CommandId::UndoDiff => self.fx.push(Effect::UndoDiff),
            CommandId::StopRun => self.fx.push(Effect::StopRun(self.model.selected)),
            // Owned by a surface that has not claimed it yet.
            _ => {}
        }
    }

    /// The thread panel: timeline over composer.
    fn thread_column(
        timeline: &mut timeline::State,
        composer: &mut composer::State,
        scx: &SurfaceCx,
        vcx: &mut ViewContext,
    ) -> AnyElement {
        let (width, height) = scx.size;
        let composer_h = composer::height(composer, scx).min(height);
        let t = timeline::view(
            timeline,
            &SurfaceCx {
                size: (width, height - composer_h),
                ..*scx
            },
            vcx,
        );
        let c = composer::view(
            composer,
            &SurfaceCx {
                size: (width, composer_h),
                ..*scx
            },
            vcx,
        );
        view! {
            <div w={width} h={height} class="flex-col" test_id="thread-panel">
                {t}
                {c}
            </div>
        }
        .into_any()
    }
}

impl UiApp for Workbench {
    type Action = Msg;
    type Message = Message;

    fn init(&mut self, cx: &mut UiContext) {
        self.sender = Some(cx.sender::<Message>());
        dock::init(&mut self.dock, cx);
    }

    fn view(&mut self, vcx: &mut ViewContext) -> AnyElement {
        let window = vcx.window_handle();
        let size = vcx.frame.size();
        self.runner_ms = vcx.frame.elapsed().as_millis() as u64;
        let Some(host) = self.dock.windows.host(window) else {
            return div().w(size.0).h(size.1).into_any();
        };
        let main = host == HostId::MAIN;
        if main {
            self.pump();
            self.schedule(vcx);
        }
        if main {
            self.main_size = size;
            self.policy = shell::sync_width(&mut self.shell, size.0, &mut self.fx);
            for effect in self.fx.drain() {
                if let Effect::SetRegionVisible(region, visible) = effect {
                    dock::set_region_visible(&mut self.dock, region, visible);
                }
            }
        }
        let theme: &Theme = vcx.theme;
        let base = SurfaceCx {
            model: &self.model,
            theme,
            size,
            scale: vcx.frame.scale_factor(),
            window: Some(window),
            host,
            width_policy: self.policy,
            focus: vcx.focus(),
            now_ms: self.clock.now(self.runner_ms),
            manual_clock: self.clock.is_manual(),
        };
        let top = if main { tokens::TOP_BAR } else { 0.0 };
        let titlebar = main.then(|| {
            shell::titlebar(
                &mut self.shell,
                &SurfaceCx {
                    size: (size.0, top),
                    ..base
                },
                vcx,
            )
        });
        let dock_size = (size.0, (size.1 - top).max(0.0));
        let overlay_open = main && shell::overlay_open(&self.shell, self.policy);
        let shell_state = &mut self.shell;
        let timeline_state = &mut self.timeline;
        let composer_state = &mut self.composer;
        let panel_states = &mut self.dock.panels;
        let mut content = |id: PanelId, psize: (f32, f32)| {
            let scx = SurfaceCx {
                size: psize,
                ..base
            };
            match id {
                panels::SIDEBAR => shell::sidebar::view(shell_state, &scx, vcx),
                panels::THREAD => Self::thread_column(timeline_state, composer_state, &scx, vcx),
                _ => dock::panel_view(panel_states, id, &scx, vcx),
            }
        };
        let mut dock_el = Dock::new(&self.dock.layout, dock_size, |e| {
            Msg::Dock(dock::Action::Dock(e)).into()
        });
        if !main {
            dock_el = dock_el.host(host);
        }
        let dock_el = dock_el.build(theme, |id| panel_title(id).to_owned(), &mut content);
        // Narrow layouts: the sidebar as a dismissible overlay.
        let sidebar_overlay = overlay_open.then(|| {
            let w = tokens::SIDEBAR_WIDTH.min(size.0);
            let h = dock_size.1;
            let panel = content(panels::SIDEBAR, (w, h));
            view! {
                <div class="absolute" left={0.0} top={top} w={size.0} h={h} z_index={30}>
                    <div class="absolute" left={0.0} top={0.0} w={size.0} h={h}
                         bg={theme.colors.overlay_scrim}
                         on:click={shell::Action::DismissOverlay} />
                    <div class="absolute" left={0.0} top={0.0} w={w} h={h}
                         border_r={theme.colors.border} aria-label="Sidebar">
                        {panel}
                    </div>
                </div>
            }
            .into_any()
        });
        let overlays = overlays::view(&mut self.overlays, &base, vcx);
        let settings = settings::view(&mut self.settings, &base, vcx);
        let notice = self.dock.windows.notice(window).map(str::to_owned);
        let element = view! {
            <div class="relative flex-col" w={size.0} h={size.1} bg={theme.colors.background}>
                {?titlebar}
                <div w={dock_size.0} h={dock_size.1} class="shrink-0">{dock_el}</div>
                {?sidebar_overlay}
                if let Some(note) = notice {
                    <div class="absolute px-3 py-2" left={0.0} top={size.1 - 36.0} w={size.0}
                         h={36.0} z_index={20} bg={theme.colors.elevated_surface}
                         role="alert" aria-label={note.clone()}>
                        <text size={13.0} color={theme.colors.text_strong}>{note}</text>
                    </div>
                }
                {?overlays}
                {?settings}
            </div>
        }
        .into_any();
        if main {
            self.marks.first_frame(self.runner_ms);
        }
        element
    }

    fn update(&mut self, msg: Msg, cx: &mut UiContext) {
        self.runner_ms = cx.window.elapsed().as_millis() as u64;
        if let Msg::Dock(action) = msg {
            dock::update(&mut self.dock, action, &self.model, &mut self.fx, cx);
            self.apply_effects(cx);
            return;
        }
        if let Msg::Command(id) = msg {
            self.run_command(id, cx);
            self.apply_effects(cx);
            return;
        }
        {
            let scx = surface_cx!(self, cx.theme(), cx);
            let fx = &mut self.fx;
            match msg {
                Msg::Shell(a) => shell::update(&mut self.shell, a, &scx, fx),
                Msg::Timeline(a) => timeline::update(&mut self.timeline, a, &scx, fx),
                Msg::Composer(a) => composer::update(&mut self.composer, a, &scx, fx),
                Msg::Overlays(a) => overlays::update(&mut self.overlays, a, &scx, fx),
                Msg::Settings(a) => settings::update(&mut self.settings, a, &scx, fx),
                Msg::Command(_) | Msg::Dock(_) => unreachable!("handled above"),
            }
        }
        self.apply_effects(cx);
    }

    fn message(&mut self, message: Message, cx: &mut UiContext) {
        self.runner_ms = cx.window.elapsed().as_millis() as u64;
        match message {
            Message::Flush => {
                for text in self.announcements.drain(..) {
                    cx.announce(text, Politeness::Polite);
                }
            }
        }
    }

    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        self.runner_ms = cx.window.elapsed().as_millis() as u64;
        if dock::input(&mut self.dock, event, cx) {
            return true;
        }
        if !matches!(event, InputEvent::KeyPress(_)) {
            return false;
        }
        let handled = {
            let scx = surface_cx!(self, cx.theme(), cx);
            let fx = &mut self.fx;
            overlays::event(&mut self.overlays, event, &scx, fx)
                || settings::event(&mut self.settings, event, &scx, fx)
                || shell::event(&mut self.shell, event, &scx, fx)
                || composer::event(&mut self.composer, event, &scx, fx)
                || timeline::event(&mut self.timeline, event, &scx, fx)
        };
        if handled {
            self.apply_effects(cx);
        }
        handled
    }

    fn edit_text(&mut self, target: FocusId, command: TextEditCommand) -> TextEditOutcome {
        let ecx = EditCx {
            model: &self.model,
            now_ms: self.runner_ms,
        };
        shell::edit_text(&mut self.shell, target, command.clone(), &ecx)
            .or_else(|| composer::edit_text(&mut self.composer, target, command.clone(), &ecx))
            .or_else(|| timeline::edit_text(&mut self.timeline, target, command.clone(), &ecx))
            .or_else(|| overlays::edit_text(&mut self.overlays, target, command.clone(), &ecx))
            .or_else(|| settings::edit_text(&mut self.settings, target, command.clone(), &ecx))
            .or_else(|| dock::edit_text(&mut self.dock, target, command, &ecx))
            .unwrap_or_default()
    }

    fn wake(&mut self, cx: &mut UiContext) {
        dock::wake(&mut self.dock, cx);
    }

    fn app_event(&mut self, event: AppEvent, cx: &mut UiContext) {
        dock::app_event(&mut self.dock, &event, cx);
    }

    fn window_opened(&mut self, window: WindowHandle, cx: &mut UiContext) {
        dock::window_opened(&mut self.dock, window, cx);
    }

    fn window_closed(&mut self, window: WindowHandle, reason: CloseReason, cx: &mut UiContext) {
        dock::window_closed(&mut self.dock, window, reason, cx);
    }

    fn window_close_requested(&mut self, reason: CloseReason, cx: &mut UiContext) -> bool {
        dock::close_requested(&mut self.dock, reason, cx)
    }

    fn drag_session_ended(&mut self, end: DragEnd, cx: &mut UiContext) {
        dock::drag_session_ended(&mut self.dock, &end, cx);
    }
}
