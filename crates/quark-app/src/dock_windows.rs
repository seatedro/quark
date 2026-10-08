//! Docking across windows: a [`DockState`] whose floating hosts each show
//! in a window of their own, with tabs and groups that drag between
//! windows, tear off into new ones, and come back.
//!
//! [`DockWindows`] binds every floating [`HostId`] to a [`WindowHandle`]
//! and carries out what the dock model asks for ([`DockEffects`]): it opens
//! and closes host windows, moves focus to a moved tab, announces moves,
//! retitles windows, and saves the workspace. The app keeps owning the
//! [`DockState`] and passes it to each call, and forwards its [`UiApp`](crate::UiApp)
//! hooks:
//!
//! | [`UiApp`](crate::UiApp) hook | Call |
//! |---|---|
//! | `init` | [`DockWindows::init`] |
//! | `view` | [`DockWindows::host`], then `Dock::new(..).host(host)` |
//! | `update`, for a dock event | [`DockWindows::apply`] |
//! | `event` | [`DockWindows::input`], returning its answer |
//! | `wake` | [`DockWindows::wake`] |
//! | `window_opened`, `window_closed` | [`DockWindows::window_opened`], [`DockWindows::window_closed`] |
//! | `app_event` | [`DockWindows::app_event`] |
//! | `window_close_requested` | [`DockWindows::close_requested`] |
//! | `drag_session_ended` | [`DockWindows::drag_session_ended`] |
//!
//! # Dragging between windows
//!
//! Once a dragged tab or group grip passes the drag threshold the dock
//! emits [`DockEvent::DragOut`]. [`DockWindows::apply`] then starts a
//! native [`DockDrag`] and hands the window's pointer capture off to a drag
//! session ([`UiContext::hand_off_drag`]), which from then on resolves
//! against the drop targets every window's dock publishes
//! ([`Dock::drop_scope`]). Where no native transport starts (no window
//! system support), the drag stays inside its window as before.
//!
//! Where the platform can move windows with the pointer
//! ([`DockDrag::can_live_detach`]: X11, Windows, macOS, and Wayland
//! compositors with `xdg_toplevel_drag_v1`), a payload dragged outside
//! every window of the app tears off at once into a new, unfocused window
//! that follows the pointer. Released over a group it re-docks there;
//! released anywhere else it stays a window; Escape puts it back where it
//! was. Where windows cannot follow the pointer, a release outside the
//! app's windows opens the new window there instead. Wayland compositors
//! without the extension cannot tell such a release from a refused drop,
//! so it moves nothing; "Move to new window" ([`DockEvent::MoveToNewHost`])
//! works everywhere.
//!
//! # Closing and quitting
//!
//! Closing a floating window puts its panels back into the main window
//! ([`DockState::close_host`]), or, when the dock's policies leave some
//! panel nowhere to go, keeps the window open with a [`DockWindows::notice`]
//! saying why. Closing the main window quits the app, keeping floating
//! windows to restore next time.
//!
//! # Persistence
//!
//! [`DockWindows::save_to`] names one checkpoint file holding the
//! [`WorkspaceSnapshot`] and a [`PlacementRecord`] per host, the main
//! window's under [`HostId::MAIN`], written atomically on every settled
//! change and before windows close on quit. [`DockWindows::restore`] reads
//! it back; [`DockWindows::init`] reopens the floating windows where they
//! were, on whatever monitors are connected now.

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use quark_components::{
    Boundary, Dock, DockDestination, DockEffects, DockEvent, DockLocation, DockOutcome, DockRegion,
    DockState, HostId, MovePayload, PaneNode, PanelId, StoredDock, TransferRefusal,
    WorkspaceSnapshot,
};
use quark_ui::accessibility::Politeness;
use quark_ui::element::{
    DragEnd, DragLocation, DragOutcome, DragResult, DropPolicy, DropTargetHit, DropTargetId,
};
use serde::{Deserialize, Serialize};

use crate::platform::dock_drag::{DockDrag, DockDragEvent, DockDragStart, Transport, WindowStack};
use crate::platform::placement::{PlacementRecord, RestoreOptions, restore};
use crate::platform::window_state::{read_checkpoint, write_checkpoint};
use crate::runner::restored_position;
use crate::{
    AppEvent, CloseReason, DesktopPoint, InputEvent, UiContext, WindowHandle, WindowOptions,
};

/// The checkpoint kind of a dock workspace file.
pub const CHECKPOINT_KIND: &str = "quark.dock-workspace";

/// The [`DockCheckpoint`] version this build writes.
pub const CHECKPOINT_VERSION: u32 = 1;

/// Size of a window a payload moves to when its group's size is unknown.
const DEFAULT_SIZE: (f64, f64) = (480.0, 360.0);

/// Smallest size of a new floating window.
const MIN_SIZE: (f64, f64) = (240.0, 160.0);

/// A workspace and where each of its windows was: what
/// [`DockWindows::save_to`] writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DockCheckpoint {
    pub workspace: WorkspaceSnapshot,
    /// One per host with a known placement, the main window's under
    /// [`HostId::MAIN`].
    #[serde(default)]
    pub placements: Vec<HostPlacement>,
}

