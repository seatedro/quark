//! Lays out a set of layouts with the vendored algorithms and with the copies of the algorithms as taffy 0.9.2
//! published them (`upstream_flexbox`, `upstream_block`), and checks that every node gets the same layout.

use core::cell::Cell;

use crate::geometry::{Rect, Size};
use crate::style::{
    AlignContent, AlignItems, AlignSelf, AvailableSpace, BoxSizing, Dimension, Display, FlexDirection, FlexWrap,
    JustifyContent, LengthPercentage, LengthPercentageAuto, Overflow, Position, Style,
};
use crate::style_helpers::{TaffyMaxContent, TaffyMinContent};
use crate::tree::{Layout, NodeId};
use crate::TaffyTree;

std::thread_local! {
    static UPSTREAM: Cell<bool> = const { Cell::new(false) };
}

/// Whether `TaffyTree` should lay flexbox and block containers out with the published algorithms.
pub(crate) fn use_upstream() -> bool {
    UPSTREAM.with(Cell::get)
}

/// A leaf that measures like wrapped text: `words` words, each `word` wide, `line` tall per line.
#[derive(Debug, Clone, Copy)]
struct Text {
    /// Number of words
    words: u16,
    /// Width of one word, including its space
    word: f32,
    /// Height of one line
    line: f32,
}

impl Text {
    /// The size of the text in the given constraints
    fn measure(self, known: Size<Option<f32>>, available: Size<AvailableSpace>) -> Size<f32> {
        let max = f32::from(self.words) * self.word;
        let width = known.width.unwrap_or(match available.width {
            AvailableSpace::MinContent => self.word,
            AvailableSpace::MaxContent => max,
            AvailableSpace::Definite(width) => width.clamp(self.word, max),
        });
        let per_line = (width / self.word).floor().max(1.0);
        let lines = (f32::from(self.words) / per_line).ceil();
        Size { width, height: known.height.unwrap_or(lines * self.line) }
    }
}

/// A tree being built
type Tree = TaffyTree<Text>;

/// Builds one layout into a tree and returns its root
type Build = fn(&mut Tree) -> NodeId;

/// A length
fn px(value: f32) -> Dimension {
    Dimension::length(value)
}

/// A percentage, 0 to 1
fn pct(value: f32) -> Dimension {
    Dimension::percent(value)
}

/// A width and height
fn wh(width: Dimension, height: Dimension) -> Size<Dimension> {
    Size { width, height }
}

/// The same length on every side
fn sides(value: f32) -> Rect<LengthPercentage> {
    Rect::length(value)
}

/// Margins on every side
fn margins(left: f32, right: f32, top: f32, bottom: f32) -> Rect<LengthPercentageAuto> {
    Rect {
        left: LengthPercentageAuto::length(left),
        right: LengthPercentageAuto::length(right),
        top: LengthPercentageAuto::length(top),
        bottom: LengthPercentageAuto::length(bottom),
    }
}

/// A container
fn node(tree: &mut Tree, style: Style, children: &[NodeId]) -> NodeId {
    tree.new_with_children(style, children).unwrap()
}

/// A leaf with no content
fn leaf(tree: &mut Tree, style: Style) -> NodeId {
    tree.new_leaf(style).unwrap()
}

/// A text leaf
fn text(tree: &mut Tree, style: Style, words: u16) -> NodeId {
    tree.new_leaf_with_context(style, Text { words, word: 30.0, line: 18.0 }).unwrap()
}

/// A row
fn row() -> Style {
    Style { display: Display::Flex, flex_direction: FlexDirection::Row, ..Default::default() }
}

/// A column
fn column() -> Style {
    Style { display: Display::Flex, flex_direction: FlexDirection::Column, ..Default::default() }
}

/// Growing items clamped by min and max sizes, which takes several passes of the freeze loop
fn grow_with_min_and_max(tree: &mut Tree) -> NodeId {
    let a = leaf(tree, Style { flex_grow: 1.0, max_size: wh(px(60.0), Dimension::auto()), ..Default::default() });
    let b = leaf(tree, Style { flex_grow: 2.0, min_size: wh(px(200.0), Dimension::auto()), ..Default::default() });
    let c = leaf(tree, Style { flex_grow: 1.0, flex_basis: px(50.0), ..Default::default() });
    let d = leaf(tree, Style { size: wh(px(40.0), px(20.0)), ..Default::default() });
    let e = text(tree, Style { flex_grow: 3.0, max_size: wh(px(150.0), Dimension::auto()), ..Default::default() }, 3);
    let f = leaf(tree, Style { flex_grow: 0.5, flex_basis: px(10.0), ..Default::default() });
    let style = Style { size: wh(px(700.0), px(100.0)), gap: Size::length(10.0), ..row() };
    node(tree, style, &[a, b, c, d, e, f])
}

