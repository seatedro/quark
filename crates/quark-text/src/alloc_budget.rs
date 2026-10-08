//! Allocation budgets for laying out text the cache has not seen. Storage
//! is warmed with different text of similar size, then unseen text is
//! measured, so the budgets cover fresh content within warmed capacity
//! rather than cache hits.

use quark::scene::FontStyle;
use quark::{FontKind, FontWeight};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use crate::cache::LayoutCache;
use crate::fonts::FontSettings;
use crate::layout::{TextQuery, TextSpan, TextStyle};
use crate::system::{TextSystem, test_system};

struct Counting;

thread_local! {
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    /// Set while `profile` runs; cleared inside the hook so the hook's own
    /// allocations are not attributed.
    static PROFILING: Cell<bool> = const { Cell::new(false) };
    static SITES: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

fn record() {
    let _ = ALLOCATIONS.try_with(|n| n.set(n.get() + 1));
    if PROFILING.try_with(|p| p.replace(false)) != Ok(true) {
        return;
    }
    let site = call_site(&std::backtrace::Backtrace::force_capture().to_string());
    SITES.with(|sites| *sites.borrow_mut().entry(site).or_default() += 1);
    PROFILING.with(|p| p.set(true));
}

/// The innermost frames of a backtrace inside text crates, so dependency
/// allocations name their source rather than the nearest quark caller.
fn call_site(trace: &str) -> String {
    const CRATES: [&str; 5] = [
        "quark_text",
        "cosmic_text",
        "unicode_bidi",
        "harfrust",
        "rangemap",
    ];
    let frames: Vec<&str> = trace
        .lines()
        .map(str::trim)
        .filter(|line| CRATES.iter().any(|name| line.contains(name)))
        .filter(|line| !line.starts_with("at ") && !line.contains("alloc_budget"))
        .take(3)
        .collect();
    frames.join(" < ")
}

// SAFETY: forwards to the system allocator unchanged; counting touches only
// a thread local.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record();
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record();
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Allocations `f` makes on this thread.
fn count<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = ALLOCATIONS.with(Cell::get);
    let result = f();
    (result, ALLOCATIONS.with(Cell::get) - before)
}

/// Run `f` and return its allocations by call site, most first. Slow:
/// every allocation captures a backtrace. For diagnosing a budget failure.
#[allow(dead_code)]
fn profile<R>(f: impl FnOnce() -> R) -> (R, Vec<(String, u64)>) {
    SITES.with(|sites| sites.borrow_mut().clear());
    PROFILING.with(|p| p.set(true));
    let result = f();
    PROFILING.with(|p| p.set(false));
    let mut sites: Vec<_> = SITES.with(|sites| sites.borrow_mut().drain().collect());
    sites.sort_by_key(|site| std::cmp::Reverse(site.1));
    (result, sites)
}

/// Warms cosmic-text's per-codepoint font lookups for the ASCII inputs.
const PANGRAM: &str = "the quick brown fox jumps over the lazy dog";

/// Owned text and spans for one query, built outside measured closures.
struct Input {
    text: String,
    spans: Vec<TextSpan>,
    style: TextStyle,
    wrap: Option<f32>,
}

impl Input {
    fn query(&self) -> TextQuery<'_> {
        TextQuery {
            text: &self.text,
            spans: &self.spans,
            ..TextQuery::new(&self.text, self.style).wrap_width(self.wrap)
        }
    }
}

fn ui(text: String) -> Input {
    Input {
        text,
        spans: Vec::new(),
        style: TextStyle::new(14.0),
        wrap: None,
    }
}

fn mono(text: String) -> Input {
    Input {
        style: TextStyle::new(13.0).kind(FontKind::Mono),
        ..ui(text)
    }
}

/// An 80-column terminal row with a bold and an italic span.
fn styled_row(seed: usize) -> Input {
    let text: String = (0..80)
        .map(|i| char::from(b'a' + ((i * 7 + seed * 13) % 26) as u8))
        .collect();
    let span = |range, weight, style| TextSpan {
        range,
        weight,
        style,
        kind: None,
    };
    Input {
        spans: vec![
            span(4..20, Some(FontWeight::Bold), None),
            span(40..60, None, Some(FontStyle::Italic)),
        ],
        ..mono(text)
    }
}

fn plain_row(seed: usize) -> Input {
    let text = (0..80)
        .map(|i| char::from(b'!' + ((i * 11 + seed * 17) % 90) as u8))
        .collect();
    mono(text)
}

