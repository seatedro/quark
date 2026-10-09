//! Token equivalence of a formatted candidate against the original.
//!
//! Both texts are lexed with rustc's lexer. Whitespace is dropped;
//! comments, literals, identifiers, and punctuation are compared by their
//! exact bytes. Punctuation characters that touch are glued into Rust's
//! compound operators first, so `::` against `: :` or `..=` against `.. =`
//! is a mismatch, while `<spacer/>` against `<spacer />` is not: the view
//! grammar reads `/` and `>` as separate tokens either way.

use crate::trivia::{LexKind, lex};

/// Rust's multi-character punctuation, longest first.
const COMPOUND: &[&str] = &[
    "...", "..=", "<<=", ">>=", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=",
    "*=", "/=", "%=", "^=", "&=", "|=", "<<", ">>", "..",
];

/// One compared token: a lexeme or glued punctuation, with its offset.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token<'a> {
    pub kind: LexKind,
    pub text: &'a str,
    pub offset: usize,
}

/// The tokens of `src` as verification sees them.
pub fn tokens(src: &str) -> Vec<Token<'_>> {
    let lexemes = lex(src, 0);
    let mut out = Vec::with_capacity(lexemes.len());
    let mut i = 0;
    while i < lexemes.len() {
        let l = &lexemes[i];
        i += 1;
        match l.kind {
            LexKind::Whitespace => continue,
            LexKind::Punct(_) => {
                // Extend over touching punctuation while it still spells a
                // compound operator.
                let mut end = i;
                let mut best = i;
                while end < lexemes.len()
                    && matches!(lexemes[end].kind, LexKind::Punct(_))
                    && lexemes[end].range.start == lexemes[end - 1].range.end
                {
                    end += 1;
                    let text = &src[l.range.start..lexemes[end - 1].range.end];
                    if COMPOUND.contains(&text) {
                        best = end;
                    } else if !COMPOUND.iter().any(|c| c.starts_with(text)) {
                        break;
                    }
                }
                let text = &src[l.range.start..lexemes[best - 1].range.end];
                out.push(Token {
                    kind: l.kind,
                    text,
                    offset: l.range.start,
                });
                i = best;
            }
            kind => {
                let mut text = &src[l.range.clone()];
                if kind == LexKind::LineComment {
                    // The lexer leaves a CRLF file's `\r` on the comment.
                    text = text.strip_suffix('\r').unwrap_or(text);
                }
                out.push(Token {
                    kind,
                    text,
                    offset: l.range.start,
                });
            }
        }
    }
    out
}

/// Where two texts first disagree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// Byte offsets of the first differing token in each text, or their
    /// lengths when one runs out first.
    pub before: usize,
    pub after: usize,
    pub message: String,
}

/// Checks that `after` differs from `before` only in whitespace between
/// tokens.
pub fn equivalent(before: &str, after: &str) -> Result<(), Mismatch> {
    let a = tokens(before);
    let b = tokens(after);
    for (x, y) in a.iter().zip(&b) {
        if (x.kind, x.text) != (y.kind, y.text) {
            return Err(Mismatch {
                before: x.offset,
                after: y.offset,
                message: format!("`{}` became `{}`", x.text, y.text),
            });
        }
    }
    if a.len() != b.len() {
        let (before_at, after_at) = match (a.get(b.len()), b.get(a.len())) {
            (Some(x), _) => (x.offset, after.len()),
            (_, Some(y)) => (before.len(), y.offset),
            _ => unreachable!("lengths differ"),
        };
        return Err(Mismatch {
            before: before_at,
            after: after_at,
            message: format!("{} tokens became {}", a.len(), b.len()),
        });
    }
    Ok(())
}