/// Shrinking items stopped at their content's minimum size and at min sizes
fn shrink_to_minimums(tree: &mut Tree) -> NodeId {
    let a = text(tree, Style { flex_shrink: 1.0, ..Default::default() }, 5);
    let b = text(tree, Style { flex_shrink: 2.0, ..Default::default() }, 5);
    let c = leaf(tree, Style { flex_shrink: 3.0, size: wh(px(120.0), px(10.0)), ..Default::default() });
    let d = text(
        tree,
        Style { overflow: crate::Point { x: Overflow::Hidden, y: Overflow::Visible }, ..Default::default() },
        6,
    );
    let e = leaf(tree, Style { flex_shrink: 0.0, size: wh(px(50.0), px(10.0)), ..Default::default() });
    let f = leaf(
        tree,
        Style {
            flex_basis: px(100.0),
            min_size: wh(px(70.0), Dimension::auto()),
            max_size: wh(px(90.0), Dimension::auto()),
            ..Default::default()
        },
    );
    let style = Style { size: wh(px(260.0), Dimension::auto()), ..row() };
    node(tree, style, &[a, b, c, d, e, f])
}

/// Shrinking under small flex factors, whose sum is below one
fn shrink_with_small_factors(tree: &mut Tree) -> NodeId {
    let a = leaf(tree, Style { flex_basis: px(100.0), flex_shrink: 0.25, ..Default::default() });
    let b = leaf(
        tree,
        Style {
            flex_basis: px(100.0),
            flex_shrink: 0.25,
            min_size: wh(px(90.0), Dimension::auto()),
            ..Default::default()
        },
    );
    let c = leaf(tree, Style { flex_basis: px(80.0), flex_shrink: 0.1, ..Default::default() });
    let style = Style { size: wh(px(150.0), px(40.0)), ..row() };
    node(tree, style, &[a, b, c])
}

/// Wrapped lines with gaps, spaced by `align-content`
fn wrap_with_gaps(tree: &mut Tree) -> NodeId {
    let mut children = [NodeId::new(0); 7];
    for (i, width) in [100.0, 80.0, 60.0, 120.0, 30.0].into_iter().enumerate() {
        children[i] = leaf(tree, Style { size: wh(px(width), px(20.0 + i as f32 * 7.0)), ..Default::default() });
    }
    children[5] = text(tree, Style { flex_grow: 1.0, ..Default::default() }, 4);
    children[6] = leaf(tree, Style { flex_basis: px(0.0), ..Default::default() });
    let style = Style {
        flex_wrap: FlexWrap::Wrap,
        size: wh(px(230.0), px(300.0)),
        gap: Size { width: LengthPercentage::length(10.0), height: LengthPercentage::length(5.0) },
        align_content: Some(AlignContent::SpaceBetween),
        align_items: Some(AlignItems::Center),
        ..row()
    };
    node(tree, style, &children)
}

/// Reversed wrapped columns whose lines stretch to the container
fn wrap_reverse_columns(tree: &mut Tree) -> NodeId {
    let mut children = [NodeId::new(0); 5];
    for (i, height) in [70.0, 90.0, 50.0, 40.0, 60.0].into_iter().enumerate() {
        let style = Style {
            size: wh(if i % 2 == 0 { Dimension::auto() } else { px(30.0 + i as f32) }, px(height)),
            align_self: (i == 3).then_some(AlignSelf::FlexEnd),
            ..Default::default()
        };
        children[i] = leaf(tree, style);
    }
    let style = Style {
        flex_wrap: FlexWrap::WrapReverse,
        size: wh(px(200.0), px(200.0)),
        align_content: Some(AlignContent::Stretch),
        justify_content: Some(JustifyContent::SpaceAround),
        ..column()
    };
    node(tree, style, &children)
}

