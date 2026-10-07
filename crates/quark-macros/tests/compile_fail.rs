//! Inputs the macros must reject with a spanned error instead of panicking
//! or silently dropping part of the input.

#[test]
fn macro_errors() {
    trybuild::TestCases::new().compile_fail("tests/ui/*.rs");
}
