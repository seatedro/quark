//! Inserting and removing rows in the middle of a markdown document: the
//! rows on screen, the selection, and find keep up.

use quark::selection::{BlockKey, Selection, SelectionPoint};
use quark_text::{LayoutCache, TextSystem};

use super::*;

const FONT_SIZE: f32 = 14.0;
const SIZE: (f32, f32) = (420.0, 300.0);
/// Wraps onto several lines at [`SIZE`], so an inserted step's estimate
/// differs from its exact height.
const STEP: &str = "A step of the agent's work, long enough to wrap onto a few lines in a \
                    narrow transcript so that its estimated height is wrong";

fn entry(row: u64, markdown: &str) -> MarkdownEntry {
    MarkdownEntry {
        row: RowKey(row),
        chrome: RowChrome::default(),
        markdown: markdown.to_owned(),
    }
}

/// Rows `0..n` reading `"row {i}"`.
fn transcript(n: u64) -> Vec<MarkdownEntry> {
    (0..n).map(|i| entry(i, &format!("row {i}"))).collect()
}

/// A markdown document laid out with real text at [`SIZE`].
struct Ui {
    text: TextSystem,
    layouts: LayoutCache,
    md: MarkdownDocument,
}

impl Ui {
    fn new(entries: Vec<MarkdownEntry>) -> Self {
        let mut md = MarkdownDocument::new(DocumentStyle::for_font_size(FONT_SIZE));
        md.extend(entries).unwrap();
        let mut ui = Self {
            text: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
            md,
        };
        ui.settle();
        ui
    }

    fn frame(&mut self) {
        let mut measurer = TextMeasurer::new(&mut self.text, &mut self.layouts, FONT_SIZE, 1.0);
        self.md.prepare(SIZE.0, SIZE.1, 0, &mut measurer);
    }

    /// Prepares, makes every height exact, and prepares again.
    fn settle(&mut self) {
        self.frame();
        self.md.finish_measures();
        self.frame();
    }

    fn scroll_to(&mut self, offset: f64) {
        self.md.document_mut().set_scroll_offset(offset);
        self.settle();
    }

    /// Where row `row`'s top sits relative to the viewport top.
    fn top(&self, row: u64) -> f32 {
        let list = self.md.document().list();
        (list.rows().offset_of(RowKey(row)).unwrap() - list.scroll_offset()) as f32
    }

    /// The row under the viewport's top edge and where its top sits.
    fn first_visible(&self) -> String {
        self.md
            .document()
            .visible_rows()
            .iter()
            .find(|r| r.top <= 0.0 && r.top + r.height > 0.0)
            .map_or("-".to_owned(), |r| format!("{}@{}", r.key.0, r.top))
    }

    fn bottom(&self) -> String {
        let view = self.md.document();
        let last = view.visible_rows().last().unwrap();
        format!(
            "{} ends at {}, pinned {}",
            last.key.0,
            last.top + last.height,
            view.is_stuck_to_bottom()
        )
    }

    fn block(&self, row: u64) -> BlockKey {
        self.md.rows()[&RowKey(row)].blocks[0].key
    }

    /// Selects from byte `from.1` of row `from.0`'s first block to byte
    /// `to.1` of row `to.0`'s.
    fn select(&mut self, from: (u64, usize), to: (u64, usize)) {
        let point = |(row, byte)| SelectionPoint::new(self.block(row), byte);
        let selection = Selection::new(point(from), point(to));
        self.md.document_mut().set_selection(Some(selection));
    }

