//! The document IR the template printer builds, and its width-aware
//! printer.
//!
//! A [`Doc`] is text plus break opportunities. A [`Doc::Group`] prints
//! flat when its whole content fits on the current line and breaks every
//! one of its own [`Doc::Line`]s otherwise (a consistent break); nested
//! groups decide for themselves. Hard breaks, line comments, multi-line
//! literals, and embedded Rust without a one-line form force every
//! enclosing group to break ([`propagate`]). Embedded Rust is resolved
//! lazily through [`Embeds`], because its layout depends on the column
//! where it lands.
//!
//! The printer produces lines with absolute indentation instead of a
//! string, so nested views can be re-based under an enclosing fragment and
//! lines inside literals stay marked as verbatim.

use crate::printer::rust::{Layout, LayoutLine, LayoutRequest};
use crate::source::columns;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Doc {
    /// Text without newlines.
    Text(String),
    /// Exact source bytes that may contain newlines: literals and block
    /// comments. Lines after the first are emitted without indentation.
    Verbatim(String),
    /// A space when flat, a newline when broken.
    Line,
    /// Nothing when flat, a newline when broken.
    SoftLine,
    /// Always a newline.
    HardLine,
    /// An empty line, then a newline.
    BlankLine,
    /// A `//` comment: the next content goes on a new line.
    LineComment(String),
    /// Content one indentation unit deeper after its newlines.
    Indent(Box<Doc>),
    /// A consistent-break group. `broken` is set by [`propagate`].
    Group {
        doc: Box<Doc>,
        broken: bool,
    },
    Concat(Vec<Doc>),
    /// Embedded Rust, by id in the view's [`Embeds`].
    Embed(usize),
    /// Width that counts when measuring but prints nothing: text after the
    /// invocation on its last line.
    Phantom(usize),
}

impl Doc {
    pub fn text(s: impl Into<String>) -> Doc {
        Doc::Text(s.into())
    }

    pub fn group(doc: Doc) -> Doc {
        Doc::Group {
            doc: Box::new(doc),
            broken: false,
        }
    }

    pub fn indent(doc: Doc) -> Doc {
        Doc::Indent(Box::new(doc))
    }

    pub fn concat(docs: impl IntoIterator<Item = Doc>) -> Doc {
        let mut out = Vec::new();
        for d in docs {
            match d {
                Doc::Concat(inner) => out.extend(inner),
                Doc::Text(s) if s.is_empty() => {}
                d => out.push(d),
            }
        }
        if out.len() == 1 {
            return out.pop().expect("one element");
        }
        Doc::Concat(out)
    }

    pub fn nil() -> Doc {
        Doc::Concat(Vec::new())
    }
}

/// Embedded Rust as the printer sees it.
pub trait Embeds {
    /// The one-line form, or `None` when the fragment must span lines.
    fn flat(&self, id: usize) -> Option<&str>;
    /// The fragment laid out for `request`; line indents are relative to
    /// `request.indent`.
    fn layout(&self, id: usize, request: &LayoutRequest) -> Layout;
}

/// Width settings for one print.
#[derive(Clone, Copy, Debug)]
pub struct PrintConfig {
    pub max_width: usize,
    pub tab_spaces: usize,
}

/// Marks every group that must break and returns whether `doc` forces a
/// break on whatever encloses it.
pub fn propagate(doc: &mut Doc, embeds: &dyn Embeds) -> bool {
    match doc {
        Doc::Text(_) | Doc::Line | Doc::SoftLine | Doc::Phantom(_) => false,
        Doc::Verbatim(s) => s.contains('\n'),
        Doc::HardLine | Doc::BlankLine | Doc::LineComment(_) => true,
        Doc::Indent(d) => propagate(d, embeds),
        Doc::Group { doc, broken } => {
            *broken |= propagate(doc, embeds);
            *broken
        }
        Doc::Concat(v) => v
            .iter_mut()
            .fold(false, |acc, d| propagate(d, embeds) | acc),
        Doc::Embed(id) => embeds.flat(*id).is_none(),
    }
}

