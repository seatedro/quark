//! The `view!` grammar, shared by `quark-macros` (which lowers it to
//! builder calls) and the view formatter (which lays it out).
//!
//! [`ast`] is the parsed form. Parsing only checks syntax; what each node
//! means is decided when `quark-macros` lowers it, so input that parses can
//! still fail to compile.

pub mod ast;
mod parse;

pub use parse::tag_name;