/// A wrapping row laid out under a min-content constraint, one item per line
fn wrap_under_min_content(tree: &mut Tree) -> NodeId {
    let a = text(tree, Style::default(), 3);
    let b = leaf(tree, Style { size: wh(px(40.0), px(10.0)), ..Default::default() });
    let c = text(tree, Style { margin: margins(5.0, 5.0, 0.0, 0.0), ..Default::default() }, 2);
    let style = Style { flex_wrap: FlexWrap::Wrap, gap: Size::length(4.0), ..row() };
    node(tree, style, &[a, b, c])
}

/// Items aligned on their baselines, including a nested container's
fn baseline_alignment(tree: &mut Tree) -> NodeId {
    let a = text(tree, Style { size: wh(px(60.0), Dimension::auto()), ..Default::default() }, 4);
    let inner_a = text(tree, Style::default(), 1);
    let inner_b = text(tree, Style::default(), 2);
    let nested = node(tree, Style { margin: margins(0.0, 0.0, 10.0, 0.0), ..column() }, &[inner_a, inner_b]);
    let b =
        leaf(tree, Style { size: wh(px(20.0), px(30.0)), margin: margins(0.0, 0.0, 5.0, 0.0), ..Default::default() });
    let c =
        leaf(tree, Style { size: wh(px(20.0), px(12.0)), align_self: Some(AlignSelf::FlexEnd), ..Default::default() });
    let d = leaf(
        tree,
        Style {
            size: wh(px(20.0), px(12.0)),
            margin: Rect { top: LengthPercentageAuto::auto(), bottom: LengthPercentageAuto::auto(), ..Rect::zero() },
            ..Default::default()
        },
    );
    let style = Style { align_items: Some(AlignItems::Baseline), size: wh(px(400.0), px(120.0)), ..row() };
    node(tree, style, &[a, nested, b, c, d])
}

/// Sizes, padding, margins, gaps, and flex bases in percentages
fn percentages(tree: &mut Tree) -> NodeId {
    let a = leaf(tree, Style { size: wh(pct(0.5), pct(0.25)), ..Default::default() });
    let b = leaf(
        tree,
        Style {
            size: wh(pct(0.3), px(20.0)),
            margin: Rect {
                left: LengthPercentageAuto::percent(0.1),
                right: LengthPercentageAuto::length(0.0),
                top: LengthPercentageAuto::percent(0.05),
                bottom: LengthPercentageAuto::length(0.0),
            },
            ..Default::default()
        },
    );
    let c1 = leaf(tree, Style { flex_basis: pct(0.4), ..Default::default() });
    let c2 = leaf(tree, Style { flex_basis: pct(0.2), flex_grow: 1.0, ..Default::default() });
    let c = node(tree, Style { size: wh(pct(1.0), pct(0.2)), gap: Size::percent(0.05), ..row() }, &[c1, c2]);
    let style = Style {
        size: wh(px(400.0), px(300.0)),
        padding: Rect::percent(0.05),
        gap: Size { width: LengthPercentage::length(0.0), height: LengthPercentage::percent(0.02) },
        ..column()
    };
    node(tree, style, &[a, b, c])
}

/// Containers nested three deep, in reversed directions, with auto margins and justification
fn nested_containers(tree: &mut Tree) -> NodeId {
    let x = leaf(tree, Style { size: wh(px(10.0), px(10.0)), ..Default::default() });
    let y = text(tree, Style::default(), 2);
    let z = leaf(
        tree,
        Style {
            size: wh(px(10.0), px(15.0)),
            margin: Rect { top: LengthPercentageAuto::auto(), ..Rect::zero() },
            ..Default::default()
        },
    );
    let inner = node(
        tree,
        Style {
            flex_direction: FlexDirection::ColumnReverse,
            justify_content: Some(JustifyContent::Center),
            flex_grow: 1.0,
            ..column()
        },
        &[x, y, z],
    );
    let p = leaf(
        tree,
        Style {
            size: wh(px(30.0), px(30.0)),
            margin: Rect { left: LengthPercentageAuto::auto(), ..Rect::zero() },
            ..Default::default()
        },
    );
    let q = text(tree, Style { flex_shrink: 2.0, ..Default::default() }, 8);
    let middle = node(
        tree,
        Style {
            flex_direction: FlexDirection::RowReverse,
            justify_content: Some(JustifyContent::SpaceEvenly),
            padding: sides(4.0),
            border: sides(1.0),
            flex_grow: 1.0,
            ..row()
        },
        &[inner, p, q],
    );
    let footer = text(tree, Style::default(), 12);
    let style = Style { size: wh(px(320.0), px(240.0)), align_items: Some(AlignItems::Stretch), ..column() };
    node(tree, style, &[middle, footer])
}

