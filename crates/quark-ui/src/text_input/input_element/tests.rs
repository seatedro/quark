use super::*;
use crate::Action;
use crate::text_input::TextEditCommand::*;
use crate::text_input::{SyntaxTokenKind, TextOffset, caret_popup};
use crate::theme::Theme;
use quark::reactive::SignalStore;
use quark_render::Primitive;
use quark_text::TextLayout;

#[test]
fn caret_and_selection_sit_on_the_painted_glyphs_after_bold_tokens() {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::new(EditorMode::CodeInput);
    // Keywords are painted semibold, which used to shift the caret.
    editor.set_syntax_highlighter(Arc::new(|text: &str| {
        text.match_indices("fn")
            .map(|(i, _)| SyntaxSpan {
                offset: i as u32,
                length: 2,
                kind: SyntaxTokenKind::Keyword,
            })
            .collect()
    }));
    editor.sync_size(400.0, 200.0);
    editor.set_text("fn main() { fn inner() {} }\nlet s = \"日本\";");
    editor.apply(SetTextCursor(5));
    editor.apply(ExtendTextSelection(editor.byte_len() - 2));
    editor.flush(&mut text);

    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    let target = FocusId::from_key("test.editor");
    let mut root = text_editor_element(target, ScrollActionBuilder::new(Action::new))
        .editor_snapshot(&editor)
        .focused(true)
        .w(400.0)
        .h(200.0)
        .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 200.0);

    // Each painted line, with where it starts in the text: the lines are
    // painted top to bottom, one per logical line.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(editor.text().match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let painted: Vec<(f32, &TextLayout)> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::RichTextRun(run) => {
                Some((run.rect.y, run.layout.downcast_ref::<TextLayout>()?))
            }
            _ => None,
        })
        .collect();
    assert_eq!(painted.len(), line_starts.len(), "one run per line");
    let (cursor, anchor) = (editor.cursor().get(), editor.anchor().get());
    let (top, layout) = painted[1];
    let caret = layout.caret(cursor - line_starts[1]);
    assert_eq!(
        (caret.x, top + caret.y),
        (editor.cursor_pos.x, editor.cursor_pos.y)
    );
    let painted_rects: Vec<_> = painted
        .iter()
        .zip(&line_starts)
        .flat_map(|(&(top, layout), &start)| {
            let end = start + layout.text().len();
            let local = anchor.clamp(start, end) - start..cursor.clamp(start, end) - start;
            layout
                .selection_rects(local)
                .map(move |r| (r.x, top + r.y, r.width, r.height))
        })
        .collect();
    let editor_rects: Vec<_> = editor
        .selection_rects()
        .iter()
        .map(|r| (r.x, r.y, r.w, r.h))
        .collect();
    assert_eq!(editor_rects, painted_rects);
}

// Regression: apps had to call Editor::set_scale_factor themselves, and
// an editor that never did stayed shaped at 1x on HiDPI screens.
#[test]
fn painting_at_a_scale_reshapes_the_editor_at_it_on_the_next_flush() {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::new(EditorMode::ProseInput);
    editor.sync_size(200.0, 100.0);
    editor.set_text("hello");
    editor.flush(&mut text);
    let shaped_at = |editor: &Editor| editor.layout().map(|l| l.scale_factor());
    let before = shaped_at(&editor);

    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 2.0, &mut text, &mut layouts, None, &signals);
    let mut root = text_editor_element(
        FocusId::from_key("e"),
        ScrollActionBuilder::new(Action::new),
    )
    .editor_snapshot(&editor)
    .w(200.0)
    .h(100.0)
    .into_any();
    render_element(&mut root, &mut Scene::default(), &mut cx, 200.0, 100.0);
    let repaint_at = cx.next_frame_ms();
    editor.flush(&mut text);

    assert_eq!((before, shaped_at(&editor)), (Some(1.0), Some(2.0)));
    assert_eq!(repaint_at, Some(0), "a repaint picks up the new layout");
}

