//! Row heights measured on a background thread.
//!
//! The UI thread measures only the rows in the overscanned window, so rows
//! farther away hold estimates and scroll positions over them are
//! approximate. [`BackgroundMeasure`] sends snapshots of those rows (their
//! blocks share their spans by `Arc`, so a snapshot is cheap) to a worker
//! that shapes them with its own `TextSystem`, built from the UI system's
//! [`TextSystemRecipe`], and returns exact heights. Rows nearest the window
//! go first.
//!
//! Every request carries a generation and the layout epoch (width, fonts,
//! scale, style) it was measured under. A result counts only when its row
//! still waits for exactly that generation: a row that changed since, or a
//! width change in between, drops it. An epoch change also tells the worker
//! to skip the queued requests of the old one.

use std::collections::{HashMap, HashSet};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};

use quark_text::{LayoutCache, TextSystem, TextSystemRecipe};

use super::{
    BlockGeometry, BlockMeasurer, Document, DocumentSource, DocumentStyle, RowItem, RowSnapshot,
    TextMeasurer, block_width, lay_out_row,
};
use crate::virtual_list::RowKey;

/// Requests in flight at once. Small enough that the worker turns to rows
/// near a new scroll position within a frame or two, large enough to keep
/// it busy between frames.
const MAX_IN_FLIGHT: usize = 64;

/// What a background thread needs to measure blocks exactly as a
/// [`BlockMeasurer`] on the UI thread does. [`TextMeasurer`] offers one.
#[derive(Debug, Clone, PartialEq)]
pub struct MeasureSpec {
    pub fonts: TextSystemRecipe,
    /// Logical points, as [`TextMeasurer::font_size`].
    pub font_size: f32,
    pub scale_factor: f32,
}

/// Everything a row's height depends on besides its header and blocks. Each distinct
/// value is one epoch.
#[derive(Debug, Clone)]
pub(super) struct RowLayout {
    spec: MeasureSpec,
    style: DocumentStyle,
    width: f32,
}

impl RowLayout {
    fn same(&self, other: &RowLayout) -> bool {
        self.spec == other.spec
            && self.style == other.style
            && self.width.to_bits() == other.width.to_bits()
    }
}

struct Job {
    row: RowKey,
    generation: u64,
    epoch: u64,
    layout: Arc<RowLayout>,
    snapshot: RowSnapshot,
}

/// A finished row. `height` is `None` when measuring it panicked.
#[derive(Clone, Copy)]
struct RowHeight {
    row: RowKey,
    generation: u64,
    height: Option<f32>,
}

/// The worker thread is gone; no more results will arrive.
struct WorkerGone;

/// Measures one row. Swapped in tests to inject a panic.
type MeasureRow = fn(&mut TextMeasurer<'_>, &RowLayout, &RowSnapshot) -> f32;

fn measure_row(measurer: &mut TextMeasurer<'_>, layout: &RowLayout, row: &RowSnapshot) -> f32 {
    let width = block_width(&layout.style, layout.width);
    measurer.apply_style(&layout.style);
    lay_out_row(
        &layout.style,
        row.header,
        row.blocks.iter().enumerate(),
        &row.adornments,
        |item, _| match item {
            RowItem::Block { block, .. } => measurer.measure(block, width).height(),
            RowItem::Adornment { height, .. } => height,
        },
    )
}

/// One background thread measuring rows. When several requests for a row
/// are queued only the newest is measured, and requests of an epoch other
/// than the current one are skipped without a result.
struct MeasureWorker {
    jobs: Sender<Job>,
    done: Receiver<RowHeight>,
    /// The epoch whose requests are measured. Dropping the worker sets it
    /// to `u64::MAX`, so the thread skips the rest of its queue and exits.
    epoch: Arc<AtomicU64>,
}

impl MeasureWorker {
    fn new(epoch: u64, measure: MeasureRow) -> Self {
        let (jobs, job_rx) = channel();
        let (done_tx, done) = channel();
        let epoch = Arc::new(AtomicU64::new(epoch));
        let current = epoch.clone();
        // Detached: the thread may be building a TextSystem with system
        // fonts (up to a second), which a drop must not wait for. A spawn
        // failure drops `done_tx`, which reads as a dead worker.
        let _ = std::thread::Builder::new()
            .name("quark-measure".to_owned())
            .spawn(move || run(job_rx, done_tx, &current, measure));
        Self { jobs, done, epoch }
    }

    /// A worker whose thread is already gone, for testing recovery.
    #[cfg(test)]
    fn gone() -> Self {
        let (jobs, _) = channel();
        let (_, done) = channel();
        Self {
            jobs,
            done,
            epoch: Arc::new(AtomicU64::new(u64::MAX)),
        }
    }

    fn send(&self, job: Job) {
        let _ = self.jobs.send(job);
    }

    fn set_epoch(&self, epoch: u64) {
        self.epoch.store(epoch, Ordering::Relaxed);
    }

    fn try_recv(&self) -> Result<Option<RowHeight>, WorkerGone> {
        match self.done.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerGone),
        }
    }

    fn recv(&self) -> Result<RowHeight, WorkerGone> {
        self.done.recv().map_err(|_| WorkerGone)
    }
}

