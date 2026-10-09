//! Lexical view of the original source: comments, literal boundaries, and
//! whitespace, from rustc's own lexer. Template structure comes from the
//! shared parser; this module only says which bytes are trivia.

use std::ops::Range;

use ra_ap_rustc_lexer::{FrontmatterAllowed, TokenKind, tokenize};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LexKind {
    Whitespace,
    LineComment,
    BlockComment,
    /// A string, char, byte, or number literal, suffix included.
    Literal,
    Ident,
    Lifetime,
    /// A single punctuation or delimiter character.
    Punct(char),
    /// Anything the lexer does not recognize.
    Other,
}

impl LexKind {
    pub fn is_trivia(self) -> bool {
        matches!(
            self,
            LexKind::Whitespace | LexKind::LineComment | LexKind::BlockComment
        )
    }

    pub fn is_comment(self) -> bool {
        matches!(self, LexKind::LineComment | LexKind::BlockComment)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lexeme {
    pub kind: LexKind,
    /// Byte range, offset by the `base` given to [`lex`].
    pub range: Range<usize>,
}

/// Lexes `src`, a slice starting at byte `base` of its file.
pub fn lex(src: &str, base: usize) -> Vec<Lexeme> {
    let mut at = 0;
    tokenize(src, FrontmatterAllowed::No)
        .map(|tok| {
            let len = tok.len as usize;
            let kind = match tok.kind {
                TokenKind::Whitespace => LexKind::Whitespace,
                TokenKind::LineComment { .. } => LexKind::LineComment,
                TokenKind::BlockComment { .. } => LexKind::BlockComment,
                TokenKind::Literal { .. } => LexKind::Literal,
                TokenKind::Ident
                | TokenKind::RawIdent
                | TokenKind::InvalidIdent
                | TokenKind::UnknownPrefix => LexKind::Ident,
                TokenKind::Lifetime { .. }
                | TokenKind::RawLifetime
                | TokenKind::UnknownPrefixLifetime => LexKind::Lifetime,
                _ => match src[at..at + len].chars().next() {
                    Some(c) if len == c.len_utf8() && c.is_ascii_punctuation() => LexKind::Punct(c),
                    _ => LexKind::Other,
                },
            };
            let lexeme = Lexeme {
                kind,
                range: base + at..base + at + len,
            };
            at += len;
            lexeme
        })
        .collect()
}
