//! Shared helpers for this crate's property tests.

use proptest::test_runner::Config as ProptestConfig;

/// `PROPTEST_CASES` overrides the per-property default for heavier runs.
pub(crate) fn proptest_config(default_cases: u32) -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        // Miri hides host env vars under isolation, so it gets its own
        // small default.
        .unwrap_or(if cfg!(miri) { 4 } else { default_cases });
    let mut config = ProptestConfig::with_cases(cases);
    if cfg!(miri) {
        // Miri's isolation forbids the regression file lookups.
        config.failure_persistence = None;
    }
    config
}
