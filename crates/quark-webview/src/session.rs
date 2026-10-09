//! The webview session state machine and the service the runner drives.
//!
//! [`Sessions`] is pure: API calls and drained native events go in, with
//! the runner's monotonic time; app events and backend commands come out.
//! [`Service`] runs those commands on the compiled [`Backend`].
//!
//! Invariants: every issued handle ends with exactly one `Closed` event;
//! every accepted evaluation ends with exactly one result, removed from the
//! table before delivery; a result reaches the app only while its document
//! is still the view's committed document.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use url::Url;

pub use crate::backend::Serviced;
use crate::backend::{
    self, ACTIVE_SERVICE_INTERVAL, Backend, ClearRequest, EvalDispatch, Inbound, Inbox,
    NativeClose, NativeEvent, NativeParent, NativeSink, OpenRequest, SERVICE_ITERATIONS,
    SharedLive, lock_live,
};
use crate::policy::NavigationPolicy;
use crate::profile::{DataStore, ProfileError, ProfileId};
use crate::script::{self, AsyncScript, OriginGuard, ScriptValue};
use crate::{
    DocumentId, EvalError, Evaluation, EvaluationId, EvaluationLimits, OpenError, PageUrl,
    PlatformError, ProfileClear, Shared, WebCloseReason, WebViewEvent, WebViewHandle,
    WebWindowOptions, complete, oneshot,
};

/// Work for the backend, queued until the current app callback returns.
pub(crate) enum Command {
    Open(Box<OpenRequest>),
    Evaluate {
        view: WebViewHandle,
        dispatch: EvalDispatch,
    },
    Cancel {
        view: WebViewHandle,
        evaluation: EvaluationId,
    },
    Close(WebViewHandle),
    Focus(WebViewHandle),
    ClearProfile(ClearRequest),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Construction requested; no `Opened` yet.
    Opening,
    Open,
    /// `Closed` was emitted; waiting for native teardown.
    Closed,
}

struct View {
    phase: Phase,
    live: SharedLive,
    policy: Arc<NavigationPolicy>,
    limits: EvaluationLimits,
    profile: Option<ProfileId>,
    /// The open command reached the backend, so teardown must be awaited.
    dispatched: bool,
}

struct Slot {
    generation: u32,
    view: Option<View>,
}

enum Delivery {
    Future(Shared<Result<ScriptValue, EvalError>>),
    Event,
}

struct Pending {
    view: WebViewHandle,
    document: DocumentId,
    deadline: Duration,
    delivery: Delivery,
}

/// A broken [`Sessions`] invariant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IntegrityError(String);

pub(crate) struct Sessions {
    slots: Vec<Slot>,
    free: Vec<u32>,
    evaluations: BTreeMap<EvaluationId, Pending>,
    next_evaluation: u64,
    /// Persistent profiles held by a view or a clear.
    profiles: BTreeSet<ProfileId>,
    clears: BTreeMap<u64, (ProfileId, Shared<Result<(), ProfileError>>)>,
    next_clear: u64,
    inbox: Arc<Inbox>,
    events: Vec<WebViewEvent>,
    commands: Vec<Command>,
    exiting: bool,
}

impl Sessions {
    pub(crate) fn new(wake: Box<dyn Fn() + Send + Sync>) -> Self {
        Self {
            slots: Vec::new(),
            free: Vec::new(),
            evaluations: BTreeMap::new(),
            next_evaluation: 1,
            profiles: BTreeSet::new(),
            clears: BTreeMap::new(),
            next_clear: 1,
            inbox: Arc::new(Inbox::new(wake)),
            events: Vec::new(),
            commands: Vec::new(),
            exiting: false,
        }
    }

    fn view(&self, handle: WebViewHandle) -> Option<&View> {
        let slot = self.slots.get(handle.index as usize)?;
        (slot.generation == handle.generation).then_some(slot.view.as_ref()?)
    }

    fn view_mut(&mut self, handle: WebViewHandle) -> Option<&mut View> {
        let slot = self.slots.get_mut(handle.index as usize)?;
        (slot.generation == handle.generation).then_some(slot.view.as_mut()?)
    }

    fn insert(&mut self, view: View) -> WebViewHandle {
        match self.free.pop() {
            Some(index) => {
                let slot = &mut self.slots[index as usize];
                slot.view = Some(view);
                WebViewHandle {
                    index,
                    generation: slot.generation,
                }
            }
            None => {
                self.slots.push(Slot {
                    generation: 0,
                    view: Some(view),
                });
                WebViewHandle {
                    index: self.slots.len() as u32 - 1,
                    generation: 0,
                }
            }
        }
    }

    /// Drop a view's slot and profile lease; its handle goes stale.
    fn release(&mut self, handle: WebViewHandle) {
        let Some(slot) = self.slots.get_mut(handle.index as usize) else {
            return;
        };
        if slot.generation != handle.generation {
            return;
        }
        let Some(view) = slot.view.take() else {
            return;
        };
        slot.generation = slot.generation.wrapping_add(1);
        self.free.push(handle.index);
        if let Some(profile) = view.profile {
            self.profiles.remove(&profile);
        }
    }

