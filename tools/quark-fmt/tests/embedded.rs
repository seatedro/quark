//! The rustfmt-backed provider on embedded Rust from the real corpus.
//!
//! Fixtures under `tests/fixtures/embedded/` hold cases of the form
//!
//! ```text
//! === name
//! context: expr
//! prefix: bg={
//! suffix: }
//! --- input
//!         bg={if on { p.toggle_on } else { p.menu_hi }}
//! --- expected
//!         bg={if on { p.toggle_on } else { p.menu_hi }}
//! ```
//!
//! The input is source text as it sits in a template: the first line's
//! indentation is the template indentation, then `prefix`, the fragment,
//! and `suffix`. `expected` is the construct as the printer writes it: the
//! one-line form when it fits, otherwise the layout. `--- error` instead
//! holds part of the reason the fragment kept its source. All cases of a
//! file go through one provider, prepared together, as a file's fragments
//! are.

use std::path::{Path, PathBuf};

use std::ops::Range;

use quark_fmt::embedded::RustfmtProvider;
use quark_fmt::printer::rust::source_layout;
use quark_fmt::rustfmt::RustfmtCommand;
use quark_fmt::trivia::{LexKind, lex};
use quark_fmt::{Layout, LayoutRequest, NestedViews, RustContext, RustFragment, RustProvider};

struct Case {
    name: String,
    context: RustContext,
    prefix: String,
    suffix: String,
    input: String,
    expected: Result<String, String>,
}

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/embedded")
}

fn parse_cases(text: &str) -> Vec<Case> {
    let mut cases = Vec::new();
    for block in text.split("\n=== ").map(|b| b.trim_start_matches("=== ")) {
        if block.trim().is_empty() {
            continue;
        }
        let (head, rest) = block.split_once("\n--- input\n").expect("--- input");
        let mut lines = head.lines();
        let name = lines.next().expect("name").trim().to_owned();
        let (mut context, mut prefix, mut suffix) =
            (RustContext::Expr, String::new(), String::new());
        for line in lines {
            let (k, v) = line
                .split_once(": ")
                .unwrap_or((line.trim_end_matches(':'), ""));
            match k {
                "context" => {
                    context = match v {
                        "expr" => RustContext::Expr,
                        "scrutinee" => RustContext::Scrutinee,
                        "cond" => RustContext::Condition,
                        "for" => RustContext::ForHeader,
                        "arm" => RustContext::ArmHead,
                        "local" => RustContext::Local,
                        "args" => RustContext::Args,
                        "type" => RustContext::Type,
                        other => panic!("{name}: unknown context {other}"),
                    }
                }
                "prefix" => prefix = v.to_owned(),
                "suffix" => suffix = v.to_owned(),
                other => panic!("{name}: unknown key {other}"),
            }
        }
        let (input, expected) = match rest.split_once("\n--- expected\n") {
            Some((i, e)) => (i, Ok(e.trim_end_matches('\n').to_owned())),
            None => {
                let (i, e) = rest.split_once("\n--- error\n").expect("expected or error");
                (i, Err(e.trim().to_owned()))
            }
        };
        cases.push(Case {
            name,
            context,
            prefix,
            suffix,
            input: input.to_owned(),
            expected,
        });
    }
    cases
}

/// Fixtures without nested views never call these.
struct NoNested;

impl NestedViews for NoNested {
    fn flat(&self, _: usize) -> Option<&str> {
        None
    }
    fn layout(&self, _: usize, _: usize) -> Layout {
        Layout::default()
    }
}

/// Nested views kept as written, moved as a whole: one line when written
/// on one line, otherwise their own lines hanging from where they land.
struct AsWritten<'a> {
    file: &'a str,
    views: &'a [Range<usize>],
}

impl NestedViews for AsWritten<'_> {
    fn flat(&self, id: usize) -> Option<&str> {
        let text = &self.file[self.views[id].clone()];
        (!text.contains('\n')).then_some(text)
    }

    fn layout(&self, id: usize, indent: usize) -> Layout {
        let view = RustFragment {
            context: RustContext::Expr,
            file: self.file,
            range: self.views[id].clone(),
            nested: Vec::new(),
        };
        let mut layout = source_layout(&view, 4, &NoNested);
        for line in layout.lines.iter_mut().skip(1).filter(|l| !l.verbatim) {
            line.indent += indent;
        }
        layout
    }
}