// Regression: code mode wrapped the editor's layout at the full width,
// so lines ran on under the clip past the text area beside the gutter.
#[test]
fn code_editor_wraps_its_text_within_the_area_beside_the_gutter() {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::new(EditorMode::CodeInput);
    editor.sync_size(200.0, 100.0);
    // Lines from about 0.6 to 1.0 of the full width: some fit the whole
    // view but not the part beside the gutter.
    let lines: Vec<String> = (14..=24).map(|n| "x".repeat(n)).collect();
    editor.set_text(&lines.join("\n"));
    editor.flush(&mut text);

    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    let mut root = text_editor_element(
        FocusId::from_key("e"),
        ScrollActionBuilder::new(Action::new),
    )
    .editor_snapshot(&editor)
    .w(200.0)
    .h(100.0)
    .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 200.0, 100.0);

    let (area, layout) = scene
        .primitives
        .iter()
        .find_map(|p| match p {
            Primitive::RichTextRun(run) => {
                Some((run.rect, run.layout.downcast_ref::<TextLayout>()?))
            }
            _ => None,
        })
        .expect("painted editor text");
    assert!(area.x > 0.0, "a gutter is painted");
    let widest = layout.lines().map(|l| l.width).fold(0.0, f32::max);
    assert!(
        widest <= area.width + 0.5,
        "line {widest} wider than {}",
        area.width
    );
}

// Catches atoms painting without their pill, or the pill drifting off
// the label's glyphs.
#[test]
fn an_atom_paints_a_pill_spanning_its_label() {
    use crate::text_input::{AtomId, InlineAtom, RichText};
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::new(EditorMode::ProseInput);
    editor.sync_size(300.0, 60.0);
    let rich = RichText {
        text: "see @main.rs now".into(),
        atoms: vec![InlineAtom {
            range: 4..12,
            id: AtomId::new(1, 0),
            export: "[main.rs](main.rs)".into(),
        }],
        styles: Vec::new(),
    };
    editor.set_rich_text(&rich);
    editor.flush(&mut text);

    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    let mut root = text_editor_element(
        FocusId::from_key("e"),
        ScrollActionBuilder::new(Action::new),
    )
    .editor_snapshot(&editor)
    .w(300.0)
    .h(60.0)
    .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 300.0, 60.0);

    let layout = editor.layout().expect("layout");
    let (start, end) = (
        layout.caret(TextOffset::snap(editor.text(), 4)).x,
        layout.caret(TextOffset::snap(editor.text(), 12)).x,
    );
    let pills: Vec<_> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::RoundedRect(r) => Some(r.rect),
            _ => None,
        })
        .filter(|r| r.x <= start && r.x + r.width >= end && r.width < end - start + 8.0)
        .collect();
    assert_eq!(pills.len(), 1, "start {start} end {end}");
}

// Catches formatting that reaches the model but not the screen: an
// underlined run must paint a line spanning exactly its glyphs, below
// their middle.
#[test]
fn an_underlined_run_paints_a_line_under_its_glyphs() {
    use crate::text_input::{InlineStyle, RichText, StyleSpan};
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::new(EditorMode::ProseInput);
    editor.sync_size(300.0, 60.0);
    editor.set_rich_text(&RichText {
        text: "see under now".into(),
        styles: vec![StyleSpan::new(4..9, InlineStyle::UNDERLINE)],
        ..RichText::default()
    });
    editor.flush(&mut text);

    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    let mut root = text_editor_element(
        FocusId::from_key("e"),
        ScrollActionBuilder::new(Action::new),
    )
    .editor_snapshot(&editor)
    .w(300.0)
    .h(60.0)
    .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 300.0, 60.0);

    let layout = editor.layout().expect("layout");
    let at = |o| layout.caret(TextOffset::snap(editor.text(), o));
    let (start, end) = (at(4), at(9));
    let lines: Vec<_> = scene
        .primitives
        .iter()
        .filter_map(|p| match p {
            Primitive::Rect(r) => Some(r.rect),
            _ => None,
        })
        .filter(|r| (r.x - start.x).abs() < 0.5 && (r.x + r.width - end.x).abs() < 0.5)
        .collect();
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].y > start.y + 14.0 * 1.35 * 0.5, "{:?}", lines[0]);
}

