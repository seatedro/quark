//! The pointer over a window: hover, the cursor, tooltips, presses and
//! wheel motion routed through the last frame, and app-drawn title chrome
//! that moves the window.

use std::sync::Arc;
use std::time::Duration;

use quark::Rect;
use quark::hit::HitId;
use quark_ui::element::{CursorHint, TooltipRegion, WheelEvent};
use winit::window::CursorIcon;

use super::focus::Modality;
use super::{Redraw, UiAdapter, UiApp, redraw};
use crate::input::PointerButton;
use crate::{EventContext, WindowHandle};

/// Whether the platform continues trackpad flings with momentum events of
/// its own; elsewhere the adapter adds inertia when the fingers lift.
const PLATFORM_MOMENTUM: bool = cfg!(any(target_os = "macos", target_os = "windows"));

/// The tooltip region (`Div::tooltip`) under the pointer, from the
/// window's last painted frame. A press hides it until the pointer leaves
/// the region.
#[derive(Debug, Clone, PartialEq)]
pub struct HoveredTooltip {
    pub text: Arc<str>,
    /// The element's bounds, in window points.
    pub bounds: Rect,
    /// Where the pointer entered the region.
    pub pointer: (f32, f32),
    /// When the pointer entered the region, on the
    /// [`FrameContext::elapsed`](crate::FrameContext::elapsed) clock.
    pub since: Duration,
}

/// The pointer over a window, and what it last showed there.
#[derive(Debug, Default)]
pub(super) struct PointerState {
    /// Where the pointer is, in window points, while it is over the window.
    pub(super) at: Option<(f32, f32)>,
    /// Hit entries under the pointer in the routed frame, topmost first:
    /// what hover styles were painted from.
    pub(super) hovered: Vec<HitId>,
    /// The cursor last shown over this window.
    pub(super) cursor: CursorIcon,
    pub(super) tooltip: TooltipHover,
    /// The last press on window drag chrome, to tell a double-click.
    pub(super) title_press: TitlePress,
    /// A drag out took the pointer, or the app cancelled the drag, during
    /// the last call into the app.
    pub(super) taken: bool,
}

/// The tooltip region under the pointer.
#[derive(Debug, Default)]
pub(super) struct TooltipHover {
    pub(super) current: Option<HoveredTooltip>,
    /// The region a press hid its tooltip in: hidden until the pointer
    /// leaves it.
    pressed: Option<(Arc<str>, Rect)>,
}

impl TooltipHover {
    /// Follow `pointer` over `regions`, in paint order. True when the
    /// hovered tooltip changed.
    pub(super) fn update(
        &mut self,
        regions: &[TooltipRegion],
        pointer: Option<(f32, f32)>,
        now: Duration,
    ) -> bool {
        let Some((x, y)) = pointer else {
            self.pressed = None;
            return self.current.take().is_some();
        };
        // A child's region comes after its parent's.
        let region = regions.iter().rev().find(|t| t.bounds.contains(x, y));
        let is = |text: &Arc<str>, bounds: Rect| {
            region.is_some_and(|r| r.text == *text && r.bounds == bounds)
        };
        if let Some((text, bounds)) = &self.pressed {
            if is(text, *bounds) {
                return false;
            }
            self.pressed = None;
        }
        if let Some(current) = &self.current
            && is(&current.text, current.bounds)
        {
            return false;
        }
        let next = region.map(|r| HoveredTooltip {
            text: r.text.clone(),
            bounds: r.bounds,
            pointer: (x, y),
            since: now,
        });
        let changed = next.is_some() || self.current.is_some();
        self.current = next;
        changed
    }

    /// Hide the tooltip until the pointer leaves its region. True when one
    /// was showing.
    pub(super) fn press(&mut self) -> bool {
        let Some(current) = self.current.take() else {
            return false;
        };
        self.pressed = Some((current.text, current.bounds));
        true
    }
}

/// Presses on app-drawn title chrome, told apart into drags and the
/// double-clicks that zoom or minimize the window.
#[derive(Debug, Default)]
pub(super) struct TitlePress {
    last: Option<(Duration, (f32, f32))>,
    /// A press held on the chrome whose window move waits for the pointer
    /// to move ([`DEFER_CHROME_DRAG`]).
    held: Option<(f32, f32)>,
}

/// Whether a press on title chrome moves the window only once the pointer
/// moves. X11 window managers grab the pointer for a move as soon as asked
/// and, when the release beat the request there, keep moving until the
/// next one, swallowing a double-click's second press; GTK waits for the
/// drag threshold on Wayland and X11 alike. AppKit and Windows want the
/// press itself.
const DEFER_CHROME_DRAG: bool = cfg!(target_os = "linux");

impl TitlePress {
    /// The longest gap between a double-click's presses: the default on
    /// macOS, Windows, and GNOME.
    const INTERVAL: Duration = Duration::from_millis(500);
    /// How far apart a double-click's presses may be, in points.
    const SLOP: f32 = 4.0;

