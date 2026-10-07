use super::*;

#[test]
fn blocks_dump_matches_commonmark_structure() {
    let cases = [
        (
            "Hello *world* and **bold _both_**",
            "p: Hello [i:world] and [b:bold ][b,i:both]",
        ),
        ("# One\n## Two\n###### Six", "h1: One\nh2: Two\nh6: Six"),
        (
            "- a\n- b\n  - c\n\n1. x\n2. y",
            "p -: a\np -: b\n  p -: c\np 1.: x\np 2.: y",
        ),
        ("3. three\n4. four", "p 3.: three\np 4.: four"),
        ("- item\n\n  second para", "p -: item\np: second para"),
        ("- [ ] todo\n- [x] done", "p [ ]: todo\np [x]: done"),
        ("> quote\n>> nested", ">p: quote\n>>p: nested"),
        ("> - in quote", ">p -: in quote"),
        (
            "```rust title\nfn main() {}\n\n```",
            "code(rust): [c:fn main() {}\\n]",
        ),
        ("    indented", "code(): [c:indented]"),
        (
            "a ~~gone~~ `code` [link **b**](https://x.y) ![alt](i.png) ![](j.png)",
            "p: a [s:gone] [c:code] [l=https://x.y:link ][b,l=https://x.y:b] [img:[alt]] [img:[image]]",
        ),
        (
            "line one\nline two  \nline three",
            "p: line one\\nline two\\nline three",
        ),
        ("a\n\n---\n\nb", "p: a\nhr: \np: b"),
        (
            "| a | b |\n|---|:-:|\n| 1 | **2** |\n| | x |",
            "table2: r0c0:a r0c1:b r1c0:1 r1c1:[b:2] r2c0: r2c1:x",
        ),
        ("-\n- b", "p -: \np -: b"),
        // An item holding only a sublist keeps its own marker.
        ("- - inner\n- b", "p -: \n  p -: inner\np -: b"),
        ("1. - inner", "p 1.: \n  p -: inner"),
    ];
    for (input, expected) in cases {
        let doc = MarkdownDoc::parse(input);
        assert_eq!(doc.verify_integrity(), Ok(()), "{input:?}");
        assert_eq!(doc.dump(), expected, "{input:?}");
    }
}

#[test]
fn plain_text_keeps_source_markers() {
    let cases = [
        ("## Two *x*", "## Two x"),
        ("- [x] done", "- [x] done"),
        ("7. seventh", "7. seventh"),
        ("> > deep\n> > two", "> > deep\n> > two"),
        ("```\nfn a() {}\nfn b() {}\n```", "fn a() {}\nfn b() {}"),
        (
            "| a | b |\n|---|---|\n| 1 | 2 |",
            "| a | b |\n| --- | --- |\n| 1 | 2 |",
        ),
    ];
    for (input, expected) in cases {
        let doc = MarkdownDoc::parse(input);
        assert_eq!(doc.plain_text(0), expected, "{input:?}");
    }
}

/// A streamed answer: prose, two fenced blocks, a list, more prose.
const STREAMED: &str = "Intro *text*.\n\n```rust\nfn a() {}\n```\n\n- one\n- two\n\n\
                        ~~~~py\nx = 1\n~~~\n~~~~\n\nTail with `code` and [a link](u).";

// Catches a split that changes the parse: every prefix of a streamed
// source, parsed incrementally chunk by chunk, must equal a full parse.
#[test]
fn incremental_parse_of_every_prefix_matches_a_full_parse() {
    let mut parser = IncrementalMarkdown::new();
    for end in (0..=STREAMED.len()).filter(|&i| STREAMED.is_char_boundary(i)) {
        let source = &STREAMED[..end];
        let (incremental, full) = (parser.parse(source), MarkdownDoc::parse(source));
        let hashes = |doc: &MarkdownDoc| (0..doc.len()).map(|b| doc.hash(b)).collect::<Vec<_>>();
        assert_eq!(incremental.verify_integrity(), Ok(()), "{source:?}");
        assert_eq!(incremental.dump(), full.dump(), "{source:?}");
        assert_eq!(hashes(&incremental), hashes(&full), "{source:?}");
    }
}

// Catches the parser reparsing finished output: once a fence is closed and
// a blank line follows, later chunks parse only what comes after.
#[test]
fn incremental_parse_reuses_the_prefix_through_the_last_closed_fence() {
    let cases = [
        ("```rust\nfn a() {}\n```", 0),
        ("```rust\nfn a() {}\n```\n", 0),
        ("```rust\nfn a() {}\n```\n\n", 23),
        ("```rust\nfn a() {}\n```\n\nnext", 23),
        // Inside a list the fence is not at the top level.
        ("- x\n  ```\n  y\n  ```\n\nnext", 0),
        // A fence closed by a shorter run is still open.
        ("````\nz\n```\n\nnext", 0),
    ];
    for (source, reused) in cases {
        let mut parser = IncrementalMarkdown::new();
        parser.parse(source);
        assert_eq!(parser.reused_len(), reused, "{source:?}");
    }
}

// Catches a kept prefix surviving a reference definition, which applies
// to links before it.
#[test]
fn reference_definition_after_the_prefix_reparses_the_whole_source() {
    let mut parser = IncrementalMarkdown::new();
    let head = "[docs]\n\n```\nx\n```\n\n";
    parser.parse(head);
    let source = format!("{head}[docs]: https://example.com");

    let doc = parser.parse(&source);

    assert_eq!(doc.dump(), MarkdownDoc::parse(&source).dump());
    assert!(doc.dump().starts_with("p: [l=https://example.com:docs]"));
}
