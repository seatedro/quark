//! Allocation budgets of the whole workbench through `UiAdapter`
//! (design section 5). Counts are UI-thread allocations from
//! `quark_ui::test_alloc::Counting`; worker threads (background row
//! measurement, image decoding) are not counted.
//!
//! The design's targets are the `*_BUDGET` constants. Until every surface
//! caches its stable parts the app runs above some of them, so those tests
//! assert a measured `*_CEILING` instead: it catches regressions now and
//! steps down toward the budget as surfaces land. The ignored report test
//! prints the counts and the top allocation sites for attribution.

mod common;

use common::*;
use quark_app::testing::UiTestHarness;
use quark_ui::test_alloc::{self, Counting};
use quark_workbench::Workbench;
use quark_workbench::contracts::{Options, ScenarioKind};

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Design target for a repeated full-app frame.
const REPEATED_FRAME_BUDGET: u64 = 64;
/// Measured at 74855fb+: 396 without accessibility, 510 with. The Dock
/// element rebuilds its tab strips, dividers, and key handlers every frame
/// (about 200), and the placeholder timeline and composer build uncached.
const REPEATED_FRAME_CEILING: u64 = 420;
const REPEATED_FRAME_CEILING_ACCESSIBLE: u64 = 540;
/// Design target for one streamed update plus its frame.
const STREAMED_UPDATE_BUDGET: u64 = 512;
/// Measured at 74855fb+: 572, the repeated frame's 396 plus the changed
/// rows' text layouts and the placeholder timeline's strings.
const STREAMED_UPDATE_CEILING: u64 = 620;
/// Design target for a scroll frame: a base plus each row entering.
const SCROLL_BASE_BUDGET: u64 = 128;
const SCROLL_ROW_BUDGET: u64 = 64;
/// Measured at 74855fb+: a sidebar wheel step costs a repeated frame plus
/// about 146 per entering row, two thirds of it shaping the row's two text
/// layouts (quark_text::TextLayout::rebuild and its per-vector reserves).
const SCROLL_ROW_CEILING: u64 = 170;

/// A settled harness: accessibility as asked, nothing focused (no caret
/// blink), warm-up frames drawn.
fn settled(options: Options, accessibility: bool) -> UiTestHarness<Workbench> {
    let mut ui = harness_with(options, WIDE);
    ui.set_accessibility_active(accessibility);
    for _ in 0..4 {
        ui.run_until_idle();
        ui.frame();
    }
    ui
}

fn options(scenario: ScenarioKind) -> Options {
    Options {
        scenario,
        seed: 7,
        ..Options::default()
    }
}

/// Allocations of a frame that repeats the last, the least of three so a
/// one-off (a cache table growing) does not count.
fn repeated_frame(ui: &mut UiTestHarness<Workbench>) -> u64 {
    (0..3)
        .map(|_| {
            test_alloc::count(|| {
                ui.frame();
            })
            .1
        })
        .min()
        .unwrap_or(0)
}

/// The stress scenario with `rows` of history, all of it adopted.
fn stress(rows: usize) -> UiTestHarness<Workbench> {
    let mut ui = settled(
        Options {
            stress_rows: Some(rows),
            ..options(ScenarioKind::Stress)
        },
        false,
    );
    while ui.app().model.history_pending() > 0 {
        ui.run_until_idle();
        ui.frame();
    }
    ui.frame();
    ui
}

// Catches a surface that rebuilds or reallocates every frame although
// nothing changed, with and without a screen reader.
#[test]
fn perf_repeated_full_app_frame_stays_under_its_ceiling() {
    let quiet = repeated_frame(&mut settled(options(ScenarioKind::Review), false));
    let accessible = repeated_frame(&mut settled(options(ScenarioKind::Review), true));
    assert!(
        quiet <= REPEATED_FRAME_CEILING,
        "{quiet} allocations (budget {REPEATED_FRAME_BUDGET})"
    );
    assert!(
        accessible <= REPEATED_FRAME_CEILING_ACCESSIBLE,
        "{accessible} allocations with accessibility (budget {REPEATED_FRAME_BUDGET})"
    );
}

