//! Observer scopes: which signals a computation reads, and the version of
//! each it saw.
//!
//! Scopes nest on one thread-local stack. A read lands in the innermost
//! open scope only. How a scope ends decides whether the enclosing scope
//! sees its reads:
//!
//! - [`Nesting::Isolate`]: it does not. A memo's compute is isolated: the
//!   reader of a memo depends on the memo, not on what the memo read, so
//!   the memo's equality check can stop a change.
//! - [`Nesting::Merge`]: it does, as if the reads were made in it. A cache
//!   boundary nested in another merges: the outer recording contains the
//!   inner one's output, so whatever invalidates the inner one invalidates
//!   the outer one too. The inner boundary still keeps only its own reads,
//!   so a change the outer boundary alone read leaves the inner one
//!   replaying.
//!
//! The stack keeps its capacity, so tracking allocates nothing once warm.

use std::cell::RefCell;

use super::SignalId;

/// A tracked read: the signal and the version the reader saw.
pub type SignalRead = (SignalId, u64);

/// How a tracking scope's reads reach the scope enclosing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nesting {
    /// The enclosing scope does not see them.
    Isolate,
    /// The enclosing scope sees them, as if made in it.
    Merge,
}

/// Reads of every open scope, outermost first.
struct Observer {
    reads: Vec<SignalRead>,
    /// Where each open scope's reads start in `reads`, innermost last.
    scopes: Vec<usize>,
}

thread_local! {
    static OBSERVER: RefCell<Observer> = const {
        RefCell::new(Observer {
            reads: Vec::new(),
            scopes: Vec::new(),
        })
    };
}

/// A read of a signal the innermost scope read among its last this many
/// reads is dropped. Repeated reads are almost always close together; one
/// that slips through only costs the snapshot an entry.
const DEDUP_WINDOW: usize = 16;

impl Observer {
    /// Whether the scope whose reads start at `start` read `id` among its
    /// last [`DEDUP_WINDOW`] reads before `end`.
    fn recent(&self, start: usize, end: usize, id: SignalId) -> bool {
        let from = end.saturating_sub(DEDUP_WINDOW).max(start);
        self.reads[from..end].iter().any(|&(read, _)| read == id)
    }

    /// Record `read` in the innermost open scope. A repeated read keeps
    /// the version first seen, so a scope that saw a signal change under
    /// it reads as changed.
    fn push(&mut self, read: SignalRead) {
        let Some(&start) = self.scopes.last() else {
            return;
        };
        if !self.recent(start, self.reads.len(), read.0) {
            self.reads.push(read);
        }
    }

    /// Fold the reads from `from` on into the innermost open scope,
    /// dropping repeats as [`Self::push`] does. In place.
    fn fold_from(&mut self, from: usize) {
        let Some(&start) = self.scopes.last() else {
            self.reads.truncate(from);
            return;
        };
        let mut end = from;
        for at in from..self.reads.len() {
            let read = self.reads[at];
            if !self.recent(start, end, read.0) {
                self.reads[end] = read;
                end += 1;
            }
        }
        self.reads.truncate(end);
    }
}

pub(super) fn track_read(id: SignalId, version: u64) {
    OBSERVER.with(|observer| observer.borrow_mut().push((id, version)));
}

/// Whether a tracking scope is open: a read now would be recorded.
pub fn is_tracking() -> bool {
    OBSERVER.with(|observer| !observer.borrow().scopes.is_empty())
}

/// Record `reads` in the innermost open scope, as if made now. A cache
/// boundary replaying a recording reports the reads that recording
/// depends on, so a boundary enclosing it depends on them too.
pub fn track_reads(reads: &[SignalRead]) {
    OBSERVER.with(|observer| {
        let mut observer = observer.borrow_mut();
        for &read in reads {
            observer.push(read);
        }
    });
}

/// Record a read of `id` at `version`, for a source that publishes its
/// state into a signal and remembers the version it published, so it can
/// report a read without the store. A version older than the signal's
/// current one only makes the reader look changed; it never hides a
/// change.
pub fn track_read_at(id: SignalId, version: u64) {
    track_read(id, version);
}

/// Record a read of a source that has no signal yet: the reader stays out
/// of date until it reads the source again, by when the source may have
/// published one.
pub fn track_unpublished() {
    track_read(SignalId::UNPUBLISHED, 0);
}

/// An open scope. Closing it hands its reads on; dropping it unclosed (a
/// panic in the tracked closure) discards them, so the thread never
/// records into a dead scope.
struct Scope {
    depth: usize,
    start: usize,
    closed: bool,
}

impl Scope {
    fn open() -> Self {
        OBSERVER.with(|observer| {
            let mut observer = observer.borrow_mut();
            let start = observer.reads.len();
            let depth = observer.scopes.len();
            observer.scopes.push(start);
            Self {
                depth,
                start,
                closed: false,
            }
        })
    }

    /// Hand the scope's reads to `take`, then keep them for the enclosing
    /// scope or drop them, as `nesting` says.
    fn close(mut self, nesting: Nesting, take: impl FnOnce(&[SignalRead])) {
        self.closed = true;
        OBSERVER.with(|observer| {
            let mut observer = observer.borrow_mut();
            debug_assert_eq!(
                observer.scopes.len(),
                self.depth + 1,
                "tracking scopes closed out of order"
            );
            observer.scopes.truncate(self.depth);
            take(&observer.reads[self.start..]);
            match nesting {
                Nesting::Isolate => observer.reads.truncate(self.start),
                Nesting::Merge => observer.fold_from(self.start),
            }
        });
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        if self.closed {
            return;
        }
        let _ = OBSERVER.try_with(|observer| {
            let mut observer = observer.borrow_mut();
            observer.scopes.truncate(self.depth);
            observer.reads.truncate(self.start);
        });
    }
}

/// Run `f` with dependency tracking. Returns the result plus the signals
/// `f` read, in the order first read. The enclosing scope, if any, does
/// not see them ([`Nesting::Isolate`]).
pub fn with_tracking<R>(f: impl FnOnce() -> R) -> (R, Vec<SignalId>) {
    let scope = Scope::open();
    let result = f();
    let mut deps = Vec::new();
    scope.close(Nesting::Isolate, |reads| {
        deps.extend(reads.iter().map(|&(id, _)| id));
    });
    (result, deps)
}

/// Run `f` in a tracking scope and append the reads it makes, with the
/// versions seen, to `reads`. `nesting` says whether the enclosing scope
/// sees them too. Allocates nothing once `reads` and the scope stack have
/// grown to fit.
pub fn track_into<R>(reads: &mut Vec<SignalRead>, nesting: Nesting, f: impl FnOnce() -> R) -> R {
    let scope = Scope::open();
    let result = f();
    scope.close(nesting, |made| reads.extend_from_slice(made));
    result
}
