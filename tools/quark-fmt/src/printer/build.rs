//! Builds the [`Doc`] for one view from the shared parser's
//! [`SyntaxTree`], applying the layout rules and placing every comment and
//! intentional blank line from the gaps between leaves.
//!
//! Leaves are consumed strictly in source order. Each one is preceded by
//! its gap, rendered with the separator the layout wants there
//! ([`Sep`]); comments in the gap come first and can turn a soft separator
//! into a hard one. Comments before a closing delimiter are taken
//! separately ([`Builder::comments_here`]) so they indent with the list
//! they end instead of with the closer.

use std::collections::HashMap;
use std::ops::Range;

use quark_view_syntax::syntax::{
    ElementForm, ExprMarker, Leaf, NodeId, NodeKind, SyntaxElement, SyntaxTree, TokenKind,
};

use crate::doc::Doc;
use crate::printer::rust::RustContext;
use crate::trivia::{self, Gap};

/// What the layout puts between the previous output and the next leaf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Sep {
    /// Nothing: `name=`, `{expr}`, `</div>`.
    Tight,
    /// A space only where the source had whitespace: attribute names and
    /// key bindings, whose tokens may or may not touch.
    Keep,
    /// One space.
    Space,
    /// A group's [`Doc::Line`].
    Line,
    /// A group's [`Doc::SoftLine`].
    SoftLine,
    /// A newline.
    Hard,
    /// The gap was already rendered, outside the group that follows.
    Done,
}

/// A fragment the view embeds, before nested views are resolved.
#[derive(Clone, Debug)]
pub(crate) struct PendingEmbed {
    pub context: RustContext,
    pub range: Range<usize>,
    /// The parser's fragments inside it, scanned for nested views.
    pub tokens: Vec<proc_macro2::TokenStream>,
}

pub(crate) struct Builder<'s> {
    src: &'s str,
    tree: &'s SyntaxTree,
    leaves: Vec<Range<usize>>,
    /// `gaps[i]` precedes leaf `i`; the last one runs to the body's end.
    gaps: Vec<Gap>,
    /// Comments of each gap already printed by `comments_here`.
    taken: Vec<usize>,
    next: usize,
    leaf_at: HashMap<usize, usize>,
    /// The parser's Rust fragments by start offset.
    rust_at: HashMap<usize, proc_macro2::TokenStream>,
    pub embeds: Vec<PendingEmbed>,
}

/// Why a view's source cannot be mapped onto its tree.
#[derive(Debug)]
pub(crate) struct MapError {
    pub offset: usize,
    pub message: String,
}

impl<'s> Builder<'s> {
    /// Maps the tree's leaves onto `body` of `src`; every byte between
    /// them must be whitespace or a comment.
    pub fn new(src: &'s str, tree: &'s SyntaxTree, body: Range<usize>) -> Result<Self, MapError> {
        let mut leaves = Vec::new();
        let mut gaps = Vec::new();
        let mut end = body.start;
        for leaf in tree.leaves(tree.root()) {
            let range = leaf.range().byte_range();
            if range.start < end || range.end > body.end || range == (0..0) {
                return Err(MapError {
                    offset: end,
                    message: format!("a token at {range:?} lies outside its view's source"),
                });
            }
            gaps.push(gap(src, end..range.start)?);
            end = range.end;
            leaves.push(range);
        }
        gaps.push(gap(src, end..body.end)?);
        let leaf_at = leaves
            .iter()
            .enumerate()
            .map(|(i, r)| (r.start, i))
            .collect();
        let rust_at = tree
            .leaves(tree.root())
            .filter_map(|l| match l {
                Leaf::Rust(r) => Some((r.range.byte_range().start, r.tokens.clone())),
                Leaf::Token(_) => None,
            })
            .collect();
        Ok(Builder {
            src,
            tree,
            taken: vec![0; gaps.len()],
            leaves,
            gaps,
            next: 0,
            leaf_at,
            rust_at,
            embeds: Vec::new(),
        })
    }

