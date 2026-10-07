use proptest::prelude::*;

use crate::{
    BlockKind, DiffDocument, Expansion, GapId, Mode, PatchError, Projection, Reveal, RowKind, Side,
    apply, diff_texts, parse_unified, write_unified,
};

/// Files, hunks, and lines of `doc` as text: a summary line per file, then
/// each hunk's header and lines with their diff tag. A line that ends its
/// store without a newline is marked `$`.
fn dump(doc: &DiffDocument) -> String {
    let mut out = String::new();
    let (h, b) = (doc.hunks(), doc.blocks());
    for s in doc.summaries() {
        let from = s
            .old_path
            .as_deref()
            .map(|p| format!("{p} -> "))
            .unwrap_or_default();
        let binary = if s.binary { " binary" } else { "" };
        out.push_str(&format!(
            "{from}{} {}{binary} +{} -{}\n",
            s.path,
            s.status.name(),
            s.additions,
            s.deletions
        ));
        let (old, new) = (doc.text(s.index, Side::Old), doc.text(s.index, Side::New));
        let line = |out: &mut String, tag: char, store: &crate::TextStore, i: u32| {
            let eol = if i + 1 == store.line_count() && store.no_newline_at_eof() {
                "$"
            } else {
                ""
            };
            out.push_str(&format!("  {tag}{}{eol}\n", store.line(i).unwrap()));
        };
        for hunk in doc.files().hunks[s.index as usize].clone() {
            let hi = hunk as usize;
            out.push_str(&format!(
                "  @@ -{},{} +{},{} @@{}\n",
                h.old_start[hi], h.old_len[hi], h.new_start[hi], h.new_len[hi], h.section[hi]
            ));
            for block in h.blocks[hi].clone() {
                let bi = block as usize;
                let (os, ns) = (b.old_store[bi], b.new_store[bi]);
                match b.kind[bi] {
                    BlockKind::Context => {
                        (0..b.old_len[bi]).for_each(|k| line(&mut out, ' ', old, os + k));
                    }
                    BlockKind::Change => {
                        (0..b.old_len[bi]).for_each(|k| line(&mut out, '-', old, os + k));
                        (0..b.new_len[bi]).for_each(|k| line(&mut out, '+', new, ns + k));
                    }
                }
            }
        }
    }
    out
}

#[test]
fn parser_reads_git_and_plain_patches() {
    let cases: &[(&str, &str, &str)] = &[
        (
            "modified with section and context",
            "diff --git a/src/lib.rs b/src/lib.rs\n\
             index 1111111..2222222 100644\n\
             --- a/src/lib.rs\n\
             +++ b/src/lib.rs\n\
             @@ -10,3 +10,3 @@ fn main() {\n \
             a\n\
             -b\n\
             +c\n \
             d\n",
            "src/lib.rs modified +1 -1\n  @@ -10,3 +10,3 @@fn main() {\n   a\n  -b\n  +c\n   d\n",
        ),
        (
            "rename without content change",
            "diff --git a/old name.txt b/new name.txt\n\
             similarity index 100%\n\
             rename from old name.txt\n\
             rename to new name.txt\n",
            "old name.txt -> new name.txt renamed +0 -0\n",
        ),
        (
            "rename with a hunk",
            "diff --git a/a.rs b/b.rs\n\
             similarity index 90%\n\
             rename from a.rs\n\
             rename to b.rs\n\
             --- a/a.rs\n\
             +++ b/b.rs\n\
             @@ -1 +1 @@\n\
             -x\n\
             +y\n",
            "a.rs -> b.rs renamed +1 -1\n  @@ -1,1 +1,1 @@\n  -x\n  +y\n",
        ),
        (
            "binary markers, both kinds",
            "diff --git a/logo.png b/logo.png\n\
             index 1111111..2222222 100644\n\
             Binary files a/logo.png and b/logo.png differ\n\
             diff --git a/icon.ico b/icon.ico\n\
             new file mode 100644\n\
             GIT binary patch\n\
             literal 4\n\
             LcmZQzWMT#Y01f~L\n\
             \n\
             literal 0\n\
             HcmV?d00001\n",
            "logo.png modified binary +0 -0\nicon.ico added binary +0 -0\n",
        ),
        (
            "no newline at end of file on both sides",
            "--- a/a.txt\n\
             +++ b/a.txt\n\
             @@ -1,2 +1,2 @@\n \
             keep\n\
             -old\n\
             \\ No newline at end of file\n\
             +new\n\
             \\ Kein Zeilenumbruch am Dateiende\n",
            "a.txt modified +1 -1\n  @@ -1,2 +1,2 @@\n   keep\n  -old$\n  +new$\n",
        ),
        (
            "plain diff -u, two files, new and deleted",
            "--- /dev/null\t2024-01-01 00:00:00\n\
             +++ b/new.txt\t2024-01-01 00:00:00\n\
             @@ -0,0 +1,2 @@\n\
             +one\n\
             +two\n\
             --- a/gone.txt\n\
             +++ /dev/null\n\
             @@ -1 +0,0 @@\n\
             -bye\n",
            "new.txt added +2 -0\n  @@ -0,0 +1,2 @@\n  +one\n  +two\n\
             gone.txt deleted +0 -1\n  @@ -1,1 +0,0 @@\n  -bye\n",
        ),
        (
            "a removed line that looks like a header stays in the hunk",
            "--- a/x\n\
             +++ b/x\n\
             @@ -1,2 +1 @@\n\
             --- a/y\n\
             -+++ b/y\n\
             +z\n",
            "x modified +1 -2\n  @@ -1,2 +1,1 @@\n  --- a/y\n  -+++ b/y\n  +z\n",
        ),
    ];
    for (name, patch, expected) in cases {
        let doc = parse_unified(patch).unwrap_or_else(|e| panic!("{name}: {e}"));
        doc.verify_integrity().unwrap();
        assert_eq!(dump(&doc), *expected, "{name}");
    }
}

