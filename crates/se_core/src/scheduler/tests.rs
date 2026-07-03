use super::*;

const TARGET: ComponentId = ComponentId::new(1);

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