/// A host's window placement in a [`DockCheckpoint`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostPlacement {
    pub host: HostId,
    pub placement: PlacementRecord,
}

/// Why a host's window is opening, and what to do once it is open.
#[derive(Debug, Clone, Copy)]
enum Opening {
    /// "Move to new window", or a release outside every window: the
    /// payload moves in once the window exists ([`DockState::commit_host`]),
    /// its grab point aligned to `align`'s desktop point.
    Reserved {
        align: Option<((f32, f32), DesktopPoint)>,
    },
    /// A live tear-off, already holding its panels: the window follows the
    /// drag from `hotspot`.
    Live { hotspot: (f32, f32) },
    /// Reopened from a checkpoint.
    Restored,
}

/// A floating host and its window.
struct Bound {
    host: HostId,
    window: WindowHandle,
    /// Until the window opens.
    opening: Option<Opening>,
    title: String,
}

/// A dock drag that left its window's router.
struct Drag {
    transport: DockDrag,
    /// What the drag picked up.
    payload: MovePayload,
    /// The floating host a live tear-off made.
    live: Option<HostId>,
    /// A live tear-off's window would not open: no second try.
    no_live: bool,
    /// The point a window made from the payload holds under the pointer.
    hotspot: (f32, f32),
    /// The size of that window.
    size: (f64, f64),
    /// The pointer's last desktop position, where the platform has one.
    desktop: Option<DesktopPoint>,
    /// What the dock shows as the drop target.
    hover: Option<(MovePayload, DockDestination)>,
}

/// Where a move's panels arrived, for its announcement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Arrival {
    /// A group that was already there.
    Existing,
    /// A window made for them.
    NewWindow,
}

/// Binds a dock's floating hosts to windows; see the [module docs](self).
pub struct DockWindows {
    title: Rc<dyn Fn(PanelId) -> String>,
    app_title: String,
    scope: u64,
    path: Option<PathBuf>,
    stack: Option<Rc<dyn Fn() -> Box<dyn WindowStack>>>,
    main: Option<WindowHandle>,
    bound: Vec<Bound>,
    drag: Option<Drag>,
    /// A drag whose window is closing after the drop: kept until it has
    /// closed, since Wayland detaches a window from a drag only while the
    /// drag lasts.
    ending: Option<(WindowHandle, DockDrag)>,
    placements: HashMap<HostId, PlacementRecord>,
    /// Restored floating hosts, to open once the main window is.
    to_open: Vec<HostId>,
    notices: Vec<(WindowHandle, String)>,
}

impl DockWindows {
    /// Windows for a dock whose panels `title` names, in window titles and
    /// announcements.
    pub fn new(title: impl Fn(PanelId) -> String + 'static) -> Self {
        Self {
            title: Rc::new(title),
            app_title: String::new(),
            scope: Dock::drop_scope(None),
            path: None,
            stack: None,
            main: None,
            bound: Vec::new(),
            drag: None,
            ending: None,
            placements: HashMap::new(),
            to_open: Vec::new(),
            notices: Vec::new(),
        }
    }

    /// Title floating windows "<their tabs> - `title`".
    pub fn app_title(mut self, title: impl Into<String>) -> Self {
        self.app_title = title.into();
        self
    }

    /// Save the workspace and window placements to `path` on every settled
    /// change and on quit; [`Self::restore`] reads it.
    pub fn save_to(mut self, path: impl Into<PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    /// The drop target scope of the app's docks, for one built with a
    /// handle ([`Dock::drop_scope`]). Defaults to a dock without one.
    pub fn drop_scope(mut self, scope: u64) -> Self {
        self.scope = scope;
        self
    }

    /// Locate drags with stacks `stack` makes instead of the platform's,
    /// such as a `ScriptedStack` in tests ([`DockDrag::start_with_stack`]).
    pub fn window_stack(mut self, stack: impl Fn() -> Box<dyn WindowStack> + 'static) -> Self {
        self.stack = Some(Rc::new(stack));
        self
    }

    // ---- Queries ---------------------------------------------------------

    /// The host `window` shows: [`HostId::MAIN`] for the main window, a
    /// floating host for one of its windows, `None` for any other window
    /// (show nothing of the dock there).
    pub fn host(&self, window: WindowHandle) -> Option<HostId> {
        if Some(window) == self.main {
            return Some(HostId::MAIN);
        }
        self.bound
            .iter()
            .find(|b| b.window == window)
            .map(|b| b.host)
    }

    /// The window showing `host`, opening or open.
    pub fn window(&self, host: HostId) -> Option<WindowHandle> {
        if host == HostId::MAIN {
            return self.main;
        }
        self.bound.iter().find(|b| b.host == host).map(|b| b.window)
    }

    /// Why `window` refused to close, until the next change to the dock.
    pub fn notice(&self, window: WindowHandle) -> Option<&str> {
        self.notices
            .iter()
            .find(|(w, _)| *w == window)
            .map(|(_, text)| text.as_str())
    }

    /// Whether a drag between windows is under way.
    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The options to open the main window with: `options` at its saved
    /// size and maximized state. [`Self::init`] moves it to its saved
    /// position.
    pub fn main_window_options(&self, options: WindowOptions) -> WindowOptions {
        match self.placements.get(&HostId::MAIN) {
            Some(record) => WindowOptions {
                size: record.size,
                maximized: record.maximized,
                ..options
            },
            None => options,
        }
    }

    // ---- Persistence -----------------------------------------------------

    /// Restore the workspace [`Self::save_to`] saved, keeping the panels
    /// `keep` accepts. Reads a bare saved dock layout ([`StoredDock`]) too.
    /// Returns false, changing nothing, when there is none or it is
    /// unreadable.
    pub fn restore(&mut self, dock: &mut DockState, keep: impl Fn(PanelId) -> bool) -> bool {
        let Some(path) = &self.path else {
            return false;
        };
        let checkpoint = match read_checkpoint(path, CHECKPOINT_KIND) {
            Ok(Some(checkpoint)) => checkpoint
                .into_current::<DockCheckpoint>(CHECKPOINT_VERSION, |version, _| {
                    Err(format!("no dock checkpoint version {version}"))
                })
                .map_err(|error| tracing::warn!("could not restore the dock: {error}"))
                .ok(),
            Ok(None) => None,
            Err(_) => std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<StoredDock>(&bytes).ok())
                .map(|stored| DockCheckpoint {
                    workspace: stored.into_workspace(),
                    placements: Vec::new(),
                }),
        };
        let Some(checkpoint) = checkpoint else {
            return false;
        };
        self.restore_checkpoint(dock, &checkpoint, keep);
        true
    }

