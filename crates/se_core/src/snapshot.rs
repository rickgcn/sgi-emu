//! Defines the version-one runtime snapshot container and component dispatch.
//!
//! A container stores fixed magic, container version, build and profile
//! fingerprints, and a canonical key-ordered component sequence. Integer framing
//! is little-endian: counts, key lengths, and schema versions use `u32`, while
//! payload lengths use `u64`. Each component payload contains the private Postcard
//! root value produced through [`StateWriter`]. A final unkeyed BLAKE3 digest
//! covers all preceding bytes; it detects corruption but does not authenticate the
//! snapshot.
//!
//! Loading requires exact build, profile, manifest, and component-version matches.
//! Components are loaded into a fresh machine from [`MachineFactory`]; the
//! candidate is returned, or replaces a running machine, only after integrity and
//! cross-component validation succeed.

use std::error::Error;
use std::fmt;
use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};

use crate::machine::{Machine, MachineCreateError, MachineFactory};
use crate::save::{SNAPSHOT_VERSION, Saveable, StateError, StateReader, StateWriter};

/// Identifies the snapshot container schema emitted and accepted by this crate.
pub const CONTAINER_VERSION: u32 = 1;

/// Limits a component key's encoded ASCII byte length.
pub const MAX_COMPONENT_KEY_LEN: usize = 4_096;

const MAGIC: [u8; 8] = *b"SESNAP01";
const CHECKSUM_LEN: u64 = 32;
const FIXED_HEADER_LEN: u64 = 8 + 4 + 32 + 32 + 4;

/// Reports an invalid stable component key.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComponentKeyError {
    /// A stable key cannot be empty.
    Empty,
    /// The key exceeds [`MAX_COMPONENT_KEY_LEN`].
    TooLong(usize),
    /// Stable keys are restricted to ASCII.
    NonAscii,
    /// The value is not a canonical slash-separated path.
    NonCanonicalPath,
}

impl fmt::Display for ComponentKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("snapshot component key is empty"),
            Self::TooLong(len) => write!(
                formatter,
                "snapshot component key length {len} exceeds {MAX_COMPONENT_KEY_LEN}"
            ),
            Self::NonAscii => formatter.write_str("snapshot component key is not ASCII"),
            Self::NonCanonicalPath => {
                formatter.write_str("snapshot component key is not a canonical path")
            }
        }
    }
}

impl Error for ComponentKeyError {}

/// Identifies one snapshot component with a stable profile-defined ASCII path.
///
/// A key is a relative slash-separated path. Every segment is nonempty, differs
/// from `.` and `..`, and contains only ASCII alphanumeric characters, `-`, `_`, or
/// `.`. Keys are ordered by their encoded bytes for canonical manifests.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ComponentKey(String);

impl ComponentKey {
    /// Validates and creates a stable component key.
    ///
    /// # Errors
    ///
    /// Returns [`ComponentKeyError::Empty`] for an empty value,
    /// [`ComponentKeyError::TooLong`] above [`MAX_COMPONENT_KEY_LEN`] bytes,
    /// [`ComponentKeyError::NonAscii`] for non-ASCII text, or
    /// [`ComponentKeyError::NonCanonicalPath`] when the path grammar is violated.
    pub fn new(value: impl Into<String>) -> Result<Self, ComponentKeyError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ComponentKeyError::Empty);
        }
        if value.len() > MAX_COMPONENT_KEY_LEN {
            return Err(ComponentKeyError::TooLong(value.len()));
        }
        if !value.is_ascii() {
            return Err(ComponentKeyError::NonAscii);
        }
        let canonical = !value.starts_with('/')
            && !value.ends_with('/')
            && value.split('/').all(|segment| {
                !segment.is_empty()
                    && segment != "."
                    && segment != ".."
                    && segment.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')
                    })
            });
        if !canonical {
            return Err(ComponentKeyError::NonCanonicalPath);
        }
        Ok(Self(value))
    }

    /// Returns the canonical path text.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns the canonical bytes used for manifest ordering.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Display for ComponentKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

/// Carries the composition root's identity for an exact application build.
///
/// This crate compares the 32 bytes exactly and does not derive or interpret them.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct BuildFingerprint([u8; 32]);

impl BuildFingerprint {
    /// Creates a build fingerprint from externally generated bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the exact fingerprint bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Carries the composition root's identity for one machine profile.
///
/// The identity covers topology and guest-visible configuration according to the
/// application's profile policy. This crate compares the bytes exactly and does
/// not derive or interpret them.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ProfileFingerprint([u8; 32]);

impl ProfileFingerprint {
    /// Creates a profile fingerprint from composition-root generated bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the exact fingerprint bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Describes one entry in a machine's canonical snapshot manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotComponent {
    /// Stable profile-defined component key.
    pub key: ComponentKey,
    /// Component schema version, which must equal [`SNAPSHOT_VERSION`].
    pub schema_version: u32,
    /// Maximum Postcard payload length enforced during save and before load allocation.
    pub max_payload_len: u64,
}

/// Reports an invalid snapshot component manifest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SnapshotManifestError {
    /// A component exposes a schema version other than one.
    UnsupportedComponentVersion {
        /// Component key with the unsupported version.
        key: ComponentKey,
        /// Rejected schema version.
        version: u32,
    },
    /// Two entries use the same stable component key.
    DuplicateKey(ComponentKey),
    /// Manifest entries are not in canonical bytewise key order.
    NonCanonicalOrder {
        /// Key immediately before the ordering violation.
        previous: ComponentKey,
        /// Key that should have appeared earlier.
        current: ComponentKey,
    },
    /// The manifest contains more records than a `u32` count can encode.
    TooManyComponents,
}

impl fmt::Display for SnapshotManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedComponentVersion { key, version } => {
                write!(
                    formatter,
                    "component {key} uses unsupported schema version {version}"
                )
            }
            Self::DuplicateKey(key) => write!(formatter, "component key {key} is duplicated"),
            Self::NonCanonicalOrder { previous, current } => write!(
                formatter,
                "component key {current} is not ordered after {previous}"
            ),
            Self::TooManyComponents => {
                formatter.write_str("snapshot manifest exceeds u32 component count")
            }
        }
    }
}

impl Error for SnapshotManifestError {}

/// Builds a canonical manifest from stable keys and bound components.
///
/// [`Self::bind`] reads each target's schema version. [`Self::finish`] sorts entries
/// by key bytes and then validates the complete manifest.
#[derive(Default)]
pub struct SnapshotManifestBuilder {
    components: Vec<SnapshotComponent>,
}

