//! Object-safe component state serialization over a single Postcard root value.

use std::error::Error;
use std::fmt;
use std::io;

use serde::de::{DeserializeOwned, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Snapshot schema version used by every component.
pub const SNAPSHOT_VERSION: u32 = 1;

/// Errors produced while encoding, decoding, or validating component state.
#[derive(Debug)]
pub enum StateError {
    /// A component attempted to encode more than one root value.
    MultipleRootValues,
    /// A component did not encode or decode a root value.
    MissingRootValue,
    /// Bytes remained after the root value was decoded.
    TrailingBytes(u64),
    /// Postcard could not encode the component value.
    Encode(postcard::Error),
    /// Postcard could not decode the component value.
    Decode(postcard::Error),
    /// An underlying stream operation failed.
    Io(io::Error),
    /// The component schema version is unsupported.
    UnsupportedVersion(u32),
    /// The decoded state violates a component invariant.
    InvalidState(String),
    /// A payload is larger than the component's declared limit.
    PayloadTooLarge {
        /// Declared or observed payload length.
        actual: u64,
        /// Maximum payload length accepted by the component.
        maximum: u64,
    },
    /// A length cannot be represented or computed safely.
    LengthOverflow,
    /// A manifest does not contain the requested component.
    UnknownComponent(String),
}

impl fmt::Display for StateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultipleRootValues => {
                formatter.write_str("component payload contains multiple root values")
            }
            Self::MissingRootValue => formatter.write_str("component payload has no root value"),
            Self::TrailingBytes(count) => {
                write!(formatter, "component payload has {count} trailing bytes")
            }
            Self::Encode(error) => write!(formatter, "cannot encode component state: {error}"),
            Self::Decode(error) => write!(formatter, "cannot decode component state: {error}"),
            Self::Io(error) => write!(formatter, "component state I/O failed: {error}"),
            Self::UnsupportedVersion(version) => {
                write!(formatter, "unsupported component schema version {version}")
            }
            Self::InvalidState(reason) => write!(formatter, "invalid component state: {reason}"),
            Self::PayloadTooLarge { actual, maximum } => write!(
                formatter,
                "component payload length {actual} exceeds limit {maximum}"
            ),
            Self::LengthOverflow => formatter.write_str("component payload length overflow"),
            Self::UnknownComponent(key) => write!(formatter, "unknown snapshot component {key}"),
        }
    }
}

impl Error for StateError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Encode(error) | Self::Decode(error) => Some(error),
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for StateError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

struct CountingWriter<'a> {
    inner: &'a mut dyn io::Write,
    written: u64,
    maximum: Option<u64>,
    failure: Option<CountingFailure>,
}

#[derive(Debug)]
enum CountingFailure {
    LengthOverflow,
    PayloadTooLarge { actual: u64, maximum: u64 },
    Io(io::Error),
}

impl io::Write for CountingWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let requested = match u64::try_from(bytes.len()) {
            Ok(requested) => requested,
            Err(_) => {
                self.failure = Some(CountingFailure::LengthOverflow);
                return Err(io::Error::other("component payload length overflow"));
            }
        };
        let attempted = match self.written.checked_add(requested) {
            Some(attempted) => attempted,
            None => {
                self.failure = Some(CountingFailure::LengthOverflow);
                return Err(io::Error::other("component payload length overflow"));
            }
        };
        if let Some(maximum) = self.maximum
            && attempted > maximum
        {
            self.failure = Some(CountingFailure::PayloadTooLarge {
                actual: attempted,
                maximum,
            });
            return Err(io::Error::other("component payload limit exceeded"));
        }

        let count = match self.inner.write(bytes) {
            Ok(0) if !bytes.is_empty() => {
                self.failure = Some(CountingFailure::Io(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "component payload sink wrote zero bytes",
                )));
                return Err(io::Error::other("component payload sink failed"));
            }
            Ok(count) => count,
            Err(error) => {
                self.failure = Some(CountingFailure::Io(error));
                return Err(io::Error::other("component payload sink failed"));
            }
        };
        let count_u64 = match u64::try_from(count) {
            Ok(count) => count,
            Err(_) => {
                self.failure = Some(CountingFailure::LengthOverflow);
                return Err(io::Error::other("component payload length overflow"));
            }
        };
        self.written = match self.written.checked_add(count_u64) {
            Some(written) => written,
            None => {
                self.failure = Some(CountingFailure::LengthOverflow);
                return Err(io::Error::other("component payload length overflow"));
            }
        };
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        match self.inner.flush() {
            Ok(()) => Ok(()),
            Err(error) => {
                self.failure = Some(CountingFailure::Io(error));
                Err(io::Error::other("component payload sink failed"))
            }
        }
    }
}

/// A component payload writer.
///
/// Exactly one call to [`Self::serialize`] must succeed before the writer is
/// finished.
pub struct StateWriter<'a> {
    sink: CountingWriter<'a>,
    root_written: bool,
}

