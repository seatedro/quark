//! Test support for crates whose tests highlight real code.
//!
//! Tests load packs from the root `syntax-pack build` writes by default
//! (`<workspace>/target/syntax-packs`), or from `$QUARK_SYNTAX_TEST_PACKS`.
//! Build the Rust pack once with `cargo run -p syntax-pack -- build rust`.
//! Without it these tests skip, unless `QUARK_REQUIRE_SYNTAX_PACKS=1` (set
//! in CI after building the fixture) turns the missing pack into a failure.

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
    let root = pack_root();
    let dir = root.join(pack::TARGET).join(language);
    if dir.join(pack::MANIFEST).is_file() {
        return Some(GrammarStore::new(StoreConfig::new().local_packs(root)));
    }
    assert!(
        std::env::var_os("QUARK_REQUIRE_SYNTAX_PACKS").is_none_or(|v| v != "1"),
        "QUARK_REQUIRE_SYNTAX_PACKS=1 but there is no {language} pack in {}; \
         build it with `cargo run -p syntax-pack -- build {language}`",
        dir.display()
    );
    eprintln!(
        "skipping: no {language} pack in {}; build it with `cargo run -p syntax-pack -- build {language}`",
        dir.display()
    );
    None
}
