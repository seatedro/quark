//! Layout fixtures: each `tests/fixtures/layout/<case>.in.rs` formats to
//! `<case>.out.rs` with the pass-through provider, and the expected output
//! is already formatted. Inputs are excerpts of the real corpus
//! (examples/codex, examples/workbench, crates/quark-components) or
//! synthetic cases named for the rule they pin. `QUARK_BLESS=1` rewrites
//! the expected files locally; review the diff before committing.

use std::path::Path;

use quark_fmt::{FormatOptions, PassThrough, format_to_string};

fn format(src: &str) -> String {
    format_to_string(src, &FormatOptions::default(), &PassThrough)
        .unwrap_or_else(|d| panic!("diagnostics: {d:#?}"))
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
        let got = format(&input);
        if bless {
            std::fs::write(&out_path, &got).unwrap();
            continue;
        }
        let want = std::fs::read_to_string(&out_path).unwrap_or_default();
        if got != want {
            failures.push(format!("{case}:\n--- want\n{want}\n--- got\n{got}"));
        } else if format(&want) != want {
            failures.push(format!("{case}: expected output is not stable"));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n\n"));
}
