use super::spans::merge_highlight_styles;
use super::*;
use crate::text_input::styles::InlineStyle;
use TextEditCommand::*;
use quark_render::scene::FontWeight;
use quark_text::TextSystem;

fn make_editor(width: f32, height: f32) -> (TextSystem, Editor) {
    let text_system = TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::default();
    editor.sync_size(width, height);
    (text_system, editor)
}

const TWO_LINES: f32 = 2.0 * 14.0 * LINE_HEIGHT_FACTOR;

#[test]
fn manual_scroll_persists_across_flush() {
    let (mut text_system, mut editor) = make_editor(220.0, TWO_LINES);
    editor.apply(InsertText("line0\nline1\nline2\nline3".into()));
    editor.flush(&mut text_system);
    let line_height = editor.scroll_line_height_px();
    assert!((editor.scroll_y - line_height * 2.0).abs() < 0.5);

    editor.scroll(-line_height);
    editor.flush(&mut text_system);

    assert!(
        (editor.scroll_y - line_height).abs() < 0.5,
        "expected manual scroll to persist, got {}",
        editor.scroll_y
    );
}

// Catches the content height (and so the scroll range) stopping short
// of the caret's line, which left the caret unreachable.
#[test]
fn typing_scrolls_the_caret_line_into_a_content_height_that_covers_it() {
    let words = "asdf asf asdf asdf fasd fasd fasdf sdaf asdf sadf ".repeat(4);
    let cases = [
        ("trailing newline", "first line\nsecond line\n", 160.0),
        ("wrapped past the viewport", words.as_str(), 120.0),
    ];
    for (name, text, width) in cases {
        let (mut text_system, mut editor) = make_editor(width, TWO_LINES);
        editor.apply(InsertText(text.into()));
        editor.flush(&mut text_system);

        let top = editor.cursor_pos.y;
        let bottom = top + editor.scroll_line_height_px();
        assert!(
            editor.content_height() + 0.5 >= bottom,
            "{name}: content height"
        );
        let visible = editor.scroll_y - 0.5..editor.scroll_y + TWO_LINES + 0.5;
        assert!(
            visible.contains(&top) && visible.contains(&bottom),
            "{name}: caret {top}..{bottom} outside {visible:?}"
        );
    }
}

// A chat input's growth: as tall as its lines (22pt each here) between
// a 48pt floor and a six-line cap, then scrolling to keep the caret on
// its last line. Catches the field growing past the cap, staying short,
// or keeping a stale scroll after it shrinks back.
#[test]
fn flush_fit_grows_with_the_lines_up_to_the_cap_then_scrolls() {
    let mut text_system = TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::default();
    editor.set_line_height(Some(22.0));
    let lines = |n: usize| vec!["line"; n].join("\n");
    let cases = [
        (1, 48.0, 0.0),
        (3, 66.0, 0.0),
        (6, 132.0, 0.0),
        (9, 132.0, 9.0 * 22.0 - 132.0),
        (2, 48.0, 0.0),
    ];
    for (n, height, scroll) in cases {
        editor.set_text(&lines(n));
        editor.apply(SetTextCursor(usize::MAX));
        let fit = editor.flush_fit(&mut text_system, 300.0, 48.0, 132.0);
        assert_eq!((n, fit), (n, height));
        assert!(
            (editor.scroll_y - scroll).abs() < 0.5,
            "{n} lines: scroll {}",
            editor.scroll_y
        );
    }
}

#[test]
fn diff_read_only_mode_blocks_user_mutations() {
    let mut editor = Editor::new(EditorMode::DiffReadOnly);
    editor.set_text("unchanged");
    editor.apply(InsertText(" edited".into()));
    editor.apply(Backspace);
    editor.apply(Undo);
    assert_eq!(editor.text(), "unchanged");
}

// Regression: select_line_at sliced the text at the raw offset, so a
// triple click inside a multibyte char panicked.
#[test]
fn select_line_at_inside_a_multibyte_char_selects_its_line() {
    let mut editor = Editor::default();
    editor.set_text("\u{e9}\nb");
    editor.apply(SelectLineAt(1));
    assert_eq!(editor.selected_text(), Some("\u{e9}\n"));
}

fn bold_first_line() -> StyleHighlighter {
    Arc::new(|text: &str| {
        let end = text.find('\n').unwrap_or(text.len());
        vec![StyleSpan::new(0..end, InlineStyle::BOLD)]
    })
}

