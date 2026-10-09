//! Layout fixtures: each `tests/fixtures/layout/<case>.in.rs` formats to
//! `<case>.out.rs` with the pass-through provider, and the expected output
//! is already formatted. Inputs are excerpts of the real corpus
//! (examples/codex, examples/workbench, crates/quark-components) or
//! synthetic cases named for the rule they pin. `QUARK_BLESS=1` rewrites
//! the expected files locally; review the diff before committing.

use std::path::Path;

use quark_fmt::{FormatOptions, PassThrough, format_to_string};

fn format(src: &str) -> Result<String, String> {
    format_to_string(src, &FormatOptions::default(), &PassThrough).map_err(|d| format!("{d:#?}"))
}

#[test]
fn layout_fixtures() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/layout");
    let bless = std::env::var_os("QUARK_BLESS").is_some();
    let mut failures = Vec::new();
    let mut cases: Vec<_> = std::fs::read_dir(&dir)
        .expect("fixture dir")
        .filter_map(|e| {
            let p = e.ok()?.path();
            let name = p.file_name()?.to_str()?.strip_suffix(".in.rs")?.to_owned();
            Some(name)
        })
        .collect();
    cases.sort();
    assert!(!cases.is_empty());
    for case in cases {
        let input = std::fs::read_to_string(dir.join(format!("{case}.in.rs"))).unwrap();
        let out_path = dir.join(format!("{case}.out.rs"));
        let got = match format(&input) {
            Ok(got) => got,
            Err(d) => {
                failures.push(format!("{case}: {d}"));
                continue;
            }
        };
        if bless {
            std::fs::write(&out_path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&out_path).unwrap_or_default();
        if got != want {
            failures.push(format!("{case}:\n--- want\n{want}\n--- got\n{got}"));
        } else if format(&want).as_ref() != Ok(&want) {
            failures.push(format!("{case}: expected output is not stable"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}

/// Input the formatter must refuse leaves the file unchanged with a
/// diagnostic naming why.
#[test]
fn rejected_inputs_report_why_and_change_nothing() {
    use quark_fmt::{DiagnosticKind as K, Severity, format_source};
    let cases = [
        ("fn f() { view! { <div> } }", K::ViewParse),
        // Rust attributes are not template syntax.
        ("fn f() { view! { #[cfg(x)] <div /> } }", K::ViewParse),
        ("fn f( {", K::RustParse),
        (
            "fn f() { view! { <text>{line!()}</text> } }",
            K::SpanSensitive,
        ),
        // One bad view keeps the good one beside it unformatted too.
        ("fn f() { view! {<a/>}; view! { <b> } }", K::ViewParse),
    ];
    for (src, kind) in cases {
        let out = format_source(src, &FormatOptions::default(), &PassThrough);
        assert!(out.edits.is_empty(), "{src}: {:?}", out.edits);
        assert!(
            out.diagnostics
                .iter()
                .any(|d| d.severity == Severity::Error && d.kind == kind),
            "{src}: {:?}",
            out.diagnostics
        );
    }
}

/// Only `expr_macros` bodies that parse as expressions are read as Rust;
/// a view in any other macro body is reported and left alone, inside a
/// view's embedded Rust too.
#[test]
fn a_view_inside_another_macro_is_reported_and_left_alone() {
    use quark_fmt::{DiagnosticKind, Severity, format_source};
    let cases = [
        "fn f() { let v = smallvec![view! {<a   />}]; }",
        "fn f() { let v = vec![view! {<a   />} => 1]; }",
        "fn f() { let v = format!(\"{:?}\", vec![view! {<a   />}]); }",
        "fn f() { view! { <b>{smallvec![view! {<a   />}]}</b> }; }",
    ];
    for src in cases {
        let out = format_source(src, &FormatOptions::default(), &PassThrough);
        assert!(out.edits.is_empty(), "{src}: {:?}", out.edits);
        let d = &out.diagnostics[0];
        assert_eq!(
            (d.severity, d.kind),
            (Severity::Warning, DiagnosticKind::HiddenMacro),
            "{src}"
        );
        // The `!` of the innermost view.
        let bang = src.rfind("view").unwrap() + "view".len();
        assert_eq!(d.range, Some(bang..bang + 1), "{src}");
    }
}

/// A provider that fails keeps its fragment as written; one that changes
/// tokens is caught before any edit is returned.
#[test]
fn provider_failures_and_token_changes() {
    use quark_fmt::{
        DiagnosticKind, Layout, LayoutRequest, NestedViews, ProviderError, RustFragment,
        RustProvider, Severity, format_source,
    };

    struct Failing;
    impl RustProvider for Failing {
        fn flat(
            &self,
            f: &RustFragment,
            _: &dyn NestedViews,
        ) -> Result<Option<String>, ProviderError> {
            Err(ProviderError {
                range: None,
                message: format!("cannot format {}", f.source()),
            })
        }
        fn layout(
            &self,
            _: &RustFragment,
            _: &LayoutRequest,
            _: &dyn NestedViews,
        ) -> Result<Layout, ProviderError> {
            unreachable!("flat failed first")
        }
    }
    let src = "fn f() { view! {<a x={y}   />} }";
    let out = format_source(src, &FormatOptions::default(), &Failing);
    assert_eq!(
        quark_fmt::apply_edits(src, &out.edits),
        "fn f() { view! { <a x={y} /> } }"
    );
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.severity == Severity::Warning && d.kind == DiagnosticKind::Provider)
    );

    /// Rewrites `a::b` as `a: :b`, which a whitespace-only formatter must
    /// never accept.
    struct Splitting;
    impl RustProvider for Splitting {
        fn flat(
            &self,
            f: &RustFragment,
            _: &dyn NestedViews,
        ) -> Result<Option<String>, ProviderError> {
            Ok(Some(f.source().replace("::", ": :")))
        }
        fn layout(
            &self,
            f: &RustFragment,
            _: &LayoutRequest,
            _: &dyn NestedViews,
        ) -> Result<Layout, ProviderError> {
            Ok(Layout::single(f.source().replace("::", ": :")))
        }
    }
    let src = "fn f() { view! {<a x={y::z}   />} }";
    let out = format_source(src, &FormatOptions::default(), &Splitting);
    assert!(out.edits.is_empty());
    assert!(
        out.diagnostics
            .iter()
            .any(|d| d.kind == DiagnosticKind::TokenMismatch)
    );
}

/// `macro_names` decides which paths are views: a configured re-export is
/// formatted and the default name, not configured, is left alone.
#[test]
fn configured_macro_names_select_views() {
    let options = FormatOptions {
        macro_names: vec!["ui::view".to_owned()],
        ..FormatOptions::default()
    };
    let src = "fn f() { ::ui::view! {<a   />}; view! {<a   />}; }";
    let out = format_to_string(src, &options, &PassThrough).unwrap();
    assert_eq!(out, "fn f() { ::ui::view! { <a /> }; view! {<a   />}; }");
}

mod properties {
    //! Random templates over the grammar, with random whitespace and
    //! comments at every gap between template tokens.

    use proptest::prelude::*;
    use quark_fmt::verify::equivalent;
    use quark_fmt::{FormatOptions, PassThrough, format_to_string};

    /// Rust islands from known-valid shapes, one nested view among them.
    const EXPRS: &[&str] = &[
        "x",
        "a.b(c, 1)",
        "move |_| { go(1) }",
        "if c { a } else { b }",
        "format!(\"{x} y\")",
        "Some(view! { <a w={1} /> })",
        "(1.0, 2.0)",
    ];

    fn attr() -> impl Strategy<Value = Vec<String>> {
        let leaf = prop_oneof![
            Just(vec!["hidden".to_owned()]),
            Just(vec!["aria-label".into(), "=".into(), "\"x y\"".into()]),
            Just(vec!["class".into(), "=".into(), "\"p-4\n    px-2\"".into()]),
            prop::sample::select(EXPRS).prop_map(|e| vec![
                "on:click".into(),
                "=".into(),
                "{".into(),
                e.to_owned(),
                "}".into()
            ]),
        ];
        leaf.prop_recursive(2, 6, 3, |inner| {
            prop::collection::vec(inner, 0..3).prop_map(|attrs| {
                let mut v = vec![
                    "@when".into(),
                    "{".into(),
                    "c".into(),
                    "}".into(),
                    "{".into(),
                ];
                v.extend(attrs.into_iter().flatten());
                v.push("}".into());
                v
            })
        })
    }

    fn node() -> impl Strategy<Value = Vec<String>> {
        let leaf = prop_oneof![
            Just(vec!["\"text\"".to_owned()]),
            Just(vec!["r#\"<a/> // no\"#".to_owned()]),
            prop::sample::select(EXPRS).prop_map(|e| vec!["{".into(), e.to_owned(), "}".into()]),
            Just(vec!["{".into(), "?".into(), "x".into(), "}".into()]),
            Just(vec!["let v = 1;".to_owned()]),
            (
                prop::sample::select(&["div", "Row", "w::Card"][..]),
                prop::collection::vec(attr(), 0..4)
            )
                .prop_map(|(tag, attrs)| {
                    let mut v = vec!["<".into(), tag.to_owned()];
                    v.extend(attrs.into_iter().flatten());
                    v.extend(["/".into(), ">".into()]);
                    v
                }),
        ];
        leaf.prop_recursive(3, 24, 4, |inner| {
            let kids = prop::collection::vec(inner, 0..4).prop_map(|k| k.concat());
            prop_oneof![
                (kids.clone(), prop::collection::vec(attr(), 0..3)).prop_map(|(kids, attrs)| {
                    let mut v = vec!["<".into(), "div".into()];
                    v.extend(attrs.into_iter().flatten());
                    v.push(">".into());
                    v.extend(kids);
                    v.extend(["<".into(), "/".into(), "div".into(), ">".into()]);
                    v
                }),
                (kids.clone(), kids.clone()).prop_map(|(a, b)| {
                    let mut v = vec!["if".into(), "let Some(x) = y".into(), "{".into()];
                    v.extend(a);
                    v.extend(["}".into(), "else".into(), "{".into()]);
                    v.extend(b);
                    v.push("}".into());
                    v
                }),
                kids.clone().prop_map(|k| {
                    let mut v: Vec<String> = [
                        "for",
                        "(i, x)",
                        "in",
                        "xs.iter()",
                        "key",
                        "=",
                        "{",
                        "i",
                        "}",
                        "{",
                    ]
                    .map(String::from)
                    .into();
                    v.extend(k);
                    v.push("}".into());
                    v
                }),
                kids.prop_map(|k| {
                    let mut v: Vec<String> = [
                        "match", "m", "{", "A", "=>", "<", "a", "/", ">", "B", "=>", "{",
                    ]
                    .map(String::from)
                    .into();
                    v.extend(k);
                    v.extend(["}", "C", "if", "z", "=>", "x", ","].map(String::from));
                    v.push("}".into());
                    v
                }),
            ]
        })
    }

    const SEPS: &[&str] = &[
        "",
        " ",
        "\n",
        "\n\n\n",
        "   ",
        " /* c */ ",
        " // c\n",
        "\n    // c\n",
    ];

    /// Joins pieces with the chosen separators, using a space wherever no
    /// separator would merge two tokens.
    fn join(pieces: &[String], seps: &[usize]) -> String {
        let mut out = String::new();
        for (i, p) in pieces.iter().enumerate() {
            if i > 0 {
                let sep = SEPS[seps[i % seps.len()]];
                let left = out.chars().last().unwrap_or(' ');
                let right = p.chars().next().unwrap_or(' ');
                let touch_ok = "<>/{}=,@".contains(left) || "<>/{}=,".contains(right);
                out.push_str(if sep.is_empty() && !touch_ok {
                    " "
                } else {
                    sep
                });
            }
            out.push_str(p);
        }
        out
    }

    proptest! {
        #[test]
        fn formatting_keeps_tokens_and_is_idempotent(
            root in node(),
            seps in prop::collection::vec(0..SEPS.len(), 1..16),
        ) {
            let body = join(&root, &seps);
            let src = format!("fn f() {{\n    view! {{ {body}\n}}\n}}\n");
            let once = format_to_string(&src, &FormatOptions::default(), &PassThrough)
                .map_err(|d| TestCaseError::fail(format!("{d:?}\n{src}")))?;
            prop_assert!(equivalent(&src, &once).is_ok(), "{src}\n=>\n{once}");
            let twice = format_to_string(&once, &FormatOptions::default(), &PassThrough)
                .map_err(|d| TestCaseError::fail(format!("{d:?}\n{once}")))?;
            prop_assert_eq!(&twice, &once);
        }
    }
}
