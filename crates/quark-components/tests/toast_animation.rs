//! Toast entrance and exit run on the window's animation table.

use quark::reactive::SignalStore;
use quark_components::{
    Toast, ToastKind, ToastStack, animate_toast_in, animate_toast_out, retire_toast,
};
use quark_render::Scene;
use quark_ui::Action;
use quark_ui::animation::AnimationTable;
use quark_ui::element::{ElementContext, IntoAnyElement, render_element};
use quark_ui::theme::Theme;

const ID: u64 = 7;

fn toasts() -> [Toast; 1] {
    [Toast {
        id: ID,
        kind: ToastKind::Info,
        message: "Saved".into(),
        description: None,
        created_at_ms: 0,
        hovered: false,
        progress: None,
        ..Default::default()
    }]
}

/// Ticks `table` to `now_ms`, paints the stack at 800x600, and returns the
/// toast's top edge.
fn toast_top(table: &mut AnimationTable, now_ms: u64) -> f32 {
    table.tick(now_ms);
    let toasts = toasts();
    let stack = ToastStack::new(&toasts, table, 800.0, 600.0, 1.0, 0.0, now_ms, &[], |_| {
        Action::new(())
    });
    let mut root = stack.build().into_any();
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut layouts = quark_text::LayoutCache::default();
    let store = SignalStore::new();
    let theme = Theme::default_dark();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &store);
    render_element(&mut root, &mut Scene::default(), &mut cx, 800.0, 600.0);
    // The toast body is the first clickable node; its close button follows.
    cx.semantic
        .nodes()
        .iter()
        .find(|node| node.actions.click)
        .map(|node| node.bounds.y)
        .expect("toast painted")
}

// Catches an entrance that never moves, never lands, or leaves rows behind.
#[test]
fn toast_entrance_slides_up_to_rest_and_retires_its_rows() {
    let mut table = AnimationTable::new();
    let rest = toast_top(&mut table, 0);
    animate_toast_in(&mut table, ID, 0);
    let start = toast_top(&mut table, 0);
    let mid = toast_top(&mut table, 100);
    assert!(start > mid && mid > rest, "{start} > {mid} > {rest}");
    assert_eq!(toast_top(&mut table, 300), rest);
    assert!(!table.has_active());

    assert!(
        !retire_toast(&mut table, ID),
        "an entered toast is not exited"
    );
    assert!(table.is_empty());
    assert_eq!(toast_top(&mut table, 400), rest);
}

// Catches an exit that snaps away or never reports completion, which
// would leave the toast in the app's list forever.
#[test]
fn toast_exit_slides_down_then_reports_done_and_retires_its_rows() {
    let mut table = AnimationTable::new();
    let rest = toast_top(&mut table, 0);
    animate_toast_out(&mut table, ID, 1_000);
    let mid = toast_top(&mut table, 1_050);
    assert!(mid > rest, "{mid} > {rest}");
    assert!(!retire_toast(&mut table, ID), "still exiting");
    assert!(!table.is_empty());

    let gone = toast_top(&mut table, 1_200);
    assert!(gone > mid, "{gone} > {mid}");
    assert!(retire_toast(&mut table, ID));
    assert!(table.is_empty());
}