/// Whether `doc` prints on one line with no group broken.
pub fn is_flat(doc: &Doc, embeds: &dyn Embeds) -> bool {
    match doc {
        Doc::Text(_) | Doc::Line | Doc::SoftLine | Doc::Phantom(_) => true,
        Doc::Verbatim(s) => !s.contains('\n'),
        Doc::HardLine | Doc::BlankLine | Doc::LineComment(_) => false,
        Doc::Indent(d) => is_flat(d, embeds),
        Doc::Group { doc, broken } => !broken && is_flat(doc, embeds),
        Doc::Concat(v) => v.iter().all(|d| is_flat(d, embeds)),
        Doc::Embed(id) => embeds.flat(*id).is_some(),
    }
}

/// `doc` on one line, as a flat group prints it.
pub fn print_flat(doc: &Doc, embeds: &dyn Embeds) -> Option<String> {
    fn go(doc: &Doc, embeds: &dyn Embeds, out: &mut String) -> Option<()> {
        match doc {
            Doc::Text(s) => out.push_str(s),
            Doc::Verbatim(s) if !s.contains('\n') => out.push_str(s),
            Doc::Line => out.push(' '),
            Doc::SoftLine | Doc::Phantom(_) => {}
            Doc::Indent(d) => go(d, embeds, out)?,
            Doc::Group { doc, broken: false } => go(doc, embeds, out)?,
            Doc::Concat(v) => {
                for d in v {
                    go(d, embeds, out)?;
                }
            }
            Doc::Embed(id) => out.push_str(embeds.flat(*id)?),
            _ => return None,
        }
        Some(())
    }
    let mut out = String::new();
    go(doc, embeds, &mut out)?;
    Some(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Flat,
    Break,
}

type Cmd<'d> = (usize, Mode, &'d Doc);

/// Prints `doc` starting at `column` on a line indented `indent` columns.
/// The first returned line holds what follows `column` on that line; its
/// `indent` field is `indent`. Later lines carry absolute indentation.
pub fn print(
    doc: &Doc,
    indent: usize,
    column: usize,
    cfg: PrintConfig,
    embeds: &dyn Embeds,
) -> Vec<LayoutLine> {
    let mut p = Printer {
        cfg,
        embeds,
        lines: vec![LayoutLine {
            indent,
            text: String::new(),
            verbatim: false,
        }],
        column,
        line_indent: indent,
        must_newline: false,
    };
    let mut cmds: Vec<Cmd> = vec![(indent, Mode::Break, doc)];
    while let Some((ind, mode, doc)) = cmds.pop() {
        match doc {
            Doc::Text(s) => p.text(s, ind),
            Doc::Verbatim(s) => p.verbatim(s, ind),
            Doc::Line | Doc::SoftLine if mode == Mode::Break => p.newline(ind),
            Doc::Line => p.text(" ", ind),
            Doc::SoftLine | Doc::Phantom(_) => {}
            Doc::HardLine => p.newline(ind),
            Doc::BlankLine => {
                p.newline(0);
                p.newline(ind);
            }
            Doc::LineComment(s) => {
                p.text(s, ind);
                p.must_newline = true;
            }
            Doc::Indent(d) => cmds.push((ind + cfg.tab_spaces, mode, d)),
            Doc::Concat(v) => cmds.extend(v.iter().rev().map(|d| (ind, mode, d))),
            Doc::Group { doc, broken } => {
                let flat = !broken
                    && (mode == Mode::Flat || {
                        let room = cfg.max_width as isize - p.column as isize;
                        p.fits((ind, Mode::Flat, doc), &cmds, room)
                    });
                cmds.push((ind, if flat { Mode::Flat } else { Mode::Break }, doc));
            }
            Doc::Embed(id) => p.embed(*id, ind, mode, &cmds),
        }
    }
    p.lines
}

struct Printer<'e> {
    cfg: PrintConfig,
    embeds: &'e dyn Embeds,
    lines: Vec<LayoutLine>,
    column: usize,
    /// Indentation of the current output line.
    line_indent: usize,
    /// A line comment ended the current line.
    must_newline: bool,
}