    pub(crate) fn open(
        &mut self,
        url: Url,
        options: WebWindowOptions,
        parent: NativeParent,
    ) -> Result<WebViewHandle, OpenError> {
        let policy = Arc::new(options.validate(&url)?);
        let profile = match &options.view.data_store {
            DataStore::Ephemeral => None,
            DataStore::Persistent(profile) if self.profiles.contains(profile) => {
                return Err(OpenError::Profile(ProfileError::InUse));
            }
            DataStore::Persistent(profile) => Some(profile.clone()),
        };
        if let Some(profile) = &profile {
            self.profiles.insert(profile.clone());
        }
        let live = SharedLive::default();
        let handle = self.insert(View {
            phase: Phase::Opening,
            live: Arc::clone(&live),
            policy: Arc::clone(&policy),
            limits: options.view.limits,
            profile,
            dispatched: false,
        });
        let sink = NativeSink::new(handle, Arc::clone(&self.inbox), live, policy);
        self.commands.push(Command::Open(Box::new(OpenRequest {
            view: handle,
            url,
            options,
            parent,
            sink,
        })));
        self.check();
        Ok(handle)
    }

    /// The open command for `view` reached the backend.
    fn dispatched(&mut self, view: WebViewHandle) {
        if let Some(view) = self.view_mut(view) {
            view.dispatched = true;
        }
    }

    /// The backend refused to construct `view`, or reported failure later.
    fn open_failed(&mut self, view: WebViewHandle, error: OpenError) {
        self.close_view(view, WebCloseReason::OpenFailed(error));
        // Nothing native is left to wait for.
        self.release(view);
        self.check();
    }

    fn evaluate(
        &mut self,
        view: WebViewHandle,
        script: &AsyncScript,
        guard: &OriginGuard,
        now: Duration,
        delivery: Delivery,
    ) -> Result<EvaluationId, EvalError> {
        let state = self.view(view).ok_or(EvalError::InvalidHandle)?;
        match state.phase {
            Phase::Closed => return Err(EvalError::InvalidHandle),
            Phase::Opening => return Err(EvalError::FrameNotReady),
            Phase::Open => {}
        }
        let document = {
            let live = lock_live(&state.live);
            let (document, origin) = live.document.as_ref().ok_or(EvalError::FrameNotReady)?;
            if *document != guard.document {
                return Err(EvalError::NavigationChanged);
            }
            if *origin != guard.origin || !state.policy.evaluation_allowed(origin) {
                return Err(EvalError::WrongOrigin);
            }
            *document
        };
        let in_flight = self
            .evaluations
            .values()
            .filter(|pending| pending.view == view)
            .count();
        if in_flight >= state.limits.max_in_flight as usize {
            return Err(EvalError::TooManyRequests);
        }
        let envelope = script::envelope(script, &guard.origin, &state.limits);
        let deadline = now + state.limits.timeout;
        let evaluation = EvaluationId(self.next_evaluation);
        self.next_evaluation += 1;
        self.evaluations.insert(
            evaluation,
            Pending {
                view,
                document,
                deadline,
                delivery,
            },
        );
        self.commands.push(Command::Evaluate {
            view,
            dispatch: EvalDispatch {
                evaluation,
                document,
                envelope,
            },
        });
        self.check();
        Ok(evaluation)
    }

    pub(crate) fn evaluate_future(
        &mut self,
        view: WebViewHandle,
        script: &AsyncScript,
        guard: &OriginGuard,
        now: Duration,
    ) -> Result<Evaluation, EvalError> {
        let shared = oneshot();
        let id = self.evaluate(
            view,
            script,
            guard,
            now,
            Delivery::Future(Arc::clone(&shared)),
        )?;
        Ok(Evaluation {
            id,
            shared,
            inbox: Arc::clone(&self.inbox),
            done: false,
        })
    }

    pub(crate) fn evaluate_event(
        &mut self,
        view: WebViewHandle,
        script: &AsyncScript,
        guard: &OriginGuard,
        now: Duration,
    ) -> Result<EvaluationId, EvalError> {
        self.evaluate(view, script, guard, now, Delivery::Event)
    }

    /// End an evaluation with `result`, removing it first so it ends once.
    /// Tells the backend to stop when it ends for any reason but its own
    /// answer.
    fn finish(
        &mut self,
        evaluation: EvaluationId,
        result: Result<ScriptValue, EvalError>,
        cancel_native: bool,
    ) {
        let Some(pending) = self.evaluations.remove(&evaluation) else {
            return;
        };
        if cancel_native {
            self.commands.push(Command::Cancel {
                view: pending.view,
                evaluation,
            });
        }
        match pending.delivery {
            Delivery::Future(shared) => complete(&shared, result),
            Delivery::Event => self.events.push(WebViewEvent::EvaluationFinished {
                view: pending.view,
                evaluation,
                result,
            }),
        }
    }