const POPUP: crate::theme::Color = crate::theme::Color::rgba(1, 2, 3, 255);

/// A 400x300 window with an editor at (20, 240) holding two lines, and
/// an 80pt popup at its caret before or after it in the tree. Returns
/// the painted popup and editor text rects, and the frame asked for.
fn render_caret_popup(
    editor: &Editor,
    text: &mut quark_text::TextSystem,
    popup_first: bool,
) -> (Option<Rect>, Option<Rect>, Option<u64>) {
    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 1.0, text, &mut layouts, None, &signals);
    let field = text_editor_element(
        FocusId::from_key("e"),
        ScrollActionBuilder::new(Action::new),
    )
    .editor_snapshot(editor)
    .w(300.0)
    .h(44.0)
    .into_any();
    let popup = caret_popup(
        editor.caret_anchor(),
        div().w(120.0).h(80.0).bg(POPUP),
        (400.0, 300.0),
    )
    .into_any();
    let children = if popup_first {
        [popup, field]
    } else {
        [field, popup]
    };
    let mut root = div()
        .w(400.0)
        .h(300.0)
        .pt(240.0)
        .pl(20.0)
        .children(children)
        .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
    let popup = scene.primitives.iter().find_map(|p| match p {
        Primitive::RoundedRect(r) if r.color == POPUP => Some(r.rect),
        Primitive::Rect(r) if r.color == POPUP => Some(r.rect),
        _ => None,
    });
    let draft = scene.primitives.iter().find_map(|p| match p {
        Primitive::RichTextRun(run) => Some(run.rect),
        _ => None,
    });
    (popup, draft, cx.next_frame_ms())
}

fn two_line_editor(text: &mut quark_text::TextSystem) -> Editor {
    let mut editor = Editor::new(EditorMode::ProseInput);
    editor.sync_size(300.0, 44.0);
    editor.set_text("first\nsecond @ma");
    editor.flush(text);
    editor
}

// Catches a completion popup that is not placed against the caret
// painted this frame, or does not flip above a composer at the bottom.
#[test]
fn a_caret_popup_after_the_editor_sits_above_its_caret_in_the_same_frame() {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let editor = two_line_editor(&mut text);
    let (popup, _, next_frame) = render_caret_popup(&editor, &mut text, false);

    let caret = (20.0 + editor.cursor_pos.x, 240.0 + editor.cursor_pos.y);
    let popup = popup.expect("popup painted");
    let bottom = popup.y + popup.height + 4.0;
    assert!(
        (popup.x - caret.0).abs() < 0.01 && (bottom - caret.1).abs() < 0.01,
        "popup {popup:?} caret {caret:?}"
    );
    assert_eq!(next_frame, None, "no second frame needed");
}

// Catches a popup earlier in the tree than its editor staying where
// last frame's caret was.
#[test]
fn a_caret_popup_before_the_editor_reaches_the_caret_on_the_next_frame() {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let editor = two_line_editor(&mut text);

    let (first, _, next_frame) = render_caret_popup(&editor, &mut text, true);
    let (second, _, _) = render_caret_popup(&editor, &mut text, true);

    assert_eq!(first, None, "nothing to anchor to yet");
    assert_eq!(next_frame, Some(0));
    let popup = second.expect("popup painted");
    let caret_y = 240.0 + editor.cursor_pos.y;
    assert!((popup.y + popup.height + 4.0 - caret_y).abs() < 0.01);
}

