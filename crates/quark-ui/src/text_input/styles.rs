//! Inline formatting: bold, italic, underline, strikethrough, code, and
//! links as a column of style runs beside the text.
//!
//! The buffer keeps a sorted list of [`StyleSpan`]s. Unlike atoms, runs
//! split and shrink freely under edits: every byte has one [`TextFormat`]
//! (plain where no run covers it), and the list is kept canonical (no empty
//! or plain runs, no two touching runs of the same format) so that undoing
//! back to a state reproduces the same list.
//!
//! Text with formatting leaves the app as Markdown or HTML
//! ([`RichExport`]); see [`super::RichText::export_as`].

use std::fmt::Write as _;
use std::ops::Range;
use std::sync::Arc;

use quark_text::offset;

use super::atoms::InlineAtom;

/// A set of inline style flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub struct InlineStyle(u8);

impl InlineStyle {
    pub const PLAIN: Self = Self(0);
    pub const BOLD: Self = Self(1);
    pub const ITALIC: Self = Self(1 << 1);
    pub const UNDERLINE: Self = Self(1 << 2);
    pub const STRIKE: Self = Self(1 << 3);
    pub const CODE: Self = Self(1 << 4);

    /// Every flag, in the order exports open them (outermost first; code
    /// innermost because nothing nests inside a Markdown code span).
    const ORDER: [Self; 5] = [
        Self::BOLD,
        Self::ITALIC,
        Self::STRIKE,
        Self::UNDERLINE,
        Self::CODE,
    ];

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }

    pub const fn is_plain(self) -> bool {
        self.0 == 0
    }
}

/// Everything formatting says about one byte.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct TextFormat {
    pub style: InlineStyle,
    /// Link target, when the text is a link.
    pub link: Option<Arc<str>>,
}

impl TextFormat {
    pub fn is_plain(&self) -> bool {
        self.style.is_plain() && self.link.is_none()
    }
}

/// `range` (bytes, on char boundaries) of the text has `format`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyleSpan {
    pub range: Range<usize>,
    pub format: TextFormat,
}

impl StyleSpan {
    pub fn new(range: Range<usize>, style: InlineStyle) -> Self {
        Self {
            range,
            format: TextFormat { style, link: None },
        }
    }

    pub fn link(range: Range<usize>, target: impl Into<Arc<str>>) -> Self {
        Self {
            range,
            format: TextFormat {
                style: InlineStyle::PLAIN,
                link: Some(target.into()),
            },
        }
    }

    fn shifted(&self, by: isize) -> Self {
        let shift = |o: usize| o.saturating_add_signed(by);
        Self {
            range: shift(self.range.start)..shift(self.range.end),
            format: self.format.clone(),
        }
    }
}

/// How formatted text is written for other apps (copy).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum RichExport {
    /// `**bold**`, `*italic*`, `~~strike~~`, `<u>underline</u>`,
    /// `` `code` ``, `[text](target)`.
    #[default]
    Markdown,
    /// `<b>`, `<i>`, `<s>`, `<u>`, `<code>`, `<a href>`, with the text
    /// escaped.
    Html,
}

/// A broken style run list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleIntegrityError {
    /// Run `index` is empty or reaches past the text.
    OutOfBounds { index: usize },
    /// Run `index` starts or ends inside a char.
    OffBoundary { index: usize },
    /// Run `index` starts before run `index - 1` ends.
    Overlap { index: usize },
    /// Run `index` is plain, or touches run `index - 1` with the same
    /// format, so the list is not canonical.
    NotCanonical { index: usize },
}

impl std::fmt::Display for StyleIntegrityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::OutOfBounds { index } => write!(f, "style run {index} is empty or out of bounds"),
            Self::OffBoundary { index } => write!(f, "style run {index} is off a char boundary"),
            Self::Overlap { index } => write!(f, "style run {index} overlaps the one before it"),
            Self::NotCanonical { index } => {
                write!(
                    f,
                    "style run {index} is plain or should merge with the one before"
                )
            }
        }
    }
}

impl std::error::Error for StyleIntegrityError {}