    /// The whole body: headers, then the root, between the invocation's
    /// delimiters. `brace` puts spaces inside a one-line `{ .. }`.
    pub fn view(&mut self, brace: bool, suffix: usize) -> Doc {
        let line = if brace { Sep::Line } else { Sep::SoftLine };
        let root = self.tree.root();
        // Headers stay after the opening delimiter when they fit there and
        // move to the body's indentation as a group otherwise.
        let mut headers = Vec::new();
        let mut lead = line;
        let mut root_doc = Doc::nil();
        for child in self.children(root) {
            let SyntaxElement::Node(id) = child else {
                continue;
            };
            match self.tree.node(id).kind {
                NodeKind::ScaleHeader => {
                    headers.push(self.token(lead, false));
                    headers.push(self.token(Sep::Tight, false));
                    lead = Sep::Space;
                }
                NodeKind::TypedHeader => {
                    headers.push(self.token(lead, false));
                    headers.push(self.embed(Sep::Space, false, 1, RustContext::Type));
                    headers.push(self.token(Sep::Tight, false));
                    lead = Sep::Space;
                }
                _ => root_doc = self.node(id, line, false),
            }
        }
        let trailing = self.comments_here(false);
        Doc::group(Doc::concat([
            Doc::group(Doc::indent(Doc::concat(headers))),
            Doc::indent(Doc::concat([root_doc, trailing])),
            self.sep_doc(line, false, &Gap::default()),
            Doc::Phantom(suffix),
        ]))
    }

    fn children(&self, id: NodeId) -> Vec<SyntaxElement> {
        self.tree.node(id).children.clone()
    }

    fn child_nodes(&self, id: NodeId) -> Vec<NodeId> {
        self.tree
            .node(id)
            .children
            .iter()
            .filter_map(|c| match c {
                SyntaxElement::Node(n) => Some(*n),
                _ => None,
            })
            .collect()
    }

    fn first_leaf(&self, id: NodeId) -> usize {
        let start = self.tree.range(id).byte_range().start;
        self.leaf_at[&start]
    }

    fn gap_has_comments(&self, leaf: usize) -> bool {
        self.gaps[leaf].comments.len() > self.taken[leaf]
    }

    /// Renders the untaken comments of the gap before the next leaf.
    fn render_comments(&mut self, list: bool) -> (Doc, Option<bool>) {
        let gap = &self.gaps[self.next];
        let mut parts = Vec::new();
        let mut last_line = None;
        for c in &gap.comments[self.taken[self.next]..] {
            if c.newlines_before == 0 {
                if c.spaced_before {
                    parts.push(Doc::text(" "));
                }
            } else if list && c.newlines_before >= 2 {
                parts.push(Doc::BlankLine);
            } else {
                parts.push(Doc::HardLine);
            }
            let text = &self.src[c.range.clone()];
            if c.line {
                parts.push(Doc::LineComment(text.trim_end_matches('\r').to_owned()));
            } else if text.contains('\n') {
                parts.push(Doc::Verbatim(text.to_owned()));
            } else {
                parts.push(Doc::text(text));
            }
            last_line = Some(c.line);
        }
        self.taken[self.next] = gap.comments.len();
        (Doc::concat(parts), last_line)
    }

    /// The comments before the next leaf, for the list that leaf closes.
    pub fn comments_here(&mut self, list: bool) -> Doc {
        self.render_comments(list).0
    }

    fn sep_doc(&self, sep: Sep, list: bool, gap: &Gap) -> Doc {
        let blank = list && gap.newlines_after >= 2;
        match sep {
            Sep::Tight | Sep::Done => Doc::nil(),
            Sep::Keep if gap.spaced => Doc::text(" "),
            Sep::Keep => Doc::nil(),
            Sep::Space => Doc::text(" "),
            Sep::Line | Sep::SoftLine | Sep::Hard if blank => Doc::BlankLine,
            Sep::Line => Doc::Line,
            Sep::SoftLine => Doc::SoftLine,
            Sep::Hard => Doc::HardLine,
        }
    }

