//! The rope's B-tree: immutable nodes shared through `Arc`, edited by
//! copy-on-write along one root-to-leaf path, or rebuilt along a seam by
//! [`split`] and [`concat`].
//!
//! Invariants (see [`super::Rope::verify_integrity`]):
//!
//! - Every leaf is at the same depth.
//! - A leaf holds at most [`MAX_LEAF`] bytes, and at least [`MIN_LEAF`]
//!   unless it is the root.
//! - An internal node has at most [`MAX_CHILDREN`] children, at least
//!   [`MIN_CHILDREN`] unless it is the root, and at least two as the root.
//! - Every node's summary is the sum of its children's, or its text's.
//!
//! The concatenation scheme follows xi-rope's: two valid trees concatenate
//! by merging along the seam between them, so only nodes on the seam are
//! rebuilt and the result is valid again.

use std::cmp::Ordering;
use std::sync::Arc;

/// Most bytes in a leaf.
pub(super) const MAX_LEAF: usize = 1024;
/// Fewest bytes in a leaf other than the root.
pub(super) const MIN_LEAF: usize = 256;
/// The leaf size text is cut into when built whole, leaving room in each
/// leaf for typing without a split.
const BUILD_LEAF: usize = 768;
/// Most children of an internal node.
pub(super) const MAX_CHILDREN: usize = 16;
/// Fewest children of an internal node other than the root.
pub(super) const MIN_CHILDREN: usize = 4;

/// What a run of text adds up to. Each node keeps its subtree's, so
/// positions in any of these units are found by descending once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TextSummary {
    pub bytes: usize,
    /// `'\n'` bytes.
    pub newlines: usize,
    /// UTF-16 code units.
    pub utf16: usize,
}

impl TextSummary {
    pub fn of(text: &str) -> Self {
        let mut summary = Self {
            bytes: text.len(),
            newlines: 0,
            utf16: 0,
        };
        for ch in text.chars() {
            summary.utf16 += ch.len_utf16();
            summary.newlines += usize::from(ch == '\n');
        }
        summary
    }
}

impl std::ops::Add for TextSummary {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            bytes: self.bytes + other.bytes,
            newlines: self.newlines + other.newlines,
            utf16: self.utf16 + other.utf16,
        }
    }
}

impl std::ops::AddAssign for TextSummary {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

#[derive(Debug, Clone)]
pub(super) enum Node {
    Leaf {
        text: String,
        summary: TextSummary,
    },
    Internal {
        children: Vec<Arc<Node>>,
        summary: TextSummary,
        /// Levels below: 1 for a parent of leaves.
        height: usize,
    },
}

impl Node {
    pub(super) fn leaf(text: String) -> Arc<Self> {
        let summary = TextSummary::of(&text);
        Arc::new(Self::Leaf { text, summary })
    }

    pub(super) fn empty() -> Arc<Self> {
        Self::leaf(String::new())
    }

    /// A parent of `children`, which all have the same height.
    fn parent(children: Vec<Arc<Node>>) -> Arc<Self> {
        let height = children.first().map_or(0, |c| c.height()) + 1;
        let summary = children
            .iter()
            .fold(TextSummary::default(), |sum, c| sum + c.summary());
        Arc::new(Self::Internal {
            children,
            summary,
            height,
        })
    }

    pub(super) fn summary(&self) -> TextSummary {
        match self {
            Self::Leaf { summary, .. } | Self::Internal { summary, .. } => *summary,
        }
    }

    pub(super) fn len(&self) -> usize {
        self.summary().bytes
    }

    pub(super) fn height(&self) -> usize {
        match self {
            Self::Leaf { .. } => 0,
            Self::Internal { height, .. } => *height,
        }
    }

    pub(super) fn children(&self) -> &[Arc<Node>] {
        match self {
            Self::Leaf { .. } => &[],
            Self::Internal { children, .. } => children,
        }
    }

    /// Whether the node is full enough to be a child.
    fn is_ok_child(&self) -> bool {
        match self {
            Self::Leaf { text, .. } => text.len() >= MIN_LEAF,
            Self::Internal { children, .. } => children.len() >= MIN_CHILDREN,
        }
    }