    pub(crate) fn cancel_evaluation(&mut self, evaluation: EvaluationId) {
        self.finish(evaluation, Err(EvalError::Cancelled), true);
        self.check();
    }

    /// Close `view` for `reason` unless it already closed: revoke scripts,
    /// fail its evaluations, tell the app, and start native teardown.
    fn close_view(&mut self, handle: WebViewHandle, reason: WebCloseReason) {
        let exiting = self.exiting;
        let Some(view) = self.view_mut(handle) else {
            return;
        };
        if view.phase == Phase::Closed {
            return;
        }
        view.phase = Phase::Closed;
        let dispatched = view.dispatched;
        {
            let mut live = lock_live(&view.live);
            live.closed = true;
            live.document = None;
        }
        let failure = match (&reason, exiting) {
            (_, true) => EvalError::AppExiting,
            (WebCloseReason::ProcessTerminated, _) => EvalError::ProcessTerminated,
            _ => EvalError::WindowClosed,
        };
        let ending: Vec<EvaluationId> = self
            .evaluations
            .iter()
            .filter(|(_, pending)| pending.view == handle)
            .map(|(&id, _)| id)
            .collect();
        for evaluation in ending {
            // The view's teardown stops native work; no separate cancel.
            self.finish(evaluation, Err(failure.clone()), false);
        }
        self.events.push(WebViewEvent::Closed {
            view: handle,
            reason,
        });
        if dispatched {
            self.commands.push(Command::Close(handle));
        } else {
            // Never reached the backend: drop the queued open instead.
            self.commands.retain(
                |command| !matches!(command, Command::Open(request) if request.view == handle),
            );
            self.release(handle);
        }
    }

    pub(crate) fn close(&mut self, view: WebViewHandle, reason: WebCloseReason) {
        self.close_view(view, reason);
        self.check();
    }

    pub(crate) fn focus(&mut self, view: WebViewHandle) {
        if self
            .view(view)
            .is_some_and(|view| view.phase != Phase::Closed && view.dispatched)
        {
            self.commands.push(Command::Focus(view));
        }
    }

    pub(crate) fn clear_profile(
        &mut self,
        profile: &ProfileId,
    ) -> Result<ProfileClear, ProfileError> {
        if self.profiles.contains(profile) {
            return Err(ProfileError::InUse);
        }
        self.profiles.insert(profile.clone());
        let id = self.next_clear;
        self.next_clear += 1;
        let shared = oneshot();
        self.clears
            .insert(id, (profile.clone(), Arc::clone(&shared)));
        self.commands.push(Command::ClearProfile(ClearRequest {
            profile: profile.clone(),
            sink: self.inbox.profile_sink(id),
        }));
        self.check();
        Ok(ProfileClear { shared })
    }

    fn profile_cleared(&mut self, id: u64, result: Result<(), ProfileError>) {
        if let Some((profile, shared)) = self.clears.remove(&id) {
            self.profiles.remove(&profile);
            complete(&shared, result);
        }
    }

    /// Apply queued native events and cancellations, then fail evaluations
    /// whose document changed or whose deadline passed.
    pub(crate) fn drain(&mut self, now: Duration) {
        let (inbound, cancels) = self.inbox.take();
        for evaluation in cancels {
            // The future is gone; nobody sees the result.
            self.finish(evaluation, Err(EvalError::Cancelled), true);
        }
        for inbound in inbound {
            match inbound {
                Inbound::View(view, event) => self.native_event(view, event),
                Inbound::ProfileCleared(id, result) => self.profile_cleared(id, result),
            }
        }
        self.sweep(now);
        self.check();
    }

    fn native_event(&mut self, handle: WebViewHandle, event: NativeEvent) {
        let Some(view) = self.view_mut(handle) else {
            return;
        };
        if view.phase == Phase::Closed {
            match event {
                NativeEvent::Destroyed | NativeEvent::OpenFailed(_) => self.release(handle),
                _ => {}
            }
            return;
        }
        let event = match event {
            NativeEvent::Opened(capabilities) => {
                if view.phase != Phase::Opening {
                    return;
                }
                view.phase = Phase::Open;
                WebViewEvent::Opened {
                    view: handle,
                    capabilities,
                }
            }
            NativeEvent::OpenFailed(error) => return self.open_failed(handle, error),
            NativeEvent::NavigationStarted { navigation, url } => WebViewEvent::NavigationStarted {
                view: handle,
                navigation,
                url: PageUrl(url),
            },
            NativeEvent::NavigationRedirected { navigation, url } => {
                WebViewEvent::NavigationRedirected {
                    view: handle,
                    navigation,
                    url: PageUrl(url),
                }
            }
            NativeEvent::NavigationBlocked { url, reason } => WebViewEvent::NavigationBlocked {
                view: handle,
                url: url.map(PageUrl),
                reason,
            },
            NativeEvent::NavigationCommitted {
                navigation,
                document,
                url,
                origin,
            } => WebViewEvent::NavigationCommitted {
                view: handle,
                navigation,
                document,
                url: PageUrl(url),
                origin,
            },
            NativeEvent::NavigationFailed {
                navigation,
                stage,
                error,
            } => WebViewEvent::NavigationFailed {
                view: handle,
                navigation,
                stage,
                error,
            },
            NativeEvent::LoadFinished {
                navigation,
                document,
                http_status,
            } => WebViewEvent::PageLoadFinished {
                view: handle,
                navigation,
                document,
                http_status,
            },
            NativeEvent::LocationChanged { document, url } => WebViewEvent::LocationChanged {
                view: handle,
                document,
                url: PageUrl(url),
            },
            NativeEvent::TitleChanged(title) => WebViewEvent::TitleChanged {
                view: handle,
                title,
            },
            NativeEvent::EvaluationSettled { evaluation, raw } => {
                return self.settled(handle, evaluation, raw);
            }
            NativeEvent::PolicyViolation => {
                return self.close_view(handle, WebCloseReason::PolicyViolation);
            }
            NativeEvent::Closed(NativeClose::User) => {
                return self.close_view(handle, WebCloseReason::User);
            }
            NativeEvent::Closed(NativeClose::ProcessTerminated) => {
                return self.close_view(handle, WebCloseReason::ProcessTerminated);
            }
            NativeEvent::Destroyed => {
                // Torn down without being asked.
                self.close_view(handle, WebCloseReason::ProcessTerminated);
                return self.release(handle);
            }
        };
        self.events.push(event);
    }