    /// A press at `at` and time `now`: whether it completes a double-click.
    /// The press after a double-click starts over.
    fn press(&mut self, now: Duration, at: (f32, f32)) -> bool {
        let double = self.last.is_some_and(|(then, (x, y))| {
            now.saturating_sub(then) <= Self::INTERVAL
                && (x - at.0).abs() <= Self::SLOP
                && (y - at.1).abs() <= Self::SLOP
        });
        self.last = (!double).then_some((now, at));
        double
    }

    /// A press elsewhere: the next press on the chrome is a first one.
    fn reset(&mut self) {
        self.last = None;
        self.held = None;
    }

    /// Wait for the pointer to move before moving the window.
    fn hold(&mut self, at: (f32, f32)) {
        self.held = Some(at);
    }

    /// The pointer moved to `at`: whether a held press has now moved far
    /// enough to be a window move.
    fn moved(&mut self, at: (f32, f32)) -> bool {
        let far = self
            .held
            .is_some_and(|(x, y)| (x - at.0).abs() > Self::SLOP || (y - at.1).abs() > Self::SLOP);
        if far {
            self.held = None;
        }
        far
    }

    /// The button was released.
    fn release(&mut self) {
        self.held = None;
    }
}

impl<U: UiApp> UiAdapter<U> {
    pub(super) fn pointer_moved(&mut self, x: f32, y: f32, cx: &mut EventContext) {
        self.win.pointer.at = Some((x, y));
        if self.win.pointer.title_press.moved((x, y))
            && let Some(window) = self.win_handle.or(cx.window_handle())
        {
            Self::start_chrome_drag(window, cx);
        }
        let delivery = self.win.router.pointer_move(x, y);
        self.deliver(delivery, cx);
        self.update_hover(cx);
        self.update_tooltip(cx);
    }

    /// Capture survives the pointer leaving: a drag outside the window
    /// keeps its moves and its release on every platform.
    pub(super) fn pointer_left(&mut self, cx: &mut EventContext) {
        self.win.pointer.at = None;
        self.update_hover(cx);
        self.update_tooltip(cx);
    }

    pub(super) fn button_down(&mut self, button: PointerButton, cx: &mut EventContext) {
        match button {
            // macOS reads Ctrl-click as a secondary click where a context
            // menu takes it; elsewhere it stays a modified primary press.
            PointerButton::Primary
                if cfg!(target_os = "macos")
                    && cx.modifiers().control_key()
                    && self.context_menu_click(cx) => {}
            PointerButton::Primary => {
                if self.win.pointer.tooltip.press() {
                    redraw(Redraw::Tooltip, self.win_handle, cx);
                }
                self.pointer_pressed(cx);
            }
            PointerButton::Middle => {
                if let Some((x, y)) = self.win.pointer.at {
                    self.win.router.middle_down(x, y);
                }
            }
            // Windows opens context menus on release, the others on press.
            PointerButton::Secondary if !cfg!(windows) => {
                self.context_menu_click(cx);
            }
            PointerButton::Secondary | PointerButton::Other => {}
        }
    }

    pub(super) fn button_up(&mut self, button: PointerButton, cx: &mut EventContext) {
        match button {
            PointerButton::Primary => {
                self.win.pointer.title_press.release();
                let delivery = self.win.router.pointer_up();
                self.deliver(delivery, cx);
                // A release the app's hook did not finish the session on
                // completes nothing it could name: back out.
                if self.shared.drag.is_some() {
                    self.end_session(cx);
                }
            }
            PointerButton::Middle => {
                if let Some((x, y)) = self.win.pointer.at {
                    let delivery = self.win.router.middle_up(x, y);
                    self.deliver(delivery, cx);
                }
            }
            PointerButton::Secondary if cfg!(windows) => {
                self.context_menu_click(cx);
            }
            PointerButton::Secondary | PointerButton::Other => {}
        }
    }

    pub(super) fn wheel(&mut self, dx: f32, dy: f32, ended: bool, cx: &mut EventContext) {
        let now_ms = cx.elapsed().as_millis() as u64;
        // Shift turns a plain vertical wheel sideways.
        let (dx, dy) = if cx.modifiers().shift_key() && dx == 0.0 {
            (dy, 0.0)
        } else {
            (dx, dy)
        };
        if let Some((x, y)) = self.win.pointer.at {
            let event = WheelEvent { dx, dy, now_ms };
            let delivery = self.win.router.scroll_wheel(x, y, event);
            self.deliver(delivery, cx);
        }
        if ended && !PLATFORM_MOMENTUM && self.win.router.fling(now_ms) {
            redraw(Redraw::Scroll, self.win_handle, cx);
        }
    }

