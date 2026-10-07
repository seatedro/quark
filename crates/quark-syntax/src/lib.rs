//! Tree-sitter syntax highlighting for code blocks, ported from Diffy's
//! `phosphor` crate with the grammars compiled in instead of loaded from
//! downloaded packs.
//!
//! Each language is a cargo feature (`rust`, `javascript`, `typescript`,
//! `python`, `bash`, `json`, `go`, or `common` for all of them). A language
//! whose feature is off is unknown: [`LanguageId::from_fence`] returns
//! `None` and callers render plain text.
//!
//! [`highlight`] runs synchronously; [`HighlightWorker`] runs it on a
//! background thread and drops requests superseded by a newer generation
//! for the same slot, which suits a code block that is still streaming.

#[cfg(feature = "engine")]
mod engine;
mod worker;

pub use worker::{HighlightWorker, Highlighted};

/// What a highlighted run of source is.
#[repr(u8)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    #[default]
    Normal = 0,
    Keyword,
    String,
    Comment,
    Number,
    Type,
    Function,
    Operator,
    Punctuation,
    Variable,
    Constant,
    Builtin,
    Attribute,
    Tag,
    Property,
    Namespace,
    Label,
    Preprocessor,
}

impl HighlightKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::Keyword => "keyword",
            Self::String => "string",
            Self::Comment => "comment",
            Self::Number => "number",
            Self::Type => "type",
            Self::Function => "function",
            Self::Operator => "operator",
            Self::Punctuation => "punctuation",
            Self::Variable => "variable",
            Self::Constant => "constant",
            Self::Builtin => "builtin",
            Self::Attribute => "attribute",
            Self::Tag => "tag",
            Self::Property => "property",
            Self::Namespace => "namespace",
            Self::Label => "label",
            Self::Preprocessor => "preprocessor",
        }
    }
}

/// A highlighted byte range of the source. Spans are sorted and do not
/// overlap; bytes outside every span are [`HighlightKind::Normal`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HighlightSpan {
    pub offset: u32,
    pub length: u32,
    pub kind: HighlightKind,
}

impl HighlightSpan {
    pub fn range(self) -> std::ops::Range<usize> {
        self.offset as usize..(self.offset + self.length) as usize
    }
}

/// A compiled-in language.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageId {
    Bash,
    Go,
    JavaScript,
    Json,
    Python,
    Rust,
    TypeScript,
}

impl LanguageId {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Bash => "bash",
            Self::Go => "go",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
        }
    }

    /// The language a Markdown fence tag (```` ```rs ````) names, matched
    /// case-insensitively, when its grammar is compiled in.
    pub fn from_fence(tag: &str) -> Option<Self> {
        let language = match tag.to_ascii_lowercase().as_str() {
            "sh" | "bash" | "shell" | "zsh" | "console" => Self::Bash,
            "go" | "golang" => Self::Go,
            "js" | "javascript" | "jsx" | "mjs" | "cjs" => Self::JavaScript,
            "json" | "jsonc" => Self::Json,
            "py" | "python" | "python3" => Self::Python,
            "rs" | "rust" => Self::Rust,
            "ts" | "typescript" | "mts" | "cts" => Self::TypeScript,
            _ => return None,
        };
        language.is_compiled().then_some(language)
    }

    /// Whether this language's grammar feature is on.
    pub const fn is_compiled(self) -> bool {
        match self {
            Self::Bash => cfg!(feature = "bash"),
            Self::Go => cfg!(feature = "go"),
            Self::JavaScript => cfg!(feature = "javascript"),
            Self::Json => cfg!(feature = "json"),
            Self::Python => cfg!(feature = "python"),
            Self::Rust => cfg!(feature = "rust"),
            Self::TypeScript => cfg!(feature = "typescript"),
        }
    }
}

impl std::fmt::Display for LanguageId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// Highlights `source`. Empty when the language is not compiled in or its
/// query fails to load; highlighting is best effort.
pub fn highlight(language: LanguageId, source: &str) -> Vec<HighlightSpan> {
    #[cfg(feature = "engine")]
    {
        engine::highlight(language, source)
    }
    #[cfg(not(feature = "engine"))]
    {
        let _ = (language, source);
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