    /// Restore `checkpoint`: the workspace now, its floating windows at
    /// [`Self::init`].
    pub fn restore_checkpoint(
        &mut self,
        dock: &mut DockState,
        checkpoint: &DockCheckpoint,
        keep: impl Fn(PanelId) -> bool,
    ) {
        self.to_open = dock.restore_workspace(&checkpoint.workspace, keep);
        self.placements = checkpoint
            .placements
            .iter()
            .map(|p| (p.host, p.placement.clone()))
            .collect();
    }

    /// The workspace and every window's placement as of now.
    pub fn checkpoint(&mut self, dock: &DockState, cx: &UiContext) -> DockCheckpoint {
        let windows: Vec<(HostId, WindowHandle)> = self
            .main
            .map(|w| (HostId::MAIN, w))
            .into_iter()
            .chain(self.bound.iter().map(|b| (b.host, b.window)))
            .collect();
        for (host, window) in windows {
            if let Some(record) = cx
                .window
                .capture_placement(window, self.placements.get(&host))
            {
                self.placements.insert(host, record);
            }
        }
        let hosts = std::iter::once(HostId::MAIN).chain(dock.hosts());
        DockCheckpoint {
            workspace: dock.workspace_snapshot(),
            placements: hosts
                .filter_map(|host| {
                    let placement = self.placements.get(&host)?.clone();
                    Some(HostPlacement { host, placement })
                })
                .collect(),
        }
    }

    /// Write the checkpoint now, if there is a file to write it to.
    pub fn save(&mut self, dock: &DockState, cx: &UiContext) {
        let Some(path) = self.path.clone() else {
            return;
        };
        let checkpoint = self.checkpoint(dock, cx);
        if let Err(error) =
            write_checkpoint(&path, CHECKPOINT_KIND, CHECKPOINT_VERSION, &checkpoint)
        {
            tracing::warn!("could not save the dock to {}: {error}", path.display());
        }
    }

    // ---- Hooks -----------------------------------------------------------

    /// From [`UiApp::init`](crate::UiApp::init): the context's window is the main window. Moves
    /// it to its saved place and reopens restored floating windows.
    pub fn init(&mut self, dock: &mut DockState, cx: &mut UiContext) {
        self.main = cx.window_handle();
        let monitors = cx.window.monitors();
        if let (Some(main), Some(record)) = (self.main, self.placements.get(&HostId::MAIN))
            && cx.window.capabilities().window_positions
        {
            let restored = restore(record, &monitors, &RestoreOptions::default());
            if let Some(position) = restored_position(&restored, &monitors) {
                cx.window.set_outer_position(main, position);
            }
        }
        let fallback = self
            .main
            .and_then(|main| cx.window.placement(main)?.monitor)
            .and_then(|monitor| monitors.iter().position(|m| *m == monitor));
        for host in std::mem::take(&mut self.to_open) {
            let mut options = WindowOptions {
                size: DEFAULT_SIZE,
                ..WindowOptions::default()
            };
            if let Some(record) = self.placements.get(&host) {
                let restored = restore(
                    record,
                    &monitors,
                    &RestoreOptions {
                        can_position: cx.window.capabilities().window_positions,
                        fallback_monitor: fallback,
                        restore_minimized: false,
                    },
                );
                options.size = restored.size;
                options.position = restored_position(&restored, &monitors);
                options.maximized = restored.maximized;
            }
            self.open(dock, host, Opening::Restored, options, cx);
        }
    }