    fn settled(
        &mut self,
        view: WebViewHandle,
        evaluation: EvaluationId,
        raw: script::RawEvaluation,
    ) {
        let Some(pending) = self.evaluations.get(&evaluation) else {
            // Late, duplicate, or already failed.
            return;
        };
        let Some(state) = self.view(view).filter(|_| pending.view == view) else {
            return;
        };
        let current = lock_live(&state.live)
            .document
            .as_ref()
            .map(|(document, _)| *document);
        let result = if current == Some(pending.document) {
            script::decode(raw, &state.limits)
        } else {
            Err(EvalError::NavigationChanged)
        };
        self.finish(evaluation, result, false);
    }

    fn sweep(&mut self, now: Duration) {
        let mut ended = Vec::new();
        for (&evaluation, pending) in &self.evaluations {
            let current = self.view(pending.view).and_then(|view| {
                lock_live(&view.live)
                    .document
                    .as_ref()
                    .map(|(document, _)| *document)
            });
            if current != Some(pending.document) {
                ended.push((evaluation, EvalError::NavigationChanged));
            } else if now >= pending.deadline {
                ended.push((evaluation, EvalError::Timeout));
            }
        }
        for (evaluation, error) in ended {
            self.finish(evaluation, Err(error), true);
        }
    }

    /// The earliest evaluation deadline.
    pub(crate) fn next_deadline(&self) -> Option<Duration> {
        self.evaluations
            .values()
            .map(|pending| pending.deadline)
            .min()
    }

    /// The app is exiting: close every view and fail what is pending.
    pub(crate) fn shutdown(&mut self) {
        self.exiting = true;
        let _ = self.inbox.take();
        let open: Vec<WebViewHandle> = self
            .slots
            .iter()
            .enumerate()
            .filter_map(|(index, slot)| {
                slot.view.as_ref().map(|_| WebViewHandle {
                    index: index as u32,
                    generation: slot.generation,
                })
            })
            .collect();
        for view in open {
            self.close_view(view, WebCloseReason::Quit);
        }
        let exiting = PlatformError::new("clear the profile", "the app is exiting");
        for (_, (_, shared)) in std::mem::take(&mut self.clears) {
            complete(&shared, Err(ProfileError::Platform(exiting.clone())));
        }
    }

    pub(crate) fn take_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.commands)
    }

    pub(crate) fn events(&mut self) -> std::vec::Drain<'_, WebViewEvent> {
        self.events.drain(..)
    }

    /// Views not yet closed, for the runner's parent bookkeeping.
    pub(crate) fn is_live(&self, view: WebViewHandle) -> bool {
        self.view(view)
            .is_some_and(|view| view.phase != Phase::Closed)
    }

    fn check(&self) {
        debug_assert_eq!(self.verify_integrity(), Ok(()));
    }

    pub(crate) fn verify_integrity(&self) -> Result<(), IntegrityError> {
        let fail = |message: String| Err(IntegrityError(message));
        let mut leased = BTreeSet::new();
        for (index, slot) in self.slots.iter().enumerate() {
            let free = self.free.contains(&(index as u32));
            if free == slot.view.is_some() {
                return fail(format!("slot {index} free list disagrees with its view"));
            }
            if let Some(profile) = slot.view.as_ref().and_then(|view| view.profile.clone())
                && !leased.insert(profile)
            {
                return fail(format!("slot {index} shares a persistent profile"));
            }
        }
        leased.extend(self.clears.values().map(|(profile, _)| profile.clone()));
        if leased != self.profiles {
            return fail("profile leases disagree with views and clears".to_owned());
        }
        for (id, pending) in &self.evaluations {
            let Some(view) = self.view(pending.view) else {
                return fail(format!("{id:?} outlived its view"));
            };
            if view.phase != Phase::Open {
                return fail(format!("{id:?} pending on a view that is not open"));
            }
        }
        for (index, slot) in self.slots.iter().enumerate() {
            let Some(view) = &slot.view else { continue };
            let handle = WebViewHandle {
                index: index as u32,
                generation: slot.generation,
            };
            let count = self
                .evaluations
                .values()
                .filter(|pending| pending.view == handle)
                .count();
            if count > view.limits.max_in_flight as usize {
                return fail(format!("slot {index} exceeds its in-flight limit"));
            }
        }
        Ok(())
    }
}

