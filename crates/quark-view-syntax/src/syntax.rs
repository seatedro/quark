//! Concrete syntax for tools that rewrite `view!` source, such as the
//! formatter.
//!
//! [`crate::parse_with_syntax`] records, next to the [`ViewInput`], a
//! [`SyntaxTree`]: every token the grammar consumed, in source order,
//! grouped into nodes that follow the grammar. The tree keeps what the AST
//! drops: delimiters, punctuation, the written closing name, `</>`, commas,
//! `{?`/`{...` markers, and whether a tag closed with `/>`. Rust the
//! grammar embeds (expressions, patterns, types, `let` statements) is one
//! opaque [`RustFragment`] leaf holding its tokens.
//!
//! The leaves cover the input: between two consecutive leaves there is
//! only whitespace and comments. Leaves store spans; outside a procedural
//! macro, with the `locations` feature, `SpanRange::byte_range` resolves
//! them against the parsed source.
//!
//! The AST link is positional. The [`NodeKind::is_child`] nodes inside an
//! element, fragment, or [`NodeKind::ChildList`] correspond one to one, in
//! order, to the AST's child [`Node`]s, and [`NodeKind::Attr`] nodes to its
//! [`Attr`]s.
//!
//! [`ViewInput`]: crate::ast::ViewInput
//! [`Node`]: crate::ast::Node
//! [`Attr`]: crate::ast::Attr

use std::fmt;

use proc_macro2::{Delimiter, Span, TokenStream};
use syn::buffer::Cursor;

/// Index of a node in its [`SyntaxTree`]. Ids follow source order (a parent
/// before its children), so the same input always gets the same ids.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(u32);

impl NodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

/// The first and last token of a leaf or node. Kept as two spans because
/// `Span::join` only works on nightly inside a procedural macro.
#[derive(Clone, Copy, Debug)]
pub struct SpanRange {
    pub first: Span,
    pub last: Span,
}

impl SpanRange {
    pub fn single(span: Span) -> Self {
        SpanRange {
            first: span,
            last: span,
        }
    }

    /// Byte offsets in the source the tokens were parsed from, from the
    /// start of the first token to the end of the last.
    #[cfg(feature = "locations")]
    pub fn byte_range(&self) -> std::ops::Range<usize> {
        self.first.byte_range().start..self.last.byte_range().end
    }
}

/// What a node is. Payloads say which spelling the source used where the
/// grammar accepts more than one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    /// The whole input: optional headers, then the root child.
    View,
    /// `scale,`: the ident and the comma.
    ScaleHeader,
    /// `-> Type,`: the arrow, the type, and the comma.
    TypedHeader,
    /// An element: its [`NodeKind::OpenTag`], then for the paired form its
    /// children and [`NodeKind::CloseTag`].
    Element(ElementForm),
    /// `<>children</>`: an open tag of `<` `>`, children, and a close tag
    /// of `<` `/` `>`. (`<fragment>` is an [`NodeKind::Element`].)
    Fragment,
    /// `<`, the [`NodeKind::TagName`], optional [`NodeKind::CtorArgs`],
    /// attributes, then `>` or `/` `>`.
    OpenTag,
    /// The tag name as written: idents with `::` between them, `.` and an
    /// ident for a slot, or `{` expression `}` for a value tag.
    TagName(TagKind),
    /// `(` arguments `)`, with each argument a fragment and each comma,
    /// trailing one included, a token.
    CtorArgs,
    /// `<` `/`, the closing name as written (possibly just the last path
    /// segment, or nothing for `</>`), then `>`.
    CloseTag,
    /// A braced Rust child, or a match arm's bare expression body.
    ExprChild(ExprMarker),
    /// A string literal child.
    Text,
    /// `if` condition, a [`NodeKind::ChildList`], and optionally `else`
    /// followed by another `If` or a `ChildList`.
    If,
    /// `for` pattern `in` iterator, an optional [`NodeKind::ForKey`], and a
    /// [`NodeKind::ChildList`].
    For,
    /// `key` `=` `{` expression `}` on a loop.
    ForKey,
    /// `match` scrutinee `{` arms `}`.
    Match,
    /// Pattern, optional `if` guard, `=>`, the body (an element or
    /// fragment, a [`NodeKind::ChildList`], or an
    /// [`ExprMarker::Bare`] expression), then any commas.
    MatchArm,
    /// A `let` statement child: one [`RustContext::Local`] fragment,
    /// semicolon included.
    Let,
    /// `{` children `}`: the body of a branch, loop, or match arm.
    ChildList,
    /// An attribute.
    Attr(AttrForm),
    /// The attribute name as written: idents with `-` or `:` between them,
    /// and for `on:key:` the binding tokens.
    AttrName,
    /// `@` `when` `{` condition `}` and a [`NodeKind::AttrList`].
    AttrWhen,
    /// `@` `for` pattern `in` iterator and a [`NodeKind::AttrList`].
    AttrFor,
    /// `{` attributes `}` of an attribute group.
    AttrList,
}

