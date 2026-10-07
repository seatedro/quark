//! Highlighting on a background thread.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender, channel};
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

/// Highlights requests on one background thread. A slot is one code block;
/// when several requests for a slot are queued, only the newest generation
/// is highlighted, so a block that streams faster than it highlights does
/// not build a backlog. Results arrive in [`HighlightWorker::try_recv`];
/// callers still compare generations, since a result can land after a
/// newer request was sent.
pub struct HighlightWorker {
    jobs: Option<Sender<Job>>,
    done: Receiver<Highlighted>,
    thread: Option<JoinHandle<()>>,
}

impl HighlightWorker {
    pub fn new() -> Self {
        let (jobs, job_rx) = channel::<Job>();
        let (done_tx, done) = channel();
        let thread = std::thread::Builder::new()
            .name("quark-syntax".to_owned())
            .spawn(move || run(job_rx, done_tx))
            .ok();
        Self {
            jobs: Some(jobs),
            done,
            thread,
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

    /// A finished highlight, without blocking.
    pub fn try_recv(&self) -> Option<Highlighted> {
        self.done.try_recv().ok()
    }

    /// The next finished highlight, blocking until one arrives. `None`
    /// when the thread is gone.
    pub fn recv(&self) -> Option<Highlighted> {
        self.done.recv().ok()
    }
}

impl Default for HighlightWorker {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for HighlightWorker {
    fn drop(&mut self) {
        // Closing the job channel ends the thread's loop.
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn run(jobs: Receiver<Job>, done: Sender<Highlighted>) {
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
            let Some(job) = newest.remove(&slot) else {
                continue;
            };
            let spans = highlight(job.language, &job.source);
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
