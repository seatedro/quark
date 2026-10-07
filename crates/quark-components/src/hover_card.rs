//! Hover cards (rich tooltips) and the anchored placement other overlays
//! share.
//!
//! A hover card opens after the pointer rests on an anchor for
//! [`HOVER_CARD_OPEN_DELAY_MS`] and stays open while the pointer is on the
//! anchor or on the card, so its content can be clicked. Leaving both
//! starts [`HOVER_CARD_CLOSE_DELAY_MS`], which crossing the gap between
//! anchor and card fits inside.
//!
//! The app reports anchors as rects (`(key, rect)` pairs) with each
//! pointer move, calls [`HoverCardState::tick`] on frames, and schedules a
//! frame at [`HoverCardState::next_wake_ms`].

use quark::SemanticRole;
use quark_render::Rect;
use quark_ui::design::Shadow;
use quark_ui::element::{AnyElement, IntoAnyElement, div};
use quark_ui::style::Styled;
use quark_ui::theme::Theme;

/// Which side of its anchor an overlay prefers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

impl Side {
    fn opposite(self) -> Self {
        match self {
            Side::Top => Side::Bottom,
            Side::Bottom => Side::Top,
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// Place a `size` overlay `gap` away from `anchor` on `side`, inside a
/// `viewport` with `margin` kept clear at its edges. When the overlay does
/// not fit on `side` and fits better on the opposite side, it flips; then
/// it is shifted (clamped) to stay inside the viewport. The overlay's
/// start edge lines up with the anchor's on the cross axis.
pub fn place_anchored(
    anchor: Rect,
    size: (f32, f32),
    side: Side,
    gap: f32,
    viewport: (f32, f32),
    margin: f32,
) -> Rect {
    let (w, h) = size;
    let (vw, vh) = viewport;
    // Room available on each side of the anchor.
    let room = |side: Side| match side {
        Side::Top => anchor.y - gap - margin,
        Side::Bottom => vh - margin - (anchor.y + anchor.height + gap),
        Side::Left => anchor.x - gap - margin,
        Side::Right => vw - margin - (anchor.x + anchor.width + gap),
    };
    let need = |side: Side| match side {
        Side::Top | Side::Bottom => h,
        Side::Left | Side::Right => w,
    };
    let side = if room(side) < need(side) && room(side.opposite()) > room(side) {
        side.opposite()
    } else {
        side
    };
    let (x, y) = match side {
        Side::Top => (anchor.x, anchor.y - gap - h),
        Side::Bottom => (anchor.x, anchor.y + anchor.height + gap),
        Side::Left => (anchor.x - gap - w, anchor.y),
        Side::Right => (anchor.x + anchor.width + gap, anchor.y),
    };
    let clamp = |v: f32, len: f32, limit: f32| v.min(limit - margin - len).max(margin);
    Rect {
        x: clamp(x, w, vw),
        y: clamp(y, h, vh),
        width: w,
        height: h,
    }
}

/// Rest time on an anchor before its card opens.
pub const HOVER_CARD_OPEN_DELAY_MS: u64 = 500;
/// Time away from anchor and card before the card closes.
pub const HOVER_CARD_CLOSE_DELAY_MS: u64 = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Hidden,
    Pending { show_at_ms: u64 },
    Shown,
    Closing { hide_at_ms: u64 },
}

/// Where a hover card is in its delay cycle, and for which anchor.
#[derive(Debug, Clone)]
pub struct HoverCardState {
    phase: Phase,
    key: u64,
    anchor: Rect,
    pub size: (f32, f32),
    pub side: Side,
    pub open_delay_ms: u64,
    pub close_delay_ms: u64,
    /// The card's placed rect, refreshed by [`Self::render`].
    card: Option<Rect>,
}

impl HoverCardState {
    /// A card of `size` (points) placed on `side` of its anchor.
    pub fn new(size: (f32, f32), side: Side) -> Self {
        Self {
            phase: Phase::Hidden,
            key: 0,
            anchor: Rect::default(),
            size,
            side,
            open_delay_ms: HOVER_CARD_OPEN_DELAY_MS,
            close_delay_ms: HOVER_CARD_CLOSE_DELAY_MS,
            card: None,
        }
    }

    /// The anchor whose card is showing.
    pub fn shown(&self) -> Option<u64> {
        matches!(self.phase, Phase::Shown | Phase::Closing { .. }).then_some(self.key)
    }

    /// The pointer moved to `pointer` (`None`: it left the window) over
    /// `anchors`. Returns whether to draw a frame: the visible card
    /// changed, or a delay started whose end the next frame schedules.
    pub fn pointer_moved(
        &mut self,
        pointer: Option<(f32, f32)>,
        anchors: &[(u64, Rect)],
        now_ms: u64,
    ) -> bool {
        let before = (self.shown(), self.next_wake_ms());
        let over_card = self.shown().is_some()
            && pointer
                .zip(self.card)
                .is_some_and(|((x, y), r)| r.contains(x, y));
        let over_anchor = pointer.and_then(|(x, y)| {
            anchors
                .iter()
                .find(|(_, rect)| rect.contains(x, y))
                .copied()
        });
        self.phase = match (self.phase, over_anchor) {
            (Phase::Shown | Phase::Closing { .. }, _) if over_card => Phase::Shown,
            (Phase::Shown | Phase::Closing { .. }, Some((key, rect))) => {
                // Moving between anchors while a card shows switches at once.
                self.key = key;
                self.anchor = rect;
                Phase::Shown
            }
            (Phase::Pending { show_at_ms }, Some((key, _))) if key == self.key => {
                Phase::Pending { show_at_ms }
            }
            (_, Some((key, rect))) => {
                self.key = key;
                self.anchor = rect;
                Phase::Pending {
                    show_at_ms: now_ms + self.open_delay_ms,
                }
            }
            (Phase::Shown, None) => Phase::Closing {
                hide_at_ms: now_ms + self.close_delay_ms,
            },
            (phase @ Phase::Closing { .. }, None) => phase,
            (Phase::Hidden | Phase::Pending { .. }, None) => Phase::Hidden,
        };
        (self.shown(), self.next_wake_ms()) != before
    }

    /// Open or close a card whose delay ran out by `now_ms`. Returns
    /// whether the visible card changed.
    pub fn tick(&mut self, now_ms: u64) -> bool {
        match self.phase {
            Phase::Pending { show_at_ms } if now_ms >= show_at_ms => {
                self.phase = Phase::Shown;
                true
            }
            Phase::Closing { hide_at_ms } if now_ms >= hide_at_ms => {
                self.phase = Phase::Hidden;
                self.card = None;
                true
            }
            _ => false,
        }
    }

    /// When the next delay runs out, for scheduling a frame.
    pub fn next_wake_ms(&self) -> Option<u64> {
        match self.phase {
            Phase::Pending { show_at_ms } => Some(show_at_ms),
            Phase::Closing { hide_at_ms } => Some(hide_at_ms),
            Phase::Hidden | Phase::Shown => None,
        }
    }

    /// Close at once, as on Escape or a click elsewhere.
    pub fn hide(&mut self) {
        self.phase = Phase::Hidden;
        self.card = None;
    }

    /// The open card holding `content`, placed beside its anchor inside
    /// `viewport`; `None` while no card shows.
    pub fn render(
        &mut self,
        content: impl IntoAnyElement,
        viewport: (f32, f32),
        theme: &Theme,
    ) -> Option<AnyElement> {
        self.shown()?;
        let m = &theme.metrics;
        let rect = place_anchored(
            self.anchor,
            self.size,
            self.side,
            m.spacing_sm,
            viewport,
            m.spacing_sm,
        );
        self.card = Some(rect);
        let tc = &theme.colors;
        Some(
            div()
                .absolute()
                .left(rect.x)
                .top(rect.y)
                .w(rect.width)
                .h(rect.height)
                .z_index(450)
                .flex_col()
                .p(m.spacing_md)
                .bg(tc.elevated_surface)
                .border(tc.border)
                .rounded(m.panel_radius)
                .shadow_preset(Shadow::POPOVER)
                .overflow_hidden()
                .id("hover-card")
                .test_id("hover-card")
                .semantic_role(SemanticRole::Group)
                .accessibility_role(accesskit::Role::Tooltip)
                .accessibility_id(format!("hover-card:{}", self.key))
                .child(content)
                .into_any(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, width: f32, height: f32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    // Catches an overlay that leaves the viewport, flips when it fits, or
    // fails to flip when only the other side has room.
    #[test]
    fn placement_flips_to_the_roomier_side_then_clamps() {
        let viewport = (400.0, 300.0);
        let cases = [
            // Fits below: placed below, aligned with the anchor's left.
            (rect(50.0, 50.0, 40.0, 20.0), Side::Bottom, (50.0, 74.0)),
            // No room below, room above: flips above.
            (rect(50.0, 250.0, 40.0, 20.0), Side::Bottom, (50.0, 146.0)),
            // No room right: flips left of the anchor.
            (rect(300.0, 50.0, 40.0, 20.0), Side::Right, (196.0, 50.0)),
            // Near the right edge, below: shifted left to stay inside.
            (rect(380.0, 50.0, 10.0, 20.0), Side::Bottom, (292.0, 74.0)),
        ];
        for (anchor, side, expected) in cases {
            let placed = place_anchored(anchor, (100.0, 100.0), side, 4.0, viewport, 8.0);
            assert_eq!((placed.x, placed.y), expected, "{anchor:?} {side:?}");
        }
    }
}