pub(super) fn verify(text: &str, spans: &[StyleSpan]) -> Result<(), StyleIntegrityError> {
    let mut prev: Option<&StyleSpan> = None;
    for (index, span) in spans.iter().enumerate() {
        let Range { start, end } = span.range;
        if start >= end || end > text.len() {
            return Err(StyleIntegrityError::OutOfBounds { index });
        }
        if !text.is_char_boundary(start) || !text.is_char_boundary(end) {
            return Err(StyleIntegrityError::OffBoundary { index });
        }
        if let Some(prev) = prev {
            if start < prev.range.end {
                return Err(StyleIntegrityError::Overlap { index });
            }
            if start == prev.range.end && span.format == prev.format {
                return Err(StyleIntegrityError::NotCanonical { index });
            }
        }
        if span.format.is_plain() {
            return Err(StyleIntegrityError::NotCanonical { index });
        }
        prev = Some(span);
    }
    Ok(())
}

/// Drop plain and empty runs and merge touching runs of one format.
fn canonicalize(spans: &mut Vec<StyleSpan>) {
    spans.retain(|s| !s.range.is_empty() && !s.format.is_plain());
    let mut out: Vec<StyleSpan> = Vec::with_capacity(spans.len());
    for span in spans.drain(..) {
        match out.last_mut() {
            Some(last) if last.range.end == span.range.start && last.format == span.format => {
                last.range.end = span.range.end;
            }
            _ => out.push(span),
        }
    }
    *spans = out;
}

/// The buffer's style runs, canonical (see the module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct StyleList(Vec<StyleSpan>);

impl StyleList {
    pub(super) fn as_slice(&self) -> &[StyleSpan] {
        &self.0
    }

    pub(super) fn clear(&mut self) {
        self.0.clear();
    }

    /// Replace the runs; `spans` need not be canonical.
    pub(super) fn set(&mut self, spans: &[StyleSpan]) {
        self.0.clear();
        self.0.extend_from_slice(spans);
        self.0.sort_by_key(|s| s.range.start);
        canonicalize(&mut self.0);
    }

    pub(super) fn verify_integrity(&self, text: &str) -> Result<(), StyleIntegrityError> {
        verify(text, &self.0)
    }

    /// The format of the byte at `at`.
    pub(super) fn format_at(&self, at: usize) -> TextFormat {
        self.0
            .iter()
            .find(|s| s.range.contains(&at))
            .map(|s| s.format.clone())
            .unwrap_or_default()
    }

    /// Runs clipped to `range`, relative to its start.
    pub(super) fn slice(&self, range: Range<usize>) -> Vec<StyleSpan> {
        self.0
            .iter()
            .filter(|s| s.range.start < range.end && range.start < s.range.end)
            .map(|s| StyleSpan {
                range: s.range.start.max(range.start) - range.start
                    ..s.range.end.min(range.end) - range.start,
                format: s.format.clone(),
            })
            .collect()
    }

    /// Mirror a text replacement: `removed_len` bytes at `at` became
    /// `inserted_len` bytes formatted by `inserted` (relative runs). Runs
    /// straddling the removed bytes are cut there. Returns the removed
    /// bytes' runs, relative to `at`.
    pub(super) fn splice(
        &mut self,
        at: usize,
        removed_len: usize,
        inserted: &[StyleSpan],
        inserted_len: usize,
    ) -> Vec<StyleSpan> {
        let end = at + removed_len;
        let removed = self.slice(at..end);
        let delta = inserted_len as isize - removed_len as isize;
        let mut out = Vec::with_capacity(self.0.len() + inserted.len() + 1);
        for span in self.0.drain(..) {
            if span.range.start < at {
                out.push(StyleSpan {
                    range: span.range.start..span.range.end.min(at),
                    format: span.format.clone(),
                });
            }
            if span.range.end > end {
                let start = span.range.start.max(end);
                out.push(
                    StyleSpan {
                        range: start..span.range.end,
                        format: span.format,
                    }
                    .shifted(delta),
                );
            }
        }
        out.extend(inserted.iter().map(|s| s.shifted(at as isize)));
        out.sort_by_key(|s| s.range.start);
        canonicalize(&mut out);
        self.0 = out;
        removed
    }
}

