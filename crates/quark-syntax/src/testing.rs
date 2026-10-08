//! Test support for crates whose tests highlight real code.
//!
//! Tests load packs from the root `syntax-pack build` writes by default
//! (`<workspace>/target/syntax-packs`), or from `$QUARK_SYNTAX_TEST_PACKS`.
//! Build the packs a test names once with `cargo run -p syntax-pack --
//! build <language>...`. Without them these tests skip, unless
//! `QUARK_REQUIRE_SYNTAX_PACKS` (set in CI after building the fixtures)
//! turns a missing pack into a failure: `1` requires every pack, and a
//! comma-separated list (`rust,javascript`) requires only those.

use std::path::PathBuf;

use crate::{GrammarStore, StoreConfig, pack};

/// The pack root tests read.
pub fn pack_root() -> PathBuf {
    std::env::var_os("QUARK_SYNTAX_TEST_PACKS")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/syntax-packs")
        })
}

/// A store over [`pack_root`] when it holds a pack for `language`.
pub fn store_with(language: &str) -> Option<GrammarStore> {
    store_with_all(&[language])
}

/// A store over [`pack_root`] when it holds a pack for every one of
/// `languages`.
pub fn store_with_all(languages: &[&str]) -> Option<GrammarStore> {
    let root = pack_root();
    let required = std::env::var("QUARK_REQUIRE_SYNTAX_PACKS").unwrap_or_default();
    for language in languages {
        let dir = root.join(pack::TARGET).join(language);
        if dir.join(pack::MANIFEST).is_file() {
            continue;
        }
        assert!(
            required != "1" && !required.split(',').any(|r| r.trim() == *language),
            "QUARK_REQUIRE_SYNTAX_PACKS={required} but there is no {language} pack in {}; \
             build it with `cargo run -p syntax-pack -- build {language}`",
            dir.display()
        );
        eprintln!(
            "skipping: no {language} pack in {}; build it with `cargo run -p syntax-pack -- build {language}`",
            dir.display()
        );
        return None;
    }
    Some(GrammarStore::new(StoreConfig::new().local_packs(root)))
}
