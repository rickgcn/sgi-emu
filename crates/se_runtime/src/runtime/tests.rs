use core::convert::Infallible;

use super::*;
use se_core::tracing::{TraceRecord, TraceValue};

const TARGET: ComponentId = ComponentId::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
enum TestEvent {
    Record(&'static str),
    ScheduleAfter {
        delay: SimDuration,
        label: &'static str,
    },
    Stop,
}

#[derive(Debug, Eq, PartialEq)]
struct CapturedRecord {
    sequence: u64,
    time: SimTime,
    source: TraceSource,
    level: TraceLevel,
    target: String,
    event: String,
    fields: Vec<CapturedField>,
}

#[derive(Debug, Eq, PartialEq)]
struct CapturedField {
    key: String,
    value: CapturedValue,
}

#[derive(Debug, Eq, PartialEq)]
enum CapturedValue {
    Bool(bool),
    U64(u64),
    I64(i64),
    Hex64(u64),
    Str(String),
}

#[derive(Default)]
struct CaptureSink {
    records: Vec<CapturedRecord>,
}

impl TraceSink for CaptureSink {
    fn record(&mut self, record: TraceRecord<'_>) {
        self.records.push(CapturedRecord {
            sequence: record.sequence,
            time: record.time,
            source: record.source,
            level: record.level,
            target: record.target.to_owned(),
            event: record.event.to_owned(),
            fields: record
                .fields
                .iter()
                .map(|field| CapturedField {
                    key: field.key.to_owned(),
                    value: match field.value {
                        TraceValue::Bool(value) => CapturedValue::Bool(value),
                        TraceValue::U64(value) => CapturedValue::U64(value),
                        TraceValue::I64(value) => CapturedValue::I64(value),
                        TraceValue::Hex64(value) => CapturedValue::Hex64(value),
                        TraceValue::Str(value) => CapturedValue::Str(value.to_owned()),
                    },
                })
                .collect(),
        });
    }
}

#[test]
fn dispatch_next_dispatches_one_event() {
    let mut runtime = Runtime::new();
    let mut seen = Vec::new();

    runtime
        .schedule_at(SimTime::new(2), TARGET, TestEvent::Record("first"))
        .unwrap();
    runtime
        .schedule_at(SimTime::new(3), TARGET, TestEvent::Record("second"))
        .unwrap();

    let status = runtime
        .dispatch_next(|event, _registry, _context| {
            if let TestEvent::Record(label) = event.payload {
                seen.push(label);
            }
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(status, RunStatus::Dispatched);
    assert_eq!(seen, vec!["first"]);
    assert_eq!(runtime.now(), SimTime::new(2));
    assert_eq!(runtime.scheduler().len(), 1);
}

#[test]
fn run_steps_stops_at_event_limit() {
    let mut runtime = Runtime::new();
    let mut seen = Vec::new();

    runtime
        .schedule_at(SimTime::new(1), TARGET, TestEvent::Record("first"))
        .unwrap();
    runtime
        .schedule_at(SimTime::new(2), TARGET, TestEvent::Record("second"))
        .unwrap();

    let status = runtime
        .run_steps(1, |event, _registry, _context| {
            if let TestEvent::Record(label) = event.payload {
                seen.push(label);
            }
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(status, RunStatus::StepLimitReached);
    assert_eq!(seen, vec!["first"]);
    assert_eq!(runtime.scheduler().len(), 1);
}

#[test]
fn run_until_time_keeps_future_events() {
    let mut runtime = Runtime::new();
    let mut seen = Vec::new();

    runtime
        .schedule_at(SimTime::new(5), TARGET, TestEvent::Record("first"))
        .unwrap();
    runtime
        .schedule_at(SimTime::new(15), TARGET, TestEvent::Record("future"))
        .unwrap();

    let status = runtime
        .run_until_time(SimTime::new(10), |event, _registry, _context| {
            if let TestEvent::Record(label) = event.payload {
                seen.push(label);
            }
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(status, RunStatus::DeadlineReached);
    assert_eq!(seen, vec!["first"]);
    assert_eq!(runtime.now(), SimTime::new(10));
    assert_eq!(runtime.scheduler().peek_next_time(), Some(SimTime::new(15)));
}

#[test]
fn run_until_time_advances_when_queue_is_empty() {
    let mut runtime = Runtime::<TestEvent>::new();

    let status = runtime
        .run_until_time(SimTime::new(99), |_event, _registry, _context| {
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(status, RunStatus::Idle);
    assert_eq!(runtime.now(), SimTime::new(99));
}

#[test]
fn run_until_time_rejects_past_deadline() {
    let mut runtime = Runtime::<TestEvent>::new();

    runtime
        .run_until_time(SimTime::new(9), |_event, _registry, _context| {
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(
        runtime.run_until_time(SimTime::new(8), |_event, _registry, _context| {
            Ok::<(), Infallible>(())
        }),
        Err(RunError::Scheduler(SchedulerError::EventInPast {
            now: SimTime::new(9),
            time: SimTime::new(8),
        }))
    );
}

#[test]
fn schedule_after_from_context_is_immediately_visible() {
    let mut runtime = Runtime::new();
    let mut seen = Vec::new();

    runtime
        .schedule_at(
            SimTime::new(4),
            TARGET,
            TestEvent::ScheduleAfter {
                delay: SimDuration::new(3),
                label: "scheduled",
            },
        )
        .unwrap();

    let status = runtime
        .run_until_time(SimTime::new(7), |event, _registry, context| {
            match event.payload {
                TestEvent::ScheduleAfter { delay, label } => {
                    context.schedule_after(delay, TARGET, TestEvent::Record(label))?;
                }
                TestEvent::Record(label) => seen.push(label),
                TestEvent::Stop => context.request_stop(),
            }
            Ok::<(), SchedulerError>(())
        })
        .unwrap();

    assert_eq!(status, RunStatus::Idle);
    assert_eq!(seen, vec!["scheduled"]);
    assert_eq!(runtime.now(), SimTime::new(7));
}

#[test]
fn time_horizon_uses_deadline_or_next_event_time() {
    let mut runtime = Runtime::new();
    let mut horizons = Vec::new();

    runtime
        .schedule_at(SimTime::new(2), TARGET, TestEvent::Record("current"))
        .unwrap();
    runtime
        .schedule_at(SimTime::new(8), TARGET, TestEvent::Record("next"))
        .unwrap();

    runtime
        .run_until_time(SimTime::new(10), |_event, _registry, context| {
            horizons.push(context.time_horizon());
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(horizons[0], SimTime::new(8));

    let mut runtime = Runtime::new();
    let mut horizons = Vec::new();
    runtime
        .schedule_at(SimTime::new(2), TARGET, TestEvent::Record("current"))
        .unwrap();

    runtime
        .run_until_time(SimTime::new(10), |_event, _registry, context| {
            horizons.push(context.time_horizon());
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(horizons, vec![SimTime::new(10)]);
}

#[test]
fn request_stop_stops_run_loop() {
    let mut runtime = Runtime::new();
    let mut seen = Vec::new();

    runtime
        .schedule_at(SimTime::new(1), TARGET, TestEvent::Stop)
        .unwrap();
    runtime
        .schedule_at(SimTime::new(2), TARGET, TestEvent::Record("after_stop"))
        .unwrap();

    let status = runtime
        .run_until_time(SimTime::new(10), |event, _registry, context| {
            match event.payload {
                TestEvent::Stop => context.request_stop(),
                TestEvent::Record(label) => seen.push(label),
                TestEvent::ScheduleAfter { .. } => {}
            }
            Ok::<(), Infallible>(())
        })
        .unwrap();

    assert_eq!(status, RunStatus::Stopped);
    assert!(runtime.is_stopped());
    assert!(seen.is_empty());
    assert_eq!(runtime.scheduler().peek_next_time(), Some(SimTime::new(2)));
}

#[test]
fn scheduler_tracing_records_schedule_and_dispatch() {
    let mut runtime = Runtime::<TestEvent, CaptureSink>::with_trace_sink(CaptureSink::default());

    runtime
        .schedule_at(SimTime::new(6), TARGET, TestEvent::Record("trace"))
        .unwrap();
    runtime
        .dispatch_next(|_event, _registry, _context| Ok::<(), Infallible>(()))
        .unwrap();

    let records = &runtime.trace_recorder().sink().records;
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].sequence, 0);
    assert_eq!(records[0].target, "scheduler");
    assert_eq!(records[0].event, "event_scheduled");
    assert_eq!(records[1].sequence, 1);
    assert_eq!(records[1].target, "scheduler");
    assert_eq!(records[1].event, "event_dispatched");
}