impl Drop for MeasureWorker {
    fn drop(&mut self) {
        self.epoch.store(u64::MAX, Ordering::Relaxed);
    }
}

fn run(jobs: Receiver<Job>, done: Sender<RowHeight>, epoch: &AtomicU64, measure: MeasureRow) {
    let mut text: Option<(TextSystemRecipe, TextSystem)> = None;
    let mut layouts = LayoutCache::new(1);
    while let Ok(first) = jobs.recv() {
        // Keep the newest request per row, in the order rows were first
        // requested (nearest the window first).
        let mut order = Vec::new();
        let mut newest: HashMap<RowKey, Job> = HashMap::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            match newest.get(&job.row) {
                Some(queued) if queued.generation > job.generation => {}
                Some(_) => {
                    newest.insert(job.row, job);
                }
                None => {
                    order.push(job.row);
                    newest.insert(job.row, job);
                }
            }
        }
        layouts.begin_frame();
        for row in order {
            let Some(job) = newest.remove(&row) else {
                continue;
            };
            if job.epoch != epoch.load(Ordering::Relaxed) {
                continue;
            }
            // A shaping bug must not take the thread down, or every later
            // row would stay estimated.
            let height = catch_unwind(AssertUnwindSafe(|| {
                let spec = &job.layout.spec;
                if text
                    .as_ref()
                    .is_none_or(|(recipe, _)| *recipe != spec.fonts)
                {
                    text = Some((spec.fonts.clone(), spec.fonts.build()));
                    // Its layouts were shaped with the old fonts.
                    layouts.clear();
                }
                let (_, system) = text.as_mut()?;
                let mut measurer =
                    TextMeasurer::new(system, &mut layouts, spec.font_size, spec.scale_factor);
                Some(measure(&mut measurer, &job.layout, &job.snapshot))
            }))
            .ok()
            .flatten();
            if height.is_none() {
                // The panic may have left the cache half updated.
                layouts = LayoutCache::new(1);
            }
            let result = RowHeight {
                row,
                generation: job.generation,
                height,
            };
            if done.send(result).is_err() {
                return;
            }
        }
        layouts.trim();
    }
}

/// The UI side of background measurement: which rows wait for which
/// request, and the worker. One per [`super::MarkdownDocument`].
pub(super) struct BackgroundMeasure {
    worker: Option<MeasureWorker>,
    layout: Option<Arc<RowLayout>>,
    epoch: u64,
    generation: u64,
    /// The generation each row waits for.
    pending: HashMap<RowKey, u64>,
    /// Rows whose measurement panicked; not requested again until they
    /// change or the epoch does. They are measured when they scroll in.
    failed: HashSet<RowKey>,
    measure: MeasureRow,
}

impl Default for BackgroundMeasure {
    fn default() -> Self {
        Self {
            worker: None,
            layout: None,
            epoch: 0,
            generation: 0,
            pending: HashMap::new(),
            failed: HashSet::new(),
            measure: measure_row,
        }
    }
}

