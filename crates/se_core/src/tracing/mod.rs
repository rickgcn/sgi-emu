//! Deterministic simulation tracing.
//!
//! The tracing module records structured simulation facts. It does not format,
//! filter, aggregate, or interpret records. Higher layers may decide how to
//! display, store, search, or analyze the collected records.
//!
//! Trace records are data, not callbacks. Emitting a trace record must not
//! advance simulated time, dispatch events, or affect component behavior.

use crate::component::ComponentId;
use crate::scheduler::SimTime;

/// Importance level of a trace record.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TraceLevel {
    /// An error condition was observed.
    Error,

    /// A suspicious condition was observed, but simulation may continue.
    Warn,

    /// A notable lifecycle or high-level state event was observed.
    Info,

    /// A detailed bringup event was observed.
    Debug,

    /// A high-frequency low-level event was observed.
    Trace,
}

/// Source that emitted a trace record.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TraceSource {
    /// Runtime or machine-level code emitted the record.
    Runtime,

    /// The scheduler emitted the record.
    Scheduler,

    /// A component emitted the record.
    Component(ComponentId),
}

/// Structured value stored in a trace field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceValue<'a> {
    /// Boolean value.
    Bool(bool),

    /// Unsigned integer value.
    U64(u64),

    /// Signed integer value.
    I64(i64),

    /// Unsigned integer value that should usually be displayed as hexadecimal.
    Hex64(u64),

    /// Borrowed string value.
    Str(&'a str),
}

/// Single ordered key-value fact in a trace record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceField<'a> {
    /// Stable field name.
    pub key: &'a str,

    /// Field value.
    pub value: TraceValue<'a>,
}

impl<'a> TraceField<'a> {
    /// Creates a boolean trace field.
    pub const fn bool(key: &'a str, value: bool) -> Self {
        Self {
            key,
            value: TraceValue::Bool(value),
        }
    }

    /// Creates an unsigned integer trace field.
    pub const fn u64(key: &'a str, value: u64) -> Self {
        Self {
            key,
            value: TraceValue::U64(value),
        }
    }

    /// Creates a signed integer trace field.
    pub const fn i64(key: &'a str, value: i64) -> Self {
        Self {
            key,
            value: TraceValue::I64(value),
        }
    }

    /// Creates a hexadecimal display hint trace field.
    pub const fn hex64(key: &'a str, value: u64) -> Self {
        Self {
            key,
            value: TraceValue::Hex64(value),
        }
    }

    /// Creates a borrowed string trace field.
    pub const fn string(key: &'a str, value: &'a str) -> Self {
        Self {
            key,
            value: TraceValue::Str(value),
        }
    }
}

/// Complete structured trace record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TraceRecord<'a> {
    /// Global trace sequence number.
    pub sequence: u64,

    /// Simulated time when the fact was recorded.
    pub time: SimTime,

    /// Source that emitted the record.
    pub source: TraceSource,

    /// Importance level.
    pub level: TraceLevel,

    /// Stable channel or subsystem name.
    pub target: &'a str,

    /// Stable event name.
    pub event: &'a str,

    /// Ordered structured fields.
    pub fields: &'a [TraceField<'a>],
}

/// Destination for trace records.
pub trait TraceSink {
    /// Records one complete trace record.
    fn record(&mut self, record: TraceRecord<'_>);
}

/// Trace sink that discards all records.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn record(&mut self, _record: TraceRecord<'_>) {}
}

/// Trace recorder that assigns deterministic sequence numbers.
pub struct TraceRecorder<S> {
    next_sequence: u64,
    sink: S,
}

impl<S> TraceRecorder<S> {
    /// Creates a recorder that writes to the given sink.
    pub const fn new(sink: S) -> Self {
        Self {
            next_sequence: 0,
            sink,
        }
    }

    /// Creates a recorder with an explicit next sequence number.
    pub const fn with_sequence(sink: S, next_sequence: u64) -> Self {
        Self {
            next_sequence,
            sink,
        }
    }

    /// Returns the sequence number that will be assigned to the next record.
    pub const fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// Returns an immutable reference to the underlying sink.
    pub const fn sink(&self) -> &S {
        &self.sink
    }

    /// Returns a mutable reference to the underlying sink.
    pub const fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    /// Consumes the recorder and returns the underlying sink.
    pub fn into_sink(self) -> S {
        self.sink
    }

    /// Records one structured simulation fact and returns its sequence number.
    pub fn record<'a>(
        &mut self,
        time: SimTime,
        source: TraceSource,
        level: TraceLevel,
        target: &'a str,
        event: &'a str,
        fields: &'a [TraceField<'a>],
    ) -> u64
    where
        S: TraceSink,
    {
        let sequence = self.allocate_sequence();
        self.sink.record(TraceRecord {
            sequence,
            time,
            source,
            level,
            target,
            event,
            fields,
        });
        sequence
    }

    fn allocate_sequence(&mut self) -> u64 {
        let sequence = self.next_sequence;
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("trace sequence overflow");
        sequence
    }
}

impl TraceRecorder<NoopTraceSink> {
    /// Creates a recorder that discards all records.
    pub const fn noop() -> Self {
        Self::new(NoopTraceSink)
    }
}

impl<S> Default for TraceRecorder<S>
where
    S: Default,
{
    fn default() -> Self {
        Self::new(S::default())
    }
}

#[cfg(test)]
mod tests;
