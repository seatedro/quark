use std::collections::HashMap;

use proptest::prelude::*;
use taffy::{Dimension, LengthPercentage, LengthPercentageAuto};

use super::super::cache::EVICT_AFTER_PASSES;
use super::*;
use crate::test_alloc;
use crate::theme::Theme;

fn sized(width: f32, height: f32) -> taffy::Style {
    taffy::Style {
        size: taffy::Size {
            width: Dimension::length(width),
            height: Dimension::length(height),
        },
        ..Default::default()
    }
}

fn flex(direction: taffy::FlexDirection) -> taffy::Style {
    taffy::Style {
        display: taffy::Display::Flex,
        flex_direction: direction,
        ..Default::default()
    }
}

/// Vendored fonts only, so measurements do not depend on the machine.
struct TestText {
    system: TextSystem,
    layouts: LayoutCache,
}

impl TestText {
    fn new() -> Self {
        Self {
            system: TextSystem::vendored_only(&Default::default()),
            layouts: LayoutCache::default(),
        }
    }

    fn measure_context(&mut self, scale: f32) -> MeasureContext<'_> {
        MeasureContext {
            text: &mut self.system,
            layouts: &mut self.layouts,
            scale,
        }
    }
}

/// One frame of a window of nested flex rows and columns, a block, a
/// wrapping row, and an absolute overlay: `width` sizes one deep leaf,
/// `popup` adds a container with two children, and `rows` keys the
/// sidebar's rows.
fn frame(
    engine: &mut LayoutEngine,
    cx: &mut MeasureContext,
    width: f32,
    popup: bool,
    rows: &[u64],
) {
    engine.clear();
    let title = engine.request_layout(sized(120.0, 20.0), &[]);
    let spacer = engine.request_layout(
        taffy::Style {
            flex_grow: 1.0,
            ..Default::default()
        },
        &[],
    );
    let button = engine.request_layout(sized(30.0, 20.0), &[]);
    let header = engine.request_layout(
        taffy::Style {
            gap: taffy::Size {
                width: LengthPercentage::length(8.0),
                height: LengthPercentage::length(0.0),
            },
            ..flex(taffy::FlexDirection::Row)
        },
        &[title, spacer, button],
    );
    engine.begin_children(None);
    for &key in rows {
        engine.begin_children(Some(key));
        let icon = engine.request_layout(sized(30.0, 30.0), &[]);
        engine.push_child(icon);
        let row = engine.finish_children(&flex(taffy::FlexDirection::Row));
        engine.push_child(row);
    }
    let sidebar = engine.finish_children(&taffy::Style {
        size: taffy::Size {
            width: Dimension::length(80.0),
            height: Dimension::auto(),
        },
        ..flex(taffy::FlexDirection::Column)
    });
    let tiles: [LayoutId; 6] =
        std::array::from_fn(|_| engine.request_layout(sized(50.0, 24.0), &[]));
    let grid = engine.request_layout(
        taffy::Style {
            flex_wrap: taffy::FlexWrap::Wrap,
            ..flex(taffy::FlexDirection::Row)
        },
        &tiles,
    );
    let deep = engine.request_layout(sized(width, 10.0), &[]);
    let column = engine.request_layout(flex(taffy::FlexDirection::Column), &[deep]);
    let content = engine.request_layout(
        taffy::Style {
            display: taffy::Display::Block,
            flex_grow: 1.0,
            ..Default::default()
        },
        &[grid, column],
    );
    let body = engine.request_layout(
        taffy::Style {
            flex_grow: 1.0,
            ..flex(taffy::FlexDirection::Row)
        },
        &[sidebar, content],
    );
    engine.begin_children(None);
    engine.push_child(header);
    engine.push_child(body);
    if popup {
        let lines = [
            engine.request_layout(sized(60.0, 12.0), &[]),
            engine.request_layout(sized(40.0, 12.0), &[]),
        ];
        let popup = engine.request_layout(
            taffy::Style {
                position: taffy::Position::Absolute,
                inset: taffy::Rect {
                    left: LengthPercentageAuto::percent(0.5),
                    top: LengthPercentageAuto::length(40.0),
                    right: LengthPercentageAuto::auto(),
                    bottom: LengthPercentageAuto::auto(),
                },
                ..flex(taffy::FlexDirection::Column)
            },
            &lines,
        );
        engine.push_child(popup);
    }
    let root = engine.finish_children(&taffy::Style {
        padding: taffy::Rect::length(4.0_f32),
        ..flex(taffy::FlexDirection::Column)
    });
    engine.compute_layout(root, 400.0, 300.0, cx);
}

