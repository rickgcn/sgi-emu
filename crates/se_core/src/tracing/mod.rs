//! Deterministic simulation tracing.
//!
//! The tracing module records structured simulation facts. It does not format,
//! filter, aggregate, or interpret records. Higher layers may decide how to
//! display, store, search, or analyze the collected records.
//!
//! Trace records are data, not callbacks. Emitting a trace record must not
//! advance simulated time, dispatch events, or affect component behavior.

use core::ops::Deref;
use std::borrow::Cow;

use smallvec::SmallVec;

use crate::component::ComponentId;
use crate::scheduler::SimTime;

/// Importance level of a trace record.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
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

/// Coarse producer interest for one trace source.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TraceInterest {
    /// No records from the source can be observed.
    None,

    /// Records may be accepted after evaluating their complete metadata.
    Filtered,

    /// Every record from the source is accepted.
    All,
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

/// Owned structured value transported from a trace producer.
///
/// Text may borrow static data or own dynamically generated data, so producers
/// do not need to allocate for stable trace names and values.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum OwnedTraceValue {
    /// Boolean value.
    Bool(bool),

    /// Unsigned integer value.
    U64(u64),

    /// Unsigned integer value that should usually be displayed as hexadecimal.
    Hex64(u64),

    /// Static or owned string value.
    String(Cow<'static, str>),

    /// Signed integer value.
    ///
    /// This variant follows the original owned trace variants to preserve their
    /// serialized discriminants.
    I64(i64),
}

impl<'a> From<&'a OwnedTraceValue> for TraceValue<'a> {
    fn from(value: &'a OwnedTraceValue) -> Self {
        match value {
            OwnedTraceValue::Bool(value) => Self::Bool(*value),
            OwnedTraceValue::U64(value) => Self::U64(*value),
            OwnedTraceValue::Hex64(value) => Self::Hex64(*value),
            OwnedTraceValue::String(value) => Self::Str(value.as_ref()),
            OwnedTraceValue::I64(value) => Self::I64(*value),
        }
    }
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

/// Single ordered owned key-value fact produced by a component.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OwnedTraceField {
    /// Stable or dynamically generated field name.
    pub key: Cow<'static, str>,

    /// Field value.
    pub value: OwnedTraceValue,
}

impl OwnedTraceField {
    /// Creates a boolean owned trace field.
    pub fn bool(key: impl Into<Cow<'static, str>>, value: bool) -> Self {
        Self {
            key: key.into(),
            value: OwnedTraceValue::Bool(value),
        }
    }

    /// Creates an unsigned integer owned trace field.
    pub fn u64(key: impl Into<Cow<'static, str>>, value: u64) -> Self {
        Self {
            key: key.into(),
            value: OwnedTraceValue::U64(value),
        }
    }

    /// Creates a signed integer owned trace field.
    pub fn i64(key: impl Into<Cow<'static, str>>, value: i64) -> Self {
        Self {
            key: key.into(),
            value: OwnedTraceValue::I64(value),
        }
    }

    /// Creates a hexadecimal display hint owned trace field.
    pub fn hex64(key: impl Into<Cow<'static, str>>, value: u64) -> Self {
        Self {
            key: key.into(),
            value: OwnedTraceValue::Hex64(value),
        }
    }

    /// Creates a static or owned string trace field.
    pub fn string(key: impl Into<Cow<'static, str>>, value: impl Into<Cow<'static, str>>) -> Self {
        Self {
            key: key.into(),
            value: OwnedTraceValue::String(value.into()),
        }
    }
}

impl<'a> From<&'a OwnedTraceField> for TraceField<'a> {
    fn from(field: &'a OwnedTraceField) -> Self {
        Self {
            key: field.key.as_ref(),
            value: (&field.value).into(),
        }
    }
}

/// Ordered owned trace fields with inline storage for common component events.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OwnedTraceFields(SmallVec<[OwnedTraceField; 8]>);