    /// Recomputes an internal node's summary from its children.
    fn resum(&mut self) {
        if let Self::Internal {
            children, summary, ..
        } = self
        {
            *summary = children
                .iter()
                .fold(TextSummary::default(), |sum, c| sum + c.summary());
        }
    }
}

/// The index of the child holding byte `at` of an internal node's text,
/// and where that child starts. `at` on a boundary between two children
/// goes to the later one; the end goes to the last child.
pub(super) fn child_at(children: &[Arc<Node>], at: usize) -> (usize, usize) {
    let mut start = 0;
    let last = children.len().saturating_sub(1);
    for (i, child) in children.iter().enumerate() {
        let len = child.len();
        if at < start + len || i == last {
            return (i, start);
        }
        start += len;
    }
    (0, 0)
}

/// `node` with internal roots of one child replaced by that child.
pub(super) fn normalize(mut node: Arc<Node>) -> Arc<Node> {
    while let Node::Internal { children, .. } = &*node
        && children.len() == 1
        && let Some(child) = children.first()
    {
        node = child.clone();
    }
    node
}

/// Two leaves as one valid tree: side by side when both are full enough,
/// else one leaf, or two halves of their joined text.
fn merge_leaves(a: &Arc<Node>, b: &Arc<Node>) -> Arc<Node> {
    if a.is_ok_child() && b.is_ok_child() {
        return Node::parent(vec![a.clone(), b.clone()]);
    }
    let (Node::Leaf { text: left, .. }, Node::Leaf { text: right, .. }) = (&**a, &**b) else {
        return Node::parent(vec![a.clone(), b.clone()]);
    };
    let mut joined = String::with_capacity(left.len() + right.len());
    joined.push_str(left);
    joined.push_str(right);
    if joined.len() <= MAX_LEAF {
        return Node::leaf(joined);
    }
    let mid = ceil_char_boundary(&joined, joined.len() / 2);
    let tail = joined.split_off(mid);
    Node::parent(vec![Node::leaf(joined), Node::leaf(tail)])
}

/// The children of two adjacent nodes under one parent, or under two
/// parents of a new root when they do not fit in one. Every child must be
/// full enough to be one.
fn merge_nodes(first: &[Arc<Node>], second: &[Arc<Node>]) -> Arc<Node> {
    let count = first.len() + second.len();
    let all = first.iter().chain(second).cloned();
    if count <= MAX_CHILDREN {
        return Node::parent(all.collect());
    }
    let split = MAX_CHILDREN.min(count - MIN_CHILDREN);
    let mut left: Vec<_> = all.collect();
    let right = left.split_off(split);
    Node::parent(vec![Node::parent(left), Node::parent(right)])
}

/// `a` followed by `b`, both valid trees, as one valid tree.
pub(super) fn concat(a: Arc<Node>, b: Arc<Node>) -> Arc<Node> {
    if a.len() == 0 {
        return b;
    }
    if b.len() == 0 {
        return a;
    }
    let (ha, hb) = (a.height(), b.height());
    match ha.cmp(&hb) {
        Ordering::Less => concat_into_left_edge(a, &b),
        Ordering::Greater => concat_into_right_edge(&a, b),
        Ordering::Equal if a.is_ok_child() && b.is_ok_child() => Node::parent(vec![a, b]),
        Ordering::Equal if ha == 0 => merge_leaves(&a, &b),
        Ordering::Equal => merge_nodes(a.children(), b.children()),
    }
}

/// `a` concatenated onto the left edge of the taller `b`.
fn concat_into_left_edge(a: Arc<Node>, b: &Arc<Node>) -> Arc<Node> {
    let hb = b.height();
    let children = b.children();
    let Some((head, rest)) = children.split_first() else {
        return a;
    };
    if a.height() + 1 == hb && a.is_ok_child() {
        return merge_nodes(&[a], children);
    }
    let joined = concat(a, head.clone());
    if joined.height() + 1 == hb {
        merge_nodes(&[joined], rest)
    } else {
        merge_nodes(joined.children(), rest)
    }
}

/// `b` concatenated onto the right edge of the taller `a`.
fn concat_into_right_edge(a: &Arc<Node>, b: Arc<Node>) -> Arc<Node> {
    let ha = a.height();
    let children = a.children();
    let Some((last, rest)) = children.split_last() else {
        return b;
    };
    if b.height() + 1 == ha && b.is_ok_child() {
        return merge_nodes(children, &[b]);
    }
    let joined = concat(last.clone(), b);
    if joined.height() + 1 == ha {
        merge_nodes(rest, &[joined])
    } else {
        merge_nodes(rest, joined.children())
    }
}

/// `children` (full enough, same height) as one valid tree.
fn tree_of(children: &[Arc<Node>]) -> Arc<Node> {
    match children {
        [] => Node::empty(),
        [only] => only.clone(),
        many => Node::parent(many.to_vec()),
    }
}

/// The text before and after byte `at` (a char boundary) as two valid
/// trees.
pub(super) fn split(node: &Arc<Node>, at: usize) -> (Arc<Node>, Arc<Node>) {
    match &**node {
        Node::Leaf { text, .. } => {
            let at = floor_char_boundary(text, at);
            let (head, tail) = text.split_at(at);
            (Node::leaf(head.to_owned()), Node::leaf(tail.to_owned()))
        }
        Node::Internal { children, .. } => {
            if at == 0 {
                return (Node::empty(), node.clone());
            }
            if at >= node.len() {
                return (node.clone(), Node::empty());
            }
            let (i, start) = child_at(children, at);
            let (before, rest) = children.split_at(i);
            let Some((child, after)) = rest.split_first() else {
                return (node.clone(), Node::empty());
            };
            if at == start {
                return (tree_of(before), tree_of(rest));
            }
            let (head, tail) = split(child, at - start);
            (
                normalize(concat(tree_of(before), head)),
                normalize(concat(tail, tree_of(after))),
            )
        }
    }
}

/// `text` as a valid tree of leaves about [`BUILD_LEAF`] long.
pub(super) fn build(text: &str) -> Arc<Node> {
    if text.len() <= MAX_LEAF {
        return Node::leaf(text.to_owned());
    }
    let count = text.len().div_ceil(BUILD_LEAF);
    let mut level = Vec::with_capacity(count);
    let mut start = 0;
    for i in 1..=count {
        let end = ceil_char_boundary(text, text.len() * i / count);
        if end > start {
            level.push(Node::leaf(
                text.get(start..end).unwrap_or_default().to_owned(),
            ));
        }
        start = end;
    }
    while level.len() > 1 {
        level = group(level);
    }
    level.pop().unwrap_or_else(Node::empty)
}

/// One level up: `nodes` under parents of at most [`MAX_CHILDREN`], sized
/// evenly so each has at least [`MIN_CHILDREN`].
fn group(nodes: Vec<Arc<Node>>) -> Vec<Arc<Node>> {
    let parents = nodes.len().div_ceil(MAX_CHILDREN);
    let total = nodes.len();
    let mut out = Vec::with_capacity(parents);
    let mut nodes = nodes.into_iter();
    let mut taken = 0;
    for i in 1..=parents {
        let end = total * i / parents;
        out.push(Node::parent(nodes.by_ref().take(end - taken).collect()));
        taken = end;
    }
    out
}

/// Whether replacing `range` with `with_len` bytes stays inside one leaf
/// below `node` and keeps that leaf within its size bounds.
pub(super) fn fits_in_leaf(
    node: &Node,
    range: std::ops::Range<usize>,
    with_len: usize,
    is_root: bool,
) -> bool {
    match node {
        Node::Leaf { text, .. } => {
            let len = text.len() - range.len() + with_len;
            range.end <= text.len() && len <= MAX_LEAF && (is_root || len >= MIN_LEAF)
        }
        Node::Internal { children, .. } => {
            let (i, start) = child_at(children, range.start);
            children.get(i).is_some_and(|child| {
                range.end <= start + child.len()
                    && fits_in_leaf(
                        child,
                        range.start - start..range.end - start,
                        with_len,
                        false,
                    )
            })
        }
    }
}

/// Replaces `range` with `with` in the leaf [`fits_in_leaf`] found,
/// copying only the nodes on the path to it that others share.
pub(super) fn edit_in_leaf(node: &mut Arc<Node>, range: std::ops::Range<usize>, with: &str) {
    match Arc::make_mut(node) {
        Node::Leaf { text, summary } => {
            text.replace_range(range, with);
            *summary = TextSummary::of(text);
        }
        internal @ Node::Internal { .. } => {
            if let Node::Internal { children, .. } = internal {
                let (i, start) = child_at(children, range.start);
                if let Some(child) = children.get_mut(i) {
                    edit_in_leaf(child, range.start - start..range.end - start, with);
                }
            }
            internal.resum();
        }
    }
}

pub(super) fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at -= 1;
    }
    at
}

pub(super) fn ceil_char_boundary(text: &str, at: usize) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at += 1;
    }
    at
}
