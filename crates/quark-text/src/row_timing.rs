//! Release timing of laying out terminal rows from text no layout has seen,
//! split into what cosmic-text and quark-text each spend. Run with
//! `cargo test --release -p quark-text --lib -- --ignored --nocapture
//! report_row_layout_timing`. Set `QUARK_ROW_TIMING_LOOPS` to repeat the
//! whole-frame loop that many times more, for a sampling profiler.

use std::time::{Duration, Instant};

use cosmic_text::fallback::FontFallbackIter;
use cosmic_text::{Attrs, AttrsList, BufferLine, Family, LineEnding, Shaping, Weight, Wrap};
use quark::FontKind;

use crate::block::TextBlock;
use crate::fonts::FontSettings;
use crate::layout::TextStyle;
use crate::system::TextSystem;

/// Rows of the terminal demo's 80x22 screen.
const ROWS: usize = 22;
const FRAMES: usize = 201;

/// The text runs of output line `n` printed with seed `seed`, as the
/// terminal lays them out: a green line number, then the rest of the row
/// in the default style up to its trimmed trailing blanks.
fn row_runs(seed: usize, n: usize) -> [String; 2] {
    [
        format!("{n:04}"),
        format!(" output {seed}.{n} of a build step"),
    ]
}

fn style() -> TextStyle {
    TextStyle::new(13.0).kind(FontKind::Mono).line_height(18.0)
}

fn median(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[v.len() / 2]
}

/// cosmic-text's words of `text`: the pieces between line break
/// opportunities without their trailing whitespace, and each whitespace
/// char alone, as `ShapeSpan::build` splits them.
fn words(text: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = 0;
    for (end, _) in unicode_linebreak::linebreaks(text) {
        let piece = text.get(start..end).unwrap_or_default();
        let trimmed = piece.trim_end();
        if !trimmed.is_empty() {
            words.push(trimmed);
        }
        let blank = piece.get(trimmed.len()..).unwrap_or_default();
        words.extend(
            blank
                .char_indices()
                .filter_map(|(i, c)| blank.get(i..i + c.len_utf8())),
        );
        start = end;
    }
    words
}

#[test]
#[ignore = "measurement, prints a report"]
fn report_row_layout_timing() {
    let mut system = TextSystem::vendored_only(&FontSettings::default());
    let style = style();
    let loops: usize = std::env::var("QUARK_ROW_TIMING_LOOPS")
        .ok()
        .and_then(|n| n.parse().ok())
        .unwrap_or(0);

    // Whole rows through TextBlock, as the terminal lays them out: every
    // frame shows text no frame showed before, and the scene holds the last
    // frame's layouts while the next is laid out.
    let mut blocks: Vec<[TextBlock; 2]> = (0..ROWS).map(|_| Default::default()).collect();
    let mut held = Vec::new();
    let mut frames = Vec::new();
    for frame in 0..FRAMES * (1 + loops) {
        let texts: Vec<_> = (0..ROWS).map(|n| row_runs(frame, n)).collect();
        let mut shown = Vec::with_capacity(ROWS * 2);
        let started = Instant::now();
        for (row, runs) in blocks.iter_mut().zip(&texts) {
            for (block, text) in row.iter_mut().zip(runs) {
                block.set_text(text);
                shown.push(block.layout(&mut system, style, None, 1.0).expect("layout"));
            }
        }
        frames.push(started.elapsed());
        held = shown;
    }
    drop(held);
    let frame = median(frames);

    // The typed character: one short run changes.
    let mut prompt = TextBlock::new();
    let mut typed = Vec::new();
    let mut shown = None;
    for i in 0..FRAMES {
        let text = format!("$ ech{}", char::from(b'a' + (i % 26) as u8));
        let started = Instant::now();
        prompt.set_text(&text);
        let layout = prompt
            .layout(&mut system, style, None, 1.0)
            .expect("layout");
        typed.push(started.elapsed());
        shown = Some(layout);
    }
    drop(shown);

    // The same runs through cosmic-text alone: shaping, then line layout.
    let fs = system.raster_font_system();
    let attrs = Attrs::new()
        .family(Family::Monospace)
        .weight(Weight(crate::layout::weight_value(
            style.font_kind,
            style.font_weight,
        )));
    let mut lines: Vec<BufferLine> = (0..ROWS * 2)
        .map(|_| {
            BufferLine::new(
                "",
                LineEnding::None,
                AttrsList::new(&attrs),
                Shaping::Advanced,
            )
        })
        .collect();
    let (mut shape, mut layout) = (Vec::new(), Vec::new());
    for frame in 0..FRAMES {
        let texts: Vec<_> = (0..ROWS).flat_map(|n| row_runs(frame, n)).collect();
        for (line, text) in lines.iter_mut().zip(&texts) {
            line.set_text(text, LineEnding::None, AttrsList::new(&attrs));
        }
        let started = Instant::now();
        for line in &mut lines {
            line.shape(fs, 8);
        }
        shape.push(started.elapsed());
        let started = Instant::now();
        for line in &mut lines {
            line.layout(fs, 13.0, None, Wrap::WordOrGlyph, None, 8);
        }
        layout.push(started.elapsed());
    }

    // Pieces of shaping, over one frame's words.
    let texts: Vec<_> = (0..ROWS).flat_map(|n| row_runs(FRAMES + 1, n)).collect();
    let all_words: Vec<&str> = texts.iter().flat_map(|t| words(t)).collect();
    let blank = all_words.iter().filter(|w| w.trim().is_empty()).count();
    let (mut breaks, mut matches, mut fallback) = (Vec::new(), Vec::new(), Vec::new());
    let family = [&attrs.family];
    for _ in 0..FRAMES {
        let started = Instant::now();
        let n: usize = (texts.iter())
            .map(|t| unicode_linebreak::linebreaks(t).count())
            .sum();
        breaks.push(started.elapsed());
        assert!(n > 0);

        let started = Instant::now();
        for _ in &all_words {
            std::hint::black_box(fs.get_font_matches(&attrs));
        }
        matches.push(started.elapsed());

        let fonts = fs.get_font_matches(&attrs);
        let started = Instant::now();
        for word in &all_words {
            let mut iter = FontFallbackIter::new(fs, &fonts, &family, &[], word, attrs.weight);
            std::hint::black_box(iter.next());
        }
        fallback.push(started.elapsed());
    }

    let per_row = |d: Duration| d / ROWS as u32;
    let block = |name: &str, d: Duration| {
        eprintln!("  {name}: {:?} per frame, {:?} per row", d, per_row(d));
    };
    eprintln!(
        "{ROWS} fresh rows, {} words ({blank} of them one blank char)",
        all_words.len()
    );
    block("TextBlock::layout (all)", frame);
    block("cosmic shape", median(shape));
    block("cosmic line layout", median(layout));
    block("  of shape: line breaks", median(breaks));
    block("  of shape: get_font_matches per word", median(matches));
    block("  of shape: fallback iter per word", median(fallback));
    eprintln!("typed char run: {:?}", median(typed));
}
