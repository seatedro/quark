//! The concrete syntax tree against real source: each case parses `view!`
//! input and prints the tree with every leaf as the exact source bytes its
//! span covers, so a wrong boundary, a dropped token, or a lost spelling
//! shows up as different text. Every case also checks that the bytes
//! between leaves are only whitespace and comments.

use proc_macro2::TokenStream;
use quark_view_syntax::ast::{Node, ViewInput};
use quark_view_syntax::parse_with_syntax;
use quark_view_syntax::syntax::{NodeId, NodeKind, SyntaxElement, SyntaxTree};
use syn::parse::Parser;

fn parse(src: &str) -> (ViewInput, SyntaxTree) {
    let tokens: TokenStream = src.parse().expect("lexes");
    let (view, tree) = parse_with_syntax.parse2(tokens).expect("parses");
    tree.verify_integrity().expect("integrity");
    assert_gaps_are_trivia(src, &tree);
    (view, tree)
}

/// `(Kind leaf leaf (Kind ...))`, tokens as their source text and Rust
/// fragments as `[Context source]`.
fn dump(src: &str) -> String {
    let (_, tree) = parse(src);
    let mut out = String::new();
    write_node(&tree, tree.root(), src, &mut out);
    out
}

fn write_node(tree: &SyntaxTree, id: NodeId, src: &str, out: &mut String) {
    let node = tree.node(id);
    out.push_str(&format!("({:?}", node.kind));
    for child in &node.children {
        out.push(' ');
        match child {
            SyntaxElement::Node(c) => write_node(tree, *c, src, out),
            SyntaxElement::Token(t) => out.push_str(&src[t.range.byte_range()]),
            SyntaxElement::Rust(r) => {
                out.push_str(&format!("[{:?} {}]", r.context, &src[r.range.byte_range()]))
            }
        }
    }
    out.push(')');
}

/// Every byte belongs to a leaf or is whitespace or a comment, so a
/// formatter that keeps the leaves and the comments loses nothing.
fn assert_gaps_are_trivia(src: &str, tree: &SyntaxTree) {
    let mut end = 0;
    let mut gaps = Vec::new();
    for leaf in tree.leaves(tree.root()) {
        let range = leaf.range().byte_range();
        gaps.push(&src[end..range.start]);
        end = range.end;
    }
    gaps.push(&src[end..]);
    for gap in gaps {
        assert!(
            strip_comments(gap).trim().is_empty(),
            "unrecorded source {gap:?} in {src:?}"
        );
    }
}

