//! Persistent scheduler state.

use core::fmt;
use std::collections::{BTreeSet, BinaryHeap};

use super::{QueuedEvent, ScheduledEvent, ScheduledEventId, Scheduler, SimTime};

/// Complete deterministic scheduler state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SchedulerState<E> {
    now: SimTime,
    next_event_id: u64,
    events: Vec<ScheduledEvent<E>>,
}

impl<E> SchedulerState<E> {
    /// Returns the current simulated time.
    pub const fn now(&self) -> SimTime {
        self.now
    }

    /// Returns the identifier that will be assigned to the next event.
    pub const fn next_event_id(&self) -> u64 {
        self.next_event_id
    }

    /// Returns pending events in deterministic delivery order.
    pub fn events(&self) -> &[ScheduledEvent<E>] {
        &self.events
    }
}

/// Invalid serialized scheduler state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerStateError {
    /// One event precedes the serialized scheduler time.
    EventInPast {
        now: SimTime,
        event_id: ScheduledEventId,
        event_time: SimTime,
    },
    /// Two pending events use the same identifier.
    DuplicateEventId { event_id: ScheduledEventId },
    /// Two adjacent events are not in deterministic delivery order.
    EventsOutOfOrder {
        previous_event_id: ScheduledEventId,
        previous_event_time: SimTime,
        event_id: ScheduledEventId,
        event_time: SimTime,
    },
    /// The next identifier would collide with a pending event.
    InvalidNextEventId {
        next_event_id: u64,
        event_id: ScheduledEventId,
    },
}

impl fmt::Display for SchedulerStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EventInPast {
                now,
                event_id,
                event_time,
            } => write!(
                formatter,
                "scheduler event {event_id} at {event_time} precedes restored time {now}"
            ),
            Self::DuplicateEventId { event_id } => {
                write!(formatter, "duplicate scheduler event identifier {event_id}")
            }
            Self::EventsOutOfOrder {
                previous_event_id,
                previous_event_time,
                event_id,
                event_time,
            } => write!(
                formatter,
                "scheduler event {event_id} at {event_time} is out of order after \
                 {previous_event_id} at {previous_event_time}"
            ),
            Self::InvalidNextEventId {
                next_event_id,
                event_id,
            } => write!(
                formatter,
                "scheduler next event identifier {next_event_id} collides with {event_id}"
            ),
        }
    }
}

impl std::error::Error for SchedulerStateError {}

impl<E> Scheduler<E> {
    /// Captures all pending events without changing scheduler order.
    pub fn save_state(&self) -> SchedulerState<E>
    where
        E: Clone,
    {
        let mut events = Vec::with_capacity(self.len());
        if let Some(next) = &self.next {
            events.push(next.inner.clone());
        }
        events.extend(self.queue.iter().map(|event| event.inner.clone()));
        events.sort_by_key(|event| (event.time, event.id));
        SchedulerState {
            now: self.now,
            next_event_id: self.next_event_id,
            events,
        }
    }