impl SnapshotManifestBuilder {
    /// Creates an empty manifest builder.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a stable key bound to a component's reported schema version.
    ///
    /// Duplicate keys are accepted provisionally and rejected by [`Self::finish`].
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotManifestError::UnsupportedComponentVersion`] without
    /// changing the builder when the target does not report [`SNAPSHOT_VERSION`].
    pub fn bind(
        &mut self,
        key: ComponentKey,
        target: &dyn Saveable,
        max_payload_len: u64,
    ) -> Result<(), SnapshotManifestError> {
        let version = target.snapshot_version();
        if version != SNAPSHOT_VERSION {
            return Err(SnapshotManifestError::UnsupportedComponentVersion { key, version });
        }
        self.components.push(SnapshotComponent {
            key,
            schema_version: version,
            max_payload_len,
        });
        Ok(())
    }

    /// Sorts entries by key bytes and returns the validated manifest.
    ///
    /// # Errors
    ///
    /// Returns [`SnapshotManifestError::TooManyComponents`] when the entry count
    /// cannot be framed as `u32`, or another [`SnapshotManifestError`] when the
    /// sorted manifest is invalid.
    pub fn finish(mut self) -> Result<Vec<SnapshotComponent>, SnapshotManifestError> {
        if self.components.len() > u32::MAX as usize {
            return Err(SnapshotManifestError::TooManyComponents);
        }
        self.components
            .sort_by(|left, right| left.key.cmp(&right.key));
        validate_manifest(&self.components)?;
        Ok(self.components)
    }
}

/// Validates a manifest without changing its supplied order.
///
/// # Errors
///
/// Returns [`SnapshotManifestError::TooManyComponents`] when its length exceeds a
/// `u32` count, [`SnapshotManifestError::UnsupportedComponentVersion`] when an
/// entry does not use [`SNAPSHOT_VERSION`],
/// [`SnapshotManifestError::DuplicateKey`] for adjacent equal keys, or
/// [`SnapshotManifestError::NonCanonicalOrder`] when keys are not in ascending
/// bytewise order.
pub fn validate_manifest(components: &[SnapshotComponent]) -> Result<(), SnapshotManifestError> {
    if components.len() > u32::MAX as usize {
        return Err(SnapshotManifestError::TooManyComponents);
    }
    for component in components {
        if component.schema_version != SNAPSHOT_VERSION {
            return Err(SnapshotManifestError::UnsupportedComponentVersion {
                key: component.key.clone(),
                version: component.schema_version,
            });
        }
    }
    for pair in components.windows(2) {
        if pair[0].key == pair[1].key {
            return Err(SnapshotManifestError::DuplicateKey(pair[0].key.clone()));
        }
        if pair[0].key > pair[1].key {
            return Err(SnapshotManifestError::NonCanonicalOrder {
                previous: pair[0].key.clone(),
                current: pair[1].key.clone(),
            });
        }
    }
    Ok(())
}

/// Defines object-safe component dispatch for a complete machine.
///
/// The returned manifest remains the canonical component set for the duration of
/// a save or load operation. Component methods select private state by stable key;
/// cross-component invariants are checked only after all candidate components have
/// loaded.
pub trait SnapshotTarget {
    /// Returns the sorted, unique, stable component manifest.
    fn snapshot_components(&self) -> &[SnapshotComponent];

    /// Saves exactly one component selected by stable key.
    ///
    /// A successful implementation writes exactly one root value through `writer`.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::UnknownComponent`] when `key` is absent, or another
    /// [`StateError`] when the selected component cannot be saved.
    fn save_component(
        &self,
        key: &ComponentKey,
        writer: &mut StateWriter<'_>,
    ) -> Result<(), StateError>;

    /// Loads, validates, and commits exactly one component selected by stable key.
    ///
    /// A failure leaves that component's prior state unchanged. Whole-machine
    /// invariants may remain temporarily unresolved until every component loads.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::UnknownComponent`] when `key` is absent,
    /// [`StateError::UnsupportedVersion`] when `version` is unsupported, or another
    /// [`StateError`] when decoding or component validation fails.
    fn load_component(
        &mut self,
        key: &ComponentKey,
        version: u32,
        reader: &mut StateReader<'_>,
    ) -> Result<(), StateError>;

    /// Validates event targets and every cross-component invariant after loading.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::InvalidState`] or another [`StateError`] when the
    /// completely loaded candidate is not internally consistent.
    fn validate_loaded_snapshot(&self) -> Result<(), StateError>;
}

