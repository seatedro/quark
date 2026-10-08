//! Toasts and tooltips: the overlays that never take focus.
//!
//! Each host window keeps its own toast stack, at most [`MAX_TOASTS`]
//! shown, and its own tooltip, which appears after the pointer has rested
//! on an element with a tooltip for [`TOOLTIP_DELAY_MS`]. Both run on the
//! window's frame clock rather than the scenario clock, which stands still
//! under `--manual-clock`.

use std::collections::VecDeque;
use std::time::Duration;

use quark::Rect;
use quark::view;
use quark_app::ViewContext;
use quark_app::quark_ui::element::*;
use quark_app::quark_ui::style::Styled;
use quark_app::quark_ui::theme::Theme;
use quark_components::{Toast as ToastView, ToastKind as ToastViewKind, ToastQueue};

use crate::contracts::{Toast, ToastKind};
use crate::design::recipes::{self, TextRole};
use crate::design::tokens;
use crate::overlays::{Action, Pick};

/// Toasts shown per host; a fourth pushes the oldest out.
pub const MAX_TOASTS: usize = 3;
/// How long a toast's Undo stays available.
pub const UNDO_MS: u64 = 8_000;
pub use tokens::TOOLTIP_DELAY_MS;
/// Gap between a tooltip and the element it describes.
const TOOLTIP_GAP: f32 = 6.0;
const TOOLTIP_MAX_WIDTH: f32 = 320.0;

#[derive(Default)]
pub struct Toasts {
    queue: ToastQueue,
    /// Pushed from updates, which have no frame clock; shown at the next
    /// frame so their lifetime starts when they appear.
    pending: Vec<Toast>,
    /// Ids of shown toasts not yet dismissed, oldest first.
    shown: VecDeque<u64>,
    /// The frame clock as of the last frame, for button presses.
    now_ms: u64,
}

impl Toasts {
    pub fn push(&mut self, toast: Toast) {
        self.pending.push(toast);
    }

    pub fn dismiss_index(&mut self, index: usize) {
        if let Some(id) = self.queue.toasts().get(index).map(|t| t.id) {
            self.shown.retain(|&s| s != id);
            self.queue.dismiss(id);
        }
    }

    /// Button `index` of toast `id` was pressed: its [`Pick`], if the
    /// toast is still live.
    pub fn activate(&mut self, id: u64, index: usize) -> Option<Pick> {
        let action = self.queue.activate(id, index, self.now_ms)?;
        self.shown.retain(|&s| s != id);
        action.downcast_ref::<Pick>().copied()
    }

    pub fn pointer_moved(&mut self, pointer: Option<(f32, f32)>) {
        self.queue.pointer_moved(pointer, self.now_ms);
    }

    /// The stack for this frame of `host`'s window, or `None` when empty.
    pub fn view(&mut self, now_ms: u64, vcx: &mut ViewContext) -> Option<AnyElement> {
        self.now_ms = now_ms;
        for toast in self.pending.drain(..) {
            let kind = match toast.kind {
                ToastKind::Error => ToastViewKind::Error,
                ToastKind::Info | ToastKind::Success => ToastViewKind::Info,
            };
            let mut view = ToastView::new(kind, toast.text);
            if let Some(command) = toast.undo {
                // Pressed through `on_action` below, which hands it back
                // to `activate`; it never reaches the app as an action.
                let undo = quark_app::quark_ui::Action::new(Pick::Command(command));
                view = view.undo(undo, UNDO_MS);
            }
            let id = self.queue.push(view, now_ms);
            self.shown.push_back(id);
        }
        let queue = &self.queue;
        self.shown.retain(|&id| {
            queue
                .toasts()
                .iter()
                .any(|t| t.id == id && t.remaining_ms(now_ms) != Some(0))
        });
        while self.shown.len() > MAX_TOASTS {
            if let Some(oldest) = self.shown.pop_front() {
                self.queue.dismiss(oldest);
            }
        }
        if let Some(at) = self.queue.tick(vcx.animations(), now_ms) {
            vcx.frame
                .request_frame_in(Duration::from_millis(at.saturating_sub(now_ms)));
        }
        if self.queue.toasts().is_empty() {
            return None;
        }
        let window = vcx.frame.size();
        let scale = vcx.theme.metrics.ui_scale();
        let stack = self.queue.stack(
            vcx.animations(),
            window,
            scale,
            0.0,
            now_ms,
            &[],
            |index| Action::Toast(ToastEvent::Dismiss(index)).into(),
            |id, button| Action::Toast(ToastEvent::Button(id, button)).into(),
        );
        Some(stack.into_any())
    }
}

/// What a toast's elements emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToastEvent {
    Dismiss(usize),
    Button(u64, usize),
}

/// The tooltip of one host window.
#[derive(Default)]
pub struct Tooltip {
    /// The element the pointer rests on, and since when.
    over: Option<(String, Rect, u64)>,
}

impl Tooltip {
    pub fn hide(&mut self) {
        self.over = None;
    }

    /// The tooltip for this frame: the pointer at `pointer` rests on an
    /// element with one, and has for the delay. `blocked` hides it (a modal
    /// covers the window).
    pub fn view(
        &mut self,
        pointer: Option<(f32, f32)>,
        blocked: bool,
        now_ms: u64,
        vcx: &mut ViewContext,
    ) -> Option<AnyElement> {
        let under = pointer
            .filter(|_| !blocked)
            .and_then(|(x, y)| vcx.tooltip_at(x, y));
        let Some((text, bounds)) = under else {
            self.over = None;
            return None;
        };
        let since = match &self.over {
            Some((t, b, since)) if t == text && *b == bounds => *since,
            _ => {
                self.over = Some((text.to_owned(), bounds, now_ms));
                now_ms
            }
        };
        let due = since + TOOLTIP_DELAY_MS;
        if now_ms < due {
            vcx.frame
                .request_frame_in(Duration::from_millis(due - now_ms));
            return None;
        }
        let text = text.to_owned();
        Some(tooltip(&text, bounds, vcx.frame.size(), vcx.theme))
    }
}

/// A tooltip under `bounds`, or above it near the window's bottom edge.
/// Nothing in it takes clicks or focus.
fn tooltip(label: &str, bounds: Rect, window: (f32, f32), theme: &Theme) -> AnyElement {
    let colors = &theme.colors;
    let pt = |points: f32| recipes::pt(theme, points);
    let (size, line) = TextRole::Meta.scaled(theme);
    let (pad_x, pad_y, gap, max_w) = (
        pt(tokens::SPACE_8),
        pt(tokens::SPACE_4),
        pt(TOOLTIP_GAP),
        pt(TOOLTIP_MAX_WIDTH),
    );
    let height = line + 2.0 * pad_y;
    // An estimate for keeping it on screen; the text sets the real width.
    let width = (label.chars().count() as f32 * size * 0.6 + 2.0 * pad_x).min(max_w);
    let left = bounds.x.min(window.0 - width - pad_x).max(pad_x);
    let below = bounds.y + bounds.height + gap;
    let top = if below + height > window.1 {
        bounds.y - gap - height
    } else {
        below
    };
    view! {
        <div class="absolute" left={left} top={top} max_w={max_w} z_index={500}
             px={pad_x} py={pad_y} rounded={pt(tokens::RADIUS_ROW)}
             bg={colors.elevated_surface} border={colors.border}
             accessibility_role={accesskit::Role::Tooltip} aria-label={label.to_owned()}
             test_id="tooltip">
            <text size={size} color={colors.text}>{label.to_owned()}</text>
        </div>
    }
    .into_any()
}
