//! Feeds arbitrary bytes through quark-terminal's libghostty-vt wrapper,
//! interleaved with resizes, scrolling, snapshots, resets, and selection
//! queries. Every snapshot must pass the grid's integrity checks, and the
//! run must end the same whether the bytes arrive in the given chunks or
//! merged into one write per stretch between operations: same grid, same
//! replies to the program, same bells, and the same final title and
//! clipboard write (a write reports only the last of each it carried).
//!
//! No PTY or process is involved, and nothing the terminal asks for (the
//! clipboard, titles) reaches the OS: effects are only recorded.
//!
//! Input: three header bytes (columns, rows, scrollback in 64-line units),
//! then bytes to feed. 0xFF starts an operation:
//!
//! - `FF 00`: chunk boundary
//! - `FF 01 c r`: resize to `1 + c % 160` by `1 + r % 60`
//! - `FF 02 d`: scroll `d` rows (signed)
//! - `FF 03`, `FF 04`: scroll to the top, to the bottom
//! - `FF 05 r`: scroll to scrollback row `r * 4`
//! - `FF 06`: snapshot and check the grid
//! - `FF 07`: full reset (RIS)
//! - `FF 08 a b c d`: select from cell `(a, b)` to `(c, d)` and read the
//!   selection, word, line, and hyperlink there
//! - `FF FF`: a literal 0xFF byte
//!
//! Any other operation byte is a chunk boundary.
//!
//! Under ASan, RSS grows by tens of KiB per run once a terminal has used
//! the alternate screen (a plain build stays flat, so this is ASan's
//! allocator holding libghostty-vt's buffers), which reaches libFuzzer's
//! 2 GB limit within a minute: give campaigns `-rss_limit_mb=8192` or
//! build with `-s none`. ASan and coverage see only the Rust side; the Zig
//! library is linked uninstrumented (see crates/quark-terminal/build.rs).

#![no_main]

use libfuzzer_sys::fuzz_target;
use quark_terminal::Grid;
use quark_terminal::vt::{Scroll, Terminal};

/// Longer inputs are skipped: they only slow the search down.
const MAX_INPUT: usize = 64 * 1024;
const MAX_OPS: usize = 512;

#[derive(Debug)]
enum Op {
    Feed(Vec<u8>),
    Boundary,
    Resize(u16, u16),
    Scroll(Scroll),
    Snapshot,
    Reset,
    Select((u16, u16), (u16, u16)),
}

fn size(c: u8, r: u8) -> (u16, u16) {
    (1 + u16::from(c) % 160, 1 + u16::from(r) % 60)
}

fn parse(body: &[u8]) -> Vec<Op> {
    let mut ops = Vec::new();
    let mut feed = Vec::new();
    let mut i = 0;
    while i < body.len() && ops.len() < MAX_OPS {
        let b = body[i];
        i += 1;
        if b != 0xff {
            feed.push(b);
            continue;
        }
        let Some(&code) = body.get(i) else { break };
        i += 1;
        if code == 0xff {
            feed.push(0xff);
            continue;
        }
        let args = |n: usize| body.get(i..i + n);
        let op = match code {
            0x01 => args(2).map(|a| {
                let (c, r) = size(a[0], a[1]);
                Op::Resize(c, r)
            }),
            0x02 => args(1).map(|a| Op::Scroll(Scroll::Delta(isize::from(a[0] as i8)))),
            0x03 => Some(Op::Scroll(Scroll::Top)),
            0x04 => Some(Op::Scroll(Scroll::Bottom)),
            0x05 => args(1).map(|a| Op::Scroll(Scroll::Row(usize::from(a[0]) * 4))),
            0x06 => Some(Op::Snapshot),
            0x07 => Some(Op::Reset),
            0x08 => args(4).map(|a| {
                Op::Select(
                    (u16::from(a[0]), u16::from(a[1])),
                    (u16::from(a[2]), u16::from(a[3])),
                )
            }),
            _ => Some(Op::Boundary),
        };
        let Some(op) = op else { break };
        i += match &op {
            Op::Resize(..) => 2,
            Op::Scroll(Scroll::Delta(_) | Scroll::Row(_)) => 1,
            Op::Select(..) => 4,
            _ => 0,
        };
        if !feed.is_empty() {
            ops.push(Op::Feed(std::mem::take(&mut feed)));
        }
        ops.push(op);
    }
    if !feed.is_empty() {
        ops.push(Op::Feed(feed));
    }
    ops
}

/// What a run leaves behind, compared across chunkings.
#[derive(Debug, Default, PartialEq)]
struct Outcome {
    /// Replies the terminal queued for the program, in order.
    pty: Vec<u8>,
    bells: u32,
    title: String,
    /// The last clipboard write.
    clipboard: Option<String>,
    /// Results of the selection queries, in order.
    queries: Vec<String>,
    /// The final grid: rows, cursor, and colors.
    screen: String,
}

