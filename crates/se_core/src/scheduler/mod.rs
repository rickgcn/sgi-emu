//! Event scheduling and simulated time.
//!
//! The scheduler owns internal simulated time and determines when events are
//! delivered. It provides deterministic ordering for delayed hardware behavior
//! without requiring components to call each other directly.
//!
//! Simulated time is not host wall-clock time. The scheduler does not use
//! [`std::time::Instant`], sleeping, or real-time pacing. A machine profile may
//! define the physical meaning of one [`SimTime`] tick, but the scheduler treats
//! it as an opaque monotonic integer.
//!
//! Events are data, not callbacks. The scheduler stores an event payload, its
//! target component, and the simulated time when it becomes ready. Runtime code
//! is responsible for popping ready events and dispatching them to components.
//!
//! The scheduler does not understand bus semantics, component internals, or
//! role dispatch. It is responsible only for time, ordering, and event storage.
//! This separation keeps hardware behavior event-driven and prevents implicit
//! time advancement through nested direct calls between components.

use core::cmp::Ordering;
use core::fmt;
use std::collections::BinaryHeap;

use crate::component::ComponentId;

/// Absolute simulated time.
///
/// The scheduler treats this value as an opaque integer tick. A machine model
/// may define what one tick means for its own timing domain. The unit is part
/// of the machine timing ABI, not a runtime scheduler setting.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SimTime(u64);

impl SimTime {
    /// Zero simulated time.
    pub const ZERO: Self = Self(0);

    /// Creates simulated time from a raw tick value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw tick value.
    pub const fn get(self) -> u64 {
        self.0
    }

    /// Adds a duration and returns `None` on overflow.
    pub fn checked_add(self, duration: SimDuration) -> Option<Self> {
        self.0.checked_add(duration.0).map(Self)
    }
}

impl fmt::Display for SimTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Relative simulated time.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SimDuration(u64);

impl SimDuration {
    /// Zero simulated duration.
    pub const ZERO: Self = Self(0);

    /// Creates a simulated duration from a raw tick value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw tick value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for SimDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Stable identifier for a scheduled event.
///
/// Event identifiers are assigned by the scheduler in insertion order. They are
/// also used as the deterministic tie-breaker for events scheduled at the same
/// simulated time.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ScheduledEventId(u64);

impl ScheduledEventId {
    /// Creates an event identifier from a raw value.
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// Returns the raw identifier value.
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ScheduledEventId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "event:{}", self.0)
    }
}

/// Event stored by the scheduler.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ScheduledEvent<E> {
    /// Stable event identifier.
    pub id: ScheduledEventId,

    /// Simulated time when the event should be delivered.
    pub time: SimTime,

    /// Target component that should receive the event.
    pub target: ComponentId,

    /// Machine-specific event payload.
    pub payload: E,
}

/// Errors produced while scheduling events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    /// The requested delivery time is earlier than the current simulated time.
    EventInPast {
        /// Current scheduler time.
        now: SimTime,

        /// Requested event delivery time.
        time: SimTime,
    },

    /// Adding a duration to the current simulated time overflowed.
    TimeOverflow {
        /// Base simulated time.
        time: SimTime,

        /// Duration that was added to the base time.
        duration: SimDuration,
    },

    /// The scheduler exhausted the event identifier space.
    EventIdOverflow,
}

impl fmt::Display for SchedulerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventInPast { now, time } => {
                write!(f, "event time {time} is earlier than current time {now}")
            }
            Self::TimeOverflow { time, duration } => {
                write!(
                    f,
                    "time overflow while adding duration {duration} to {time}"
                )
            }
            Self::EventIdOverflow => write!(f, "event identifier overflow"),
        }
    }
}

impl std::error::Error for SchedulerError {}

/// Deterministic simulated-time event queue.
///
/// Events are ordered by simulated time first, then by insertion order. Events
/// scheduled for the same time are popped in the order they were scheduled.
pub struct Scheduler<E> {
    now: SimTime,
    next_event_id: u64,
    next: Option<QueuedEvent<E>>,
    queue: BinaryHeap<QueuedEvent<E>>,
}