    /// Apply a dock event from the app's `update`, and carry out what it
    /// asks for: start a drag between windows, open a window for "Move to
    /// new window", close emptied windows, focus and announce a moved tab,
    /// save. Returns the dock's outcome.
    pub fn apply(
        &mut self,
        dock: &mut DockState,
        event: DockEvent,
        cx: &mut UiContext,
    ) -> DockOutcome {
        if let DockEvent::DragOut { payload, at } = event {
            self.start_drag(dock, payload, at, cx);
            return DockOutcome::default();
        }
        let now_ms = cx.window.elapsed().as_millis() as u64;
        let outcome = dock.apply_event(event, now_ms);
        if let Some(refusal) = outcome.refused {
            let text = self.refusal_text(refusal);
            cx.announce(text, Politeness::Polite);
        }
        if let Some(host) = outcome.effects.create_host {
            let size = self.payload_size(dock, event, cx);
            let options = WindowOptions {
                size,
                ..WindowOptions::default()
            };
            self.open(dock, host, Opening::Reserved { align: None }, options, cx);
        }
        let focused = self.settle(dock, &outcome.effects, Arrival::Existing, cx);
        if !focused && let Some(focus) = outcome.focus() {
            cx.set_focus(Some(focus));
        }
        if outcome.settled && !outcome.effects.persist {
            self.save(dock, cx);
        }
        // A newly selected tab renames its window.
        self.retitle(dock, cx);
        cx.window.request_redraw_all();
        outcome
    }

    /// From [`UiApp::event`](crate::UiApp::event), for every window's input: moves, releases,
    /// and Escape drive a drag between windows. Returns true when the
    /// event ended the drag and the app should consume it.
    pub fn input(&mut self, dock: &mut DockState, event: &InputEvent, cx: &mut UiContext) -> bool {
        let Some(drag) = &mut self.drag else {
            return false;
        };
        if let InputEvent::PointerMoved { x, y } = event
            && let Some(window) = cx.window_handle()
            && let Some(at) = cx.window.to_desktop(window, (*x, *y))
        {
            drag.desktop = Some(at);
        }
        let Some(event) = drag.transport.handle_input(cx.window, event) else {
            return false;
        };
        let ended = !matches!(event, DockDragEvent::Moved(_));
        self.on_transport(dock, event, cx);
        ended
    }

    /// From [`UiApp::wake`](crate::UiApp::wake): the compositor's news of a Wayland drag.
    pub fn wake(&mut self, dock: &mut DockState, cx: &mut UiContext) {
        let events = match &mut self.drag {
            Some(drag) => drag.transport.poll(cx.window),
            None => return,
        };
        for event in events {
            self.on_transport(dock, event, cx);
        }
    }

    /// From [`UiApp::window_opened`](crate::UiApp::window_opened): move a reserved payload into its new
    /// window, or have a torn-off one follow the pointer.
    pub fn window_opened(
        &mut self,
        dock: &mut DockState,
        window: WindowHandle,
        cx: &mut UiContext,
    ) {
        let Some(bound) = self.bound.iter_mut().find(|b| b.window == window) else {
            return;
        };
        let host = bound.host;
        let opening = bound.opening.take();
        match opening {
            Some(Opening::Reserved { align }) => match dock.commit_host(host) {
                Ok(effects) => {
                    if let Some((hotspot, at)) = align {
                        cx.window.align_window(window, hotspot, at);
                    }
                    self.settle(dock, &effects, Arrival::NewWindow, cx);
                }
                Err(refusal) => {
                    self.unbind(window);
                    cx.window.close_window(window);
                    let text = self.refusal_text(refusal);
                    self.announce_in(self.main, text, cx);
                }
            },
            Some(Opening::Live { hotspot }) => {
                if let Some(drag) = self.drag.as_mut().filter(|d| d.live == Some(host))
                    && let Err(error) = drag.transport.follow(cx.window, window, hotspot)
                {
                    tracing::debug!("dock: the torn-off window cannot follow the pointer: {error}");
                }
            }
            Some(Opening::Restored) | None => {}
        }
        self.retitle(dock, cx);
        cx.window.request_redraw_all();
    }

    /// From [`UiApp::window_closed`](crate::UiApp::window_closed): forget the window. A window that
    /// could not open gives its panels back; one closed on quit keeps them,
    /// to restore next time.
    pub fn window_closed(
        &mut self,
        dock: &mut DockState,
        window: WindowHandle,
        reason: CloseReason,
        cx: &mut UiContext,
    ) {
        if let Some(event) = self
            .drag
            .as_mut()
            .and_then(|drag| drag.transport.window_closed(window))
        {
            self.on_transport(dock, event, cx);
        }
        if self.ending.as_ref().is_some_and(|(w, _)| *w == window) {
            self.ending = None;
        }
        self.notices.retain(|(w, _)| *w != window);
        if Some(window) == self.main {
            self.main = None;
            return;
        }
        let Some(bound) = self.unbind(window) else {
            return;
        };
        let host = bound.host;
        let effects = match (reason, bound.opening) {
            (CloseReason::Quit, _) => return,
            (CloseReason::OpenFailed, Some(Opening::Reserved { .. })) => {
                dock.abort_host(host);
                self.announce_in(self.main, "Could not open a new window".to_owned(), cx);
                return;
            }
            (CloseReason::OpenFailed, Some(Opening::Live { .. })) => {
                if let Some(drag) = self.drag.as_mut().filter(|d| d.live == Some(host)) {
                    drag.live = None;
                    drag.no_live = true;
                }
                dock.cancel_live_detach(host)
            }
            // Its panels must not be lost: a restored window that would not
            // open, or one closed by something other than the user.
            _ if dock.hosts().contains(&host) => dock.recover_host(host),
            _ => return,
        };
        if let Ok(effects) = effects {
            self.settle(dock, &effects, Arrival::Existing, cx);
        }
        cx.window.request_redraw_all();
    }