    /// Restores a previously captured deterministic event queue.
    pub fn restore_state(&mut self, state: SchedulerState<E>) -> Result<(), SchedulerStateError> {
        let mut identifiers = BTreeSet::new();
        for event in &state.events {
            if event.time < state.now {
                return Err(SchedulerStateError::EventInPast {
                    now: state.now,
                    event_id: event.id,
                    event_time: event.time,
                });
            }
            if !identifiers.insert(event.id) {
                return Err(SchedulerStateError::DuplicateEventId { event_id: event.id });
            }
            if event.id.get() >= state.next_event_id {
                return Err(SchedulerStateError::InvalidNextEventId {
                    next_event_id: state.next_event_id,
                    event_id: event.id,
                });
            }
        }
        if let Some(events) = state
            .events
            .windows(2)
            .find(|events| (events[0].time, events[0].id) >= (events[1].time, events[1].id))
        {
            let previous = &events[0];
            let event = &events[1];
            return Err(SchedulerStateError::EventsOutOfOrder {
                previous_event_id: previous.id,
                previous_event_time: previous.time,
                event_id: event.id,
                event_time: event.time,
            });
        }

        let mut events = state.events.into_iter();
        self.now = state.now;
        self.next_event_id = state.next_event_id;
        self.next = events.next().map(|inner| QueuedEvent { inner });
        self.queue = events
            .map(|inner| QueuedEvent { inner })
            .collect::<BinaryHeap<_>>();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::component::ComponentId;

    use super::*;

    #[test]
    fn round_trip_preserves_time_ids_and_delivery_order() {
        let mut scheduler = Scheduler::new();
        scheduler
            .schedule_at(SimTime::new(10), ComponentId::new(1), 1_u8)
            .unwrap();
        scheduler
            .schedule_at(SimTime::new(5), ComponentId::new(2), 2_u8)
            .unwrap();
        scheduler
            .schedule_at(SimTime::new(5), ComponentId::new(3), 3_u8)
            .unwrap();
        scheduler.advance_to(SimTime::new(4)).unwrap();

        let state = scheduler.save_state();
        let mut restored = Scheduler::new();
        restored.restore_state(state).unwrap();

        assert_eq!(restored.now(), SimTime::new(4));
        assert_eq!(restored.pop_next().unwrap().payload, 2);
        assert_eq!(restored.pop_next().unwrap().payload, 3);
        assert_eq!(restored.pop_next().unwrap().payload, 1);
        assert_eq!(
            restored
                .schedule_at(SimTime::new(11), ComponentId::new(4), 4)
                .unwrap(),
            ScheduledEventId::new(3)
        );
    }

    #[test]
    fn restore_rejects_events_out_of_time_order_without_mutation() {
        let state = SchedulerState {
            now: SimTime::ZERO,
            next_event_id: 2,
            events: vec![
                ScheduledEvent {
                    id: ScheduledEventId::new(0),
                    time: SimTime::new(10),
                    target: ComponentId::new(1),
                    payload: 1_u8,
                },
                ScheduledEvent {
                    id: ScheduledEventId::new(1),
                    time: SimTime::new(5),
                    target: ComponentId::new(2),
                    payload: 2,
                },
            ],
        };
        let mut scheduler = Scheduler::new();
        scheduler
            .schedule_at(SimTime::new(3), ComponentId::new(3), 3_u8)
            .unwrap();
        scheduler.advance_to(SimTime::new(2)).unwrap();
        let original = scheduler.save_state();

        assert_eq!(
            scheduler.restore_state(state),
            Err(SchedulerStateError::EventsOutOfOrder {
                previous_event_id: ScheduledEventId::new(0),
                previous_event_time: SimTime::new(10),
                event_id: ScheduledEventId::new(1),
                event_time: SimTime::new(5),
            })
        );
        assert_eq!(scheduler.save_state(), original);
    }

    #[test]
    fn restore_rejects_equal_time_events_out_of_id_order_without_mutation() {
        let state = SchedulerState {
            now: SimTime::ZERO,
            next_event_id: 2,
            events: vec![
                ScheduledEvent {
                    id: ScheduledEventId::new(1),
                    time: SimTime::new(5),
                    target: ComponentId::new(1),
                    payload: 1_u8,
                },
                ScheduledEvent {
                    id: ScheduledEventId::new(0),
                    time: SimTime::new(5),
                    target: ComponentId::new(2),
                    payload: 2,
                },
            ],
        };
        let mut scheduler = Scheduler::new();
        scheduler
            .schedule_at(SimTime::new(3), ComponentId::new(3), 3_u8)
            .unwrap();
        scheduler.advance_to(SimTime::new(2)).unwrap();
        let original = scheduler.save_state();

        assert_eq!(
            scheduler.restore_state(state),
            Err(SchedulerStateError::EventsOutOfOrder {
                previous_event_id: ScheduledEventId::new(1),
                previous_event_time: SimTime::new(5),
                event_id: ScheduledEventId::new(0),
                event_time: SimTime::new(5),
            })
        );
        assert_eq!(scheduler.save_state(), original);
    }
}