    /// Cancel the pointer capture the app gave up: the platform's drag loop
    /// gets the release of a drag out, and a cancelled drag gets none.
    pub(super) fn end_taken_pointer(&mut self, cx: &mut EventContext) {
        if std::mem::take(&mut self.win.pointer.taken) {
            let delivery = self.win.router.cancel_pointer();
            self.deliver(delivery, cx);
        }
    }

    /// Track the elements under the pointer; hover styles only change when
    /// they do.
    fn update_hover(&mut self, cx: &mut EventContext) {
        let hovered = self.hovered_at(self.win.pointer.at);
        if hovered != self.win.pointer.hovered {
            self.win.pointer.hovered = hovered;
            self.update_cursor(cx);
            redraw(Redraw::Hover, self.win_handle, cx);
        }
    }

    fn update_tooltip(&mut self, cx: &mut EventContext) {
        let win = &mut *self.win;
        if win
            .pointer
            .tooltip
            .update(&win.spare.tooltip_regions, win.pointer.at, cx.elapsed())
        {
            redraw(Redraw::Tooltip, self.win_handle, cx);
        }
    }

    fn hovered_at(&self, pointer: Option<(f32, f32)>) -> Vec<HitId> {
        pointer.map_or_else(Vec::new, |(x, y)| {
            self.win.router.frame().hits.stack_at(x, y)
        })
    }

    fn pointer_pressed(&mut self, cx: &mut EventContext) {
        let Some((x, y)) = self.win.pointer.at else {
            return;
        };
        let win = &mut *self.win;
        let (before, was_visible) = (win.focus.current, win.focus.is_visible());
        let delivery = win.router.pointer_down(x, y, &mut win.focus.current);
        // A press hides the ring of whatever has focus after it, as
        // `:focus-visible`: a clicked button or dragged divider shows none.
        win.focus.modality = Modality::Pointer;
        if win.focus.current != before || (was_visible && win.focus.current.is_some()) {
            redraw(Redraw::Focus, self.win_handle, cx);
        }
        // A press nothing else took, on app-drawn title chrome, moves the
        // window natively; it must start now, while the press is current.
        if delivery.window_drag {
            self.chrome_pressed((x, y), cx);
        } else {
            win.pointer.title_press.reset();
        }
        self.deliver(delivery, cx);
    }

    /// A secondary click at the pointer: the context menu handler under it.
    /// False when no handler took it.
    fn context_menu_click(&mut self, cx: &mut EventContext) -> bool {
        let Some((x, y)) = self.win.pointer.at else {
            return false;
        };
        let delivery = self.win.router.context_menu_at(x, y);
        let taken = delivery.node.is_some();
        self.deliver(delivery, cx);
        taken
    }

    /// A press on window drag chrome: the platform's title double-click
    /// action when it completes one, else a native window move.
    fn chrome_pressed(&mut self, at: (f32, f32), cx: &mut EventContext) {
        let Some(window) = self.win_handle.or(cx.window_handle()) else {
            return;
        };
        if self.win.pointer.title_press.press(cx.elapsed(), at) {
            cx.title_double_click(window);
        } else if DEFER_CHROME_DRAG {
            self.win.pointer.title_press.hold(at);
        } else {
            Self::start_chrome_drag(window, cx);
        }
    }

    fn start_chrome_drag(window: WindowHandle, cx: &mut EventContext) {
        if let Err(error) = cx.start_window_drag(window) {
            tracing::debug!("no window drag from the title chrome: {error}");
        }
    }

    fn update_cursor(&mut self, cx: &mut EventContext) {
        let win = &mut *self.win;
        let hint = (win.pointer.at)
            .map(|(x, y)| win.router.cursor_at(x, y))
            .unwrap_or_default();
        let cursor = cursor_icon(hint);
        win.pointer.cursor = cursor;
        cx.set_cursor(cursor);
    }
}

pub(super) fn cursor_icon(hint: CursorHint) -> CursorIcon {
    match hint {
        CursorHint::Default => CursorIcon::Default,
        CursorHint::Pointer => CursorIcon::Pointer,
        CursorHint::Text => CursorIcon::Text,
        CursorHint::ResizeCol => CursorIcon::ColResize,
        CursorHint::ResizeRow => CursorIcon::RowResize,
        CursorHint::ResizeNs => CursorIcon::NsResize,
        CursorHint::ResizeEw => CursorIcon::EwResize,
        CursorHint::ResizeNesw => CursorIcon::NeswResize,
        CursorHint::ResizeNwse => CursorIcon::NwseResize,
        CursorHint::Move => CursorIcon::Move,
        CursorHint::Grab => CursorIcon::Grab,
        CursorHint::Grabbing => CursorIcon::Grabbing,
        CursorHint::NotAllowed => CursorIcon::NotAllowed,
        CursorHint::Wait => CursorIcon::Wait,
        CursorHint::Progress => CursorIcon::Progress,
        CursorHint::Crosshair => CursorIcon::Crosshair,
        CursorHint::Help => CursorIcon::Help,
    }
}