#[test]
fn style_highlighter_bolds_prose_for_layout_only() {
    let (mut text_system, mut editor) = make_editor(400.0, 200.0);
    editor.set_style_highlighter(bold_first_line());
    editor.apply(InsertText("Fix\n\nbody".into()));
    editor.flush(&mut text_system);
    // The first line's layout, where the subject is.
    let bold = |editor: &Editor| {
        let first = editor.window().lines().first().expect("first line");
        first
            .layout
            .spans()
            .iter()
            .filter(|s| s.weight == Some(FontWeight::Bold))
            .map(|s| (s.range.start, s.range.end))
            .collect::<Vec<_>>()
    };
    assert_eq!(bold(&editor), [(0, 3)]);

    editor.apply(SetTextCursor(3));
    editor.apply(InsertText(" bug".into()));
    editor.flush(&mut text_system);
    assert_eq!(bold(&editor), [(0, 7)]);
    assert_eq!(editor.cursor().get(), 7);
    assert!(editor.rich_text().styles.is_empty());

    editor.apply(Undo);
    editor.flush(&mut text_system);
    assert_eq!(editor.text(), "Fix\n\nbody");
    assert_eq!(bold(&editor), [(0, 3)]);
}

#[test]
fn highlight_styles_union_with_the_text_formatting() {
    let text = "subject line";
    let document = [StyleSpan::new(8..12, InlineStyle::ITALIC)];
    let highlight = [
        StyleSpan::new(0..7, InlineStyle::BOLD),
        StyleSpan::new(4..10, InlineStyle::BOLD),
        StyleSpan::new(5..99, InlineStyle::UNDERLINE),
    ];
    let merged = merge_highlight_styles(text, &document, &highlight);
    assert_eq!(
        merged,
        [
            StyleSpan::new(0..8, InlineStyle::BOLD),
            StyleSpan::new(8..10, InlineStyle::BOLD.union(InlineStyle::ITALIC)),
            StyleSpan::new(10..12, InlineStyle::ITALIC),
        ]
    );
}

/// `lines` lines of code, each about 45 bytes and ending in a newline.
fn code_lines(lines: usize) -> String {
    (0..lines)
        .map(|i| format!("fn line_{i:05}() {{ let x = {i} * 2; }} // note\n"))
        .collect()
}

/// The painted line holding the caret.
fn caret_line(editor: &Editor) -> &WindowLine {
    let line = editor.rope().byte_to_line(editor.cursor().get());
    editor
        .window()
        .lines()
        .iter()
        .find(|l| l.line == line)
        .expect("the caret's line is shaped")
}

// Catches layout, editing, or hit-testing that costs O(document): a 2 MB
// document edited in its middle shapes only the lines around the view,
// never copies its text whole, and places caret, hits, and selection on
// the glyphs it shaped.
#[test]
fn editing_the_middle_of_a_multi_megabyte_document_shapes_only_the_view() {
    let text = code_lines(45_000);
    assert!(text.len() > 2_000_000);
    let (mut text_system, mut editor) = (
        TextSystem::vendored_only(&Default::default()),
        Editor::new(EditorMode::CodeInput),
    );
    editor.sync_size(800.0, 600.0);
    editor.set_text(&text);
    editor.apply(SetTextCursor(text.len() / 2));
    editor.flush(&mut text_system);

    let ((), bytes) = crate::test_alloc::bytes(|| {
        editor.apply(InsertText("typed".into()));
        editor.flush(&mut text_system);
    });
    assert!(
        editor.shaped_line_count() < 120,
        "{} lines shaped",
        editor.shaped_line_count()
    );
    assert!(
        bytes < text.len() as u64 / 8,
        "the edit allocated {bytes} bytes"
    );

    let painted = caret_line(&editor);
    let caret = painted.layout.caret(editor.cursor().get() - painted.start);
    let at = (caret.x, painted.top + caret.y);
    assert_eq!((editor.cursor_pos.x, editor.cursor_pos.y), at);
    let view = editor.scroll_y..editor.scroll_y + 600.0;
    assert!(view.contains(&at.1), "caret {at:?} outside {view:?}");
    assert_eq!(editor.offset_at(at.0, at.1 + 1.0), Some(editor.cursor()));
    assert!(editor.line_str(painted.line).contains("typed"));

    editor.apply(SelectWordAt(editor.cursor().get() - 1));
    editor.flush(&mut text_system);
    let painted = caret_line(&editor);
    let selection = editor.selection();
    let local = selection.start.get() - painted.start..selection.end.get() - painted.start;
    let expected: Vec<_> = painted
        .layout
        .selection_rects(local)
        .map(|r| (r.x, painted.top + r.y, r.width))
        .collect();
    let rects: Vec<_> = editor
        .selection_rects()
        .iter()
        .map(|r| (r.x, r.y, r.w))
        .collect();
    assert_eq!(rects, expected);
    assert_eq!(editor.text_len(), text.len() + "typed".len());
}