    /// The gap before the next leaf: its comments, then `sep`.
    fn lead(&mut self, sep: Sep, list: bool) -> Doc {
        if sep == Sep::Done {
            return Doc::nil();
        }
        let (comments, last) = self.render_comments(list);
        let gap = self.gaps[self.next].clone();
        let sep = match (sep, last) {
            // A block comment keeps a space before a touching token only if
            // the source had one.
            (Sep::Tight | Sep::Keep, Some(false)) if gap.spaced_after => Doc::text(" "),
            _ => self.sep_doc(sep, list, &gap),
        };
        Doc::concat([comments, sep])
    }

    /// The next leaf as written, after its gap.
    fn token(&mut self, sep: Sep, list: bool) -> Doc {
        let lead = self.lead(sep, list);
        let text = &self.src[self.leaves[self.next].clone()];
        self.next += 1;
        let text = if text.contains('\n') {
            Doc::Verbatim(text.to_owned())
        } else {
            Doc::text(text)
        };
        Doc::concat([lead, text])
    }

    /// `count` leaves as one embedded fragment.
    fn embed(&mut self, sep: Sep, list: bool, count: usize, context: RustContext) -> Doc {
        let lead = self.lead(sep, list);
        let first = self.next;
        let last = first + count - 1;
        let tokens = self.fragment_tokens(first..last + 1);
        self.next += count;
        self.embeds.push(PendingEmbed {
            context,
            range: self.leaves[first].start..self.leaves[last].end,
            tokens,
        });
        Doc::concat([lead, Doc::Embed(self.embeds.len() - 1)])
    }

    /// The parser's Rust fragments among leaves `range`.
    fn fragment_tokens(&self, range: Range<usize>) -> Vec<proc_macro2::TokenStream> {
        range
            .filter_map(|i| self.rust_at.get(&self.leaves[i].start).cloned())
            .collect()
    }

    /// A child node: element, fragment, text, braced Rust, control flow,
    /// or `let`.
    fn node(&mut self, id: NodeId, lead: Sep, list: bool) -> Doc {
        match self.tree.node(id).kind {
            NodeKind::Element(form) => {
                self.element(id, form == ElementForm::SelfClosing, lead, list)
            }
            NodeKind::Fragment => self.element(id, false, lead, list),
            NodeKind::ExprChild(marker) => self.expr_child(marker, lead, list),
            NodeKind::Text => self.token(lead, list),
            NodeKind::Let => self.embed(lead, list, 1, RustContext::Local),
            NodeKind::If => self.if_node(id, lead, list),
            NodeKind::For => self.for_node(id, lead, list),
            NodeKind::Match => self.match_node(id, lead, list),
            NodeKind::ChildList => self.child_list(id, lead, list),
            other => unreachable!("{other:?} is not a child"),
        }
    }

    fn expr_child(&mut self, marker: ExprMarker, lead: Sep, list: bool) -> Doc {
        if marker == ExprMarker::Bare {
            return self.embed(lead, list, 1, RustContext::Expr);
        }
        let mut parts = vec![self.token(lead, list)];
        if marker != ExprMarker::Plain {
            parts.push(self.token(Sep::Tight, false));
        }
        parts.push(self.embed(Sep::Tight, false, 1, RustContext::Expr));
        parts.push(self.token(Sep::Tight, false));
        Doc::concat(parts)
    }

    /// An element or `<>` fragment.
    fn element(&mut self, id: NodeId, self_closing: bool, lead: Sep, list: bool) -> Doc {
        // The separator before the element stays outside its groups.
        let pre = self.lead(lead, list);
        let body = self.element_body(id, self_closing);
        Doc::concat([pre, body])
    }