/// `range`'s runs (relative to it) after `change` is applied to the format
/// of every byte in it.
pub(super) fn restyled(
    current: &[StyleSpan],
    len: usize,
    change: impl Fn(&mut TextFormat),
) -> Vec<StyleSpan> {
    let mut out = Vec::new();
    let mut at = 0;
    let mut push = |range: Range<usize>, mut format: TextFormat| {
        if !range.is_empty() {
            change(&mut format);
            out.push(StyleSpan { range, format });
        }
    };
    for span in current {
        push(at..span.range.start, TextFormat::default());
        push(span.range.clone(), span.format.clone());
        at = span.range.end;
    }
    push(at..len, TextFormat::default());
    canonicalize(&mut out);
    out
}

/// Write `text` with `atoms` replaced by their exports and `styles` as
/// markup in `format`. Plain text is escaped only when there is markup to
/// keep apart from it.
pub(super) fn export(
    text: &str,
    atoms: &[InlineAtom],
    styles: &[StyleSpan],
    format: RichExport,
) -> String {
    let mut out = String::with_capacity(text.len() + 16 * styles.len());
    let escape = !styles.is_empty() || format == RichExport::Html;
    let mut writer = MarkupWriter {
        out: &mut out,
        format,
        open: Vec::new(),
    };
    let mut at = 0;
    let mut pieces = Vec::with_capacity(styles.len() * 2 + 1);
    for span in styles {
        pieces.push((at..span.range.start, TextFormat::default()));
        pieces.push((span.range.clone(), span.format.clone()));
        at = span.range.end;
    }
    pieces.push((at..text.len(), TextFormat::default()));
    for (range, piece_format) in pieces {
        if range.is_empty() {
            continue;
        }
        // Markup does not cross line breaks (a blank line ends emphasis).
        let piece = offset::slice(text, range.clone());
        let mut line_start = range.start;
        for line in piece.split_inclusive('\n') {
            let body = line.strip_suffix('\n').unwrap_or(line);
            let body_range = line_start..line_start + body.len();
            if !body.is_empty() {
                writer.set(&piece_format, body);
                let code = piece_format.style.contains(InlineStyle::CODE);
                write_text(writer.out, text, body_range, atoms, escape && !code, format);
            }
            if body.len() < line.len() {
                writer.set(&TextFormat::default(), "");
                writer.out.push('\n');
            }
            line_start += line.len();
        }
    }
    writer.set(&TextFormat::default(), "");
    out
}

/// One open markup element.
#[derive(Debug, Clone, PartialEq)]
enum Open {
    Link(Arc<str>),
    Style(InlineStyle),
}

struct MarkupWriter<'a> {
    out: &'a mut String,
    format: RichExport,
    open: Vec<(Open, String)>,
}

impl MarkupWriter<'_> {
    /// Close and open elements so that `target` is what is open; `next` is
    /// the text about to be written (for sizing a code fence).
    fn set(&mut self, target: &TextFormat, next: &str) {
        let wanted = wanted(target);
        // Keep the longest prefix of what is open that `wanted` starts with.
        let keep = self
            .open
            .iter()
            .zip(&wanted)
            .take_while(|((open, _), want)| open == *want)
            .count();
        while self.open.len() > keep {
            if let Some((_, close)) = self.open.pop() {
                self.out.push_str(&close);
            }
        }
        for element in wanted.into_iter().skip(keep) {
            let (open, close) = markup(&element, self.format, next);
            self.out.push_str(&open);
            self.open.push((element, close));
        }
    }
}

fn wanted(format: &TextFormat) -> Vec<Open> {
    let mut wanted = Vec::new();
    if let Some(link) = &format.link {
        wanted.push(Open::Link(link.clone()));
    }
    for flag in InlineStyle::ORDER {
        if format.style.contains(flag) {
            wanted.push(Open::Style(flag));
        }
    }
    wanted
}