    /// From [`UiApp::app_event`](crate::UiApp::app_event): windows that moved or resized update
    /// their saved placement.
    pub fn app_event(&mut self, event: &AppEvent, cx: &mut UiContext) {
        let window = match *event {
            AppEvent::WindowMoved { window, .. }
            | AppEvent::WindowResized { window, .. }
            | AppEvent::WindowScaleChanged { window, .. } => window,
            _ => return,
        };
        let Some(host) = self.host(window) else {
            return;
        };
        // A window following the pointer moves on every motion; its
        // placement is read once it lands.
        if self.drag.as_ref().is_some_and(|d| d.live == Some(host)) {
            return;
        }
        if let Some(record) = cx
            .window
            .capture_placement(window, self.placements.get(&host))
        {
            self.placements.insert(host, record);
        }
    }

    /// From [`UiApp::window_close_requested`](crate::UiApp::window_close_requested). Quitting saves the
    /// workspace, floating windows included. Closing the main window quits.
    /// Closing a floating window re-docks its panels, or refuses with a
    /// [`Self::notice`] when one of them has nowhere to go.
    pub fn close_requested(
        &mut self,
        dock: &mut DockState,
        reason: CloseReason,
        cx: &mut UiContext,
    ) -> bool {
        let Some(window) = cx.window_handle() else {
            return true;
        };
        if reason == CloseReason::Quit {
            self.save(dock, cx);
            return true;
        }
        if Some(window) == self.main {
            // Floating windows close for the quit, kept in the workspace.
            self.save(dock, cx);
            cx.window.exit();
            return true;
        }
        let Some(host) = self.host(window).filter(|h| dock.hosts().contains(h)) else {
            return true;
        };
        if self.drag.as_ref().is_some_and(|d| d.live == Some(host)) {
            return false;
        }
        match dock.close_host(host) {
            Ok(effects) => {
                self.unbind(window);
                self.settle(dock, &effects, Arrival::Existing, cx);
                cx.window.request_redraw_all();
                true
            }
            Err(refusal) => {
                let text = format!("Cannot close this window: {}", self.refusal_text(refusal));
                self.notices.retain(|(w, _)| *w != window);
                self.notices.push((window, text.clone()));
                cx.announce_in(window, text, Politeness::Assertive);
                cx.window.request_redraw_window(window);
                false
            }
        }
    }

    /// From [`UiApp::drag_session_ended`](crate::UiApp::drag_session_ended): the adapter ended the drag
    /// session by itself (its source window closed, say), so the drag is
    /// over, a live tear-off put back.
    pub fn drag_session_ended(&mut self, dock: &mut DockState, _end: &DragEnd, cx: &mut UiContext) {
        if let Some(drag) = self.drag.take() {
            dock.set_drag_hover(None);
            self.finish(dock, drag, DragResult::Cancelled, cx);
        }
    }

    // ---- Drags -----------------------------------------------------------

    /// A dock drag passed its threshold at `at` in the context's window:
    /// start the platform's transport and hand the drag off to a session.
    fn start_drag(
        &mut self,
        dock: &DockState,
        payload: MovePayload,
        at: (f32, f32),
        cx: &mut UiContext,
    ) {
        if self.drag.is_some() || cx.drag_session().is_some() {
            return;
        }
        let Some(source) = cx.window_handle() else {
            return;
        };
        let kind = match payload {
            MovePayload::Panel(_) => "panel",
            MovePayload::Group(_) => "group",
        };
        let start = DockDragStart::new(source, at, kind);
        let transport = match &self.stack {
            Some(stack) => DockDrag::start_with_stack(cx.window, start, stack()),
            None => DockDrag::start(cx.window, start),
        };
        let transport = match transport {
            Ok(transport) => transport,
            Err(error) => {
                tracing::debug!("dock: the drag stays in its window: {error}");
                return;
            }
        };
        if let Err(error) = cx.hand_off_drag() {
            tracing::debug!("dock: the drag was not handed off: {error:?}");
            return;
        }
        let grab = cx.drag_session().map_or((0.0, 0.0), |s| s.hotspot());
        let size = self.group_size(dock, payload, source, cx);
        let hotspot = match payload {
            // The tab is the new window's first.
            MovePayload::Panel(_) => grab,
            // The grip ends the new window's tab strip.
            MovePayload::Group(_) => ((size.0 as f32 - 12.0).max(0.0), grab.1),
        };
        let desktop = cx.window.to_desktop(source, at);
        self.drag = Some(Drag {
            transport,
            payload,
            live: None,
            no_live: false,
            hotspot,
            size,
            desktop,
            hover: None,
        });
    }

