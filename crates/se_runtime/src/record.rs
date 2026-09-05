//! Deterministic cold-start recording and replay support.
//!
//! A record stores the cold machine configuration, bytes accepted by the
//! emulated serial controller at exact instruction boundaries, sparse CPU
//! checkpoints, and disk before-images. Replay normally constructs a new
//! machine at the first PROM instruction; an explicitly created Replay
//! snapshot may instead restore one paused execution boundary. These
//! machine-specific restore points are not general save states.
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
use se_core::time::VirtualInstant;
use se_machine::machine::{MachineNonvolatileState, MachineSnapshot, MachineStartupConfiguration};
use se_machine::serial::SerialPort;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
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
const FRAME_HEADER_BYTES: usize = 8;
const MAX_FRAME_BYTES: usize = 1024 * 1024;
const CACHE_HEADER_BYTES: usize = 24;
const SNAPSHOT_MAGIC: [u8; 8] = *b"SGICKPT\0";
const INDEX_MAGIC: [u8; 8] = *b"SGISIDX\0";
const CACHE_SCHEMA: u32 = 1;

/// Granularity used for writable-disk before-images and replay COW pages.
pub const DISK_PAGE_BYTES: usize = 4096;

/// Deterministic boundary immediately before the next guest instruction.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ExecutionPosition {
    /// Reset epoch, beginning at zero for cold power-on.
    pub epoch: u64,
    /// Instructions completed within the current epoch.
    pub completed_instructions: u64,
}

/// Stable content identity for one configured host medium.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
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

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RecordIdentity {
    size_bytes: u64,
    sha256: [u8; 32],
}

/// One manually created restore point associated with a complete Record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplaySnapshotInfo {
    id: String,
    position: ExecutionPosition,
    pc: u32,
}

impl ReplaySnapshotInfo {
    /// Returns the opaque identifier accepted by Replay open operations.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the exact execution boundary represented by the snapshot.
    #[must_use]
    pub const fn position(&self) -> ExecutionPosition {
        self.position
    }

    /// Returns the next guest instruction address captured at that boundary.
    #[must_use]
    pub const fn pc(&self) -> u32 {
        self.pc
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct SnapshotCatalog {
    record: RecordIdentity,
    entries: Vec<SnapshotCatalogEntry>,
}

#[derive(Clone, Deserialize, Serialize)]
struct SnapshotCatalogEntry {
    id: String,
    position: ExecutionPosition,
    pc: u32,
    timeline_cursor: usize,
    last_verified_checkpoint: Option<ExecutionPosition>,
    machine_fingerprint: [u8; 32],
    file_size_bytes: u64,
    file_sha256: [u8; 32],
}

#[derive(Deserialize, Serialize)]
struct ReplaySnapshot {
    record: RecordIdentity,
    position: ExecutionPosition,
    completed_instructions: u64,
    virtual_instant: VirtualInstant,
    cpu_clock_remainder: u128,
    timeline_cursor: usize,
    last_verified_checkpoint: Option<ExecutionPosition>,
    machine_fingerprint: [u8; 32],
    machine: MachineSnapshot,
    cow_pages: BTreeMap<u64, BeforeImageData>,
    pc: u32,
}

pub(crate) struct ReplayRestoreState {
    pub(crate) position: ExecutionPosition,
    pub(crate) completed_instructions: u64,
    pub(crate) virtual_instant: VirtualInstant,
    pub(crate) cpu_clock_remainder: u128,
    pub(crate) machine_fingerprint: [u8; 32],
    pub(crate) machine: MachineSnapshot,
}

/// Complete immutable configuration recorded before the first instruction.
///
/// Machine-specific nonvolatile state remains owned by the machine layer.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RecordManifest {
    machine: MachineStartupConfiguration,
    prom: MediaIdentity,
    disk: Option<MediaIdentity>,
    cdrom: Option<MediaIdentity>,
    nonvolatile_state: MachineNonvolatileState,
}

impl RecordManifest {
    /// Creates a complete cold-start manifest.
    #[must_use]
    pub fn new(
        machine: MachineStartupConfiguration,
        prom: MediaIdentity,
        disk: Option<MediaIdentity>,
        cdrom: Option<MediaIdentity>,
        nonvolatile_state: MachineNonvolatileState,
    ) -> Self {
        Self {
            machine,
            prom,
            disk,
            cdrom,
            nonvolatile_state,
        }
    }

