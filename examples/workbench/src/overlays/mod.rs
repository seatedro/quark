//! Overlays (stream F): the command palette, context menus, tooltips, and
//! toasts, with one overlay stack per host window.
//!
//! Policy, in one place:
//!
//! - Each host keeps a stack of transient layers (palette, menu), topmost
//!   last, each with the focus to restore when it closes. Escape closes
//!   only the top layer, and a submenu before its menu. A drag (handled in
//!   `app.rs`) and an IME composition take Escape first.
//! - The palette is modal in its window: while it is open, commands from
//!   keys or the toolbar wait (see [`command`]), and right-clicks open
//!   nothing.
//! - Palette items, menu items, toast buttons, and key bindings all end in
//!   [`Effect::Command`] (or the thread effects the sidebar uses), so a
//!   command behaves the same whichever way it was chosen.
//! - Tooltips and toasts never take focus.

pub mod feedback;
pub mod menus;
pub mod palette;

use quark_app::quark_ui::FocusId;
use quark_app::quark_ui::element::{AnyElement, IntoAnyElement, div};
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::text_input::{TextEditCommand, TextEditOutcome};
use quark_app::winit::event::{ElementState, MouseButton};
use quark_app::{InputEvent, ViewContext};
use quark_components::{
    ContextMenuOutcome, ContextMenuState, HostId, PALETTE_INPUT, PaletteEvent, PaletteOutcome,
};

use crate::contracts::{
    CommandId, EditCx, Effect, Effects, Msg, SurfaceCx, ThreadId, Toast, ToastKind,
};
use crate::shell::sidebar;
use feedback::{ToastEvent, Toasts, Tooltip};
use menus::MenuTarget;
use palette::Palette;

/// What a palette item, menu item, or toast button chose. Every way of
/// choosing ends in [`run`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Command(CommandId),
    Thread(ThreadId),
    CopyTitle(ThreadId),
    Stop(ThreadId),
}

/// A transient surface on a host's stack.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    Palette,
    Menu(MenuTarget),
}

#[derive(Debug, Clone, Copy)]
struct Entry {
    layer: Layer,
    /// Focus when the layer opened, restored when it closes.
    return_focus: Option<FocusId>,
}

/// One host window's overlays.
struct Host {
    id: HostId,
    stack: Vec<Entry>,
    menu: ContextMenuState,
    toasts: Toasts,
    tooltip: Tooltip,
    pointer: Option<(f32, f32)>,
    /// An IME composition is under way: Escape belongs to it.
    composing: bool,
    /// A right-click at this point, resolved against the next frame's
    /// geometry (events see no layout).
    pending_click: Option<(f32, f32)>,
    /// A menu asked for from the keyboard, anchored at its target's bounds
    /// in the next frame.
    pending_key_menu: Option<(MenuTarget, Option<FocusId>)>,
    /// Space kept clear under the toast stack (the main window's composer).
    toast_inset: f32,
}

impl Host {
    fn new(id: HostId) -> Self {
        Self {
            id,
            stack: Vec::new(),
            menu: ContextMenuState::default(),
            toasts: Toasts::default(),
            tooltip: Tooltip::default(),
            pointer: None,
            composing: false,
            pending_click: None,
            pending_key_menu: None,
            toast_inset: 0.0,
        }
    }

    fn top(&self) -> Option<Layer> {
        self.stack.last().map(|e| e.layer)
    }

    /// Pop the top layer if it is `layer`'s kind; the focus to restore.
    fn pop(&mut self, palette: bool) -> Option<Option<FocusId>> {
        let top = self.stack.last()?;
        if matches!(top.layer, Layer::Palette) != palette {
            return None;
        }
        let entry = self.stack.pop()?;
        if !palette {
            self.menu.close();
        }
        Some(entry.return_focus)
    }

    /// Close an open menu (transient: anything else opening closes it),
    /// without moving focus.
    fn close_menu(&mut self) {
        if matches!(self.top(), Some(Layer::Menu(_))) {
            self.stack.pop();
            self.menu.close();
        }
    }
}

#[derive(Default)]
pub struct State {
    hosts: Vec<Host>,
    /// One palette, open in at most one host (the top of its stack).
    palette: Palette,
}

