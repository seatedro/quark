//! Drag and keyboard reordering of a scrolling list, with autoscroll on
//! the harness's fake clock.

use accesskit::Role;
use quark_ui::FocusId;
use quark_ui::accessibility::Politeness;
use quark_ui::element::{AnyElement, IntoAnyElement, ScrollHandle, div};
use quark_ui::style::Styled;
use quark_ui::virtual_list::{
    KEY_MOVE_DOWN, KEY_MOVE_UP, Reorder, ReorderMsg, RowGeometry, UniformRows,
};

use crate::InputEvent;
use crate::testing::{By, UiTestHarness};
use crate::ui::{UiApp, UiContext, ViewContext};

const ROW: f32 = 40.0;
const VIEWPORT: f32 = 200.0;

#[derive(Debug, Clone, PartialEq)]
struct Msg(ReorderMsg);

impl From<Msg> for quark_ui::Action {
    fn from(msg: Msg) -> Self {
        quark_ui::Action::new(msg)
    }
}

/// Ten 40 point rows, `r0`..`r9`, in a 200 point scrolling viewport.
struct List {
    items: Vec<String>,
    reorder: Reorder,
    scroll: ScrollHandle,
}

impl List {
    fn new() -> Self {
        Self {
            items: (0..10).map(|i| format!("r{i}")).collect(),
            reorder: Reorder::new(),
            scroll: ScrollHandle::new(),
        }
    }

    fn rows(&self) -> UniformRows {
        UniformRows {
            count: self.items.len(),
            extent: ROW,
            gap: 0.0,
        }
    }

    fn order(&self) -> String {
        self.items.join(" ")
    }
}

impl UiApp for List {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let now_ms = cx.frame.elapsed().as_millis() as u64;
        let rows = self.rows();
        if let Some(next) = self
            .reorder
            .autoscroll(now_ms, &rows, self.scroll.offset().1, VIEWPORT)
        {
            self.scroll.set_offset(0.0, next);
            cx.frame.request_frame();
        }
        let dragged = self.reorder.dragged();
        let shift = self.reorder.dragged_shift().unwrap_or(0.0);
        let list = div()
            .w(200.0)
            .h(VIEWPORT)
            .flex_col()
            .track_scroll(&self.scroll)
            .overflow_y_scroll()
            .children_from(self.items.iter().enumerate().map(|(i, item)| {
                div()
                    .key(item.as_str())
                    .test_id(item.as_str())
                    .w_full()
                    .h(ROW)
                    .flex_shrink_0()
                    .focus_ring(FocusId::from_key(item))
                    .on_drag(Reorder::drag_start(i, Msg))
                    .on_key(
                        KEY_MOVE_UP,
                        Msg(ReorderMsg::Step {
                            index: i,
                            delta: -1,
                        }),
                    )
                    .on_key(KEY_MOVE_DOWN, Msg(ReorderMsg::Step { index: i, delta: 1 }))
                    .when(dragged == Some(i), |row| {
                        row.translate(0.0, shift).z_index(1)
                    })
            }));
        let list = match self.reorder.drop_indicator(&rows) {
            Some(y) => list.child(
                div()
                    .absolute()
                    .top(y - 1.0)
                    .w_full()
                    .h(2.0)
                    .test_id("drop-indicator"),
            ),
            None => list,
        };
        list.into_any()
    }

    /// Escape mid-drag cancels it, as most apps do.
    fn event(&mut self, event: &InputEvent, cx: &mut UiContext) -> bool {
        let escape = matches!(event, InputEvent::KeyPress(chord)
            if chord.named() == Some(winit::keyboard::NamedKey::Escape));
        if escape && cx.is_dragging() {
            cx.cancel_drag();
            return true;
        }
        false
    }

    fn update(&mut self, Msg(msg): Msg, cx: &mut UiContext) {
        let rows = self.rows();
        if let Some(event) = self.reorder.update(msg, &rows, self.scroll.offset().1) {
            let label = self.items[event.from].clone();
            event.apply(&mut self.items);
            cx.announce(
                event.announcement(&label, self.items.len()),
                Politeness::Assertive,
            );
        }
    }
}