/// Allocations of the last of `frames`, each `(width, popup, rows)`.
fn last_frame_allocations(frames: &[(f32, bool, &[u64])]) -> u64 {
    let mut ts = TestText::new();
    let mut cx = ts.measure_context(1.0);
    let mut engine = LayoutEngine::new();
    let (last, warmup) = frames.split_last().unwrap();
    for &(width, popup, rows) in warmup {
        frame(&mut engine, &mut cx, width, popup, rows);
    }
    test_alloc::count(|| frame(&mut engine, &mut cx, last.0, last.1, last.2)).1
}

#[test]
fn warmed_relayout_allocates_nothing() {
    let rows: &[u64] = &[1, 2];
    let frames = [
        (10.0, false, rows),
        (20.0, false, rows),
        (10.0, false, rows),
        (20.0, false, rows),
    ];
    assert_eq!(last_frame_allocations(&frames), 0);
}

#[test]
fn warmed_frame_that_adds_nodes_allocates_nothing() {
    let rows: &[u64] = &[1, 2];
    let frames = [
        (10.0, false, rows),
        (10.0, true, rows),
        (10.0, false, rows),
        (10.0, true, rows),
    ];
    assert_eq!(last_frame_allocations(&frames), 0);
}

#[test]
fn warmed_frame_that_inserts_a_keyed_row_allocates_nothing() {
    let (rows, inserted): (&[u64], &[u64]) = (&[1, 2], &[0, 1, 2]);
    let frames = [
        (10.0, false, rows),
        (10.0, false, inserted),
        (10.0, false, rows),
        (10.0, false, inserted),
    ];
    assert_eq!(last_frame_allocations(&frames), 0);
}

/// A frame of one container of rows keyed by `keys`, each a container of
/// one leaf; the rows' nodes.
fn keyed_frame(engine: &mut LayoutEngine, cx: &mut MeasureContext, keys: &[u64]) -> Vec<LayoutId> {
    engine.clear();
    engine.begin_children(None);
    let rows: Vec<LayoutId> = keys
        .iter()
        .map(|&key| {
            engine.begin_children(Some(key));
            let leaf = engine.request_layout(sized(20.0, 10.0), &[]);
            engine.push_child(leaf);
            let row = engine.finish_children(&flex(taffy::FlexDirection::Row));
            engine.push_child(row);
            row
        })
        .collect();
    let root = engine.finish_children(&flex(taffy::FlexDirection::Column));
    engine.compute_layout(root, 400.0, 300.0, cx);
    rows
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// Runs of frames, each run laying out its own distinct keys in its own
    /// order for a number of passes, so keys come and go, move, and stay
    /// away past eviction.
    #[test]
    fn keyed_rows_keep_their_nodes_until_evicted(
        runs in prop::collection::vec(
            (
                prop::collection::btree_set(0..24_u64, 0..10)
                    .prop_map(Vec::from_iter)
                    .prop_shuffle(),
                1..=80_usize,
            ),
            1..6,
        )
    ) {
        let mut ts = TestText::new();
        let mut cx = ts.measure_context(1.0);
        let mut engine = LayoutEngine::new();
        // Each key's node and the frame it was last laid out in.
        let mut seen: HashMap<u64, (LayoutId, u64)> = HashMap::new();
        let mut frame = 0_u64;
        for (keys, passes) in runs {
            for _ in 0..passes {
                let rows = keyed_frame(&mut engine, &mut cx, &keys);
                prop_assert_eq!(engine.verify_integrity(), Ok(()));
                for (&key, &row) in keys.iter().zip(&rows) {
                    if let Some(&(node, last)) = seen.get(&key)
                        && frame - last <= EVICT_AFTER_PASSES
                    {
                        prop_assert_eq!(row, node, "key {} after {} frames", key, frame - last);
                    }
                    seen.insert(key, (row, frame));
                }
                frame += 1;
            }
        }
    }
}