impl Printer<'_> {
    fn width(&self, s: &str) -> usize {
        columns(s, self.cfg.tab_spaces)
    }

    fn newline(&mut self, indent: usize) {
        self.lines.push(LayoutLine {
            indent,
            text: String::new(),
            verbatim: false,
        });
        self.column = indent;
        self.line_indent = indent;
        self.must_newline = false;
    }

    fn text(&mut self, s: &str, indent: usize) {
        if s.is_empty() {
            return;
        }
        if self.must_newline {
            self.newline(indent);
        }
        self.column += self.width(s);
        self.lines.last_mut().expect("a line").text.push_str(s);
    }

    fn verbatim(&mut self, s: &str, indent: usize) {
        let mut parts = s.split('\n');
        self.text(parts.next().unwrap_or_default(), indent);
        for part in parts {
            let ws = part.len() - part.trim_start_matches([' ', '\t']).len();
            self.line_indent = self.width(&part[..ws]);
            self.column = self.width(part);
            self.lines.push(LayoutLine {
                indent: 0,
                text: part.to_owned(),
                verbatim: true,
            });
        }
    }

    fn embed(&mut self, id: usize, indent: usize, mode: Mode, rest: &[Cmd]) {
        if self.must_newline {
            self.newline(indent);
        }
        let suffix = self.rest_width(rest);
        if let Some(flat) = self.embeds.flat(id) {
            let w = self.width(flat);
            if mode == Mode::Flat || self.column + w + suffix <= self.cfg.max_width {
                let flat = flat.to_owned();
                self.text(&flat, indent);
                return;
            }
        }
        let request = LayoutRequest {
            indent: self.line_indent,
            prefix: self.column - self.line_indent,
            suffix,
            max_width: self.cfg.max_width,
            tab_spaces: self.cfg.tab_spaces,
        };
        let base = request.indent;
        let layout = self.embeds.layout(id, &request);
        for (i, line) in layout.lines.into_iter().enumerate() {
            if i == 0 {
                self.text(&line.text, indent);
            } else if line.verbatim {
                self.verbatim(&format!("\n{}", line.text), indent);
            } else {
                self.newline(base + line.indent);
                self.text(&line.text, indent);
            }
        }
    }

    /// Columns the remaining commands put on the current line.
    fn rest_width(&self, rest: &[Cmd]) -> usize {
        let mut width = 0;
        let mut stack: Vec<Cmd> = Vec::new();
        let mut rest_idx = rest.len();
        loop {
            let (ind, mode, doc) = match stack.pop() {
                Some(c) => c,
                None if rest_idx == 0 => return width,
                None => {
                    rest_idx -= 1;
                    rest[rest_idx]
                }
            };
            match doc {
                Doc::Text(s) | Doc::LineComment(s) => width += self.width(s),
                Doc::Verbatim(s) => {
                    width += self.width(s.split('\n').next().unwrap_or_default());
                    if s.contains('\n') {
                        return width;
                    }
                }
                Doc::Line if mode == Mode::Flat => width += 1,
                Doc::SoftLine if mode == Mode::Flat => {}
                Doc::Line | Doc::SoftLine | Doc::HardLine | Doc::BlankLine => return width,
                Doc::Phantom(n) => width += n,
                Doc::Indent(d) => stack.push((ind, mode, d)),
                Doc::Group { doc, broken } => {
                    let m = if *broken { Mode::Break } else { mode };
                    stack.push((ind, m, doc));
                }
                Doc::Concat(v) => stack.extend(v.iter().rev().map(|d| (ind, mode, d))),
                Doc::Embed(id) => match self.embeds.flat(*id) {
                    Some(s) => width += self.width(s),
                    None => return width,
                },
            }
        }
    }

    /// Whether `next` printed flat, followed by the rest of the current
    /// line from `rest`, fits in `room` columns.
    fn fits(&self, next: Cmd, rest: &[Cmd], mut room: isize) -> bool {
        let mut stack = vec![next];
        let mut rest_idx = rest.len();
        loop {
            if room < 0 {
                return false;
            }
            let (ind, mode, doc) = match stack.pop() {
                Some(c) => c,
                None if rest_idx == 0 => return true,
                None => {
                    rest_idx -= 1;
                    rest[rest_idx]
                }
            };
            match doc {
                Doc::Text(s) => room -= self.width(s) as isize,
                Doc::Verbatim(s) => {
                    room -= self.width(s.split('\n').next().unwrap_or_default()) as isize;
                    if s.contains('\n') {
                        return room >= 0;
                    }
                }
                Doc::LineComment(s) => return room - self.width(s) as isize >= 0,
                Doc::Line if mode == Mode::Flat => room -= 1,
                Doc::SoftLine if mode == Mode::Flat => {}
                Doc::Line | Doc::SoftLine | Doc::HardLine | Doc::BlankLine => return true,
                Doc::Phantom(n) => room -= *n as isize,
                Doc::Indent(d) => stack.push((ind, mode, d)),
                Doc::Group { doc, broken } => {
                    let m = if *broken { Mode::Break } else { mode };
                    stack.push((ind, m, doc));
                }
                Doc::Concat(v) => stack.extend(v.iter().rev().map(|d| (ind, mode, d))),
                Doc::Embed(id) => match self.embeds.flat(*id) {
                    Some(s) => room -= self.width(s) as isize,
                    None => return true,
                },
            }
        }
    }
}

