//! Frame allocation budgets through the whole `UiAdapter` path: view,
//! layout, paint, and handing the frame to input routing. These guard the
//! zero-allocation steady state: a frame that repeats the last one must not
//! call the allocator beyond a small constant, and a frame that changes a
//! few cached rows allocates in proportion to those rows.

use std::sync::Arc;

use accesskit::Role;
use quark_ui::element::{AnyElement, IntoAnyElement, cached, div, text, text_input};
use quark_ui::style::Styled;
use quark_ui::test_alloc::{self, Counting};
use quark_ui::text_input::TextField;
use quark_ui::{Action, FocusId};

use crate::runner::{App, TestRunner};
use crate::ui::{UiAdapter, UiApp, UiContext, ViewContext};

#[global_allocator]
static ALLOCATOR: Counting = Counting;

#[derive(Debug, Clone, PartialEq)]
enum Msg {
    Greet,
}

impl From<Msg> for Action {
    fn from(msg: Msg) -> Self {
        Action::new(msg)
    }
}

const NAME: FocusId = FocusId::from_key("budget.name");

/// The hello_ui example's form: a dialog with a heading, a text field, a
/// greeting, and two buttons.
struct Form {
    name: TextField,
}

impl Form {
    fn button(id: &'static str, label: &'static str, cx: &ViewContext) -> AnyElement {
        div()
            .accessibility_id(id)
            .accessibility_role(Role::Button)
            .accessibility_label(label)
            .on_click(Msg::Greet)
            .px(16.0)
            .h(36.0)
            .rounded(8.0)
            .bg(cx.theme.colors.accent)
            .hover_bg(cx.theme.colors.accent_strong)
            .child(text(label).semibold())
            .into_any()
    }
}

impl UiApp for Form {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        div()
            .size_full()
            .items_center()
            .justify_center()
            .child(
                div()
                    .accessibility_id("budget.dialog")
                    .accessibility_role(Role::Dialog)
                    .accessibility_label("Hello Quark")
                    .w(320.0)
                    .p(24.0)
                    .gap(16.0)
                    .flex_col()
                    .bg(cx.theme.colors.surface)
                    .child(text("Hello from Quark").text_lg().bold())
                    .child(
                        text_input("Name", "")
                            .field(&self.name)
                            .focus_target(NAME)
                            .w_full()
                            .h(52.0),
                    )
                    .child(text("Type a name, then press Greet."))
                    .child(
                        div()
                            .flex_row()
                            .gap(8.0)
                            .child(Self::button("budget.greet", "Greet", cx))
                            .child(Self::button("budget.clear", "Clear", cx)),
                    ),
            )
            .into_any()
    }

    fn update(&mut self, _msg: Msg, _cx: &mut UiContext) {}
}

/// A 2,000-row transcript-like list; each row is cached under its index
/// and revision.
struct List {
    rows: Vec<(Arc<str>, u64)>,
}

impl List {
    fn new(rows: usize) -> Self {
        Self {
            rows: (0..rows)
                .map(|i| (Arc::from(format!("Message {i} with a few words")), 0))
                .collect(),
        }
    }
}

impl UiApp for List {
    type Action = Msg;
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let surface = cx.theme.colors.surface;
        div()
            .size_full()
            .flex_col()
            .scroll_y(0.0)
            .children(self.rows.iter().enumerate().map(|(i, (body, revision))| {
                let body = body.clone();
                cached(i as u64, *revision, move || {
                    div()
                        .w_full()
                        .flex_col()
                        .p(4.0)
                        .bg(surface)
                        .child(text("Author").size(12.0).semibold())
                        .child(text(&*body).size(14.0))
                })
                .into_any()
            }))
            .into_any()
    }

    fn update(&mut self, _msg: Msg, _cx: &mut UiContext) {}
}

/// Paint `adapter` `frames` times without assistive tech, then return the
/// allocations of one more frame.
fn steady<U: UiApp>(adapter: &mut UiAdapter<U>, runner: &mut TestRunner) -> u64 {
    runner.accessibility_active = false;
    for elapsed in 0..3 {
        let scene = runner.frame(adapter, elapsed);
        adapter.recycle_scene(scene);
    }
    let (scene, allocated) = test_alloc::count(|| runner.frame(adapter, 3));
    adapter.recycle_scene(scene);
    allocated
}