fn harness() -> UiTestHarness<List> {
    let mut ui = UiTestHarness::new(List::new(), (200.0, VIEWPORT), 1.0);
    ui.frame();
    ui
}

fn announcement(ui: &UiTestHarness<List>) -> Option<String> {
    ui.accessibility_update()
        .nodes
        .iter()
        .find(|(_, node)| node.role() == Role::Status)
        .and_then(|(_, node)| node.label().map(str::to_owned))
}

#[test]
fn dragging_a_row_past_two_middles_drops_it_two_places_down() {
    let mut ui = harness();
    let (x, y) = ui.find(By::test_id("r0")).center();

    ui.pointer_down((x, y));
    ui.pointer_move((x, y + 90.0));
    let indicator = ui.find(By::test_id("drop-indicator")).bounds.y;
    ui.pointer_up((x, y + 90.0));

    // The line sits under r2, where r0 lands.
    assert_eq!(indicator, 119.0);
    assert_eq!(ui.app().order(), "r1 r2 r0 r3 r4 r5 r6 r7 r8 r9");
    assert_eq!(
        announcement(&ui).as_deref(),
        Some("Moved r0 to position 3 of 10")
    );
}

// Regression: a cancelled drag fell back to its release and dropped the
// row wherever the pointer was.
#[test]
fn a_cancelled_drag_leaves_the_order_alone() {
    type Cancel = fn(&mut UiTestHarness<List>);
    let cancels: [(&str, Cancel); 2] = [
        ("focus loss", |ui| ui.focus_loss()),
        ("cancel_drag", |ui| ui.key("escape")),
    ];
    for (name, cancel) in cancels {
        let mut ui = harness();
        let (x, y) = ui.find(By::test_id("r0")).center();

        ui.pointer_down((x, y));
        ui.pointer_move((x, y + 90.0));
        let held = ui.try_find(By::test_id("drop-indicator")).is_some();
        cancel(&mut ui);
        ui.frame();
        let shown = ui.try_find(By::test_id("drop-indicator")).is_some();
        ui.pointer_up((x, y + 90.0));

        assert!(held && !shown, "{name}: indicator {held} then {shown}");
        assert_eq!(ui.app().order(), "r0 r1 r2 r3 r4 r5 r6 r7 r8 r9", "{name}");
    }
}

#[test]
fn holding_a_row_at_the_bottom_edge_scrolls_until_the_end() {
    let mut ui = harness();
    let (x, y) = ui.find(By::test_id("r1")).center();

    // r1's bottom passes the 40 point edge zone: full speed, 800 pt/s.
    ui.pointer_down((x, y));
    ui.pointer_move((x, 190.0));
    ui.advance(100);
    let partway = ui.app().scroll.offset().1;
    ui.advance(400);
    let end = ui.app().scroll.offset().1;
    ui.pointer_up((x, 190.0));

    assert!(
        (60.0..=100.0).contains(&partway),
        "scrolled {partway} in 100 ms"
    );
    // 400 points of rows in a 200 point viewport.
    assert_eq!(end, ui.app().rows().total_extent() - VIEWPORT);
    assert!(!ui.frame_requested(), "autoscroll stops at the end");
    assert_eq!(ui.app().order(), "r0 r2 r3 r4 r5 r6 r7 r8 r9 r1");
}

#[test]
fn alt_arrows_move_the_focused_row_and_focus_follows_it() {
    let mut ui = harness();
    for _ in 0..4 {
        ui.key("tab");
    }
    assert_eq!(ui.focus(), Some(FocusId::from_key("r3")));

    ui.key("alt+arrowdown");
    let after_down = ui.app().order();
    ui.key("alt+arrowup");
    ui.key("alt+arrowup");

    assert_eq!(after_down, "r0 r1 r2 r4 r3 r5 r6 r7 r8 r9");
    assert_eq!(ui.app().order(), "r0 r1 r3 r2 r4 r5 r6 r7 r8 r9");
    assert_eq!(ui.focus(), Some(FocusId::from_key("r3")));
    assert_eq!(
        announcement(&ui).as_deref(),
        Some("Moved r3 to position 3 of 10")
    );
}
