//! Non-blocking transport from simulation tracing to the Qt user interface.

use std::{
    collections::VecDeque,
    sync::{
        Arc, LazyLock, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use se_core::tracing::{
    TraceInterest, TraceLevel, TraceRecord, TraceSink, TraceSource, TraceValue,
};

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
        session: u64,
        captured: u64,
        dropped: u64,
    }

    extern "Rust" {
        fn drain_trace_records(max_records: usize) -> Vec<UiTraceRecord>;
        fn clear_trace_records();
        fn trace_stats() -> UiTraceStats;
        fn set_trace_capture_enabled(enabled: bool);
        fn set_scheduler_trace_capture_enabled(enabled: bool);
    }
}

struct TraceQueue {
    capacity: usize,
    records: Mutex<VecDeque<ffi::UiTraceRecord>>,
    session: AtomicU64,
    captured: AtomicU64,
    dropped: AtomicU64,
    capture_enabled: AtomicBool,
    scheduler_capture_enabled: AtomicBool,
}

impl TraceQueue {
    fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "trace queue capacity must be nonzero");
        Self {
            capacity,
            records: Mutex::new(VecDeque::with_capacity(capacity)),
            session: AtomicU64::new(0),
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            capture_enabled: AtomicBool::new(true),
            scheduler_capture_enabled: AtomicBool::new(false),
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
            session: self.session.load(Ordering::Acquire),
            captured: self.captured.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
        }
    }

    fn begin_session(&self) -> u64 {
        self.clear();
        self.captured.store(0, Ordering::Relaxed);
        self.dropped.store(0, Ordering::Relaxed);
        self.session.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn enabled(&self, source: TraceSource) -> bool {
        self.capture_enabled.load(Ordering::Relaxed)
            && (!matches!(source, TraceSource::Scheduler)
                || self.scheduler_capture_enabled.load(Ordering::Relaxed))
    }

    fn enqueue(&self, records: &mut VecDeque<ffi::UiTraceRecord>, record: TraceRecord<'_>) {
        if records.len() == self.capacity {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
        records.push_back(ffi::UiTraceRecord::from(record));
    }
}

static APPLICATION_TRACE_QUEUE: LazyLock<Arc<TraceQueue>> =
    LazyLock::new(|| Arc::new(TraceQueue::new(APPLICATION_QUEUE_CAPACITY)));

/// Trace sink used by the native application tracing window.
///
/// Recording never waits for the Qt thread. A record is dropped if the queue
/// is currently locked or has reached its fixed capacity. Capacity is checked
/// before the borrowed record is converted into owned UI data.
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
    fn interest(&self, source: TraceSource) -> TraceInterest {
        if self.queue.enabled(source) {
            TraceInterest::All
        } else {
            TraceInterest::None
        }
    }

    fn enabled(
        &self,
        source: TraceSource,
        _level: TraceLevel,
        _target: &str,
        _event: &str,
    ) -> bool {
        self.queue.enabled(source)
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        if !self.queue.enabled(record.source) {
            return;
        }
        self.queue.captured.fetch_add(1, Ordering::Relaxed);

        match self.queue.records.try_lock() {
            Ok(mut records) => {
                self.queue.enqueue(&mut records, record);
            }
            Err(TryLockError::WouldBlock) => {
                self.queue.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TryLockError::Poisoned(error)) => {
                let mut records = error.into_inner();
                self.queue.enqueue(&mut records, record);
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

fn set_trace_capture_enabled(enabled: bool) {
    APPLICATION_TRACE_QUEUE
        .capture_enabled
        .store(enabled, Ordering::Relaxed);
}

fn set_scheduler_trace_capture_enabled(enabled: bool) {
    APPLICATION_TRACE_QUEUE
        .scheduler_capture_enabled
        .store(enabled, Ordering::Relaxed);
}

pub(crate) fn begin_application_trace_session() -> u64 {
    APPLICATION_TRACE_QUEUE.begin_session()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use se_core::{
        component::ComponentId,
        scheduler::SimTime,
        tracing::{TraceField, TraceInterest, TraceLevel, TraceRecord, TraceSink, TraceSource},
    };
    use se_machine::o2::ip32::machine::{Ip32Machine, Ip32MachineConfig};

    use super::{Ordering, UiTraceSink, ffi};

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
    fn queue_drops_new_records_and_counts_overflow() {
        let mut sink = UiTraceSink::with_capacity(2);
        for sequence in 0..3 {
            let mut trace = record(&[]);
            trace.sequence = sequence;
            sink.record(trace);
        }

        let records = sink.queue.drain(8);
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].sequence, 0);
        assert_eq!(records[1].sequence, 1);
        assert_eq!(sink.queue.stats().captured, 3);
        assert_eq!(sink.queue.stats().dropped, 1);
    }

    #[test]
    fn lock_contention_drops_without_modifying_the_queue() {
        let mut sink = UiTraceSink::with_capacity(2);
        let queue = sink.queue.clone();
        let guard = queue
            .records
            .lock()
            .unwrap_or_else(|error| error.into_inner());

        sink.record(record(&[]));

        assert!(guard.is_empty());
        assert_eq!(sink.queue.stats().captured, 1);
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

    #[test]
    fn capture_gates_filter_before_counting_records() {
        let mut sink = UiTraceSink::with_capacity(8);
        let scheduler_record = TraceRecord {
            source: TraceSource::Scheduler,
            ..record(&[])
        };

        assert!(!sink.enabled(
            TraceSource::Scheduler,
            TraceLevel::Trace,
            "scheduler",
            "event_dispatched"
        ));
        assert_eq!(sink.interest(TraceSource::Scheduler), TraceInterest::None);
        assert!(sink.enabled(
            TraceSource::Component(ComponentId::new(7)),
            TraceLevel::Debug,
            "ip32.sysad",
            "access"
        ));
        assert_eq!(
            sink.interest(TraceSource::Component(ComponentId::new(7))),
            TraceInterest::All
        );
        sink.record(scheduler_record);
        assert!(sink.queue.drain(8).is_empty());
        assert_eq!(sink.queue.stats().captured, 0);

        sink.queue
            .scheduler_capture_enabled
            .store(true, Ordering::Relaxed);
        assert!(sink.enabled(
            TraceSource::Scheduler,
            TraceLevel::Trace,
            "scheduler",
            "event_dispatched"
        ));
        sink.record(scheduler_record);
        assert_eq!(sink.queue.drain(8).len(), 1);
        assert_eq!(sink.queue.stats().captured, 1);

        sink.queue.capture_enabled.store(false, Ordering::Relaxed);
        assert_eq!(
            sink.interest(TraceSource::Component(ComponentId::new(7))),
            TraceInterest::None
        );
        assert!(!sink.enabled(
            TraceSource::Component(ComponentId::new(7)),
            TraceLevel::Debug,
            "ip32.sysad",
            "access"
        ));
        sink.record(record(&[]));
        assert!(sink.queue.drain(8).is_empty());
        assert_eq!(sink.queue.stats().captured, 1);
        assert_eq!(sink.queue.stats().dropped, 0);
    }

    #[test]
    fn beginning_a_session_clears_records_and_statistics() {
        let mut sink = UiTraceSink::with_capacity(8);
        sink.record(record(&[]));

        assert_eq!(sink.queue.begin_session(), 1);
        assert!(sink.queue.drain(8).is_empty());
        assert_eq!(sink.queue.stats().session, 1);
        assert_eq!(sink.queue.stats().captured, 0);
        assert_eq!(sink.queue.stats().dropped, 0);
    }

    #[test]
    #[ignore = "requires a local proprietary IP32 PROM image"]
    fn local_ip32_prom_trace_throughput_probe() {
        let path = std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name a local image");
        let prom = std::fs::read(path).expect("the local PROM image must be readable");
        let max_events = std::env::var("IP32_PROM_EVENTS")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(2_000_000);
        let requested_mode =
            std::env::var("IP32_PROM_TRACE_MODE").unwrap_or_else(|_| "all".to_owned());

        for (label, capture, scheduler_capture) in [
            ("capture-disabled", false, false),
            ("component-capture", true, false),
            ("scheduler-capture", true, true),
        ]
        .into_iter()
        .filter(|(label, _, _)| requested_mode == "all" || requested_mode == *label)
        {
            let sink = UiTraceSink::with_capacity(super::APPLICATION_QUEUE_CAPACITY);
            sink.queue.capture_enabled.store(capture, Ordering::Relaxed);
            sink.queue
                .scheduler_capture_enabled
                .store(scheduler_capture, Ordering::Relaxed);
            let queue = sink.queue.clone();
            let config = Ip32MachineConfig {
                prom_image: prom.clone(),
                ..Ip32MachineConfig::default()
            };
            let mut machine = Ip32Machine::from_config_with_trace_sink(config, sink)
                .expect("the local IP32 machine must build");
            machine
                .schedule_power_on()
                .expect("power-on must be scheduled");

            let started = Instant::now();
            machine
                .run_steps(max_events)
                .expect("the local PROM run must not fail");
            let elapsed = started.elapsed();
            let performance = machine.performance_snapshot();
            let elapsed_seconds = elapsed.as_secs_f64();
            let simulated_seconds = performance.sim_time.get() as f64 / 1_000_000_000.0;
            let retired = performance.cpu.retired_instructions;
            let dispatched = performance.runtime.dispatched_events;
            let stats = queue.stats();
            let queued = queue
                .records
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .len();
            eprintln!(
                "{label}: events={max_events}, elapsed={elapsed:?}, simulated={simulated_seconds:.6}s, rtf={:.3}, instructions/s={:.0}, events/s={:.0}, events/instruction={:.3}, sysad={}, memory={}, cmi={}, cgi={}, captured={}, dropped={}, queued={queued}",
                simulated_seconds / elapsed_seconds,
                retired as f64 / elapsed_seconds,
                dispatched as f64 / elapsed_seconds,
                dispatched as f64 / retired.max(1) as f64,
                performance.sysad_transactions,
                performance.memory_transactions,
                performance.cmi_transactions,
                performance.cgi_transactions,
                stats.captured,
                stats.dropped
            );
        }
    }
}
