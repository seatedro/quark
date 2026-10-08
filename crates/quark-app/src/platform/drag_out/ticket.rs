//! Handing a drag start from the UI thread to the Wayland drag thread
//! without letting the thread use a window the UI may have closed.
//!
//! The start names the window's `wl_surface`, which winit destroys when the
//! window closes, on the UI thread. The UI keeps the window borrowed while
//! it waits for the answer, so the surface is live exactly as long as the
//! UI is waiting. The thread must touch it only then, and the UI must not
//! stop waiting while it does, however long the compositor takes.
//!
//! So the thread first claims the request, and the UI, once its patience
//! runs out, abandons it, whichever comes first under one lock. A request
//! the UI abandoned can no longer be claimed, and the thread refuses it
//! without touching the surface. A claimed one holds the UI until the
//! thread answers: the claimed work only queues requests and flushes
//! without blocking, so that wait is short. A flag the UI sets on timeout
//! would not do: the thread could check it just before the UI sets it and
//! returns.

use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

use super::DragOutError;

/// The thread's answer: what the request made, or why it failed.
type Answer<T> = Result<T, DragOutError>;

enum Phase<T> {
    /// Sent, and neither side has acted.
    Waiting,
    /// The thread is using the window. `overdue`: the UI's timeout passed
    /// meanwhile.
    Claimed {
        overdue: bool,
    },
    Answered(Answer<T>),
    /// The UI stopped waiting first.
    Abandoned,
}

struct Shared<T> {
    phase: Mutex<Phase<T>>,
    changed: Condvar,
}

impl<T> Shared<T> {
    fn lock(&self) -> MutexGuard<'_, Phase<T>> {
        // A panic while holding the lock leaves the phase consistent: every
        // change is a single assignment.
        self.phase
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    fn answer(&self, answer: Answer<T>) {
        *self.lock() = Phase::Answered(answer);
        self.changed.notify_all();
    }
}

/// A new request's two ends: the UI's and the thread's.
pub(super) fn ticket<T>() -> (Waiter<T>, Ticket<T>) {
    let shared = Arc::new(Shared {
        phase: Mutex::new(Phase::Waiting),
        changed: Condvar::new(),
    });
    (Waiter(Arc::clone(&shared)), Ticket(Some(shared)))
}

fn stopped() -> DragOutError {
    DragOutError::Platform("the Wayland drag thread stopped".into())
}

/// The UI thread's end.
pub(super) struct Waiter<T>(Arc<Shared<T>>);

impl<T> Waiter<T> {
    /// The thread's answer. If it has not claimed the request within
    /// `timeout`, the request is abandoned and this fails; once it has, this
    /// waits for the answer however long it takes.
    pub fn wait(self, timeout: Duration) -> Answer<T> {
        let shared = &*self.0;
        let pending =
            |phase: &mut Phase<T>| matches!(phase, Phase::Waiting | Phase::Claimed { .. });
        let (mut phase, _) = shared
            .changed
            .wait_timeout_while(shared.lock(), timeout, pending)
            .unwrap_or_else(|poison| poison.into_inner());
        match &mut *phase {
            Phase::Waiting => {
                *phase = Phase::Abandoned;
                return Err(DragOutError::Platform(
                    "the Wayland drag thread did not answer".into(),
                ));
            }
            Phase::Claimed { overdue } => {
                *overdue = true;
                shared.changed.notify_all();
                phase = shared
                    .changed
                    .wait_while(phase, pending)
                    .unwrap_or_else(|poison| poison.into_inner());
            }
            Phase::Answered(_) | Phase::Abandoned => {}
        }
        match std::mem::replace(&mut *phase, Phase::Abandoned) {
            Phase::Answered(answer) => answer,
            _ => Err(stopped()),
        }
    }
}

/// The drag thread's end. Dropped unclaimed, it answers that the thread
/// stopped, so the UI does not wait out its timeout.
pub(super) struct Ticket<T>(Option<Arc<Shared<T>>>);