fn snapshot(term: &mut Terminal, grid: &mut Grid) {
    term.snapshot(grid);
    let (cols, rows) = (term.cols(), term.rows());
    assert_eq!(grid.cols, cols);
    assert_eq!(grid.rows.len(), usize::from(rows));
    for (y, row) in grid.rows.iter().enumerate() {
        assert_eq!(row.verify_integrity(), Ok(()), "row {y}: {row:?}");
        let end = row.runs.last().map_or(0, |r| r.col + r.cols);
        assert!(end <= cols, "row {y} ends at column {end} of {cols}");
        if let Some((from, to)) = row.selection {
            assert!(from <= to && to < cols, "row {y} selection {from}..={to}");
        }
    }
    // The view keys per-row caches by id.
    let mut ids: Vec<u64> = grid.rows.iter().map(|r| r.id).collect();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), grid.rows.len(), "row ids repeat");
    if let Some((x, y)) = grid.cursor.at {
        assert!(
            x < cols && y < rows,
            "cursor at ({x}, {y}) in {cols}x{rows}"
        );
    }
    let bar = term.scrollbar();
    assert!(
        bar.offset + bar.len <= bar.total,
        "scrollbar {bar:?} past its content"
    );
}

/// The grid as text precise enough to tell two grids apart by what they
/// draw. Row ids are left out: an id names a row's storage, unique in the
/// process, so two grids never share one.
fn dump(grid: &Grid) -> String {
    let mut out = format!(
        "{}x{} {:?} {:?}\n",
        grid.cols,
        grid.rows.len(),
        grid.cursor,
        grid.colors
    );
    for row in &grid.rows {
        out += &format!(
            "{:?} {:?} {:?} {} {:016x}\n",
            row.text, row.runs, row.selection, row.wrapped, row.hash
        );
    }
    out
}

fn take_effects(term: &mut Terminal, out: &mut Outcome) {
    let effects = term.take_effects();
    out.pty.extend_from_slice(&effects.pty);
    out.bells += effects.bells;
    if effects.clipboard.is_some() {
        out.clipboard = effects.clipboard;
    }
}

fn flush(term: &mut Terminal, out: &mut Outcome, pending: &mut Vec<u8>) {
    if !pending.is_empty() {
        term.write(pending);
        pending.clear();
        take_effects(term, out);
    }
}

fn run(header: [u8; 3], ops: &[Op], merge: bool) -> Outcome {
    let (cols, rows) = size(header[0], header[1]);
    let mut term = Terminal::new(cols, rows);
    term.set_scrollback_lines(usize::from(header[2]) * 64);
    term.set_clipboard_write(true);
    let mut grid = Grid::default();
    let mut out = Outcome::default();
    let mut pending = Vec::new();
    for op in ops {
        match op {
            Op::Feed(bytes) if merge => pending.extend_from_slice(bytes),
            Op::Feed(bytes) => {
                term.write(bytes);
                take_effects(&mut term, &mut out);
            }
            Op::Boundary => {}
            op => {
                flush(&mut term, &mut out, &mut pending);
                match op {
                    Op::Resize(c, r) => term.resize(*c, *r, 8, 16),
                    Op::Scroll(to) => term.scroll(*to),
                    Op::Snapshot => snapshot(&mut term, &mut grid),
                    Op::Reset => {
                        term.write(b"\x1bc");
                        take_effects(&mut term, &mut out);
                    }
                    Op::Select(anchor, focus) => {
                        term.select(*anchor, *focus);
                        out.queries.push(format!(
                            "{:?} {:?} {:?} {:?}",
                            term.selection_text(),
                            term.word_at(*focus),
                            term.line_at(*focus),
                            term.hyperlink_at(focus.0, focus.1),
                        ));
                    }
                    Op::Feed(_) | Op::Boundary => unreachable!(),
                }
            }
        }
    }
    flush(&mut term, &mut out, &mut pending);
    snapshot(&mut term, &mut grid);

    // A grid kept up to date snapshot by snapshot matches one read fresh.
    let mut fresh = Grid::default();
    term.snapshot(&mut fresh);
    let screen = dump(&grid);
    assert_eq!(
        screen,
        dump(&fresh),
        "incremental and fresh snapshots differ"
    );
    out.screen = screen;
    out.title = term.title();
    out
}

fuzz_target!(|data: &[u8]| {
    if data.len() > MAX_INPUT {
        return;
    }
    let Some((&header, body)) = data.split_first_chunk::<3>() else {
        return;
    };
    let ops = parse(body);
    let chunked = run(header, &ops, false);
    let merged = run(header, &ops, true);
    assert_eq!(chunked, merged, "chunking changed the outcome");
});