impl BackgroundMeasure {
    /// Sets what rows are measured under. A change drops every request in
    /// flight; `None` stops background measurement.
    pub(super) fn configure<M: BlockMeasurer>(
        &mut self,
        measurer: &M,
        style: DocumentStyle,
        width: f32,
    ) {
        let layout = measurer
            .background_spec()
            .map(|spec| RowLayout { spec, style, width });
        let unchanged = match (&self.layout, &layout) {
            (Some(a), Some(b)) => a.same(b),
            (None, None) => true,
            _ => false,
        };
        if unchanged {
            return;
        }
        self.epoch += 1;
        self.layout = layout.map(Arc::new);
        self.pending.clear();
        self.failed.clear();
        if let Some(worker) = &self.worker {
            worker.set_epoch(self.epoch);
        }
    }

    /// The row changed or left; any result for it is stale.
    pub(super) fn forget(&mut self, row: RowKey) {
        self.pending.remove(&row);
        self.failed.remove(&row);
    }

    /// Whether requests are in flight; draw frames until they land.
    pub(super) fn is_busy(&self) -> bool {
        !self.pending.is_empty()
    }

    /// Takes the results that have arrived. Returns whether any row's
    /// height changed.
    pub(super) fn apply<G: BlockGeometry>(&mut self, document: &mut Document<G>) -> bool {
        let mut changed = false;
        while let Some(worker) = &self.worker {
            match worker.try_recv() {
                Ok(Some(result)) => changed |= self.take(result, document),
                Ok(None) => break,
                Err(WorkerGone) => {
                    self.respawn();
                    break;
                }
            }
        }
        changed
    }

    /// Requests the estimated rows nearest the window, up to the in-flight
    /// limit.
    pub(super) fn request<G: BlockGeometry>(
        &mut self,
        document: &Document<G>,
        source: &impl DocumentSource,
    ) {
        let Some(layout) = &self.layout else {
            return;
        };
        let (pending, failed) = (&self.pending, &self.failed);
        let rows = document.unmeasured_near_window(
            MAX_IN_FLIGHT.saturating_sub(pending.len()),
            pending.len() + failed.len(),
            |row| pending.contains_key(&row) || failed.contains(&row),
        );
        if rows.is_empty() {
            return;
        }
        let (epoch, measure) = (self.epoch, self.measure);
        let worker = self
            .worker
            .get_or_insert_with(|| MeasureWorker::new(epoch, measure));
        for row in rows {
            self.generation += 1;
            self.pending.insert(row, self.generation);
            worker.send(Job {
                row,
                generation: self.generation,
                epoch,
                layout: layout.clone(),
                snapshot: document.row_snapshot(source, row),
            });
        }
    }

    /// Measures every estimated row and applies the heights, blocking until
    /// done. For tests and screenshots. Returns whether any height changed.
    pub(super) fn finish<G: BlockGeometry>(
        &mut self,
        document: &mut Document<G>,
        source: &impl DocumentSource,
    ) -> bool {
        let mut changed = self.apply(document);
        // One respawn per call, so a worker that cannot run leaves rows
        // estimated instead of hanging.
        let mut respawned = false;
        loop {
            self.request(document, source);
            let Some(worker) = self.worker.as_ref().filter(|_| self.is_busy()) else {
                return changed;
            };
            match worker.recv() {
                Ok(result) => changed |= self.take(result, document),
                Err(WorkerGone) if !respawned => {
                    respawned = true;
                    self.respawn();
                }
                Err(WorkerGone) => return changed,
            }
        }
    }

    fn take<G: BlockGeometry>(&mut self, result: RowHeight, document: &mut Document<G>) -> bool {
        if self.pending.get(&result.row) != Some(&result.generation) {
            return false;
        }
        self.pending.remove(&result.row);
        match result.height {
            Some(height) => document.set_background_height(result.row, height),
            None => {
                self.failed.insert(result.row);
                false
            }
        }
    }

    /// Drops a dead worker; the next request starts a new one and asks
    /// again for every row that was waiting.
    fn respawn(&mut self) {
        self.worker = None;
        self.pending.clear();
    }

    /// Measures rows with `measure` instead of the text measurer.
    #[cfg(test)]
    pub(super) fn set_measure(&mut self, measure: MeasureRow) {
        self.measure = measure;
        self.worker = None;
        self.pending.clear();
    }

