//! The `view!` grammar, shared by `quark-macros` (which lowers it to
//! builder calls) and the view formatter (which lays it out).
//!
//! [`ast`] is the parsed form, which `ViewInput`'s `syn::parse::Parse`
//! impl produces. Parsing only checks syntax; what each node means is
//! decided when `quark-macros` lowers it, so input that parses can still
//! fail to compile.
//!
//! [`parse_with_syntax`] runs the same parser and also returns a
//! [`syntax::SyntaxTree`]: the tokens and source spans a tool needs to
//! rewrite the input without inventing or dropping any of it.

pub mod ast;
mod parse;
pub mod syntax;

pub use parse::{parse_with_syntax, tag_name};