/// Allocations the test runner itself makes per frame (its window entry),
/// measured with an app whose view is a single div.
fn runner_overhead(runner: &mut TestRunner) -> u64 {
    struct Empty;
    impl UiApp for Empty {
        type Action = Msg;
        type Message = ();
        fn view(&mut self, _cx: &mut ViewContext) -> AnyElement {
            div().into_any()
        }
        fn update(&mut self, _msg: Msg, _cx: &mut UiContext) {}
    }
    steady(&mut UiAdapter::new(Empty, "empty"), runner)
}

#[test]
fn a_repeated_form_frame_allocates_only_what_its_builders_own() {
    let mut runner = TestRunner::new();
    let overhead = runner_overhead(&mut runner);
    let mut adapter = UiAdapter::new(
        Form {
            name: TextField::new(""),
        },
        "form",
    );
    let allocated = steady(&mut adapter, &mut runner) - overhead;
    assert!(allocated <= FORM_BUDGET, "{allocated} allocations");
}

#[test]
fn a_repeated_list_frame_allocates_nothing_beyond_the_runner() {
    let mut runner = TestRunner::new();
    let overhead = runner_overhead(&mut runner);
    let mut adapter = UiAdapter::new(List::new(2_000), "list");
    let allocated = steady(&mut adapter, &mut runner) - overhead;
    assert_eq!(allocated, 0, "allocations of a repeated list frame");
}

#[test]
fn a_streaming_frame_allocates_for_the_changed_rows_only() {
    let mut runner = TestRunner::new();
    let mut adapter = UiAdapter::new(List::new(2_000), "list");
    let still = steady(&mut adapter, &mut runner);
    let mut change = |rows: usize| {
        for row in &mut adapter.app.rows[..rows] {
            row.1 += 1;
        }
        let (scene, allocated) = test_alloc::count(|| runner.frame(&mut adapter, 4));
        adapter.recycle_scene(scene);
        allocated - still
    };
    let one = change(1);
    let ten = change(10);
    assert!(one <= ROW_BUDGET, "{one} allocations for one row");
    assert!(ten <= 10 * ROW_BUDGET, "{ten} allocations for ten rows");
}

/// What the form's builders own each frame: its text and label strings,
/// boxed actions and click handlers, and the field's value copy. A cached
/// view does not pay them; this view rebuilds every frame.
const FORM_BUDGET: u64 = 30;
const ROW_BUDGET: u64 = 40;

/// Allocation counts and top call sites of repeated frames. Run with
/// `--ignored --nocapture`.
#[test]
#[ignore = "measurement, prints a report"]
fn report_app_frame_allocations() {
    fn report<U: UiApp>(name: &str, adapter: &mut UiAdapter<U>, accessibility: bool) {
        let mut runner = TestRunner::new();
        runner.accessibility_active = accessibility;
        for elapsed in 0..3 {
            let started = std::time::Instant::now();
            let (scene, n) = test_alloc::count(|| runner.frame(adapter, elapsed));
            eprintln!(
                "{name} frame {elapsed}: {n} allocations, {:?}",
                started.elapsed()
            );
            adapter.recycle_scene(scene);
        }
        let (_, sites) = test_alloc::profile(|| runner.frame(adapter, 3));
        eprintln!("{name} top sites:");
        for (site, n) in sites.iter().take(20) {
            eprintln!("  {n:6}  {site}");
        }
    }
    for accessibility in [false, true] {
        let suffix = if accessibility { "+a11y" } else { "" };
        let form = Form {
            name: TextField::new(""),
        };
        report(
            &format!("form{suffix}"),
            &mut UiAdapter::new(form, "form"),
            accessibility,
        );
        report(
            &format!("list{suffix}"),
            &mut UiAdapter::new(List::new(2_000), "list"),
            accessibility,
        );
    }
}