impl<'a> StateWriter<'a> {
    /// Creates a writer over a component payload sink.
    pub fn new(sink: &'a mut dyn io::Write) -> Self {
        Self {
            sink: CountingWriter {
                inner: sink,
                written: 0,
                maximum: None,
                failure: None,
            },
            root_written: false,
        }
    }

    pub(crate) fn with_limit(sink: &'a mut dyn io::Write, maximum: u64) -> Self {
        Self {
            sink: CountingWriter {
                inner: sink,
                written: 0,
                maximum: Some(maximum),
                failure: None,
            },
            root_written: false,
        }
    }

    /// Serializes the single root value directly into the payload sink.
    pub fn serialize<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), StateError> {
        if self.root_written {
            return Err(StateError::MultipleRootValues);
        }
        if let Err(error) = postcard::to_io(value, &mut self.sink) {
            return Err(match self.sink.failure.take() {
                Some(CountingFailure::LengthOverflow) => StateError::LengthOverflow,
                Some(CountingFailure::PayloadTooLarge { actual, maximum }) => {
                    StateError::PayloadTooLarge { actual, maximum }
                }
                Some(CountingFailure::Io(error)) => StateError::Io(error),
                None => StateError::Encode(error),
            });
        }
        self.root_written = true;
        Ok(())
    }

    /// Verifies the root-value contract and returns the encoded byte count.
    pub fn finish(self) -> Result<u64, StateError> {
        if !self.root_written {
            return Err(StateError::MissingRootValue);
        }
        Ok(self.sink.written)
    }
}

/// A component payload reader constrained to one declared payload slice.
///
/// Exactly one call to [`Self::deserialize`] must succeed before the reader is
/// finished. Any bytes following that root value are rejected.
pub struct StateReader<'a> {
    payload: &'a [u8],
    root_read: bool,
}

impl<'a> StateReader<'a> {
    /// Creates a reader over one complete component payload.
    #[must_use]
    pub const fn new(payload: &'a [u8]) -> Self {
        Self {
            payload,
            root_read: false,
        }
    }

    /// Decodes the single owned root value and rejects trailing bytes.
    pub fn deserialize<T: DeserializeOwned>(&mut self) -> Result<T, StateError> {
        if self.root_read {
            return Err(StateError::MultipleRootValues);
        }
        let (value, remaining) =
            postcard::take_from_bytes(self.payload).map_err(StateError::Decode)?;
        if !remaining.is_empty() {
            return Err(StateError::TrailingBytes(
                u64::try_from(remaining.len()).map_err(|_| StateError::LengthOverflow)?,
            ));
        }
        self.root_read = true;
        Ok(value)
    }

    /// Verifies that a root value was decoded.
    pub fn finish(self) -> Result<(), StateError> {
        if !self.root_read {
            return Err(StateError::MissingRootValue);
        }
        Ok(())
    }
}

/// Object-safe snapshot behavior implemented by stateful components.
pub trait Saveable {
    /// Returns the component's snapshot schema version.
    fn snapshot_version(&self) -> u32;

    /// Encodes the component's private state DTO.
    fn save(&self, writer: &mut StateWriter<'_>) -> Result<(), StateError>;

    /// Decodes, validates, and atomically commits the component's private state DTO.
    fn load(&mut self, version: u32, reader: &mut StateReader<'_>) -> Result<(), StateError>;
}

/// A borrowed byte array that selects Serde's byte-string data model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ByteSlice<'a>(&'a [u8]);

impl<'a> ByteSlice<'a> {
    /// Wraps a borrowed byte array without copying it.
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }

    /// Returns the wrapped bytes.
    #[must_use]
    pub const fn as_slice(self) -> &'a [u8] {
        self.0
    }
}

impl Serialize for ByteSlice<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_bytes(self.0)
    }
}

/// An owned byte array decoded from Serde's byte-string data model.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ByteVec(Vec<u8>);

impl ByteVec {
    /// Wraps an owned byte array.
    #[must_use]
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Returns the wrapped bytes.
    #[must_use]
    pub fn as_slice(&self) -> &[u8] {
        &self.0
    }

    /// Consumes the wrapper and returns its byte array.
    #[must_use]
    pub fn into_inner(self) -> Vec<u8> {
        self.0
    }
}

struct ByteVecVisitor;

impl<'de> Visitor<'de> for ByteVecVisitor {
    type Value = ByteVec;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a byte array")
    }

    fn visit_bytes<E: serde::de::Error>(self, value: &[u8]) -> Result<Self::Value, E> {
        Ok(ByteVec::new(value.to_vec()))
    }

    fn visit_byte_buf<E: serde::de::Error>(self, value: Vec<u8>) -> Result<Self::Value, E> {
        Ok(ByteVec::new(value))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<Self::Value, A::Error> {
        let mut bytes = Vec::with_capacity(sequence.size_hint().unwrap_or(0));
        while let Some(byte) = sequence.next_element()? {
            bytes.push(byte);
        }
        Ok(ByteVec::new(bytes))
    }
}

