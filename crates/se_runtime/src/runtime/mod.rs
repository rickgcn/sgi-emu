//! Simulated-time runtime.
//!
//! The runtime drives scheduled events until an internal [`SimTime`] boundary.
//! It does not use host wall-clock time and does not know machine-specific event
//! semantics. A machine layer supplies a dispatch closure that interprets event
//! payloads and may schedule more events through [`RuntimeContext`].

pub mod event_chain;
pub mod state;

use core::fmt;

use se_core::component::ComponentId;
use se_core::scheduler::{
    ScheduledEvent, ScheduledEventId, Scheduler, SchedulerError, SimDuration, SimTime,
};
use se_core::tracing::{
    NoopTraceSink, TraceField, TraceInterest, TraceLevel, TraceRecorder, TraceSink, TraceSource,
};

use crate::registry::ComponentRegistry;

const UNBOUNDED_DEADLINE: SimTime = SimTime::new(u64::MAX);

/// Cumulative runtime event counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct RuntimeStatistics {
    /// Events accepted by the scheduler since runtime construction.
    pub scheduled_events: u64,

    /// Events removed from the scheduler and delivered since construction.
    pub dispatched_events: u64,
}

/// Runtime loop status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunStatus {
    /// One event was dispatched.
    Dispatched,

    /// No events were pending.
    Idle,

    /// The requested simulated-time deadline was reached.
    DeadlineReached,

    /// The requested event dispatch limit was reached.
    StepLimitReached,

    /// The runtime stopped after a stop request.
    Stopped,
}

/// Error produced by runtime execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RunError<D> {
    /// Scheduler operation failed.
    Scheduler(SchedulerError),

    /// Machine-specific dispatch failed.
    Dispatch(D),
}

impl<D> From<SchedulerError> for RunError<D> {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl<D> fmt::Display for RunError<D>
where
    D: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scheduler(error) => write!(f, "{error}"),
            Self::Dispatch(error) => write!(f, "{error}"),
        }
    }
}

impl<D> std::error::Error for RunError<D> where D: std::error::Error + 'static {}

/// Event-driven runtime.
pub struct Runtime<E, S = NoopTraceSink> {
    scheduler: Scheduler<E>,
    registry: ComponentRegistry,
    trace: TraceRecorder<S>,
    stopped: bool,
    statistics: RuntimeStatistics,
}

impl<E> Runtime<E, NoopTraceSink> {
    /// Creates a runtime with a noop trace sink.
    pub fn new() -> Self {
        Self::with_trace_sink(NoopTraceSink)
    }

    /// Creates a runtime with a noop trace sink.
    pub fn noop() -> Self {
        Self::new()
    }
}

impl<E> Default for Runtime<E, NoopTraceSink> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E, S> Runtime<E, S> {
    /// Creates a runtime with the given trace sink.
    pub fn with_trace_sink(sink: S) -> Self {
        Self {
            scheduler: Scheduler::new(),
            registry: ComponentRegistry::new(),
            trace: TraceRecorder::new(sink),
            stopped: false,
            statistics: RuntimeStatistics {
                scheduled_events: 0,
                dispatched_events: 0,
            },
        }
    }

    /// Creates a runtime from existing runtime parts.
    pub const fn from_parts(
        scheduler: Scheduler<E>,
        registry: ComponentRegistry,
        trace: TraceRecorder<S>,
    ) -> Self {
        Self {
            scheduler,
            registry,
            trace,
            stopped: false,
            statistics: RuntimeStatistics {
                scheduled_events: 0,
                dispatched_events: 0,
            },
        }
    }

    /// Returns the current simulated time.
    pub const fn now(&self) -> SimTime {
        self.scheduler.now()
    }

    /// Returns whether the runtime is stopped.
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Returns cumulative event counters.
    pub const fn statistics(&self) -> RuntimeStatistics {
        self.statistics
    }

    /// Clears the stopped state.
    pub fn clear_stop(&mut self) {
        self.stopped = false;
    }

    /// Returns an immutable scheduler reference.
    pub const fn scheduler(&self) -> &Scheduler<E> {
        &self.scheduler
    }

    /// Returns a mutable scheduler reference.
    pub const fn scheduler_mut(&mut self) -> &mut Scheduler<E> {
        &mut self.scheduler
    }

    /// Returns an immutable component registry reference.
    pub const fn registry(&self) -> &ComponentRegistry {
        &self.registry
    }

    /// Returns a mutable component registry reference.
    pub const fn registry_mut(&mut self) -> &mut ComponentRegistry {
        &mut self.registry
    }

    /// Returns an immutable trace recorder reference.
    pub const fn trace_recorder(&self) -> &TraceRecorder<S> {
        &self.trace
    }

    /// Returns a mutable trace recorder reference.
    pub const fn trace_recorder_mut(&mut self) -> &mut TraceRecorder<S> {
        &mut self.trace
    }

    /// Consumes the runtime and returns its parts.
    pub fn into_parts(self) -> (Scheduler<E>, ComponentRegistry, TraceRecorder<S>) {
        (self.scheduler, self.registry, self.trace)
    }
}

