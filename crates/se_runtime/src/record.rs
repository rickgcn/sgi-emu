//! Deterministic cold-start recording and replay support.
//!
//! A record stores the cold machine configuration, bytes accepted by the
//! emulated serial controller at exact instruction boundaries, sparse CPU
//! checkpoints, and disk before-images. It is not a save state: replay always
//! constructs a new machine and starts at the first PROM instruction.
//!
//! Recording writes synchronously. A complete file is renamed from
//! `.serec.partial` only after its footer has been flushed and synchronized.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crc32fast::hash as crc32;
use se_machine::serial::SerialPort;
use sha2::{Digest, Sha256};

#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW,
};

const FILE_MAGIC: [u8; 8] = *b"SGISEREC";
const FORMAT_VERSION: u32 = 1;
const FILE_HEADER_BYTES: usize = 12;
const FRAME_HEADER_BYTES: usize = 9;
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_STRING_BYTES: usize = 1024 * 1024;

/// Granularity used for writable-disk before-images and replay COW pages.
pub const DISK_PAGE_BYTES: usize = 4096;

const FRAME_MANIFEST: u8 = 1;
const FRAME_SERIAL_BYTE: u8 = 2;
const FRAME_RESET: u8 = 3;
const FRAME_CHECKPOINT: u8 = 4;
const FRAME_DISK_BEFORE_IMAGE: u8 = 5;
const FRAME_FOOTER: u8 = 255;

/// Deterministic boundary immediately before the next guest instruction.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct ExecutionPosition {
    /// Reset epoch, beginning at zero for cold power-on.
    pub epoch: u64,
    /// Instructions completed within the current epoch.
    pub completed_instructions: u64,
}

/// Stable content identity for one configured host medium.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaIdentity {
    /// Original path used as a user-facing hint only.
    pub path_hint: String,
    /// Exact medium size in bytes.
    pub size_bytes: u64,
    /// SHA-256 of the complete medium contents.
    pub sha256: [u8; 32],
}

impl MediaIdentity {
    /// Hashes an already-open host file and restores its original seek
    /// position.
    ///
    /// # Errors
    ///
    /// Returns the host I/O error when metadata, seeking, or reading fails.
    pub fn from_file(path_hint: &Path, file: &mut File) -> io::Result<Self> {
        let original_position = file.stream_position()?;
        let result = (|| {
            let size_bytes = file.metadata()?.len();
            file.seek(SeekFrom::Start(0))?;
            let mut hasher = Sha256::new();
            let mut buffer = [0; 128 * 1024];
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(Self {
                path_hint: path_hint.to_string_lossy().into_owned(),
                size_bytes,
                sha256: hasher.finalize().into(),
            })
        })();
        let restore_result = file.seek(SeekFrom::Start(original_position));
        match (result, restore_result) {
            (Ok(identity), Ok(_)) => Ok(identity),
            (Err(error), _) | (Ok(_), Err(error)) => Err(error),
        }
    }

    /// Computes an identity for an in-memory read-only medium.
    #[must_use]
    pub fn from_bytes(path_hint: &Path, bytes: &[u8]) -> Self {
        Self {
            path_hint: path_hint.to_string_lossy().into_owned(),
            size_bytes: bytes.len() as u64,
            sha256: Sha256::digest(bytes).into(),
        }
    }
}

/// Complete immutable configuration recorded before the first instruction.
///
/// Machine-specific nonvolatile state remains opaque to the Record container
/// and is encoded and decoded by the application that constructs the machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordManifest {
    machine_model: String,
    float_backend: String,
    prom: MediaIdentity,
    disk: Option<MediaIdentity>,
    cdrom: Option<MediaIdentity>,
    nonvolatile_state: Vec<u8>,
}

impl RecordManifest {
    /// Creates a complete cold-start manifest.
    #[must_use]
    pub fn new(
        machine_model: String,
        float_backend: String,
        prom: MediaIdentity,
        disk: Option<MediaIdentity>,
        cdrom: Option<MediaIdentity>,
        nonvolatile_state: Vec<u8>,
    ) -> Self {
        Self {
            machine_model,
            float_backend,
            prom,
            disk,
            cdrom,
            nonvolatile_state,
        }
    }

    /// Returns the stable configured machine identifier.
    #[must_use]
    pub fn machine_model(&self) -> &str {
        &self.machine_model
    }

    /// Returns the stable floating-point backend identifier.
    #[must_use]
    pub fn float_backend(&self) -> &str {
        &self.float_backend
    }

    /// Returns the recorded PROM identity.
    #[must_use]
    pub const fn prom(&self) -> &MediaIdentity {
        &self.prom
    }

    /// Returns the recorded writable-disk identity, when one was attached.
    #[must_use]
    pub const fn disk(&self) -> Option<&MediaIdentity> {
        self.disk.as_ref()
    }

    /// Returns the recorded CD-ROM identity, when one was attached.
    #[must_use]
    pub const fn cdrom(&self) -> Option<&MediaIdentity> {
        self.cdrom.as_ref()
    }

    /// Returns the opaque, machine-specific cold-start nonvolatile state.
    #[must_use]
    pub fn nonvolatile_state_bytes(&self) -> &[u8] {
        &self.nonvolatile_state
    }
}

/// Error returned by Record or Replay file operations.
#[derive(Debug)]
pub enum RecordError {
    /// Host file I/O failed.
    Io(io::Error),
    /// The record is malformed, incomplete, unsupported, or inconsistent.
    InvalidData(String),
}

impl fmt::Display for RecordError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => error.fmt(formatter),
            Self::InvalidData(reason) => formatter.write_str(reason),
        }
    }
}

impl Error for RecordError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::InvalidData(_) => None,
        }
    }
}

impl From<io::Error> for RecordError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Shared append-only writer used by the runtime and writable-disk adapter.
#[derive(Clone)]
pub struct Recorder {
    inner: Arc<Mutex<RecorderInner>>,
    failed: Arc<AtomicBool>,
}

impl Recorder {
    /// Creates a new `.serec.partial` file.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when the destination exists or cannot be
    /// created and initialized.
    pub fn create(path: impl AsRef<Path>) -> Result<Self, RecordError> {
        Self::create_inner(path.as_ref(), false)
    }