impl<T> Ticket<T> {
    /// Take the request on, or `None` if the UI stopped waiting for it, in
    /// which case its window may be gone and nothing of it may be touched.
    pub fn claim(mut self) -> Option<Claim<T>> {
        let shared = self.0.take()?;
        let mut phase = shared.lock();
        if !matches!(*phase, Phase::Waiting) {
            return None;
        }
        *phase = Phase::Claimed { overdue: false };
        drop(phase);
        Some(Claim(Some(shared)))
    }

    /// Whether the UI stopped waiting, so the request can be dropped.
    pub fn abandoned(&self) -> bool {
        self.0
            .as_ref()
            .is_some_and(|shared| matches!(*shared.lock(), Phase::Abandoned))
    }
}

impl<T> Drop for Ticket<T> {
    fn drop(&mut self) {
        if let Some(shared) = self.0.take()
            && matches!(*shared.lock(), Phase::Waiting)
        {
            shared.answer(Err(stopped()));
        }
    }
}

/// A request the thread has taken on: the UI waits, holding the window,
/// until this answers. Dropped unanswered (a panic), it answers that the
/// thread stopped, so the UI is never left waiting.
pub(super) struct Claim<T>(Option<Arc<Shared<T>>>);

impl<T> Claim<T> {
    pub fn answer(mut self, answer: Answer<T>) {
        let Some(shared) = self.0.take() else {
            return;
        };
        if matches!(*shared.lock(), Phase::Claimed { overdue: true }) {
            tracing::debug!("drag out: answered after the UI's timeout");
        }
        shared.answer(answer);
    }

    /// Block until the UI's timeout has passed while this is claimed.
    #[cfg(test)]
    fn wait_until_overdue(&self) {
        let shared = self.0.as_ref().expect("unanswered");
        let _phase = shared
            .changed
            .wait_while(shared.lock(), |phase| {
                matches!(phase, Phase::Claimed { overdue: false })
            })
            .unwrap();
    }
}

impl<T> Drop for Claim<T> {
    fn drop(&mut self) {
        if let Some(shared) = self.0.take() {
            shared.answer(Err(stopped()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_start_the_ui_stopped_waiting_for_cannot_be_claimed() {
        let (waiter, ticket) = ticket::<()>();
        assert_eq!(
            waiter.wait(Duration::ZERO),
            Err(DragOutError::Platform(
                "the Wayland drag thread did not answer".into()
            ))
        );
        assert!(ticket.claim().is_none());
    }

    #[test]
    fn a_claimed_start_holds_the_ui_past_its_timeout_until_answered() {
        let (waiter, ticket) = ticket::<()>();
        let claim = ticket.claim().expect("still waited for");
        // Answers only once the UI's (zero) timeout has passed, so a waiter
        // that gave up on timeout would return the timeout error instead.
        std::thread::spawn(move || {
            claim.wait_until_overdue();
            claim.answer(Err(DragOutError::AmbiguousSeat));
        });
        assert_eq!(
            waiter.wait(Duration::ZERO),
            Err(DragOutError::AmbiguousSeat)
        );
    }

    #[test]
    fn a_start_the_thread_lets_go_of_answers_the_ui_at_once() {
        type LetGo = fn(Ticket<()>);
        let unclaimed: LetGo = |ticket| drop(ticket);
        let claimed_then_dropped: LetGo = |ticket| drop(ticket.claim());
        let cases: [(&str, LetGo); 2] = [
            ("dropped unclaimed, as on stop", unclaimed),
            ("dropped claimed, as on a panic", claimed_then_dropped),
        ];
        for (name, let_go) in cases {
            let (waiter, ticket) = ticket::<()>();
            let_go(ticket);
            // A timeout far past the test's runtime: only the answer ends it.
            assert_eq!(
                waiter.wait(Duration::from_secs(3600)),
                Err(stopped()),
                "{name}"
            );
        }
    }
}