/// Opening and closing markup for `element`.
fn markup(element: &Open, format: RichExport, next: &str) -> (String, String) {
    let pair = |a: &str, b: &str| (a.to_owned(), b.to_owned());
    match (format, element) {
        (RichExport::Markdown, Open::Link(target)) => ("[".to_owned(), format!("]({target})")),
        (RichExport::Html, Open::Link(target)) => {
            let mut open = String::from("<a href=\"");
            escape_html(&mut open, target);
            open.push_str("\">");
            (open, "</a>".to_owned())
        }
        (RichExport::Markdown, Open::Style(style)) => match *style {
            InlineStyle::BOLD => pair("**", "**"),
            InlineStyle::ITALIC => pair("*", "*"),
            InlineStyle::STRIKE => pair("~~", "~~"),
            InlineStyle::UNDERLINE => pair("<u>", "</u>"),
            _ => {
                // A fence longer than any backtick run inside, padded when
                // the code starts or ends with one.
                let longest = next.split(|c| c != '`').map(str::len).max().unwrap_or(0);
                let fence = "`".repeat(longest + 1);
                let pad = if next.starts_with('`') || next.ends_with('`') {
                    " "
                } else {
                    ""
                };
                (format!("{fence}{pad}"), format!("{pad}{fence}"))
            }
        },
        (RichExport::Html, Open::Style(style)) => match *style {
            InlineStyle::BOLD => pair("<b>", "</b>"),
            InlineStyle::ITALIC => pair("<i>", "</i>"),
            InlineStyle::STRIKE => pair("<s>", "</s>"),
            InlineStyle::UNDERLINE => pair("<u>", "</u>"),
            _ => pair("<code>", "</code>"),
        },
    }
}

/// Write `range` of `text`, putting each atom starting in it as its export
/// and skipping atom bytes otherwise.
fn write_text(
    out: &mut String,
    text: &str,
    range: Range<usize>,
    atoms: &[InlineAtom],
    escape: bool,
    format: RichExport,
) {
    let mut at = range.start;
    let push = |out: &mut String, s: &str| match (escape, format) {
        (false, RichExport::Markdown) => out.push_str(s),
        (true, RichExport::Markdown) => escape_markdown(out, s),
        (_, RichExport::Html) => escape_html(out, s),
    };
    for atom in atoms
        .iter()
        .filter(|a| a.range.start < range.end && range.start < a.range.end)
    {
        if atom.range.start > at {
            push(out, offset::slice(text, at..atom.range.start));
        }
        if atom.range.start >= range.start {
            match format {
                RichExport::Markdown => out.push_str(&atom.export),
                RichExport::Html => escape_html(out, &atom.export),
            }
        }
        at = atom.range.end.min(range.end).max(at);
    }
    if at < range.end {
        push(out, offset::slice(text, at..range.end));
    }
}

fn escape_markdown(out: &mut String, s: &str) {
    for c in s.chars() {
        if matches!(c, '\\' | '*' | '_' | '~' | '`' | '[' | ']' | '<') {
            out.push('\\');
        }
        out.push(c);
    }
}

