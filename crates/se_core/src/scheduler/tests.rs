use super::*;

use std::collections::BinaryHeap;

const TARGET: ComponentId = ComponentId::new(1);

#[test]
fn fractional_clock_projection_matches_iterated_cycles() {
    for (timebase_hz, frequency_hz, initial_remainder) in [
        (10_u64, 3_u64, 0_u64),
        (1_000_000_000, 66_666_667, 66_666_666),
        (4_000_000_000, 180_000_000, 17),
        (4_000_000_000, 90_000_000, 89_999_999),
    ] {
        let projection =
            FractionalClockProjection::new(timebase_hz, frequency_hz, initial_remainder);
        let mut remainder = initial_remainder;
        let mut elapsed = 0_u64;
        for cycles in 0..=1_024_u64 {
            assert_eq!(projection.elapsed(cycles), Some(SimDuration::new(elapsed)));
            assert_eq!(
                projection.cycles_until_elapsed_at_least(elapsed),
                Some(if elapsed == 0 { 0 } else { cycles }),
            );
            let numerator = timebase_hz + remainder;
            elapsed += numerator / frequency_hz;
            remainder = numerator % frequency_hz;
        }
        let mut advanced = projection;
        assert_eq!(advanced.advance(1_025), projection.elapsed(1_025),);
        assert_eq!(advanced.remainder(), remainder);
    }
}

#[test]
fn pops_events_by_time_then_insertion_order() {
    let mut scheduler = Scheduler::new();

    scheduler
        .schedule_at(SimTime::new(10), TARGET, "second")
        .unwrap();
    scheduler
        .schedule_at(SimTime::new(5), TARGET, "first")
        .unwrap();
    scheduler
        .schedule_at(SimTime::new(10), TARGET, "third")
        .unwrap();

    assert_eq!(scheduler.pop_next().unwrap().payload, "first");
    assert_eq!(scheduler.pop_next().unwrap().payload, "second");
    assert_eq!(scheduler.pop_next().unwrap().payload, "third");
    assert!(scheduler.is_empty());
}

#[test]
fn schedule_after_uses_current_time() {
    let mut scheduler = Scheduler::new();

    scheduler
        .schedule_after(SimDuration::new(4), TARGET, "first")
        .unwrap();
    assert_eq!(scheduler.pop_next().unwrap().time, SimTime::new(4));

    scheduler
        .schedule_after(SimDuration::new(3), TARGET, "second")
        .unwrap();
    assert_eq!(scheduler.pop_next().unwrap().time, SimTime::new(7));
}

#[test]
fn advance_to_updates_current_time() {
    let mut scheduler = Scheduler::<&str>::new();

    scheduler.advance_to(SimTime::new(11)).unwrap();
    assert_eq!(scheduler.now(), SimTime::new(11));

    scheduler.advance_to(SimTime::new(11)).unwrap();
    assert_eq!(scheduler.now(), SimTime::new(11));
}

#[test]
fn advance_to_rejects_past_time() {
    let mut scheduler = Scheduler::<&str>::new();

    scheduler.advance_to(SimTime::new(8)).unwrap();

    assert_eq!(
        scheduler.advance_to(SimTime::new(7)),
        Err(SchedulerError::EventInPast {
            now: SimTime::new(8),
            time: SimTime::new(7),
        })
    );
}

#[test]
fn schedule_after_uses_advanced_time() {
    let mut scheduler = Scheduler::new();

    scheduler.advance_to(SimTime::new(20)).unwrap();
    scheduler
        .schedule_after(SimDuration::new(5), TARGET, "after")
        .unwrap();

    assert_eq!(scheduler.pop_next().unwrap().time, SimTime::new(25));
}

#[test]
fn zero_delay_event_is_allowed() {
    let mut scheduler = Scheduler::new();

    scheduler
        .schedule_after(SimDuration::ZERO, TARGET, "zero")
        .unwrap();

    assert_eq!(scheduler.pop_next().unwrap().time, SimTime::ZERO);
}

#[test]
fn rejects_events_before_current_time() {
    let mut scheduler = Scheduler::new();

    scheduler
        .schedule_at(SimTime::new(8), TARGET, "advance")
        .unwrap();
    scheduler.pop_next().unwrap();

    assert_eq!(
        scheduler.schedule_at(SimTime::new(7), TARGET, "past"),
        Err(SchedulerError::EventInPast {
            now: SimTime::new(8),
            time: SimTime::new(7),
        })
    );
}

#[test]
fn reports_time_overflow() {
    let mut scheduler = Scheduler::new();

    scheduler
        .schedule_at(SimTime::new(u64::MAX), TARGET, "advance")
        .unwrap();
    scheduler.pop_next().unwrap();

    assert_eq!(
        scheduler.schedule_after(SimDuration::new(1), TARGET, "overflow"),
        Err(SchedulerError::TimeOverflow {
            time: SimTime::new(u64::MAX),
            duration: SimDuration::new(1),
        })
    );
}

#[test]
fn next_event_fast_lane_matches_binary_heap_reference() {
    let mut scheduler = Scheduler::new();
    let mut reference = BinaryHeap::new();
    let mut next_id = 0;
    let mut state = 0x9e37_79b9_7f4a_7c15_u64;

    for step in 0..10_000_u64 {
        state ^= state << 7;
        state ^= state >> 9;
        state ^= state << 8;
        if reference.is_empty() || state & 3 != 0 {
            let time = scheduler.now().get() + state % 64;
            let payload = step;
            let id = scheduler
                .schedule_at(SimTime::new(time), TARGET, payload)
                .unwrap();
            assert_eq!(id, ScheduledEventId::new(next_id));
            reference.push(QueuedEvent {
                inner: ScheduledEvent {
                    id,
                    time: SimTime::new(time),
                    target: TARGET,
                    payload,
                },
            });
            next_id += 1;
        } else {
            let expected = reference.pop().unwrap().inner;
            assert_eq!(scheduler.peek_next_time(), Some(expected.time));
            assert_eq!(scheduler.pop_next(), Some(expected));
        }
        assert_eq!(scheduler.len(), reference.len());
    }

    while let Some(expected) = reference.pop() {
        assert_eq!(scheduler.pop_next(), Some(expected.inner));
    }
    assert!(scheduler.is_empty());
}