/// Joins printed lines into text: indentation in the configured style,
/// `newline` between lines, a bare `\n` before verbatim lines (whose
/// literal keeps its own `\r`), and no indentation on empty lines.
pub fn render(lines: &[LayoutLine], tab_spaces: usize, hard_tabs: bool, newline: &str) -> String {
    let mut out = String::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push_str(if line.verbatim { "\n" } else { newline });
            if !line.verbatim && !line.text.is_empty() {
                out.push_str(&crate::source::indent_string(
                    line.indent,
                    tab_spaces,
                    hard_tabs,
                ));
            }
        }
        out.push_str(&line.text);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    struct NoEmbeds;
    impl Embeds for NoEmbeds {
        fn flat(&self, _: usize) -> Option<&str> {
            None
        }
        fn layout(&self, _: usize, _: &LayoutRequest) -> Layout {
            Layout::default()
        }
    }

    fn show(mut doc: Doc, width: usize) -> String {
        propagate(&mut doc, &NoEmbeds);
        let cfg = PrintConfig {
            max_width: width,
            tab_spaces: 4,
        };
        render(&print(&doc, 0, 0, cfg, &NoEmbeds), 4, false, "\n")
    }

    /// `<a x y>`: an opening tag as the template printer builds one.
    fn tag(attrs: &[&str]) -> Doc {
        let mut inner = Vec::new();
        for a in attrs {
            inner.push(Doc::Line);
            inner.push(Doc::text(*a));
        }
        Doc::group(Doc::concat([
            Doc::text("<a"),
            Doc::indent(Doc::concat(inner)),
            Doc::SoftLine,
            Doc::text(">"),
        ]))
    }

    #[test]
    fn a_group_that_overflows_breaks_every_line_it_owns() {
        let attrs = ["one=1", "two=2", "three=3"];
        assert_eq!(show(tag(&attrs), 80), "<a one=1 two=2 three=3>");
        // 23 columns flat; at 22 none of the attributes share a line.
        assert_eq!(
            show(tag(&attrs), 22),
            "<a\n    one=1\n    two=2\n    three=3\n>"
        );
    }

    #[test]
    fn a_line_comment_breaks_its_group_and_ends_its_line() {
        let doc = Doc::group(Doc::concat([
            Doc::text("<a"),
            Doc::indent(Doc::concat([
                Doc::Line,
                Doc::text("x=1"),
                Doc::text(" "),
                Doc::LineComment("// why".into()),
                Doc::text("y=2"),
            ])),
            Doc::SoftLine,
            Doc::text(">"),
        ]));
        assert_eq!(show(doc, 80), "<a\n    x=1 // why\n    y=2\n>");
    }

    #[test]
    fn verbatim_lines_keep_their_bytes_and_skip_indentation() {
        let doc = Doc::indent(Doc::concat([
            Doc::HardLine,
            Doc::Verbatim("\"a\n  b\"".into()),
            Doc::HardLine,
            Doc::text("c"),
        ]));
        assert_eq!(show(doc, 80), "\n    \"a\n  b\"\n    c");
    }
}