fn escape_html(out: &mut String, s: &str) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => {
                let _ = out.write_char(c);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::super::{Editor, RichClipboard, RichText, TextEditCommand};
    use super::*;
    use TextEditCommand::*;

    const B: InlineStyle = InlineStyle::BOLD;
    const I: InlineStyle = InlineStyle::ITALIC;

    /// Flag letters in a marked run: `[bi:text]` is bold italic, `l` a
    /// link to `url`.
    const FLAGS: [(char, InlineStyle); 5] = [
        ('b', InlineStyle::BOLD),
        ('i', InlineStyle::ITALIC),
        ('u', InlineStyle::UNDERLINE),
        ('s', InlineStyle::STRIKE),
        ('c', InlineStyle::CODE),
    ];

    /// Rich text from `marked`: `[flags:text]` is a run, `|` the caret,
    /// `^` the anchor.
    fn parse(marked: &str) -> (RichText, usize, Option<usize>) {
        let mut rich = RichText::default();
        let (mut caret, mut anchor) = (0, None);
        let mut run: Option<(usize, TextFormat)> = None;
        let mut chars = marked.chars();
        while let Some(ch) = chars.next() {
            match ch {
                '|' => caret = rich.text.len(),
                '^' => anchor = Some(rich.text.len()),
                '[' => {
                    let mut format = TextFormat::default();
                    for flag in chars.by_ref().take_while(|&c| c != ':') {
                        match FLAGS.iter().find(|(c, _)| *c == flag) {
                            Some((_, style)) => format.style = format.style.union(*style),
                            None => format.link = Some("url".into()),
                        }
                    }
                    run = Some((rich.text.len(), format));
                }
                ']' => {
                    let (start, format) = run.take().expect("open run");
                    rich.styles.push(StyleSpan {
                        range: start..rich.text.len(),
                        format,
                    });
                }
                _ => rich.text.push(ch),
            }
        }
        (rich, caret, anchor)
    }

    fn editor(marked: &str) -> Editor {
        let (rich, caret, anchor) = parse(marked);
        let mut editor = Editor::default();
        editor.set_rich_text(&rich);
        editor.apply(SetTextCursor(anchor.unwrap_or(caret)));
        editor.apply(ExtendTextSelection(caret));
        editor
    }

    /// The editor in the notation [`parse`] reads.
    fn dump(editor: &Editor) -> String {
        let text = editor.text();
        let styles = editor.styles();
        let mut out = String::new();
        for (i, ch) in text.char_indices().chain([(text.len(), '\0')]) {
            if styles.iter().any(|s| s.range.end == i) {
                out.push(']');
            }
            if editor.anchor() == i && editor.anchor() != editor.cursor() {
                out.push('^');
            }
            if editor.cursor() == i {
                out.push('|');
            }
            if let Some(span) = styles.iter().find(|s| s.range.start == i) {
                out.push('[');
                for (letter, style) in FLAGS {
                    if span.format.style.contains(style) {
                        out.push(letter);
                    }
                }
                if span.format.link.is_some() {
                    out.push('l');
                }
                out.push(':');
            }
            if ch != '\0' {
                out.push(ch);
            }
        }
        out
    }

    #[test]
    fn style_edits_and_undo() {
        let cases: &[(&str, &str, &[TextEditCommand], &str)] = &[
            (
                "bold over a selection",
                "a ^bc| d",
                &[ToggleStyle(B)],
                "a ^[b:bc]| d",
            ),
            (
                "off when all of it is bold",
                "^[b:abc]|",
                &[ToggleStyle(B)],
                "^abc|",
            ),
            (
                "on when part of it is",
                "^a[b:b]c|",
                &[ToggleStyle(B)],
                "^[b:abc]|",
            ),
            (
                "a flag joins the others",
                "[b:a^bc|]",
                &[ToggleStyle(I)],
                "[b:a]^[bi:bc]|",
            ),
            (
                "typing inside a run",
                "[b:ab|c]",
                &[InsertText("x".into())],
                "[b:abx|c]",
            ),
            (
                "typing after a run",
                "[b:ab]|",
                &[InsertText("x".into())],
                "[b:abx]|",
            ),
            (
                "typing after a link",
                "[l:ab]|",
                &[InsertText("x".into())],
                "[l:ab]x|",
            ),
            (
                "typing inside a link",
                "[l:a|b]",
                &[InsertText("x".into())],
                "[l:ax|b]",
            ),
            (
                "toggle at the caret, then type",
                "a|",
                &[ToggleStyle(I), InsertText("bc".into())],
                "a[i:bc]|",
            ),
            (
                "moving drops a toggle at the caret",
                "a|b",
                &[ToggleStyle(B), CursorRight, InsertText("x".into())],
                "abx|",
            ),
            (
                "deleting shrinks a run",
                "[b:a^bc|]d",
                &[Backspace],
                "[b:a]|d",
            ),
            (
                "deleting a run removes it",
                "x^[b:ab]|y",
                &[Backspace],
                "x|y",
            ),
            (
                "joined runs of one style merge",
                "[b:a]^x|[b:b]",
                &[Backspace],
                "[b:a|b]",
            ),
            (
                "undo of a toggle",
                "a ^bc| d",
                &[ToggleStyle(B), Undo],
                "a ^bc| d",
            ),
            (
                "undo of a delete restores the run",
                "[b:a^bc|]d",
                &[Backspace, Undo],
                "[b:a^bc]|d",
            ),
            (
                "redo of a toggle",
                "^ab|",
                &[ToggleStyle(B), Undo, Redo],
                "^[b:ab]|",
            ),
            (
                "a link over a selection",
                "go ^here|",
                &[SetLink(Some("url".into()))],
                "go ^[l:here]|",
            ),
        ];
        for (name, before, commands, after) in cases {
            let mut e = editor(before);
            for command in commands.iter() {
                e.apply(command.clone());
            }
            assert_eq!(dump(&e), *after, "{name}");
        }
    }

    #[test]
    fn copied_formatting_pastes_back_and_exports_as_markup() {
        let clipboard = RichClipboard::default();
        let mut source = editor("^[b:bold] and [i:it]|");
        source.set_rich_clipboard(clipboard.clone());
        let exported = source.apply(Copy).clipboard_write;
        assert_eq!(exported.as_deref(), Some("**bold** and *it*"));

        let mut target = editor("> |");
        target.set_rich_clipboard(clipboard.clone());
        target.apply(Paste("**bold** and *it*".into()));
        assert_eq!(dump(&target), "> [b:bold] and [i:it]|");

        // Text from elsewhere takes the format at the caret.
        let mut plain = editor("[b:x|]");
        plain.set_rich_clipboard(clipboard);
        plain.apply(Paste("**y**".into()));
        assert_eq!(dump(&plain), "[b:x**y**]|");
    }

    #[test]
    fn export_table() {
        use RichExport::*;
        let cases = [
            ("nested", "[b:a][bi:b][b:c]", Markdown, "**a*b*c**"),
            ("link around bold", "[l:a][bl:b]", Markdown, "[a**b**](url)"),
            ("code with a backtick", "[c:a`b]", Markdown, "``a`b``"),
            ("code starting with one", "[c:`a]", Markdown, "`` `a ``"),
            ("plain text is escaped", "[b:a]*_", Markdown, "**a**\\*\\_"),
            ("code is not escaped", "[c:*]", Markdown, "`*`"),
            (
                "markup stops at line ends",
                "[b:a\nb]",
                Markdown,
                "**a**\n**b**",
            ),
            ("underline and strike", "[us:a]", Markdown, "~~<u>a</u>~~"),
            (
                "html",
                "[b:<a>][l:&]",
                Html,
                "<b>&lt;a&gt;</b><a href=\"url\">&amp;</a>",
            ),
            ("html code", "x[ci:y]", Html, "x<i><code>y</code></i>"),
        ];
        for (name, marked, format, expected) in cases {
            let (rich, _, _) = parse(marked);
            assert_eq!(rich.verify_integrity(), Ok(()), "{name}");
            assert_eq!(rich.export_as(format), expected, "{name}");
        }
    }

    fn command() -> impl Strategy<Value = TextEditCommand> {
        let raw = 0usize..20;
        prop_oneof![
            prop::sample::select(vec!["a", " ", "\u{e9}", "\n"]).prop_map(|s| InsertText(s.into())),
            raw.clone().prop_map(SetTextCursor),
            raw.prop_map(ExtendTextSelection),
            prop::sample::select(vec![
                ToggleStyle(B),
                ToggleStyle(I),
                ToggleStyle(InlineStyle::CODE),
                SetLink(Some("u".into())),
                SetLink(None),
                Backspace,
                DeleteForwardWord,
                Cut,
                Undo,
                Redo,
            ]),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(128))]

        // Catches an edit or toggle leaving the runs non-canonical (the
        // buffer's integrity check fails inside `apply`), or undo failing
        // to bring back the exact runs it started from.
        #[test]
        fn undoing_everything_restores_the_formatting(
            commands in prop::collection::vec(command(), 1..32),
        ) {
            let mut e = editor("[b:ab] c[il:de]f");
            let initial = e.rich_text();
            for cmd in commands {
                e.apply(cmd);
            }
            while e.can_undo() {
                e.apply(Undo);
            }
            prop_assert_eq!(e.rich_text(), initial);
        }
    }
}
