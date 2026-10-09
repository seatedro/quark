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

/// A comment between two template leaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub range: Range<usize>,
    /// A `//` comment, which must end its line.
    pub line: bool,
    /// Newlines between the previous leaf or comment and this one.
    pub newlines_before: usize,
    /// Whitespace separates it from what precedes it.
    pub spaced_before: bool,
}

/// The trivia between two consecutive leaves.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Gap {
    pub comments: Vec<Comment>,
    /// Newlines after the last comment, or in the whole gap without one.
    pub newlines_after: usize,
    /// The gap is not empty: the leaves around it were not touching.
    pub spaced: bool,
    /// Whitespace follows the last comment.
    pub spaced_after: bool,
}

impl Gap {
    pub fn has_comments(&self) -> bool {
        !self.comments.is_empty()
    }
}

/// Reads the gap `range` of `source`, which must hold only whitespace and
/// comments. Anything else means the parser did not record a token there,
/// and the bytes would have no owner.
pub fn gap(source: &str, range: Range<usize>) -> Result<Gap, usize> {
    let mut out = Gap {
        spaced: !range.is_empty(),
        ..Gap::default()
    };
    for lexeme in lex(&source[range.clone()], range.start) {
        let text = &source[lexeme.range.clone()];
        match lexeme.kind {
            LexKind::Whitespace => {
                out.newlines_after += text.matches('\n').count();
                out.spaced_after = true;
            }
            LexKind::LineComment | LexKind::BlockComment => {
                out.comments.push(Comment {
                    range: lexeme.range,
                    line: lexeme.kind == LexKind::LineComment,
                    newlines_before: std::mem::take(&mut out.newlines_after),
                    spaced_before: std::mem::take(&mut out.spaced_after),
                });
            }
            _ => return Err(lexeme.range.start),
        }
    }
    Ok(out)
}