    fn element_body(&mut self, id: NodeId, self_closing: bool) -> Doc {
        let nodes = self.child_nodes(id);
        let open = self.open_tag(nodes[0], self_closing, Sep::Done, false);
        if self_closing {
            return open;
        }
        let close_id = *nodes.last().expect("a close tag");
        let children = &nodes[1..nodes.len() - 1];
        let close_leaf = self.first_leaf(close_id);
        if children.is_empty() {
            if !self.gap_has_comments(close_leaf) {
                return Doc::concat([open, self.close_tag(close_id, Sep::Tight)]);
            }
            let inner = self.comments_here(false);
            return Doc::concat([
                open,
                Doc::indent(inner),
                self.close_tag(close_id, Sep::Hard),
            ]);
        }
        let simple = children.len() == 1
            && matches!(
                self.tree.node(children[0]).kind,
                NodeKind::Text | NodeKind::ExprChild(_)
            )
            && !self.gap_has_comments(self.first_leaf(children[0]))
            && !self.gap_has_comments(close_leaf);
        if simple {
            let child = self.node(children[0], Sep::SoftLine, false);
            let close = self.close_tag(close_id, Sep::SoftLine);
            return Doc::group(Doc::concat([open, Doc::indent(child), close]));
        }
        let body = self.siblings(children);
        let close = self.close_tag(close_id, Sep::Hard);
        Doc::concat([open, Doc::indent(body), close])
    }

    /// Children one per line, keeping single blank lines between them,
    /// then the comments that end the list.
    fn siblings(&mut self, children: &[NodeId]) -> Doc {
        let mut parts = Vec::new();
        for (i, &child) in children.iter().enumerate() {
            parts.push(self.node(child, Sep::Hard, i > 0));
        }
        parts.push(self.comments_here(true));
        Doc::concat(parts)
    }

    fn open_tag(&mut self, id: NodeId, self_closing: bool, lead: Sep, list: bool) -> Doc {
        let mut head = vec![self.token(lead, list)];
        let mut attrs = Vec::new();
        for child in self.child_nodes(id) {
            let kind = self.tree.node(child).kind;
            match kind {
                NodeKind::TagName(_) => head.push(self.tag_name(child)),
                NodeKind::CtorArgs => {
                    // Without attributes the arguments break with the tag,
                    // so a long call does not leave `/>` alone instead.
                    let has_attrs = self
                        .child_nodes(id)
                        .iter()
                        .any(|&c| self.tree.node(c).kind.is_attr());
                    head.push(self.ctor_args(child, has_attrs));
                }
                _ => {
                    let list = !attrs.is_empty();
                    attrs.push(self.attr(child, Sep::Line, list));
                }
            }
        }
        if head.len() == 1 {
            // `<>`: no name, no attributes.
            head.push(self.token(Sep::Tight, false));
            return Doc::concat(head);
        }
        attrs.push(self.comments_here(true));
        head.push(Doc::indent(Doc::concat(attrs)));
        if self_closing {
            head.push(self.token(Sep::Line, false));
            head.push(self.token(Sep::Tight, false));
        } else {
            head.push(self.token(Sep::SoftLine, false));
        }
        Doc::group(Doc::concat(head))
    }

    fn tag_name(&mut self, id: NodeId) -> Doc {
        let mut parts = Vec::new();
        for child in self.children(id) {
            parts.push(match child {
                SyntaxElement::Rust(_) => self.embed(Sep::Tight, false, 1, RustContext::Expr),
                _ => self.token(Sep::Tight, false),
            });
        }
        Doc::concat(parts)
    }

    /// `(a, b)`: one line, or one argument per line, commas as written.
    fn ctor_args(&mut self, id: NodeId, grouped: bool) -> Doc {
        let children = self.children(id);
        let open = self.token(Sep::Tight, false);
        if children.len() == 2 {
            let close = self.token(Sep::Tight, false);
            return Doc::concat([open, close]);
        }
        let mut inner = Vec::new();
        let mut after_comma = false;
        for child in &children[1..children.len() - 1] {
            match child {
                SyntaxElement::Rust(_) => {
                    let sep = if after_comma {
                        Sep::Line
                    } else {
                        Sep::SoftLine
                    };
                    inner.push(self.embed(sep, false, 1, RustContext::Expr));
                }
                _ => {
                    inner.push(self.token(Sep::Tight, false));
                    after_comma = true;
                }
            }
        }
        inner.push(self.comments_here(false));
        let close = self.token(Sep::SoftLine, false);
        let args = Doc::concat([open, Doc::indent(Doc::concat(inner)), close]);
        if grouped { Doc::group(args) } else { args }
    }