/// Ten seven-letter words: rows whose words all shape alike, so what
/// cosmic-text allocates for each is the same.
fn word_row(seed: usize) -> Input {
    let text = (0..10)
        .map(|w| {
            (0..7)
                .map(|i| char::from(b'a' + ((seed * 31 + w * 7 + i * 3) % 26) as u8))
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join(" ");
    mono(text)
}

fn bidi_emoji(seed: usize) -> Input {
    let words = ["alpha", "bravo", "charlie", "delta", "echo"];
    let word = |i: usize| words[(seed + i) % words.len()];
    let text = format!(
        "{} \u{5e9}\u{5dc}\u{5d5}\u{5dd} {} \u{1f600} \u{627}\u{644}\u{639} {} \
         \u{1f469}\u{200d}\u{1f4bb} {}",
        word(0),
        word(1),
        word(2),
        word(3),
    );
    Input {
        wrap: Some(160.0),
        ..ui(text)
    }
}

/// Lays out `warm` twice over (so capacity the first pass grew is in
/// place), evicting it after each pass.
fn warmed_cache(system: &mut TextSystem, warm: &[Input]) -> LayoutCache {
    let mut cache = LayoutCache::new(0);
    for _ in 0..2 {
        cache.begin_frame();
        for input in warm {
            cache.layout_query(system, &input.query()).expect("warm");
        }
        cache.begin_frame();
        cache.trim();
    }
    cache
}

/// Lays out each case's measured inputs in a cache warmed with its warm
/// inputs, failing when that allocates more than the case's budget.
fn assert_budgets(system: &mut TextSystem, cases: Vec<(&str, Vec<Input>, Vec<Input>, u64)>) {
    for (name, warm, measured, budget) in cases {
        let mut cache = warmed_cache(system, &warm);
        let mut layouts = Vec::with_capacity(measured.len());
        let ((), allocations) = count(|| {
            for input in &measured {
                layouts.push(cache.layout_query(system, &input.query()).expect("layout"));
            }
        });
        assert!(allocations <= budget, "{name}: {allocations} > {budget}");
        for (layout, input) in layouts.iter().zip(&measured) {
            assert_eq!(layout.text().as_ref(), input.text, "{name}");
            assert_eq!(layout.verify_integrity(), Ok(()), "{name}");
        }
    }
}

// What laying out unseen text still allocates is all inside cosmic-text and
// its dependencies: unicode-bidi's paragraph analysis (five vectors per
// paragraph), cosmic-text's attribute span maps, and its per-word glyph
// vectors (which word gets which retained vector varies, so one can grow).
// The budgets sit just above that, so storage quark-text allocates or grows
// per layout (each one costs at least 14 glyph columns) breaks them.
#[test]
fn fresh_text_within_warmed_capacity_allocates_only_inside_shaping() {
    let cases = vec![
        (
            "short ascii line",
            vec![ui(PANGRAM.into()), ui("jumpy otter".into())],
            vec![ui("brisk eagle".into())],
            6,
        ),
        (
            "80-column styled row",
            vec![styled_row(1), styled_row(2)],
            vec![styled_row(3)],
            6,
        ),
        (
            "30 fresh rows",
            (0..30).map(plain_row).collect(),
            (30..60).map(plain_row).collect(),
            30 * 6,
        ),
        (
            "bidi and emoji paragraph",
            vec![bidi_emoji(1)],
            vec![bidi_emoji(2)],
            // No vendored font has Hebrew or Arabic, so fallback shapes
            // those runs with every font: 69 shape plans, which must all
            // stay cached, and a glyph vector and missing-glyph list per
            // font tried, which must be reused.
            22,
        ),
    ];
    assert_budgets(&mut test_system(), cases);
}

// With ligatures off every attribute set carries a feature vector, so
// copies of it per glyph, word, or span would allocate. What is left is
// one copy per attribute span (and a few per emoji cluster) on top of the
// residual above.
#[test]
fn fresh_text_with_ligatures_off_copies_features_per_span_only() {
    let mut system = TextSystem::vendored_only(&FontSettings {
        ligatures: false,
        ..FontSettings::default()
    });
    let cases = vec![
        (
            "80-column styled row",
            vec![styled_row(1), styled_row(2)],
            vec![styled_row(3)],
            6 + 3,
        ),
        (
            "row of words",
            vec![word_row(1), word_row(2)],
            vec![word_row(3)],
            5 + 1,
        ),
        (
            "bidi and emoji paragraph",
            vec![bidi_emoji(1)],
            vec![bidi_emoji(2)],
            22 + 9,
        ),
    ];
    assert_budgets(&mut system, cases);
}

// A stream of fresh rows past the cache's entry cap evicts inside the
// measured frames; the evicted layouts must come back as storage.
#[test]
fn stream_past_cache_capacity_refills_evicted_layouts() {
    let mut system = test_system();
    let rows: Vec<Input> = (0..96).map(word_row).collect();
    let mut cache = LayoutCache::new(240).with_max_entries(8);
    cache.begin_frame();
    for row in &rows[..32] {
        cache.layout_query(&mut system, &row.query()).expect("warm");
    }
    let (last, allocations) = count(|| {
        let mut last = None;
        for row in &rows[32..] {
            last = cache.layout_query(&mut system, &row.query()).ok();
        }
        last
    });
    // Each row's residual is unicode-bidi's five vectors. Its 19 words'
    // monospace fallback candidates reuse one vector, its lines' reordering
    // reuses another, and the evictions themselves must add nothing.
    assert!(
        allocations <= 64 * 5 + 2,
        "{allocations} allocations for 64 rows"
    );
    let last = last.expect("layout");
    assert_eq!(last.text().as_ref(), rows[95].text);
    assert_eq!(last.verify_integrity(), Ok(()));
}

// A layout still held elsewhere when evicted cannot be refilled then; once
// its holder drops it, the next miss must reuse it.
#[test]
fn layout_released_after_eviction_is_refilled() {
    let mut system = test_system();
    // The one pooled layout, with an anagram of the measured text so the
    // fonts' codepoint lookups are warm too.
    let mut cache = warmed_cache(&mut system, &[ui("eagle brisk".into())]);
    cache.begin_frame();
    let held = cache
        .layout_query(&mut system, &ui("plump heron".into()).query())
        .expect("layout");
    cache.begin_frame();
    cache.trim();
    drop(held);
    let fresh = ui("brisk eagle".into());
    let (layout, allocations) = count(|| cache.layout_query(&mut system, &fresh.query()));
    assert!(allocations <= 6, "{allocations} allocations");
    assert_eq!(layout.expect("layout").text().as_ref(), fresh.text);
}
