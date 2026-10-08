//! Drag (and keyboard) reordering for lists and virtual lists.
//!
//! [`Reorder`] is app-owned state. Rows start a drag with
//! [`Reorder::drag_start`] (on a handle or the whole row); the drag sends
//! [`ReorderMsg`]s through the app's action type, and the app hands them
//! back to [`Reorder::update`] with the list's [`RowGeometry`] and scroll
//! offset. While a drag is on, [`Reorder::drop_indicator`] says where to
//! draw the drop line and [`Reorder::dragged_shift`] how far to move the
//! dragged row; near the viewport's edges [`Reorder::autoscroll`] scrolls
//! each frame. A drop yields a [`ReorderEvent`], which [`ReorderEvent::apply`]
//! applies to a `Vec`.
//!
//! Keyboard: bind [`KEY_MOVE_UP`] and [`KEY_MOVE_DOWN`] (Alt+Up, Alt+Down)
//! on each focusable row to [`ReorderMsg::Step`]; it yields the same
//! event. [`ReorderEvent::announcement`] is the text to announce for
//! assistive tech after either kind of move.
//!
//! Positions are in list content coordinates: 0 is the top of the first
//! row, and scrolling does not change them. The drag works from pointer
//! deltas, so neither the app nor the list needs to know where the
//! viewport is on screen.

use std::rc::Rc;

use crate::Action;
use crate::element::{ClickEvent, CursorHint, DragHandler, DragReleaseResult};

use super::RowTable;

/// Row positions of a list, for hit testing the drag.
pub trait RowGeometry {
    fn row_count(&self) -> usize;
    /// Top of row `index`, in content coordinates.
    fn row_top(&self, index: usize) -> f32;
    /// Height of row `index`, including any gap after it.
    fn row_extent(&self, index: usize) -> f32;
    /// The row covering `offset`, clamped to the first and last rows.
    fn row_at(&self, offset: f32) -> usize;
    fn total_extent(&self) -> f32;
}

/// Rows of one height with a gap between them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UniformRows {
    pub count: usize,
    pub extent: f32,
    pub gap: f32,
}

impl RowGeometry for UniformRows {
    fn row_count(&self) -> usize {
        self.count
    }

    fn row_top(&self, index: usize) -> f32 {
        index as f32 * (self.extent + self.gap)
    }

    fn row_extent(&self, _index: usize) -> f32 {
        self.extent + self.gap
    }

    fn row_at(&self, offset: f32) -> usize {
        let stride = self.extent + self.gap;
        if stride <= 0.0 || self.count == 0 {
            return 0;
        }
        ((offset / stride).floor().max(0.0) as usize).min(self.count - 1)
    }

    fn total_extent(&self) -> f32 {
        super::virtual_list_total_extent(self.count, self.extent, self.gap)
    }
}

/// A [`super::VariableList`]'s rows, measured or estimated.
impl RowGeometry for RowTable {
    fn row_count(&self) -> usize {
        self.len()
    }

    fn row_top(&self, index: usize) -> f32 {
        self.offset_of_index(index)
    }

    fn row_extent(&self, index: usize) -> f32 {
        self.offset_of_index(index + 1) - self.offset_of_index(index)
    }

    fn row_at(&self, offset: f32) -> usize {
        let last = self.len().saturating_sub(1);
        RowTable::row_at(self, offset.max(0.0)).unwrap_or(last)
    }

    fn total_extent(&self) -> f32 {
        RowTable::total_extent(self)
    }
}

/// What a drag started by [`Reorder::drag_start`] reports.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ReorderMsg {
    /// The pointer went down on row `index`.
    Press {
        index: usize,
    },
    /// The pointer is `dy` points below where it went down.
    Move {
        dy: f32,
    },
    Release,
    /// Abandon the drag (Escape, focus loss); the list stays as it was.
    Cancel,
    /// Move row `index` by `delta` places, from the keyboard.
    Step {
        index: usize,
        delta: i32,
    },
}

/// Keymap binding that moves the focused row up one place.
pub const KEY_MOVE_UP: &str = "alt+arrowup";
/// Keymap binding that moves the focused row down one place.
pub const KEY_MOVE_DOWN: &str = "alt+arrowdown";

/// A row moved from `from` to `to`: remove it at `from`, then insert it at
/// `to`, as [`Self::apply`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReorderEvent {
    pub from: usize,
    pub to: usize,
}

impl ReorderEvent {
    pub fn apply<T>(self, items: &mut Vec<T>) {
        let item = items.remove(self.from);
        items.insert(self.to, item);
    }

    /// What to announce after the move, such as `"Moved Apples to
    /// position 3 of 5"`.
    pub fn announcement(self, label: &str, len: usize) -> String {
        format!("Moved {label} to position {} of {len}", self.to + 1)
    }
}