fn strip_comments(gap: &str) -> String {
    let mut out = String::new();
    let mut rest = gap;
    while !rest.is_empty() {
        if let Some(line) = rest.strip_prefix("//") {
            rest = line.find('\n').map_or("", |i| &line[i..]);
        } else if rest.starts_with("/*") {
            let (mut depth, mut i) = (0, 0);
            while i < rest.len() {
                if rest[i..].starts_with("/*") {
                    depth += 1;
                    i += 2;
                } else if rest[i..].starts_with("*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += rest[i..].chars().next().map_or(1, char::len_utf8);
                }
            }
            rest = &rest[i..];
        } else {
            let c = rest.chars().next().unwrap();
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

#[test]
fn source_boundaries() {
    let cases = [
        (
            "headers keep `scale,` and `-> Type,` with their commas",
            "scale, -> Div<'a>, <div />",
            "(View (ScaleHeader scale ,) (TypedHeader -> [Type Div<'a>] ,) \
             (Element(SelfClosing) (OpenTag < (TagName(Builtin) div) / >)))",
        ),
        (
            "a paired empty element stays paired and a fragment keeps `<>` and `</>`",
            "<><div></div><spacer/></>",
            "(View (Fragment (OpenTag < >) \
             (Element(Paired) (OpenTag < (TagName(Builtin) div) >) (CloseTag < / div >)) \
             (Element(SelfClosing) (OpenTag < (TagName(Builtin) spacer) / >)) \
             (CloseTag < / >)))",
        ),
        (
            "a component closed by its last segment keeps that short name",
            "<widgets::Button></Button>",
            "(View (Element(Paired) (OpenTag < (TagName(Component) widgets :: Button) >) \
             (CloseTag < / Button >)))",
        ),
        (
            "a value tag keeps its braces and closes with `</>`",
            "<{self.input} focused={f}></>",
            "(View (Element(Paired) (OpenTag < (TagName(Value) { [Expr self.input] }) \
             (Attr(Expr) (AttrName focused) = { [Expr f] }) >) (CloseTag < / >)))",
        ),
        (
            "a slot tag keeps its dot in both names",
            "<.title>\"x\"</.title>",
            "(View (Element(Paired) (OpenTag < (TagName(Slot) . title) >) (Text \"x\") \
             (CloseTag < / . title >)))",
        ),
        (
            "constructor arguments are separate fragments with every comma kept",
            "<nav_row(p, view! { <div /> }, \"x\",) />",
            "(View (Element(SelfClosing) (OpenTag < (TagName(Function) nav_row) \
             (CtorArgs ( [Expr p] , [Expr view! { <div /> }] , [Expr \"x\"] , )) / >)))",
        ),
        (
            "attribute names keep kebab, event, and key binding spellings",
            "<div aria-label=\"a\" on:click={go} on:key:mod+s={save} on:key:\"cmd+k\"={k} />",
            "(View (Element(SelfClosing) (OpenTag < (TagName(Builtin) div) \
             (Attr(Literal) (AttrName aria - label) = \"a\") \
             (Attr(Expr) (AttrName on : click) = { [Expr go] }) \
             (Attr(Expr) (AttrName on : key : mod + s) = { [Expr save] }) \
             (Attr(Expr) (AttrName on : key : \"cmd+k\") = { [Expr k] }) / >)))",
        ),
        (
            "attribute values keep flags, literal signs, `@`, and an `if` without `else`",
            "<div hidden w=12 dx=- 1.5 neg=-true h={@sig} bg={if on { a }} class=\"p-4 px-2\" />",
            "(View (Element(SelfClosing) (OpenTag < (TagName(Builtin) div) \
             (Attr(Flag) (AttrName hidden)) (Attr(Literal) (AttrName w) = 12) \
             (Attr(Literal) (AttrName dx) = - 1.5) \
             (Attr(NegativeLiteral) (AttrName neg) = - true) \
             (Attr(Reactive) (AttrName h) = { @ [Expr sig] }) \
             (Attr(If) (AttrName bg) = { [Expr if on { a }] }) \
             (Attr(Class) (AttrName class) = \"p-4 px-2\") / >)))",
        ),
        (
            "attribute groups keep their condition, loop header, and nested braces",
            "<div @when {on} { class=\"a\" b } @for (k, v) in KEYS { x={k} } />",
            "(View (Element(SelfClosing) (OpenTag < (TagName(Builtin) div) \
             (AttrWhen @ when { [Expr on] } (AttrList { (Attr(Class) (AttrName class) = \"a\") \
             (Attr(Flag) (AttrName b)) })) \
             (AttrFor @ for [Pattern (k, v)] in [Iterator KEYS] (AttrList { \
             (Attr(Expr) (AttrName x) = { [Expr k] }) })) / >)))",
        ),
        (
            "braced children keep their `?` and `...` markers",
            "<div>{x} {?maybe} {...items} \"text\"</div>",
            "(View (Element(Paired) (OpenTag < (TagName(Builtin) div) >) \
             (ExprChild(Plain) { [Expr x] }) (ExprChild(Optional) { ? [Expr maybe] }) \
             (ExprChild(Spread) { ... [Expr items] }) (Text \"text\") (CloseTag < / div >)))",
        ),
        (
            "`if let` chains, `else if`, and an empty `else` keep their braces",
            "if let Some(a) = a && b { \"a\" } else if c { <br /> } else {}",
            "(View (If if [Condition let Some(a) = a && b] (ChildList { (Text \"a\") }) else \
             (If if [Condition c] (ChildList { (Element(SelfClosing) (OpenTag < \
             (TagName(Builtin) br) / >)) }) else (ChildList { }))))",
        ),
        (
            "a keyed loop keeps its key, and a `let` child its semicolon",
            "for (i, item) in items.iter().enumerate() key={item.id} { let k: u8 = i; <row /> }",
            "(View (For for [Pattern (i, item)] in [Iterator items.iter().enumerate()] \
             (ForKey key = { [Expr item.id] }) (ChildList { (Let [Local let k: u8 = i;]) \
             (Element(SelfClosing) (OpenTag < (TagName(Builtin) row) / >)) })))",
        ),
        (
            "match arms keep guards, braces, and exactly the commas written",
            "match x { A => <a />, B if g => { \"b\" } C => c, D => d }",
            "(View (Match match [Scrutinee x] { \
             (MatchArm [Pattern A] => (Element(SelfClosing) (OpenTag < (TagName(Builtin) a) / >)) ,) \
             (MatchArm [Pattern B] if [Expr g] => (ChildList { (Text \"b\") })) \
             (MatchArm [Pattern C] => (ExprChild(Bare) [Expr c]) ,) \
             (MatchArm [Pattern D] => (ExprChild(Bare) [Expr d])) }))",
        ),
        (
            "boundaries are byte offsets past multibyte text, CRLF, and comments",
            "<text>\"héllo ✓\"\r\n/* é */ {x} // ✓\r\n</text>",
            "(View (Element(Paired) (OpenTag < (TagName(Builtin) text) >) (Text \"héllo ✓\") \
             (ExprChild(Plain) { [Expr x] }) (CloseTag < / text >)))",
        ),
    ];
    for (name, src, expected) in cases {
        assert_eq!(dump(src), expected, "{name}");
    }
}

/// The tree's child and attribute nodes line up, in order, with the AST's
/// children and attributes, which is how a tool finds the syntax of an AST
/// node.
#[test]
fn child_nodes_line_up_with_the_ast() {
    let src = "<div a @when {c} { b }>\
               {x} \"t\" let y = 1; <>{?o}</> if c { <p /> } \
               for i in xs { {i} } match m { A => a, B => <b /> }</div>";
    let (view, tree) = parse(src);
    let Node::Element(root) = &view.root else {
        panic!("root is an element")
    };
    let root_id = tree
        .nodes()
        .find(|(_, n)| matches!(n.kind, NodeKind::Element(_)))
        .map(|(id, _)| id)
        .unwrap();
    let ast: Vec<&str> = root.children.iter().map(ast_kind).collect();
    let syntax: Vec<String> = child_kinds(&tree, root_id, NodeKind::is_child);
    assert_eq!(
        ast,
        ["expr", "text", "let", "fragment", "if", "for", "match"]
    );
    assert_eq!(
        syntax,
        [
            "ExprChild(Plain)",
            "Text",
            "Let",
            "Fragment",
            "If",
            "For",
            "Match"
        ]
    );
    let open_tag = child_ids(&tree, root_id)[0];
    assert_eq!(
        child_kinds(&tree, open_tag, NodeKind::is_attr).len(),
        root.attrs.len()
    );
}

fn ast_kind(node: &Node) -> &'static str {
    match node {
        Node::Element(_) => "element",
        Node::Fragment(_) => "fragment",
        Node::Expr(_) => "expr",
        Node::OptionalExpr(_) => "optional",
        Node::SpreadExpr(_) => "spread",
        Node::Text(_) => "text",
        Node::If(_) => "if",
        Node::For(_) => "for",
        Node::Match(_) => "match",
        Node::Let(_) => "let",
    }
}

fn child_ids(tree: &SyntaxTree, id: NodeId) -> Vec<NodeId> {
    tree.node(id)
        .children
        .iter()
        .filter_map(|c| match c {
            SyntaxElement::Node(c) => Some(*c),
            _ => None,
        })
        .collect()
}

fn child_kinds(tree: &SyntaxTree, id: NodeId, keep: fn(NodeKind) -> bool) -> Vec<String> {
    child_ids(tree, id)
        .into_iter()
        .map(|c| tree.node(c).kind)
        .filter(|k| keep(*k))
        .map(|k| format!("{k:?}"))
        .collect()
}