impl<E, S> Runtime<E, S>
where
    S: TraceSink,
{
    /// Schedules an initial event at an absolute simulated time.
    pub fn schedule_at(
        &mut self,
        time: SimTime,
        target: ComponentId,
        payload: E,
    ) -> Result<ScheduledEventId, SchedulerError> {
        let id = self.scheduler.schedule_at(time, target, payload)?;
        self.statistics.scheduled_events = self.statistics.scheduled_events.saturating_add(1);
        trace_event_scheduled(&mut self.trace, self.scheduler.now(), id, target, time);
        Ok(id)
    }

    /// Schedules an initial event after a relative simulated delay.
    pub fn schedule_after(
        &mut self,
        delay: SimDuration,
        target: ComponentId,
        payload: E,
    ) -> Result<ScheduledEventId, SchedulerError> {
        let time = self
            .scheduler
            .now()
            .checked_add(delay)
            .ok_or(SchedulerError::TimeOverflow {
                time: self.scheduler.now(),
                duration: delay,
            })?;

        self.schedule_at(time, target, payload)
    }

    /// Dispatches the next pending event.
    pub fn dispatch_next<D, F>(&mut self, mut dispatch: F) -> Result<RunStatus, RunError<D>>
    where
        F: for<'ctx> FnMut(
            ScheduledEvent<E>,
            &'ctx mut ComponentRegistry,
            &'ctx mut RuntimeContext<'ctx, E, S>,
        ) -> Result<(), D>,
    {
        self.dispatch_next_with_deadline(UNBOUNDED_DEADLINE, &mut dispatch)
    }

    /// Dispatches at most `max_events` pending events.
    pub fn run_steps<D, F>(
        &mut self,
        max_events: usize,
        mut dispatch: F,
    ) -> Result<RunStatus, RunError<D>>
    where
        F: for<'ctx> FnMut(
            ScheduledEvent<E>,
            &'ctx mut ComponentRegistry,
            &'ctx mut RuntimeContext<'ctx, E, S>,
        ) -> Result<(), D>,
    {
        if self.stopped {
            return Ok(RunStatus::Stopped);
        }

        let mut dispatched = 0;
        while dispatched < max_events {
            match self.dispatch_next_with_deadline(UNBOUNDED_DEADLINE, &mut dispatch)? {
                RunStatus::Dispatched => dispatched += 1,
                status => return Ok(status),
            }
        }

        Ok(RunStatus::StepLimitReached)
    }

    /// Runs scheduled events until the given internal simulated-time deadline.
    pub fn run_until_time<D, F>(
        &mut self,
        deadline: SimTime,
        mut dispatch: F,
    ) -> Result<RunStatus, RunError<D>>
    where
        F: for<'ctx> FnMut(
            ScheduledEvent<E>,
            &'ctx mut ComponentRegistry,
            &'ctx mut RuntimeContext<'ctx, E, S>,
        ) -> Result<(), D>,
    {
        if deadline < self.scheduler.now() {
            return Err(SchedulerError::EventInPast {
                now: self.scheduler.now(),
                time: deadline,
            }
            .into());
        }

        if self.stopped {
            return Ok(RunStatus::Stopped);
        }

        loop {
            match self.scheduler.peek_next_time() {
                None => {
                    self.scheduler.advance_to(deadline)?;
                    return Ok(RunStatus::Idle);
                }
                Some(time) if time > deadline => {
                    self.scheduler.advance_to(deadline)?;
                    return Ok(RunStatus::DeadlineReached);
                }
                Some(_) => match self.dispatch_next_with_deadline(deadline, &mut dispatch)? {
                    RunStatus::Dispatched => {}
                    status => return Ok(status),
                },
            }
        }
    }

    fn dispatch_next_with_deadline<D, F>(
        &mut self,
        deadline: SimTime,
        dispatch: &mut F,
    ) -> Result<RunStatus, RunError<D>>
    where
        F: for<'ctx> FnMut(
            ScheduledEvent<E>,
            &'ctx mut ComponentRegistry,
            &'ctx mut RuntimeContext<'ctx, E, S>,
        ) -> Result<(), D>,
    {
        if self.stopped {
            return Ok(RunStatus::Stopped);
        }

        let Some(event) = self.scheduler.pop_next() else {
            return Ok(RunStatus::Idle);
        };
        self.statistics.dispatched_events = self.statistics.dispatched_events.saturating_add(1);

        trace_event_dispatched(
            &mut self.trace,
            self.scheduler.now(),
            event.id,
            event.target,
            event.time,
        );

        let mut context = RuntimeContext {
            scheduler: &mut self.scheduler,
            trace: &mut self.trace,
            stopped: &mut self.stopped,
            statistics: &mut self.statistics,
            deadline,
        };

        dispatch(event, &mut self.registry, &mut context).map_err(RunError::Dispatch)?;

        if self.stopped {
            Ok(RunStatus::Stopped)
        } else {
            Ok(RunStatus::Dispatched)
        }
    }
}