/// Absolutely positioned and hidden children of flex and block containers
fn absolute_children(tree: &mut Tree) -> NodeId {
    let flow = leaf(tree, Style { size: wh(px(50.0), px(50.0)), ..Default::default() });
    let stretched = leaf(
        tree,
        Style {
            position: Position::Absolute,
            inset: Rect {
                left: LengthPercentageAuto::percent(0.1),
                right: LengthPercentageAuto::length(20.0),
                top: LengthPercentageAuto::length(5.0),
                bottom: LengthPercentageAuto::percent(0.25),
            },
            ..Default::default()
        },
    );
    let aligned = text(
        tree,
        Style {
            position: Position::Absolute,
            align_self: Some(AlignSelf::Center),
            inset: Rect { right: LengthPercentageAuto::length(0.0), ..Rect::auto() },
            ..Default::default()
        },
        3,
    );
    let hidden = leaf(tree, Style { display: Display::None, size: wh(px(99.0), px(99.0)), ..Default::default() });
    let flex = node(
        tree,
        Style {
            size: wh(px(300.0), px(200.0)),
            justify_content: Some(JustifyContent::FlexEnd),
            padding: sides(10.0),
            ..row()
        },
        &[flow, stretched, aligned, hidden],
    );
    let block_flow = text(tree, Style::default(), 9);
    let block_abs = leaf(
        tree,
        Style {
            position: Position::Absolute,
            size: wh(pct(0.5), px(30.0)),
            inset: Rect { bottom: LengthPercentageAuto::length(0.0), ..Rect::auto() },
            ..Default::default()
        },
    );
    let block = node(
        tree,
        Style { display: Display::Block, padding: sides(6.0), ..Default::default() },
        &[block_flow, block_abs],
    );
    node(tree, Style { size: wh(px(320.0), Dimension::auto()), ..column() }, &[flex, block])
}

/// Block flow with collapsing margins, a nested flex container, and content-box sizing
fn block_flow(tree: &mut Tree) -> NodeId {
    let a = leaf(
        tree,
        Style { size: wh(Dimension::auto(), px(20.0)), margin: margins(0.0, 0.0, 10.0, 15.0), ..Default::default() },
    );
    let empty =
        leaf(tree, Style { display: Display::Block, margin: margins(0.0, 0.0, 8.0, 4.0), ..Default::default() });
    let b = text(tree, Style { margin: margins(5.0, 5.0, 12.0, 0.0), ..Default::default() }, 11);
    let fa = leaf(tree, Style { flex_grow: 1.0, size: wh(Dimension::auto(), px(10.0)), ..Default::default() });
    let fb = text(tree, Style::default(), 2);
    let flex = node(tree, Style { margin: margins(0.0, 0.0, 6.0, 6.0), ..row() }, &[fa, fb]);
    let c = leaf(
        tree,
        Style {
            box_sizing: BoxSizing::ContentBox,
            size: wh(pct(0.5), px(10.0)),
            padding: sides(3.0),
            border: sides(2.0),
            ..Default::default()
        },
    );
    let inner = node(tree, Style { display: Display::Block, ..Default::default() }, &[a, empty, b]);
    let style = Style { display: Display::Block, size: wh(px(300.0), Dimension::auto()), ..Default::default() };
    node(tree, style, &[inner, flex, c])
}

/// A row with no definite width, sized from its items' contributions
fn intrinsic_row(tree: &mut Tree) -> NodeId {
    let a = text(tree, Style { flex_grow: 1.0, ..Default::default() }, 4);
    let b = text(tree, Style { flex_shrink: 0.0, max_size: wh(px(70.0), Dimension::auto()), ..Default::default() }, 6);
    let c = leaf(tree, Style { flex_basis: px(40.0), min_size: wh(px(55.0), Dimension::auto()), ..Default::default() });
    let d = leaf(
        tree,
        Style {
            flex_basis: px(30.0),
            flex_grow: 0.0,
            flex_shrink: 0.0,
            size: wh(px(20.0), Dimension::auto()),
            ..Default::default()
        },
    );
    node(tree, Style { gap: Size::length(3.0), padding: sides(2.0), ..row() }, &[a, b, c, d])
}