impl<E> Scheduler<E> {
    /// Creates an empty scheduler at time zero.
    pub fn new() -> Self {
        Self {
            now: SimTime::ZERO,
            next_event_id: 0,
            next: None,
            queue: BinaryHeap::new(),
        }
    }

    /// Returns the current simulated time.
    pub const fn now(&self) -> SimTime {
        self.now
    }

    /// Returns the number of pending events.
    pub fn len(&self) -> usize {
        self.queue.len() + usize::from(self.next.is_some())
    }

    /// Returns whether the scheduler has no pending events.
    pub fn is_empty(&self) -> bool {
        self.next.is_none()
    }

    /// Removes all pending events without changing the current time.
    pub fn clear(&mut self) {
        self.queue.clear();
        self.next = None;
    }

    /// Returns the delivery time of the next event without removing it.
    pub fn peek_next_time(&self) -> Option<SimTime> {
        self.next.as_ref().map(|event| event.inner.time)
    }

    /// Advances simulated time without delivering events.
    ///
    /// This is used by runtimes that advance to an externally selected
    /// simulated-time boundary. It never dispatches events, creates events, or
    /// changes event ordering.
    pub fn advance_to(&mut self, time: SimTime) -> Result<(), SchedulerError> {
        if time < self.now {
            return Err(SchedulerError::EventInPast {
                now: self.now,
                time,
            });
        }

        self.now = time;
        Ok(())
    }

    /// Schedules an event at an absolute simulated time.
    pub fn schedule_at(
        &mut self,
        time: SimTime,
        target: ComponentId,
        payload: E,
    ) -> Result<ScheduledEventId, SchedulerError> {
        if time < self.now {
            return Err(SchedulerError::EventInPast {
                now: self.now,
                time,
            });
        }

        let id = self.allocate_event_id()?;
        let queued = QueuedEvent {
            inner: ScheduledEvent {
                id,
                time,
                target,
                payload,
            },
        };
        match self.next.take() {
            None => self.next = Some(queued),
            Some(next) if queued.precedes(&next) => {
                self.queue.push(next);
                self.next = Some(queued);
            }
            Some(next) => {
                self.queue.push(queued);
                self.next = Some(next);
            }
        }

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
            .now
            .checked_add(delay)
            .ok_or(SchedulerError::TimeOverflow {
                time: self.now,
                duration: delay,
            })?;

        self.schedule_at(time, target, payload)
    }

    /// Pops the next pending event and advances simulated time to its time.
    pub fn pop_next(&mut self) -> Option<ScheduledEvent<E>> {
        let event = self.next.take()?.inner;
        self.next = self.queue.pop();
        self.now = event.time;
        Some(event)
    }

    fn allocate_event_id(&mut self) -> Result<ScheduledEventId, SchedulerError> {
        let id = ScheduledEventId::new(self.next_event_id);
        self.next_event_id = self
            .next_event_id
            .checked_add(1)
            .ok_or(SchedulerError::EventIdOverflow)?;
        Ok(id)
    }
}

impl<E> Default for Scheduler<E> {
    fn default() -> Self {
        Self::new()
    }
}

struct QueuedEvent<E> {
    inner: ScheduledEvent<E>,
}

impl<E> QueuedEvent<E> {
    fn precedes(&self, other: &Self) -> bool {
        (self.inner.time, self.inner.id) < (other.inner.time, other.inner.id)
    }
}

impl<E> Ord for QueuedEvent<E> {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .inner
            .time
            .cmp(&self.inner.time)
            .then_with(|| other.inner.id.cmp(&self.inner.id))
    }
}

impl<E> PartialOrd for QueuedEvent<E> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<E> PartialEq for QueuedEvent<E> {
    fn eq(&self, other: &Self) -> bool {
        self.inner.id == other.inner.id
    }
}

impl<E> Eq for QueuedEvent<E> {}

#[cfg(test)]
mod tests;
