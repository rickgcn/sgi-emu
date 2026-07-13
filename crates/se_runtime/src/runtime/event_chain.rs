//! Deterministic dispatch-local event chains.
//!
//! Event chains remove repeated outer runtime dispatches without creating a
//! second scheduling authority. This module owns local ordering, time-horizon
//! checks, deadlines, tracing fallback, and final Scheduler materialization.
//! Machine profiles provide only event classification and compact encoding.

use core::fmt;

use se_core::component::ComponentId;
use se_core::scheduler::{ScheduledEventId, SchedulerError, SimDuration, SimTime};
use se_core::tracing::{TraceField, TraceInterest, TraceLevel, TraceSink, TraceSource};
use smallvec::SmallVec;

use super::RuntimeContext;

/// Machine-specific classification and compact encoding for one event chain.
pub trait EventChainPolicy<E> {
    /// Compact representation used for inlineable events.
    type CompactEvent: Copy;

    /// Returns whether this policy enables event chaining at all.
    fn is_active(&self) -> bool;

    /// Returns the maximum number of inline transitions in one outer dispatch.
    fn budget(&self) -> u8;

    /// Encodes an inlineable event or returns a non-inlineable event unchanged.
    fn encode(&self, event: E) -> Result<Self::CompactEvent, E>;

    /// Restores one compact event before machine dispatch.
    fn decode(&self, event: Self::CompactEvent) -> E;

    /// Returns whether the encoded event's class is enabled by this policy.
    fn is_enabled(&self, event: &Self::CompactEvent) -> bool;

    /// Returns whether dispatching this event prevents another inline transition.
    fn is_barrier(&self, event: &E) -> bool;
}

/// Error produced while managing a dispatch-local event chain.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventChainError {
    /// The underlying simulated-time scheduler rejected an operation.
    Scheduler(SchedulerError),

    /// The dispatch-local production-order counter was exhausted.
    OrdinalOverflow,
}

impl From<SchedulerError> for EventChainError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

impl fmt::Display for EventChainError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scheduler(error) => error.fmt(formatter),
            Self::OrdinalOverflow => write!(formatter, "event-chain ordinal overflow"),
        }
    }
}

impl std::error::Error for EventChainError {}

struct PendingCompact<C> {
    time: SimTime,
    target: ComponentId,
    ordinal: u64,
    event: C,
}

struct PendingBarrier<E> {
    time: SimTime,
    target: ComponentId,
    ordinal: u64,
    event: E,
}

/// Scheduling and tracing context used during one chained outer dispatch.
pub struct EventChainContext<'runtime, 'context, E, S, P>
where
    P: EventChainPolicy<E>,
{
    runtime: &'runtime mut RuntimeContext<'context, E, S>,
    policy: P,
    pending_compact: SmallVec<[PendingCompact<P::CompactEvent>; 8]>,
    pending_barriers: SmallVec<[PendingBarrier<E>; 4]>,
    next_ordinal: u64,
    remaining_budget: u8,
    active: bool,
    barrier: bool,
}

