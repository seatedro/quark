//! Parses arbitrary text as a unified diff. A parse that succeeds must
//! yield a document and projections that pass their integrity checks, and
//! writing it back must not panic.
//!
//! Invalid UTF-8 is replaced, so every input reaches the parser.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quark_diff::{Expansion, Mode, Projection, parse_unified, write_unified};

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let Ok(doc) = parse_unified(&source) else {
        return;
    };
    assert_eq!(doc.verify_integrity(), Ok(()));
    let expansion = Expansion::new(&doc);
    for mode in [Mode::Unified, Mode::Split] {
        let projection = Projection::new(&doc, mode, &expansion);
        assert_eq!(projection.verify_integrity(&doc), Ok(()));
    }
    let _ = write_unified(&doc);
});