/// `view! { ... }` invocations inside `range`, outermost only.
fn nested_views(source: &str, range: Range<usize>) -> Vec<Range<usize>> {
    let toks: Vec<_> = lex(&source[range.clone()], range.start)
        .into_iter()
        .filter(|t| !t.kind.is_trivia())
        .collect();
    let mut views = Vec::new();
    let mut i = 0;
    while i + 2 < toks.len() {
        let is_view = &source[toks[i].range.clone()] == "view"
            && toks[i + 1].kind == LexKind::Punct('!')
            && toks[i + 2].kind == LexKind::Punct('{');
        if !is_view {
            i += 1;
            continue;
        }
        let mut depth = 0;
        let mut j = i + 2;
        loop {
            match toks[j].kind {
                LexKind::Punct('{') => depth += 1,
                LexKind::Punct('}') => depth -= 1,
                _ => {}
            }
            if depth == 0 {
                break;
            }
            j += 1;
        }
        views.push(toks[i].range.start..toks[j].range.end);
        i = j + 1;
    }
    views
}

fn indent_of(source: &str) -> usize {
    source.len() - source.trim_start_matches(' ').len()
}

/// The case's fragment, first token to last, as the printer sends it.
fn fragment<'a>(case: &Case, source: &'a str) -> RustFragment<'a> {
    let indent = indent_of(source);
    assert!(
        source[indent..].starts_with(&case.prefix) && source.ends_with(&case.suffix),
        "{}: input must be indent, prefix, fragment, suffix",
        case.name
    );
    let start = indent + case.prefix.len();
    let inner = &source[start..source.len() - case.suffix.len()];
    let start = start + (inner.len() - inner.trim_start().len());
    let range = start..start + inner.trim().len();
    RustFragment {
        context: case.context,
        file: source,
        nested: nested_views(source, range.clone()),
        range,
    }
}

/// What the printer writes: the one-line form when it fits, otherwise
/// the layout, with continuation lines indented from the template indent.
/// A layout that starts on the next line ends on a line of its own.
fn print(
    provider: &RustfmtProvider,
    (max_width, tab_spaces): (usize, usize),
    case: &Case,
    frag: &RustFragment<'_>,
) -> Result<String, String> {
    let indent = indent_of(frag.file);
    let pad = " ".repeat(indent);
    let (prefix, suffix) = (case.prefix.chars().count(), case.suffix.chars().count());
    let nested = AsWritten {
        file: frag.file,
        views: &frag.nested,
    };
    let flat = provider.flat(frag, &nested).map_err(|e| e.message)?;
    if let Some(flat) = flat.filter(|f| indent + prefix + f.chars().count() + suffix <= max_width) {
        return Ok(format!("{pad}{}{flat}{}", case.prefix, case.suffix));
    }
    let request = LayoutRequest {
        indent,
        prefix,
        suffix,
        max_width,
        tab_spaces,
    };
    let layout = provider
        .layout(frag, &request, &nested)
        .map_err(|e| e.message)?;
    let mut out = format!("{pad}{}", case.prefix);
    for (i, line) in layout.lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
            if !line.verbatim {
                out.push_str(&" ".repeat(indent + line.indent));
            }
        }
        out.push_str(&line.text);
    }
    out.push_str(&case.suffix);
    // Blank lines carry no indentation.
    Ok(out
        .lines()
        .map(|l| if l.trim().is_empty() { "" } else { l })
        .collect::<Vec<_>>()
        .join("\n"))
}

/// Prints each case's `inputs` entry with one provider for the file.
fn print_all(dir: &Path, cases: &[Case], inputs: &[String]) -> Vec<Result<String, String>> {
    let command = RustfmtCommand::new(dir);
    // The printer's width settings come from the same rustfmt.toml.
    let widths = command.widths().expect("rustfmt --print-config");
    let provider = RustfmtProvider::new(command);
    let frags: Vec<RustFragment<'_>> = cases
        .iter()
        .zip(inputs)
        .map(|(c, s)| fragment(c, s))
        .collect();
    provider.prepare(&frags);
    cases
        .iter()
        .zip(&frags)
        .map(|(case, frag)| print(&provider, widths, case, frag))
        .collect()
}