/// The webview service one runner owns: the session state machine plus the
/// compiled backend, if any.
pub struct Service {
    sessions: Sessions,
    backend: Option<Box<dyn Backend>>,
    /// The last service pass ran out of budget.
    exhausted: bool,
    /// Guards against a backend reentering the service.
    servicing: bool,
}

impl Service {
    /// `wake` must make the runner call [`Self::service`] soon, from any
    /// thread.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self {
            sessions: Sessions::new(Box::new(wake)),
            backend: backend::native(),
            exhausted: false,
            servicing: false,
        }
    }

    /// Whether a webview engine is built in.
    pub fn supported(&self) -> bool {
        self.backend.is_some()
    }

    /// Validate and reserve a view; it is built on the next [`Self::flush`].
    pub fn open(
        &mut self,
        url: Url,
        options: WebWindowOptions,
        parent: NativeParent,
    ) -> Result<WebViewHandle, OpenError> {
        if self.backend.is_none() {
            return Err(OpenError::Unsupported);
        }
        self.sessions.open(url, options, parent)
    }

    pub fn evaluate_script(
        &mut self,
        view: WebViewHandle,
        script: &AsyncScript,
        guard: &OriginGuard,
        now: Duration,
    ) -> Result<Evaluation, EvalError> {
        self.sessions.evaluate_future(view, script, guard, now)
    }

    pub fn evaluate_script_event(
        &mut self,
        view: WebViewHandle,
        script: &AsyncScript,
        guard: &OriginGuard,
        now: Duration,
    ) -> Result<EvaluationId, EvalError> {
        self.sessions.evaluate_event(view, script, guard, now)
    }

    pub fn cancel_evaluation(&mut self, evaluation: EvaluationId) {
        self.sessions.cancel_evaluation(evaluation);
    }

    /// Close `view`; idempotent, stale handles are ignored.
    pub fn close(&mut self, view: WebViewHandle, reason: WebCloseReason) {
        self.sessions.close(view, reason);
    }

    pub fn focus(&mut self, view: WebViewHandle) {
        self.sessions.focus(view);
    }

    pub fn clear_profile(&mut self, profile: &ProfileId) -> Result<ProfileClear, ProfileError> {
        if self.backend.is_none() {
            return Err(ProfileError::Unsupported);
        }
        self.sessions.clear_profile(profile)
    }

    pub fn is_live(&self, view: WebViewHandle) -> bool {
        self.sessions.is_live(view)
    }

    /// Run queued commands on the backend. Call after app callbacks
    /// return, never inside one.
    pub fn flush(&mut self) {
        if self.servicing {
            return;
        }
        self.servicing = true;
        loop {
            let commands = self.sessions.take_commands();
            if commands.is_empty() {
                break;
            }
            for command in commands {
                self.run(command);
            }
        }
        self.servicing = false;
    }

    fn run(&mut self, command: Command) {
        let Some(backend) = self.backend.as_mut() else {
            return;
        };
        match command {
            Command::Open(request) => {
                let view = request.view;
                self.sessions.dispatched(view);
                if let Err(error) = backend.open(*request) {
                    self.sessions.open_failed(view, error);
                }
            }
            Command::Evaluate { view, dispatch } => backend.evaluate(view, dispatch),
            Command::Cancel { view, evaluation } => backend.cancel(view, evaluation),
            Command::Close(view) => backend.close(view),
            Command::Focus(view) => backend.focus(view),
            Command::ClearProfile(request) => backend.clear_profile(request),
        }
    }

    /// Pump the backend within its budget, apply what it reported, expire
    /// evaluations, and run the resulting commands. `now` is the runner's
    /// monotonic time.
    pub fn service(&mut self, now: Duration) -> Serviced {
        if self.servicing {
            return Serviced::Idle;
        }
        let serviced = match self.backend.as_mut() {
            Some(backend) => {
                self.servicing = true;
                let serviced = backend.service(SERVICE_ITERATIONS);
                self.servicing = false;
                serviced
            }
            None => Serviced::Idle,
        };
        self.exhausted = serviced == Serviced::Exhausted;
        self.sessions.drain(now);
        self.flush();
        serviced
    }

    /// Events for the app, in order.
    pub fn events(&mut self) -> std::vec::Drain<'_, WebViewEvent> {
        self.sessions.events()
    }

    /// When the runner must call [`Self::service`] next, at the latest:
    /// now after an exhausted pass, within [`ACTIVE_SERVICE_INTERVAL`]
    /// while the backend has native work, and at the earliest evaluation
    /// deadline.
    pub fn next_deadline(&self, now: Duration) -> Option<Duration> {
        if self.exhausted {
            return Some(now);
        }
        let active = self
            .backend
            .as_ref()
            .is_some_and(|backend| backend.needs_service())
            .then(|| now + ACTIVE_SERVICE_INTERVAL);
        [active, self.sessions.next_deadline()]
            .into_iter()
            .flatten()
            .min()
    }

    /// The app is exiting: close every view with
    /// [`WebCloseReason::Quit`], fail pending work with
    /// [`EvalError::AppExiting`], and release the engine.
    pub fn shutdown(&mut self) {
        self.sessions.shutdown();
        self.flush();
        if let Some(backend) = self.backend.as_mut() {
            backend.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll};

    use super::*;
    use crate::policy::Origin;
    use crate::script::RawEvaluation;
    use crate::{
        Appearance, Capabilities, DataStore, ParentRelationship, ProfileMode, WebViewOptions,
    };

    const APP: &str = "https://app.example";
    const IDP: &str = "https://idp.example";

    /// Sessions plus the native sinks of the views they opened, driven the
    /// way a backend drives them.
    struct Harness {
        sessions: Sessions,
        sinks: BTreeMap<WebViewHandle, NativeSink>,
        now: Duration,
        /// Commands the backend received, as text.
        sent: Vec<String>,
    }

    fn origin(origin: &str) -> Origin {
        Origin::parse(origin).unwrap()
    }

    fn url(url: &str) -> Url {
        Url::parse(url).unwrap()
    }

    fn parent() -> NativeParent {
        use raw_window_handle::{
            RawDisplayHandle, RawWindowHandle, XlibDisplayHandle, XlibWindowHandle,
        };
        NativeParent {
            window: RawWindowHandle::Xlib(XlibWindowHandle::new(1)),
            display: RawDisplayHandle::Xlib(XlibDisplayHandle::new(None, 0)),
        }
    }

    fn options() -> WebWindowOptions {
        WebWindowOptions::new(
            WebViewOptions::new([origin(APP), origin(IDP)]).evaluation_origins([origin(APP)]),
        )
    }

    fn script() -> AsyncScript {
        AsyncScript::new("return window.getToken();")
    }

    fn ok(value: &str) -> RawEvaluation {
        RawEvaluation::Envelope(format!(r#"{{"quark":1,"status":"ok","value":{value}}}"#))
    }

    impl Harness {
        fn new() -> Self {
            Self {
                sessions: Sessions::new(Box::new(|| {})),
                sinks: BTreeMap::new(),
                now: Duration::ZERO,
                sent: Vec::new(),
            }
        }

        /// Hand queued commands to the "backend": record them and keep the
        /// sinks of opened views.
        fn flush(&mut self) {
            for command in self.sessions.take_commands() {
                let text = match command {
                    Command::Open(request) => {
                        self.sessions.dispatched(request.view);
                        self.sinks.insert(request.view, request.sink);
                        "open".to_owned()
                    }
                    Command::Evaluate { dispatch, .. } => {
                        format!("evaluate {}", dispatch.evaluation.0)
                    }
                    Command::Cancel { evaluation, .. } => format!("cancel {}", evaluation.0),
                    Command::Close(_) => "close".to_owned(),
                    Command::Focus(_) => "focus".to_owned(),
                    Command::ClearProfile(request) => {
                        request.sink.finished(Ok(()));
                        "clear".to_owned()
                    }
                };
                self.sent.push(text);
            }
        }

        fn drain(&mut self) {
            self.sessions.drain(self.now);
            self.flush();
        }

        fn sink(&self, view: WebViewHandle) -> NativeSink {
            self.sinks[&view].clone()
        }

        fn open_with(&mut self, options: WebWindowOptions) -> Result<WebViewHandle, OpenError> {
            let view = self
                .sessions
                .open(url("https://app.example/login"), options, parent())?;
            self.flush();
            Ok(view)
        }

        /// A view opened and committed on the app origin.
        fn open_committed(&mut self) -> (WebViewHandle, DocumentId) {
            let view = self.open_with(options()).unwrap();
            let sink = self.sink(view);
            sink.opened(Capabilities::new(
                ParentRelationship::Native,
                ProfileMode::Ephemeral,
                Appearance::System,
            ));
            let document = self.navigate(view, "https://app.example/home");
            self.events();
            (view, document)
        }

        fn navigate(&mut self, view: WebViewHandle, to: &str) -> DocumentId {
            let sink = self.sink(view);
            let navigation = sink.navigation_started(&url(to));
            let document = sink.navigation_committed(navigation, &url(to)).unwrap();
            self.drain();
            document
        }

        fn guard(&self, document: DocumentId) -> OriginGuard {
            OriginGuard::new(origin(APP), document)
        }

        fn evaluate(
            &mut self,
            view: WebViewHandle,
            document: DocumentId,
        ) -> Result<EvaluationId, EvalError> {
            let guard = self.guard(document);
            let id = self
                .sessions
                .evaluate_event(view, &script(), &guard, self.now);
            self.flush();
            id
        }

        /// The app events since the last call, as text.
        fn events(&mut self) -> Vec<String> {
            self.sessions
                .events()
                .map(|event| match event {
                    WebViewEvent::Opened { .. } => "opened".to_owned(),
                    WebViewEvent::NavigationStarted { url, .. } => {
                        format!("started {}", url.as_str())
                    }
                    WebViewEvent::NavigationCommitted { url, .. } => {
                        format!("committed {}", url.as_str())
                    }
                    WebViewEvent::NavigationBlocked { reason, .. } => format!("blocked {reason}"),
                    WebViewEvent::PageLoadFinished { .. } => "finished".to_owned(),
                    WebViewEvent::NavigationFailed { error, .. } => format!("failed {error:?}"),
                    WebViewEvent::TitleChanged { title, .. } => format!("title {title}"),
                    WebViewEvent::EvaluationFinished {
                        evaluation, result, ..
                    } => match result {
                        Ok(value) => format!("eval {} ok {}", evaluation.0, value.into_json()),
                        Err(error) => format!("eval {} {error:?}", evaluation.0),
                    },
                    WebViewEvent::Closed { reason, .. } => format!("closed {reason:?}"),
                    other => format!("{other:?}"),
                })
                .collect()
        }
    }

    #[test]
    fn a_result_after_navigating_away_and_back_is_refused() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        let evaluation = h.evaluate(view, document).unwrap();
        h.navigate(view, "https://app.example/home");
        h.sink(view)
            .evaluation_settled(evaluation, ok(r#""token""#));
        h.drain();
        let results: Vec<String> = h
            .events()
            .into_iter()
            .filter(|event| event.starts_with("eval"))
            .collect();
        assert_eq!(results, ["eval 1 NavigationChanged"]);
    }

    #[test]
    fn a_result_for_the_current_document_is_delivered_once() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        let evaluation = h.evaluate(view, document).unwrap();
        let sink = h.sink(view);
        sink.evaluation_settled(evaluation, ok("null"));
        sink.evaluation_settled(evaluation, ok(r#""second""#));
        h.drain();
        assert_eq!(h.events(), ["eval 1 ok null"]);
    }

    #[test]
    fn scripts_are_refused_without_a_matching_committed_document() {
        let mut h = Harness::new();
        let view = h.open_with(options()).unwrap();
        let unknown = DocumentId(999);
        assert_eq!(h.evaluate(view, unknown), Err(EvalError::FrameNotReady));

        let (view, document) = h.open_committed();
        assert_eq!(h.evaluate(view, unknown), Err(EvalError::NavigationChanged));
        let idp_guard = OriginGuard::new(origin(IDP), document);
        let refused = h
            .sessions
            .evaluate_event(view, &script(), &idp_guard, h.now);
        assert_eq!(refused, Err(EvalError::WrongOrigin));

        // Navigable but not an evaluation origin.
        let idp_document = h.navigate(view, "https://idp.example/authorize");
        let idp_guard = OriginGuard::new(origin(IDP), idp_document);
        let refused = h
            .sessions
            .evaluate_event(view, &script(), &idp_guard, h.now);
        assert_eq!(refused, Err(EvalError::WrongOrigin));

        // Mid-navigation, before commit.
        let document = h.navigate(view, "https://app.example/home");
        h.sink(view)
            .navigation_started(&url("https://app.example/next"));
        assert_eq!(h.evaluate(view, document), Err(EvalError::FrameNotReady));
    }

    #[test]
    fn evaluations_past_the_in_flight_limit_are_refused() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        for _ in 0..8 {
            h.evaluate(view, document).unwrap();
        }
        assert_eq!(h.evaluate(view, document), Err(EvalError::TooManyRequests));
    }

    #[test]
    fn a_timed_out_evaluation_ignores_its_late_answer() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        let evaluation = h.evaluate(view, document).unwrap();
        h.now = Duration::from_secs(10);
        h.drain();
        h.sink(view).evaluation_settled(evaluation, ok("1"));
        h.drain();
        assert_eq!(h.events(), ["eval 1 Timeout"]);
        assert_eq!(h.sent.last().map(String::as_str), Some("cancel 1"));
    }

    #[test]
    fn closing_fails_pending_work_and_reports_one_close() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        let evaluation = h.evaluate(view, document).unwrap();
        h.sessions.close(view, WebCloseReason::Program);
        h.sessions.close(view, WebCloseReason::Program);
        let sink = h.sink(view);
        sink.closed(NativeClose::User);
        sink.evaluation_settled(evaluation, ok("1"));
        h.drain();
        assert_eq!(h.events(), ["eval 1 WindowClosed", "closed Program"]);
        assert_eq!(h.evaluate(view, document), Err(EvalError::InvalidHandle));
    }

    #[test]
    fn a_stale_sink_never_reaches_a_view_that_reuses_its_slot() {
        let mut h = Harness::new();
        let (old, _) = h.open_committed();
        let old_sink = h.sink(old);
        h.sessions.close(old, WebCloseReason::Program);
        h.flush();
        old_sink.destroyed();
        h.drain();
        h.events();
        let (new, _) = h.open_committed();
        assert_eq!(new.index, old.index);
        old_sink.title_changed("stale");
        old_sink.closed(NativeClose::User);
        h.drain();
        assert_eq!(h.events(), Vec::<String>::new());
    }

    #[test]
    fn dropping_an_evaluation_future_cancels_it() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        let future = h
            .sessions
            .evaluate_future(view, &script(), &h.guard(document), h.now)
            .unwrap();
        h.flush();
        drop(future);
        h.drain();
        assert_eq!(h.sent.last().map(String::as_str), Some("cancel 1"));
    }

    #[test]
    fn an_evaluation_future_resolves_with_the_decoded_value() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        let future = h
            .sessions
            .evaluate_future(view, &script(), &h.guard(document), h.now)
            .unwrap();
        h.flush();
        h.sink(view)
            .evaluation_settled(future.id(), ok(r#"{"token":"t"}"#));
        h.drain();
        let mut future = pin!(future);
        let mut cx = Context::from_waker(std::task::Waker::noop());
        let Poll::Ready(Ok(value)) = future.as_mut().poll(&mut cx) else {
            panic!("the evaluation did not resolve");
        };
        assert_eq!(value.into_json(), serde_json::json!({ "token": "t" }));
    }

    #[test]
    fn a_disallowed_commit_closes_the_view() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        let sink = h.sink(view);
        let navigation = sink.navigation_started(&url("https://app.example/redirect"));
        assert_eq!(
            sink.navigation_committed(navigation, &url("https://evil.test/")),
            None
        );
        h.drain();
        assert_eq!(
            h.events().last().map(String::as_str),
            Some("closed PolicyViolation")
        );
        assert_eq!(h.evaluate(view, document), Err(EvalError::InvalidHandle));
    }

    #[test]
    fn a_failed_navigation_never_reports_finished() {
        let mut h = Harness::new();
        let (view, _) = h.open_committed();
        let sink = h.sink(view);
        let next = url("https://app.example/next");
        let navigation = sink.navigation_started(&next);
        sink.navigation_committed(navigation, &next);
        sink.navigation_failed(
            navigation,
            crate::FailureStage::Committed,
            crate::NavigationError::Tls,
        );
        sink.load_finished(navigation, Some(200));
        h.drain();
        assert_eq!(
            h.events(),
            [
                "started https://app.example/next",
                "committed https://app.example/next",
                "failed Tls"
            ]
        );
    }

    #[test]
    fn title_floods_coalesce_to_the_latest() {
        let mut h = Harness::new();
        let (view, _) = h.open_committed();
        let sink = h.sink(view);
        for n in 0..5000 {
            sink.title_changed(&format!("title {n}"));
        }
        h.drain();
        assert_eq!(h.events(), ["title title 4999"]);
    }

    #[test]
    fn a_coalesced_title_arrives_after_the_commit_before_it() {
        let mut h = Harness::new();
        let (view, _) = h.open_committed();
        let sink = h.sink(view);
        sink.title_changed("old page");
        let next = url("https://app.example/next");
        let navigation = sink.navigation_started(&next);
        sink.navigation_committed(navigation, &next);
        sink.title_changed("new page");
        h.drain();
        assert_eq!(
            h.events(),
            [
                "started https://app.example/next",
                "committed https://app.example/next",
                "title new page"
            ]
        );
    }

    #[test]
    fn a_persistent_profile_serves_one_view_and_clears_once_released() {
        let mut h = Harness::new();
        let profile = ProfileId::new("com.example.app", "sign-in").unwrap();
        let persistent = || {
            options()
                .view
                .clone()
                .data_store(DataStore::Persistent(profile.clone()))
        };
        let first = h.open_with(WebWindowOptions::new(persistent())).unwrap();
        assert_eq!(
            h.open_with(WebWindowOptions::new(persistent())),
            Err(OpenError::Profile(ProfileError::InUse))
        );
        h.sessions.close(first, WebCloseReason::Program);
        h.flush();
        // Teardown has not finished: the store may still be open.
        assert_eq!(
            h.sessions.clear_profile(&profile).err(),
            Some(ProfileError::InUse)
        );
        h.sink(first).destroyed();
        h.drain();
        let clear = h.sessions.clear_profile(&profile).unwrap();
        h.flush();
        h.drain();
        let mut clear = pin!(clear);
        let mut cx = Context::from_waker(std::task::Waker::noop());
        assert_eq!(clear.as_mut().poll(&mut cx), Poll::Ready(Ok(())));
    }

    #[test]
    fn shutdown_fails_pending_work_as_exiting() {
        let mut h = Harness::new();
        let (view, document) = h.open_committed();
        h.evaluate(view, document).unwrap();
        h.sessions.shutdown();
        assert_eq!(h.events(), ["eval 1 AppExiting", "closed Quit"]);
    }
}