#[test]
fn parser_rejects_malformed_hunks() {
    let cases: &[(&str, PatchError)] = &[
        ("", PatchError::Empty),
        (
            "--- a/x\n+++ b/x\n@@ -1 +1 @\n",
            PatchError::InvalidHunkHeader { line: 3 },
        ),
        (
            "--- a/x\n+++ b/x\n@@ -1,2 +1,2 @@\n a\n",
            PatchError::TruncatedHunk { line: 5 },
        ),
        (
            "--- a/x\n+++ b/x\n@@ -1 +1 @@\n*a\n",
            PatchError::MalformedHunkLine { line: 4 },
        ),
        (
            "--- a/x\n+++ b/x\n@@ -5 +5 @@\n-a\n+b\n@@ -1 +1 @@\n-c\n+d\n",
            PatchError::HunkOutOfOrder { line: 6 },
        ),
        // A no-newline marker after an empty line cannot happen.
        (
            "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-\n\\ No newline\n+a\n",
            PatchError::MalformedHunkLine { line: 5 },
        ),
    ];
    for (patch, expected) in cases {
        assert_eq!(
            parse_unified(patch).err().as_ref(),
            Some(expected),
            "{patch:?}"
        );
    }
}

/// Text from a small alphabet of lines, so diffs have common lines.
fn text() -> impl Strategy<Value = String> {
    let line = prop::sample::select(vec!["a", "b", "c", "fn x() {", "}", "", "\tz\r"]);
    (prop::collection::vec(line, 0..30), any::<bool>()).prop_map(|(lines, eol)| {
        let mut text = lines.join("\n");
        if eol && !text.is_empty() {
            text.push('\n');
        }
        text
    })
}