#[test]
fn nodes_evicted_past_the_spares_leave_the_tree() {
    let mut ts = TestText::new();
    let mut cx = ts.measure_context(1.0);
    let mut engine = LayoutEngine::new();
    let keys: Vec<u64> = (0..400).collect();
    keyed_frame(&mut engine, &mut cx, &keys);
    let held = engine.tree.total_node_count();
    for _ in 0..EVICT_AFTER_PASSES + 2 {
        keyed_frame(&mut engine, &mut cx, &[]);
    }
    let left = engine.tree.total_node_count();
    assert!(left < held / 2, "{left} of {held} nodes left");
}

/// Text layout lookups laying `root` out in `engine` makes past the ones
/// building it makes: the wrapped text it measures.
fn measure_lookups(
    engine: &mut LayoutEngine,
    cx: &mut ElementContext,
    mut root: AnyElement,
) -> u64 {
    let lookups = |cx: &ElementContext| {
        let stats = cx.layouts.stats();
        stats.hits + stats.misses
    };
    engine.clear();
    let id = root.request_layout(engine, cx);
    let built = lookups(cx);
    engine.compute_layout(id, 400.0, 300.0, &mut cx.measure_context());
    lookups(cx) - built
}

/// A 60-point column of `rows`, each built from its text by `row`.
fn column(rows: &[&'static str], row: impl Fn(&'static str) -> AnyElement) -> AnyElement {
    div()
        .w(60.0)
        .flex_col()
        .children(rows.iter().map(|&line| row(line)))
        .into_any()
}

/// Lines too long for 60 points, which measure by wrapping, and one that
/// fits.
const B: &str = "a row long enough to wrap";
const C: &str = "another row that wraps";
const D: &str = "a third row, wrapping";
const SHORT: &str = "x";

#[test]
fn inserting_or_removing_a_keyed_row_measures_no_other_row() {
    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut ts = TestText::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut ts.system, &mut ts.layouts, None, &signals);
    let keyed = |line: &'static str| div().key(line).child(text(line)).into_any();
    let changes: [(&[&str], &[&str]); 3] = [
        (&[B, C, D], &[SHORT, B, C, D]),
        (&[SHORT, B, C, D], &[B, C, D]),
        (&[B, C, D], &[B, D]),
    ];
    for (before, after) in changes {
        let mut engine = LayoutEngine::new();
        measure_lookups(&mut engine, &mut cx, column(before, keyed));
        let lookups = measure_lookups(&mut engine, &mut cx, column(after, keyed));
        assert_eq!(lookups, 0, "{before:?} to {after:?}");
    }
}

#[test]
fn a_rebuilt_boundary_lays_out_in_its_own_subtree() {
    let theme = Theme::default_dark();
    let signals = SignalStore::new();
    let mut ts = TestText::new();
    let mut cx = ElementContext::new(&theme, 1.0, &mut ts.system, &mut ts.layouts, None, &signals);
    // Without an element cache every boundary builds.
    let boundary = |line: &'static str| cached(line, 0, move || text(line)).into_any();
    let changes: [(&[&str], &[&str]); 2] = [(&[B, C], &[C]), (&[B, C], &[C, B])];
    for (before, after) in changes {
        let mut engine = LayoutEngine::new();
        measure_lookups(&mut engine, &mut cx, column(before, boundary));
        let lookups = measure_lookups(&mut engine, &mut cx, column(after, boundary));
        assert_eq!(lookups, 0, "{before:?} to {after:?}");
    }
}

