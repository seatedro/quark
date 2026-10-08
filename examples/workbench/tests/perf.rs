//! Allocation budgets of the whole workbench through `UiAdapter`
//! (design section 5). Counts are UI-thread allocations from
//! `quark_ui::test_alloc::Counting`; worker threads (background row
//! measurement, image decoding) are not counted.
//!
//! The design's targets are the `*_BUDGET` constants. Each test asserts a
//! measured `*_CEILING` next to its budget, so a regression fails even
//! while the count is under budget, and the one case still over budget
//! (cold sidebar rows) is visible. The ignored report test prints every
//! count and the top allocation sites for attribution.

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
/// Measured on wb/integrate after the C merge: 45 without accessibility,
/// 53 with (507 and 643 before the dock, split, toolbar, and sidebar
/// caches). The ceilings leave a few allocations for element pool jitter.
const REPEATED_FRAME_CEILING: u64 = 48;
const REPEATED_FRAME_CEILING_ACCESSIBLE: u64 = 56;
/// Design target for one streamed update plus its frame.
const STREAMED_UPDATE_BUDGET: u64 = 512;
/// Measured: 337, the repeated frame plus about 290 for the chunk
/// (re-shaping the growing block, markdown re-conversion, the row's
/// rebuild); 686 before.
const STREAMED_UPDATE_CEILING: u64 = 360;
/// Design target for a scroll frame: a base plus each row entering.
const SCROLL_BASE_BUDGET: u64 = 128;
const SCROLL_ROW_BUDGET: u64 = 64;
/// Measured: a sidebar wheel step costs a repeated frame plus about 151
/// per entering row, over budget. Each row shapes three text layouts for
/// the first time, and a cold quark_text::TextLayout allocates every glyph
/// column on its own (the layout pool only recycles evicted layouts, which
/// a fresh app has none of).
const SCROLL_ROW_CEILING: u64 = 160;
/// Measured: a warm wheel step up the stress transcript costs 55 with two
/// rows entering (their cached row elements replay).
const TRANSCRIPT_SCROLL_BASE_CEILING: u64 = 56;
const TRANSCRIPT_SCROLL_ROW_CEILING: u64 = 8;

/// A settled harness: accessibility as asked, nothing focused (no caret
/// blink), warm-up frames drawn, the transcript's workers finished.
fn settled(options: Options, accessibility: bool) -> UiTestHarness<Workbench> {
    let mut ui = harness_with(options, WIDE);
    ui.set_accessibility_active(accessibility);
    for _ in 0..4 {
        ui.run_until_idle();
        ui.frame();
    }
    settle_transcript(&mut ui);
    ui
}

/// Finish the transcript's background row measurement, highlighting, and
/// image decoding, which run on worker threads the fake clock does not
/// drive, and draw the frames that take their results (the measured
/// heights settle the scroll anchor one frame later).
fn settle_transcript(ui: &mut UiTestHarness<Workbench>) {
    ui.app_mut().timeline.finish_measures();
    ui.frame();
    ui.advance(32);
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
    settle_transcript(&mut ui);
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
    // thread pays one more steady allocation while it is the only one. The
    // two also share the thread's element pool, so which pooled child list
    // a div gets (and whether it grows once) can differ by an allocation or
    // two between them; work per history row would cost far more.
    let (small, large) = (repeated_frame(&mut small), repeated_frame(&mut large));
    assert!(small.abs_diff(large) <= 2, "{small} vs {large}");
}

/// One text delta applied and drawn mid-answer: (frames drawn, allocations).
fn streamed_update(ui: &mut UiTestHarness<Workbench>) -> (u64, u64) {
    type_in_composer(ui, "Make it layout independent");
    ui.key("enter");
    // Leave the composer so its caret blink draws no extra frames.
    ui.click((540.0, 300.0));
    ui.set_accessibility_active(false);
    // Into the second prose segment, past the tool card's start.
    ui.advance(1_600);
    let frames = ui.frame_count();
    let ((), allocated) = test_alloc::count(|| ui.advance(40));
    (ui.frame_count() - frames, allocated)
}

