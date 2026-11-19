//! The rope against a `String` doing the same edits: every query must
//! agree, and the B-tree must stay valid after each edit.

use proptest::prelude::*;
use quark_text::{TextOffset, offset};

use super::*;

/// Text pieces that stress chunk and line edges: multi-byte chars,
/// combining marks, emoji sequences, CJK, and both line endings.
const PIECES: &[&str] = &[
    "a",
    "word ",
    " ",
    "\n",
    "\r\n",
    "\r",
    ".",
    "foo.bar",
    "_x",
    "\u{e9}",
    "e\u{301}",
    "\u{65e5}\u{672c}",
    "\u{1f44d}\u{1f3fd}",
    "\u{1f469}\u{200d}\u{1f4bb}",
    "\u{1f1ef}\u{1f1f5}",
];

fn text(max_pieces: usize) -> impl Strategy<Value = String> {
    prop::collection::vec(prop::sample::select(PIECES), 0..max_pieces).prop_map(|p| p.concat())
}

/// A long text: a few pieces repeated so the rope has many leaves.
fn long_text() -> impl Strategy<Value = String> {
    (text(24), 1usize..400).prop_map(|(t, n)| t.repeat(n))
}

#[derive(Debug, Clone)]
enum Op {
    /// Replace the bytes between two fractions of the length with text.
    Replace(f64, f64, String),
}

fn op() -> impl Strategy<Value = Op> {
    let inserted = prop_oneof![3 => text(6), 1 => long_text()];
    (0.0..=1.0f64, 0.0..=1.0f64, inserted).prop_map(|(a, b, t)| Op::Replace(a, b, t))
}

/// The char boundary at or before `byte` of `s`.
fn floor(s: &str, byte: usize) -> usize {
    let mut at = byte.min(s.len());
    while !s.is_char_boundary(at) {
        at -= 1;
    }
    at
}

/// `s[range]`, which the module's lints spell out.
fn sub(s: &str, range: std::ops::Range<usize>) -> &str {
    s.get(range).expect("char boundaries")
}

fn at_fraction(s: &str, f: f64) -> usize {
    floor(s, (s.len() as f64 * f) as usize)
}

/// Every query, at every char boundary of a sample, against the string.
fn assert_agrees(rope: &Rope, s: &str) -> Result<(), TestCaseError> {
    prop_assert_eq!(rope.verify_integrity(), Ok(()));
    prop_assert_eq!(rope.len(), s.len());
    prop_assert_eq!(&rope.to_string(), s);
    prop_assert!(*rope == *s);
    prop_assert_eq!(rope.line_count(), s.matches('\n').count() + 1);
    prop_assert_eq!(rope.summary().utf16, s.encode_utf16().count());
    let step = (s.len() / 97).max(1);
    for byte in (0..=s.len()).step_by(step).chain([s.len()]) {
        let b = floor(s, byte);
        prop_assert_eq!(rope.is_char_boundary(byte), s.is_char_boundary(byte));
        prop_assert_eq!(
            rope.byte_to_line(b),
            sub(s, 0..b).matches('\n').count(),
            "line of {}",
            b
        );
        prop_assert_eq!(rope.byte_to_utf16(b), sub(s, 0..b).encode_utf16().count());
        prop_assert_eq!(rope.char_at(b), sub(s, b..s.len()).chars().next());
        let snapped = TextOffset::snap(s, b);
        prop_assert_eq!(rope.floor_grapheme(b), snapped.get(), "floor at {}", b);
        prop_assert_eq!(rope.ceil_grapheme(b), TextOffset::snap_up(s, b).get());
        prop_assert_eq!(
            rope.prev_grapheme_boundary(snapped.get()),
            offset::prev_grapheme(s, snapped).get()
        );
        prop_assert_eq!(
            rope.next_grapheme_boundary(snapped.get()),
            offset::next_grapheme(s, snapped).get()
        );
        prop_assert_eq!(rope.line_start_at(b), offset::line_start(s, b).get());
        prop_assert_eq!(rope.line_end_at(b), offset::line_end(s, b).get());
        let range = offset::line_range_at(s, b);
        prop_assert_eq!(rope.line_range_at(b), range.start.get()..range.end.get());
        prop_assert_eq!(
            rope.prev_word_start(b),
            offset::prev_word_start(s, b).get(),
            "prev word at {}",
            b
        );
        prop_assert_eq!(
            rope.next_word_start(b),
            offset::next_word_start(s, b).get(),
            "next word at {}",
            b
        );
        prop_assert_eq!(
            rope.next_word_end(b),
            offset::next_word_end(s, b).get(),
            "word end at {}",
            b
        );
        let word = offset::word_range_at(s, b);
        prop_assert_eq!(rope.word_range_at(b), word.start.get()..word.end.get());
        let end = floor(s, b + step * 3);
        prop_assert_eq!(&*rope.slice(b..end), sub(s, b..end));
        prop_assert_eq!(
            rope.chunks_in_range(b..end).collect::<String>(),
            sub(s, b..end)
        );
        prop_assert!(rope.slice_eq(b, sub(s, b..end)));
    }
    Ok(())
}