impl OwnedTraceFields {
    /// Returns whether the fields spilled beyond inline storage.
    pub fn spilled(&self) -> bool {
        self.0.spilled()
    }
}

impl Deref for OwnedTraceFields {
    type Target = [OwnedTraceField];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<[OwnedTraceField]> for OwnedTraceFields {
    fn as_ref(&self) -> &[OwnedTraceField] {
        self
    }
}

impl From<Vec<OwnedTraceField>> for OwnedTraceFields {
    fn from(value: Vec<OwnedTraceField>) -> Self {
        Self(SmallVec::from_vec(value))
    }
}

impl<const N: usize> From<[OwnedTraceField; N]> for OwnedTraceFields {
    fn from(value: [OwnedTraceField; N]) -> Self {
        Self(value.into_iter().collect())
    }
}

impl FromIterator<OwnedTraceField> for OwnedTraceFields {
    fn from_iter<T: IntoIterator<Item = OwnedTraceField>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

/// Complete owned trace event transported from a component to its integration layer.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OwnedTraceEvent {
    /// Importance level.
    pub level: TraceLevel,

    /// Producer-local trace target.
    pub target: Cow<'static, str>,

    /// Stable or dynamically generated event name.
    pub event: Cow<'static, str>,

    /// Ordered fields.
    pub fields: OwnedTraceFields,
}

impl OwnedTraceEvent {
    /// Creates an owned trace event.
    pub fn new(
        level: TraceLevel,
        target: impl Into<Cow<'static, str>>,
        event: impl Into<Cow<'static, str>>,
        fields: impl Into<OwnedTraceFields>,
    ) -> Self {
        Self {
            level,
            target: target.into(),
            event: event.into(),
            fields: fields.into(),
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
    /// Returns the sink's coarse interest in one source.
    fn interest(&self, _source: TraceSource) -> TraceInterest {
        TraceInterest::Filtered
    }

    /// Returns whether a record with the given metadata should be constructed.
    fn enabled(
        &self,
        _source: TraceSource,
        _level: TraceLevel,
        _target: &str,
        _event: &str,
    ) -> bool {
        true
    }

    /// Records one complete trace record.
    fn record(&mut self, record: TraceRecord<'_>);
}

/// Trace sink that discards all records.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NoopTraceSink;

impl TraceSink for NoopTraceSink {
    fn interest(&self, _source: TraceSource) -> TraceInterest {
        TraceInterest::None
    }

    fn enabled(
        &self,
        _source: TraceSource,
        _level: TraceLevel,
        _target: &str,
        _event: &str,
    ) -> bool {
        false
    }

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

    /// Returns the sink's coarse interest in one trace source.
    pub fn interest(&self, source: TraceSource) -> TraceInterest
    where
        S: TraceSink,
    {
        self.sink.interest(source)
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

    /// Lazily constructs and records one structured simulation fact.
    ///
    /// An uninterested source does not allocate a sequence number. A filtered
    /// record allocates its sequence before the metadata gate is evaluated, so
    /// fine-grained filtering preserves gaps without constructing fields.
    pub fn record_lazy<'a, F, T>(
        &mut self,
        time: SimTime,
        source: TraceSource,
        level: TraceLevel,
        target: &'a str,
        event: &'a str,
        build_fields: F,
    ) -> Option<u64>
    where
        S: TraceSink,
        F: FnOnce() -> T,
        T: AsRef<[TraceField<'a>]>,
    {
        let interest = self.sink.interest(source);
        if interest == TraceInterest::None {
            return None;
        }

        let sequence = self.allocate_sequence();
        if interest == TraceInterest::Filtered && !self.sink.enabled(source, level, target, event) {
            return Some(sequence);
        }

        let fields = build_fields();
        self.sink.record(TraceRecord {
            sequence,
            time,
            source,
            level,
            target,
            event,
            fields: fields.as_ref(),
        });
        Some(sequence)
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
