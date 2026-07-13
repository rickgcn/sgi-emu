use super::*;
use crate::component::ComponentId;
use crate::scheduler::SimTime;
use core::cell::Cell;

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
                    value: CapturedValue::from(field.value),
                })
                .collect(),
        });
    }
}

impl From<TraceValue<'_>> for CapturedValue {
    fn from(value: TraceValue<'_>) -> Self {
        match value {
            TraceValue::Bool(value) => Self::Bool(value),
            TraceValue::U64(value) => Self::U64(value),
            TraceValue::I64(value) => Self::I64(value),
            TraceValue::Hex64(value) => Self::Hex64(value),
            TraceValue::Str(value) => Self::Str(value.to_owned()),
        }
    }
}

#[test]
fn recorder_assigns_monotonic_sequences() {
    let mut recorder = TraceRecorder::new(CaptureSink::default());

    let first = recorder.record(
        SimTime::new(3),
        TraceSource::Scheduler,
        TraceLevel::Trace,
        "scheduler",
        "event_scheduled",
        &[],
    );
    let second = recorder.record(
        SimTime::new(3),
        TraceSource::Scheduler,
        TraceLevel::Trace,
        "scheduler",
        "event_dispatched",
        &[],
    );

    assert_eq!(first, 0);
    assert_eq!(second, 1);
    assert_eq!(recorder.next_sequence(), 2);

    let sink = recorder.into_sink();
    assert_eq!(sink.records[0].sequence, 0);
    assert_eq!(sink.records[1].sequence, 1);
}

#[test]
fn recorder_preserves_structured_record_data() {
    let mut recorder = TraceRecorder::new(CaptureSink::default());
    let fields = [
        TraceField::bool("cached", true),
        TraceField::u64("width", 4),
        TraceField::i64("offset", -8),
        TraceField::hex64("address", 0x1fc0_0000),
        TraceField::string("phase", "route"),
    ];

    recorder.record(
        SimTime::new(12),
        TraceSource::Component(ComponentId::new(7)),
        TraceLevel::Debug,
        "main_bus",
        "transaction_routed",
        &fields,
    );

    assert_eq!(
        recorder.into_sink().records,
        vec![CapturedRecord {
            sequence: 0,
            time: SimTime::new(12),
            source: TraceSource::Component(ComponentId::new(7)),
            level: TraceLevel::Debug,
            target: "main_bus".to_owned(),
            event: "transaction_routed".to_owned(),
            fields: vec![
                CapturedField {
                    key: "cached".to_owned(),
                    value: CapturedValue::Bool(true),
                },
                CapturedField {
                    key: "width".to_owned(),
                    value: CapturedValue::U64(4),
                },
                CapturedField {
                    key: "offset".to_owned(),
                    value: CapturedValue::I64(-8),
                },
                CapturedField {
                    key: "address".to_owned(),
                    value: CapturedValue::Hex64(0x1fc0_0000),
                },
                CapturedField {
                    key: "phase".to_owned(),
                    value: CapturedValue::Str("route".to_owned()),
                },
            ],
        }]
    );
}

#[test]
fn field_constructors_build_expected_fields() {
    assert_eq!(
        TraceField::hex64("pc", 0x8000_0000),
        TraceField {
            key: "pc",
            value: TraceValue::Hex64(0x8000_0000),
        }
    );
    assert_eq!(
        TraceField::string("state", "reset"),
        TraceField {
            key: "state",
            value: TraceValue::Str("reset"),
        }
    );
}

#[test]
fn noop_recorder_discards_records_but_keeps_sequence() {
    let mut recorder = TraceRecorder::noop();

    assert_eq!(
        recorder.record(
            SimTime::ZERO,
            TraceSource::Runtime,
            TraceLevel::Info,
            "runtime",
            "created",
            &[],
        ),
        0
    );
    assert_eq!(recorder.next_sequence(), 1);
}

#[derive(Default)]
struct SchedulerDisabledSink {
    records: Vec<(u64, String)>,
}

impl TraceSink for SchedulerDisabledSink {
    fn enabled(
        &self,
        source: TraceSource,
        _level: TraceLevel,
        _target: &str,
        _event: &str,
    ) -> bool {
        !matches!(source, TraceSource::Scheduler)
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        self.records
            .push((record.sequence, record.event.to_owned()));
    }
}

#[derive(Default)]
struct SchedulerUninterestedSink {
    records: Vec<(u64, String)>,
}

impl TraceSink for SchedulerUninterestedSink {
    fn interest(&self, source: TraceSource) -> TraceInterest {
        if matches!(source, TraceSource::Scheduler) {
            TraceInterest::None
        } else {
            TraceInterest::All
        }
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        self.records
            .push((record.sequence, record.event.to_owned()));
    }
}

#[test]
fn lazy_recording_skips_field_construction_and_preserves_sequence_space() {
    let mut recorder = TraceRecorder::new(SchedulerDisabledSink::default());
    let fields_built = Cell::new(false);

    let disabled = recorder.record_lazy(
        SimTime::new(3),
        TraceSource::Scheduler,
        TraceLevel::Trace,
        "scheduler",
        "event_dispatched",
        || {
            fields_built.set(true);
            [TraceField::u64("event_id", 1)]
        },
    );
    let enabled = recorder.record_lazy(
        SimTime::new(4),
        TraceSource::Runtime,
        TraceLevel::Info,
        "runtime",
        "continued",
        || [TraceField::bool("running", true)],
    );

    assert_eq!(disabled, Some(0));
    assert_eq!(enabled, Some(1));
    assert!(!fields_built.get());
    assert_eq!(recorder.next_sequence(), 2);
    assert_eq!(recorder.sink().records, vec![(1, "continued".to_owned())]);
}

#[test]
fn uninterested_source_skips_sequence_and_field_construction() {
    let mut recorder = TraceRecorder::new(SchedulerUninterestedSink::default());
    let fields_built = Cell::new(false);

    let disabled = recorder.record_lazy(
        SimTime::new(3),
        TraceSource::Scheduler,
        TraceLevel::Trace,
        "scheduler",
        "event_dispatched",
        || {
            fields_built.set(true);
            [TraceField::u64("event_id", 1)]
        },
    );
    let enabled = recorder.record_lazy(
        SimTime::new(4),
        TraceSource::Runtime,
        TraceLevel::Info,
        "runtime",
        "continued",
        || [TraceField::bool("running", true)],
    );

    assert_eq!(disabled, None);
    assert_eq!(enabled, Some(0));
    assert!(!fields_built.get());
    assert_eq!(recorder.next_sequence(), 1);
    assert_eq!(recorder.sink().records, vec![(0, "continued".to_owned())]);
}
