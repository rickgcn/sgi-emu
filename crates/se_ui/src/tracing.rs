//! Non-blocking transport from simulation tracing to the Qt user interface.

use std::{
    collections::VecDeque,
    sync::{
        Arc, LazyLock, Mutex, TryLockError,
        atomic::{AtomicU64, Ordering},
    },
};

use se_core::tracing::{TraceLevel, TraceRecord, TraceSink, TraceSource, TraceValue};

const APPLICATION_QUEUE_CAPACITY: usize = 8_192;

#[cxx::bridge(namespace = "se::ui")]
mod ffi {
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UiTraceLevel {
        Error,
        Warn,
        Info,
        Debug,
        Trace,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UiTraceSourceKind {
        Runtime,
        Scheduler,
        Component,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum UiTraceValueKind {
        Bool,
        U64,
        I64,
        Hex64,
        Str,
    }

    struct UiTraceField {
        key: String,
        kind: UiTraceValueKind,
        bool_value: bool,
        unsigned_value: u64,
        signed_value: i64,
        string_value: String,
    }

    struct UiTraceRecord {
        sequence: u64,
        time: u64,
        source_kind: UiTraceSourceKind,
        source_component: u64,
        level: UiTraceLevel,
        target: String,
        event: String,
        fields: Vec<UiTraceField>,
    }

    struct UiTraceStats {
        captured: u64,
        dropped: u64,
    }

    extern "Rust" {
        fn drain_trace_records(max_records: usize) -> Vec<UiTraceRecord>;
        fn clear_trace_records();
        fn trace_stats() -> UiTraceStats;
    }
}

struct TraceQueue {
    capacity: usize,
    records: Mutex<VecDeque<ffi::UiTraceRecord>>,
    captured: AtomicU64,
    dropped: AtomicU64,
}

impl TraceQueue {
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "trace queue capacity must be nonzero");
        Self {
            capacity,
            records: Mutex::new(VecDeque::with_capacity(capacity)),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
        }
    }

    fn drain(&self, max_records: usize) -> Vec<ffi::UiTraceRecord> {
        let mut records = self
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let count = max_records.min(records.len());
        records.drain(..count).collect()
    }

    fn clear(&self) {
        self.records
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    fn stats(&self) -> ffi::UiTraceStats {
        ffi::UiTraceStats {
            captured: self.captured.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }
}

static APPLICATION_TRACE_QUEUE: LazyLock<Arc<TraceQueue>> =
    LazyLock::new(|| Arc::new(TraceQueue::new(APPLICATION_QUEUE_CAPACITY)));

/// Trace sink used by the native application tracing window.
///
/// Recording never waits for the Qt thread. A record is dropped if the queue
/// is currently locked, and the oldest queued record is dropped if the queue
/// has reached its fixed capacity.
#[derive(Clone)]
pub struct UiTraceSink {
    queue: Arc<TraceQueue>,
}

impl UiTraceSink {
    /// Returns a sink connected to the application's tracing window.
    pub fn application() -> Self {
        Self {
            queue: Arc::clone(&APPLICATION_TRACE_QUEUE),
        }
    }

    #[cfg(test)]
    fn with_capacity(capacity: usize) -> Self {
        Self {
            queue: Arc::new(TraceQueue::new(capacity)),
        }
    }
}

impl TraceSink for UiTraceSink {
    fn record(&mut self, record: TraceRecord<'_>) {
        self.queue.captured.fetch_add(1, Ordering::Relaxed);
        let record = ffi::UiTraceRecord::from(record);

        match self.queue.records.try_lock() {
            Ok(mut records) => {
                if records.len() == self.queue.capacity {
                    records.pop_front();
                    self.queue.dropped.fetch_add(1, Ordering::Relaxed);
                }
                records.push_back(record);
            }
            Err(TryLockError::WouldBlock) => {
                self.queue.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TryLockError::Poisoned(error)) => {
                let mut records = error.into_inner();
                if records.len() == self.queue.capacity {
                    records.pop_front();
                    self.queue.dropped.fetch_add(1, Ordering::Relaxed);
                }
                records.push_back(record);
            }
        }
    }
}

impl From<TraceRecord<'_>> for ffi::UiTraceRecord {
    fn from(record: TraceRecord<'_>) -> Self {
        let (source_kind, source_component) = match record.source {
            TraceSource::Runtime => (ffi::UiTraceSourceKind::Runtime, 0),
            TraceSource::Scheduler => (ffi::UiTraceSourceKind::Scheduler, 0),
            TraceSource::Component(component) => {
                (ffi::UiTraceSourceKind::Component, component.get())
            }
        };

        Self {
            sequence: record.sequence,
            time: record.time.get(),
            source_kind,
            source_component,
            level: match record.level {
                TraceLevel::Error => ffi::UiTraceLevel::Error,
                TraceLevel::Warn => ffi::UiTraceLevel::Warn,
                TraceLevel::Info => ffi::UiTraceLevel::Info,
                TraceLevel::Debug => ffi::UiTraceLevel::Debug,
                TraceLevel::Trace => ffi::UiTraceLevel::Trace,
            },
            target: record.target.to_owned(),
            event: record.event.to_owned(),
            fields: record
                .fields
                .iter()
                .map(|field| ffi::UiTraceField::from(field.value).with_key(field.key))
                .collect(),
        }
    }
}

impl From<TraceValue<'_>> for ffi::UiTraceField {
    fn from(value: TraceValue<'_>) -> Self {
        let mut field = Self {
            key: String::new(),
            kind: ffi::UiTraceValueKind::Bool,
            bool_value: false,
            unsigned_value: 0,
            signed_value: 0,
            string_value: String::new(),
        };

        match value {
            TraceValue::Bool(value) => field.bool_value = value,
            TraceValue::U64(value) => {
                field.kind = ffi::UiTraceValueKind::U64;
                field.unsigned_value = value;
            }
            TraceValue::I64(value) => {
                field.kind = ffi::UiTraceValueKind::I64;
                field.signed_value = value;
            }
            TraceValue::Hex64(value) => {
                field.kind = ffi::UiTraceValueKind::Hex64;
                field.unsigned_value = value;
            }
            TraceValue::Str(value) => {
                field.kind = ffi::UiTraceValueKind::Str;
                field.string_value = value.to_owned();
            }
        }
        field
    }
}