    fn on_transport(&mut self, dock: &mut DockState, event: DockDragEvent, cx: &mut UiContext) {
        match event {
            DockDragEvent::Moved(location) => {
                self.tear_off(dock, location, cx);
                let Some(drag) = &mut self.drag else {
                    return;
                };
                let policy = Policy::new(dock, self.scope, drag);
                let outcome = cx.update_drag(location, &policy);
                let hover = match outcome {
                    Some(DragOutcome::Target { hit, .. }) => {
                        Dock::drop_destination(dock, &hit).map(|d| (policy.payload, d))
                    }
                    _ => None,
                };
                if hover != drag.hover {
                    drag.hover = hover;
                    dock.set_drag_hover(hover);
                    cx.window.request_redraw_all();
                }
            }
            DockDragEvent::Released(location) => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                let policy = Policy::new(dock, self.scope, &drag);
                let result = cx
                    .finish_drag(Some(location), &policy)
                    .map_or(DragResult::Cancelled, |end| end.result);
                dock.set_drag_hover(None);
                self.finish(dock, drag, result, cx);
            }
            DockDragEvent::Cancelled(reason) => {
                let Some(drag) = self.drag.take() else {
                    return;
                };
                tracing::debug!("dock: drag cancelled: {reason:?}");
                cx.cancel_drag_session();
                dock.set_drag_hover(None);
                self.finish(dock, drag, DragResult::Cancelled, cx);
            }
        }
    }

    /// The pointer left every window of the app: tear the payload off into
    /// a window that follows it, where the platform can.
    fn tear_off(&mut self, dock: &mut DockState, location: DragLocation, cx: &mut UiContext) {
        let Some(drag) = &mut self.drag else {
            return;
        };
        if drag.live.is_some() || drag.no_live {
            return;
        }
        let outside = match location {
            DragLocation::Outside => true,
            // A Wayland drag that left our windows.
            DragLocation::Unknown => {
                matches!(
                    drag.transport.transport(),
                    Transport::Wayland { live: true }
                )
            }
            DragLocation::Window { .. } => false,
        };
        if !outside
            || !drag.transport.can_live_detach(cx.window)
            || !dock.can_live_detach(drag.payload)
        {
            return;
        }
        let Ok(effects) = dock.begin_live_detach(drag.payload) else {
            return;
        };
        let Some(host) = effects.open_host else {
            return;
        };
        drag.live = Some(host);
        let hotspot = drag.hotspot;
        let position = drag
            .desktop
            .filter(|_| cx.window.capabilities().window_positions)
            .map(|at| outer_at(cx, hotspot, at));
        let options = WindowOptions {
            size: drag.size,
            position,
            active: false,
            ..WindowOptions::default()
        };
        self.open(dock, host, Opening::Live { hotspot }, options, cx);
        cx.window.request_redraw_all();
    }

    /// End `drag` with `result`: commit a drop on a target, keep or open a
    /// window for a release outside, or put a live tear-off back.
    fn finish(&mut self, dock: &mut DockState, drag: Drag, result: DragResult, cx: &mut UiContext) {
        let Drag {
            transport,
            payload,
            live,
            hotspot,
            size,
            desktop,
            ..
        } = drag;
        match (result, live) {
            (DragResult::Dropped(DragOutcome::Target { hit, .. }), Some(host)) => {
                let dropped = Dock::drop_destination(dock, &hit)
                    .ok_or(TransferRefusal::MissingDestination)
                    .and_then(|d| dock.drop_live_detach(host, d));
                match dropped {
                    Ok(effects) => {
                        self.keep_until_closed(host, transport);
                        self.settle(dock, &effects, Arrival::Existing, cx);
                    }
                    Err(_) => self.end_live(dock, host, cx),
                }
            }
            (DragResult::Dropped(DragOutcome::Target { hit, .. }), None) => {
                let moved = Dock::drop_destination(dock, &hit)
                    .ok_or(TransferRefusal::MissingDestination)
                    .and_then(|d| dock.transfer(payload, d));
                if let Ok(effects) = moved {
                    self.settle(dock, &effects, Arrival::Existing, cx);
                }
            }
            (DragResult::Dropped(DragOutcome::Outside), None) => {
                self.detach_on_release(dock, payload, hotspot, size, desktop, cx);
            }
            (DragResult::Dropped(_), Some(host)) => self.end_live(dock, host, cx),
            (DragResult::Dropped(_), None) | (DragResult::Cancelled, None) => {}
            (DragResult::Cancelled, Some(host)) => {
                if let Ok(effects) = dock.cancel_live_detach(host) {
                    self.keep_until_closed(host, transport);
                    self.settle(dock, &effects, Arrival::Existing, cx);
                }
            }
        }
        cx.window.request_redraw_all();
    }

    /// A live tear-off released away from any group stays a window.
    fn end_live(&mut self, dock: &mut DockState, host: HostId, cx: &mut UiContext) {
        if let Ok(effects) = dock.end_live_detach(host) {
            self.settle(dock, &effects, Arrival::NewWindow, cx);
        }
    }

    /// Released outside every window where no window could follow the
    /// pointer: open one there for the payload.
    fn detach_on_release(
        &mut self,
        dock: &mut DockState,
        payload: MovePayload,
        hotspot: (f32, f32),
        size: (f64, f64),
        desktop: Option<DesktopPoint>,
        cx: &mut UiContext,
    ) {
        let Ok(host) = dock.reserve_host(payload) else {
            return;
        };
        let positions = cx.window.capabilities().window_positions;
        let at = desktop.filter(|_| positions);
        let options = WindowOptions {
            size,
            position: at.map(|at| outer_at(cx, hotspot, at)),
            ..WindowOptions::default()
        };
        let align = at.map(|at| (hotspot, at));
        self.open(dock, host, Opening::Reserved { align }, options, cx);
    }

    /// Keep `transport` until `host`'s window has closed.
    fn keep_until_closed(&mut self, host: HostId, transport: DockDrag) {
        if let Some(window) = self.window(host) {
            self.ending = Some((window, transport));
        }
    }

    // ---- Effects ---------------------------------------------------------

    /// Carry out a settled move: close emptied windows, focus the moved
    /// tab, announce, save, and retitle. Returns whether it set focus.
    fn settle(
        &mut self,
        dock: &mut DockState,
        effects: &DockEffects,
        arrival: Arrival,
        cx: &mut UiContext,
    ) -> bool {
        if effects.moved.is_empty() && effects.closed_hosts.is_empty() && !effects.persist {
            return false;
        }
        self.notices.clear();
        for &host in &effects.closed_hosts {
            if let Some(window) = self.window(host).filter(|w| Some(*w) != self.main) {
                self.unbind(window);
                cx.window.close_window(window);
            }
        }
        let mut focused = false;
        if let Some((host, focus)) = effects.focus()
            && let Some(window) = self.window(host)
        {
            cx.set_focus_in(window, Some(focus));
            focused = true;
        }
        if effects.announce
            && let Some(destination) = effects.destination
        {
            let text = self.move_text(dock, &effects.moved, destination, arrival);
            self.announce_in(self.window(destination.host), text, cx);
        }
        if effects.persist {
            self.save(dock, cx);
        }
        self.retitle(dock, cx);
        focused
    }

    /// Open a window for `host`.
    fn open(
        &mut self,
        dock: &DockState,
        host: HostId,
        opening: Opening,
        options: WindowOptions,
        cx: &mut UiContext,
    ) {
        let title = self.window_title(dock, host);
        let options = WindowOptions {
            title: title.clone(),
            size: (
                options.size.0.max(MIN_SIZE.0),
                options.size.1.max(MIN_SIZE.1),
            ),
            ..options
        };
        let window = cx.window.open_window(options);
        cx.set_window_name_in(window, title.clone());
        self.bound.push(Bound {
            host,
            window,
            opening: Some(opening),
            title,
        });
    }

    fn unbind(&mut self, window: WindowHandle) -> Option<Bound> {
        let at = self.bound.iter().position(|b| b.window == window)?;
        Some(self.bound.remove(at))
    }

    /// Title every floating window, and name its tab lists for assistive
    /// tech, by its tabs.
    fn retitle(&mut self, dock: &mut DockState, cx: &mut UiContext) {
        for bound in &self.bound {
            let tabs = self.host_title(dock, bound.host);
            if !tabs.is_empty() {
                let label = format!("{tabs} window");
                if dock.host_label(bound.host) != label {
                    dock.set_host_label(bound.host, label);
                }
            }
        }
        let titles: Vec<(usize, String)> = self
            .bound
            .iter()
            .enumerate()
            .filter(|(_, b)| dock.host_root(b.host).is_some())
            .map(|(i, b)| (i, self.window_title(dock, b.host)))
            .filter(|(i, title)| *title != self.bound[*i].title)
            .collect();
        for (i, title) in titles {
            let window = self.bound[i].window;
            cx.window.set_window_title(window, &title);
            cx.set_window_name_in(window, title.clone());
            self.bound[i].title = title;
        }
    }

    fn announce_in(&self, window: Option<WindowHandle>, text: String, cx: &mut UiContext) {
        match window {
            Some(window) => cx.announce_in(window, text, Politeness::Polite),
            None => cx.announce(text, Politeness::Polite),
        }
    }

    // ---- Names -----------------------------------------------------------

    /// A floating host's name: its groups' active tabs.
    fn host_title(&self, dock: &DockState, host: HostId) -> String {
        let Some(root) = dock.host_root(host) else {
            return String::new();
        };
        root.groups()
            .iter()
            .filter_map(|g| g.active_panel())
            .map(|p| (self.title)(p))
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn window_title(&self, dock: &DockState, host: HostId) -> String {
        let tabs = self.host_title(dock, host);
        match (tabs.is_empty(), self.app_title.is_empty()) {
            (true, _) => self.app_title.clone(),
            (false, true) => tabs,
            (false, false) => format!("{tabs} - {}", self.app_title),
        }
    }

    /// A group's name for menus and announcements: its region's label,
    /// numbered when the region is split, or for a group in a floating
    /// host, its window's tabs and "window".
    pub fn group_name(&self, dock: &DockState, pane: quark_components::PaneId) -> String {
        let Some(host) = dock.host_of(pane) else {
            return String::new();
        };
        let region = DockRegion::ALL
            .into_iter()
            .filter(|_| host == HostId::MAIN)
            .find(|&r| dock.root(r).group(pane).is_some())
            .unwrap_or(DockRegion::Center);
        self.location_name(dock, DockLocation { host, region, pane })
    }

    fn location_name(&self, dock: &DockState, at: DockLocation) -> String {
        if at.host != HostId::MAIN {
            return dock.host_label(at.host).to_owned();
        }
        let groups = dock.root(at.region).groups();
        let label = dock.label(at.region);
        match groups.iter().position(|g| g.id == at.pane) {
            Some(i) if groups.len() > 1 => format!("{label} {}", i + 1),
            _ => label.to_owned(),
        }
    }

    fn move_text(
        &self,
        dock: &DockState,
        moved: &[PanelId],
        destination: DockLocation,
        arrival: Arrival,
    ) -> String {
        let panels = moved
            .iter()
            .map(|&p| (self.title)(p))
            .collect::<Vec<_>>()
            .join(", ");
        let to = match arrival {
            Arrival::NewWindow => "a new window".to_owned(),
            Arrival::Existing => self.location_name(dock, destination),
        };
        format!("Moved {panels} to {to}")
    }

    fn refusal_text(&self, refusal: TransferRefusal) -> String {
        match refusal {
            TransferRefusal::Policy { panel, boundary } => {
                let name = (self.title)(panel);
                match boundary {
                    Boundary::Confined => format!("{name} stays where it is"),
                    Boundary::CannotLeave => format!("{name} cannot leave its area"),
                    Boundary::NotAccepted => format!("no group takes {name}"),
                }
            }
            TransferRefusal::NoMove => "nothing to move".to_owned(),
            _ => "the layout changed".to_owned(),
        }
    }

    /// The size of the group `payload` leaves, from the drop targets
    /// `source` last painted.
    fn group_size(
        &self,
        dock: &DockState,
        payload: MovePayload,
        source: WindowHandle,
        cx: &UiContext,
    ) -> (f64, f64) {
        let pane = match payload {
            MovePayload::Panel(panel) => dock.location(panel).map(|(at, _)| at.pane),
            MovePayload::Group(pane) => Some(pane),
        };
        let size = pane
            .zip(cx.drop_targets_in(source))
            .and_then(|(pane, targets)| {
                let key = u64::from(pane.0) << 1;
                let body = targets.geometry(DropTargetId {
                    scope: self.scope,
                    key,
                })?;
                let strip = targets
                    .geometry(DropTargetId {
                        scope: self.scope,
                        key: key | 1,
                    })
                    .map_or(0.0, |strip| strip.bounds.height);
                Some((
                    f64::from(body.bounds.width),
                    f64::from(body.bounds.height + strip),
                ))
            });
        size.unwrap_or(DEFAULT_SIZE)
    }

    /// The size of a window for a "Move to new window" event's payload.
    fn payload_size(&self, dock: &DockState, event: DockEvent, cx: &UiContext) -> (f64, f64) {
        match (event, cx.window_handle()) {
            (DockEvent::MoveToNewHost(payload), Some(source)) => {
                self.group_size(dock, payload, source, cx)
            }
            _ => DEFAULT_SIZE,
        }
    }
}

