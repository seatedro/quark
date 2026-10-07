//! Frame allocation budgets through the whole `UiAdapter` path (view,
//! layout, paint, and handing the frame to input routing), driven by
//! [`UiTestHarness`]. They guard the zero-allocation steady state: a frame
//! that repeats the last one does not call the allocator, and a frame that
//! changes some cached rows allocates in proportion to those rows.

use std::sync::Arc;

use quark_ui::element::{AnyElement, IntoAnyElement, NoopAction, cached, div, text};
use quark_ui::style::Styled;
use quark_ui::test_alloc::{self, Counting};
use quark_ui::transcript::{
    MarkdownEntry, MarkdownTranscript, TextMeasurer, TranscriptRole, TranscriptStyle,
};
use quark_ui::virtual_list::RowKey;

use crate::testing::UiTestHarness;
use crate::ui::{UiApp, UiContext, ViewContext};

#[global_allocator]
static ALLOCATOR: Counting = Counting;

const ROWS: usize = 2_000;

/// Allocations of one changed row: its build closure's elements and
/// strings, its layout subtree, and its recording.
const ROW_BUDGET: u64 = 40;

/// A transcript-like list; each row is cached under its index and
/// revision.
struct List {
    rows: Vec<(Arc<str>, u64)>,
}

impl UiApp for List {
    type Action = ();
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

    fn update(&mut self, _action: (), _cx: &mut UiContext) {}
}

/// The list in a harness with no assistive tech, past its warm-up frames.
fn list() -> UiTestHarness<List> {
    let rows = (0..ROWS)
        .map(|i| (Arc::from(format!("Message {i} with a few words")), 0))
        .collect();
    let mut ui = UiTestHarness::new(List { rows }, (800.0, 600.0), 1.0);
    ui.set_accessibility_active(false);
    for _ in 0..3 {
        ui.frame();
    }
    ui
}

#[test]
fn a_repeated_list_frame_allocates_nothing() {
    let mut ui = list();
    let ((), allocated) = test_alloc::count(|| {
        ui.frame();
    });
    assert_eq!(allocated, 0);
}

#[test]
fn a_repeated_list_frame_with_a_screen_reader_allocates_nothing() {
    let mut ui = list();
    ui.set_accessibility_active(true);
    // The first frame with assistive tech builds every row's nodes.
    ui.frame();
    ui.frame();
    let ((), allocated) = test_alloc::count(|| {
        ui.frame();
    });
    assert_eq!(allocated, 0);
}

#[test]
fn a_streaming_frame_allocates_for_the_changed_rows_only() {
    let mut ui = list();
    let mut change = |rows: usize| {
        for row in &mut ui.app_mut().rows[..rows] {
            row.1 += 1;
        }
        test_alloc::count(|| {
            ui.frame();
        })
        .1
    };
    let one = change(1);
    let ten = change(10);
    assert!(one <= ROW_BUDGET, "{one} allocations for one row");
    assert!(ten <= 10 * ROW_BUDGET, "{ten} allocations for ten rows");
}

/// Allocation counts, times, and top call sites of repeated list frames,
/// with and without assistive tech. Run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement, prints a report"]
fn report_list_frame_allocations() {
    for accessibility in [false, true] {
        let mut ui = list();
        ui.set_accessibility_active(accessibility);
        for frame in 0..3 {
            let started = std::time::Instant::now();
            let ((), n) = test_alloc::count(|| {
                ui.frame();
            });
            eprintln!(
                "list a11y={accessibility} frame {frame}: {n} allocations, {:?}",
                started.elapsed()
            );
        }
        let ((), sites) = test_alloc::profile(|| {
            ui.frame();
        });
        for (site, n) in sites.iter().take(12) {
            eprintln!("  {n:6}  {site}");
        }
    }
}

/// Messages in the transcript budget tests.
const MESSAGES: u64 = 2_000;

/// A markdown transcript filling the window, as a chat view shows one.
struct Chat {
    transcript: MarkdownTranscript,
}

impl UiApp for Chat {
    type Action = ();
    type Message = ();