fn run_file(dir: &Path, file: &str) {
    let text = std::fs::read_to_string(dir.join(file)).expect("fixture");
    let cases = parse_cases(&text);
    let inputs: Vec<String> = cases.iter().map(|c| c.input.clone()).collect();
    let mut failures = Vec::new();
    for (case, result) in cases.iter().zip(print_all(dir, &cases, &inputs)) {
        match (&case.expected, result) {
            (Ok(want), Ok(got)) if &got != want => {
                failures.push(format!("{}: got\n{got}\n--- want\n{want}", case.name));
            }
            (Err(want), Err(e)) if !e.contains(want.as_str()) => {
                failures.push(format!("{}: error `{e}` lacks `{want}`", case.name));
            }
            (Ok(_), Err(e)) => failures.push(format!("{}: failed: {e}", case.name)),
            (Err(want), Ok(got)) => failures.push(format!(
                "{}: expected an error containing `{want}`, got\n{got}",
                case.name
            )),
            _ => {}
        }
    }
    assert!(failures.is_empty(), "{file}:\n{}", failures.join("\n\n"));
}

#[test]
fn attribute_values_from_the_corpus() {
    run_file(&fixtures(), "attributes.case");
}

#[test]
fn every_rust_context() {
    run_file(&fixtures(), "contexts.case");
}

#[test]
fn comments_and_literals_survive() {
    run_file(&fixtures(), "comments.case");
}

#[test]
fn failures_keep_the_fragment() {
    run_file(&fixtures(), "failures.case");
}

#[test]
fn nested_views_size_their_enclosing_call() {
    run_file(&fixtures(), "nested.case");
}

#[test]
fn rustfmt_toml_applies() {
    run_file(&fixtures().join("narrow"), "width.case");
}

/// Every expected output is a fixed point: printing it again changes
/// nothing, so a second formatter pass leaves templates alone.
#[test]
fn formatted_fragments_are_fixed_points() {
    for (dir, file) in [
        (fixtures(), "attributes.case"),
        (fixtures(), "contexts.case"),
        (fixtures(), "comments.case"),
        (fixtures(), "nested.case"),
        (fixtures().join("narrow"), "width.case"),
    ] {
        let text = std::fs::read_to_string(dir.join(file)).expect("fixture");
        let cases: Vec<Case> = parse_cases(&text)
            .into_iter()
            .filter(|c| c.expected.is_ok())
            .collect();
        let outputs: Vec<String> = cases.iter().map(|c| c.expected.clone().unwrap()).collect();
        for ((case, want), got) in cases
            .iter()
            .zip(&outputs)
            .zip(print_all(&dir, &cases, &outputs))
        {
            assert_eq!(
                got.as_ref(),
                Ok(want),
                "{file}: {} is not a fixed point",
                case.name
            );
        }
    }
}

/// Without a rustfmt to run, every fragment keeps its source and says why.
#[test]
fn a_missing_rustfmt_keeps_every_fragment() {
    let mut command = RustfmtCommand::new(fixtures());
    command.program = "/nonexistent/rustfmt".into();
    let provider = RustfmtProvider::new(command);
    let source = "        bg={ a+b }";
    let frag = RustFragment {
        context: RustContext::Expr,
        file: source,
        range: 13..16,
        nested: Vec::new(),
    };
    provider.prepare(std::slice::from_ref(&frag));
    let request = LayoutRequest {
        indent: 8,
        prefix: 4,
        suffix: 1,
        max_width: 100,
        tab_spaces: 4,
    };
    let errors = [
        provider
            .flat(&frag, &NoNested)
            .expect_err("no rustfmt, no flat form"),
        provider
            .layout(&frag, &request, &NoNested)
            .expect_err("no rustfmt, no layout"),
    ];
    for e in errors {
        assert!(e.message.contains("could not run rustfmt"), "{}", e.message);
    }
}