// Catches an edit reshaping lines it did not touch: typing on a line
// reshapes it alone, and a newline splitting a line above others leaves
// their layouts as they were, only moved down.
#[test]
fn an_edit_reshapes_only_the_lines_it_changed() {
    let (mut text_system, mut editor) = make_editor(400.0, 400.0);
    let text: String = (0..10).map(|i| format!("line {i}\n")).collect();
    editor.set_text(&text);
    editor.flush(&mut text_system);
    let layouts = |editor: &Editor| -> Vec<(usize, Arc<quark_text::TextLayout>)> {
        editor
            .window()
            .lines()
            .iter()
            .map(|l| (l.line, l.layout.clone()))
            .collect()
    };
    let same = |a: &[(usize, Arc<quark_text::TextLayout>)],
                b: &[(usize, Arc<quark_text::TextLayout>)]| {
        a.iter()
            .map(|(line, layout)| {
                b.iter()
                    .any(|(l, other)| l == line && Arc::ptr_eq(layout, other))
            })
            .collect::<Vec<_>>()
    };
    let before = layouts(&editor);

    editor.apply(SetTextCursor("line 0\nline 1\nline 2\nli".len()));
    editor.apply(InsertText("x".into()));
    editor.flush(&mut text_system);
    let typed = layouts(&editor);
    let kept = same(&before, &typed);
    assert_eq!(kept.iter().filter(|k| !**k).count(), 1, "{kept:?}");
    assert!(!kept[3], "line 3 was reshaped");

    editor.apply(InsertNewline);
    editor.flush(&mut text_system);
    let split = layouts(&editor);
    let moved: Vec<_> = typed
        .iter()
        .filter(|(line, _)| *line > 3)
        .map(|(line, layout)| (line + 1, layout.clone()))
        .collect();
    assert!(
        same(&moved, &split).iter().all(|k| *k),
        "lines below kept their layouts"
    );
    assert!(
        same(&typed[..3], &split).iter().all(|k| *k),
        "lines above kept theirs"
    );
}

// Catches lines jumping on screen when shaping refines the estimated
// heights of lines above the view: the line scrolled to the top stays
// there.
#[test]
fn refining_heights_above_the_view_keeps_the_top_line_in_place() {
    let (mut text_system, mut editor) = make_editor(200.0, 300.0);
    editor.set_line_height(Some(20.0));
    // Every line wraps, so each is taller than the one-line estimate.
    let text: String = (0..2000)
        .map(|i| format!("{i} {}\n", "word ".repeat(30)))
        .collect();
    editor.set_text(&text);
    editor.flush(&mut text_system);
    // Lines 0..500 are unshaped, at 20 each: line 500 is at the top.
    editor.scroll_y = 500.0 * 20.0;
    editor.flush(&mut text_system);

    let top = editor
        .window()
        .lines()
        .iter()
        .find(|l| l.line == 500)
        .expect("line 500 shaped")
        .top;
    assert!(
        (top - editor.scroll_y).abs() < 0.5,
        "line 500 at {top}, view at {}",
        editor.scroll_y
    );
    assert!(
        editor
            .window()
            .lines()
            .first()
            .is_some_and(|l| l.line < 500),
        "lines above it were shaped"
    );
}

