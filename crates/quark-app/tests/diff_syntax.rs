//! Syntax colors of diffs, through the diff view's syntax bridge, with real
//! grammar packs.
//!
//! Packs come from `quark_syntax::testing::pack_root` (build them with
//! `cargo run -p syntax-pack -- build rust javascript html python`); a test
//! whose packs are missing skips unless `QUARK_REQUIRE_SYNTAX_PACKS=1`.
//! Run with `cargo test -p quark-app --features syntax --test diff_syntax`.
#![cfg(feature = "syntax")]

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use quark_app::quark_ui::quark_syntax::{GrammarStore, HighlightKind, testing};
use quark_components::diff_view::syntax::{DiffSyntax, SyntaxBudget, SyntaxStatus};
use quark_diff::{DiffDocument, Side, diff_texts, parse_unified};

const COLORS: &[HighlightKind] = &[
    HighlightKind::Keyword,
    HighlightKind::String,
    HighlightKind::Comment,
    HighlightKind::Function,
];

/// `kind:text` for each run of the first line of `side` containing
/// `needle` whose kind is in [`COLORS`], in order.
fn dump(syntax: &DiffSyntax, doc: &DiffDocument, side: Side, needle: &str) -> String {
    let store = doc.text(0, side);
    let line = (0..store.line_count())
        .find(|&i| store.display_line(i).unwrap().contains(needle))
        .unwrap_or_else(|| panic!("no {side:?} line contains {needle:?}"));
    let text = store.display_line(line).unwrap();
    let (spans, tones) = syntax.spans(0, side, store.display_range(line).unwrap());
    spans
        .iter()
        .zip(tones.iter())
        .filter(|(_, kind)| COLORS.contains(kind))
        .map(|(span, kind)| format!("{}:{}", kind.name(), &text[span.range.clone()]))
        .collect::<Vec<_>>()
        .join(" ")
}

/// A bridge over `store` that has every first result for `doc`.
fn highlighted(doc: &DiffDocument, store: GrammarStore) -> DiffSyntax {
    let mut syntax = DiffSyntax::default();
    syntax.enable(store);
    syntax.request(doc, 1);
    syntax.finish_pending();
    syntax
}

/// A wake callback that signals the returned receiver.
fn woken(syntax: &mut DiffSyntax) -> Receiver<()> {
    let (tx, rx) = channel();
    syntax.set_wake(move || {
        let _ = tx.send(());
    });
    rx
}

fn wait(woke: &Receiver<()>) {
    woke.recv_timeout(Duration::from_secs(60))
        .expect("the worker publishes a result");
}

/// `count` numbered filler lines, so hunks stay apart.
fn filler(count: usize) -> String {
    (0..count).map(|i| format!("x{i}\n")).collect()
}

// Catches whole files being highlighted hunk by hunk, which loses lexical
// state that starts outside a hunk: a block comment opened before the
// first hunk, and a template string spanning the gap between two hunks,
// must color the changed lines inside them.
#[test]
fn whole_sources_carry_lexical_state_across_hunks() {
    let Some(store) = testing::store_with("javascript") else {
        return;
    };
    // (old, new, hunks, needle, expected): the comment case has one hunk,
    // starting after its opener; the template case two, with the gap
    // between them inside the string.
    let cases = [
        (
            format!("/* notes\n{}let x = 1;\n*/\n", filler(8)),
            format!("/* notes\n{}let x = 2;\n*/\n", filler(8)),
            1,
            "let x = 2;",
            "comment:let x = 2;",
        ),
        (
            format!("const t = `\nalpha\n{}beta\n`;\n", filler(10)),
            format!("const t = `\nalpha 2\n{}beta 2\n`;\n", filler(10)),
            2,
            "beta 2",
            "string:beta 2",
        ),
    ];
    for (old, new, hunks, needle, expected) in cases {
        let doc = diff_texts(Some("a.js"), Some("a.js"), Some(&old), Some(&new), 3);
        assert_eq!(doc.hunk_count(), hunks, "{needle}");
        let syntax = highlighted(&doc, store.clone());

        assert_eq!(dump(&syntax, &doc, Side::New, needle), expected, "{needle}");
    }
}

// Catches a patch's hunks being highlighted as one concatenated source: a
// comment opened in one hunk (and closed in a later one) must not color
// the hunk between them, whose real surroundings the patch does not show.
#[test]
fn patch_hunks_do_not_share_lexical_state() {
    let Some(store) = testing::store_with("rust") else {
        return;
    };
    let patch = "\
--- a/lib.rs
+++ b/lib.rs
@@ -1,2 +1,2 @@
 /* opened
-let a = 1;
+let a = 2;
@@ -20,2 +20,2 @@
 x
-let b = 1;
+let b = 2;
@@ -40,2 +40,2 @@
 */
-y
+z
";
    let doc = parse_unified(patch).unwrap();
    let syntax = highlighted(&doc, store);

    assert_eq!(dump(&syntax, &doc, Side::New, "let b"), "keyword:let");
}

// Catches injected languages losing their host's lexical state across a
// hunk boundary: a script element opened before the hunk makes its
// changed line JavaScript.
#[test]
fn script_opened_before_the_hunk_takes_javascript_colors() {
    let Some(store) = testing::store_with_all(&["html", "javascript"]) else {
        return;
    };
    let old = format!("<script>\n{}const a = f(1);\n</script>\n", filler(8));
    let new = format!("<script>\n{}const a = f(2);\n</script>\n", filler(8));
    let doc = diff_texts(Some("a.html"), Some("a.html"), Some(&old), Some(&new), 3);
    let syntax = highlighted(&doc, store);

    assert_eq!(
        dump(&syntax, &doc, Side::New, "f(2)"),
        "keyword:const function:f"
    );
}