/// The drag in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Drag {
    from: usize,
    /// Pointer motion since the press.
    dy: f32,
    /// Scroll offset at the press and now; content under a still pointer
    /// moves by their difference.
    scroll_at_press: f32,
    scroll: f32,
    /// Where the row lands if dropped now.
    to: usize,
    /// Clock of the last autoscroll step.
    last_tick_ms: Option<u64>,
}

/// Drag reorder state for one list. See the [module docs](self).
#[derive(Debug, Clone, PartialEq)]
pub struct Reorder {
    drag: Option<Drag>,
    /// How close to the viewport's top or bottom edge (in points) the
    /// dragged row starts scrolling the list.
    pub edge: f32,
    /// Scroll speed, in points per second, with the row at (or past) the
    /// viewport's edge; it ramps up linearly across `edge`.
    pub max_speed: f32,
}

impl Default for Reorder {
    fn default() -> Self {
        Self {
            drag: None,
            edge: 40.0,
            max_speed: 800.0,
        }
    }
}

impl Reorder {
    pub fn new() -> Self {
        Self::default()
    }

    /// A pointer drag for row `index`, for `Div::on_drag` on the row or on
    /// a drag handle inside it. `wrap` turns each [`ReorderMsg`] into the
    /// app's action.
    pub fn drag_start<A: Into<Action>>(
        index: usize,
        wrap: impl Fn(ReorderMsg) -> A + 'static,
    ) -> impl Fn(ClickEvent) -> Box<dyn DragHandler> + 'static {
        let wrap: Rc<dyn Fn(ReorderMsg) -> Action> = Rc::new(move |msg| wrap(msg).into());
        move |press| {
            Box::new(RowDrag {
                index,
                press,
                wrap: Rc::clone(&wrap),
            })
        }
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// The row being dragged.
    pub fn dragged(&self) -> Option<usize> {
        self.drag.map(|drag| drag.from)
    }

    /// Apply a drag message, with the list's rows and current scroll
    /// offset. Returns the move when a drop changes the order.
    pub fn update(
        &mut self,
        msg: ReorderMsg,
        rows: &impl RowGeometry,
        scroll: f32,
    ) -> Option<ReorderEvent> {
        match msg {
            ReorderMsg::Press { index } if index < rows.row_count() => {
                self.drag = Some(Drag {
                    from: index,
                    dy: 0.0,
                    scroll_at_press: scroll,
                    scroll,
                    to: index,
                    last_tick_ms: None,
                });
                None
            }
            ReorderMsg::Press { .. } => None,
            ReorderMsg::Move { dy } => {
                if let Some(drag) = &mut self.drag {
                    drag.dy = dy;
                    drag.scroll = scroll;
                    drag.to = target(drag, rows);
                }
                None
            }
            ReorderMsg::Release => {
                let drag = self.drag.take()?;
                (drag.to != drag.from).then_some(ReorderEvent {
                    from: drag.from,
                    to: drag.to,
                })
            }
            ReorderMsg::Cancel => {
                self.drag = None;
                None
            }
            ReorderMsg::Step { index, delta } => {
                let last = rows.row_count().checked_sub(1)?;
                let to = index.checked_add_signed(delta as isize)?.min(last);
                (index <= last && to != index).then_some(ReorderEvent { from: index, to })
            }
        }
    }

    /// Where the dragged row's top is now, in content coordinates: its
    /// own top moved with the pointer and the scroll.
    fn dragged_top(drag: &Drag, rows: &impl RowGeometry) -> f32 {
        rows.row_top(drag.from) + drag.dy + (drag.scroll - drag.scroll_at_press)
    }

    /// How far to paint the dragged row from its laid-out place (a
    /// `translate` on the row), so it follows the pointer.
    pub fn dragged_shift(&self) -> Option<f32> {
        self.drag
            .map(|drag| drag.dy + (drag.scroll - drag.scroll_at_press))
    }

    /// Content y of the line to draw where the row would land, or `None`
    /// while it would land where it started.
    pub fn drop_indicator(&self, rows: &impl RowGeometry) -> Option<f32> {
        let drag = self.drag?;
        if drag.to < drag.from {
            Some(rows.row_top(drag.to))
        } else if drag.to > drag.from {
            Some(rows.row_top(drag.to) + rows.row_extent(drag.to))
        } else {
            None
        }
    }