// Catches vertical movement losing its column or line across the edges
// of logical lines, now that each line has its own layout.
#[test]
fn up_and_down_cross_lines_at_the_remembered_column() {
    let (mut text_system, mut editor) = (
        TextSystem::vendored_only(&Default::default()),
        Editor::new(EditorMode::CodeInput),
    );
    editor.sync_size(400.0, 200.0);
    editor.set_text("abc\ndefgh\nij");
    editor.apply(SetTextCursor(2));
    let mut seen = Vec::new();
    for cmd in [
        CursorDown, CursorDown, CursorUp, CursorUp, CursorUp, CursorDown,
    ] {
        editor.flush(&mut text_system);
        editor.apply(cmd);
        seen.push(editor.cursor().get());
    }
    // Column 2 of "defgh", the end of "ij", back to column 2 (the
    // remembered x), then column 2 of "abc", past the top to 0, and down
    // to "defgh" at column 0.
    assert_eq!(seen, [6, 12, 6, 2, 0, 4]);
}

// Catches a frame doing work, or allocating, when nothing changed.
#[test]
fn a_flush_with_nothing_changed_allocates_nothing() {
    let text = code_lines(45_000);
    let (mut text_system, mut editor) = (
        TextSystem::vendored_only(&Default::default()),
        Editor::new(EditorMode::CodeInput),
    );
    editor.set_line_highlighter(Arc::new(Keywords::default()));
    editor.sync_size(800.0, 600.0);
    editor.set_text(&text);
    editor.apply(SetTextCursor(text.len() / 2));
    for _ in 0..40 {
        editor.flush(&mut text_system);
    }
    assert!(!editor.is_highlighting());
    let ((), allocations) = crate::test_alloc::count(|| editor.flush(&mut text_system));
    assert_eq!(allocations, 0);
}

/// Marks each `fn` a keyword, counting the lines it was asked for.
#[derive(Default)]
struct Keywords {
    lines: std::sync::atomic::AtomicUsize,
}

impl LineHighlighter for Keywords {
    fn highlight_line(
        &self,
        line: &str,
        state: LineState,
        spans: &mut Vec<SyntaxSpan>,
    ) -> LineState {
        self.lines
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        spans.extend(line.match_indices("fn").map(|(i, _)| SyntaxSpan {
            offset: i as u32,
            length: 2,
            kind: SyntaxTokenKind::Keyword,
        }));
        state
    }
}

/// Whether the last line in view has a keyword painted semibold at its
/// start.
fn last_line_has_a_keyword(editor: &Editor) -> bool {
    editor
        .window()
        .lines()
        .iter()
        .rev()
        .find(|l| !l.layout.text().is_empty())
        .is_some_and(|l| {
            l.layout
                .spans()
                .iter()
                .any(|s| s.range == (0..2) && s.weight == Some(FontWeight::Semibold))
        })
}

// Catches line highlighting that walks the whole document before showing
// the view, re-highlights lines it already did, or redoes the view after
// an edit that changed one line.
#[test]
fn a_line_highlighter_highlights_a_long_documents_view_first_and_each_line_once() {
    let text = code_lines(12_000);
    assert!(text.len() > 256 * 1024);
    let (mut text_system, mut editor) = (
        TextSystem::vendored_only(&Default::default()),
        Editor::new(EditorMode::CodeInput),
    );
    let keywords = Arc::new(Keywords::default());
    editor.set_line_highlighter(keywords.clone());
    editor.sync_size(800.0, 400.0);
    editor.set_text(&text);
    editor.flush(&mut text_system);
    let asked = |k: &Keywords| k.lines.load(std::sync::atomic::Ordering::Relaxed);

    assert!(
        last_line_has_a_keyword(&editor),
        "the view at the end is highlighted"
    );
    assert!(
        asked(&keywords) < 3000,
        "{} lines highlighted for the first view",
        asked(&keywords)
    );
    let mut flushes = 0;
    while editor.is_highlighting() && flushes < 50 {
        editor.flush(&mut text_system);
        flushes += 1;
    }
    assert!(!editor.is_highlighting());
    assert!(
        asked(&keywords) <= 12_001 + 64,
        "{} lines highlighted",
        asked(&keywords)
    );

    let before = asked(&keywords);
    editor.apply(SetTextCursor(text.len() - 10));
    editor.apply(InsertText("fn ".into()));
    editor.flush(&mut text_system);
    assert_eq!(asked(&keywords) - before, 1);
}

/// `/* ... */` comments that span lines: state 1 is inside one.
struct BlockComments;