    /// Creates a new `.serec.partial` file that may replace its destination.
    ///
    /// The existing complete record remains untouched until the replacement
    /// has a synchronized footer and is ready to commit. An existing partial
    /// file is never replaced.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when the partial file already exists or the
    /// writer cannot be created and initialized.
    pub fn create_or_replace(path: impl AsRef<Path>) -> Result<Self, RecordError> {
        Self::create_inner(path.as_ref(), true)
    }

    fn create_inner(path: &Path, replace_existing: bool) -> Result<Self, RecordError> {
        let final_path = normalized_record_path(path);
        if !replace_existing && final_path.exists() {
            return Err(RecordError::Io(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("record already exists: {}", final_path.display()),
            )));
        }
        let partial_path = partial_record_path(&final_path);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&partial_path)
            .map_err(|error| {
                if error.kind() == io::ErrorKind::AlreadyExists {
                    RecordError::Io(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("partial record already exists: {}", partial_path.display()),
                    ))
                } else {
                    RecordError::Io(error)
                }
            })?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&FILE_MAGIC)?;
        writer.write_all(&FORMAT_VERSION.to_le_bytes())?;
        Ok(Self {
            inner: Arc::new(Mutex::new(RecorderInner {
                writer: Some(writer),
                final_path,
                partial_path,
                replace_existing,
                captured_pages: BTreeSet::new(),
                started: false,
                active: true,
                failure: None,
            })),
            failed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Writes the complete manifest before guest execution begins.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when the writer is inactive, the manifest was
    /// already written, or synchronous file output fails.
    pub fn start(&self, manifest: &RecordManifest) -> Result<(), RecordError> {
        self.update(|inner| {
            if inner.started {
                return Err(invalid_record("record manifest was already written"));
            }
            write_frame(inner, FRAME_MANIFEST, &encode_manifest(manifest)?)?;
            inner.started = true;
            Ok(())
        })
    }

    /// Creates the capability used by the writable-disk host adapter.
    #[must_use]
    pub fn disk(&self) -> RecordDisk {
        RecordDisk {
            recorder: self.clone(),
        }
    }

    pub(crate) fn record_serial_byte(
        &self,
        position: ExecutionPosition,
        port: SerialPort,
        value: u8,
    ) -> Result<(), RecordError> {
        self.append(FRAME_SERIAL_BYTE, encode_serial_byte(position, port, value))
    }

    pub(crate) fn record_reset(&self, position: ExecutionPosition) -> Result<(), RecordError> {
        self.append(FRAME_RESET, encode_reset(position))
    }

    pub(crate) fn record_checkpoint(
        &self,
        position: ExecutionPosition,
        digest: [u8; 32],
    ) -> Result<(), RecordError> {
        self.append(FRAME_CHECKPOINT, encode_checkpoint(position, digest))
    }

    pub(crate) fn failure(&self) -> Option<String> {
        self.failed
            .load(Ordering::Acquire)
            .then(|| lock_unpoisoned(&self.inner).failure.clone())
            .flatten()
    }

    pub(crate) fn disable(&self) {
        let mut inner = lock_unpoisoned(&self.inner);
        inner.active = false;
        inner.writer.take();
    }

    pub(crate) fn finalize(
        &self,
        position: ExecutionPosition,
        outcome: &RecordOutcome,
    ) -> Result<(), RecordError> {
        self.update(|inner| {
            require_active(inner)?;
            write_frame(inner, FRAME_FOOTER, &encode_footer(position, outcome)?)?;
            let mut writer = inner
                .writer
                .take()
                .ok_or_else(|| invalid_record("record writer is unavailable"))?;
            writer.flush()?;
            writer.get_ref().sync_all()?;
            drop(writer);
            commit_record_file(
                &inner.partial_path,
                &inner.final_path,
                inner.replace_existing,
            )?;
            inner.active = false;
            Ok(())
        })
    }

    fn append(&self, kind: u8, payload: Vec<u8>) -> Result<(), RecordError> {
        self.update(|inner| {
            require_active(inner)?;
            write_frame(inner, kind, &payload)
        })
    }

    fn update<T>(
        &self,
        operation: impl FnOnce(&mut RecorderInner) -> Result<T, RecordError>,
    ) -> Result<T, RecordError> {
        let mut inner = lock_unpoisoned(&self.inner);
        if let Some(failure) = &inner.failure {
            return Err(invalid_record(format!(
                "recording already failed: {failure}"
            )));
        }
        let result = operation(&mut inner);
        if let Err(error) = &result {
            inner.failure = Some(error.to_string());
            self.failed.store(true, Ordering::Release);
        }
        result
    }
}

/// Capability used by the application-owned writable-disk adapter.
#[derive(Clone)]
pub struct RecordDisk {
    recorder: Recorder,
}

/// Parsed complete record. Mutable Timeline state moves into the worker.
pub struct Replayer {
    manifest: RecordManifest,
    timeline: Vec<TimelineEntry>,
    footer: RecordFooter,
    storage: ReplayStorageHandle,
}

impl Replayer {
    /// Opens, validates, and indexes a complete `.serec` file.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] for I/O failures or malformed/incomplete data.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RecordError> {
        let path = path.as_ref();
        if path.extension().and_then(|extension| extension.to_str()) == Some("partial") {
            return Err(invalid_record("partial records cannot be replayed"));
        }
        parse_record(File::open(path)?)
    }

    /// Returns the complete validated Manifest.
    #[must_use]
    pub const fn manifest(&self) -> &RecordManifest {
        &self.manifest
    }

    /// Creates the capability used by the Replay disk adapter.
    #[must_use]
    pub fn disk(&self) -> ReplayDisk {
        ReplayDisk {
            storage: Arc::clone(&self.storage),
        }
    }

    pub(crate) fn into_session(self) -> ReplaySession {
        ReplaySession {
            timeline: self.timeline,
            cursor: 0,
            footer: self.footer,
            storage: self.storage,
        }
    }
}