// Catches suggestions taking space in the composer's layout, which
// pushed the draft down while they were open.
#[test]
fn opening_a_caret_popup_leaves_the_draft_in_place() {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let editor = two_line_editor(&mut text);
    let (_, with_popup, _) = render_caret_popup(&editor, &mut text, false);

    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    let field = text_editor_element(
        FocusId::from_key("e"),
        ScrollActionBuilder::new(Action::new),
    )
    .editor_snapshot(&editor)
    .w(300.0)
    .h(44.0);
    let mut root = div()
        .w(400.0)
        .h(300.0)
        .pt(240.0)
        .pl(20.0)
        .child(field)
        .into_any();
    let mut scene = Scene::default();
    render_element(&mut root, &mut scene, &mut cx, 400.0, 300.0);
    let alone = scene.primitives.iter().find_map(|p| match p {
        Primitive::RichTextRun(run) => Some(run.rect),
        _ => None,
    });

    assert!(alone.is_some());
    assert_eq!(with_popup, alone);
}

// Catches the element painting the caret at its own 1.35em line height
// instead of the editor's, so it no longer spans the line it sits on.
#[test]
fn the_painted_caret_spans_the_editors_line_height() {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::new(EditorMode::ProseInput);
    editor.set_line_height(Some(22.0));
    editor.sync_size(300.0, 60.0);
    editor.set_text("first\nsecond");
    editor.flush(&mut text);

    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut cx = ElementContext::new(&theme, 1.0, &mut text, &mut layouts, None, &signals);
    let mut root = text_editor_element(
        FocusId::from_key("e"),
        ScrollActionBuilder::new(Action::new),
    )
    .editor_snapshot(&editor)
    .focused(true)
    .w(300.0)
    .h(60.0)
    .into_any();
    render_element(&mut root, &mut Scene::default(), &mut cx, 300.0, 60.0);

    let caret = cx.text_input_hit_areas[0].caret.expect("caret");
    assert_eq!(
        (caret.y, caret.y + caret.height),
        (22.0 + 1.0, 44.0 + 1.0 - Sz::CURSOR_WIDTH)
    );
}

/// Allocations of one steady frame (snapshot and paint) of an editor of
/// `lines` lines, scrolled to its middle, after two warm-up frames.
fn steady_frame_allocations(lines: usize) -> u64 {
    let mut text = quark_text::TextSystem::vendored_only(&Default::default());
    let mut editor = Editor::new(EditorMode::ProseInput);
    editor.sync_size(400.0, 300.0);
    let body: String = (0..lines)
        .map(|i| format!("line {i} of some prose\n"))
        .collect();
    editor.set_text(&body);
    editor.apply(SetTextCursor(body.len() / 2));
    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut layouts = quark_text::LayoutCache::default();
    let mut frame = |editor: &mut Editor, text: &mut quark_text::TextSystem| {
        editor.flush(text);
        let mut cx = ElementContext::new(&theme, 1.0, text, &mut layouts, None, &signals);
        let mut root = text_editor_element(
            FocusId::from_key("e"),
            ScrollActionBuilder::new(Action::new),
        )
        .editor_snapshot(editor)
        .focused(true)
        .w(400.0)
        .h(300.0)
        .into_any();
        render_element(&mut root, &mut Scene::default(), &mut cx, 400.0, 300.0);
    };
    frame(&mut editor, &mut text);
    frame(&mut editor, &mut text);
    crate::test_alloc::count(|| frame(&mut editor, &mut text)).1
}

// Catches painting that grows with the document (copying its text,
// walking its lines, or painting lines out of view): a frame of 40,000
// lines allocates what a frame of 200 does.
#[test]
fn a_steady_frame_costs_the_same_for_a_long_document_as_a_short_one() {
    let short = steady_frame_allocations(200);
    let long = steady_frame_allocations(40_000);
    assert!(long <= short + 4, "short {short}, long {long}");
}