/// Aspect ratios, scroll containers with scrollbars, and overflowing content
fn aspect_ratio_and_scrolling(tree: &mut Tree) -> NodeId {
    let a = leaf(tree, Style { size: wh(px(40.0), Dimension::auto()), aspect_ratio: Some(2.0), ..Default::default() });
    let b = leaf(
        tree,
        Style {
            flex_grow: 1.0,
            aspect_ratio: Some(0.5),
            max_size: wh(Dimension::auto(), px(90.0)),
            ..Default::default()
        },
    );
    let long = text(tree, Style { flex_shrink: 0.0, ..Default::default() }, 30);
    let scroller = node(
        tree,
        Style {
            overflow: crate::Point { x: Overflow::Scroll, y: Overflow::Scroll },
            scrollbar_width: 8.0,
            size: wh(px(120.0), px(60.0)),
            ..column()
        },
        &[long],
    );
    let style = Style { size: wh(px(260.0), px(110.0)), align_items: Some(AlignItems::FlexStart), ..row() };
    node(tree, style, &[a, b, scroller])
}

/// Blocks without a definite width around flex containers, measured by a row and sized from their content
fn intrinsic_blocks(tree: &mut Tree) -> NodeId {
    let a = text(tree, Style { flex_grow: 1.0, ..Default::default() }, 5);
    let b = leaf(tree, Style { size: wh(px(30.0), px(14.0)), ..Default::default() });
    let flex_row = node(tree, Style { margin: margins(4.0, 6.0, 3.0, 2.0), gap: Size::length(5.0), ..row() }, &[a, b]);
    let c = text(tree, Style::default(), 7);
    let d = text(tree, Style { margin: margins(2.0, 2.0, 0.0, 0.0), ..Default::default() }, 2);
    let wrapping =
        node(tree, Style { flex_wrap: FlexWrap::Wrap, max_size: wh(Dimension::auto(), px(40.0)), ..column() }, &[c, d]);
    let leaf_text = text(tree, Style { margin: margins(0.0, 0.0, 6.0, 6.0), ..Default::default() }, 3);
    let block = node(
        tree,
        Style { display: Display::Block, padding: sides(2.0), ..Default::default() },
        &[flex_row, wrapping, leaf_text],
    );
    let e = text(tree, Style::default(), 4);
    let percent = node(tree, Style { size: wh(pct(0.5), Dimension::auto()), ..column() }, &[e]);
    let other = node(tree, Style { display: Display::Block, flex_shrink: 0.0, ..Default::default() }, &[percent]);
    node(tree, Style { gap: Size::length(3.0), ..row() }, &[block, other])
}

/// A block with no style around one flex container with margins, as a host wraps a separately laid out subtree
fn block_around_flex(tree: &mut Tree) -> NodeId {
    let a = text(tree, Style::default(), 9);
    let b = leaf(tree, Style { size: wh(pct(0.25), px(12.0)), ..Default::default() });
    let c = text(tree, Style { flex_grow: 1.0, ..Default::default() }, 4);
    let tiles = node(tree, Style { flex_wrap: FlexWrap::Wrap, gap: Size::length(4.0), ..row() }, &[b, c]);
    let content = node(
        tree,
        Style { margin: margins(3.0, 5.0, 7.0, 2.0), padding: sides(4.0), gap: Size::length(2.0), ..column() },
        &[a, tiles],
    );
    node(tree, Style { display: Display::Block, ..Default::default() }, &[content])
}