/// Reports a snapshot framing, compatibility, integrity, or state failure.
#[derive(Debug)]
pub enum SnapshotError {
    /// An underlying stream operation failed.
    Io(io::Error),
    /// The output stream must be empty and positioned at its start.
    OutputNotEmpty,
    /// The fixed snapshot magic does not match this container format.
    InvalidMagic,
    /// The snapshot container version is not version one.
    UnsupportedContainerVersion(u32),
    /// The exact application build differs from the snapshot build.
    BuildFingerprintMismatch,
    /// The exact machine profile differs from the snapshot profile.
    ProfileFingerprintMismatch,
    /// The current machine exposes an invalid component manifest.
    Manifest(SnapshotManifestError),
    /// The snapshot and candidate machine have different component counts.
    ComponentCountMismatch {
        /// Number of records declared by the snapshot.
        found: u32,
        /// Number of records required by the candidate machine.
        expected: u32,
    },
    /// A component key in the container is invalid.
    InvalidComponentKey(ComponentKeyError),
    /// The same component key occurs more than once.
    DuplicateComponent(ComponentKey),
    /// Component records are not in canonical bytewise key order.
    NonCanonicalComponentOrder {
        /// Previous container key.
        previous: ComponentKey,
        /// Current out-of-order container key.
        current: ComponentKey,
    },
    /// A record key does not match the candidate manifest at that position.
    ComponentKeyMismatch {
        /// Candidate key required at this position.
        expected: ComponentKey,
        /// Snapshot key found at this position.
        found: ComponentKey,
    },
    /// A component record uses a schema version other than one.
    UnsupportedComponentVersion {
        /// Component record key.
        key: ComponentKey,
        /// Rejected schema version.
        version: u32,
    },
    /// A payload length exceeds the current profile's component limit.
    PayloadTooLarge {
        /// Component record key.
        key: ComponentKey,
        /// Declared or attempted payload length.
        actual: u64,
        /// Maximum payload length accepted by the candidate.
        maximum: u64,
    },
    /// A declared container length cannot be represented safely.
    LengthOverflow,
    /// Bytes remain between the canonical component records and checksum.
    TrailingData(u64),
    /// The unkeyed BLAKE3 integrity value does not match the framed contents.
    IntegrityMismatch,
    /// A component failed to save, load, or validate its private DTO.
    ComponentState {
        /// Component selected when the error occurred.
        key: ComponentKey,
        /// Underlying component-state error.
        error: StateError,
    },
    /// A fresh candidate machine could not be assembled.
    MachineCreate(MachineCreateError),
    /// Cross-component validation rejected the completely loaded candidate.
    MachineValidation(StateError),
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(formatter, "snapshot I/O failed: {error}"),
            Self::OutputNotEmpty => {
                formatter.write_str("snapshot output is not empty at position zero")
            }
            Self::InvalidMagic => formatter.write_str("snapshot magic is invalid"),
            Self::UnsupportedContainerVersion(version) => {
                write!(
                    formatter,
                    "unsupported snapshot container version {version}"
                )
            }
            Self::BuildFingerprintMismatch => {
                formatter.write_str("snapshot build fingerprint does not match")
            }
            Self::ProfileFingerprintMismatch => {
                formatter.write_str("snapshot profile fingerprint does not match")
            }
            Self::Manifest(error) => write!(formatter, "invalid snapshot manifest: {error}"),
            Self::ComponentCountMismatch { found, expected } => write!(
                formatter,
                "snapshot contains {found} components but machine requires {expected}"
            ),
            Self::InvalidComponentKey(error) => {
                write!(formatter, "invalid snapshot component key: {error}")
            }
            Self::DuplicateComponent(key) => {
                write!(formatter, "snapshot component {key} is duplicated")
            }
            Self::NonCanonicalComponentOrder { previous, current } => write!(
                formatter,
                "snapshot component {current} is not ordered after {previous}"
            ),
            Self::ComponentKeyMismatch { expected, found } => write!(
                formatter,
                "snapshot component {found} does not match expected component {expected}"
            ),
            Self::UnsupportedComponentVersion { key, version } => write!(
                formatter,
                "snapshot component {key} uses unsupported schema version {version}"
            ),
            Self::PayloadTooLarge {
                key,
                actual,
                maximum,
            } => write!(
                formatter,
                "snapshot component {key} payload length {actual} exceeds limit {maximum}"
            ),
            Self::LengthOverflow => formatter.write_str("snapshot length overflow"),
            Self::TrailingData(count) => {
                write!(
                    formatter,
                    "snapshot has {count} trailing bytes before its checksum"
                )
            }
            Self::IntegrityMismatch => formatter.write_str("snapshot integrity check failed"),
            Self::ComponentState { key, error } => {
                write!(formatter, "snapshot component {key} failed: {error}")
            }
            Self::MachineCreate(error) => {
                write!(formatter, "cannot create snapshot target: {error}")
            }
            Self::MachineValidation(error) => {
                write!(formatter, "loaded machine state is invalid: {error}")
            }
        }
    }
}

impl Error for SnapshotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Manifest(error) => Some(error),
            Self::InvalidComponentKey(error) => Some(error),
            Self::ComponentState { error, .. } => Some(error),
            Self::MachineCreate(error) => Some(error),
            Self::MachineValidation(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for SnapshotError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<SnapshotManifestError> for SnapshotError {
    fn from(error: SnapshotManifestError) -> Self {
        Self::Manifest(error)
    }
}

fn map_component_save_error(key: &ComponentKey, error: StateError) -> SnapshotError {
    match error {
        StateError::Io(error) => SnapshotError::Io(error),
        StateError::PayloadTooLarge { actual, maximum } => SnapshotError::PayloadTooLarge {
            key: key.clone(),
            actual,
            maximum,
        },
        error => SnapshotError::ComponentState {
            key: key.clone(),
            error,
        },
    }
}

/// Writes one canonical snapshot directly into an empty seekable stream.
///
/// The stream must have length zero and be positioned at zero. The function writes
/// component payloads directly, backpatches their lengths, flushes those writes
/// before rereading all framed bytes, appends the resulting integrity digest, and
/// flushes the completed stream. A failure after writing begins may leave a
/// partial snapshot in `output`. `build` and `profile` are written verbatim;
/// deriving them and matching them to `machine` are composition-root
/// responsibilities.
///
/// # Errors
///
/// Returns [`SnapshotError::OutputNotEmpty`] if the stream is not initially empty
/// at position zero, [`SnapshotError::Manifest`] for an invalid machine manifest,
/// [`SnapshotError::PayloadTooLarge`] as soon as a component crosses its declared
/// limit, [`SnapshotError::ComponentState`] when a component cannot be encoded, or
/// another [`SnapshotError`] for I/O or framing-length failures.
pub fn write_snapshot<W: Read + Write + Seek>(
    machine: &dyn Machine,
    build: BuildFingerprint,
    profile: ProfileFingerprint,
    output: &mut W,
) -> Result<(), SnapshotError> {
    validate_manifest(machine.snapshot_components())?;
    let position = output.stream_position()?;
    let end = output.seek(SeekFrom::End(0))?;
    if position != 0 || end != 0 {
        return Err(SnapshotError::OutputNotEmpty);
    }
    output.seek(SeekFrom::Start(0))?;
    output.write_all(&MAGIC)?;
    write_u32(output, CONTAINER_VERSION)?;
    output.write_all(build.as_bytes())?;
    output.write_all(profile.as_bytes())?;
    let component_count = u32::try_from(machine.snapshot_components().len())
        .map_err(|_| SnapshotError::LengthOverflow)?;
    write_u32(output, component_count)?;

    for component in machine.snapshot_components() {
        let key_len = u32::try_from(component.key.as_bytes().len())
            .map_err(|_| SnapshotError::LengthOverflow)?;
        write_u32(output, key_len)?;
        output.write_all(component.key.as_bytes())?;
        write_u32(output, component.schema_version)?;
        let length_position = output.stream_position()?;
        write_u64(output, 0)?;
        let payload_start = output.stream_position()?;
        let mut writer = StateWriter::with_limit(output, component.max_payload_len);
        machine
            .save_component(&component.key, &mut writer)
            .map_err(|error| map_component_save_error(&component.key, error))?;
        let observed_length = writer
            .finish()
            .map_err(|error| map_component_save_error(&component.key, error))?;
        let payload_end = output.stream_position()?;
        let seek_length = payload_end
            .checked_sub(payload_start)
            .ok_or(SnapshotError::LengthOverflow)?;
        if observed_length != seek_length {
            return Err(SnapshotError::LengthOverflow);
        }
        if observed_length > component.max_payload_len {
            return Err(SnapshotError::PayloadTooLarge {
                key: component.key.clone(),
                actual: observed_length,
                maximum: component.max_payload_len,
            });
        }
        output.seek(SeekFrom::Start(length_position))?;
        write_u64(output, observed_length)?;
        output.seek(SeekFrom::Start(payload_end))?;
    }

    let checksum_position = output.stream_position()?;
    output.flush()?;
    output.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut remaining = checksum_position;
    let mut buffer = [0_u8; 16 * 1_024];
    while remaining != 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64))
            .map_err(|_| SnapshotError::LengthOverflow)?;
        output.read_exact(&mut buffer[..count])?;
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    output.seek(SeekFrom::Start(checksum_position))?;
    output.write_all(hasher.finalize().as_bytes())?;
    output.flush()?;
    Ok(())
}