    fn close_tag(&mut self, id: NodeId, lead: Sep) -> Doc {
        let count = self.tree.leaves(id).count();
        let mut parts = vec![self.token(lead, false)];
        for _ in 1..count {
            parts.push(self.token(Sep::Tight, false));
        }
        Doc::concat(parts)
    }

    fn attr(&mut self, id: NodeId, lead: Sep, list: bool) -> Doc {
        match self.tree.node(id).kind {
            NodeKind::AttrWhen => {
                // `@when {cond} { attrs }`
                let mut parts = vec![self.token(lead, list), self.token(Sep::Tight, false)];
                parts.push(self.token(Sep::Space, false));
                parts.push(self.embed(Sep::Tight, false, 1, RustContext::Expr));
                parts.push(self.token(Sep::Tight, false));
                let list_id = *self.child_nodes(id).last().expect("an attribute list");
                parts.push(self.attr_list(list_id));
                Doc::concat(parts)
            }
            NodeKind::AttrFor => {
                // `@for pat in iter { attrs }`
                let mut parts = vec![self.token(lead, list), self.token(Sep::Tight, false)];
                parts.push(self.embed(Sep::Space, false, 3, RustContext::ForHeader));
                let list_id = *self.child_nodes(id).last().expect("an attribute list");
                parts.push(self.attr_list(list_id));
                Doc::concat(parts)
            }
            _ => {
                let mut parts = Vec::new();
                for child in self.children(id) {
                    match child {
                        SyntaxElement::Node(name) => {
                            for i in 0..self.tree.leaves(name).count() {
                                let sep = if i == 0 { lead } else { Sep::Keep };
                                parts.push(self.token(sep, list && i == 0));
                            }
                        }
                        SyntaxElement::Rust(_) => {
                            parts.push(self.embed(Sep::Tight, false, 1, RustContext::Expr));
                        }
                        SyntaxElement::Token(_) => parts.push(self.token(Sep::Tight, false)),
                    }
                }
                Doc::concat(parts)
            }
        }
    }

    /// `{ attrs }` of an attribute group: one line if it fits.
    fn attr_list(&mut self, id: NodeId) -> Doc {
        let open = self.token(Sep::Space, false);
        let attrs = self.child_nodes(id);
        let close_leaf = self.first_leaf(id) + self.tree.leaves(id).count() - 1;
        if attrs.is_empty() && !self.gap_has_comments(close_leaf) {
            let close = self.token(Sep::Tight, false);
            return Doc::concat([open, close]);
        }
        let mut inner = Vec::new();
        for (i, a) in attrs.iter().enumerate() {
            inner.push(self.attr(*a, Sep::Line, i > 0));
        }
        inner.push(self.comments_here(true));
        let close = self.token(Sep::Line, false);
        Doc::group(Doc::concat([open, Doc::indent(Doc::concat(inner)), close]))
    }

    /// `{ children }` of a branch, loop, or match arm: `{}` when empty,
    /// otherwise always one child per line.
    fn child_list(&mut self, id: NodeId, lead: Sep, list: bool) -> Doc {
        let open = self.token(lead, list);
        let children = self.child_nodes(id);
        let close_leaf = self.first_leaf(id) + self.tree.leaves(id).count() - 1;
        if children.is_empty() && !self.gap_has_comments(close_leaf) {
            let close = self.token(Sep::Tight, false);
            return Doc::concat([open, close]);
        }
        let body = self.siblings(&children);
        let close = self.token(Sep::Hard, false);
        Doc::concat([open, Doc::indent(body), close])
    }

