//! Highlighting on a background thread.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::thread::JoinHandle;

use crate::{HighlightSpan, LanguageId, highlight};

struct Job {
    slot: u64,
    generation: u64,
    language: LanguageId,
    source: Arc<str>,
}

/// A finished highlight of the source requested for `slot` at
/// `generation`.
#[derive(Debug, Clone)]
pub struct Highlighted {
    pub slot: u64,
    pub generation: u64,
    pub source: Arc<str>,
    pub spans: Vec<HighlightSpan>,
}

/// The worker thread is gone (it could not be spawned), so no more results
/// will arrive. Requests sent since are lost; make a new worker and send
/// them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerGone;

type HighlightFn = fn(LanguageId, &str) -> Vec<HighlightSpan>;

/// Highlights requests on one background thread. A slot is one code block;
/// when several requests for a slot are queued, only the newest generation
/// is highlighted, so a block that streams faster than it highlights does
/// not build a backlog. Results arrive in [`HighlightWorker::try_recv`];
/// callers still compare generations, since a result can land after a
/// newer request was sent.
///
/// A highlight that panics yields a result with no spans (the block stays
/// plain) and the thread keeps serving requests.
pub struct HighlightWorker {
    jobs: Option<Sender<Job>>,
    done: Receiver<Highlighted>,
    thread: Option<JoinHandle<()>>,
    /// Set on drop so the thread stops between highlights instead of
    /// finishing its batch while the dropping thread waits.
    cancel: Arc<AtomicBool>,
}

impl HighlightWorker {
    pub fn new() -> Self {
        Self::with_highlighter(highlight)
    }

    fn with_highlighter(highlight: HighlightFn) -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("quark-syntax".to_owned())
            .spawn(move || run(job_rx, done_tx, &stop, highlight))
            .ok();
        Self {
            jobs: Some(jobs),
            done,
            thread,
            cancel,
        }
    }

    /// A worker whose thread is already gone, as when spawning fails. For
    /// testing callers' recovery.
    #[doc(hidden)]
    pub fn gone() -> Self {
        let (jobs, _) = channel();
        let (_, done) = channel();
        Self {
            jobs: Some(jobs),
            done,
            thread: None,
            cancel: Arc::new(AtomicBool::new(true)),
        }
    }

    pub fn request(&self, slot: u64, generation: u64, language: LanguageId, source: Arc<str>) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(Job {
                slot,
                generation,
                language,
                source,
            });
        }
    }

    /// A finished highlight, without blocking: `Ok(None)` when none is ready
    /// yet.
    pub fn try_recv(&self) -> Result<Option<Highlighted>, WorkerGone> {
        match self.done.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Disconnected) => Err(WorkerGone),
        }
    }

    /// The next finished highlight, blocking until one arrives.
    pub fn recv(&self) -> Result<Highlighted, WorkerGone> {
        self.done.recv().map_err(|_| WorkerGone)
    }
}

impl Default for HighlightWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HighlightWorker {
    fn drop(&mut self) {
        // The flag stops a batch in progress; closing the job channel ends
        // the thread's loop. The wait is at most one highlight.
        self.cancel.store(true, Ordering::Relaxed);
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(
    jobs: Receiver<Job>,
    done: Sender<Highlighted>,
    cancel: &AtomicBool,
    highlight: HighlightFn,
) {
    while let Ok(first) = jobs.recv() {
        // Coalesce everything queued: keep the newest job per slot, in the
        // order the slots were first requested.
        let mut order = Vec::new();
        let mut newest: HashMap<u64, Job> = HashMap::new();
        for job in std::iter::once(first).chain(jobs.try_iter()) {
            match newest.get(&job.slot) {
                Some(queued) if queued.generation > job.generation => {}
                Some(_) => {
                    newest.insert(job.slot, job);
                }
                None => {
                    order.push(job.slot);
                    newest.insert(job.slot, job);
                }
            }
        }
        for slot in order {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let Some(job) = newest.remove(&slot) else {
                continue;
            };
            // A grammar bug must not take the thread down: every later block
            // would silently stay plain.
            let spans = catch_unwind(AssertUnwindSafe(|| highlight(job.language, &job.source)))
                .unwrap_or_default();
            let result = Highlighted {
                slot,
                generation: job.generation,
                source: job.source,
                spans,
            };
            if done.send(result).is_err() {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panics_on_boom(_: LanguageId, source: &str) -> Vec<HighlightSpan> {
        assert_ne!(source, "boom", "highlighter bug");
        vec![HighlightSpan {
            offset: 0,
            length: source.len() as u32,
            kind: crate::HighlightKind::Keyword,
        }]
    }

    // Catches a panicking highlight killing the thread, after which every
    // block would stay plain.
    #[test]
    fn worker_survives_a_panicking_highlight() {
        let worker = HighlightWorker::with_highlighter(panics_on_boom);
        worker.request(1, 1, LanguageId::Rust, Arc::from("boom"));
        let failed = worker.recv().map(|r| (r.slot, r.spans.len()));
        worker.request(2, 2, LanguageId::Rust, Arc::from("fn"));
        let next = worker.recv().map(|r| (r.slot, r.spans.len()));

        assert_eq!((failed, next), (Ok((1, 0)), Ok((2, 1))));
    }
}