/// Whether `v` points is a whole number of device pixels at `scale`.
fn on_grid(v: f32, scale: f32) -> bool {
    let px = v * scale;
    (px - px.round()).abs() < 1e-3
}

/// A cache boundary's content lands on whole device pixels at its
/// final size, after its parent sized it at fractional widths.
#[test]
fn boundary_content_lands_on_whole_pixels() {
    for scale in [1.0, 1.25, 1.5, 2.0] {
        boundary_content_lands_on_whole_pixels_at(scale);
    }
}

fn boundary_content_lands_on_whole_pixels_at(scale: f32) {
    let mut ts = TestText::new();
    let mut cx = ts.measure_context(scale);
    let mut engine = LayoutEngine::new();
    let index = engine.begin_subtree(CacheKey(1));
    let sub = engine.subtree_mut(index);
    let third = sub.request_layout(
        taffy::Style {
            size: taffy::Size {
                width: Dimension::percent(1.0 / 3.0),
                height: Dimension::length(10.25),
            },
            ..Default::default()
        },
        &[],
    );
    // As tall as text of a fixed area wrapped to its width.
    let wrapped = sub.request_measured_layout(taffy::Style::default(), |known, available| {
        let width = known.width.unwrap_or(match available.width {
            AvailableSpace::Definite(width) => width,
            _ => 50.0,
        });
        taffy::Size {
            width,
            height: 700.0 / width,
        }
    });
    let content = sub.request_layout(
        taffy::Style {
            padding: taffy::Rect::length(4.5_f32),
            ..flex(taffy::FlexDirection::Column)
        },
        &[third, wrapped],
    );
    let boundary = engine.finish_subtree(index, taffy::Style::default(), content, false);
    let root = engine.request_layout(flex(taffy::FlexDirection::Column), &[boundary]);
    engine.compute_layout(root, 137.5, 300.0, &mut cx);

    let sub = engine.subtree(index);
    for node in sub.nodes.claimed_nodes() {
        let b = sub.layout_bounds(node);
        let edges = [b.x, b.y, b.width, b.height];
        assert!(
            edges.iter().all(|&v| on_grid(v, scale)),
            "{b:?} at {scale}x"
        );
    }
}

#[test]
fn fractional_padding_insets_evenly_on_the_device_grid() {
    let mut ts = TestText::new();
    let scale = 2.0;
    let mut cx = ts.measure_context(scale);
    let mut engine = LayoutEngine::new();
    let child = engine.request_layout(
        taffy::Style {
            flex_grow: 1.0,
            ..Default::default()
        },
        &[],
    );
    let padded = engine.request_layout(
        taffy::Style {
            size: taffy::Size {
                width: Dimension::length(100.0),
                height: Dimension::length(40.0),
            },
            padding: taffy::Rect::length(1.625_f32),
            ..flex(taffy::FlexDirection::Row)
        },
        &[child],
    );
    let root = engine.request_layout(
        taffy::Style {
            padding: taffy::Rect::length(0.25_f32),
            ..flex(taffy::FlexDirection::Row)
        },
        &[padded],
    );
    engine.compute_layout(root, 400.0, 300.0, &mut cx);

    let outer = engine.layout_bounds(padded);
    let inner = engine.layout_bounds(child);
    let insets = [
        inner.x - outer.x,
        outer.right() - inner.right(),
        inner.y - outer.y,
        outer.bottom() - inner.bottom(),
    ];
    for b in [outer, inner] {
        let edges = [b.x, b.y, b.width, b.height];
        assert!(edges.iter().all(|&v| on_grid(v, scale)), "{b:?}");
    }
    let pixel = 1.0 / scale;
    assert!(
        insets.iter().all(|inset| (inset - 1.625).abs() <= pixel),
        "{insets:?}"
    );
    assert!((insets[0] - insets[1]).abs() <= pixel, "{insets:?}");
    assert!((insets[2] - insets[3]).abs() <= pixel, "{insets:?}");
}