/// A 96-point wheel step over the sidebar's 2,000 threads: (allocations
/// over a repeated frame, rows entering).
fn sidebar_scroll(ui: &mut UiTestHarness<Workbench>) -> (u64, u64) {
    ui.pointer_move((100.0, 400.0));
    ui.wheel(0.0, 96.0);
    let base = repeated_frame(ui);
    let ((), allocated) = test_alloc::count(|| ui.wheel(0.0, 96.0));
    // 96 points of 32-point rows.
    (allocated.saturating_sub(base), 3)
}

/// Keys of the transcript rows the selected thread has materialized.
fn transcript_rows(ui: &UiTestHarness<Workbench>) -> Vec<quark_ui::virtual_list::RowKey> {
    let wb = ui.app();
    wb.timeline
        .thread_view(wb.model.selected)
        .map(|view| {
            view.document()
                .document()
                .visible_rows()
                .iter()
                .map(|row| row.key)
                .collect()
        })
        .unwrap_or_default()
}

/// A 96-point wheel step up the stress transcript, over rows it has
/// shown before (warm caches, as the design's scroll budget assumes):
/// (allocations, rows entering).
fn transcript_scroll(ui: &mut UiTestHarness<Workbench>) -> (u64, u64) {
    let step = |ui: &mut UiTestHarness<Workbench>, dy: f32| {
        ui.wheel(0.0, dy);
        settle_transcript(ui);
    };
    ui.pointer_move((640.0, 400.0));
    for dy in [-96.0, -96.0, -96.0, 96.0, 96.0, 96.0] {
        step(ui, dy);
    }
    let before = transcript_rows(ui);
    let ((), allocated) = test_alloc::count(|| ui.wheel(0.0, -96.0));
    let entering = transcript_rows(ui)
        .iter()
        .filter(|key| !before.contains(key))
        .count();
    (allocated, entering as u64)
}

// Catches a streamed chunk costing more than its ceiling, or playback
// drawing more than one frame per chunk: one text delta applied and
// drawn, mid-answer.
#[test]
fn perf_one_streamed_update_and_frame_within_budget() {
    let mut ui = settled(options(ScenarioKind::Review), true);
    let (frames, allocated) = streamed_update(&mut ui);
    assert_eq!(frames, 1, "frames per chunk");
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
    let (extra, entering) = sidebar_scroll(&mut stress(5_000));
    assert!(
        extra <= SCROLL_ROW_CEILING * entering,
        "{extra} allocations over a repeated frame for {entering} entering rows \
         (budget {SCROLL_BASE_BUDGET} + {SCROLL_ROW_BUDGET} per row)"
    );
}

// Catches transcript rows rebuilt although they are cached, or a scroll
// that rebuilds the rows on screen: a wheel step back over history costs
// a base plus a share per row entering the viewport.
#[test]
fn perf_transcript_scroll_frame_costs_only_entering_rows() {
    let (allocated, entering) = transcript_scroll(&mut stress(5_000));
    assert!(entering > 0, "the step brings rows into view");
    assert!(
        allocated <= TRANSCRIPT_SCROLL_BASE_CEILING + TRANSCRIPT_SCROLL_ROW_CEILING * entering,
        "{allocated} allocations for {entering} entering rows \
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
    let (frames, n) = streamed_update(&mut settled(options(ScenarioKind::Review), true));
    eprintln!("streamed update: {n} allocations over {frames} frame(s)");
    let (extra, entering) = sidebar_scroll(&mut stress(5_000));
    eprintln!("sidebar scroll: {extra} over a repeated frame, {entering} rows entering");
    let (n, entering) = transcript_scroll(&mut stress(5_000));
    eprintln!("transcript scroll: {n} allocations, {entering} rows entering");

    let mut ui = settled(options(ScenarioKind::Review), false);
    let ((), sites) = test_alloc::profile(|| {
        ui.frame();
    });
    eprintln!("repeated frame sites:");
    for (site, n) in sites.iter().take(25) {
        eprintln!("{n:6} {site}");
    }
}
