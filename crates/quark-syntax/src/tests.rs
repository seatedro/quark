#![cfg_attr(not(feature = "engine"), allow(dead_code, unused_imports))]

use super::*;

/// `kind:text` for every span whose kind is in `kinds`, in source order.
fn dump(language: LanguageId, source: &str, kinds: &[HighlightKind]) -> String {
    highlight(language, source)
        .into_iter()
        .filter(|span| kinds.contains(&span.kind))
        .map(|span| format!("{}:{}", span.kind.name(), &source[span.range()]))
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(feature = "rust")]
#[test]
fn rust_snippet_marks_keywords_and_strings() {
    let source = "fn main() {\n    let greeting = \"hi\";\n    if true { return; }\n}\n";

    let dumped = dump(
        LanguageId::Rust,
        source,
        &[HighlightKind::Keyword, HighlightKind::String],
    );

    assert_eq!(
        dumped,
        "keyword:fn keyword:let string:\"hi\" keyword:if keyword:return"
    );
}

/// Each compiled-in language's query loads against its grammar and marks
/// a keyword or string in a one-line sample. Catches a query that no
/// longer compiles (for TypeScript, the JavaScript layer underneath it).
#[test]
fn every_compiled_language_highlights_its_sample() {
    let samples = [
        (LanguageId::Bash, "if [ -n \"$x\" ]; then echo hi; fi"),
        (LanguageId::Go, "func main() { s := \"hi\"; return }"),
        (LanguageId::JavaScript, "const x = \"hi\"; return x;"),
        (LanguageId::Json, "{\"name\": \"quark\"}"),
        (LanguageId::Python, "def f():\n    return \"hi\""),
        (LanguageId::Rust, "fn f() -> &'static str { \"hi\" }"),
        (
            LanguageId::TypeScript,
            "const x: string = \"hi\"; interface A {}",
        ),
    ];
    let missing: Vec<&str> = samples
        .iter()
        .filter(|(language, _)| language.is_compiled())
        .filter(|(language, source)| {
            dump(
                *language,
                source,
                &[HighlightKind::Keyword, HighlightKind::String],
            )
            .is_empty()
        })
        .map(|(language, _)| language.name())
        .collect();

    assert_eq!(missing, Vec::<&str>::new());
}
