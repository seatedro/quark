//! Parses arbitrary text as markdown and checks the document's integrity
//! (span tiling, ranges in bounds, links, table cells).
//!
//! Invalid UTF-8 is replaced, so every input parses.

#![no_main]

use libfuzzer_sys::fuzz_target;
use quark_ui::markdown::MarkdownDoc;

fuzz_target!(|data: &[u8]| {
    let source = String::from_utf8_lossy(data);
    let doc = MarkdownDoc::parse(&source);
    assert_eq!(doc.verify_integrity(), Ok(()));
});