// Catches both sides of a rename taking the new path's language: the old
// side of a Python file renamed to Rust is still Python.
#[test]
fn renamed_sides_take_their_own_languages() {
    let Some(store) = testing::store_with_all(&["python", "rust"]) else {
        return;
    };
    let patch = "\
diff --git a/tool.py b/tool.rs
similarity index 10%
rename from tool.py
rename to tool.rs
--- a/tool.py
+++ b/tool.rs
@@ -1 +1 @@
-def run(): pass
+fn run() {}
";
    let doc = parse_unified(patch).unwrap();
    let syntax = highlighted(&doc, store);

    assert_eq!(
        [
            dump(&syntax, &doc, Side::Old, "run"),
            dump(&syntax, &doc, Side::New, "run"),
        ],
        [
            "keyword:def function:run keyword:pass",
            "keyword:fn function:run"
        ]
    );
}

// Catches a grammar that arrives after the first highlight never reaching
// the diff, or the side still claiming to wait for it: the side renders
// plain while pending, then the worker's newer revision recolors it and
// wakes the app to take it.
#[test]
fn late_grammar_recolors_the_waiting_side() {
    let Some(store) = testing::store_deferring(&["rust"], &["rust"]) else {
        return;
    };
    let doc = diff_texts(
        Some("lib.rs"),
        Some("lib.rs"),
        Some("fn a() {}\n"),
        Some("fn b() {}\n"),
        3,
    );
    let mut syntax = DiffSyntax::default();
    let woke = woken(&mut syntax);
    syntax.enable(store.clone());
    syntax.request(&doc, 1);
    syntax.finish_pending();
    let before = (
        syntax.status(0, Side::New),
        dump(&syntax, &doc, Side::New, "fn b"),
    );
    while woke.try_recv().is_ok() {}

    store.release_deferred();
    // Both sides recolor.
    wait(&woke);
    wait(&woke);
    syntax.poll(1);

    assert_eq!(
        [
            before,
            (
                syntax.status(0, Side::New),
                dump(&syntax, &doc, Side::New, "fn b")
            )
        ],
        [
            (Some(SyntaxStatus::Pending), String::new()),
            (
                Some(SyntaxStatus::Ready),
                "keyword:fn function:b".to_owned()
            ),
        ]
    );
}

// Catches a result for replaced text landing on its replacement: the
// highlight of the first document is still in flight when the file is
// replaced by text too large to highlight, and must not color it.
#[test]
fn result_for_replaced_text_is_dropped() {
    let Some(store) = testing::store_with("rust") else {
        return;
    };
    let first = diff_texts(
        Some("lib.rs"),
        Some("lib.rs"),
        Some("fn a() {}\n"),
        Some("fn b() {}\n"),
        3,
    );
    let large = format!("let long = 1; {}\n", "/".repeat(80));
    let second = diff_texts(
        Some("lib.rs"),
        Some("lib.rs"),
        Some("fn a() {}\n"),
        Some(&large),
        3,
    );
    let mut syntax = DiffSyntax::default();
    let woke = woken(&mut syntax);
    syntax.enable(store);
    syntax.request(&first, 1);
    // Both sides' results are waiting, not yet taken.
    wait(&woke);
    wait(&woke);

    syntax.set_budget(SyntaxBudget { side_bytes: 64 });
    syntax.request(&second, 2);
    syntax.poll(2);

    assert_eq!(
        (
            syntax.status(0, Side::New),
            dump(&syntax, &second, Side::New, "long")
        ),
        (Some(SyntaxStatus::Limited), String::new())
    );
}

// Catches a status that misstates how far colors can be trusted: whole
// files are ready, patch hunks best effort, languages without a grammar
// and oversized sides plain, each named so the header can say so.
#[test]
fn file_status_names_how_far_colors_can_be_trusted() {
    let Some(store) = testing::store_with("rust") else {
        return;
    };
    let texts = |path: &str, new: &str| {
        diff_texts(Some(path), Some(path), Some("fn a() {}\n"), Some(new), 3)
    };
    let patch = "--- a/lib.rs\n+++ b/lib.rs\n@@ -1 +1 @@\n-fn a() {}\n+fn b() {}\n";
    let cases = [
        (
            "whole files",
            texts("lib.rs", "fn b() {}\n"),
            SyntaxStatus::Ready,
        ),
        (
            "patch",
            parse_unified(patch).unwrap(),
            SyntaxStatus::PartialSource,
        ),
        (
            "no pack",
            texts("lib.klingon", "fn b() {}\n"),
            SyntaxStatus::MissingGrammar,
        ),
        (
            "no extension",
            texts("Makefile", "fn b() {}\n"),
            SyntaxStatus::MissingGrammar,
        ),
        (
            "too large",
            texts("lib.rs", &"// long\n".repeat(20)),
            SyntaxStatus::Limited,
        ),
    ];
    for (name, doc, expected) in cases {
        let mut syntax = DiffSyntax::default();
        syntax.set_budget(SyntaxBudget { side_bytes: 100 });
        syntax.enable(store.clone());
        syntax.request(&doc, 1);
        syntax.finish_pending();

        assert_eq!(syntax.file_status(0), Some(expected), "{name}");
    }
}