/// Encodes one canonical snapshot into a newly allocated byte array.
///
/// # Errors
///
/// Returns any error reported by [`write_snapshot`].
pub fn encode_snapshot(
    machine: &dyn Machine,
    build: BuildFingerprint,
    profile: ProfileFingerprint,
) -> Result<Vec<u8>, SnapshotError> {
    let mut output = Cursor::new(Vec::new());
    write_snapshot(machine, build, profile, &mut output)?;
    Ok(output.into_inner())
}

/// Loads a snapshot into a fresh machine and returns it after full validation.
///
/// Framing and compatibility fields are checked before candidate construction.
/// Component payloads are then decoded into that isolated candidate in canonical
/// manifest order. The integrity digest is verified before cross-component
/// validation and before the candidate is returned; a failure discards the entire
/// candidate. The digest is not an authentication boundary, so callers decide
/// whether the input source is trusted.
///
/// # Errors
///
/// Returns [`SnapshotError::Io`] for stream failures or truncated framing,
/// compatibility and manifest variants for any build, profile, version, key, or
/// component-set mismatch, [`SnapshotError::MachineCreate`] if the factory cannot
/// construct the candidate, [`SnapshotError::ComponentState`] if a component
/// cannot load, [`SnapshotError::IntegrityMismatch`] for corrupted framed bytes, or
/// [`SnapshotError::MachineValidation`] when final cross-component validation
/// fails.
pub fn read_snapshot<R: Read + Seek>(
    input: &mut R,
    expected_build: BuildFingerprint,
    factory: &dyn MachineFactory,
) -> Result<Box<dyn Machine>, SnapshotError> {
    let file_len = input.seek(SeekFrom::End(0))?;
    if file_len < FIXED_HEADER_LEN + CHECKSUM_LEN {
        return Err(SnapshotError::Io(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "snapshot is shorter than its fixed framing",
        )));
    }
    input.seek(SeekFrom::Start(0))?;
    let content_len = file_len
        .checked_sub(CHECKSUM_LEN)
        .ok_or(SnapshotError::LengthOverflow)?;
    let mut framed = HashingReader::new(input, content_len);

    let mut magic = [0_u8; 8];
    framed.read_exact(&mut magic)?;
    if magic != MAGIC {
        return Err(SnapshotError::InvalidMagic);
    }
    let container_version = read_u32(&mut framed)?;
    if container_version != CONTAINER_VERSION {
        return Err(SnapshotError::UnsupportedContainerVersion(
            container_version,
        ));
    }
    let mut build_bytes = [0_u8; 32];
    framed.read_exact(&mut build_bytes)?;
    if BuildFingerprint::from_bytes(build_bytes) != expected_build {
        return Err(SnapshotError::BuildFingerprintMismatch);
    }
    let mut profile_bytes = [0_u8; 32];
    framed.read_exact(&mut profile_bytes)?;
    if ProfileFingerprint::from_bytes(profile_bytes) != factory.profile_fingerprint() {
        return Err(SnapshotError::ProfileFingerprintMismatch);
    }
    let component_count = read_u32(&mut framed)?;

    let mut candidate = factory.create().map_err(SnapshotError::MachineCreate)?;
    validate_manifest(candidate.snapshot_components())?;
    let expected_count = u32::try_from(candidate.snapshot_components().len())
        .map_err(|_| SnapshotError::LengthOverflow)?;
    if component_count != expected_count {
        return Err(SnapshotError::ComponentCountMismatch {
            found: component_count,
            expected: expected_count,
        });
    }

    let manifest = candidate.snapshot_components().to_vec();
    let mut previous_key: Option<ComponentKey> = None;
    for expected in manifest {
        let key_len = read_u32(&mut framed)?;
        let key_len = usize::try_from(key_len).map_err(|_| SnapshotError::LengthOverflow)?;
        if key_len > MAX_COMPONENT_KEY_LEN {
            return Err(SnapshotError::InvalidComponentKey(
                ComponentKeyError::TooLong(key_len),
            ));
        }
        let mut key_bytes = vec![0_u8; key_len];
        framed.read_exact(&mut key_bytes)?;
        let key_text = String::from_utf8(key_bytes)
            .map_err(|_| SnapshotError::InvalidComponentKey(ComponentKeyError::NonAscii))?;
        let key = ComponentKey::new(key_text).map_err(SnapshotError::InvalidComponentKey)?;
        if let Some(previous) = &previous_key {
            if previous == &key {
                return Err(SnapshotError::DuplicateComponent(key));
            }
            if previous > &key {
                return Err(SnapshotError::NonCanonicalComponentOrder {
                    previous: previous.clone(),
                    current: key,
                });
            }
        }
        if key != expected.key {
            return Err(SnapshotError::ComponentKeyMismatch {
                expected: expected.key,
                found: key,
            });
        }
        previous_key = Some(key.clone());

        let version = read_u32(&mut framed)?;
        if version != SNAPSHOT_VERSION {
            return Err(SnapshotError::UnsupportedComponentVersion { key, version });
        }
        let payload_len = read_u64(&mut framed)?;
        if payload_len > expected.max_payload_len {
            return Err(SnapshotError::PayloadTooLarge {
                key,
                actual: payload_len,
                maximum: expected.max_payload_len,
            });
        }
        let allocation = usize::try_from(payload_len).map_err(|_| SnapshotError::LengthOverflow)?;
        let mut payload = vec![0_u8; allocation];
        framed.read_exact(&mut payload)?;
        let mut reader = StateReader::new(&payload);
        candidate
            .load_component(&expected.key, version, &mut reader)
            .map_err(|error| SnapshotError::ComponentState {
                key: expected.key.clone(),
                error,
            })?;
        reader
            .finish()
            .map_err(|error| SnapshotError::ComponentState {
                key: expected.key,
                error,
            })?;
    }

    if framed.remaining() != 0 {
        return Err(SnapshotError::TrailingData(framed.remaining()));
    }
    let actual_checksum = framed.finish();
    let mut expected_checksum = [0_u8; 32];
    input.read_exact(&mut expected_checksum)?;
    if actual_checksum.as_bytes() != &expected_checksum {
        return Err(SnapshotError::IntegrityMismatch);
    }
    candidate
        .validate_loaded_snapshot()
        .map_err(SnapshotError::MachineValidation)?;
    Ok(candidate)
}