    fn if_node(&mut self, id: NodeId, lead: Sep, list: bool) -> Doc {
        let mut parts = vec![self.token(lead, list)];
        parts.push(self.embed(Sep::Space, false, 1, RustContext::Condition));
        for child in self.children(id).into_iter().skip(2) {
            match child {
                SyntaxElement::Node(n) => parts.push(self.node(n, Sep::Space, false)),
                // `else`
                _ => parts.push(self.token(Sep::Space, false)),
            }
        }
        Doc::concat(parts)
    }

    fn for_node(&mut self, id: NodeId, lead: Sep, list: bool) -> Doc {
        let mut parts = vec![self.token(lead, list)];
        parts.push(self.embed(Sep::Space, false, 3, RustContext::ForHeader));
        for child in self.child_nodes(id) {
            match self.tree.node(child).kind {
                NodeKind::ForKey => {
                    // `key={expr}`
                    parts.push(self.token(Sep::Space, false));
                    parts.push(self.token(Sep::Tight, false));
                    parts.push(self.token(Sep::Tight, false));
                    parts.push(self.embed(Sep::Tight, false, 1, RustContext::Expr));
                    parts.push(self.token(Sep::Tight, false));
                }
                _ => parts.push(self.child_list(child, Sep::Space, false)),
            }
        }
        Doc::concat(parts)
    }

    fn match_node(&mut self, id: NodeId, lead: Sep, list: bool) -> Doc {
        let mut parts = vec![self.token(lead, list)];
        parts.push(self.embed(Sep::Space, false, 1, RustContext::Scrutinee));
        parts.push(self.token(Sep::Space, false));
        let arms = self.child_nodes(id);
        let close_leaf = self.first_leaf(id) + self.tree.leaves(id).count() - 1;
        if arms.is_empty() && !self.gap_has_comments(close_leaf) {
            parts.push(self.token(Sep::Tight, false));
            return Doc::concat(parts);
        }
        let mut body = Vec::new();
        for (i, arm) in arms.iter().enumerate() {
            body.push(self.arm(*arm, i > 0));
        }
        body.push(self.comments_here(true));
        parts.push(Doc::indent(Doc::concat(body)));
        parts.push(self.token(Sep::Hard, false));
        Doc::concat(parts)
    }

    /// `pat [if guard] => body [,]`, body forms and commas as written.
    fn arm(&mut self, id: NodeId, list: bool) -> Doc {
        let children = self.children(id);
        let guarded = matches!(
            children.get(1),
            Some(SyntaxElement::Token(t)) if t.kind == TokenKind::Keyword("if")
        );
        let head_leaves = if guarded { 3 } else { 1 };
        let mut parts = vec![self.embed(Sep::Hard, list, head_leaves, RustContext::ArmHead)];
        // `=>`
        parts.push(self.token(Sep::Space, false));
        for child in children.into_iter().skip(head_leaves + 1) {
            match child {
                SyntaxElement::Node(n)
                    if matches!(
                        self.tree.node(n).kind,
                        NodeKind::Element(_) | NodeKind::Fragment
                    ) =>
                {
                    // A long node moves below `=>` instead of breaking
                    // its tag; one that spans lines anyway stays.
                    let pre = self.lead(Sep::Tight, false);
                    let body = self.node(n, Sep::Done, false);
                    parts.push(Doc::concat([pre, Doc::Hang(Box::new(body))]));
                }
                SyntaxElement::Node(n) => parts.push(self.node(n, Sep::Space, false)),
                // Commas after the body.
                _ => parts.push(self.token(Sep::Tight, false)),
            }
        }
        Doc::concat(parts)
    }
}

fn gap(src: &str, range: Range<usize>) -> Result<Gap, MapError> {
    trivia::gap(src, range).map_err(|offset| MapError {
        offset,
        message: "source the view parser did not record".to_owned(),
    })
}