impl<'de> Deserialize<'de> for ByteVec {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_byte_buf(ByteVecVisitor)
    }
}

#[cfg(test)]
mod tests {
    use std::io;

    use serde::{Deserialize, Serialize};

    use super::{ByteSlice, ByteVec, StateError, StateReader, StateWriter};

    #[derive(Serialize)]
    struct BorrowedState<'a> {
        bytes: ByteSlice<'a>,
        marker: u32,
    }

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct OwnedState {
        bytes: ByteVec,
        marker: u32,
    }

    enum FailurePoint {
        Write,
        Flush,
    }

    struct FailingWriter {
        point: FailurePoint,
    }

    impl io::Write for FailingWriter {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            match self.point {
                FailurePoint::Write => Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected component write failure",
                )),
                FailurePoint::Flush => Ok(bytes.len()),
            }
        }

        fn flush(&mut self) -> io::Result<()> {
            match self.point {
                FailurePoint::Write => Ok(()),
                FailurePoint::Flush => Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "injected component flush failure",
                )),
            }
        }
    }

    #[test]
    fn byte_wrappers_share_one_serde_data_model() {
        let mut payload = Vec::new();
        let mut writer = StateWriter::new(&mut payload);
        writer
            .serialize(&BorrowedState {
                bytes: ByteSlice::new(&[1, 2, 3, 4]),
                marker: 42,
            })
            .unwrap();
        assert_eq!(writer.finish().unwrap(), payload.len() as u64);

        let mut reader = StateReader::new(&payload);
        let decoded: OwnedState = reader.deserialize().unwrap();
        reader.finish().unwrap();
        assert_eq!(
            decoded,
            OwnedState {
                bytes: ByteVec::new(vec![1, 2, 3, 4]),
                marker: 42,
            }
        );
    }

    #[test]
    fn writer_requires_exactly_one_root() {
        let mut payload = Vec::new();
        let mut writer = StateWriter::new(&mut payload);
        writer.serialize(&1_u32).unwrap();
        assert!(matches!(
            writer.serialize(&2_u32),
            Err(StateError::MultipleRootValues)
        ));

        let empty_writer = StateWriter::new(&mut payload);
        assert!(matches!(
            empty_writer.finish(),
            Err(StateError::MissingRootValue)
        ));
    }

    #[test]
    fn writer_enforces_payload_limit_before_forwarding_bytes() {
        let bytes = [0x5a; 4_096];
        let mut limited_payload = Vec::new();
        {
            let mut limited = StateWriter::with_limit(&mut limited_payload, 2);
            let error = limited.serialize(&ByteSlice::new(&bytes)).unwrap_err();
            assert!(matches!(
                error,
                StateError::PayloadTooLarge {
                    actual,
                    maximum: 2
                } if actual > 2
            ));
        }
        assert_eq!(limited_payload.len(), 2);

        let expected = postcard::to_stdvec(&ByteSlice::new(&[1, 2, 3, 4])).unwrap();
        let mut exact_payload = Vec::new();
        let mut exact = StateWriter::with_limit(&mut exact_payload, expected.len() as u64);
        exact.serialize(&ByteSlice::new(&[1, 2, 3, 4])).unwrap();
        assert_eq!(exact.finish().unwrap(), expected.len() as u64);
        assert_eq!(exact_payload, expected);
    }

    #[test]
    fn writer_preserves_sink_write_and_flush_errors() {
        let mut write_sink = FailingWriter {
            point: FailurePoint::Write,
        };
        let mut writer = StateWriter::new(&mut write_sink);
        match writer.serialize(&7_u32) {
            Err(StateError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
                assert_eq!(error.to_string(), "injected component write failure");
            }
            result => panic!("unexpected write result: {result:?}"),
        }

        let mut flush_sink = FailingWriter {
            point: FailurePoint::Flush,
        };
        let mut writer = StateWriter::new(&mut flush_sink);
        match writer.serialize(&7_u32) {
            Err(StateError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                assert_eq!(error.to_string(), "injected component flush failure");
            }
            result => panic!("unexpected flush result: {result:?}"),
        }
    }

    #[test]
    fn reader_rejects_trailing_and_malformed_payloads() {
        let mut encoded = postcard::to_stdvec(&7_u32).unwrap();
        encoded.push(0);
        let mut trailing = StateReader::new(&encoded);
        assert!(matches!(
            trailing.deserialize::<u32>(),
            Err(StateError::TrailingBytes(1))
        ));

        let mut malformed = StateReader::new(&[0x80]);
        assert!(matches!(
            malformed.deserialize::<u64>(),
            Err(StateError::Decode(_))
        ));
    }
}