/// Every layout the test compares, with the space its root is given
const CASES: &[(&str, Size<AvailableSpace>, Build)] = &[
    ("grow with min and max", DEFINITE, grow_with_min_and_max),
    ("shrink to minimums", DEFINITE, shrink_to_minimums),
    ("shrink with small factors", DEFINITE, shrink_with_small_factors),
    ("wrap with gaps", DEFINITE, wrap_with_gaps),
    ("wrap reverse columns", DEFINITE, wrap_reverse_columns),
    ("wrap under min-content", Size::MIN_CONTENT, wrap_under_min_content),
    ("wrap under definite space", DEFINITE, wrap_under_min_content),
    ("baseline alignment", DEFINITE, baseline_alignment),
    ("percentages", DEFINITE, percentages),
    ("nested containers", DEFINITE, nested_containers),
    ("absolute children", DEFINITE, absolute_children),
    ("block flow", DEFINITE, block_flow),
    ("intrinsic row under max-content", Size::MAX_CONTENT, intrinsic_row),
    ("intrinsic row under min-content", Size::MIN_CONTENT, intrinsic_row),
    ("intrinsic row in definite space", DEFINITE, intrinsic_row),
    ("aspect ratio and scrolling", DEFINITE, aspect_ratio_and_scrolling),
    ("intrinsic blocks under max-content", Size::MAX_CONTENT, intrinsic_blocks),
    ("intrinsic blocks under min-content", Size::MIN_CONTENT, intrinsic_blocks),
    ("intrinsic blocks in definite space", DEFINITE, intrinsic_blocks),
    ("intrinsic blocks in a narrow space", NARROW, intrinsic_blocks),
    ("block around flex in definite space", DEFINITE, block_around_flex),
    ("block around flex in a narrow space", NARROW, block_around_flex),
    ("block around flex under max-content", Size::MAX_CONTENT, block_around_flex),
];

/// A space narrower than most layouts' content
const NARROW: Size<AvailableSpace> =
    Size { width: AvailableSpace::Definite(150.0), height: AvailableSpace::Definite(600.0) };

/// The space most roots are given
const DEFINITE: Size<AvailableSpace> =
    Size { width: AvailableSpace::Definite(800.0), height: AvailableSpace::Definite(600.0) };

/// Lay `root` out and return every node's unrounded and rounded layouts, depth first
fn lay_out(tree: &mut Tree, root: NodeId, available: Size<AvailableSpace>) -> std::vec::Vec<(Layout, Layout)> {
    tree.compute_layout_with_measure(root, available, |known, available, _, text, _| {
        text.map_or(Size::ZERO, |text| text.measure(known, available))
    })
    .unwrap();
    let mut layouts = std::vec::Vec::new();
    let mut stack = std::vec![root];
    while let Some(node) = stack.pop() {
        layouts.push((*tree.unrounded_layout(node), *tree.layout(node).unwrap()));
        stack.extend(tree.children(node).unwrap().into_iter().rev());
    }
    layouts
}

/// Mark every node below `root` dirty, so the next layout recomputes all of them
fn mark_all_dirty(tree: &mut Tree, root: NodeId) {
    for child in tree.children(root).unwrap() {
        mark_all_dirty(tree, child);
    }
    tree.mark_dirty(root).unwrap();
}

#[test]
fn patched_layouts_match_the_published_algorithms() {
    UPSTREAM.with(|upstream| upstream.set(true));
    let expected: std::vec::Vec<_> = CASES
        .iter()
        .map(|&(_, available, build)| {
            let mut tree = Tree::new();
            let root = build(&mut tree);
            lay_out(&mut tree, root, available)
        })
        .collect();
    UPSTREAM.with(|upstream| upstream.set(false));

    // One tree for every case, so each layout reuses storage that other shapes left behind: first in order,
    // then again in reverse with every node recomputed.
    let mut tree = Tree::new();
    let roots: std::vec::Vec<_> = CASES.iter().map(|&(_, _, build)| build(&mut tree)).collect();
    let order = (0..CASES.len()).chain((0..CASES.len()).rev());
    for (pass, case) in order.enumerate() {
        let (name, available, _) = CASES[case];
        if pass >= CASES.len() {
            mark_all_dirty(&mut tree, roots[case]);
        }
        let actual = lay_out(&mut tree, roots[case], available);
        assert_eq!(actual.len(), expected[case].len(), "{name}");
        for (i, (actual, expected)) in actual.iter().zip(&expected[case]).enumerate() {
            assert_eq!(actual, expected, "{name}, node {i} (depth first), pass {pass}");
        }
    }
}

#[test]
fn root_size_is_the_size_layout_gives_the_root() {
    for &(name, available, build) in CASES {
        let mut tree = Tree::new();
        let root = build(&mut tree);
        let size = tree
            .compute_size_with_measure(root, available, |known, available, _, text, _| {
                text.map_or(Size::ZERO, |text| text.measure(known, available))
            })
            .unwrap();
        lay_out(&mut tree, root, available);
        assert_eq!(size, tree.unrounded_layout(root).size, "{name}");
    }
}