impl<'runtime, 'context, E, S, P> EventChainContext<'runtime, 'context, E, S, P>
where
    S: TraceSink,
    P: EventChainPolicy<E>,
{
    /// Starts one dispatch-local chain over an active runtime context.
    pub fn new(runtime: &'runtime mut RuntimeContext<'context, E, S>, policy: P) -> Self {
        let active = policy.is_active()
            && policy.budget() != 0
            && runtime.trace_interest(TraceSource::Scheduler) == TraceInterest::None;
        let remaining_budget = policy.budget();
        Self {
            runtime,
            policy,
            pending_compact: SmallVec::new(),
            pending_barriers: SmallVec::new(),
            next_ordinal: 0,
            remaining_budget,
            active,
            barrier: false,
        }
    }

    /// Returns the current simulated time.
    pub fn now(&self) -> SimTime {
        self.runtime.now()
    }

    /// Returns the active run deadline.
    pub fn deadline(&self) -> SimTime {
        self.runtime.deadline()
    }

    /// Returns the earliest global or dispatch-local event time.
    pub fn next_event_time(&self) -> Option<SimTime> {
        let local = self
            .pending_compact
            .iter()
            .map(|event| event.time)
            .chain(self.pending_barriers.iter().map(|event| event.time))
            .min();
        match (self.runtime.next_event_time(), local) {
            (Some(global), Some(local)) => Some(global.min(local)),
            (Some(global), None) => Some(global),
            (None, Some(local)) => Some(local),
            (None, None) => None,
        }
    }

    /// Advances time only when no local, global, or deadline boundary is crossed.
    pub fn try_advance_to(&mut self, time: SimTime) -> Result<bool, EventChainError> {
        if self
            .pending_compact
            .iter()
            .any(|transition| transition.time <= time)
            || self
                .pending_barriers
                .iter()
                .any(|transition| transition.time <= time)
        {
            return Ok(false);
        }
        self.runtime.try_advance_to(time).map_err(Into::into)
    }

    /// Returns whether the runtime has been requested to stop.
    pub fn stop_requested(&self) -> bool {
        self.runtime.stop_requested()
    }

    /// Requests that the runtime stop after the current dispatch.
    pub fn request_stop(&mut self) {
        self.runtime.request_stop();
    }

    /// Prevents another inline transition in the current chain.
    pub fn request_barrier(&mut self) {
        self.barrier = true;
    }

    /// Applies the machine policy's barrier classification to one dispatched event.
    pub fn enter_event(&mut self, event: &E) {
        if self.policy.is_barrier(event) {
            self.barrier = true;
        }
    }

    /// Returns the trace sink's coarse interest in one source.
    pub fn trace_interest(&self, source: TraceSource) -> TraceInterest {
        self.runtime.trace_interest(source)
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
        self.runtime
            .trace_lazy(source, level, target, event, build_fields)
    }

    /// Schedules or locally buffers one event at an absolute simulated time.
    ///
    /// A returned identifier exists only when the event entered the Scheduler
    /// immediately. Locally buffered events receive identifiers during
    /// [`Self::finish`].
    pub fn schedule_at(
        &mut self,
        time: SimTime,
        target: ComponentId,
        event: E,
    ) -> Result<Option<ScheduledEventId>, EventChainError> {
        if !self.active {
            return self
                .runtime
                .schedule_at(time, target, event)
                .map(Some)
                .map_err(Into::into);
        }
        if time < self.now() {
            return Err(SchedulerError::EventInPast {
                now: self.now(),
                time,
            }
            .into());
        }
        let ordinal = self.next_ordinal;
        self.next_ordinal = self
            .next_ordinal
            .checked_add(1)
            .ok_or(EventChainError::OrdinalOverflow)?;
        match self.policy.encode(event) {
            Ok(event) => self.pending_compact.push(PendingCompact {
                time,
                target,
                ordinal,
                event,
            }),
            Err(event) => self.pending_barriers.push(PendingBarrier {
                time,
                target,
                ordinal,
                event,
            }),
        }
        Ok(None)
    }

    /// Schedules or locally buffers one event after a relative delay.
    pub fn schedule_after(
        &mut self,
        delay: SimDuration,
        target: ComponentId,
        event: E,
    ) -> Result<Option<ScheduledEventId>, EventChainError> {
        let time = self
            .now()
            .checked_add(delay)
            .ok_or(SchedulerError::TimeOverflow {
                time: self.now(),
                duration: delay,
            })?;
        self.schedule_at(time, target, event)
    }

    /// Removes the next transition when deterministic inline execution is safe.
    pub fn take_next_inline(&mut self) -> Result<Option<(ComponentId, E)>, EventChainError> {
        if !self.active || self.barrier || self.remaining_budget == 0 || self.stop_requested() {
            return Ok(None);
        }
        if self.runtime.trace_interest(TraceSource::Scheduler) != TraceInterest::None {
            self.barrier = true;
            return Ok(None);
        }
        let Some(index) = self
            .pending_compact
            .iter()
            .enumerate()
            .min_by_key(|(_, transition)| (transition.time, transition.ordinal))
            .map(|(index, _)| index)
        else {
            return Ok(None);
        };
        let candidate = &self.pending_compact[index];
        if self
            .pending_barriers
            .iter()
            .any(|barrier| (barrier.time, barrier.ordinal) <= (candidate.time, candidate.ordinal))
            || !self.policy.is_enabled(&candidate.event)
            || candidate.time > self.deadline()
            || self
                .runtime
                .next_event_time()
                .is_some_and(|time| time <= candidate.time)
        {
            return Ok(None);
        }
        let time = candidate.time;
        if !self.runtime.try_advance_to(time)? {
            return Ok(None);
        }
        let transition = self.pending_compact.swap_remove(index);
        self.remaining_budget -= 1;
        Ok(Some((
            transition.target,
            self.policy.decode(transition.event),
        )))
    }

    /// Materializes all remaining events in original production order.
    pub fn finish(&mut self) -> Result<(), EventChainError> {
        while !self.pending_compact.is_empty() || !self.pending_barriers.is_empty() {
            let compact = self
                .pending_compact
                .iter()
                .enumerate()
                .min_by_key(|(_, transition)| transition.ordinal)
                .map(|(index, transition)| (index, transition.ordinal));
            let barrier = self
                .pending_barriers
                .iter()
                .enumerate()
                .min_by_key(|(_, transition)| transition.ordinal)
                .map(|(index, transition)| (index, transition.ordinal));
            if barrier.is_none_or(|(_, barrier_ordinal)| {
                compact.is_some_and(|(_, compact_ordinal)| compact_ordinal < barrier_ordinal)
            }) {
                let transition = self
                    .pending_compact
                    .swap_remove(compact.expect("a pending compact event exists").0);
                self.runtime.schedule_at(
                    transition.time,
                    transition.target,
                    self.policy.decode(transition.event),
                )?;
            } else {
                let transition = self.pending_barriers.swap_remove(
                    barrier
                        .expect("a pending barrier exists when no compact event precedes it")
                        .0,
                );
                self.runtime
                    .schedule_at(transition.time, transition.target, transition.event)?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use se_core::scheduler::Scheduler;
    use se_core::tracing::{NoopTraceSink, TraceRecord, TraceRecorder};

    use super::*;
    use crate::runtime::RuntimeStatistics;

    const TARGET: ComponentId = ComponentId::new(1);

    #[derive(Clone, Debug, Eq, PartialEq)]
    enum TestEvent {
        Compact(u8),
        Barrier(u8),
    }

    #[derive(Clone, Copy)]
    struct TestPolicy {
        active: bool,
        enabled: bool,
        budget: u8,
    }

    impl EventChainPolicy<TestEvent> for TestPolicy {
        type CompactEvent = u8;

        fn is_active(&self) -> bool {
            self.active
        }

        fn budget(&self) -> u8 {
            self.budget
        }

        fn encode(&self, event: TestEvent) -> Result<Self::CompactEvent, TestEvent> {
            match event {
                TestEvent::Compact(value) => Ok(value),
                event @ TestEvent::Barrier(_) => Err(event),
            }
        }

        fn decode(&self, event: Self::CompactEvent) -> TestEvent {
            TestEvent::Compact(event)
        }

        fn is_enabled(&self, _event: &Self::CompactEvent) -> bool {
            self.enabled
        }

        fn is_barrier(&self, event: &TestEvent) -> bool {
            matches!(event, TestEvent::Barrier(_))
        }
    }

    fn active_policy(budget: u8) -> TestPolicy {
        TestPolicy {
            active: true,
            enabled: true,
            budget,
        }
    }

    #[test]
    fn budget_inlines_safe_events_and_materializes_the_remainder() {
        let mut scheduler = Scheduler::new();
        let mut trace = TraceRecorder::new(NoopTraceSink);
        let mut stopped = false;
        let mut statistics = RuntimeStatistics::default();
        {
            let mut runtime = RuntimeContext {
                scheduler: &mut scheduler,
                trace: &mut trace,
                stopped: &mut stopped,
                statistics: &mut statistics,
                deadline: SimTime::new(20),
            };
            let mut chain = EventChainContext::new(&mut runtime, active_policy(1));
            assert_eq!(
                chain
                    .schedule_at(SimTime::new(5), TARGET, TestEvent::Compact(1))
                    .unwrap(),
                None
            );
            chain
                .schedule_at(SimTime::new(6), TARGET, TestEvent::Compact(2))
                .unwrap();
            assert_eq!(
                chain.take_next_inline().unwrap(),
                Some((TARGET, TestEvent::Compact(1)))
            );
            assert_eq!(chain.now(), SimTime::new(5));
            assert_eq!(chain.take_next_inline().unwrap(), None);
            chain.finish().unwrap();
        }
        assert_eq!(statistics.scheduled_events, 1);
        let event = scheduler.pop_next().unwrap();
        assert_eq!(event.time, SimTime::new(6));
        assert_eq!(event.payload, TestEvent::Compact(2));
    }

    #[test]
    fn barriers_and_global_events_preserve_scheduler_order() {
        let mut scheduler = Scheduler::new();
        let global_id = scheduler
            .schedule_at(SimTime::new(4), TARGET, TestEvent::Barrier(9))
            .unwrap();
        let mut trace = TraceRecorder::new(NoopTraceSink);
        let mut stopped = false;
        let mut statistics = RuntimeStatistics::default();
        {
            let mut runtime = RuntimeContext {
                scheduler: &mut scheduler,
                trace: &mut trace,
                stopped: &mut stopped,
                statistics: &mut statistics,
                deadline: SimTime::new(20),
            };
            let mut chain = EventChainContext::new(&mut runtime, active_policy(8));
            chain
                .schedule_at(SimTime::new(10), TARGET, TestEvent::Compact(1))
                .unwrap();
            chain
                .schedule_at(SimTime::new(5), TARGET, TestEvent::Barrier(2))
                .unwrap();
            chain
                .schedule_at(SimTime::new(5), TARGET, TestEvent::Compact(3))
                .unwrap();
            assert_eq!(chain.next_event_time(), Some(SimTime::new(4)));
            assert_eq!(chain.take_next_inline().unwrap(), None);
            chain.finish().unwrap();
        }

        let global = scheduler.pop_next().unwrap();
        assert_eq!(global.id, global_id);
        assert_eq!(global.payload, TestEvent::Barrier(9));
        let barrier = scheduler.pop_next().unwrap();
        let compact = scheduler.pop_next().unwrap();
        assert_eq!(barrier.payload, TestEvent::Barrier(2));
        assert_eq!(compact.payload, TestEvent::Compact(3));
        assert!(barrier.id < compact.id);
        assert_eq!(scheduler.pop_next().unwrap().payload, TestEvent::Compact(1));
    }

    #[test]
    fn deadline_stop_and_explicit_barrier_prevent_inline_dispatch() {
        for mode in 0..4 {
            let mut scheduler = Scheduler::new();
            let mut trace = TraceRecorder::new(NoopTraceSink);
            let mut stopped = false;
            let mut statistics = RuntimeStatistics::default();
            let mut runtime = RuntimeContext {
                scheduler: &mut scheduler,
                trace: &mut trace,
                stopped: &mut stopped,
                statistics: &mut statistics,
                deadline: SimTime::new(if mode == 0 { 4 } else { 20 }),
            };
            let mut chain = EventChainContext::new(&mut runtime, active_policy(8));
            chain
                .schedule_at(SimTime::new(5), TARGET, TestEvent::Compact(1))
                .unwrap();
            match mode {
                0 => {}
                1 => chain.request_stop(),
                2 => chain.request_barrier(),
                3 => chain.enter_event(&TestEvent::Barrier(9)),
                _ => unreachable!(),
            }
            assert_eq!(chain.take_next_inline().unwrap(), None);
            chain.finish().unwrap();
        }
    }

    #[derive(Default)]
    struct SchedulerCaptureSink;

    impl TraceSink for SchedulerCaptureSink {
        fn interest(&self, source: TraceSource) -> TraceInterest {
            if source == TraceSource::Scheduler {
                TraceInterest::All
            } else {
                TraceInterest::None
            }
        }

        fn record(&mut self, _record: TraceRecord<'_>) {}
    }

    #[test]
    fn scheduler_capture_uses_immediate_runtime_scheduling() {
        let mut scheduler = Scheduler::new();
        let mut trace = TraceRecorder::new(SchedulerCaptureSink);
        let mut stopped = false;
        let mut statistics = RuntimeStatistics::default();
        {
            let mut runtime = RuntimeContext {
                scheduler: &mut scheduler,
                trace: &mut trace,
                stopped: &mut stopped,
                statistics: &mut statistics,
                deadline: SimTime::new(20),
            };
            let mut chain = EventChainContext::new(&mut runtime, active_policy(8));
            assert!(
                chain
                    .schedule_at(SimTime::new(5), TARGET, TestEvent::Compact(1))
                    .unwrap()
                    .is_some()
            );
            assert_eq!(chain.take_next_inline().unwrap(), None);
            chain.finish().unwrap();
        }
        assert_eq!(statistics.scheduled_events, 1);
        assert_eq!(scheduler.len(), 1);
    }

    #[test]
    fn invalid_time_and_ordinal_overflow_are_distinct_errors() {
        let mut scheduler = Scheduler::new();
        scheduler.advance_to(SimTime::new(5)).unwrap();
        let mut trace = TraceRecorder::new(NoopTraceSink);
        let mut stopped = false;
        let mut statistics = RuntimeStatistics::default();
        let mut runtime = RuntimeContext {
            scheduler: &mut scheduler,
            trace: &mut trace,
            stopped: &mut stopped,
            statistics: &mut statistics,
            deadline: SimTime::new(20),
        };
        let mut chain = EventChainContext::new(&mut runtime, active_policy(8));
        assert!(matches!(
            chain.schedule_at(SimTime::new(4), TARGET, TestEvent::Compact(1)),
            Err(EventChainError::Scheduler(
                SchedulerError::EventInPast { .. }
            ))
        ));
        chain.next_ordinal = u64::MAX;
        assert_eq!(
            chain.schedule_at(SimTime::new(6), TARGET, TestEvent::Compact(2)),
            Err(EventChainError::OrdinalOverflow)
        );
    }
}