proptest! {
    #[test]
    fn a_written_patch_parses_back_and_applies_to_the_old_text(
        old in text(),
        new in text(),
        context in 0u32..4,
    ) {
        let doc = diff_texts(Some("f"), Some("f"), Some(&old), Some(&new), context);
        doc.verify_integrity().unwrap();
        let patch = write_unified(&doc);
        let parsed = parse_unified(&patch).unwrap();
        parsed.verify_integrity().unwrap();
        prop_assert_eq!(apply(&parsed, 0, &old).unwrap(), new.clone());
        prop_assert_eq!(apply(&doc, 0, &old).unwrap(), new);
        prop_assert_eq!(write_unified(&parsed), patch);
    }

    #[test]
    fn projections_cover_every_line_once_when_fully_expanded(
        old in text(),
        new in text(),
        split in any::<bool>(),
    ) {
        let doc = diff_texts(Some("f"), Some("f"), Some(&old), Some(&new), 1);
        let mut expansion = Expansion::new(&doc);
        for hunk in 0..doc.hunk_count() {
            expansion.reveal(&doc, GapId { file: 0, hunk: Some(hunk) }, Reveal::All, 0);
        }
        expansion.reveal(&doc, GapId { file: 0, hunk: None }, Reveal::All, 0);
        let mode = if split { Mode::Split } else { Mode::Unified };
        let p = Projection::new(&doc, mode, &expansion);
        p.verify_integrity(&doc).unwrap();
        prop_assert!(!p.kind.contains(&RowKind::Gap));
        for side in [Side::Old, Side::New] {
            let lines = doc.text(0, side).line_count();
            let shown = (0..p.len()).filter(|&r| p.line(r, side).is_some()).count();
            prop_assert_eq!(shown as u32, lines);
        }
    }
}

/// Projection rows as text: kind tag, old and new line numbers, text.
fn rows(doc: &DiffDocument, p: &Projection) -> String {
    let mut out = String::new();
    for r in 0..p.len() {
        let side_text = |side| {
            p.line(r, side)
                .map(|i| (i + 1, doc.text(p.file[r as usize], side).line(i).unwrap()))
        };
        let (old, new) = (side_text(Side::Old), side_text(Side::New));
        let num = |l: Option<(u32, &str)>| l.map_or("-".to_owned(), |(n, _)| n.to_string());
        let line = match p.kind[r as usize] {
            RowKind::FileHeader => format!("file {}", doc.path(p.file[r as usize])),
            RowKind::HunkHeader => "@@".to_owned(),
            RowKind::Gap => format!("... {} hidden", p.gap(r).unwrap().hidden),
            kind => {
                let tag = match kind {
                    RowKind::Removed => '-',
                    RowKind::Added => '+',
                    RowKind::Modified => '~',
                    _ => ' ',
                };
                let text = match (old, new) {
                    (Some((_, o)), Some((_, n))) if o != n => format!("{o} | {n}"),
                    (Some((_, t)), _) | (_, Some((_, t))) => t.to_owned(),
                    _ => String::new(),
                };
                format!("{tag} {} {} {text}", num(old), num(new))
            }
        };
        out.push_str(&line);
        out.push('\n');
    }
    out
}

fn numbered(lines: std::ops::Range<u32>) -> String {
    lines.map(|i| format!("l{i}\n")).collect()
}

#[test]
fn side_by_side_pairs_changed_lines_and_pads_the_shorter_side() {
    let old = "a\nb\nc\nd\n";
    let new = "a\nB\nC\nC2\nd\n";
    let doc = diff_texts(Some("f"), Some("f"), Some(old), Some(new), 1);
    let p = Projection::new(&doc, Mode::Split, &Expansion::new(&doc));
    assert_eq!(
        rows(&doc, &p),
        "file f\n  1 1 a\n~ 2 2 b | B\n~ 3 3 c | C\n+ - 4 C2\n  4 5 d\n"
    );
    let p = Projection::new(&doc, Mode::Unified, &Expansion::new(&doc));
    assert_eq!(
        rows(&doc, &p),
        "file f\n  1 1 a\n- 2 - b\n- 3 - c\n+ - 2 B\n+ - 3 C\n+ - 4 C2\n  4 5 d\n"
    );
}

