//! `view!`: the grammar and its AST live in `quark-view-syntax`, shared
//! with the formatter; this module lowers the AST to builder calls
//! (`emit.rs`).

pub(crate) use quark_view_syntax::ast;
pub(crate) mod emit;
pub(crate) mod text;