impl ffi::UiTraceField {
    fn with_key(mut self, key: &str) -> Self {
        self.key = key.to_owned();
        self
    }
}

fn drain_trace_records(max_records: usize) -> Vec<ffi::UiTraceRecord> {
    APPLICATION_TRACE_QUEUE.drain(max_records)
}

fn clear_trace_records() {
    APPLICATION_TRACE_QUEUE.clear();
}

fn trace_stats() -> ffi::UiTraceStats {
    APPLICATION_TRACE_QUEUE.stats()
}

#[cfg(test)]
mod tests {
    use se_core::{
        component::ComponentId,
        scheduler::SimTime,
        tracing::{TraceField, TraceLevel, TraceRecord, TraceSink, TraceSource},
    };

    use super::{UiTraceSink, ffi};

    fn record<'a>(fields: &'a [TraceField<'a>]) -> TraceRecord<'a> {
        TraceRecord {
            sequence: 42,
            time: SimTime::new(99),
            source: TraceSource::Component(ComponentId::new(7)),
            level: TraceLevel::Debug,
            target: "ip32.sysad",
            event: "access",
            fields,
        }
    }

    #[test]
    fn sink_preserves_record_and_all_field_types() {
        let fields = [
            TraceField::bool("enabled", true),
            TraceField::u64("width", 8),
            TraceField::i64("offset", -4),
            TraceField::hex64("address", 0x1fc0_0000),
            TraceField::string("operation", "read"),
        ];
        let mut sink = UiTraceSink::with_capacity(8);
        sink.record(record(&fields));

        let records = sink.queue.drain(8);
        assert_eq!(records.len(), 1);
        let captured = &records[0];
        assert_eq!(captured.sequence, 42);
        assert_eq!(captured.time, 99);
        assert_eq!(captured.source_kind, ffi::UiTraceSourceKind::Component);
        assert_eq!(captured.source_component, 7);
        assert_eq!(captured.level, ffi::UiTraceLevel::Debug);
        assert_eq!(captured.target, "ip32.sysad");
        assert_eq!(captured.event, "access");
        assert_eq!(captured.fields[0].kind, ffi::UiTraceValueKind::Bool);
        assert!(captured.fields[0].bool_value);
        assert_eq!(captured.fields[1].kind, ffi::UiTraceValueKind::U64);
        assert_eq!(captured.fields[1].unsigned_value, 8);
        assert_eq!(captured.fields[2].kind, ffi::UiTraceValueKind::I64);
        assert_eq!(captured.fields[2].signed_value, -4);
        assert_eq!(captured.fields[3].kind, ffi::UiTraceValueKind::Hex64);
        assert_eq!(captured.fields[3].unsigned_value, 0x1fc0_0000);
        assert_eq!(captured.fields[4].kind, ffi::UiTraceValueKind::Str);
        assert_eq!(captured.fields[4].string_value, "read");
    }

    #[test]
    fn queue_keeps_newest_records_and_counts_overflow() {
        let mut sink = UiTraceSink::with_capacity(2);
        for sequence in 0..3 {
            let mut trace = record(&[]);
            trace.sequence = sequence;
            sink.record(trace);
        }

        let records = sink.queue.drain(8);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sequence, 1);
        assert_eq!(records[1].sequence, 2);
        assert_eq!(sink.queue.stats().captured, 3);
        assert_eq!(sink.queue.stats().dropped, 1);
    }

    #[test]
    fn drain_limits_batch_size_and_clear_preserves_statistics() {
        let mut sink = UiTraceSink::with_capacity(8);
        sink.record(record(&[]));
        sink.record(record(&[]));

        assert_eq!(sink.queue.drain(1).len(), 1);
        sink.queue.clear();
        assert!(sink.queue.drain(8).is_empty());
        assert_eq!(sink.queue.stats().captured, 2);
        assert_eq!(sink.queue.stats().dropped, 0);
    }
}