#[test]
fn expanding_a_gap_reveals_lines_from_the_chosen_end() {
    // Changes at lines 2 and 20 of 30, one line of context.
    let old = numbered(1..31);
    let new = old.replace("l2\n", "x2\n").replace("l20\n", "x20\n");
    let doc = diff_texts(Some("f"), Some("f"), Some(&old), Some(&new), 1);
    let mut expansion = Expansion::new(&doc);
    let shown = |e: &Expansion| {
        let p = Projection::new(&doc, Mode::Unified, e);
        p.verify_integrity(&doc).unwrap();
        rows(&doc, &p)
            .lines()
            .filter(|l| l.starts_with("...") || l.starts_with("  "))
            .collect::<Vec<_>>()
            .join(",")
    };
    assert_eq!(
        shown(&expansion),
        "  1 1 l1,  3 3 l3,... 15 hidden,  19 19 l19,  21 21 l21,... 9 hidden"
    );
    let between = GapId {
        file: 0,
        hunk: Some(1),
    };
    assert!(expansion.reveal(&doc, between, Reveal::Up, 2));
    assert!(expansion.reveal(&doc, between, Reveal::Down, 1));
    assert_eq!(
        shown(&expansion),
        "  1 1 l1,  3 3 l3,  4 4 l4,... 12 hidden,  17 17 l17,  18 18 l18,  19 19 l19,  21 21 l21,... 9 hidden"
    );
    // Revealing all but a few hidden lines shows the rest too.
    assert!(expansion.reveal(&doc, between, Reveal::Down, 10));
    assert!(!shown(&expansion).contains("... 12"));
    assert!(!expansion.reveal(&doc, between, Reveal::Down, 1));
}

#[test]
fn file_list_reports_paths_status_and_stats() {
    let mut doc = diff_texts(
        Some("a.rs"),
        Some("b.rs"),
        Some("x\ny\n"),
        Some("x\nz\nw\n"),
        3,
    );
    doc.append(diff_texts(None, Some("new.md"), None, Some("hi\n"), 3));
    doc.verify_integrity().unwrap();
    let list: Vec<String> = doc
        .summaries()
        .map(|s| {
            format!(
                "{:?} {} {} +{} -{}",
                s.old_path,
                s.path,
                s.status.name(),
                s.additions,
                s.deletions
            )
        })
        .collect();
    assert_eq!(
        list,
        [
            r#"Some("a.rs") b.rs renamed +2 -1"#,
            "None new.md added +1 -0"
        ]
    );
    assert_eq!(doc.totals(), (3, 1));
}

#[test]
fn a_hundred_thousand_line_diff_projects_every_row() {
    let old = numbered(0..100_000);
    let new = old.replace("l500\n", "x500\n");
    let doc = diff_texts(Some("big"), Some("big"), Some(&old), Some(&new), 3);
    let mut expansion = Expansion::new(&doc);
    expansion.reveal(
        &doc,
        GapId {
            file: 0,
            hunk: Some(0),
        },
        Reveal::All,
        0,
    );
    expansion.reveal(
        &doc,
        GapId {
            file: 0,
            hunk: None,
        },
        Reveal::All,
        0,
    );
    let p = Projection::new(&doc, Mode::Unified, &expansion);
    // A header, 99,999 unchanged lines, and the changed pair.
    assert_eq!(p.len(), 100_002);
    assert_eq!(p.row_of(0, Side::New, 99_999), Some(100_001));
}

/// Diff times of large inputs. Run with `--ignored --nocapture` in release
/// mode for meaningful numbers.
#[test]
#[ignore = "measurement, prints a report"]
fn report_diff_times() {
    let old = numbered(0..100_000);
    let sparse: String = old
        .lines()
        .enumerate()
        .map(|(i, l)| {
            if i % 100 == 7 {
                format!("edit {i}\n")
            } else {
                format!("{l}\n")
            }
        })
        .collect();
    let rewritten: String = (0..20_000).map(|i| format!("other {i}\n")).collect();
    let small_old = numbered(0..20_000);
    let shuffled: String = (0..20_000)
        .map(|i| format!("l{}\n", (i * 7_919) % 20_000))
        .collect();
    for (name, a, b) in [
        ("100k lines, 1% edited", &old, &sparse),
        ("20k lines, all rewritten", &small_old, &rewritten),
        ("20k lines, reordered", &small_old, &shuffled),
    ] {
        let started = std::time::Instant::now();
        let doc = diff_texts(Some("f"), Some("f"), Some(a), Some(b), 3);
        let p = Projection::new(&doc, Mode::Split, &Expansion::new(&doc));
        eprintln!(
            "{name}: {:?}, {} hunks, {} rows",
            started.elapsed(),
            doc.hunk_count(),
            p.len()
        );
    }
}