// Catches an idle app that keeps drawing: with no run and nothing
// animating, no frame is scheduled.
#[test]
fn perf_idle_window_schedules_no_frame() {
    let ui = settled(options(ScenarioKind::Review), false);
    assert_eq!(ui.next_frame_in(), None);
}

// Catches per-frame work that grows with history: a repeated frame costs
// the same with 5,000 and 50,000 rows behind the same viewport.
#[test]
fn perf_repeated_frame_does_not_scale_with_history() {
    let (mut small, mut large) = (stress(5_000), stress(50_000));
    // Both exist before either is measured: the first harness built in a
    // thread pays one more steady allocation while it is the only one.
    assert_eq!(repeated_frame(&mut small), repeated_frame(&mut large));
}

// Catches a streamed chunk costing more than its ceiling, or playback
// drawing more than one frame per chunk: one text delta applied and
// drawn, mid-answer.
#[test]
fn perf_one_streamed_update_and_frame_within_budget() {
    let mut ui = settled(options(ScenarioKind::Review), true);
    type_in_composer(&mut ui, "Make it layout independent");
    ui.key("enter");
    // Leave the composer so its caret blink draws no extra frames.
    ui.click((540.0, 300.0));
    ui.set_accessibility_active(false);
    // Into the second prose segment, past the tool card's start.
    ui.advance(1_600);
    let frames = ui.frame_count();
    let ((), allocated) = test_alloc::count(|| ui.advance(40));
    assert_eq!(ui.frame_count() - frames, 1, "frames per chunk");
    assert!(
        allocated <= STREAMED_UPDATE_CEILING,
        "{allocated} allocations (budget {STREAMED_UPDATE_BUDGET})"
    );
}

// Catches the sidebar rebuilding rows that only moved: a wheel step over
// the 2,000-thread stress list costs a repeated frame plus a share per
// row entering the window, not per row on screen.
#[test]
fn perf_sidebar_scroll_frame_costs_only_entering_rows() {
    let mut ui = stress(5_000);
    ui.pointer_move((100.0, 400.0));
    ui.wheel(0.0, 96.0);
    let base = repeated_frame(&mut ui);
    // 96 points of 32-point rows.
    let entering = 3;
    let ((), allocated) = test_alloc::count(|| ui.wheel(0.0, 96.0));
    let extra = allocated.saturating_sub(base);
    assert!(
        extra <= SCROLL_ROW_CEILING * entering,
        "{extra} allocations over a repeated frame for {entering} entering rows \
         (budget {SCROLL_BASE_BUDGET} + {SCROLL_ROW_BUDGET} per row)"
    );
}

#[test]
#[ignore = "diagnostic: cargo test -p quark-workbench --test perf -- --ignored --nocapture"]
fn report_workbench_frame_allocations() {
    for accessibility in [false, true] {
        let mut ui = settled(options(ScenarioKind::Review), accessibility);
        let n = repeated_frame(&mut ui);
        eprintln!("repeated frame, accessibility {accessibility}: {n} allocations");
    }
    for rows in [5_000, 50_000] {
        let n = repeated_frame(&mut stress(rows));
        eprintln!("repeated frame, {rows} history rows: {n} allocations");
    }
    let mut ui = settled(options(ScenarioKind::Review), false);
    let ((), sites) = test_alloc::profile(|| {
        ui.frame();
    });
    eprintln!("repeated frame sites:");
    for (site, n) in sites.iter().take(25) {
        eprintln!("{n:6} {site}");
    }
    let mut ui = stress(5_000);
    ui.pointer_move((100.0, 400.0));
    ui.wheel(0.0, 96.0);
    ui.frame();
    let ((), sites) = test_alloc::profile(|| ui.wheel(0.0, 96.0));
    eprintln!("sidebar scroll sites:");
    for (site, n) in sites.iter().take(25) {
        eprintln!("{n:6} {site}");
    }
}