/// Line starts and UTF-16 offsets, against the string's.
fn assert_lines_agree(rope: &Rope, s: &str) -> Result<(), TestCaseError> {
    let starts: Vec<usize> = std::iter::once(0)
        .chain(s.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    for (line, &start) in starts.iter().enumerate() {
        prop_assert_eq!(rope.line_to_byte(line), start);
        let end = starts.get(line + 1).copied().unwrap_or(s.len());
        prop_assert_eq!(rope.line_byte_range(line), start..end);
        let content = sub(s, start..end).trim_end_matches('\n');
        let content = content
            .strip_suffix('\r')
            .filter(|_| sub(s, start..end).ends_with("\r\n"))
            .unwrap_or(content);
        prop_assert_eq!(&*rope.slice(rope.line_content_range(line)), content);
    }
    prop_assert_eq!(rope.line_to_byte(starts.len()), s.len());
    let mut units = 0;
    for (i, ch) in s.char_indices() {
        prop_assert_eq!(rope.utf16_to_byte(units), i);
        if ch.len_utf16() == 2 {
            // Inside a surrogate pair: the char holding it.
            prop_assert_eq!(rope.utf16_to_byte(units + 1), i);
        }
        units += ch.len_utf16();
    }
    prop_assert_eq!(rope.utf16_to_byte(units), s.len());
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    // Catches any edit leaving the tree unbalanced, a leaf out of bounds,
    // a summary off, or a query disagreeing with the same text as one
    // string: in short and multi-leaf texts, across chunk edges, through
    // emoji, combining marks, CJK, and both line endings.
    #[test]
    fn edits_and_queries_agree_with_a_string(
        initial in prop_oneof![text(40), long_text()],
        ops in prop::collection::vec(op(), 1..12),
    ) {
        let mut rope = Rope::from(initial.as_str());
        let mut s = initial;
        assert_agrees(&rope, &s)?;
        for Op::Replace(a, b, inserted) in ops {
            let (a, b) = (at_fraction(&s, a), at_fraction(&s, b));
            let range = a.min(b)..a.max(b);
            rope.replace(range.clone(), &inserted);
            s.replace_range(range, &inserted);
            prop_assert_eq!(rope.verify_integrity(), Ok(()));
        }
        assert_agrees(&rope, &s)?;
        assert_lines_agree(&rope, &s)?;
    }

    // Catches a snapshot seeing edits made after it: a clone is a cheap
    // shared root, and edits copy only their own path.
    #[test]
    fn a_clone_keeps_its_text_through_later_edits(
        initial in long_text(),
        ops in prop::collection::vec(op(), 1..6),
    ) {
        let mut rope = Rope::from(initial.as_str());
        let snapshot = rope.clone();
        for Op::Replace(a, b, inserted) in ops {
            let len = rope.len();
            let (a, b) = (rope.floor_char_boundary((len as f64 * a) as usize), rope.floor_char_boundary((len as f64 * b) as usize));
            rope.replace(a.min(b)..a.max(b), &inserted);
        }
        prop_assert_eq!(snapshot.to_string(), initial);
        prop_assert_eq!(snapshot.verify_integrity(), Ok(()));
    }
}

// Typing a character at a time into a long text (the editor's common
// case) goes through the in-leaf path and the split path at leaf edges;
// catches either drifting from the string.
#[test]
fn typing_through_a_long_text_matches_a_string() {
    let base = "fn main() { println!(\"\u{65e5}\u{672c}\"); }\r\n".repeat(200);
    let mut rope = Rope::from(base.as_str());
    let mut s = base;
    let mut at = floor(&s, s.len() / 3);
    for (i, ch) in "let x = \u{1f44d}\u{1f3fd} + e\u{301};\n"
        .chars()
        .cycle()
        .take(3000)
        .enumerate()
    {
        let piece = ch.to_string();
        rope.insert(at, &piece);
        s.insert_str(at, &piece);
        at += piece.len();
        if i % 7 == 0 {
            let start = floor(&s, at.saturating_sub(5));
            rope.remove(start..at);
            s.replace_range(start..at, "");
            at = start;
        }
    }
    assert_eq!(rope.verify_integrity(), Ok(()));
    assert_eq!(rope.to_string(), s);
}

// Catches equality depending on how two ropes happen to be chunked.
#[test]
fn ropes_of_the_same_text_are_equal_however_they_are_chunked() {
    let text = "\u{65e5}\u{672c}\u{8a9e} and e\u{301} ".repeat(300);
    let built = Rope::from(text.as_str());
    let mut typed = Rope::new();
    for ch in text.chars() {
        typed.push_str(&ch.to_string());
    }
    let mut other = typed.clone();
    other.replace(6..9, "\u{672c}");
    assert_eq!(typed.verify_integrity(), Ok(()));
    assert_eq!((built == typed, typed == other), (true, false));
}
