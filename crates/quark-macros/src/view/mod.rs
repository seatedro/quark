//! `view!`: parse (`parse.rs`) into an AST (`ast.rs`), then lower it to
//! builder calls (`emit.rs`).

pub(crate) mod ast;
pub(crate) mod emit;
mod parse;
pub(crate) mod text;