impl LineHighlighter for BlockComments {
    fn highlight_line(
        &self,
        line: &str,
        state: LineState,
        spans: &mut Vec<SyntaxSpan>,
    ) -> LineState {
        let (mut at, mut inside) = (0, state == LineState(1));
        while at < line.len() {
            let rest = line.get(at..).unwrap_or_default();
            let (marker, closing) = if inside { ("*/", true) } else { ("/*", false) };
            let Some(found) = rest.find(marker) else {
                if inside {
                    spans.push(SyntaxSpan {
                        offset: at as u32,
                        length: rest.len() as u32,
                        kind: SyntaxTokenKind::Comment,
                    });
                }
                break;
            };
            let end = at + found + marker.len();
            if closing {
                spans.push(SyntaxSpan {
                    offset: at as u32,
                    length: (end - at) as u32,
                    kind: SyntaxTokenKind::Comment,
                });
            }
            at = if closing { end } else { at + found };
            inside = !closing;
        }
        LineState(u64::from(inside))
    }
}

/// Each line in view's comment ranges, as `(start, end)`.
fn comments(editor: &Editor) -> Vec<Vec<(usize, usize)>> {
    editor
        .window()
        .lines()
        .iter()
        .map(|l| {
            l.layout
                .spans()
                .iter()
                .zip(l.kinds.iter())
                .filter(|(_, kind)| **kind == SyntaxTokenKind::Comment)
                .map(|(s, _)| (s.range.start, s.range.end))
                .collect()
        })
        .collect()
}

// Catches line highlighting that ignores the state a line starts in: an
// edit opening a block comment re-highlights the lines below it until
// the comment closes, and undoing it puts them back.
#[test]
fn opening_a_block_comment_rehighlights_the_lines_below_until_it_closes() {
    let (mut text_system, mut editor) = (
        TextSystem::vendored_only(&Default::default()),
        Editor::new(EditorMode::CodeInput),
    );
    editor.set_line_highlighter(Arc::new(BlockComments));
    editor.sync_size(400.0, 300.0);
    editor.set_text("let a;\nlet b;\nc */ d\nlet e;");
    editor.flush(&mut text_system);
    let plain: Vec<Vec<(usize, usize)>> = vec![vec![]; 4];
    assert_eq!(comments(&editor), plain);

    editor.apply(SetTextCursor(0));
    editor.apply(InsertText("/*".into()));
    editor.flush(&mut text_system);
    assert_eq!(
        comments(&editor),
        [vec![(0, 8)], vec![(0, 6)], vec![(0, 4)], vec![]]
    );

    editor.apply(Undo);
    editor.flush(&mut text_system);
    assert_eq!(comments(&editor), plain);
}

// Regression: a whole-text highlighter stopped at 256 KiB, so a long
// document showed no highlighting at all.
#[test]
fn a_whole_text_highlighter_highlights_the_view_of_a_long_document() {
    let text = code_lines(12_000);
    let longest = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = longest.clone();
    let (mut text_system, mut editor) = (
        TextSystem::vendored_only(&Default::default()),
        Editor::new(EditorMode::CodeInput),
    );
    editor.set_syntax_highlighter(Arc::new(move |text: &str| {
        seen.fetch_max(text.len(), std::sync::atomic::Ordering::Relaxed);
        text.match_indices("fn")
            .map(|(i, _)| SyntaxSpan {
                offset: i as u32,
                length: 2,
                kind: SyntaxTokenKind::Keyword,
            })
            .collect()
    }));
    editor.sync_size(800.0, 400.0);
    editor.set_text(&text);
    editor.flush(&mut text_system);

    assert!(last_line_has_a_keyword(&editor));
    let longest = longest.load(std::sync::atomic::Ordering::Relaxed);
    assert!(
        longest < 64 * 1024,
        "the highlighter was handed {longest} bytes"
    );
}

// Catches a formatting change, which edits no text, leaving its line
// painted as it was.
#[test]
fn toggling_bold_reshapes_the_selected_line_bold() {
    let (mut text_system, mut editor) = make_editor(400.0, 200.0);
    editor.set_text("plain words\nsecond");
    editor.flush(&mut text_system);
    editor.apply(SetTextCursor(0));
    editor.apply(ExtendTextSelection(5));
    editor.apply(ToggleStyle(InlineStyle::BOLD));
    editor.flush(&mut text_system);

    let first = editor.window().lines().first().expect("first line");
    let bold: Vec<_> = first
        .layout
        .spans()
        .iter()
        .filter(|s| s.weight == Some(FontWeight::Bold))
        .map(|s| (s.range.start, s.range.end))
        .collect();
    assert_eq!(bold, [(0, 5)]);
}