impl NodeKind {
    /// Whether this node is a child of an element, fragment, or child list
    /// (an AST `Node`).
    pub fn is_child(self) -> bool {
        matches!(
            self,
            NodeKind::Element(_)
                | NodeKind::Fragment
                | NodeKind::ExprChild(_)
                | NodeKind::Text
                | NodeKind::If
                | NodeKind::For
                | NodeKind::Match
                | NodeKind::Let
        )
    }

    /// Whether this node is an attribute (an AST `Attr`).
    pub fn is_attr(self) -> bool {
        matches!(
            self,
            NodeKind::Attr(_) | NodeKind::AttrWhen | NodeKind::AttrFor
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ElementForm {
    /// `<tag ... />`
    SelfClosing,
    /// `<tag ...>...</tag>`, even with no children.
    Paired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagKind {
    Builtin,
    Component,
    Function,
    Slot,
    Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExprMarker {
    /// `{expr}`
    Plain,
    /// `{?expr}`
    Optional,
    /// `{...expr}`
    Spread,
    /// A match arm body with no braces: just the fragment.
    Bare,
}

/// How an attribute was written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttrForm {
    /// `name`, no value.
    Flag,
    /// `name="lit"`, `name=12`, or `name=-12`: syn reads a negative number
    /// as one literal, recorded as one token from `-` to the digits.
    Literal,
    /// `name=-lit` that syn does not read as one literal, such as
    /// `name=-true`: the `-` and the literal are separate tokens.
    NegativeLiteral,
    /// `name={expr}`
    Expr,
    /// `name={@expr}`
    Reactive,
    /// `name={if ..}`: the whole `if` is one expression fragment.
    If,
    /// `class="..."`
    Class,
}

/// A token the grammar itself consumed.
#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokenKind,
    pub range: SpanRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    /// Punctuation, spelled as written. `::`, `...`, `->`, and `=>` are one
    /// token; `</` and `/>` are two.
    Punct(&'static str),
    /// `if`, `else`, `for`, `in`, `match`, and the contextual `when` and
    /// `key`.
    Keyword(&'static str),
    /// A name: tag, attribute segment, event, or the `scale` ident.
    Ident,
    /// A literal: text, class, or attribute value.
    Literal,
    /// One token tree of an `on:key:` binding, such as `mod`, `+`, `s`, or
    /// `"mod+s"`.
    Binding,
    Open(Delimiter),
    Close(Delimiter),
}

/// What kind of Rust a fragment is, which decides how a tool can wrap it to
/// reparse or format it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RustContext {
    /// A complete expression: braced children and attribute values,
    /// constructor arguments, keys, guards, and bare match arm bodies.
    Expr,
    /// An `if` condition, parsed without struct literals; may hold `let`
    /// and let chains.
    Condition,
    /// A `for` or `@for` iterator, parsed without struct literals.
    Iterator,
    /// A `match` scrutinee, parsed without struct literals.
    Scrutinee,
    /// A pattern with an optional leading `|`: loop bindings and arms.
    Pattern,
    /// The type of `-> Type,`.
    Type,
    /// A whole `let` statement, `let` through `;`.
    Local,
}

/// Rust embedded in the template, kept as its own tokens with their spans.
#[derive(Clone, Debug)]
pub struct RustFragment {
    pub context: RustContext,
    pub tokens: TokenStream,
    pub range: SpanRange,
}

#[derive(Clone, Debug)]
pub enum SyntaxElement {
    Node(NodeId),
    Token(Token),
    Rust(RustFragment),
}

#[derive(Clone, Debug)]
pub struct SyntaxNode {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// Tokens, fragments, and child nodes in source order.
    pub children: Vec<SyntaxElement>,
}

/// A token or fragment: the parts of a [`SyntaxTree`] that own source text.
#[derive(Clone, Copy, Debug)]
pub enum Leaf<'a> {
    Token(&'a Token),
    Rust(&'a RustFragment),
}

impl Leaf<'_> {
    pub fn range(&self) -> SpanRange {
        match self {
            Leaf::Token(t) => t.range,
            Leaf::Rust(r) => r.range,
        }
    }
}

/// The concrete syntax of one `view!` input. Node 0 is the
/// [`NodeKind::View`] root.
#[derive(Clone, Debug)]
pub struct SyntaxTree {
    nodes: Vec<SyntaxNode>,
}

impl SyntaxTree {
    pub fn root(&self) -> NodeId {
        NodeId(0)
    }

    pub fn node(&self, id: NodeId) -> &SyntaxNode {
        &self.nodes[id.index()]
    }

    /// Every node, in id order.
    pub fn nodes(&self) -> impl Iterator<Item = (NodeId, &SyntaxNode)> {
        self.nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (NodeId(i as u32), n))
    }

    /// The leaves under `id`, in source order.
    pub fn leaves(&self, id: NodeId) -> impl Iterator<Item = Leaf<'_>> {
        let mut stack = vec![self.node(id).children.iter()];
        std::iter::from_fn(move || {
            loop {
                let next = stack.last_mut()?.next();
                match next {
                    None => {
                        stack.pop();
                    }
                    Some(SyntaxElement::Node(child)) => {
                        stack.push(self.node(*child).children.iter())
                    }
                    Some(SyntaxElement::Token(t)) => return Some(Leaf::Token(t)),
                    Some(SyntaxElement::Rust(r)) => return Some(Leaf::Rust(r)),
                }
            }
        })
    }

    /// From the first leaf under `id` to its last. Every node has a leaf.
    pub fn range(&self, id: NodeId) -> SpanRange {
        let mut leaves = self.leaves(id);
        let first = leaves.next().expect("node without leaves").range();
        let last = leaves.last().map_or(first, |l| l.range());
        SpanRange {
            first: first.first,
            last: last.last,
        }
    }

    /// Checks that the nodes form one tree rooted at the
    /// [`NodeKind::View`], with parent links that match, ids in source
    /// order, and a leaf under every node; with `locations`, also that the
    /// leaves' byte ranges are ordered and do not overlap.
    pub fn verify_integrity(&self) -> Result<(), IntegrityError> {
        let fail = |msg: String| Err(IntegrityError(msg));
        let Some(root) = self.nodes.first() else {
            return fail("no root node".into());
        };
        if root.kind != NodeKind::View || root.parent.is_some() {
            return fail(format!(
                "root is {:?} with parent {:?}",
                root.kind, root.parent
            ));
        }
        let mut seen = vec![false; self.nodes.len()];
        seen[0] = true;
        for (id, node) in self.nodes() {
            if node.children.is_empty() {
                return fail(format!("{id:?} ({:?}) has no children", node.kind));
            }
            for child in &node.children {
                let SyntaxElement::Node(c) = child else {
                    continue;
                };
                if *c <= id || c.index() >= self.nodes.len() {
                    return fail(format!("{id:?} has child {c:?} out of source order"));
                }
                if std::mem::replace(&mut seen[c.index()], true) {
                    return fail(format!("{c:?} has two parents"));
                }
                if self.nodes[c.index()].parent != Some(id) {
                    return fail(format!("{c:?} does not point back to its parent {id:?}"));
                }
            }
        }
        if let Some(orphan) = seen.iter().position(|s| !s) {
            return fail(format!("NodeId({orphan}) is not in the tree"));
        }
        #[cfg(feature = "locations")]
        {
            let mut end = 0;
            for leaf in self.leaves(self.root()) {
                let range = leaf.range().byte_range();
                if range.start < end || range.end < range.start {
                    return fail(format!("leaf at {range:?} overlaps or precedes byte {end}"));
                }
                end = range.end;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IntegrityError(pub String);

impl fmt::Display for IntegrityError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for IntegrityError {}

/// Collects a [`SyntaxTree`] while the parser runs, or does nothing when
/// the macro parses (`Recorder::off`), so expansion pays no allocation.
pub(crate) struct Recorder {
    tree: Option<Builder>,
}

struct Builder {
    nodes: Vec<SyntaxNode>,
    open: Vec<NodeId>,
}

impl Recorder {
    pub(crate) fn off() -> Self {
        Recorder { tree: None }
    }

    pub(crate) fn on() -> Self {
        Recorder {
            tree: Some(Builder {
                nodes: Vec::new(),
                open: Vec::new(),
            }),
        }
    }

    /// Opens a node as the last child of the open one. Close it with
    /// [`Recorder::finish`] once its last token is recorded.
    pub(crate) fn start(&mut self, kind: NodeKind) -> NodeId {
        let Some(b) = &mut self.tree else {
            return NodeId(0);
        };
        let id = NodeId(b.nodes.len() as u32);
        let parent = b.open.last().copied();
        if let Some(p) = parent {
            b.nodes[p.index()].children.push(SyntaxElement::Node(id));
        }
        b.nodes.push(SyntaxNode {
            kind,
            parent,
            children: Vec::new(),
        });
        b.open.push(id);
        id
    }

    pub(crate) fn finish(&mut self) {
        if let Some(b) = &mut self.tree {
            b.open.pop();
        }
    }

    /// Replaces the kind of `id`, for spellings known only after parsing
    /// past the node's start.
    pub(crate) fn set_kind(&mut self, id: NodeId, kind: NodeKind) {
        if let Some(b) = &mut self.tree {
            b.nodes[id.index()].kind = kind;
        }
    }

    pub(crate) fn token(&mut self, kind: TokenKind, span: Span) {
        self.tokens(kind, SpanRange::single(span));
    }

    /// A token made of several characters, such as `::` or `->`.
    pub(crate) fn tokens(&mut self, kind: TokenKind, range: SpanRange) {
        self.push(SyntaxElement::Token(Token { kind, range }));
    }

    /// The tokens parsed between `begin` and `end` in one buffer, as one
    /// fragment.
    pub(crate) fn rust(&mut self, context: RustContext, begin: Cursor, end: Cursor) {
        if self.tree.is_none() {
            return;
        }
        if let Some((tokens, range)) = consumed(begin, end) {
            self.push(SyntaxElement::Rust(RustFragment {
                context,
                tokens,
                range,
            }));
        }
    }

    fn push(&mut self, element: SyntaxElement) {
        if let Some(b) = &mut self.tree {
            let open = *b.open.last().expect("token outside a node");
            b.nodes[open.index()].children.push(element);
        }
    }

    pub(crate) fn into_tree(self) -> Option<SyntaxTree> {
        self.tree.map(|b| SyntaxTree { nodes: b.nodes })
    }
}

fn consumed(begin: Cursor, end: Cursor) -> Option<(TokenStream, SpanRange)> {
    let mut tokens = TokenStream::new();
    let mut range: Option<SpanRange> = None;
    let mut cursor = begin;
    while cursor != end {
        let Some((tt, next)) = cursor.token_tree() else {
            break;
        };
        let span = tt.span();
        range = Some(match range {
            None => SpanRange::single(span),
            Some(r) => SpanRange {
                first: r.first,
                last: span,
            },
        });
        tokens.extend([tt]);
        cursor = next;
    }
    Some((tokens, range?))
}