/// Decodes a snapshot byte array into a fresh fully validated machine.
///
/// # Errors
///
/// Returns any error reported by [`read_snapshot`].
pub fn decode_snapshot(
    bytes: &[u8],
    expected_build: BuildFingerprint,
    factory: &dyn MachineFactory,
) -> Result<Box<dyn Machine>, SnapshotError> {
    read_snapshot(&mut Cursor::new(bytes), expected_build, factory)
}

/// Replaces a running machine only after a fresh candidate loads successfully.
///
/// `running` remains unchanged if decoding, compatibility checks, component loads,
/// integrity verification, or final validation fail.
///
/// # Errors
///
/// Returns any error reported by [`decode_snapshot`].
pub fn restore_machine(
    running: &mut Box<dyn Machine>,
    bytes: &[u8],
    expected_build: BuildFingerprint,
    factory: &dyn MachineFactory,
) -> Result<(), SnapshotError> {
    let candidate = decode_snapshot(bytes, expected_build, factory)?;
    *running = candidate;
    Ok(())
}

struct HashingReader<'a, R> {
    input: &'a mut R,
    remaining: u64,
    hasher: blake3::Hasher,
}

impl<'a, R> HashingReader<'a, R> {
    fn new(input: &'a mut R, remaining: u64) -> Self {
        Self {
            input,
            remaining,
            hasher: blake3::Hasher::new(),
        }
    }

    const fn remaining(&self) -> u64 {
        self.remaining
    }

    fn finish(self) -> blake3::Hash {
        debug_assert_eq!(self.remaining, 0);
        self.hasher.finalize()
    }
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if self.remaining == 0 || output.is_empty() {
            return Ok(0);
        }
        let limit = usize::try_from(self.remaining.min(output.len() as u64))
            .map_err(|_| io::Error::other("snapshot read length overflow"))?;
        let count = self.input.read(&mut output[..limit])?;
        if count != 0 {
            self.hasher.update(&output[..count]);
            self.remaining -= count as u64;
        }
        Ok(count)
    }
}

fn write_u32(output: &mut dyn Write, value: u32) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn write_u64(output: &mut dyn Write, value: u64) -> io::Result<()> {
    output.write_all(&value.to_le_bytes())
}

fn read_u32(input: &mut dyn Read) -> io::Result<u32> {
    let mut bytes = [0_u8; 4];
    input.read_exact(&mut bytes)?;
    Ok(u32::from_le_bytes(bytes))
}