/// The outer corner that puts a new window's `hotspot` at desktop point
/// `at`, its decorations unknown until it opens.
fn outer_at(cx: &UiContext, hotspot: (f32, f32), at: DesktopPoint) -> DesktopPoint {
    let scale = cx
        .window_handle()
        .and_then(|w| cx.window.placement(w))
        .map_or(1.0, |p| p.desktop_scale);
    (
        at.0 - f64::from(hotspot.0) * scale,
        at.1 - f64::from(hotspot.1) * scale,
    )
}

/// What may drop where during a drag: the dock's checks, against its
/// targets at the dock's current revision.
struct Policy<'a> {
    dock: &'a DockState,
    scope: u64,
    /// What moves: a live tear-off's host's group once it is torn off.
    payload: MovePayload,
}

impl<'a> Policy<'a> {
    fn new(dock: &'a DockState, scope: u64, drag: &Drag) -> Self {
        let payload = match drag.live.and_then(|host| dock.host_root(host)) {
            Some(PaneNode::Tabs(group)) => MovePayload::Group(group.id),
            _ => drag.payload,
        };
        Self {
            dock,
            scope,
            payload,
        }
    }
}

impl DropPolicy for Policy<'_> {
    fn revision(&self, target: DropTargetId) -> Option<u64> {
        let pane = Dock::drop_target_pane(target);
        (target.scope == self.scope && self.dock.group(pane).is_some())
            .then(|| self.dock.layout_revision())
    }

    fn accepts(&self, hit: &DropTargetHit, _payload: &dyn std::any::Any) -> bool {
        Dock::drop_destination(self.dock, hit)
            .is_some_and(|d| self.dock.prepare(self.payload, d).is_ok())
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests;