impl State {
    fn host_mut(&mut self, id: HostId) -> &mut Host {
        match self.hosts.iter().position(|h| h.id == id) {
            Some(i) => &mut self.hosts[i],
            None => {
                self.hosts.push(Host::new(id));
                self.hosts.last_mut().expect("just pushed")
            }
        }
    }

    fn host(&self, id: HostId) -> Option<&Host> {
        self.hosts.iter().find(|h| h.id == id)
    }

    /// The layers open in `host`, bottom first.
    pub fn layers(&self, host: HostId) -> Vec<Layer> {
        self.host(host)
            .map(|h| h.stack.iter().map(|e| e.layer).collect())
            .unwrap_or_default()
    }

    /// Open the menu for `target` at `at` over `host`'s window.
    fn open_menu(
        &mut self,
        host: HostId,
        target: MenuTarget,
        at: (f32, f32),
        return_focus: Option<FocusId>,
        scx: &SurfaceCx,
    ) {
        let h = self.host_mut(host);
        h.close_menu();
        h.tooltip.hide();
        h.menu.open(menus::entries(target, scx), at.0, at.1);
        h.stack.push(Entry {
            layer: Layer::Menu(target),
            return_focus,
        });
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Palette(PaletteEvent),
    /// A context menu item was clicked.
    Menu(Pick),
    Toast(ToastEvent),
    /// Close the top layer of the window (a click outside it).
    Dismiss,
}

pub fn new_state() -> State {
    State::default()
}

/// Keep `inset` points at the bottom of `host`'s window clear of toasts,
/// so the stack sits above the composer instead of over Send.
pub fn set_toast_inset(state: &mut State, host: HostId, inset: f32) {
    state.host_mut(host).toast_inset = inset;
}

/// The overlay layer of `scx.window`, drawn above everything; `None`
/// when nothing is open there.
pub fn view(state: &mut State, scx: &SurfaceCx, vcx: &mut ViewContext) -> Option<AnyElement> {
    let now_ms = vcx.frame.elapsed().as_millis() as u64;
    let window = vcx.frame.size();
    let palette_here = matches!(
        state.host(scx.host).and_then(Host::top),
        Some(Layer::Palette)
    );
    // A modal painted last frame (settings) covers everything under it.
    let modal = vcx.geometry().by_id("overlay.backdrop").is_ok();

    let host = state.host_mut(scx.host);
    if let Some(at) = host.pending_click.take()
        && !modal
        && let Some((target, _)) = menus::target_at(vcx.geometry(), scx, at)
    {
        let focus = scx.focus;
        state.open_menu(scx.host, target, at, focus, scx);
    }
    let host = state.host_mut(scx.host);
    if let Some((target, focus)) = host.pending_key_menu.take()
        && !modal
    {
        let at = menus::target_bounds(vcx.geometry(), target)
            .map_or((window.0 / 2.0, window.1 / 3.0), |r| (r.x, r.y + r.height));
        state.open_menu(scx.host, target, at, focus, scx);
        // From the keyboard, start on the first item.
        let down = "arrowdown".parse().expect("valid binding");
        state.host_mut(scx.host).menu.handle_key(&down);
    }

    let host = state.host_mut(scx.host);
    let blocked = modal || !host.stack.is_empty();
    let tooltip = host.tooltip.view(host.pointer, blocked, now_ms, vcx);
    let toasts = host.toasts.view(now_ms, host.toast_inset, vcx);
    let menu = host.menu.render(window, vcx.theme);
    let palette = if palette_here {
        state
            .palette
            .render(window, vcx.theme, vcx.is_focused(PALETTE_INPUT), |event| {
                Action::Palette(event).into()
            })
    } else {
        None
    };
    if tooltip.is_none() && toasts.is_none() && menu.is_none() && palette.is_none() {
        return None;
    }
    // Window-sized so children anchored to the bottom or right (the toast
    // stack) resolve against the window. A div with no handlers takes no
    // hits, so the layer does not block the surfaces under it.
    let mut layer = div().absolute().left(0.0).top(0.0).w(window.0).h(window.1);
    for child in [toasts, menu, tooltip, palette].into_iter().flatten() {
        layer = layer.child(child);
    }
    Some(layer.into_any())
}

pub fn update(state: &mut State, action: Action, scx: &SurfaceCx, fx: &mut Effects) {
    match action {
        Action::Palette(event) => {
            if let Some(outcome) = state.palette.apply(event) {
                palette_outcome(state, outcome, scx, fx);
            }
        }
        Action::Menu(pick) => {
            if let Some(focus) = state.host_mut(scx.host).pop(false) {
                fx.push(Effect::Focus(focus));
            }
            run(pick, scx, fx);
        }
        Action::Toast(ToastEvent::Dismiss(index)) => {
            state.host_mut(scx.host).toasts.dismiss_index(index);
        }
        Action::Toast(ToastEvent::Button(id, index)) => {
            if let Some(pick) = state.host_mut(scx.host).toasts.activate(id, index) {
                run(pick, scx, fx);
            }
        }
        Action::Dismiss => close_top(state, scx, fx),
    }
}

/// Close the top layer of `scx`'s host and restore the focus it took.
fn close_top(state: &mut State, scx: &SurfaceCx, fx: &mut Effects) {
    let host = state.host_mut(scx.host);
    let focus = match host.top() {
        Some(Layer::Palette) => {
            host.pop(true);
            state.palette.close()
        }
        Some(Layer::Menu(_)) => host.pop(false).flatten(),
        None => return,
    };
    fx.push(Effect::Focus(focus));
}

fn palette_outcome(state: &mut State, outcome: PaletteOutcome, scx: &SurfaceCx, fx: &mut Effects) {
    match outcome {
        PaletteOutcome::Handled => {}
        PaletteOutcome::Run {
            action,
            restore_focus,
        } => {
            state.host_mut(scx.host).pop(true);
            fx.push(Effect::Focus(restore_focus));
            if let Some(pick) = action.downcast_ref::<Pick>() {
                run(*pick, scx, fx);
            }
        }
        PaletteOutcome::Closed { restore_focus } => {
            state.host_mut(scx.host).pop(true);
            fx.push(Effect::Focus(restore_focus));
        }
    }
}

/// Carry out a choice from the palette, a menu, or a toast.
fn run(pick: Pick, scx: &SurfaceCx, fx: &mut Effects) {
    match pick {
        Pick::Command(id) => fx.command(id),
        Pick::Thread(id) => fx.push(Effect::SelectThread(id)),
        Pick::Stop(id) => fx.push(Effect::StopRun(id)),
        Pick::CopyTitle(id) => {
            if let Some(thread) = scx.model.thread(id) {
                fx.push(Effect::CopyText(thread.title.clone()));
                fx.push(Effect::Toast(Toast {
                    kind: ToastKind::Info,
                    text: format!("Copied “{}”", thread.title),
                    undo: None,
                }));
            }
        }
    }
}

/// Sees every event of `scx.window` before the surfaces: keys while a
/// layer is open, the pointer for hover and right-clicks, and IME
/// composition, which owns Escape while it lasts.
pub fn event(state: &mut State, event: &InputEvent, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    let host = state.host_mut(scx.host);
    match event {
        InputEvent::KeyPress(chord) => {
            host.tooltip.hide();
            let Some(pressed) = chord.binding() else {
                return false;
            };
            if pressed.key == "escape" && host.composing {
                return false;
            }
            match host.top() {
                Some(Layer::Palette) => {
                    if let Some(outcome) = state.palette.handle_key(&pressed) {
                        palette_outcome(state, outcome, scx, fx);
                        return true;
                    }
                }
                Some(Layer::Menu(_)) => {
                    if let Some(outcome) = host.menu.handle_key(&pressed) {
                        match outcome {
                            ContextMenuOutcome::Handled => {}
                            ContextMenuOutcome::Closed => {
                                // The menu closed itself; drop its entry.
                                let entry = host.stack.pop();
                                fx.push(Effect::Focus(entry.and_then(|e| e.return_focus)));
                            }
                            ContextMenuOutcome::Activate(action) => {
                                let entry = host.stack.pop();
                                fx.push(Effect::Focus(entry.and_then(|e| e.return_focus)));
                                if let Some(Msg::Overlays(Action::Menu(pick))) =
                                    action.downcast_ref::<Msg>()
                                {
                                    run(*pick, scx, fx);
                                }
                            }
                        }
                        return true;
                    }
                }
                None => {
                    let in_sidebar =
                        matches!(scx.focus, Some(f) if f == sidebar::LIST || f == sidebar::SEARCH);
                    if menus::is_menu_key(&pressed) && in_sidebar {
                        host.pending_key_menu =
                            Some((MenuTarget::Thread(scx.model.selected), scx.focus));
                        return true;
                    }
                }
            }
            false
        }
        InputEvent::PointerMoved { x, y } => {
            host.pointer = Some((*x, *y));
            host.menu.pointer_moved(*x, *y);
            host.toasts.pointer_moved(host.pointer);
            false
        }
        InputEvent::PointerLeft => {
            host.pointer = None;
            host.toasts.pointer_moved(None);
            false
        }
        InputEvent::PointerButton {
            button: MouseButton::Right,
            state: ElementState::Pressed,
        } => {
            if host.top() == Some(Layer::Palette) {
                // Modal: nothing under it opens a menu.
                return true;
            }
            host.pending_click = host.pointer;
            host.pending_click.is_some()
        }
        InputEvent::PointerButton {
            button: MouseButton::Left,
            state: ElementState::Pressed,
        } => {
            // A press outside an open menu closes it and goes on to what
            // it landed on, which takes focus as it would anyway.
            if matches!(host.top(), Some(Layer::Menu(_)))
                && !host.pointer.is_some_and(|(x, y)| host.menu.contains(x, y))
            {
                host.close_menu();
            }
            false
        }
        InputEvent::ImePreedit(text, _) => {
            host.composing = !text.is_empty();
            false
        }
        InputEvent::ImeCommit(_) => {
            host.composing = false;
            false
        }
        InputEvent::Focused(false) => {
            host.composing = false;
            host.tooltip.hide();
            false
        }
        _ => false,
    }
}

pub fn edit_text(
    state: &mut State,
    target: FocusId,
    command: TextEditCommand,
    ecx: &EditCx,
) -> Option<TextEditOutcome> {
    state.palette.edit(target, command, ecx.now_ms)
}

/// An IME composition in the palette's search field.
pub fn set_preedit(
    state: &mut State,
    target: FocusId,
    text: String,
    cursor: Option<(usize, usize)>,
) {
    state.palette.set_preedit(target, text, cursor);
}

/// Commands the overlays own (`OpenPalette`), and the palette's modality:
/// while it is open in `scx.window`, every other command waits. True
/// when handled.
pub fn command(state: &mut State, id: CommandId, scx: &SurfaceCx, fx: &mut Effects) -> bool {
    let host = state.host_mut(scx.host);
    if host.top() == Some(Layer::Palette) {
        // Its own shortcut toggles it closed; the rest are blocked.
        if id == CommandId::OpenPalette {
            close_top(state, scx, fx);
        }
        return true;
    }
    // Any command closes an open menu, as a click elsewhere would.
    host.close_menu();
    if id != CommandId::OpenPalette {
        return false;
    }
    host.tooltip.hide();
    // Open where asked; a palette open in another window closes there.
    for other in &mut state.hosts {
        if other.top() == Some(Layer::Palette) {
            other.stack.pop();
        }
    }
    let focus = state.palette.open(scx);
    state.host_mut(scx.host).stack.push(Entry {
        layer: Layer::Palette,
        return_focus: scx.focus,
    });
    fx.push(Effect::Focus(Some(focus)));
    true
}

/// Whether a modal overlay is open in `scx.window`: background commands
/// wait.
pub fn blocks_background(state: &State, scx: &SurfaceCx) -> bool {
    state.host(scx.host).and_then(Host::top) == Some(Layer::Palette)
}

/// Show `toast` in the host of `scx.window`.
pub fn toast(state: &mut State, toast: Toast, scx: &SurfaceCx) {
    state.host_mut(scx.host).toasts.push(toast);
}
