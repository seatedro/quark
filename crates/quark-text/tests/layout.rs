use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use quark::scene::FontStyle;
use quark::{FontKind, FontWeight};
use quark_text::{
    FontSettings, LayoutCache, RowHeights, RowMeasure, TextError, TextLayout, TextParams, TextSpan,
    TextStyle, TextSystem,
};
use unicode_segmentation::UnicodeSegmentation;

const LOREM: &str = "The quick brown fox jumps over the lazy dog while the sleepy cat watches \
                     from a sunny windowsill and dreams of mice.";

// Building a FontSystem scans system fonts; do it once for the whole binary.
fn system() -> MutexGuard<'static, TextSystem> {
    static SYSTEM: OnceLock<Mutex<TextSystem>> = OnceLock::new();
    SYSTEM
        .get_or_init(|| Mutex::new(TextSystem::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn layout(text: &str, wrap: Option<f32>) -> TextLayout {
    let params = TextParams::new(text, TextStyle::new(14.0)).wrap_width(wrap);
    system().layout(&params).expect("layout")
}

fn assert_lines_tile_paragraphs(layout: &TextLayout) {
    let text = layout.text();
    let mut expected_start = 0;
    for line in layout.lines() {
        assert_eq!(
            line.byte_range.start, expected_start,
            "line gap in {text:?}"
        );
        expected_start = line.byte_range.end;
        // Skip the paragraph's line ending, if any.
        let rest = &text[expected_start..];
        if rest.starts_with("\r\n") || rest.starts_with("\n\r") {
            expected_start += 2;
        } else if rest.starts_with('\n') || rest.starts_with('\r') {
            expected_start += 1;
        }
    }
    assert_eq!(expected_start, text.len());
}

fn grapheme_boundaries(text: &str) -> Vec<usize> {
    let mut out: Vec<usize> = text.grapheme_indices(true).map(|(i, _)| i).collect();
    out.push(text.len());
    out
}

fn has_real_glyph(layout: &TextLayout, byte: usize) -> bool {
    let g = layout.glyphs();
    (0..g.len()).any(|i| g.byte_start[i] as usize == byte && g.glyph_id[i] != 0)
}

#[test]
fn vendored_ui_font_is_used() {
    let layout = layout("Hello", None);
    let font_id = layout.glyphs().font_id[0];
    let sys = system();
    let face = sys.font_system().db().face(font_id).expect("face");
    assert!(face.families.iter().any(|(name, _)| name == "Geist"));
}

#[test]
fn wraps_more_as_width_shrinks() {
    let unwrapped = layout(LOREM, None);
    assert_eq!(unwrapped.line_count(), 1);
    let mut previous = 1;
    for width in [400.0, 200.0, 100.0] {
        let wrapped = layout(LOREM, Some(width));
        assert!(wrapped.line_count() > previous, "width {width}");
        previous = wrapped.line_count();
        assert_lines_tile_paragraphs(&wrapped);
        let (w, h) = wrapped.size();
        assert!(w <= width + 1.0, "width {w} exceeds wrap {width}");
        let line_height = 14.0 * 1.35;
        assert!((h - line_height * wrapped.line_count() as f32).abs() < 0.5);
    }
}

#[test]
fn hidpi_layout_reports_logical_sizes() {
    let params = TextParams::new(LOREM, TextStyle::new(14.0)).wrap_width(Some(200.0));
    let one = system().layout(&params).expect("1x");
    let two = system()
        .layout(&params.clone().scale_factor(2.0))
        .expect("2x");
    assert_eq!(one.line_count(), two.line_count());
    assert!((one.size().1 - two.size().1).abs() < 0.5);
    assert!((one.size().0 - two.size().0).abs() < 4.0);
}

#[test]
fn multi_paragraph_offsets_are_absolute() {
    let text = "hello\nworld\r\n\nend";
    let layout = layout(text, None);
    assert_eq!(layout.line_count(), 4);
    let ranges: Vec<_> = layout.lines().map(|l| l.byte_range).collect();
    assert_eq!(ranges, vec![0..5, 6..11, 13..13, 14..17]);
    let g = layout.glyphs();
    for i in 0..g.len() {
        let cluster = &text[g.byte_start[i] as usize..g.byte_end[i] as usize];
        assert!(!cluster.contains(['\n', '\r']));
    }
    let w = (0..g.len())
        .find(|&i| &text[g.byte_start[i] as usize..g.byte_end[i] as usize] == "w")
        .expect("w glyph");
    assert_eq!(g.byte_start[w], 6);
    assert_eq!(g.line[w], 1);

    let trailing = self::layout("abc\n", None);
    assert_eq!(trailing.line_count(), 2);
    assert_eq!(trailing.line(1).map(|l| l.byte_range), Some(4..4));
}

#[test]
fn hit_and_caret_round_trip() {
    let text = "Hello world, this wraps.\nSecond paragraph here\n\nfi ligature office";
    let layout = layout(text, Some(90.0));
    assert!(layout.line_count() > 4);
    for b in grapheme_boundaries(text) {
        let caret = layout.caret(b);
        let hit = layout.hit(caret.x, caret.y + caret.height * 0.5);
        assert_eq!(hit, b, "byte {b} caret {caret:?}");
    }
}

#[test]
fn hit_clamps_outside_bounds() {
    let text = "one\ntwo";
    let layout = layout(text, None);
    assert_eq!(layout.hit(-50.0, -50.0), 0);
    assert_eq!(layout.hit(1000.0, -50.0), 3);
    assert_eq!(layout.hit(-50.0, 1000.0), 4);
    assert_eq!(layout.hit(1000.0, 1000.0), text.len());
    assert_eq!(layout.caret(999).line, 1);
}

#[test]
fn caret_snaps_inside_grapheme() {
    let text = "e\u{301}x";
    let layout = layout(text, None);
    assert_eq!(layout.caret(2), layout.caret(0));
}

#[test]
fn selection_rects_span_lines() {
    let layout = layout(LOREM, Some(120.0));
    assert!(layout.line_count() >= 4);
    let line0 = layout.line(0).expect("line 0");
    let line2 = layout.line(2).expect("line 2");
    let a = line0.byte_range.start + 2;
    let b = line2.byte_range.start + 3;
    let rects: Vec<_> = layout.selection_rects(a..b).collect();
    assert_eq!(rects.len(), 3, "{rects:?}");
    assert!(rects.windows(2).all(|w| w[0].y < w[1].y));
    let start = layout.caret(a);
    let end = layout.caret(b);
    assert!((rects[0].x - start.x).abs() < 0.01);
    assert!((rects[2].x + rects[2].width - end.x).abs() < 0.01);
    assert!((rects[1].x).abs() < 0.01);
    assert!(rects.iter().all(|r| r.width > 0.0));
    assert_eq!(layout.selection_rects(5..5).count(), 0);
    // Reversed ranges are normalized.
    assert_eq!(layout.selection_rects(b..a).count(), 3);
}

#[test]
fn selection_marks_empty_lines() {
    let layout = layout("a\n\nb", None);
    let rects: Vec<_> = layout.selection_rects(0..4).collect();
    assert_eq!(rects.len(), 3);
}

#[test]
fn glyph_runs_split_by_span() {
    let text = "plain bold plain";
    let spans = vec![TextSpan {
        range: 6..10,
        weight: Some(FontWeight::Bold),
        style: Some(FontStyle::Italic),
        kind: None,
    }];
    let params = TextParams::new(text, TextStyle::new(14.0)).spans(spans);
    let layout = system().layout(&params).expect("layout");
    let runs: Vec<_> = layout.glyph_runs().collect();
    assert_eq!(
        runs.iter().map(|r| r.span).collect::<Vec<_>>(),
        vec![0, 1, 0]
    );
    let g = layout.glyphs();
    let bold = &runs[1];
    assert_eq!(g.byte_start[bold.glyphs.start], 6);
    assert!(
        layout
            .physical_glyph(bold.glyphs.start, (10.5, 20.0))
            .is_some()
    );
    assert!(layout.physical_glyph(g.len(), (0.0, 0.0)).is_none());
}

#[test]
fn emoji_and_cjk_fall_back() {
    let text = "a\u{4e2d}\u{1f600}";
    let layout = layout(text, None);
    let g = layout.glyphs();
    let latin_font = g.font_id[0];
    let mut checked = 0;
    for byte in [1, 4] {
        if !has_real_glyph(&layout, byte) {
            eprintln!("skipping fallback check for byte {byte}: no system font covers it");
            continue;
        }
        let i = (0..g.len())
            .find(|&i| g.byte_start[i] as usize == byte)
            .expect("glyph");
        assert_ne!(
            g.font_id[i], latin_font,
            "byte {byte} should use a fallback font"
        );
        checked += 1;
    }
    // Mono used Basic shaping before; it must fall back too.
    let params = TextParams::new(text, TextStyle::new(14.0).kind(FontKind::Mono));
    let mono = system().layout(&params).expect("mono");
    for byte in [1, 4] {
        if has_real_glyph(&layout, byte) {
            assert!(has_real_glyph(&mono, byte), "mono byte {byte}");
        }
    }
    eprintln!("fallback checks run: {checked}");
}

#[test]
fn rtl_paragraph_round_trips() {
    let text = "\u{5e9}\u{5dc}\u{5d5}\u{5dd} \u{5e2}\u{5d5}\u{5dc}\u{5dd}";
    let layout = layout(text, None);
    if !has_real_glyph(&layout, 0) {
        eprintln!("skipping RTL test: no Hebrew font");
        return;
    }
    assert!(layout.line(0).expect("line").rtl);
    assert!(layout.caret(0).x > layout.caret(text.len()).x);
    for b in grapheme_boundaries(text) {
        let caret = layout.caret(b);
        assert_eq!(layout.hit(caret.x, caret.y + 1.0), b, "byte {b}");
    }

    // Mixed LTR paragraph with an RTL run: carets stay monotonic within each
    // run and selecting the RTL word yields one rect covering its glyphs.
    let mixed = "ab \u{5e9}\u{5dc}\u{5d5}\u{5dd} cd";
    let layout = self::layout(mixed, None);
    assert!(!layout.line(0).expect("line").rtl);
    assert!(layout.caret(3).x > layout.caret(5).x);
    let rects: Vec<_> = layout.selection_rects(3..11).collect();
    assert_eq!(rects.len(), 1, "{rects:?}");
    for b in [0, 1, 2, 12, 13, 14] {
        let caret = layout.caret(b);
        assert_eq!(layout.hit(caret.x, caret.y + 1.0), b, "mixed byte {b}");
    }
}

#[test]
fn invalid_params_error() {
    let mut sys = system();
    let bad_size = TextParams::new("x", TextStyle::new(0.0));
    assert_eq!(
        sys.layout(&bad_size).err(),
        Some(TextError::InvalidFontSize(0.0))
    );
    let bad_span = TextParams::new("x", TextStyle::new(12.0)).spans(vec![TextSpan {
        range: 0..5,
        weight: None,
        style: None,
        kind: None,
    }]);
    assert!(matches!(
        sys.layout(&bad_span),
        Err(TextError::InvalidSpan { index: 0, .. })
    ));
}

#[test]
fn cache_hits_and_trims() {
    let mut sys = system();
    let mut cache = LayoutCache::new(2);
    let text: Arc<str> = Arc::from(LOREM);
    let params = TextParams::new(text.clone(), TextStyle::new(14.0)).wrap_width(Some(150.0));
    let other = params.clone().wrap_width(Some(300.0));

    cache.begin_frame();
    let first = cache.layout(&mut sys, &params).expect("layout");
    let again = cache.layout(&mut sys, &params).expect("layout");
    assert!(Arc::ptr_eq(&first, &again));
    // Equal content in a different Arc still hits.
    let copy = TextParams::new(LOREM, TextStyle::new(14.0)).wrap_width(Some(150.0));
    assert!(Arc::ptr_eq(
        &first,
        &cache.layout(&mut sys, &copy).expect("layout")
    ));
    let wide = cache.layout(&mut sys, &other).expect("layout");
    assert!(!Arc::ptr_eq(&first, &wide));
    assert_eq!(cache.len(), 2);

    // Keep `params` alive, let `other` go idle.
    for _ in 0..3 {
        cache.begin_frame();
        cache.layout(&mut sys, &params).expect("layout");
    }
    assert_eq!(cache.trim(), 1);
    assert_eq!(cache.len(), 1);
    assert!(Arc::ptr_eq(
        &first,
        &cache.layout(&mut sys, &params).expect("layout")
    ));

    // Font changes invalidate.
    let original = sys.font_settings().clone();
    sys.set_font_settings(&FontSettings {
        ui_family: "Inter".into(),
        ..original.clone()
    });
    let after = cache.layout(&mut sys, &params).expect("layout");
    assert!(!Arc::ptr_eq(&first, &after));
    sys.set_font_settings(&original);
}

#[test]
fn row_heights_memoize_per_width() {
    let mut calls = 0;
    let mut rows = RowHeights::new(|key: u64, width: f32| {
        calls += 1;
        key as f32 + width
    });
    assert_eq!(rows.measure(1, 10.0), 11.0);
    assert_eq!(rows.measure(1, 10.0), 11.0);
    assert_eq!(rows.measure(2, 10.0), 12.0);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows.measure(1, 20.0), 21.0);
    assert_eq!(rows.len(), 1);
    rows.invalidate(1);
    assert_eq!(rows.measure(1, 20.0), 21.0);
    drop(rows);
    assert_eq!(calls, 4);
}