    fn view(&mut self, cx: &mut ViewContext) -> AnyElement {
        let (width, height) = cx.frame.size();
        let scale = cx.frame.scale_factor();
        let font_size = self.transcript.transcript().style().font_size;
        let text = cx.frame.text();
        let mut measurer = TextMeasurer::new(&mut text.system, &mut text.layouts, font_size, scale);
        self.transcript.prepare(width, height, 0, &mut measurer);
        self.transcript
            .element(cx.theme, |_| NoopAction.into())
            .into_any()
    }

    fn update(&mut self, _action: (), _cx: &mut UiContext) {}
}

fn chat_markdown(i: u64) -> String {
    format!(
        "Message {i} has a **bold** word and `code`.\n\n- a list item\n- another one\n\nA closing paragraph for message {i}."
    )
}

/// The transcript in a harness past its warm-up frames, every row's
/// height measured.
fn chat(accessibility: bool) -> UiTestHarness<Chat> {
    let mut transcript = MarkdownTranscript::new(TranscriptStyle::for_font_size(14.0));
    transcript
        .extend((0..MESSAGES).map(|i| MarkdownEntry {
            row: RowKey(i),
            role: TranscriptRole::Assistant,
            author: "Assistant".into(),
            markdown: chat_markdown(i),
        }))
        .unwrap();
    let mut ui = UiTestHarness::new(Chat { transcript }, (800.0, 600.0), 1.0);
    ui.set_accessibility_active(accessibility);
    ui.frame();
    ui.app_mut().transcript.finish_measures();
    for _ in 0..3 {
        ui.frame();
    }
    ui
}

/// Allocations of a frame that repeats the last one, whatever the row
/// count: the element's row list and event closures, the list's semantic
/// label, the background measurer's font recipe, and the growth of the
/// frame's selectable text list.
const TRANSCRIPT_FRAME_BUDGET: u64 = 16;

#[test]
fn a_repeated_transcript_frame_allocates_a_constant_few() {
    for (accessibility, find) in [(false, None), (true, None), (false, Some("message"))] {
        let mut ui = chat(accessibility);
        if let Some(query) = find {
            ui.app_mut().transcript.set_find_query(query);
            // The rows rebuild with highlights, then settle.
            ui.frame();
            ui.frame();
        }
        let ((), allocated) = test_alloc::count(|| {
            ui.frame();
        });
        assert!(
            allocated <= TRANSCRIPT_FRAME_BUDGET,
            "{allocated} allocations (a11y={accessibility}, find={find:?})"
        );
    }
}

/// Allocations of a frame after text streams into one message: parsing and
/// converting its markdown, shaping the changed block, and rebuilding that
/// row's subtree. The other rows on screen replay.
const STREAMED_ROW_BUDGET: u64 = 320;

#[test]
fn a_streaming_transcript_frame_allocates_for_the_changed_row_only() {
    for accessibility in [false, true] {
        let mut ui = chat(accessibility);
        // Twice the rows on screen must not cost more.
        ui.resize(800.0, 1200.0);
        ui.frame();
        let last = RowKey(MESSAGES - 1);
        let mut markdown = chat_markdown(MESSAGES - 1);
        markdown.push_str(" streamed");
        let ((), allocated) = test_alloc::count(|| {
            ui.app_mut()
                .transcript
                .set_markdown(last, &markdown)
                .unwrap();
            ui.frame();
        });
        assert!(
            allocated <= STREAMED_ROW_BUDGET,
            "{allocated} allocations (a11y={accessibility})"
        );
    }
}

/// Allocation counts and top call sites of repeated transcript frames.
/// Run with `--ignored --nocapture`.
#[test]
#[ignore = "measurement, prints a report"]
fn report_transcript_frame_allocations() {
    for accessibility in [false, true] {
        let mut ui = chat(accessibility);
        let ((), n) = test_alloc::count(|| {
            ui.frame();
        });
        eprintln!("transcript a11y={accessibility}: {n} allocations");

        let ((), sites) = test_alloc::profile(|| {
            ui.frame();
        });
        for (site, n) in sites.iter().take(15) {
            eprintln!("  {n:6}  {site}");
        }
        let last = RowKey(MESSAGES - 1);
        let ((), n) = test_alloc::count(|| {
            let markdown = format!("{} more", chat_markdown(MESSAGES - 1));
            ui.app_mut()
                .transcript
                .set_markdown(last, &markdown)
                .unwrap();
            ui.frame();
        });
        eprintln!("transcript a11y={accessibility} streaming frame: {n} allocations");
    }
}