    /// Replaces the worker with one whose thread is gone.
    #[cfg(test)]
    pub(super) fn kill_worker(&mut self) {
        self.worker = Some(MeasureWorker::gone());
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;
    use quark_text::{LayoutCache, TextSystem};

    use super::*;
    use crate::document::{MarkdownDocument, MarkdownEntry, RowChrome};

    const FONT_SIZE: f32 = 14.0;

    /// Markdown pieces covering every block kind, wide and emoji text, and
    /// long words that wrap.
    const PIECES: &[&str] = &[
        "Rows far from the viewport are measured on another thread.",
        "## A heading that is long enough to wrap at narrow widths",
        "- first item\n- second item with **bold** and `code`\n  - nested",
        "> quoted text that also wraps when the column gets narrow",
        "```rust\nfn main() {\n    println!(\"hi\");\n}\n```",
        "| a | b |\n|---|---|\n| 1 | two |",
        "---",
        "\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{6587}\u{7ae0} \u{1f600} mixed text",
        "supercalifragilisticexpialidocious_identifier_without_breaks",
        "1. one\n2. two\n3. three",
    ];

    fn entry(row: u64, markdown: String) -> MarkdownEntry {
        MarkdownEntry {
            row: RowKey(row),
            // A header band, so background heights must count it too.
            chrome: RowChrome {
                header_height: 22.0,
                ..RowChrome::default()
            },
            markdown,
        }
    }

    /// Message `i` of a history: one to four pieces chosen by `i`.
    fn history(n: u64) -> Vec<MarkdownEntry> {
        (0..n)
            .map(|i| {
                let parts: Vec<&str> = (0..1 + i % 4)
                    .map(|k| PIECES[((i * 7 + k * 3) as usize) % PIECES.len()])
                    .collect();
                entry(i, parts.join("\n\n"))
            })
            .collect()
    }

    /// A markdown document with the UI thread's text system.
    struct Ui {
        text: TextSystem,
        layouts: LayoutCache,
        md: MarkdownDocument,
        size: (f32, f32),
    }

    impl Ui {
        fn new(entries: Vec<MarkdownEntry>, size: (f32, f32)) -> Self {
            let mut md = MarkdownDocument::new(DocumentStyle::for_font_size(FONT_SIZE));
            md.extend(entries).unwrap();
            let mut ui = Self {
                text: TextSystem::vendored_only(&Default::default()),
                layouts: LayoutCache::default(),
                md,
                size,
            };
            ui.frame();
            ui
        }

        fn frame(&mut self) {
            let mut measurer = TextMeasurer::new(&mut self.text, &mut self.layouts, FONT_SIZE, 1.0);
            self.md
                .prepare(self.size.0, self.size.1, 0, false, &mut measurer);
        }

        fn scroll_to(&mut self, offset: f32) {
            self.md.document_mut().set_scroll_offset(offset);
            self.frame();
        }

        /// `key:height` per row, `key:~` for a row still estimated.
        fn heights(&self) -> String {
            let rows = self.md.document().list().rows();
            rows.keys()
                .iter()
                .map(|&key| match rows.is_measured(key) {
                    Some(true) => format!("{}:{}", key.0, rows.height_of(key).unwrap()),
                    _ => format!("{}:~", key.0),
                })
                .collect::<Vec<_>>()
                .join(" ")
        }

        /// The heights the UI thread measures for every row at this width,
        /// from a plain document whose viewport holds them all.
        fn synchronous_heights(&mut self) -> String {
            let messages = self.md.rows();
            let mut keys: Vec<&RowKey> = messages.keys().collect();
            keys.sort();
            let mut reference = Document::new(*self.md.document().style());
            reference
                .extend(keys.into_iter().map(|k| &messages[k]))
                .unwrap();
            let mut measurer = TextMeasurer::new(&mut self.text, &mut self.layouts, FONT_SIZE, 1.0);
            reference.prepare(self.size.0, 1.0e7, 0, false, messages, &mut measurer);
            let rows = reference.list().rows();
            rows.keys()
                .iter()
                .map(|&key| format!("{}:{}", key.0, rows.height_of(key).unwrap()))
                .collect::<Vec<_>>()
                .join(" ")
        }

        /// The row under the viewport's top edge and where its top sits.
        fn anchor(&self) -> String {
            let view = self.md.document();
            view.visible_rows()
                .iter()
                .find(|r| r.top <= 0.0 && r.top + r.height > 0.0)
                .map_or("-".to_owned(), |r| format!("{}@{}", r.key.0, r.top))
        }
    }

    // Catches background heights that differ from what the row measures
    // once visible, or that shift the rows on screen as they land.
    #[test]
    fn offscreen_heights_converge_to_exact_without_moving_the_anchor() {
        let mut ui = Ui::new(history(80), (420.0, 300.0));
        let max = ui.md.document().max_scroll_offset();
        ui.scroll_to((max * 0.5).round());
        let anchor = ui.anchor();

        ui.md.finish_measures();
        ui.frame();

        let expected = ui.synchronous_heights();
        assert_eq!((ui.heights(), ui.anchor()), (expected, anchor));
    }

    // Catches a height measured at the old width being applied after a
    // resize, which would leave the row marked exact at the wrong height.
    #[test]
    fn result_measured_before_a_width_change_is_dropped() {
        let mut ui = Ui::new(history(40), (420.0, 300.0));
        // Every request at 420 finishes before the resize is seen.
        let (background, _) = ui.md.background_mut();
        let in_flight = background.pending.len();
        let worker = background.worker.as_ref().unwrap();
        let stale: Vec<RowHeight> = (0..in_flight)
            .map(|_| worker.recv().ok().unwrap())
            .collect();

        ui.size.0 = 300.0;
        ui.frame();
        let (background, document) = ui.md.background_mut();
        let taken = stale
            .into_iter()
            .filter(|result| background.take(*result, document))
            .count();
        ui.md.finish_measures();

        let expected = ui.synchronous_heights();
        assert_eq!((taken, ui.heights()), (0, expected));
    }

    // Catches a streaming row off screen keeping the height of its first
    // measurement, or a stale result for its old text being applied.
    #[test]
    fn offscreen_streaming_row_gets_the_height_of_its_new_text() {
        let mut ui = Ui::new(history(30), (420.0, 300.0));
        ui.scroll_to(0.0);
        ui.md.finish_measures();

        let grown = format!("{}\n\n{}", PIECES[0], PIECES[4]);
        ui.md.set_markdown(RowKey(29), &grown).unwrap();
        ui.frame();
        ui.md.finish_measures();

        let expected = ui.synchronous_heights();
        assert_eq!(ui.heights(), expected);
    }

    // Catches the worker leaving adornments out of a row's height, which
    // would make every tool card jump as it scrolls in.
    #[test]
    fn offscreen_rows_count_their_adornments() {
        use crate::document::{AdornmentKey, AdornmentSlot, RowAdornment};
        use crate::element::IntoAnyElement;
        let mut ui = Ui::new(history(30), (420.0, 1.0));
        let first_block = |ui: &Ui, row: u64| ui.md.rows()[&RowKey(row)].blocks[0].key;
        for row in (0..30).step_by(3) {
            let adornment = |key, slot, height| {
                RowAdornment::new(AdornmentKey(key), slot, height, 0, |_| {
                    crate::element::div().into_any()
                })
            };
            let adornments = vec![
                adornment(0, AdornmentSlot::Start, 36.0),
                adornment(1, AdornmentSlot::Before(first_block(&ui, row)), 12.0),
                adornment(2, AdornmentSlot::End, 28.0),
            ];
            ui.md.set_adornments(RowKey(row), adornments).unwrap();
        }
        ui.frame();

        ui.md.finish_measures();

        let expected = ui.synchronous_heights();
        assert_eq!(ui.heights(), expected);
    }

    fn panics_on_boom(
        measurer: &mut TextMeasurer<'_>,
        layout: &RowLayout,
        row: &RowSnapshot,
    ) -> f32 {
        assert!(
            row.blocks.iter().all(|b| !b.text().contains("boom")),
            "shaping bug"
        );
        measure_row(measurer, layout, row)
    }

    // Catches a panicking measurement killing the worker (every later row
    // would stay estimated) or being requested again forever.
    #[test]
    fn panicking_row_stays_estimated_and_the_rest_converge() {
        let mut entries = history(20);
        entries[3].markdown = "boom".to_owned();
        let mut ui = Ui::new(entries, (420.0, 200.0));
        ui.md.background_mut().0.set_measure(panics_on_boom);
        ui.frame();

        ui.md.finish_measures();

        let expected: Vec<String> = ui
            .synchronous_heights()
            .split(' ')
            .map(|h| {
                if h.starts_with("3:") {
                    "3:~".to_owned()
                } else {
                    h.to_owned()
                }
            })
            .collect();
        let expected = expected.join(" ");
        assert_eq!((ui.heights(), ui.md.is_measuring()), (expected, false));
    }

    // Catches a dead worker leaving rows estimated for good, or hanging
    // the caller waiting for results that never come.
    #[test]
    fn dead_worker_is_replaced_and_does_not_hang() {
        let mut ui = Ui::new(history(20), (420.0, 200.0));
        ui.md.background_mut().0.kill_worker();

        ui.md.finish_measures();

        let expected = ui.synchronous_heights();
        assert_eq!(ui.heights(), expected);
    }

    // Catches rows measured before a font change keeping their old heights,
    // on screen or off, and the worker reusing layouts its previous fonts
    // shaped: rows added after the change hold text it measured before.
    #[test]
    fn every_row_gets_its_height_in_the_new_fonts() {
        let inter = quark_text::FontSettings {
            ui_family: "Inter".into(),
            ..Default::default()
        };
        type Change = fn(&mut Ui, &quark_text::FontSettings);
        let changes: [(&str, Change); 2] = [
            ("font settings", |ui, settings| {
                ui.text.set_font_settings(settings)
            }),
            ("replaced text system", |ui, settings| {
                ui.text = TextSystem::vendored_only(settings)
            }),
        ];
        let text = "iiiiMMMM ".repeat(16);
        let rows = |keys: std::ops::Range<u64>| keys.map(|i| entry(i, text.clone())).collect();
        for (name, change) in changes {
            // A few rows on screen, the rest measured by the worker.
            let mut ui = Ui::new(rows(0..10), (300.0, 150.0));
            ui.md.finish_measures();
            let before = ui.heights();

            change(&mut ui, &inter);
            ui.md.extend(rows(10..20)).unwrap();
            ui.frame();
            ui.md.finish_measures();

            let expected = ui.synchronous_heights();
            let height = |heights: &str| heights.split([' ', ':']).nth(1).unwrap_or("").to_owned();
            assert_ne!(
                height(&before),
                height(&expected),
                "the fonts wrap the text differently"
            );
            assert_eq!(ui.heights(), expected, "{name}");
        }
    }

    // Catches the worker measuring at another line height than the UI
    // thread, and a line height change moving the rows on screen: after
    // the change every height, on screen or off, is the one the UI thread
    // measures, and the row at the top keeps its place.
    #[test]
    fn a_line_height_change_remeasures_every_row_without_moving_the_anchor() {
        use crate::element::LineHeight;
        let mut ui = Ui::new(history(60), (420.0, 300.0));
        let max = ui.md.document().max_scroll_offset();
        ui.scroll_to((max * 0.5).round());
        ui.md.finish_measures();
        ui.frame();
        let (before, anchor) = (ui.heights(), ui.anchor());

        let style = ui
            .md
            .document()
            .style()
            .with_line_height(LineHeight::Points(22.0));
        ui.md.document_mut().set_style(style);
        ui.frame();
        ui.md.finish_measures();
        ui.frame();

        let expected = ui.synchronous_heights();
        assert_ne!(before, expected, "the line height changes the heights");
        assert_eq!((ui.heights(), ui.anchor()), (expected, anchor));
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(12))]

        // Catches the worker shaping differently from the UI thread: other
        // fonts, scale, width, or gaps would make rows jump when they
        // scroll in.
        #[test]
        fn background_heights_equal_synchronous_heights(
            messages in prop::collection::vec(
                prop::collection::vec(prop::sample::select(PIECES), 1..4),
                2..8,
            ),
            width in 120.0f32..700.0,
        ) {
            let entries = messages
                .iter()
                .enumerate()
                .map(|(i, parts)| entry(i as u64, parts.join("\n\n")))
                .collect();
            // A viewport of one pixel leaves nearly every row to the worker.
            let mut ui = Ui::new(entries, (width.round(), 1.0));

            ui.md.finish_measures();

            let expected = ui.synchronous_heights();
            prop_assert_eq!(ui.heights(), expected);
        }
    }
}