/// Capability providing the initial disk overlay and ephemeral Replay COW.
#[derive(Clone)]
pub struct ReplayDisk {
    storage: ReplayStorageHandle,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct TimelineEntry {
    pub(crate) position: ExecutionPosition,
    pub(crate) action: TimelineAction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum TimelineAction {
    SerialByte { port: SerialPort, value: u8 },
    Reset,
    Checkpoint { digest: [u8; 32] },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum RecordOutcome {
    UserStopped,
    Shutdown,
    ExecutionError { address: u32, description: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RecordFooter {
    position: ExecutionPosition,
    outcome: RecordOutcome,
}

pub(crate) struct ReplaySession {
    timeline: Vec<TimelineEntry>,
    cursor: usize,
    footer: RecordFooter,
    storage: ReplayStorageHandle,
}

impl ReplaySession {
    pub(crate) fn next_entry(&self) -> Option<&TimelineEntry> {
        self.timeline.get(self.cursor)
    }

    pub(crate) fn next_boundary_position(&self) -> ExecutionPosition {
        self.next_entry().map_or(self.footer.position, |entry| {
            entry.position.min(self.footer.position)
        })
    }

    pub(crate) fn advance(&mut self) {
        self.cursor += 1;
    }

    pub(crate) const fn final_position(&self) -> ExecutionPosition {
        self.footer.position
    }

    pub(crate) const fn outcome(&self) -> &RecordOutcome {
        &self.footer.outcome
    }

    pub(crate) fn timeline_consumed(&self) -> bool {
        self.cursor == self.timeline.len()
    }

    pub(crate) fn storage_failure(&self) -> Option<String> {
        self.storage
            .failed
            .load(Ordering::Acquire)
            .then(|| lock_unpoisoned(&self.storage.state).failure.clone())
            .flatten()
    }
}

struct RecorderInner {
    writer: Option<BufWriter<File>>,
    final_path: PathBuf,
    partial_path: PathBuf,
    replace_existing: bool,
    captured_pages: BTreeSet<u64>,
    started: bool,
    active: bool,
    failure: Option<String>,
}

type ReplayStorageHandle = Arc<ReplayStorage>;

struct ReplayStorage {
    state: Mutex<ReplayStorageState>,
    failed: AtomicBool,
}

struct ReplayStorageState {
    file: File,
    before_images: BTreeMap<u64, BeforeImageLocation>,
    cow_pages: BTreeMap<u64, Vec<u8>>,
    failure: Option<String>,
}

#[derive(Clone, Copy)]
struct BeforeImageLocation {
    data_offset: u64,
    length: usize,
    zero: bool,
}

fn require_active(inner: &RecorderInner) -> Result<(), RecordError> {
    if inner.started && inner.active && inner.writer.is_some() {
        Ok(())
    } else {
        Err(invalid_record("record writer is not active"))
    }
}

fn invalid_record(reason: impl Into<String>) -> RecordError {
    RecordError::InvalidData(reason.into())
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn parse_record(mut file: File) -> Result<Replayer, RecordError> {
    let file_length = file.metadata()?.len();
    let mut header = [0; FILE_HEADER_BYTES];
    file.read_exact(&mut header)?;
    if header[..8] != FILE_MAGIC {
        return Err(invalid_record("invalid Record file magic"));
    }
    if u32::from_le_bytes(header[8..12].try_into().unwrap()) != FORMAT_VERSION {
        return Err(invalid_record("unsupported Record format version"));
    }

    let mut manifest = None;
    let mut timeline = Vec::new();
    let mut before_images = BTreeMap::new();
    let mut footer = None;
    let mut last_position = None;
    let mut frame_index = 0_usize;

    while file.stream_position()? < file_length {
        if footer.is_some() {
            return Err(invalid_record("Record contains data after its footer"));
        }
        let mut frame_header = [0; FRAME_HEADER_BYTES];
        file.read_exact(&mut frame_header)?;
        let kind = frame_header[0];
        let payload_length =
            usize::try_from(u32::from_le_bytes(frame_header[1..5].try_into().unwrap()))
                .map_err(|_| invalid_record("Record frame length does not fit usize"))?;
        if payload_length > MAX_FRAME_BYTES {
            return Err(invalid_record("Record frame exceeds the size limit"));
        }
        let expected_crc = u32::from_le_bytes(frame_header[5..9].try_into().unwrap());
        let payload_offset = file.stream_position()?;
        let mut payload = vec![0; payload_length];
        file.read_exact(&mut payload)?;
        if crc32(&payload) != expected_crc {
            return Err(invalid_record("Record frame CRC mismatch"));
        }

        let mut reader = SliceReader::new(&payload);
        match kind {
            FRAME_MANIFEST => {
                if frame_index != 0 || manifest.is_some() {
                    return Err(invalid_record(
                        "Record manifest must be the unique first frame",
                    ));
                }
                manifest = Some(decode_manifest(&mut reader)?);
            }
            FRAME_SERIAL_BYTE => {
                require_manifest(&manifest)?;
                let position = reader.position()?;
                update_position(&mut last_position, position)?;
                timeline.push(TimelineEntry {
                    position,
                    action: TimelineAction::SerialByte {
                        port: decode_serial_port(reader.u8()?)?,
                        value: reader.u8()?,
                    },
                });
            }
            FRAME_RESET => {
                require_manifest(&manifest)?;
                let position = reader.position()?;
                update_position(&mut last_position, position)?;
                timeline.push(TimelineEntry {
                    position,
                    action: TimelineAction::Reset,
                });
            }
            FRAME_CHECKPOINT => {
                require_manifest(&manifest)?;
                let position = reader.position()?;
                update_position(&mut last_position, position)?;
                timeline.push(TimelineEntry {
                    position,
                    action: TimelineAction::Checkpoint {
                        digest: reader.array_32()?,
                    },
                });
            }
            FRAME_DISK_BEFORE_IMAGE => {
                let manifest = require_manifest(&manifest)?;
                let disk = manifest.disk().ok_or_else(|| {
                    invalid_record("Record has a disk before-image without a disk")
                })?;
                let page_index = reader.u64()?;
                let length = usize::from(reader.u16()?);
                validate_before_image(page_index, length, disk.size_bytes)?;
                let encoding = reader.u8()?;
                let (zero, data_offset) = match encoding {
                    0 => {
                        if reader.remaining() != length {
                            return Err(invalid_record(
                                "raw disk before-image has the wrong payload length",
                            ));
                        }
                        let data_offset = payload_offset
                            .checked_add(reader.consumed() as u64)
                            .ok_or_else(|| invalid_record("before-image file offset overflow"))?;
                        reader.skip(length)?;
                        (false, data_offset)
                    }
                    1 if reader.remaining() == 0 => (true, 0),
                    1 => {
                        return Err(invalid_record(
                            "zero disk before-image contains raw payload bytes",
                        ));
                    }
                    _ => return Err(invalid_record("invalid disk before-image encoding")),
                };
                if before_images
                    .insert(
                        page_index,
                        BeforeImageLocation {
                            data_offset,
                            length,
                            zero,
                        },
                    )
                    .is_some()
                {
                    return Err(invalid_record("duplicate disk before-image page"));
                }
            }
            FRAME_FOOTER => {
                require_manifest(&manifest)?;
                let position = reader.position()?;
                if last_position.is_some_and(|previous| position < previous) {
                    return Err(invalid_record("Record footer precedes its Timeline"));
                }
                footer = Some(RecordFooter {
                    position,
                    outcome: decode_outcome(&mut reader)?,
                });
            }
            _ => return Err(invalid_record("unknown Record frame kind")),
        }
        reader.finish()?;
        frame_index += 1;
    }

    let manifest = manifest.ok_or_else(|| invalid_record("Record has no manifest"))?;
    let footer = footer.ok_or_else(|| invalid_record("Record has no footer"))?;
    Ok(Replayer {
        manifest,
        timeline,
        footer,
        storage: Arc::new(ReplayStorage {
            state: Mutex::new(ReplayStorageState {
                file,
                before_images,
                cow_pages: BTreeMap::new(),
                failure: None,
            }),
            failed: AtomicBool::new(false),
        }),
    })
}

fn write_frame(inner: &mut RecorderInner, kind: u8, payload: &[u8]) -> Result<(), RecordError> {
    if payload.len() > MAX_FRAME_BYTES {
        return Err(invalid_record("Record frame exceeds the size limit"));
    }
    let payload_length =
        u32::try_from(payload.len()).map_err(|_| invalid_record("Record frame length overflow"))?;
    let mut header = [0; FRAME_HEADER_BYTES];
    header[0] = kind;
    header[1..5].copy_from_slice(&payload_length.to_le_bytes());
    header[5..9].copy_from_slice(&crc32(payload).to_le_bytes());
    let writer = inner
        .writer
        .as_mut()
        .ok_or_else(|| invalid_record("record writer is unavailable"))?;
    writer.write_all(&header)?;
    writer.write_all(payload)?;
    Ok(())
}

fn encode_manifest(manifest: &RecordManifest) -> Result<Vec<u8>, RecordError> {
    let mut output = Vec::new();
    push_string(&mut output, &manifest.machine_model)?;
    push_string(&mut output, &manifest.float_backend)?;
    encode_media_identity(&mut output, Some(&manifest.prom))?;
    encode_media_identity(&mut output, manifest.disk.as_ref())?;
    encode_media_identity(&mut output, manifest.cdrom.as_ref())?;
    push_bytes(&mut output, &manifest.nonvolatile_state)?;
    Ok(output)
}

fn encode_serial_byte(position: ExecutionPosition, port: SerialPort, value: u8) -> Vec<u8> {
    let mut output = encode_position(position);
    output.push(serial_port_code(port));
    output.push(value);
    output
}

fn encode_reset(position: ExecutionPosition) -> Vec<u8> {
    encode_position(position)
}

fn encode_checkpoint(position: ExecutionPosition, digest: [u8; 32]) -> Vec<u8> {
    let mut output = encode_position(position);
    output.extend_from_slice(&digest);
    output
}

fn encode_before_image(page_index: u64, bytes: &[u8]) -> Result<Vec<u8>, RecordError> {
    if bytes.is_empty() || bytes.len() > DISK_PAGE_BYTES {
        return Err(invalid_record("invalid disk before-image length"));
    }
    let length = u16::try_from(bytes.len())
        .map_err(|_| invalid_record("disk before-image length overflow"))?;
    let mut output = Vec::with_capacity(11 + bytes.len());
    output.extend_from_slice(&page_index.to_le_bytes());
    output.extend_from_slice(&length.to_le_bytes());
    if bytes.iter().all(|&byte| byte == 0) {
        output.push(1);
    } else {
        output.push(0);
        output.extend_from_slice(bytes);
    }
    Ok(output)
}

fn encode_footer(
    position: ExecutionPosition,
    outcome: &RecordOutcome,
) -> Result<Vec<u8>, RecordError> {
    let mut output = encode_position(position);
    match outcome {
        RecordOutcome::UserStopped => output.push(0),
        RecordOutcome::Shutdown => output.push(1),
        RecordOutcome::ExecutionError {
            address,
            description,
        } => {
            output.push(2);
            output.extend_from_slice(&address.to_le_bytes());
            push_string(&mut output, description)?;
        }
    }
    Ok(output)
}

fn decode_manifest(reader: &mut SliceReader<'_>) -> Result<RecordManifest, RecordError> {
    let machine_model = reader.string()?;
    let float_backend = reader.string()?;
    let prom = decode_media_identity(reader)?
        .ok_or_else(|| invalid_record("Record manifest has no PROM identity"))?;
    let disk = decode_media_identity(reader)?;
    let cdrom = decode_media_identity(reader)?;
    let nonvolatile_state = reader.bytes()?;
    Ok(RecordManifest {
        machine_model,
        float_backend,
        prom,
        disk,
        cdrom,
        nonvolatile_state,
    })
}

fn decode_outcome(reader: &mut SliceReader<'_>) -> Result<RecordOutcome, RecordError> {
    match reader.u8()? {
        0 => Ok(RecordOutcome::UserStopped),
        1 => Ok(RecordOutcome::Shutdown),
        2 => Ok(RecordOutcome::ExecutionError {
            address: reader.u32()?,
            description: reader.string()?,
        }),
        _ => Err(invalid_record("invalid Record footer outcome")),
    }
}

fn encode_media_identity(
    output: &mut Vec<u8>,
    identity: Option<&MediaIdentity>,
) -> Result<(), RecordError> {
    let Some(identity) = identity else {
        output.push(0);
        return Ok(());
    };
    output.push(1);
    output.extend_from_slice(&identity.size_bytes.to_le_bytes());
    output.extend_from_slice(&identity.sha256);
    push_string(output, &identity.path_hint)
}

fn decode_media_identity(
    reader: &mut SliceReader<'_>,
) -> Result<Option<MediaIdentity>, RecordError> {
    match reader.u8()? {
        0 => Ok(None),
        1 => Ok(Some(MediaIdentity {
            size_bytes: reader.u64()?,
            sha256: reader.array_32()?,
            path_hint: reader.string()?,
        })),
        _ => Err(invalid_record("invalid media identity presence flag")),
    }
}

fn encode_position(position: ExecutionPosition) -> Vec<u8> {
    let mut output = Vec::with_capacity(16);
    output.extend_from_slice(&position.epoch.to_le_bytes());
    output.extend_from_slice(&position.completed_instructions.to_le_bytes());
    output
}

fn push_string(output: &mut Vec<u8>, value: &str) -> Result<(), RecordError> {
    if value.len() > MAX_STRING_BYTES {
        return Err(invalid_record("Record string exceeds the size limit"));
    }
    let length =
        u32::try_from(value.len()).map_err(|_| invalid_record("Record string length overflow"))?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn push_bytes(output: &mut Vec<u8>, value: &[u8]) -> Result<(), RecordError> {
    let length =
        u32::try_from(value.len()).map_err(|_| invalid_record("Record byte length overflow"))?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(value);
    Ok(())
}

fn validate_before_image(
    page_index: u64,
    length: usize,
    disk_size: u64,
) -> Result<(), RecordError> {
    let page_offset = page_index
        .checked_mul(DISK_PAGE_BYTES as u64)
        .ok_or_else(|| invalid_record("disk before-image offset overflow"))?;
    if page_offset >= disk_size {
        return Err(invalid_record("disk before-image page is out of range"));
    }
    let expected = usize::try_from((disk_size - page_offset).min(DISK_PAGE_BYTES as u64))
        .map_err(|_| invalid_record("disk before-image length does not fit usize"))?;
    if length != expected {
        return Err(invalid_record("disk before-image has the wrong length"));
    }
    Ok(())
}

fn require_manifest(manifest: &Option<RecordManifest>) -> Result<&RecordManifest, RecordError> {
    manifest
        .as_ref()
        .ok_or_else(|| invalid_record("Record data precedes its manifest"))
}

fn update_position(
    previous: &mut Option<ExecutionPosition>,
    current: ExecutionPosition,
) -> Result<(), RecordError> {
    if previous.is_some_and(|position| current < position) {
        return Err(invalid_record(
            "Record Timeline positions are not monotonic",
        ));
    }
    *previous = Some(current);
    Ok(())
}

fn normalized_record_path(path: &Path) -> PathBuf {
    if path.extension().and_then(|extension| extension.to_str()) == Some("serec") {
        path.to_path_buf()
    } else {
        path.with_extension("serec")
    }
}

fn partial_record_path(final_path: &Path) -> PathBuf {
    let mut name = final_path.as_os_str().to_os_string();
    name.push(".partial");
    PathBuf::from(name)
}

fn serial_port_code(port: SerialPort) -> u8 {
    match port {
        SerialPort::A => 0,
        SerialPort::B => 1,
    }
}

fn decode_serial_port(code: u8) -> Result<SerialPort, RecordError> {
    match code {
        0 => Ok(SerialPort::A),
        1 => Ok(SerialPort::B),
        _ => Err(invalid_record("invalid serial port in Record Timeline")),
    }
}

struct SliceReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> SliceReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    const fn consumed(&self) -> usize {
        self.cursor
    }

    const fn remaining(&self) -> usize {
        self.bytes.len() - self.cursor
    }

    fn read_exact(&mut self, output: &mut [u8]) -> Result<(), RecordError> {
        let end = self
            .cursor
            .checked_add(output.len())
            .ok_or_else(|| invalid_record("Record payload cursor overflow"))?;
        let source = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| invalid_record("truncated Record frame payload"))?;
        output.copy_from_slice(source);
        self.cursor = end;
        Ok(())
    }

    fn skip(&mut self, length: usize) -> Result<(), RecordError> {
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| invalid_record("Record payload cursor overflow"))?;
        if end > self.bytes.len() {
            return Err(invalid_record("truncated Record frame payload"));
        }
        self.cursor = end;
        Ok(())
    }

    fn u8(&mut self) -> Result<u8, RecordError> {
        let mut bytes = [0; 1];
        self.read_exact(&mut bytes)?;
        Ok(bytes[0])
    }

    fn u16(&mut self) -> Result<u16, RecordError> {
        let mut bytes = [0; 2];
        self.read_exact(&mut bytes)?;
        Ok(u16::from_le_bytes(bytes))
    }

    fn u32(&mut self) -> Result<u32, RecordError> {
        let mut bytes = [0; 4];
        self.read_exact(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn u64(&mut self) -> Result<u64, RecordError> {
        let mut bytes = [0; 8];
        self.read_exact(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn position(&mut self) -> Result<ExecutionPosition, RecordError> {
        Ok(ExecutionPosition {
            epoch: self.u64()?,
            completed_instructions: self.u64()?,
        })
    }

    fn array_32(&mut self) -> Result<[u8; 32], RecordError> {
        let mut bytes = [0; 32];
        self.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    fn string(&mut self) -> Result<String, RecordError> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| invalid_record("Record string length does not fit usize"))?;
        if length > MAX_STRING_BYTES {
            return Err(invalid_record("Record string exceeds the size limit"));
        }
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| invalid_record("Record string cursor overflow"))?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| invalid_record("truncated Record string"))?;
        self.cursor = end;
        String::from_utf8(bytes.to_vec()).map_err(|_| invalid_record("Record string is not UTF-8"))
    }

    fn bytes(&mut self) -> Result<Vec<u8>, RecordError> {
        let length = usize::try_from(self.u32()?)
            .map_err(|_| invalid_record("Record byte length does not fit usize"))?;
        let end = self
            .cursor
            .checked_add(length)
            .ok_or_else(|| invalid_record("Record byte cursor overflow"))?;
        let bytes = self
            .bytes
            .get(self.cursor..end)
            .ok_or_else(|| invalid_record("truncated Record bytes"))?;
        self.cursor = end;
        Ok(bytes.to_vec())
    }

    fn finish(&self) -> Result<(), RecordError> {
        if self.cursor == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid_record("Record frame has trailing payload bytes"))
        }
    }
}

fn commit_record_file(
    partial_path: &Path,
    final_path: &Path,
    replace_existing: bool,
) -> io::Result<()> {
    if replace_existing {
        replace_file(partial_path, final_path)
    } else {
        fs::rename(partial_path, final_path)
    }
}

#[cfg(not(windows))]
fn replace_file(partial_path: &Path, final_path: &Path) -> io::Result<()> {
    fs::rename(partial_path, final_path)
}

#[cfg(windows)]
fn replace_file(partial_path: &Path, final_path: &Path) -> io::Result<()> {
    let partial_path = wide_path(partial_path);
    let final_path = wide_path(final_path);
    // SAFETY: Both paths are live, null-terminated UTF-16 buffers for the
    // duration of the call, and the flags require no additional pointers.
    let result = unsafe {
        MoveFileExW(
            partial_path.as_ptr(),
            final_path.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn wide_path(path: &Path) -> Vec<u16> {
    path.as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

impl RecordDisk {
    /// Saves one page as it existed before the first Recording write.
    ///
    /// Repeated calls for the same page are no-ops. Once Recording finishes or
    /// fails, the capability also becomes a no-op so the application adapter
    /// can continue as ordinary direct storage. `read_page` is called only
    /// when the page needs its first Before Image.
    ///
    /// # Errors
    ///
    /// Returns an I/O-shaped [`io::Error`] when Record output fails.
    pub fn capture_before_image(
        &self,
        page_index: u64,
        read_page: impl FnOnce() -> io::Result<Vec<u8>>,
    ) -> io::Result<()> {
        let mut inner = lock_unpoisoned(&self.recorder.inner);
        if !inner.active {
            return Ok(());
        }
        if let Some(failure) = &inner.failure {
            return Err(io::Error::other(format!(
                "recording already failed: {failure}"
            )));
        }
        if inner.captured_pages.contains(&page_index) {
            return Ok(());
        }
        let result: Result<(), RecordError> = (|| {
            require_active(&inner)?;
            let bytes = read_page().map_err(RecordError::Io)?;
            let payload = encode_before_image(page_index, &bytes)?;
            write_frame(&mut inner, FRAME_DISK_BEFORE_IMAGE, &payload)?;
            inner.captured_pages.insert(page_index);
            Ok(())
        })();
        if let Err(error) = &result {
            inner.failure = Some(error.to_string());
            self.recorder.failed.store(true, Ordering::Release);
        }
        result.map_err(record_io_error)
    }

    /// Retains a host storage failure for the runtime worker.
    pub fn report_storage_error(&self, error: &io::Error) {
        let mut inner = lock_unpoisoned(&self.recorder.inner);
        if inner.active && inner.failure.is_none() {
            inner.failure = Some(format!("host storage error: {error}"));
            self.recorder.failed.store(true, Ordering::Release);
        }
    }
}

impl ReplayDisk {
    /// Applies Replay COW or recorded Before Images to a completed base read.
    ///
    /// # Errors
    ///
    /// Returns an error when the range overflows or lazy Before Image I/O
    /// fails.
    pub fn overlay_read(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let mut state = lock_unpoisoned(&self.storage.state);
        ensure_replay_storage_healthy(&state)?;
        overlay_range(&mut state, offset, buffer, true)
    }

    /// Applies only Before Images while validating the logical initial disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the range overflows or lazy Before Image I/O
    /// fails.
    pub fn overlay_initial_read(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let mut state = lock_unpoisoned(&self.storage.state);
        ensure_replay_storage_healthy(&state)?;
        overlay_range(&mut state, offset, buffer, false)
    }

    /// Writes into an in-memory page COW without modifying the base disk.
    ///
    /// The callback supplies one complete current base page when Replay first
    /// writes that page.
    ///
    /// # Errors
    ///
    /// Returns an error for invalid ranges, base reads, or Before Image I/O.
    pub fn write_all_at(
        &self,
        offset: u64,
        data: &[u8],
        size_bytes: u64,
        mut read_base_page: impl FnMut(u64, &mut [u8]) -> io::Result<()>,
    ) -> io::Result<()> {
        check_range(offset, data.len(), size_bytes)?;
        if data.is_empty() {
            return Ok(());
        }
        let mut state = lock_unpoisoned(&self.storage.state);
        ensure_replay_storage_healthy(&state)?;
        let end = offset + data.len() as u64;
        let first_page = offset / DISK_PAGE_BYTES as u64;
        let last_page = (end - 1) / DISK_PAGE_BYTES as u64;
        for page_index in first_page..=last_page {
            let page_offset = page_index * DISK_PAGE_BYTES as u64;
            let page_length =
                usize::try_from((size_bytes - page_offset).min(DISK_PAGE_BYTES as u64))
                    .map_err(|_| io::Error::other("disk page length does not fit usize"))?;
            if !state.cow_pages.contains_key(&page_index) {
                let mut page = vec![0; page_length];
                read_base_page(page_offset, &mut page)?;
                if let Some(before) = read_before_image(&mut state, page_index)? {
                    page.copy_from_slice(&before);
                }
                state.cow_pages.insert(page_index, page);
            }
            let write_start = offset.max(page_offset);
            let write_end = end.min(page_offset + page_length as u64);
            let source_start = usize::try_from(write_start - offset)
                .map_err(|_| io::Error::other("Replay write source offset does not fit usize"))?;
            let source_end = usize::try_from(write_end - offset)
                .map_err(|_| io::Error::other("Replay write source end does not fit usize"))?;
            let page_start = usize::try_from(write_start - page_offset)
                .map_err(|_| io::Error::other("Replay page offset does not fit usize"))?;
            let page_end = page_start + (source_end - source_start);
            state
                .cow_pages
                .get_mut(&page_index)
                .expect("the Replay COW page was inserted")[page_start..page_end]
                .copy_from_slice(&data[source_start..source_end]);
        }
        Ok(())
    }

    /// Retains a host storage failure for the runtime worker.
    pub fn report_storage_error(&self, error: &io::Error) {
        let mut state = lock_unpoisoned(&self.storage.state);
        if state.failure.is_none() {
            state.failure = Some(format!("host storage error: {error}"));
            self.storage.failed.store(true, Ordering::Release);
        }
    }
}

fn overlay_range(
    state: &mut ReplayStorageState,
    offset: u64,
    buffer: &mut [u8],
    include_cow: bool,
) -> io::Result<()> {
    if buffer.is_empty() {
        return Ok(());
    }
    let end = offset
        .checked_add(buffer.len() as u64)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "Replay read range overflow"))?;
    let first_page = offset / DISK_PAGE_BYTES as u64;
    let last_page = (end - 1) / DISK_PAGE_BYTES as u64;
    for page_index in first_page..=last_page {
        let page_offset = page_index * DISK_PAGE_BYTES as u64;
        let start = offset.max(page_offset);
        let finish = end.min(page_offset + DISK_PAGE_BYTES as u64);
        let target_start = usize::try_from(start - offset)
            .map_err(|_| io::Error::other("Replay overlay offset does not fit usize"))?;
        let target_end = usize::try_from(finish - offset)
            .map_err(|_| io::Error::other("Replay overlay end does not fit usize"))?;
        let page_start = usize::try_from(start - page_offset)
            .map_err(|_| io::Error::other("Replay page offset does not fit usize"))?;
        let page_end = page_start + (target_end - target_start);

        if include_cow && let Some(page) = state.cow_pages.get(&page_index) {
            buffer[target_start..target_end].copy_from_slice(&page[page_start..page_end]);
            continue;
        }
        if let Some(page) = read_before_image(state, page_index)? {
            buffer[target_start..target_end].copy_from_slice(&page[page_start..page_end]);
        }
    }
    Ok(())
}

fn read_before_image(
    state: &mut ReplayStorageState,
    page_index: u64,
) -> io::Result<Option<Vec<u8>>> {
    let Some(location) = state.before_images.get(&page_index).copied() else {
        return Ok(None);
    };
    if location.zero {
        return Ok(Some(vec![0; location.length]));
    }
    let mut bytes = vec![0; location.length];
    state.file.seek(SeekFrom::Start(location.data_offset))?;
    state.file.read_exact(&mut bytes)?;
    Ok(Some(bytes))
}

fn ensure_replay_storage_healthy(state: &ReplayStorageState) -> io::Result<()> {
    match &state.failure {
        Some(failure) => Err(io::Error::other(format!(
            "Replay storage already failed: {failure}"
        ))),
        None => Ok(()),
    }
}

fn check_range(offset: u64, length: usize, size_bytes: u64) -> io::Result<()> {
    let byte_count = u64::try_from(length)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "storage length overflow"))?;
    let end = offset
        .checked_add(byte_count)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "storage range overflow"))?;
    if end > size_bytes {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "storage range exceeds fixed storage capacity",
        ));
    }
    Ok(())
}