    /// Returns the recorded construction-time machine configuration.
    #[must_use]
    pub const fn machine(&self) -> &MachineStartupConfiguration {
        &self.machine
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

    /// Returns the exact cold-start nonvolatile machine state.
    #[must_use]
    pub const fn nonvolatile_state(&self) -> &MachineNonvolatileState {
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
            write_frame(inner, &RecordFrame::Manifest(Box::new(manifest.clone())))?;
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
        self.append(RecordFrame::Timeline(TimelineEntry {
            position,
            action: TimelineAction::SerialByte { port, value },
        }))
    }

    pub(crate) fn record_reset(&self, position: ExecutionPosition) -> Result<(), RecordError> {
        self.append(RecordFrame::Timeline(TimelineEntry {
            position,
            action: TimelineAction::Reset,
        }))
    }

    pub(crate) fn record_checkpoint(
        &self,
        position: ExecutionPosition,
        digest: [u8; 32],
    ) -> Result<(), RecordError> {
        self.append(RecordFrame::Timeline(TimelineEntry {
            position,
            action: TimelineAction::Checkpoint { digest },
        }))
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
            write_frame(
                inner,
                &RecordFrame::Footer(RecordFooter {
                    position,
                    outcome: outcome.clone(),
                }),
            )?;
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

    fn append(&self, frame: RecordFrame) -> Result<(), RecordError> {
        self.update(|inner| {
            require_active(inner)?;
            write_frame(inner, &frame)
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

/// Parsed complete Record and optional manual restore point.
///
/// Mutable Timeline state moves into the runtime worker. Opening a snapshot
/// still validates the authoritative Record, its catalog identity, Timeline
/// cursor, and Replay disk COW before exposing the session.
pub struct Replayer {
    record_path: PathBuf,
    record_identity: RecordIdentity,
    manifest: RecordManifest,
    timeline: Vec<TimelineEntry>,
    footer: RecordFooter,
    storage: ReplayStorageHandle,
    initial_cursor: usize,
    restore_state: Option<ReplayRestoreState>,
}

impl Replayer {
    /// Opens, validates, and indexes a complete `.serec` file.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] for I/O failures or malformed/incomplete data.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, RecordError> {
        Self::open_inner(path.as_ref(), None)
    }

    /// Opens a complete Record at one manually created restore point.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when the Record, snapshot, identity, Timeline
    /// cursor, or Replay COW is invalid.
    pub fn open_snapshot(path: impl AsRef<Path>, snapshot_id: &str) -> Result<Self, RecordError> {
        Self::open_inner(path.as_ref(), Some(snapshot_id))
    }

    fn open_inner(path: &Path, snapshot_id: Option<&str>) -> Result<Self, RecordError> {
        if path.extension().and_then(|extension| extension.to_str()) == Some("partial") {
            return Err(invalid_record("partial records cannot be replayed"));
        }
        let mut file = File::open(path)?;
        let record_identity = identity_for_open_file(&mut file)?;
        file.seek(SeekFrom::Start(0))?;
        let mut replayer = parse_record(file, path.to_path_buf(), record_identity)?;
        if let Some(snapshot_id) = snapshot_id {
            replayer.install_snapshot(snapshot_id)?;
        }
        Ok(replayer)
    }

    /// Lists valid catalog entries associated with a complete Record.
    ///
    /// A missing or invalid index is rebuilt from complete checkpoint files.
    ///
    /// # Errors
    ///
    /// Returns [`RecordError`] when the Record cannot be identified or the
    /// snapshot directory cannot be read.
    pub fn snapshot_catalog(
        path: impl AsRef<Path>,
    ) -> Result<Vec<ReplaySnapshotInfo>, RecordError> {
        let path = path.as_ref();
        let mut file = File::open(path)?;
        let record_identity = identity_for_open_file(&mut file)?;
        let catalog = load_or_rebuild_catalog(path, &record_identity)?;
        Ok(snapshot_infos(catalog))
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

    pub(crate) fn into_session(self) -> (ReplaySession, Option<ReplayRestoreState>) {
        (
            ReplaySession {
                record_path: self.record_path,
                record_identity: self.record_identity,
                timeline: self.timeline,
                cursor: self.initial_cursor,
                footer: self.footer,
                storage: self.storage,
            },
            self.restore_state,
        )
    }

    fn install_snapshot(&mut self, snapshot_id: &str) -> Result<(), RecordError> {
        validate_snapshot_id(snapshot_id)?;
        let catalog = load_or_rebuild_catalog(&self.record_path, &self.record_identity)?;
        let entry = catalog
            .entries
            .iter()
            .find(|entry| entry.id == snapshot_id)
            .ok_or_else(|| invalid_record("Replay snapshot is not present in the catalog"))?;
        let path = checkpoint_directory(&self.record_path).join(snapshot_id);
        let (snapshot, file_size_bytes, file_sha256): (ReplaySnapshot, _, _) =
            read_cache_file_with_identity(&path, SNAPSHOT_MAGIC, "Replay snapshot")?;
        if file_size_bytes != entry.file_size_bytes || file_sha256 != entry.file_sha256 {
            return Err(invalid_record(
                "Replay snapshot identity does not match its index",
            ));
        }
        if snapshot.position != entry.position
            || snapshot.pc != entry.pc
            || snapshot.timeline_cursor != entry.timeline_cursor
            || snapshot.last_verified_checkpoint != entry.last_verified_checkpoint
            || snapshot.machine_fingerprint != entry.machine_fingerprint
        {
            return Err(invalid_record(
                "Replay snapshot metadata does not match its index",
            ));
        }
        validate_replay_snapshot(self, &snapshot)?;
        let cow_pages = snapshot
            .cow_pages
            .into_iter()
            .map(|(page_index, data)| decode_before_image(data).map(|bytes| (page_index, bytes)))
            .collect::<Result<_, _>>()?;
        lock_unpoisoned(&self.storage.state).cow_pages = cow_pages;
        self.initial_cursor = snapshot.timeline_cursor;
        self.restore_state = Some(ReplayRestoreState {
            position: snapshot.position,
            completed_instructions: snapshot.completed_instructions,
            virtual_instant: snapshot.virtual_instant,
            cpu_clock_remainder: snapshot.cpu_clock_remainder,
            machine_fingerprint: snapshot.machine_fingerprint,
            machine: snapshot.machine,
        });
        Ok(())
    }
}

fn snapshot_infos(catalog: SnapshotCatalog) -> Vec<ReplaySnapshotInfo> {
    catalog
        .entries
        .into_iter()
        .map(|entry| ReplaySnapshotInfo {
            id: entry.id,
            position: entry.position,
            pc: entry.pc,
        })
        .collect()
}

/// Capability providing the initial disk overlay and ephemeral Replay COW.
#[derive(Clone)]
pub struct ReplayDisk {
    storage: ReplayStorageHandle,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct TimelineEntry {
    pub(crate) position: ExecutionPosition,
    pub(crate) action: TimelineAction,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum TimelineAction {
    SerialByte { port: SerialPort, value: u8 },
    Reset,
    Checkpoint { digest: [u8; 32] },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) enum RecordOutcome {
    UserStopped,
    Shutdown,
    ExecutionError { address: u32, description: String },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct RecordFooter {
    position: ExecutionPosition,
    outcome: RecordOutcome,
}

#[derive(Deserialize, Serialize)]
enum RecordFrame {
    Manifest(Box<RecordManifest>),
    Timeline(TimelineEntry),
    DiskBeforeImage {
        page_index: u64,
        data: BeforeImageData,
    },
    Footer(RecordFooter),
}

#[derive(Clone, Deserialize, Serialize)]
enum BeforeImageData {
    Raw(Vec<u8>),
    Zero(usize),
}

pub(crate) struct ReplaySession {
    record_path: PathBuf,
    record_identity: RecordIdentity,
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

    #[allow(
        clippy::too_many_arguments,
        reason = "the fields form one complete manual Replay restore point"
    )]
    pub(crate) fn create_snapshot(
        &self,
        position: ExecutionPosition,
        completed_instructions: u64,
        virtual_instant: VirtualInstant,
        cpu_clock_remainder: u128,
        machine_fingerprint: [u8; 32],
        machine: MachineSnapshot,
        pc: u32,
    ) -> Result<ReplaySnapshotInfo, RecordError> {
        if let Some(failure) = self.storage_failure() {
            return Err(invalid_record(format!(
                "Replay storage already failed: {failure}"
            )));
        }
        let last_verified_checkpoint = self.timeline[..self.cursor]
            .iter()
            .rev()
            .find(|entry| matches!(entry.action, TimelineAction::Checkpoint { .. }))
            .map(|entry| entry.position);
        let cow_pages = lock_unpoisoned(&self.storage.state)
            .cow_pages
            .iter()
            .map(|(&page_index, bytes)| {
                encode_before_image(bytes.clone()).map(|data| (page_index, data))
            })
            .collect::<Result<_, _>>()?;
        let snapshot = ReplaySnapshot {
            record: self.record_identity.clone(),
            position,
            completed_instructions,
            virtual_instant,
            cpu_clock_remainder,
            timeline_cursor: self.cursor,
            last_verified_checkpoint,
            machine_fingerprint,
            machine,
            cow_pages,
            pc,
        };

        let id = snapshot_file_name(position);
        let directory = checkpoint_directory(&self.record_path);
        fs::create_dir_all(&directory)?;
        let snapshot_path = directory.join(&id);
        let (file_size_bytes, file_sha256) =
            write_cache_file(&snapshot_path, SNAPSHOT_MAGIC, &snapshot)?;

        let mut catalog = load_or_rebuild_catalog(&self.record_path, &self.record_identity)?;
        catalog.entries.retain(|entry| entry.position != position);
        catalog.entries.push(SnapshotCatalogEntry {
            id: id.clone(),
            position,
            pc,
            timeline_cursor: self.cursor,
            last_verified_checkpoint,
            machine_fingerprint,
            file_size_bytes,
            file_sha256,
        });
        catalog.entries.sort_by_key(|entry| entry.position);
        write_cache_file(&index_path(&self.record_path), INDEX_MAGIC, &catalog)?;
        Ok(ReplaySnapshotInfo { id, position, pc })
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
    payload_offset: u64,
    payload_length: usize,
    payload_crc: u32,
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

fn parse_record(
    mut file: File,
    record_path: PathBuf,
    record_identity: RecordIdentity,
) -> Result<Replayer, RecordError> {
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
        let payload_length =
            usize::try_from(u32::from_le_bytes(frame_header[..4].try_into().unwrap()))
                .map_err(|_| invalid_record("Record frame length does not fit usize"))?;
        if payload_length > MAX_FRAME_BYTES {
            return Err(invalid_record("Record frame exceeds the size limit"));
        }
        let expected_crc = u32::from_le_bytes(frame_header[4..8].try_into().unwrap());
        let payload_offset = file.stream_position()?;
        let mut payload = vec![0; payload_length];
        file.read_exact(&mut payload)?;
        if crc32(&payload) != expected_crc {
            return Err(invalid_record("Record frame CRC mismatch"));
        }

        match decode_value::<RecordFrame>(&payload, "Record frame")? {
            RecordFrame::Manifest(value) => {
                if frame_index != 0 || manifest.is_some() {
                    return Err(invalid_record(
                        "Record manifest must be the unique first frame",
                    ));
                }
                manifest = Some(*value);
            }
            RecordFrame::Timeline(entry) => {
                require_manifest(&manifest)?;
                update_position(&mut last_position, entry.position)?;
                timeline.push(entry);
            }
            RecordFrame::DiskBeforeImage { page_index, data } => {
                let manifest = require_manifest(&manifest)?;
                let disk = manifest.disk().ok_or_else(|| {
                    invalid_record("Record has a disk before-image without a disk")
                })?;
                validate_before_image(page_index, before_image_length(&data)?, disk.size_bytes)?;
                if before_images
                    .insert(
                        page_index,
                        BeforeImageLocation {
                            payload_offset,
                            payload_length,
                            payload_crc: expected_crc,
                        },
                    )
                    .is_some()
                {
                    return Err(invalid_record("duplicate disk before-image page"));
                }
            }
            RecordFrame::Footer(value) => {
                require_manifest(&manifest)?;
                if last_position.is_some_and(|previous| value.position < previous) {
                    return Err(invalid_record("Record footer precedes its Timeline"));
                }
                footer = Some(value);
            }
        }
        frame_index += 1;
    }

    let manifest = manifest.ok_or_else(|| invalid_record("Record has no manifest"))?;
    let footer = footer.ok_or_else(|| invalid_record("Record has no footer"))?;
    Ok(Replayer {
        record_path,
        record_identity,
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
        initial_cursor: 0,
        restore_state: None,
    })
}

fn write_frame(inner: &mut RecorderInner, frame: &RecordFrame) -> Result<(), RecordError> {
    let payload = encode_value(frame, "Record frame")?;
    if payload.len() > MAX_FRAME_BYTES {
        return Err(invalid_record("Record frame exceeds the size limit"));
    }
    let payload_length =
        u32::try_from(payload.len()).map_err(|_| invalid_record("Record frame length overflow"))?;
    let mut header = [0; FRAME_HEADER_BYTES];
    header[..4].copy_from_slice(&payload_length.to_le_bytes());
    header[4..8].copy_from_slice(&crc32(&payload).to_le_bytes());
    let writer = inner
        .writer
        .as_mut()
        .ok_or_else(|| invalid_record("record writer is unavailable"))?;
    writer.write_all(&header)?;
    writer.write_all(&payload)?;
    Ok(())
}

fn encode_value<T: Serialize>(value: &T, name: &str) -> Result<Vec<u8>, RecordError> {
    bincode::serde::encode_to_vec(
        value,
        bincode::config::standard()
            .with_fixed_int_encoding()
            .with_little_endian(),
    )
    .map_err(|error| invalid_record(format!("failed to encode {name}: {error}")))
}

fn decode_value<T: DeserializeOwned>(bytes: &[u8], name: &str) -> Result<T, RecordError> {
    let (value, consumed) = bincode::serde::decode_from_slice(
        bytes,
        bincode::config::standard()
            .with_fixed_int_encoding()
            .with_little_endian(),
    )
    .map_err(|error| invalid_record(format!("failed to decode {name}: {error}")))?;
    if consumed != bytes.len() {
        return Err(invalid_record(format!("{name} contains trailing data")));
    }
    Ok(value)
}

fn encode_before_image(bytes: Vec<u8>) -> Result<BeforeImageData, RecordError> {
    if bytes.is_empty() || bytes.len() > DISK_PAGE_BYTES {
        return Err(invalid_record("invalid disk before-image length"));
    }
    if bytes.iter().all(|&byte| byte == 0) {
        Ok(BeforeImageData::Zero(bytes.len()))
    } else {
        Ok(BeforeImageData::Raw(bytes))
    }
}

fn before_image_length(data: &BeforeImageData) -> Result<usize, RecordError> {
    let length = match data {
        BeforeImageData::Raw(bytes) => bytes.len(),
        BeforeImageData::Zero(length) => *length,
    };
    if length == 0 || length > DISK_PAGE_BYTES {
        Err(invalid_record("invalid disk before-image length"))
    } else {
        Ok(length)
    }
}

fn decode_before_image(data: BeforeImageData) -> Result<Vec<u8>, RecordError> {
    before_image_length(&data)?;
    Ok(match data {
        BeforeImageData::Raw(bytes) => bytes,
        BeforeImageData::Zero(length) => vec![0; length],
    })
}

fn identity_for_open_file(file: &mut File) -> io::Result<RecordIdentity> {
    let original_position = file.stream_position()?;
    let result: io::Result<RecordIdentity> = (|| {
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
        Ok(RecordIdentity {
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

fn snapshot_file_name(position: ExecutionPosition) -> String {
    format!(
        "{:020}-{:020}.ckpt",
        position.epoch, position.completed_instructions
    )
}

fn index_path(record_path: &Path) -> PathBuf {
    append_path_suffix(record_path, ".idx")
}

fn checkpoint_directory(record_path: &Path) -> PathBuf {
    append_path_suffix(record_path, ".ckpt")
}

fn append_path_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut value = path.as_os_str().to_os_string();
    value.push(suffix);
    PathBuf::from(value)
}

fn validate_snapshot_id(snapshot_id: &str) -> Result<(), RecordError> {
    let path = Path::new(snapshot_id);
    if path.file_name().and_then(|name| name.to_str()) != Some(snapshot_id)
        || path.extension().and_then(|extension| extension.to_str()) != Some("ckpt")
    {
        return Err(invalid_record("invalid Replay snapshot identifier"));
    }
    Ok(())
}

fn write_cache_file<T: Serialize>(
    path: &Path,
    magic: [u8; 8],
    value: &T,
) -> Result<(u64, [u8; 32]), RecordError> {
    let payload = encode_value(value, "Replay cache")?;
    let payload_length = u64::try_from(payload.len())
        .map_err(|_| invalid_record("Replay cache payload length overflow"))?;
    let mut bytes = Vec::with_capacity(CACHE_HEADER_BYTES + payload.len());
    bytes.extend_from_slice(&magic);
    bytes.extend_from_slice(&CACHE_SCHEMA.to_le_bytes());
    bytes.extend_from_slice(&payload_length.to_le_bytes());
    bytes.extend_from_slice(&crc32(&payload).to_le_bytes());
    bytes.extend_from_slice(&payload);

    let partial_path = append_path_suffix(path, ".partial");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&partial_path)?;
    let result: io::Result<()> = (|| {
        file.write_all(&bytes)?;
        file.flush()?;
        file.sync_all()?;
        Ok(())
    })();
    drop(file);
    result?;
    replace_file(&partial_path, path)?;

    let size_bytes = bytes.len() as u64;
    let sha256 = Sha256::digest(&bytes).into();
    Ok((size_bytes, sha256))
}

fn read_cache_file<T: DeserializeOwned>(
    path: &Path,
    magic: [u8; 8],
    name: &str,
) -> Result<T, RecordError> {
    read_cache_file_with_identity(path, magic, name).map(|(value, _, _)| value)
}

fn read_cache_file_with_identity<T: DeserializeOwned>(
    path: &Path,
    magic: [u8; 8],
    name: &str,
) -> Result<(T, u64, [u8; 32]), RecordError> {
    let bytes = fs::read(path)?;
    if bytes.len() < CACHE_HEADER_BYTES || bytes[..8] != magic {
        return Err(invalid_record(format!("invalid {name} magic")));
    }
    if u32::from_le_bytes(bytes[8..12].try_into().unwrap()) != CACHE_SCHEMA {
        return Err(invalid_record(format!("unsupported {name} schema")));
    }
    let payload_length = usize::try_from(u64::from_le_bytes(bytes[12..20].try_into().unwrap()))
        .map_err(|_| invalid_record(format!("{name} length does not fit usize")))?;
    if payload_length != bytes.len() - CACHE_HEADER_BYTES {
        return Err(invalid_record(format!("{name} has the wrong length")));
    }
    let payload = &bytes[CACHE_HEADER_BYTES..];
    if crc32(payload) != u32::from_le_bytes(bytes[20..24].try_into().unwrap()) {
        return Err(invalid_record(format!("{name} CRC mismatch")));
    }
    let value = decode_value(payload, name)?;
    Ok((value, bytes.len() as u64, Sha256::digest(&bytes).into()))
}

fn load_or_rebuild_catalog(
    record_path: &Path,
    record_identity: &RecordIdentity,
) -> Result<SnapshotCatalog, RecordError> {
    let path = index_path(record_path);
    if let Ok(catalog) = read_cache_file::<SnapshotCatalog>(&path, INDEX_MAGIC, "Replay index")
        && catalog.record == *record_identity
        && validate_catalog(record_path, &catalog).is_ok()
    {
        return Ok(catalog);
    }
    rebuild_catalog(record_path, record_identity)
}

fn validate_catalog(record_path: &Path, catalog: &SnapshotCatalog) -> Result<(), RecordError> {
    let mut ids = BTreeSet::new();
    let mut positions = BTreeSet::new();
    for entry in &catalog.entries {
        validate_snapshot_id(&entry.id)?;
        if !ids.insert(entry.id.as_str()) || !positions.insert(entry.position) {
            return Err(invalid_record("Replay index contains duplicate entries"));
        }
        let path = checkpoint_directory(record_path).join(&entry.id);
        if fs::metadata(path)?.len() != entry.file_size_bytes {
            return Err(invalid_record(
                "Replay snapshot size does not match its index",
            ));
        }
    }
    if catalog
        .entries
        .windows(2)
        .any(|entries| entries[0].position >= entries[1].position)
    {
        return Err(invalid_record("Replay index entries are not sorted"));
    }
    Ok(())
}

fn rebuild_catalog(
    record_path: &Path,
    record_identity: &RecordIdentity,
) -> Result<SnapshotCatalog, RecordError> {
    let directory = checkpoint_directory(record_path);
    let mut entries = Vec::new();
    match fs::read_dir(&directory) {
        Ok(files) => {
            for file in files {
                let Ok(file) = file else {
                    continue;
                };
                let path = file.path();
                let Some(id) = path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(String::from)
                else {
                    continue;
                };
                if validate_snapshot_id(&id).is_err() {
                    continue;
                }
                let Ok((snapshot, file_size_bytes, file_sha256)) =
                    read_cache_file_with_identity::<ReplaySnapshot>(
                        &path,
                        SNAPSHOT_MAGIC,
                        "Replay snapshot",
                    )
                else {
                    continue;
                };
                if snapshot.record != *record_identity {
                    continue;
                }
                entries.push(SnapshotCatalogEntry {
                    id,
                    position: snapshot.position,
                    pc: snapshot.pc,
                    timeline_cursor: snapshot.timeline_cursor,
                    last_verified_checkpoint: snapshot.last_verified_checkpoint,
                    machine_fingerprint: snapshot.machine_fingerprint,
                    file_size_bytes,
                    file_sha256,
                });
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(RecordError::Io(error)),
    }
    entries.sort_by_key(|entry| entry.position);
    entries.dedup_by_key(|entry| entry.position);
    let catalog = SnapshotCatalog {
        record: record_identity.clone(),
        entries,
    };
    write_cache_file(&index_path(record_path), INDEX_MAGIC, &catalog)?;
    Ok(catalog)
}

fn validate_replay_snapshot(
    replayer: &Replayer,
    snapshot: &ReplaySnapshot,
) -> Result<(), RecordError> {
    if snapshot.record != replayer.record_identity {
        return Err(invalid_record(
            "Replay snapshot belongs to a different Record",
        ));
    }
    if snapshot.timeline_cursor > replayer.timeline.len()
        || snapshot.position > replayer.footer.position
    {
        return Err(invalid_record(
            "Replay snapshot contains an invalid execution position",
        ));
    }
    if replayer.timeline[..snapshot.timeline_cursor]
        .last()
        .is_some_and(|entry| entry.position > snapshot.position)
        || replayer.timeline[snapshot.timeline_cursor..]
            .first()
            .is_some_and(|entry| entry.position <= snapshot.position)
    {
        return Err(invalid_record(
            "Replay snapshot Timeline cursor is inconsistent",
        ));
    }
    let last_checkpoint = replayer.timeline[..snapshot.timeline_cursor]
        .iter()
        .rev()
        .find(|entry| matches!(entry.action, TimelineAction::Checkpoint { .. }))
        .map(|entry| entry.position);
    if snapshot.last_verified_checkpoint != last_checkpoint {
        return Err(invalid_record(
            "Replay snapshot checkpoint history is inconsistent",
        ));
    }

    match replayer.manifest.disk() {
        Some(disk) => {
            for (&page_index, page) in &snapshot.cow_pages {
                validate_before_image(page_index, before_image_length(page)?, disk.size_bytes)?;
            }
        }
        None if snapshot.cow_pages.is_empty() => {}
        None => {
            return Err(invalid_record(
                "Replay snapshot has COW pages without a disk",
            ));
        }
    }
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
            let data = encode_before_image(bytes)?;
            write_frame(
                &mut inner,
                &RecordFrame::DiskBeforeImage { page_index, data },
            )?;
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
    /// Returns an error when the range overflows or Replay storage failed.
    pub fn overlay_read(&self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let mut state = lock_unpoisoned(&self.storage.state);
        ensure_replay_storage_healthy(&state)?;
        overlay_range(&mut state, offset, buffer, true)
    }

    /// Applies only Before Images while validating the logical initial disk.
    ///
    /// # Errors
    ///
    /// Returns an error when the range overflows or Replay storage failed.
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
    /// Returns an error for invalid ranges or base reads.
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
    state.file.seek(SeekFrom::Start(location.payload_offset))?;
    let mut payload = vec![0; location.payload_length];
    state.file.read_exact(&mut payload)?;
    if crc32(&payload) != location.payload_crc {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Record before-image frame CRC mismatch",
        ));
    }
    let frame = decode_value::<RecordFrame>(&payload, "Record before-image frame")
        .map_err(record_io_error)?;
    let RecordFrame::DiskBeforeImage {
        page_index: decoded_page_index,
        data,
    } = frame
    else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Record before-image index refers to another frame type",
        ));
    };
    if decoded_page_index != page_index {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Record before-image index refers to another page",
        ));
    }
    decode_before_image(data).map(Some).map_err(record_io_error)
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

    use se_core::time::VirtualInstant;
    use se_float::backend::Backend;
    use se_machine::indigo::ip12::{
        Ip12, Ip12MemoryConfiguration, Ip12NonvolatileState, Ip12NonvolatileStateParts,
    };
    use se_machine::machine::{Machine, MachineNonvolatileState, MachineStartupConfiguration};

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
            startup_configuration(),
            MediaIdentity::from_bytes(Path::new("prom.bin"), &[1, 2, 3]),
            None,
            None,
            nonvolatile_state(),
        )
    }

    fn startup_configuration() -> MachineStartupConfiguration {
        MachineStartupConfiguration::IndigoIp12 {
            floating_point_backend: Backend::SoftFloat,
            memory: Ip12MemoryConfiguration::try_from_simm_mib([2, 0, 8]).unwrap(),
        }
    }

    fn nonvolatile_state() -> MachineNonvolatileState {
        let mut words = [u16::MAX; 64];
        words[3] = 0x1234;
        MachineNonvolatileState::IndigoIp12(
            Ip12NonvolatileState::try_from_parts(Ip12NonvolatileStateParts {
                nvram_words: words,
                rtc_registers: [0; 32],
                rtc_alternate_control_registers: [0; 4],
                rtc_prescaler_phase_attoseconds: 0,
                rtc_millisecond_within_hundredth: 0,
                rtc_oscillator_failed: false,
                rtc_single_supply: false,
                rtc_alarm_match_active: false,
            })
            .unwrap(),
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
        assert_eq!(replayer.manifest().machine(), &startup_configuration());
        assert_eq!(
            replayer.manifest().nonvolatile_state(),
            &nonvolatile_state()
        );
        let (session, restore) = replayer.into_session();
        assert!(restore.is_none());
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
        assert_eq!(replayer.manifest().machine(), &startup_configuration());
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
    fn machine_nonvolatile_state_round_trips() {
        let path = temporary_path("opaque-state");
        write_complete_record(&path);

        let replayer = Replayer::open(&path).unwrap();
        assert_eq!(
            replayer.manifest().nonvolatile_state(),
            &nonvolatile_state()
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
            startup_configuration(),
            MediaIdentity::from_bytes(Path::new("prom.bin"), &[1, 2, 3]),
            Some(MediaIdentity::from_bytes(Path::new("disk.img"), disk)),
            None,
            nonvolatile_state(),
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
    fn replay_snapshot_round_trips_cow_pages() {
        let path = temporary_path("snapshot-cow");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.idx", path.display()));
        let _ = fs::remove_dir_all(format!("{}.ckpt", path.display()));
        let initial = vec![1; DISK_PAGE_BYTES];
        let recorder = Recorder::create(&path).unwrap();
        recorder.start(&disk_manifest(&initial)).unwrap();
        recorder
            .disk()
            .capture_before_image(0, || Ok(initial.clone()))
            .unwrap();
        recorder
            .finalize(ExecutionPosition::default(), &RecordOutcome::UserStopped)
            .unwrap();

        let replayer = Replayer::open(&path).unwrap();
        replayer
            .disk()
            .write_all_at(10, &[7, 8], DISK_PAGE_BYTES as u64, |_offset, page| {
                page.fill(9);
                Ok(())
            })
            .unwrap();
        let (session, restore) = replayer.into_session();
        assert!(restore.is_none());
        let machine = Machine::IndigoIp12(
            Ip12::new(vec![0; 0x40000], Backend::SoftFloat, None, None).unwrap(),
        );
        let info = session
            .create_snapshot(
                ExecutionPosition::default(),
                0,
                VirtualInstant::ZERO,
                0,
                [3; 32],
                machine.snapshot().unwrap(),
                0xbfc0_0000,
            )
            .unwrap();

        let restored = Replayer::open_snapshot(&path, info.id()).unwrap();
        let restored_disk = restored.disk();
        let mut bytes = vec![9; DISK_PAGE_BYTES];
        restored_disk.overlay_read(0, &mut bytes).unwrap();
        assert_eq!(&bytes[..10], &initial[..10]);
        assert_eq!(&bytes[10..12], &[7, 8]);
        assert_eq!(&bytes[12..], &initial[12..]);

        drop(restored_disk);
        drop(restored);
        fs::remove_file(format!("{}.idx", path.display())).unwrap();
        fs::remove_dir_all(format!("{}.ckpt", path.display())).unwrap();
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