    /// One autoscroll step at `now_ms`, for a viewport `viewport` points
    /// tall scrolled to `scroll`: the new scroll offset, or `None` when the
    /// dragged row is clear of both edges (or the list is already at that
    /// end). Call every frame while dragging and request another frame
    /// while it returns `Some`; the first call after the row reaches an
    /// edge only starts the clock.
    pub fn autoscroll(
        &mut self,
        now_ms: u64,
        rows: &impl RowGeometry,
        scroll: f32,
        viewport: f32,
    ) -> Option<f32> {
        let (edge, max_speed) = (self.edge.max(1.0), self.max_speed);
        let drag = self.drag.as_mut()?;
        drag.scroll = scroll;
        let top = Self::dragged_top(drag, rows) - scroll;
        let bottom = top + rows.row_extent(drag.from);
        let max_scroll = (rows.total_extent() - viewport).max(0.0);
        // Depth into an edge zone, 0..1, signed by direction.
        let pull = if top < edge && scroll > 0.0 {
            -((edge - top) / edge).min(1.0)
        } else if bottom > viewport - edge && scroll < max_scroll {
            ((bottom - (viewport - edge)) / edge).min(1.0)
        } else {
            drag.last_tick_ms = None;
            return None;
        };
        let elapsed_ms = drag
            .last_tick_ms
            .map_or(0, |last| now_ms.saturating_sub(last));
        drag.last_tick_ms = Some(now_ms);
        let next = (scroll + pull * max_speed * elapsed_ms as f32 / 1000.0).clamp(0.0, max_scroll);
        drag.scroll = next;
        drag.to = target(drag, rows);
        Some(next)
    }
}

/// Where the dragged row lands: past every row whose middle its middle
/// has crossed.
fn target(drag: &Drag, rows: &impl RowGeometry) -> usize {
    let center = Reorder::dragged_top(drag, rows) + rows.row_extent(drag.from) / 2.0;
    let over = rows.row_at(center);
    let middle = rows.row_top(over) + rows.row_extent(over) / 2.0;
    match over.cmp(&drag.from) {
        std::cmp::Ordering::Equal => drag.from,
        std::cmp::Ordering::Less if center < middle => over,
        std::cmp::Ordering::Less => over + 1,
        std::cmp::Ordering::Greater if center > middle => over,
        std::cmp::Ordering::Greater => over - 1,
    }
}

struct RowDrag {
    index: usize,
    press: ClickEvent,
    wrap: Rc<dyn Fn(ReorderMsg) -> Action>,
}

impl DragHandler for RowDrag {
    fn on_press(&mut self) -> Vec<Action> {
        vec![(self.wrap)(ReorderMsg::Press { index: self.index })]
    }

    fn on_move(&mut self, _x: f32, y: f32) -> Vec<Action> {
        vec![(self.wrap)(ReorderMsg::Move {
            dy: y - self.press.y,
        })]
    }

    fn on_release(&mut self) -> DragReleaseResult {
        DragReleaseResult {
            actions: vec![(self.wrap)(ReorderMsg::Release)],
        }
    }

    fn on_cancel(&mut self) -> Vec<Action> {
        vec![(self.wrap)(ReorderMsg::Cancel)]
    }

    fn cursor(&self) -> CursorHint {
        CursorHint::Grabbing
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ROWS: UniformRows = UniformRows {
        count: 6,
        extent: 40.0,
        gap: 0.0,
    };

    #[test]
    fn drop_target_follows_the_dragged_rows_middle() {
        // (from, dy, to): row 2's middle is at 100; neighbours' middles
        // are 40 points away.
        let cases = [
            (2, 0.0, 2),
            (2, 19.0, 2),
            (2, 41.0, 3),
            (2, -41.0, 1),
            (2, 90.0, 4),
            (2, 500.0, 5),
            (2, -500.0, 0),
            (0, 41.0, 1),
        ];
        for (from, dy, expected) in cases {
            let mut reorder = Reorder::new();
            reorder.update(ReorderMsg::Press { index: from }, &ROWS, 0.0);
            reorder.update(ReorderMsg::Move { dy }, &ROWS, 0.0);
            let dropped = reorder.update(ReorderMsg::Release, &ROWS, 0.0);
            let to = dropped.map_or(from, |event| event.to);
            assert_eq!(to, expected, "row {from} dragged {dy}");
        }
    }

    // Regression: a cancelled drag fell back to its release, so focus loss
    // or Escape mid-drag dropped the row wherever the pointer was.
    #[test]
    fn cancelling_a_drag_moves_no_row() {
        let mut reorder = Reorder::new();
        let mut drag = Reorder::drag_start(2, crate::Action::new)(ClickEvent { x: 10.0, y: 100.0 });
        let msgs = |actions: Vec<Action>| -> Vec<ReorderMsg> {
            actions
                .iter()
                .map(|a| *a.downcast_ref::<ReorderMsg>().unwrap())
                .collect()
        };
        for msg in msgs(drag.on_press())
            .into_iter()
            .chain(msgs(drag.on_move(10.0, 190.0)))
        {
            reorder.update(msg, &ROWS, 0.0);
        }
        let held = reorder.drop_indicator(&ROWS);

        let moved: Vec<_> = msgs(drag.on_cancel())
            .into_iter()
            .filter_map(|msg| reorder.update(msg, &ROWS, 0.0))
            .collect();

        assert_eq!(held, Some(200.0), "row 2 was over row 4");
        assert_eq!(moved, []);
        assert_eq!(reorder.drop_indicator(&ROWS), None);
    }
}