fn read_u64(input: &mut dyn Read) -> io::Result<u64> {
    let mut bytes = [0_u8; 8];
    input.read_exact(&mut bytes)?;
    Ok(u64::from_le_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::fmt::Write as FmtWrite;
    use std::io::{self, Cursor, Read, Seek, SeekFrom, Write};
    use std::rc::Rc;

    use crate::event::ScheduledEvent;
    use crate::inspect::{InspectCommand, InspectError, Introspect};
    use crate::machine::{
        CpuExit, Machine, MachineCreateError, MachineError, MachineFactory, StateDigest,
    };
    use crate::save::{Saveable, StateError, StateReader, StateWriter};

    use super::{
        BuildFingerprint, CONTAINER_VERSION, ComponentKey, ComponentKeyError, MAGIC,
        ProfileFingerprint, SnapshotComponent, SnapshotError, SnapshotManifestBuilder,
        SnapshotTarget, decode_snapshot, encode_snapshot, restore_machine, write_snapshot,
    };

    const BUILD: BuildFingerprint = BuildFingerprint::from_bytes([0x11; 32]);
    const PROFILE: ProfileFingerprint = ProfileFingerprint::from_bytes([0x22; 32]);

    struct FlushFailingOutput {
        inner: Cursor<Vec<u8>>,
    }

    impl FlushFailingOutput {
        fn new() -> Self {
            Self {
                inner: Cursor::new(Vec::new()),
            }
        }
    }

    impl Read for FlushFailingOutput {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            self.inner.read(buffer)
        }
    }

    impl Write for FlushFailingOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.inner.write(bytes)
        }

        fn flush(&mut self) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "injected snapshot flush failure",
            ))
        }
    }

    impl Seek for FlushFailingOutput {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.inner.seek(position)
        }
    }

    struct FlushPublishedOutput {
        logical: Vec<u8>,
        readable: Vec<u8>,
        position: u64,
        flush_count: u32,
    }

    impl FlushPublishedOutput {
        fn new() -> Self {
            Self {
                logical: Vec::new(),
                readable: Vec::new(),
                position: 0,
                flush_count: 0,
            }
        }

        fn into_readable(self) -> Vec<u8> {
            self.readable
        }

        fn offset_position(base: u64, offset: i64) -> io::Result<u64> {
            let position = if offset < 0 {
                base.checked_sub(offset.unsigned_abs())
            } else {
                base.checked_add(offset.unsigned_abs())
            };
            position.ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "seek position is out of range")
            })
        }
    }

    impl Read for FlushPublishedOutput {
        fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
            let readable_len = u64::try_from(self.readable.len()).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "readable length exceeds u64")
            })?;
            if self.position >= readable_len {
                return Ok(0);
            }
            let start = usize::try_from(self.position).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "read position exceeds usize")
            })?;
            let count = output.len().min(self.readable.len() - start);
            output[..count].copy_from_slice(&self.readable[start..start + count]);
            self.position += count as u64;
            Ok(count)
        }
    }

    impl Write for FlushPublishedOutput {
        fn write(&mut self, input: &[u8]) -> io::Result<usize> {
            let start = usize::try_from(self.position).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "write position exceeds usize")
            })?;
            let end = start.checked_add(input.len()).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "write range exceeds usize")
            })?;
            if end > self.logical.len() {
                self.logical.resize(end, 0);
            }
            self.logical[start..end].copy_from_slice(input);
            self.position = u64::try_from(end).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "write position exceeds u64")
            })?;
            Ok(input.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            self.readable.clone_from(&self.logical);
            self.flush_count += 1;
            Ok(())
        }
    }

    impl Seek for FlushPublishedOutput {
        fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
            self.position = match position {
                SeekFrom::Start(position) => position,
                SeekFrom::End(offset) => {
                    let end = u64::try_from(self.logical.len()).map_err(|_| {
                        io::Error::new(io::ErrorKind::InvalidInput, "stream length exceeds u64")
                    })?;
                    Self::offset_position(end, offset)?
                }
                SeekFrom::Current(offset) => Self::offset_position(self.position, offset)?,
            };
            Ok(self.position)
        }
    }

    struct MockMachine {
        manifest: Vec<SnapshotComponent>,
        values: Vec<u64>,
        now: u64,
        load_count: Rc<Cell<u32>>,
        validation_fails: bool,
    }

    impl MockMachine {
        fn new(
            keys: &[&str],
            maximums: &[u64],
            values: &[u64],
            load_count: Rc<Cell<u32>>,
            validation_fails: bool,
        ) -> Self {
            let manifest = keys
                .iter()
                .zip(maximums)
                .map(|(key, maximum)| SnapshotComponent {
                    key: ComponentKey::new(*key).unwrap(),
                    schema_version: 1,
                    max_payload_len: *maximum,
                })
                .collect();
            Self {
                manifest,
                values: values.to_vec(),
                now: 0,
                load_count,
                validation_fails,
            }
        }

        fn component_index(&self, key: &ComponentKey) -> Result<usize, StateError> {
            self.manifest
                .binary_search_by(|component| component.key.cmp(key))
                .map_err(|_| StateError::UnknownComponent(key.to_string()))
        }
    }

    impl SnapshotTarget for MockMachine {
        fn snapshot_components(&self) -> &[SnapshotComponent] {
            &self.manifest
        }

        fn save_component(
            &self,
            key: &ComponentKey,
            writer: &mut StateWriter<'_>,
        ) -> Result<(), StateError> {
            let index = self.component_index(key)?;
            writer.serialize(&self.values[index])
        }

        fn load_component(
            &mut self,
            key: &ComponentKey,
            version: u32,
            reader: &mut StateReader<'_>,
        ) -> Result<(), StateError> {
            if version != 1 {
                return Err(StateError::UnsupportedVersion(version));
            }
            let index = self.component_index(key)?;
            let value: u64 = reader.deserialize()?;
            if value == u64::MAX {
                return Err(StateError::InvalidState(
                    "mock value uses a reserved sentinel".to_owned(),
                ));
            }
            self.values[index] = value;
            self.load_count.set(self.load_count.get() + 1);
            Ok(())
        }

        fn validate_loaded_snapshot(&self) -> Result<(), StateError> {
            if self.validation_fails {
                return Err(StateError::InvalidState(
                    "mock cross-component validation failed".to_owned(),
                ));
            }
            Ok(())
        }
    }

    impl Introspect for MockMachine {
        fn commands(&self) -> &[InspectCommand] {
            &[]
        }

        fn execute(
            &mut self,
            command: &str,
            _arguments: &[&str],
            _output: &mut dyn FmtWrite,
        ) -> Result<(), InspectError> {
            Err(InspectError::UnknownCommand(command.to_owned()))
        }
    }

    impl Machine for MockMachine {
        fn now(&self) -> u64 {
            self.now
        }

        fn front_event_time(&mut self) -> Option<u64> {
            None
        }

        fn run_cpu_until(&mut self, deadline: u64) -> Result<CpuExit, MachineError> {
            self.now = deadline;
            Ok(CpuExit::Deadline)
        }

        fn pop_event(&mut self) -> Result<Option<ScheduledEvent>, MachineError> {
            Ok(None)
        }

        fn dispatch_event(&mut self, _event: ScheduledEvent) -> Result<(), MachineError> {
            Ok(())
        }

        fn state_digest(&self) -> Result<StateDigest, MachineError> {
            let mut hasher = blake3::Hasher::new();
            hasher.update(&self.now.to_le_bytes());
            for value in &self.values {
                hasher.update(&value.to_le_bytes());
            }
            Ok(StateDigest::from_bytes(*hasher.finalize().as_bytes()))
        }
    }

    struct MockFactory {
        profile: ProfileFingerprint,
        keys: Vec<&'static str>,
        maximums: Vec<u64>,
        initial_values: Vec<u64>,
        create_count: Rc<Cell<u32>>,
        load_count: Rc<Cell<u32>>,
        validation_fails: bool,
    }

    impl MockFactory {
        fn empty() -> Self {
            Self {
                profile: PROFILE,
                keys: Vec::new(),
                maximums: Vec::new(),
                initial_values: Vec::new(),
                create_count: Rc::new(Cell::new(0)),
                load_count: Rc::new(Cell::new(0)),
                validation_fails: false,
            }
        }

        fn two_components() -> Self {
            Self {
                profile: PROFILE,
                keys: vec!["core/aaaa", "core/bbbb"],
                maximums: vec![16, 16],
                initial_values: vec![0, 0],
                create_count: Rc::new(Cell::new(0)),
                load_count: Rc::new(Cell::new(0)),
                validation_fails: false,
            }
        }

        fn machine(&self, values: &[u64]) -> MockMachine {
            MockMachine::new(
                &self.keys,
                &self.maximums,
                values,
                Rc::clone(&self.load_count),
                false,
            )
        }
    }

    impl MachineFactory for MockFactory {
        fn profile_fingerprint(&self) -> ProfileFingerprint {
            self.profile
        }

        fn create(&self) -> Result<Box<dyn Machine>, MachineCreateError> {
            self.create_count.set(self.create_count.get() + 1);
            Ok(Box::new(MockMachine::new(
                &self.keys,
                &self.maximums,
                &self.initial_values,
                Rc::clone(&self.load_count),
                self.validation_fails,
            )))
        }
    }

    struct Versioned(u32);

    impl Saveable for Versioned {
        fn snapshot_version(&self) -> u32 {
            self.0
        }

        fn save(&self, writer: &mut StateWriter<'_>) -> Result<(), StateError> {
            writer.serialize(&())
        }

        fn load(&mut self, version: u32, reader: &mut StateReader<'_>) -> Result<(), StateError> {
            if version != self.0 {
                return Err(StateError::UnsupportedVersion(version));
            }
            reader.deserialize::<()>()
        }
    }

    struct RawRecord<'a> {
        key: &'a str,
        version: u32,
        payload: Vec<u8>,
        declared_len: Option<u64>,
    }

    fn raw_snapshot(
        build: BuildFingerprint,
        profile: ProfileFingerprint,
        records: &[RawRecord<'_>],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&CONTAINER_VERSION.to_le_bytes());
        bytes.extend_from_slice(build.as_bytes());
        bytes.extend_from_slice(profile.as_bytes());
        bytes.extend_from_slice(&(records.len() as u32).to_le_bytes());
        for record in records {
            bytes.extend_from_slice(&(record.key.len() as u32).to_le_bytes());
            bytes.extend_from_slice(record.key.as_bytes());
            bytes.extend_from_slice(&record.version.to_le_bytes());
            bytes.extend_from_slice(
                &record
                    .declared_len
                    .unwrap_or(record.payload.len() as u64)
                    .to_le_bytes(),
            );
            bytes.extend_from_slice(&record.payload);
        }
        let checksum = blake3::hash(&bytes);
        bytes.extend_from_slice(checksum.as_bytes());
        bytes
    }

    fn encoded(value: u64) -> Vec<u8> {
        postcard::to_stdvec(&value).unwrap()
    }

    #[test]
    fn component_keys_and_manifest_are_canonical() {
        assert!(ComponentKey::new("cpu/0").is_ok());
        assert!(matches!(
            ComponentKey::new("/cpu/0"),
            Err(ComponentKeyError::NonCanonicalPath)
        ));
        assert!(matches!(
            ComponentKey::new("cpu//0"),
            Err(ComponentKeyError::NonCanonicalPath)
        ));
        assert!(matches!(
            ComponentKey::new("cpu/\u{96f6}"),
            Err(ComponentKeyError::NonAscii)
        ));

        let component = Versioned(1);
        let mut builder = SnapshotManifestBuilder::new();
        builder
            .bind(ComponentKey::new("memory/main").unwrap(), &component, 100)
            .unwrap();
        builder
            .bind(ComponentKey::new("core/events").unwrap(), &component, 200)
            .unwrap();
        let manifest = builder.finish().unwrap();
        assert_eq!(manifest[0].key.as_str(), "core/events");
        assert_eq!(manifest[1].key.as_str(), "memory/main");

        let mut unsupported = SnapshotManifestBuilder::new();
        assert!(
            unsupported
                .bind(ComponentKey::new("core/bad").unwrap(), &Versioned(2), 1)
                .is_err()
        );
    }

    #[test]
    fn same_state_has_deterministic_bytes_and_round_trips() {
        let factory = MockFactory::two_components();
        let original = factory.machine(&[7, 42]);
        let first = encode_snapshot(&original, BUILD, PROFILE).unwrap();
        let second = encode_snapshot(&original, BUILD, PROFILE).unwrap();
        assert_eq!(first, second);

        let restored = decode_snapshot(&first, BUILD, &factory).unwrap();
        assert_eq!(factory.create_count.get(), 1);
        assert_eq!(factory.load_count.get(), 2);
        assert_eq!(
            encode_snapshot(restored.as_ref(), BUILD, PROFILE).unwrap(),
            first
        );
    }

    #[test]
    fn fingerprints_are_rejected_before_creation_or_payload_decode() {
        let source_factory = MockFactory::two_components();
        let bytes = encode_snapshot(&source_factory.machine(&[1, 2]), BUILD, PROFILE).unwrap();

        let build_factory = MockFactory::two_components();
        assert!(matches!(
            decode_snapshot(
                &bytes,
                BuildFingerprint::from_bytes([0x33; 32]),
                &build_factory
            ),
            Err(SnapshotError::BuildFingerprintMismatch)
        ));
        assert_eq!(build_factory.create_count.get(), 0);
        assert_eq!(build_factory.load_count.get(), 0);

        let mut profile_factory = MockFactory::two_components();
        profile_factory.profile = ProfileFingerprint::from_bytes([0x44; 32]);
        assert!(matches!(
            decode_snapshot(&bytes, BUILD, &profile_factory),
            Err(SnapshotError::ProfileFingerprintMismatch)
        ));
        assert_eq!(profile_factory.create_count.get(), 0);
        assert_eq!(profile_factory.load_count.get(), 0);
    }

    #[test]
    fn alpha_accepts_only_container_and_component_version_one() {
        let factory = MockFactory::two_components();
        let mut wrong_container =
            encode_snapshot(&factory.machine(&[1, 2]), BUILD, PROFILE).unwrap();
        wrong_container[8..12].copy_from_slice(&2_u32.to_le_bytes());
        assert!(matches!(
            decode_snapshot(&wrong_container, BUILD, &factory),
            Err(SnapshotError::UnsupportedContainerVersion(2))
        ));

        let wrong_component = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/aaaa",
                    version: 2,
                    payload: encoded(1),
                    declared_len: None,
                },
                RawRecord {
                    key: "core/bbbb",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
            ],
        );
        let component_factory = MockFactory::two_components();
        assert!(matches!(
            decode_snapshot(&wrong_component, BUILD, &component_factory),
            Err(SnapshotError::UnsupportedComponentVersion { version: 2, .. })
        ));
        assert_eq!(component_factory.load_count.get(), 0);
    }

    #[test]
    fn duplicate_missing_extra_and_wrong_order_records_are_rejected() {
        let duplicate = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: encoded(1),
                    declared_len: None,
                },
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
            ],
        );
        assert!(matches!(
            decode_snapshot(&duplicate, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::DuplicateComponent(_))
        ));

        let wrong_order = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/bbbb",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: encoded(1),
                    declared_len: None,
                },
            ],
        );
        assert!(matches!(
            decode_snapshot(&wrong_order, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::ComponentKeyMismatch { .. })
        ));

        let missing = raw_snapshot(
            BUILD,
            PROFILE,
            &[RawRecord {
                key: "core/aaaa",
                version: 1,
                payload: encoded(1),
                declared_len: None,
            }],
        );
        assert!(matches!(
            decode_snapshot(&missing, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::ComponentCountMismatch {
                found: 1,
                expected: 2
            })
        ));

        let extra = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: encoded(1),
                    declared_len: None,
                },
                RawRecord {
                    key: "core/bbbb",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
                RawRecord {
                    key: "core/cccc",
                    version: 1,
                    payload: encoded(3),
                    declared_len: None,
                },
            ],
        );
        assert!(matches!(
            decode_snapshot(&extra, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::ComponentCountMismatch {
                found: 3,
                expected: 2
            })
        ));
    }

    #[test]
    fn payload_bounds_malformed_roots_and_trailing_bytes_are_rejected() {
        let too_large = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: encoded(1),
                    declared_len: None,
                },
                RawRecord {
                    key: "core/bbbb",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
            ],
        );
        let mut limited = MockFactory::two_components();
        limited.maximums[0] = 0;
        assert!(matches!(
            decode_snapshot(&too_large, BUILD, &limited),
            Err(SnapshotError::PayloadTooLarge { .. })
        ));
        assert_eq!(limited.load_count.get(), 0);

        let malformed = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: vec![0x80],
                    declared_len: None,
                },
                RawRecord {
                    key: "core/bbbb",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
            ],
        );
        assert!(matches!(
            decode_snapshot(&malformed, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::ComponentState {
                error: StateError::Decode(_),
                ..
            })
        ));

        let trailing = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: vec![1, 0],
                    declared_len: None,
                },
                RawRecord {
                    key: "core/bbbb",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
            ],
        );
        assert!(matches!(
            decode_snapshot(&trailing, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::ComponentState {
                error: StateError::TrailingBytes(1),
                ..
            })
        ));
    }

    #[test]
    fn snapshot_write_stops_at_the_manifest_payload_limit() {
        let mut factory = MockFactory::two_components();
        factory.maximums[0] = 0;
        let mut output = Cursor::new(Vec::new());
        let error =
            write_snapshot(&factory.machine(&[1, 2]), BUILD, PROFILE, &mut output).unwrap_err();
        assert!(matches!(
            error,
            SnapshotError::PayloadTooLarge {
                ref key,
                actual: 1,
                maximum: 0
            } if key.as_str() == "core/aaaa"
        ));

        let payload_start = MAGIC.len()
            + size_of::<u32>()
            + BUILD.as_bytes().len()
            + PROFILE.as_bytes().len()
            + size_of::<u32>()
            + size_of::<u32>()
            + "core/aaaa".len()
            + size_of::<u32>()
            + size_of::<u64>();
        assert_eq!(output.into_inner().len(), payload_start);
    }

    #[test]
    fn snapshot_preserves_component_sink_io_errors() {
        let factory = MockFactory::two_components();
        let mut output = FlushFailingOutput::new();
        match write_snapshot(&factory.machine(&[1, 2]), BUILD, PROFILE, &mut output) {
            Err(SnapshotError::Io(error)) => {
                assert_eq!(error.kind(), io::ErrorKind::BrokenPipe);
                assert_eq!(error.to_string(), "injected snapshot flush failure");
            }
            result => panic!("unexpected snapshot result: {result:?}"),
        }
    }

    #[test]
    fn snapshot_flushes_framed_writes_before_checksum_read() {
        let factory = MockFactory::empty();
        let mut output = FlushPublishedOutput::new();

        write_snapshot(&factory.machine(&[]), BUILD, PROFILE, &mut output).unwrap();

        assert_eq!(output.flush_count, 2);
        let snapshot = output.into_readable();
        assert!(decode_snapshot(&snapshot, BUILD, &factory).is_ok());
    }

    #[test]
    fn truncation_integrity_failure_and_file_tail_are_rejected() {
        let truncated_payload = raw_snapshot(
            BUILD,
            PROFILE,
            &[
                RawRecord {
                    key: "core/aaaa",
                    version: 1,
                    payload: encoded(1),
                    declared_len: Some(100),
                },
                RawRecord {
                    key: "core/bbbb",
                    version: 1,
                    payload: encoded(2),
                    declared_len: None,
                },
            ],
        );
        let mut generous = MockFactory::two_components();
        generous.maximums[0] = 200;
        assert!(matches!(
            decode_snapshot(&truncated_payload, BUILD, &generous),
            Err(SnapshotError::Io(error)) if error.kind() == std::io::ErrorKind::UnexpectedEof
        ));

        let factory = MockFactory::two_components();
        let mut corrupt = encode_snapshot(&factory.machine(&[1, 2]), BUILD, PROFILE).unwrap();
        let payload_byte = corrupt.len() - 33;
        corrupt[payload_byte] ^= 0x40;
        assert!(matches!(
            decode_snapshot(&corrupt, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::IntegrityMismatch) | Err(SnapshotError::ComponentState { .. })
        ));

        let mut tailed = encode_snapshot(&factory.machine(&[1, 2]), BUILD, PROFILE).unwrap();
        tailed.extend_from_slice(&[1, 2, 3]);
        assert!(matches!(
            decode_snapshot(&tailed, BUILD, &MockFactory::two_components()),
            Err(SnapshotError::TrailingData(3))
        ));
    }

    #[test]
    fn failed_load_never_replaces_the_running_machine() {
        let factory = MockFactory::two_components();
        let source = factory.machine(&[7, 8]);
        let valid = encode_snapshot(&source, BUILD, PROFILE).unwrap();
        let mut invalid = valid.clone();
        let last = invalid.len() - 1;
        invalid[last] ^= 1;

        let running_factory = MockFactory::two_components();
        let mut running: Box<dyn Machine> = Box::new(running_factory.machine(&[90, 91]));
        let before = running.state_digest().unwrap();
        assert!(restore_machine(&mut running, &invalid, BUILD, &factory).is_err());
        assert_eq!(running.state_digest().unwrap(), before);

        restore_machine(&mut running, &valid, BUILD, &factory).unwrap();
        assert_eq!(
            running.state_digest().unwrap(),
            source.state_digest().unwrap()
        );
    }

    #[test]
    fn cross_component_validation_runs_after_integrity_check() {
        let source_factory = MockFactory::two_components();
        let bytes = encode_snapshot(&source_factory.machine(&[1, 2]), BUILD, PROFILE).unwrap();
        let mut failing_factory = MockFactory::two_components();
        failing_factory.validation_fails = true;
        assert!(matches!(
            decode_snapshot(&bytes, BUILD, &failing_factory),
            Err(SnapshotError::MachineValidation(_))
        ));
        assert_eq!(failing_factory.load_count.get(), 2);
    }

    #[test]
    fn object_safe_snapshot_and_factory_surfaces_are_usable() {
        fn accept_target(_target: &dyn SnapshotTarget) {}
        fn accept_factory(_factory: &dyn MachineFactory) {}

        let factory = MockFactory::two_components();
        let machine = factory.machine(&[1, 2]);
        accept_target(&machine);
        accept_factory(&factory);
    }
}
