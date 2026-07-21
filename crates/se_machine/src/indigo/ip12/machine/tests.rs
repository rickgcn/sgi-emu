use super::*;

#[test]
fn new_starts_at_zero() {
    let machine = Ip12Machine::new();

    assert_eq!(machine.runtime().now(), SimTime::ZERO);
}

#[test]
fn schedule_power_on_queues_zero_time_event() {
    let mut machine = Ip12Machine::new();

    machine.schedule_power_on().unwrap();
    let event = machine.runtime.scheduler_mut().pop_next().unwrap();

    assert_eq!(event.time, SimTime::ZERO);
    assert_eq!(event.target, component_ids::MACHINE);
    assert_eq!(event.payload, Ip12Event::PowerOn);
}

#[test]
fn run_until_time_consumes_power_on_and_reset_events() {
    let mut machine = Ip12Machine::new();

    machine.schedule_power_on().unwrap();
    machine.schedule_reset().unwrap();

    let status = machine.run_until_time(SimTime::ZERO).unwrap();

    assert_eq!(status, RunStatus::Idle);
    assert_eq!(machine.runtime().now(), SimTime::ZERO);
    assert!(machine.runtime().scheduler().is_empty());
}

#[test]
fn run_until_time_advances_when_queue_is_empty() {
    let mut machine = Ip12Machine::new();

    let status = machine.run_until_time(SimTime::new(42)).unwrap();

    assert_eq!(status, RunStatus::Idle);
    assert_eq!(machine.runtime().now(), SimTime::new(42));
}