/// Runtime context passed to machine-specific event dispatch.
pub struct RuntimeContext<'a, E, S> {
    scheduler: &'a mut Scheduler<E>,
    trace: &'a mut TraceRecorder<S>,
    stopped: &'a mut bool,
    statistics: &'a mut RuntimeStatistics,
    deadline: SimTime,
}

impl<E, S> RuntimeContext<'_, E, S> {
    /// Returns the current simulated time.
    pub const fn now(&self) -> SimTime {
        self.scheduler.now()
    }

    /// Returns the active run deadline.
    pub const fn deadline(&self) -> SimTime {
        self.deadline
    }

    /// Returns the next scheduled event time.
    pub fn next_event_time(&self) -> Option<SimTime> {
        self.scheduler.peek_next_time()
    }

    /// Returns the time horizon that active components should not execute past.
    pub fn time_horizon(&self) -> SimTime {
        self.next_event_time()
            .map_or(self.deadline, |time| time.min(self.deadline))
    }

    /// Advances to a component-selected time only when no event or deadline is crossed.
    pub fn try_advance_to(&mut self, time: SimTime) -> Result<bool, SchedulerError> {
        if time < self.scheduler.now() {
            return Err(SchedulerError::EventInPast {
                now: self.scheduler.now(),
                time,
            });
        }
        if time > self.deadline
            || self
                .scheduler
                .peek_next_time()
                .is_some_and(|event_time| event_time <= time)
        {
            return Ok(false);
        }
        self.scheduler.advance_to(time)?;
        Ok(true)
    }

    /// Returns whether the runtime has been requested to stop.
    pub fn stop_requested(&self) -> bool {
        *self.stopped
    }

    /// Requests that the active run loop stop after the current dispatch.
    pub fn request_stop(&mut self) {
        *self.stopped = true;
    }
}

impl<E, S> RuntimeContext<'_, E, S>
where
    S: TraceSink,
{
    /// Returns the trace sink's coarse interest in one source.
    pub fn trace_interest(&self, source: TraceSource) -> TraceInterest {
        self.trace.interest(source)
    }

    /// Schedules an event at an absolute simulated time.
    pub fn schedule_at(
        &mut self,
        time: SimTime,
        target: ComponentId,
        payload: E,
    ) -> Result<ScheduledEventId, SchedulerError> {
        let id = self.scheduler.schedule_at(time, target, payload)?;
        self.statistics.scheduled_events = self.statistics.scheduled_events.saturating_add(1);
        trace_event_scheduled(self.trace, self.scheduler.now(), id, target, time);
        Ok(id)
    }

    /// Schedules an event after a relative simulated delay.
    pub fn schedule_after(
        &mut self,
        delay: SimDuration,
        target: ComponentId,
        payload: E,
    ) -> Result<ScheduledEventId, SchedulerError> {
        let time = self
            .scheduler
            .now()
            .checked_add(delay)
            .ok_or(SchedulerError::TimeOverflow {
                time: self.scheduler.now(),
                duration: delay,
            })?;

        self.schedule_at(time, target, payload)
    }

    /// Records one structured trace fact at the current simulated time.
    pub fn trace<'field>(
        &mut self,
        source: TraceSource,
        level: TraceLevel,
        target: &'field str,
        event: &'field str,
        fields: &'field [TraceField<'field>],
    ) -> u64 {
        self.trace
            .record(self.scheduler.now(), source, level, target, event, fields)
    }

    /// Lazily constructs and records one trace fact at the current time.
    pub fn trace_lazy<'field, F, T>(
        &mut self,
        source: TraceSource,
        level: TraceLevel,
        target: &'field str,
        event: &'field str,
        build_fields: F,
    ) -> Option<u64>
    where
        F: FnOnce() -> T,
        T: AsRef<[TraceField<'field>]>,
    {
        self.trace.record_lazy(
            self.scheduler.now(),
            source,
            level,
            target,
            event,
            build_fields,
        )
    }
}

fn trace_event_scheduled<S>(
    trace: &mut TraceRecorder<S>,
    now: SimTime,
    id: ScheduledEventId,
    target: ComponentId,
    delivery_time: SimTime,
) where
    S: TraceSink,
{
    trace.record_lazy(
        now,
        TraceSource::Scheduler,
        TraceLevel::Trace,
        "scheduler",
        "event_scheduled",
        || {
            [
                TraceField::u64("event_id", id.get()),
                TraceField::u64("target_component", target.get()),
                TraceField::u64("delivery_time", delivery_time.get()),
            ]
        },
    );
}

fn trace_event_dispatched<S>(
    trace: &mut TraceRecorder<S>,
    now: SimTime,
    id: ScheduledEventId,
    target: ComponentId,
    event_time: SimTime,
) where
    S: TraceSink,
{
    trace.record_lazy(
        now,
        TraceSource::Scheduler,
        TraceLevel::Trace,
        "scheduler",
        "event_dispatched",
        || {
            [
                TraceField::u64("event_id", id.get()),
                TraceField::u64("target_component", target.get()),
                TraceField::u64("event_time", event_time.get()),
            ]
        },
    );
}

#[cfg(test)]
mod tests;
