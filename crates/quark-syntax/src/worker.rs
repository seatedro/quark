//! Highlighting on a background thread.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::JoinHandle;

use crate::store::Outcome;
use crate::{GrammarStore, HighlightSpan, LanguageId};

#[derive(Clone)]
struct Job {
    slot: u64,
    generation: u64,
    language: LanguageId,
    source: Arc<str>,
}

enum Message {
    Job(Job),
    /// A pending grammar resolved: rerun the jobs waiting for one.
    Retry,
    Stop,
}

/// A finished highlight of the source requested for `slot` at
/// `generation`.
///
/// A request can get several results while grammars arrive: each has the
/// same generation and a higher `revision`, and the last has `pending`
/// unset. Keep a result when its `(generation, revision)` is newer than the
/// one held for the slot, and drop it otherwise.
#[derive(Debug, Clone)]
pub struct Highlighted {
    pub slot: u64,
    pub generation: u64,
    /// Counts the results for this slot and generation, from 0.
    pub revision: u32,
    pub source: Arc<str>,
    pub spans: Vec<HighlightSpan>,
    /// Grammars in `unresolved` are still arriving, so another result for
    /// this slot and generation follows once one of them resolves.
    pub pending: bool,
    /// Languages whose grammars are still arriving. When the requested
    /// language is one, `spans` is empty; otherwise these are languages
    /// embedded in the source, whose regions keep the host's colors until
    /// they arrive.
    pub unresolved: Vec<LanguageId>,
}

/// The worker thread is gone (it could not be spawned), so no more results
/// will arrive. Requests sent since are lost; make a new worker and send
/// them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerGone;

/// Highlights a source, stopping early once the callback returns true.
type Highlight = dyn Fn(&LanguageId, &str, &dyn Fn() -> bool) -> Outcome + Send + Sync;
type HighlightFn = Arc<Highlight>;

/// The newest generation requested per slot, written by
/// [`HighlightWorker::request`] before the job is sent, so a highlight in
/// progress can see that it was superseded without draining the channel.
/// A slot's entry goes once its newest generation is done.
type Latest = Arc<Mutex<HashMap<u64, u64>>>;

/// Highlights requests on one background thread with a [`GrammarStore`]'s
/// grammars. A slot is one code block; when several requests for a slot
/// are queued, only the newest generation is highlighted, so a block that
/// streams faster than it highlights does not build a backlog. Results
/// arrive in [`HighlightWorker::try_recv`]; callers still compare
/// generations, since a result can land after a newer request was sent.
///
/// A request whose grammars are still downloading gets the best result
/// available at once (plain when its own grammar is missing, host colors
/// when only embedded ones are) marked [`Highlighted::pending`], then a
/// result with a higher revision each time an arriving grammar changes it,
/// unless a newer request for the slot superseded it.
///
/// A highlight in progress stops at its next checkpoint (tree-sitter polls
/// one every hundred or so parse or query steps) once a newer request for
/// its slot arrives, and yields no result.
///
/// A highlight that panics yields a result with no spans (the block stays
/// plain) and the thread keeps serving requests.
pub struct HighlightWorker {
    jobs: Option<Sender<Message>>,
    done: Receiver<Highlighted>,
    thread: Option<JoinHandle<()>>,
    /// Set on drop so the thread stops at the next checkpoint instead of
    /// finishing its batch while the dropping thread waits.
    cancel: Arc<AtomicBool>,
    latest: Latest,
}

impl HighlightWorker {
    pub fn new(store: GrammarStore) -> Self {
        let highlighter = store.clone();
        let worker = Self::with_highlighter(Arc::new(move |language, source, cancelled| {
            highlighter.highlight_until(language, source, cancelled)
        }));
        if let Some(jobs) = worker.jobs.clone() {
            store.subscribe(move || jobs.send(Message::Retry).is_ok());
        }
        worker
    }

    fn with_highlighter(highlight: HighlightFn) -> Self {
        let (jobs, job_rx) = channel::<Message>();
        let (done_tx, done) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let latest = Latest::default();
        let (stop, newest) = (cancel.clone(), latest.clone());
        let thread = std::thread::Builder::new()
            .name("quark-syntax".to_owned())
            .spawn(move || run(&job_rx, &done_tx, &stop, &newest, &*highlight))
            .ok();
        Self {
            jobs: Some(jobs),
            done,
            thread,
            cancel,
            latest,
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
            latest: Latest::default(),
        }
    }