    /// `"<row>:<start>..<end>"` per match, the current one starred.
    fn find_dump(&self) -> String {
        let Some(find) = self.md.document().find() else {
            return "closed".to_owned();
        };
        let row_of = |block: BlockKey| {
            self.md
                .rows()
                .values()
                .find(|r| r.blocks.iter().any(|b| b.key == block))
                .map_or(u64::MAX, |r| r.key.0)
        };
        find.matches()
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let star = if find.current_index() == Some(i) {
                    "*"
                } else {
                    ""
                };
                format!(
                    "{star}{}:{}..{}",
                    row_of(m.block),
                    m.range.start,
                    m.range.end
                )
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

// Catches inserted rows moving the rows on screen (before or after their
// heights become exact), or leaving the selection: steps expand above the
// view, between the selected rows, and inside the view.
#[test]
fn inserting_steps_keeps_the_visible_row_and_selection() {
    type Setup = fn(&mut Ui);
    type Position = fn(&Ui) -> String;
    let cases: [(&str, Setup, Position); 3] = [
        (
            "first visible row",
            |ui| {
                let row = ui.md.document().list().rows().offset_of(RowKey(30));
                ui.scroll_to(row.unwrap() - 10.0);
            },
            Ui::first_visible,
        ),
        ("pinned to the bottom", |_| {}, Ui::bottom),
        (
            "anchored row",
            |ui| {
                ui.md.document_mut().anchor_row(RowKey(30), 150.0).unwrap();
                ui.settle();
            },
            |ui| format!("30@{}", ui.top(30)),
        ),
    ];
    let directions = [((10, 0), (11, 6)), ((11, 6), (10, 0))];
    for ((name, setup, position), (from, to)) in
        cases.into_iter().zip(directions.into_iter().cycle())
    {
        let mut ui = Ui::new(transcript(60));
        setup(&mut ui);
        // Forward and backward selections alike.
        ui.select(from, to);
        let before = position(&ui);

        ui.md
            .insert_after(
                RowKey(10),
                [
                    entry(100, &format!("{STEP} a")),
                    entry(101, &format!("{STEP} b")),
                ],
            )
            .unwrap();
        // Inside the anchored case's view, above its anchored row.
        let after_28 = ui.md.document().list().rows().index_of(RowKey(28)).unwrap() + 1;
        ui.md.insert(after_28, [entry(102, STEP)]).unwrap();
        // Inside the pinned case's view.
        ui.md.insert_after(RowKey(55), [entry(103, STEP)]).unwrap();
        ui.settle();

        assert_eq!(
            (position(&ui), ui.md.selected_text()),
            (before, format!("row 10\n\n{STEP} a\n\n{STEP} b\n\nrow 11")),
            "{name}"
        );
    }
}

// Catches a rejected batch adopting some of its rows (or their markdown)
// before failing, which would leave rows the document never shows.
#[test]
fn a_rejected_insert_changes_no_document_content() {
    type Insert = fn(&mut MarkdownDocument) -> Result<(), RowError>;
    let cases: [(&str, Insert, RowError); 6] = [
        (
            "index past the end",
            |md| md.insert(5, [entry(4, "row 4 word")]),
            RowError::IndexOutOfBounds { index: 5, len: 4 },
        ),
        (
            "empty batch past the end",
            |md| md.insert(5, []),
            RowError::IndexOutOfBounds { index: 5, len: 4 },
        ),
        (
            "missing after-key",
            |md| md.insert_after(RowKey(9), [entry(4, "row 4 word")]),
            RowError::UnknownKey(RowKey(9)),
        ),
        (
            "empty batch after a missing key",
            |md| md.insert_after(RowKey(9), []),
            RowError::UnknownKey(RowKey(9)),
        ),
        (
            "key already present",
            |md| md.insert(1, [entry(4, "row 4 word"), entry(2, "row 2 word")]),
            RowError::DuplicateKey(RowKey(2)),
        ),
        (
            "key repeated in the batch",
            |md| md.insert(1, [entry(4, "row 4 word"), entry(4, "row 4 word")]),
            RowError::DuplicateKey(RowKey(4)),
        ),
    ];
    let dump = |ui: &Ui| {
        let order: Vec<u64> = ui
            .md
            .document()
            .list()
            .rows()
            .keys()
            .iter()
            .map(|k| k.0)
            .collect();
        let markdown: Vec<Option<&str>> = (0..5).map(|row| ui.md.markdown(RowKey(row))).collect();
        format!(
            "{order:?} {markdown:?} {:?} {}",
            ui.md.selected_text(),
            ui.find_dump()
        )
    };
    for (name, insert, error) in cases {
        let mut ui = Ui::new((0..4).map(|i| entry(i, &format!("row {i} word"))).collect());
        ui.select((1, 4), (2, 3));
        ui.md.set_find_query("word");
        ui.md.find_next(ScrollAlign::Center);
        let before = dump(&ui);

        let result = insert(&mut ui.md);
        ui.settle();

        assert_eq!((result, dump(&ui)), (Err(error), before), "{name}");
    }
}

// Catches find answering from before an edit until the next prepare, or
// losing its place: inserted matches join in row order around the current
// one, and removing the current match's row moves on to the next match.
#[test]
fn inserted_matches_keep_the_current_match_and_follow_row_order() {
    let mut ui = Ui::new((0..5).map(|i| entry(i, &format!("word {i}"))).collect());
    ui.md.set_find_query("word");
    ui.md.find_next(ScrollAlign::Center);
    ui.md.find_next(ScrollAlign::Center);
    let mut seen = vec![ui.find_dump()];

    ui.md
        .insert_after(RowKey(1), [entry(10, "a word")])
        .unwrap();
    ui.md
        .insert_after(RowKey(2), [entry(11, "word, word")])
        .unwrap();
    seen.push(ui.find_dump());
    ui.md.find_next(ScrollAlign::Center);
    seen.push(ui.find_dump());
    ui.md.remove(RowKey(11)).unwrap();
    seen.push(ui.find_dump());
    ui.md.find_next(ScrollAlign::Center);
    ui.md.remove(RowKey(4)).unwrap();
    seen.push(ui.find_dump());

    assert_eq!(
        seen,
        [
            "0:0..4 1:0..4 *2:0..4 3:0..4 4:0..4",
            "0:0..4 1:0..4 10:2..6 *2:0..4 11:0..4 11:6..10 3:0..4 4:0..4",
            "0:0..4 1:0..4 10:2..6 2:0..4 *11:0..4 11:6..10 3:0..4 4:0..4",
            "0:0..4 1:0..4 10:2..6 2:0..4 *3:0..4 4:0..4",
            "*0:0..4 1:0..4 10:2..6 2:0..4 3:0..4",
        ]
    );
}

// Catches removing the first visible row shifting the rows below it up
// under the reader, or the selection keeping an endpoint in the removed
// row. With no row after it, the row before it stays and the view clamps
// to the end.
#[test]
fn removing_the_visible_row_preserves_the_next_survivor() {
    let tall = format!(
        "row 59 {}",
        "words in a reply too long to fit the view ".repeat(40)
    );
    let mut entries = transcript(59);
    entries.push(entry(59, &tall));
    // (name, removed row, selection, what is checked, where it ends up,
    // copy). `None` expects the position from before the removal.
    type Position = fn(&Ui) -> String;
    type Case<'a> = (
        &'a str,
        u64,
        (u64, usize),
        (u64, usize),
        Position,
        Option<&'a str>,
        &'a str,
    );
    let cases: [Case; 2] = [
        (
            "a row in the middle",
            30,
            (30, 2),
            (31, 3),
            |ui| format!("31@{}", ui.top(31)),
            None,
            "row",
        ),
        (
            "the last row",
            59,
            (58, 3),
            (59, 2),
            |ui| {
                format!(
                    "58 ends at {}",
                    ui.top(58)
                        + ui.md
                            .document()
                            .list()
                            .rows()
                            .height_of(RowKey(58))
                            .unwrap()
                )
            },
            Some("58 ends at 300"),
            " 58",
        ),
    ];
    for (name, removed, from, to, position, expected, copy) in cases {
        let mut ui = Ui::new(entries.clone());
        let top = ui.md.document().list().rows().offset_of(RowKey(removed));
        ui.scroll_to(top.unwrap() + 5.0);
        ui.select(from, to);
        let expected = expected.map_or_else(|| position(&ui), str::to_owned);

        ui.md.remove(RowKey(removed)).unwrap();
        ui.settle();

        assert_eq!(
            (position(&ui), ui.md.selected_text()),
            (expected, copy.to_owned()),
            "{name}"
        );
    }
}