fn record_io_error(error: RecordError) -> io::Error {
    match error {
        RecordError::Io(error) => error,
        RecordError::InvalidData(reason) => io::Error::other(reason),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        DISK_PAGE_BYTES, ExecutionPosition, FILE_HEADER_BYTES, FRAME_HEADER_BYTES, MediaIdentity,
        RecordManifest, RecordOutcome, Recorder, Replayer,
    };

    fn temporary_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sgi-emu-record-{name}-{}.serec",
            std::process::id()
        ))
    }

    fn manifest() -> RecordManifest {
        RecordManifest::new(
            String::from("indigo-ip12"),
            String::from("softfloat"),
            MediaIdentity::from_bytes(Path::new("prom.bin"), &[1, 2, 3]),
            None,
            None,
            vec![0x12, 0x34, 0x56],
        )
    }

    #[test]
    fn complete_record_preserves_manifest_and_footer() {
        let path = temporary_path("round-trip");
        let _ = fs::remove_file(&path);
        let recorder = Recorder::create(&path).unwrap();
        recorder.start(&manifest()).unwrap();
        recorder
            .record_checkpoint(ExecutionPosition::default(), [7; 32])
            .unwrap();
        recorder
            .finalize(ExecutionPosition::default(), &RecordOutcome::UserStopped)
            .unwrap();

        let replayer = Replayer::open(&path).unwrap();
        assert_eq!(replayer.manifest().machine_model(), "indigo-ip12");
        assert_eq!(
            replayer.manifest().nonvolatile_state_bytes(),
            [0x12, 0x34, 0x56]
        );
        let session = replayer.into_session();
        assert_eq!(session.final_position(), ExecutionPosition::default());
        assert_eq!(session.outcome(), &RecordOutcome::UserStopped);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn creating_record_does_not_replace_existing_destination() {
        let path = temporary_path("create-existing");
        let partial = PathBuf::from(format!("{}.partial", path.display()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&partial);
        fs::write(&path, b"old complete record").unwrap();

        let error = match Recorder::create(&path) {
            Ok(_) => panic!("the create-only API must not replace its destination"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("record already exists"));
        assert_eq!(fs::read(&path).unwrap(), b"old complete record");
        assert!(!partial.exists());

        fs::remove_file(path).unwrap();
    }

    #[test]
    fn replacing_record_preserves_old_file_until_commit() {
        let path = temporary_path("replace");
        let partial = PathBuf::from(format!("{}.partial", path.display()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&partial);
        fs::write(&path, b"old complete record").unwrap();

        let recorder = Recorder::create_or_replace(&path).unwrap();
        recorder.start(&manifest()).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"old complete record");
        recorder
            .finalize(ExecutionPosition::default(), &RecordOutcome::UserStopped)
            .unwrap();

        let replayer = Replayer::open(&path).unwrap();
        assert_eq!(replayer.manifest().machine_model(), "indigo-ip12");
        drop(replayer);
        assert!(!partial.exists());
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn replacing_record_does_not_overwrite_existing_partial() {
        let path = temporary_path("replace-partial");
        let partial = PathBuf::from(format!("{}.partial", path.display()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&partial);
        fs::write(&path, b"old complete record").unwrap();
        fs::write(&partial, b"recording in progress").unwrap();

        let error = match Recorder::create_or_replace(&path) {
            Ok(_) => panic!("an existing partial record must not be overwritten"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("partial record already exists"));
        assert_eq!(fs::read(&path).unwrap(), b"old complete record");
        assert_eq!(fs::read(&partial).unwrap(), b"recording in progress");

        fs::remove_file(partial).unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn partial_record_is_not_replayable() {
        let path = temporary_path("partial");
        let partial = PathBuf::from(format!("{}.partial", path.display()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&partial);
        let recorder = Recorder::create(&path).unwrap();
        recorder.start(&manifest()).unwrap();
        assert!(Replayer::open(&partial).is_err());
        drop(recorder);
        fs::remove_file(partial).unwrap();
    }

    fn write_complete_record(path: &Path) {
        let _ = fs::remove_file(path);
        let recorder = Recorder::create(path).unwrap();
        recorder.start(&manifest()).unwrap();
        recorder
            .record_checkpoint(ExecutionPosition::default(), [7; 32])
            .unwrap();
        recorder
            .finalize(ExecutionPosition::default(), &RecordOutcome::UserStopped)
            .unwrap();
    }

    #[test]
    fn opaque_nonvolatile_state_round_trips_without_machine_types() {
        let path = temporary_path("opaque-state");
        write_complete_record(&path);

        let replayer = Replayer::open(&path).unwrap();
        assert_eq!(
            replayer.manifest().nonvolatile_state_bytes(),
            [0x12, 0x34, 0x56]
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn crc_truncation_and_footer_trailing_data_are_rejected() {
        let crc_path = temporary_path("crc");
        write_complete_record(&crc_path);
        let mut bytes = fs::read(&crc_path).unwrap();
        bytes[FILE_HEADER_BYTES + FRAME_HEADER_BYTES] ^= 0x80;
        fs::write(&crc_path, bytes).unwrap();
        assert!(Replayer::open(&crc_path).is_err());
        fs::remove_file(crc_path).unwrap();

        let truncated_path = temporary_path("truncated");
        write_complete_record(&truncated_path);
        let mut bytes = fs::read(&truncated_path).unwrap();
        bytes.pop();
        fs::write(&truncated_path, bytes).unwrap();
        assert!(Replayer::open(&truncated_path).is_err());
        fs::remove_file(truncated_path).unwrap();

        let trailing_path = temporary_path("trailing");
        write_complete_record(&trailing_path);
        let mut bytes = fs::read(&trailing_path).unwrap();
        bytes.push(0);
        fs::write(&trailing_path, bytes).unwrap();
        assert!(Replayer::open(&trailing_path).is_err());
        fs::remove_file(trailing_path).unwrap();
    }

    #[test]
    fn decreasing_timeline_position_is_rejected() {
        let path = temporary_path("position");
        let _ = fs::remove_file(&path);
        let recorder = Recorder::create(&path).unwrap();
        recorder.start(&manifest()).unwrap();
        recorder
            .record_reset(ExecutionPosition {
                epoch: 1,
                completed_instructions: 0,
            })
            .unwrap();
        recorder
            .record_checkpoint(ExecutionPosition::default(), [0; 32])
            .unwrap();
        recorder
            .finalize(
                ExecutionPosition {
                    epoch: 1,
                    completed_instructions: 0,
                },
                &RecordOutcome::UserStopped,
            )
            .unwrap();
        assert!(Replayer::open(&path).is_err());
        fs::remove_file(path).unwrap();
    }

    fn disk_manifest(disk: &[u8]) -> RecordManifest {
        RecordManifest::new(
            String::from("indigo-ip12"),
            String::from("softfloat"),
            MediaIdentity::from_bytes(Path::new("prom.bin"), &[1, 2, 3]),
            Some(MediaIdentity::from_bytes(Path::new("disk.img"), disk)),
            None,
            vec![0x12, 0x34, 0x56],
        )
    }

    #[test]
    fn before_image_is_captured_once_and_cow_has_priority() {
        let path = temporary_path("priority");
        let _ = fs::remove_file(&path);
        let initial = vec![1; DISK_PAGE_BYTES];
        let recorder = Recorder::create(&path).unwrap();
        recorder.start(&disk_manifest(&initial)).unwrap();
        let disk = recorder.disk();
        disk.capture_before_image(0, || Ok(initial.clone()))
            .unwrap();
        disk.capture_before_image(0, || panic!("a captured page must not be read again"))
            .unwrap();
        recorder
            .finalize(ExecutionPosition::default(), &RecordOutcome::UserStopped)
            .unwrap();
        disk.capture_before_image(1, || {
            panic!("an inactive Record disk must not read the base image")
        })
        .unwrap();

        let replayer = Replayer::open(&path).unwrap();
        let replay_disk = replayer.disk();
        let mut base = vec![9; DISK_PAGE_BYTES];
        replay_disk.overlay_initial_read(0, &mut base).unwrap();
        assert_eq!(base, initial);

        replay_disk
            .write_all_at(10, &[7, 8], DISK_PAGE_BYTES as u64, |_offset, page| {
                page.fill(9);
                Ok(())
            })
            .unwrap();
        let mut replay_read = vec![9; DISK_PAGE_BYTES];
        replay_disk.overlay_read(0, &mut replay_read).unwrap();
        assert_eq!(&replay_read[..10], &initial[..10]);
        assert_eq!(&replay_read[10..12], &[7, 8]);
        assert_eq!(&replay_read[12..], &initial[12..]);
        drop(replay_disk);
        drop(replayer);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn partial_tail_before_image_restores_only_its_real_length() {
        let path = temporary_path("tail");
        let _ = fs::remove_file(&path);
        let mut initial = vec![0; DISK_PAGE_BYTES + 3];
        initial[DISK_PAGE_BYTES..].copy_from_slice(&[1, 2, 3]);
        let recorder = Recorder::create(&path).unwrap();
        recorder.start(&disk_manifest(&initial)).unwrap();
        recorder
            .disk()
            .capture_before_image(1, || Ok(vec![1, 2, 3]))
            .unwrap();
        recorder
            .finalize(ExecutionPosition::default(), &RecordOutcome::UserStopped)
            .unwrap();
        let replayer = Replayer::open(&path).unwrap();
        let mut tail = [9, 9, 9];
        let replay_disk = replayer.disk();
        replay_disk
            .overlay_initial_read(DISK_PAGE_BYTES as u64, &mut tail)
            .unwrap();
        assert_eq!(tail, [1, 2, 3]);
        drop(replay_disk);
        drop(replayer);
        fs::remove_file(path).unwrap();
    }
}