    pub fn request(&self, slot: u64, generation: u64, language: LanguageId, source: Arc<str>) {
        if let Some(jobs) = &self.jobs {
            let mut latest = lock(&self.latest);
            let newest = latest.entry(slot).or_insert(generation);
            *newest = (*newest).max(generation);
            drop(latest);
            let _ = jobs.send(Message::Job(Job {
                slot,
                generation,
                language,
                source,
            }));
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
    /// A worker without grammars: every result is plain.
    fn default() -> Self {
        Self::new(GrammarStore::none())
    }
}

impl Drop for HighlightWorker {
    fn drop(&mut self) {
        // The flag stops a batch in progress; `Stop` ends the thread's loop
        // (the store's subscription keeps a sender, so the channel does not
        // close). The wait is at most one checkpoint.
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(jobs) = self.jobs.take() {
            let _ = jobs.send(Message::Stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// A job still waiting for grammars, and what it last published.
struct Parked {
    job: Job,
    revision: u32,
    spans: Vec<HighlightSpan>,
    unresolved: Vec<LanguageId>,
}

fn run(
    messages: &Receiver<Message>,
    done: &Sender<Highlighted>,
    cancel: &AtomicBool,
    latest: &Mutex<HashMap<u64, u64>>,
    highlight: &Highlight,
) {
    // Jobs whose grammars were pending, newest per slot, rerun on `Retry`.
    let mut parked: HashMap<u64, Parked> = HashMap::new();
    while let Ok(first) = messages.recv() {
        // Coalesce everything queued: keep the newest job per slot, in the
        // order the slots were first requested.
        let mut order = Vec::new();
        let mut newest: HashMap<u64, Job> = HashMap::new();
        let mut retry = false;
        for message in std::iter::once(first).chain(messages.try_iter()) {
            let job = match message {
                Message::Job(job) => job,
                Message::Retry => {
                    retry = true;
                    continue;
                }
                Message::Stop => return,
            };
            parked.remove(&job.slot);
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
        // Slots rerun for a retry, with what each last published: a rerun
        // that changes nothing is parked again without another result.
        let mut retried: HashMap<u64, Parked> = HashMap::new();
        if retry {
            for (slot, waiting) in parked.drain() {
                order.push(slot);
                newest.insert(slot, waiting.job.clone());
                retried.insert(slot, waiting);
            }
        }
        for slot in order {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let Some(job) = newest.remove(&slot) else {
                continue;
            };
            let superseded = || {
                cancel.load(Ordering::Relaxed)
                    || lock(latest)
                        .get(&slot)
                        .is_some_and(|&newest| newest > job.generation)
            };
            // A grammar bug must not take the thread down: every later block
            // would silently stay plain.
            let outcome = catch_unwind(AssertUnwindSafe(|| {
                highlight(&job.language, &job.source, &superseded)
            }))
            .unwrap_or_default();
            if superseded() {
                // The newer job is queued and replaces this one, parked
                // state included.
                continue;
            }
            {
                let mut latest = lock(latest);
                if latest
                    .get(&slot)
                    .is_some_and(|&newest| newest <= job.generation)
                {
                    latest.remove(&slot);
                }
            }
            let mut previous = retried.remove(&slot);
            if let Some(unchanged) = previous.take_if(|previous| {
                previous.spans == outcome.spans && previous.unresolved == outcome.unresolved
            }) {
                // Still waiting on the same grammars.
                parked.insert(slot, unchanged);
                continue;
            }
            let revision = previous.map_or(0, |previous| previous.revision + 1);
            let pending = outcome.pending();
            if pending {
                parked.insert(
                    slot,
                    Parked {
                        job: job.clone(),
                        revision,
                        spans: outcome.spans.clone(),
                        unresolved: outcome.unresolved.clone(),
                    },
                );
            }
            let result = Highlighted {
                slot,
                generation: job.generation,
                revision,
                source: job.source,
                spans: outcome.spans,
                pending,
                unresolved: outcome.unresolved,
            };
            if done.send(result).is_err() {
                return;
            }
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    // The map stays valid if a panic interrupted an update.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panics_on_boom(_: &LanguageId, source: &str, _: &dyn Fn() -> bool) -> Outcome {
        assert_ne!(source, "boom", "highlighter bug");
        Outcome {
            spans: vec![HighlightSpan {
                offset: 0,
                length: source.len() as u32,
                kind: crate::HighlightKind::Keyword,
            }],
            ..Outcome::default()
        }
    }

    fn rust() -> LanguageId {
        LanguageId::from_fence("rust").unwrap()
    }

    // Catches a panicking highlight killing the thread, after which every
    // block would stay plain.
    #[test]
    fn worker_survives_a_panicking_highlight() {
        let worker = HighlightWorker::with_highlighter(Arc::new(panics_on_boom));
        worker.request(1, 1, rust(), Arc::from("boom"));
        let failed = worker.recv().map(|r| (r.slot, r.spans.len()));
        worker.request(2, 2, rust(), Arc::from("fn"));
        let next = worker.recv().map(|r| (r.slot, r.spans.len()));

        assert_eq!((failed, next), (Ok((1, 0)), Ok((2, 1))));
    }

    // Catches a superseded highlight running to the end and publishing:
    // once a newer generation of its slot is requested, the highlight in
    // progress sees it at its next checkpoint and sends no result.
    #[test]
    fn superseded_highlight_stops_at_a_checkpoint_without_a_result() {
        let (started_tx, started) = channel();
        let (resume_tx, resume) = channel::<()>();
        let (seen_tx, seen) = channel();
        let resume = Mutex::new(resume);
        let worker = HighlightWorker::with_highlighter(Arc::new(move |_, source, cancelled| {
            if source == "first" {
                started_tx.send(()).unwrap();
                resume.lock().unwrap().recv().unwrap();
                // The checkpoint a real highlight reaches while parsing.
                seen_tx.send(cancelled()).unwrap();
            }
            panics_on_boom(&rust(), source, cancelled)
        }));
        worker.request(1, 1, rust(), Arc::from("first"));
        started.recv().unwrap();
        worker.request(1, 2, rust(), Arc::from("second"));
        resume_tx.send(()).unwrap();
        let first_result = worker.recv().map(|r| (r.generation, r.source));

        assert_eq!(
            (seen.recv(), first_result),
            (Ok(true), Ok((2, Arc::from("second"))))
        );
    }
}
