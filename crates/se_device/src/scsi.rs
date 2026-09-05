//! Shared types and functional bus coordination for SCSI targets.

use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::scsi_cdrom::ScsiCdrom;
use crate::scsi_disk::ScsiDisk;
use crate::storage::BlockStorage;

const FIXED_SENSE_BYTES: usize = 18;
const TARGET_COUNT: usize = 8;
const LUN_COUNT: usize = 8;
const TARGET_SLOT_COUNT: usize = TARGET_COUNT * LUN_COUNT;

/// The status returned by a SCSI target command.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ScsiStatus {
    /// The command completed successfully.
    Good,
    /// Sense data describes why the command could not complete.
    CheckCondition,
}

impl ScsiStatus {
    /// Returns the status byte placed on the SCSI bus.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Self::Good => 0x00,
            Self::CheckCondition => 0x02,
        }
    }
}

/// Work requested by one decoded SCSI command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ScsiCommandPlan {
    /// The target can complete without consulting block storage.
    Complete {
        /// Target status.
        status: ScsiStatus,
        /// Bytes returned to the initiator.
        data_in: Vec<u8>,
    },
    /// The bus must read one byte range from the attached storage.
    ReadStorage {
        /// First byte offset in the attached storage.
        offset: u64,
        /// Number of bytes to return to the initiator.
        byte_count: u64,
    },
    /// The bus must write one byte range to the attached storage.
    WriteStorage {
        /// First byte offset in the attached storage.
        offset: u64,
        /// Number of bytes to receive from the initiator.
        byte_count: u64,
    },
}

/// A functional SCSI target attached to [`ScsiBus`].
pub trait ScsiTarget: Send {
    /// Returns the required backing-storage size in bytes.
    fn storage_size_bytes(&self) -> u64;

    /// Decodes one command descriptor block.
    fn execute(&mut self, cdb: &[u8]) -> ScsiCommandPlan;

    /// Completes storage-backed I/O and returns its target status.
    fn complete_storage(&mut self, succeeded: bool) -> ScsiStatus;

    /// Captures target-local protocol state without its backing storage.
    fn snapshot(&self) -> Option<ScsiTargetSnapshot> {
        None
    }

    /// Reports whether a snapshot can be restored without changing topology.
    fn accepts_snapshot(&self, _snapshot: &ScsiTargetSnapshot) -> bool {
        false
    }

    /// Restores target-local state while preserving the backing storage.
    fn restore_snapshot(&mut self, _snapshot: ScsiTargetSnapshot) -> bool {
        false
    }
}

/// Restorable state of a supported functional SCSI target.
#[derive(Clone, Deserialize, Serialize)]
pub enum ScsiTargetSnapshot {
    /// Direct-access disk state.
    Disk(ScsiDisk),
    /// Read-only CD-ROM state.
    Cdrom(ScsiCdrom),
}

impl ScsiTargetSnapshot {
    fn storage_size_bytes(&self) -> u64 {
        match self {
            Self::Disk(target) => target.storage_size_bytes(),
            Self::Cdrom(target) => target.storage_size_bytes(),
        }
    }
}

/// An invalid storage capacity supplied to a SCSI target constructor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScsiStorageSizeError {
    bytes: u64,
}

impl ScsiStorageSizeError {
    pub(crate) const fn new(bytes: u64) -> Self {
        Self { bytes }
    }

    /// Returns the rejected storage capacity in bytes.
    #[must_use]
    pub const fn bytes(self) -> u64 {
        self.bytes
    }
}

impl fmt::Display for ScsiStorageSizeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid SCSI target storage size: {} bytes",
            self.bytes
        )
    }
}

impl Error for ScsiStorageSizeError {}

/// An error encountered while attaching a target and storage to a SCSI bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiAttachError {
    /// The target or LUN is outside the eight-by-eight address space.
    InvalidAddress {
        /// Requested target ID.
        target_id: u8,
        /// Requested logical unit number.
        lun: u8,
    },
    /// The requested target and LUN already contain an attachment.
    AddressOccupied {
        /// Requested target ID.
        target_id: u8,
        /// Requested logical unit number.
        lun: u8,
    },
    /// The target capacity differs from the attached storage capacity.
    StorageSizeMismatch {
        /// Capacity required by the target.
        target_bytes: u64,
        /// Capacity reported by the storage object.
        storage_bytes: u64,
    },
}

impl fmt::Display for ScsiAttachError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress { target_id, lun } => {
                write!(
                    formatter,
                    "invalid SCSI address: target {target_id}, LUN {lun}"
                )
            }
            Self::AddressOccupied { target_id, lun } => {
                write!(
                    formatter,
                    "SCSI target {target_id}, LUN {lun} is already occupied"
                )
            }
            Self::StorageSizeMismatch {
                target_bytes,
                storage_bytes,
            } => write!(
                formatter,
                "SCSI target requires {target_bytes} storage bytes, attached storage has {storage_bytes} bytes"
            ),
        }
    }
}

impl Error for ScsiAttachError {}

/// A caller error encountered while coordinating a SCSI transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiBusError {
    /// The target or LUN is outside the eight-by-eight address space.
    InvalidAddress {
        /// Requested target ID.
        target_id: u8,
        /// Requested logical unit number.
        lun: u8,
    },
    /// A transaction is already active for the reported address.
    TransactionActive {
        /// Active target ID.
        target_id: u8,
        /// Active logical unit number.
        lun: u8,
    },
    /// No Data In transaction is active.
    NoDataInTransaction,
    /// No Data Out transaction is active.
    NoDataOutTransaction,
    /// The caller supplied a zero-byte transfer limit.
    EmptyDataBuffer,
}

impl fmt::Display for ScsiBusError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidAddress { target_id, lun } => {
                write!(
                    formatter,
                    "invalid SCSI address: target {target_id}, LUN {lun}"
                )
            }
            Self::TransactionActive { target_id, lun } => write!(
                formatter,
                "SCSI transaction is already active for target {target_id}, LUN {lun}"
            ),
            Self::NoDataInTransaction => {
                formatter.write_str("no SCSI Data In transaction is active")
            }
            Self::NoDataOutTransaction => {
                formatter.write_str("no SCSI Data Out transaction is active")
            }
            Self::EmptyDataBuffer => formatter.write_str("SCSI transfer limit is zero"),
        }
    }
}

impl Error for ScsiBusError {}

/// The result of starting one target command.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiCommandStart {
    /// No target is attached at the selected address.
    SelectionTimeout,
    /// The target completed without a data transaction.
    Complete {
        /// Final target status.
        status: ScsiStatus,
    },
    /// The target has bytes available through [`ScsiBus::transfer_data_in`].
    DataIn {
        /// Total bytes in the new Data In transaction.
        byte_count: u64,
    },
    /// The target expects bytes through [`ScsiBus::transfer_data_out`].
    DataOut {
        /// Total bytes in the new Data Out transaction.
        byte_count: u64,
    },
}

/// The data direction of an active SCSI transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiDataDirection {
    /// Bytes move from the target to the initiator.
    In,
    /// Bytes move from the initiator to the target.
    Out,
}

/// The result of one accepted or rejected data-transfer chunk.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScsiTransferResult {
    /// The callback rejected the chunk and the transaction did not advance.
    Rejected,
    /// The chunk was accepted and more bytes remain.
    More {
        /// Bytes accepted in this call.
        transferred: usize,
        /// Bytes remaining in the transaction.
        remaining: u64,
    },
    /// The transaction ended with a target status.
    Complete {
        /// Bytes accepted in this call.
        transferred: usize,
        /// Final target status.
        status: ScsiStatus,
    },
}

struct TargetAttachment {
    target: Box<dyn ScsiTarget>,
    storage: Box<dyn BlockStorage>,
}

type TargetRegistry = [Option<TargetAttachment>; TARGET_SLOT_COUNT];

#[derive(Clone, Deserialize, Serialize)]
struct ScsiTransaction {
    target_slot: usize,
    transfer: ScsiTransfer,
}

#[derive(Clone, Deserialize, Serialize)]
enum ScsiTransfer {
    ImmediateDataIn {
        data: Vec<u8>,
        next_offset: usize,
        final_status: ScsiStatus,
    },
    StorageDataIn {
        next_offset: u64,
        remaining: u64,
    },
    StorageDataOut {
        next_offset: u64,
        remaining: u64,
    },
}

/// A functional SCSI bus with fixed target/LUN attachment slots.
pub struct ScsiBus {
    targets: TargetRegistry,
    active_transaction: Option<ScsiTransaction>,
}

/// Complete restorable SCSI protocol state without backing-storage objects.
#[derive(Clone, Deserialize, Serialize)]
pub struct ScsiBusSnapshot {
    targets: Vec<(u8, u8, ScsiTargetSnapshot)>,
    active_transaction: Option<ScsiTransaction>,
}

/// A snapshot that is incompatible with the cold-constructed SCSI topology.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScsiSnapshotError;

impl fmt::Display for ScsiSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SCSI snapshot does not match the configured topology or storage")
    }
}

impl Error for ScsiSnapshotError {}

impl ScsiBus {
    /// Creates an empty bus with no active transaction.
    #[must_use]
    pub fn new() -> Self {
        Self {
            targets: std::array::from_fn(|_| None),
            active_transaction: None,
        }
    }

    /// Captures target and active-transaction state without storage objects.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiSnapshotError`] when an attached target does not expose
    /// restorable state.
    pub fn snapshot(&self) -> Result<ScsiBusSnapshot, ScsiSnapshotError> {
        let mut targets = Vec::new();
        for (slot, attachment) in self.targets.iter().enumerate() {
            let Some(attachment) = attachment else {
                continue;
            };
            let snapshot = attachment.target.snapshot().ok_or(ScsiSnapshotError)?;
            let (target_id, lun) = address_for_slot(slot);
            targets.push((target_id, lun, snapshot));
        }
        Ok(ScsiBusSnapshot {
            targets,
            active_transaction: self.active_transaction.clone(),
        })
    }

    /// Restores protocol state while retaining the currently attached storage
    /// objects.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiSnapshotError`] without changing state when target
    /// topology, capacity, or an active transfer is incompatible.
    pub fn restore_snapshot(&mut self, snapshot: ScsiBusSnapshot) -> Result<(), ScsiSnapshotError> {
        let mut target_states: [Option<ScsiTargetSnapshot>; TARGET_SLOT_COUNT] =
            std::array::from_fn(|_| None);
        for (target_id, lun, state) in snapshot.targets {
            let slot = target_slot(target_id, lun).ok_or(ScsiSnapshotError)?;
            if target_states[slot].replace(state).is_some() {
                return Err(ScsiSnapshotError);
            }
        }

        for (attachment, state) in self.targets.iter().zip(&target_states) {
            match (attachment, state) {
                (None, None) => {}
                (Some(attachment), Some(state))
                    if attachment.target.accepts_snapshot(state)
                        && state.storage_size_bytes() == attachment.storage.size_bytes() => {}
                _ => return Err(ScsiSnapshotError),
            }
        }
        if let Some(transaction) = &snapshot.active_transaction {
            validate_snapshot_transaction(transaction, &self.targets)?;
        }

        for (attachment, state) in self.targets.iter_mut().zip(target_states) {
            let (Some(attachment), Some(state)) = (attachment, state) else {
                continue;
            };
            if !attachment.target.restore_snapshot(state) {
                return Err(ScsiSnapshotError);
            }
        }
        self.active_transaction = snapshot.active_transaction;
        Ok(())
    }

    /// Attaches one target and its backing storage.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiAttachError`] for an invalid or occupied address, or when
    /// target and storage capacities differ.
    pub fn attach(
        &mut self,
        target_id: u8,
        lun: u8,
        target: Box<dyn ScsiTarget>,
        storage: Box<dyn BlockStorage>,
    ) -> Result<(), ScsiAttachError> {
        let slot = target_slot(target_id, lun)
            .ok_or(ScsiAttachError::InvalidAddress { target_id, lun })?;
        if self.targets[slot].is_some() {
            return Err(ScsiAttachError::AddressOccupied { target_id, lun });
        }
        let target_bytes = target.storage_size_bytes();
        let storage_bytes = storage.size_bytes();
        if target_bytes != storage_bytes {
            return Err(ScsiAttachError::StorageSizeMismatch {
                target_bytes,
                storage_bytes,
            });
        }
        self.targets[slot] = Some(TargetAttachment { target, storage });
        Ok(())
    }

    /// Starts one command for the selected target and LUN.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiBusError`] for an invalid address or when another
    /// transaction is active.
    pub fn start_command(
        &mut self,
        target_id: u8,
        lun: u8,
        cdb: &[u8],
    ) -> Result<ScsiCommandStart, ScsiBusError> {
        let slot =
            target_slot(target_id, lun).ok_or(ScsiBusError::InvalidAddress { target_id, lun })?;
        if let Some((target_id, lun)) = self.active_address() {
            return Err(ScsiBusError::TransactionActive { target_id, lun });
        }
        let Some(attachment) = self.targets[slot].as_mut() else {
            return Ok(ScsiCommandStart::SelectionTimeout);
        };
        let plan = attachment.target.execute(cdb);

        match plan {
            ScsiCommandPlan::Complete { status, data_in } if data_in.is_empty() => {
                Ok(ScsiCommandStart::Complete { status })
            }
            ScsiCommandPlan::Complete { status, data_in } => {
                let byte_count = data_in.len() as u64;
                self.active_transaction = Some(ScsiTransaction {
                    target_slot: slot,
                    transfer: ScsiTransfer::ImmediateDataIn {
                        data: data_in,
                        next_offset: 0,
                        final_status: status,
                    },
                });
                Ok(ScsiCommandStart::DataIn { byte_count })
            }
            ScsiCommandPlan::ReadStorage { offset, byte_count } => {
                Ok(self.start_storage_transfer(slot, offset, byte_count, ScsiDataDirection::In))
            }
            ScsiCommandPlan::WriteStorage { offset, byte_count } => {
                Ok(self.start_storage_transfer(slot, offset, byte_count, ScsiDataDirection::Out))
            }
        }
    }

    fn start_storage_transfer(
        &mut self,
        target_slot: usize,
        offset: u64,
        byte_count: u64,
        direction: ScsiDataDirection,
    ) -> ScsiCommandStart {
        if byte_count == 0 {
            let status = self.complete_target_storage(target_slot, true);
            return ScsiCommandStart::Complete { status };
        }
        let range_is_valid = offset.checked_add(byte_count).is_some_and(|end| {
            self.targets[target_slot]
                .as_ref()
                .is_some_and(|attachment| end <= attachment.storage.size_bytes())
        });
        if !range_is_valid {
            let status = self.complete_target_storage(target_slot, false);
            return ScsiCommandStart::Complete { status };
        }
        let transfer = match direction {
            ScsiDataDirection::In => ScsiTransfer::StorageDataIn {
                next_offset: offset,
                remaining: byte_count,
            },
            ScsiDataDirection::Out => ScsiTransfer::StorageDataOut {
                next_offset: offset,
                remaining: byte_count,
            },
        };
        self.active_transaction = Some(ScsiTransaction {
            target_slot,
            transfer,
        });
        match direction {
            ScsiDataDirection::In => ScsiCommandStart::DataIn { byte_count },
            ScsiDataDirection::Out => ScsiCommandStart::DataOut { byte_count },
        }
    }

    /// Returns the address owning the active transaction.
    #[must_use]
    pub fn active_address(&self) -> Option<(u8, u8)> {
        self.active_transaction
            .as_ref()
            .map(|transaction| address_for_slot(transaction.target_slot))
    }

    /// Returns the direction of the active data transaction.
    #[must_use]
    pub fn active_data_direction(&self) -> Option<ScsiDataDirection> {
        self.active_transaction
            .as_ref()
            .map(|transaction| match &transaction.transfer {
                ScsiTransfer::ImmediateDataIn { .. } | ScsiTransfer::StorageDataIn { .. } => {
                    ScsiDataDirection::In
                }
                ScsiTransfer::StorageDataOut { .. } => ScsiDataDirection::Out,
            })
    }

    /// Offers at most `maximum_bytes` from the active Data In transaction.
    ///
    /// The transaction advances only when `accept` returns `true`. The
    /// callback is not invoked when the host storage read fails.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiBusError`] when the limit is zero or no Data In
    /// transaction is active.
    pub fn transfer_data_in(
        &mut self,
        maximum_bytes: usize,
        accept: impl FnOnce(&[u8]) -> bool,
    ) -> Result<ScsiTransferResult, ScsiBusError> {
        if maximum_bytes == 0 {
            return Err(ScsiBusError::EmptyDataBuffer);
        }
        let Some(transaction) = self.active_transaction.as_ref() else {
            return Err(ScsiBusError::NoDataInTransaction);
        };

        match &transaction.transfer {
            ScsiTransfer::ImmediateDataIn { .. } => {
                self.transfer_immediate_data_in(maximum_bytes, accept)
            }
            ScsiTransfer::StorageDataIn { .. } => {
                self.transfer_storage_data_in(maximum_bytes, accept)
            }
            ScsiTransfer::StorageDataOut { .. } => Err(ScsiBusError::NoDataInTransaction),
        }
    }

    /// Requests at most `maximum_bytes` for the active Data Out transaction.
    ///
    /// The callback receives the exact chunk buffer and must fill it before
    /// returning `true`. A rejected callback leaves the transaction and
    /// storage unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`ScsiBusError`] when the limit is zero or no Data Out
    /// transaction is active.
    pub fn transfer_data_out(
        &mut self,
        maximum_bytes: usize,
        provide: impl FnOnce(&mut [u8]) -> bool,
    ) -> Result<ScsiTransferResult, ScsiBusError> {
        if maximum_bytes == 0 {
            return Err(ScsiBusError::EmptyDataBuffer);
        }
        let Some(ScsiTransaction {
            target_slot,
            transfer:
                ScsiTransfer::StorageDataOut {
                    next_offset,
                    remaining,
                },
        }) = self.active_transaction.as_ref()
        else {
            return Err(ScsiBusError::NoDataOutTransaction);
        };
        let target_slot = *target_slot;
        let next_offset = *next_offset;
        let active_remaining = *remaining;
        let byte_count = maximum_bytes.min(usize::try_from(active_remaining).unwrap_or(usize::MAX));
        let mut bytes = vec![0; byte_count];
        if !provide(&mut bytes) {
            return Ok(ScsiTransferResult::Rejected);
        }

        let write_succeeded = self.targets[target_slot]
            .as_mut()
            .is_some_and(|attachment| attachment.storage.write_all_at(next_offset, &bytes).is_ok());
        if !write_succeeded {
            self.active_transaction = None;
            let status = self.complete_target_storage(target_slot, false);
            return Ok(ScsiTransferResult::Complete {
                transferred: 0,
                status,
            });
        }

        let byte_count_u64 = byte_count as u64;
        let remaining = active_remaining - byte_count_u64;
        if remaining == 0 {
            self.active_transaction = None;
            let status = self.complete_target_storage(target_slot, true);
            return Ok(ScsiTransferResult::Complete {
                transferred: byte_count,
                status,
            });
        }
        if let Some(ScsiTransaction {
            transfer:
                ScsiTransfer::StorageDataOut {
                    next_offset,
                    remaining: active_remaining,
                },
            ..
        }) = self.active_transaction.as_mut()
        {
            *next_offset = (*next_offset)
                .checked_add(byte_count_u64)
                .expect("validated SCSI storage range cannot overflow");
            *active_remaining = remaining;
        }
        Ok(ScsiTransferResult::More {
            transferred: byte_count,
            remaining,
        })
    }

    /// Cancels the active transaction without changing target state.
    pub fn cancel_transaction(&mut self) {
        self.active_transaction = None;
    }

    fn transfer_immediate_data_in(
        &mut self,
        maximum_bytes: usize,
        accept: impl FnOnce(&[u8]) -> bool,
    ) -> Result<ScsiTransferResult, ScsiBusError> {
        let Some(ScsiTransaction {
            transfer:
                ScsiTransfer::ImmediateDataIn {
                    data,
                    next_offset,
                    final_status,
                },
            ..
        }) = self.active_transaction.as_ref()
        else {
            return Err(ScsiBusError::NoDataInTransaction);
        };
        let end = next_offset.saturating_add(maximum_bytes).min(data.len());
        let transferred = end - *next_offset;
        let status = *final_status;
        if !accept(&data[*next_offset..end]) {
            return Ok(ScsiTransferResult::Rejected);
        }

        if end == data.len() {
            self.active_transaction = None;
            return Ok(ScsiTransferResult::Complete {
                transferred,
                status,
            });
        }
        let remaining = (data.len() - end) as u64;
        if let Some(ScsiTransaction {
            transfer: ScsiTransfer::ImmediateDataIn { next_offset, .. },
            ..
        }) = self.active_transaction.as_mut()
        {
            *next_offset = end;
        }
        Ok(ScsiTransferResult::More {
            transferred,
            remaining,
        })
    }

    fn transfer_storage_data_in(
        &mut self,
        maximum_bytes: usize,
        accept: impl FnOnce(&[u8]) -> bool,
    ) -> Result<ScsiTransferResult, ScsiBusError> {
        let Some(ScsiTransaction {
            target_slot,
            transfer:
                ScsiTransfer::StorageDataIn {
                    next_offset,
                    remaining,
                },
        }) = self.active_transaction.as_ref()
        else {
            return Err(ScsiBusError::NoDataInTransaction);
        };
        let target_slot = *target_slot;
        let next_offset = *next_offset;
        let active_remaining = *remaining;
        let byte_count = maximum_bytes.min(usize::try_from(active_remaining).unwrap_or(usize::MAX));
        let mut bytes = vec![0; byte_count];
        let read_succeeded = self.targets[target_slot]
            .as_mut()
            .is_some_and(|attachment| {
                attachment
                    .storage
                    .read_exact_at(next_offset, &mut bytes)
                    .is_ok()
            });
        if !read_succeeded {
            self.active_transaction = None;
            let status = self.complete_target_storage(target_slot, false);
            return Ok(ScsiTransferResult::Complete {
                transferred: 0,
                status,
            });
        }
        if !accept(&bytes) {
            return Ok(ScsiTransferResult::Rejected);
        }

        let byte_count_u64 = byte_count as u64;
        let remaining = active_remaining - byte_count_u64;
        if remaining == 0 {
            self.active_transaction = None;
            let status = self.complete_target_storage(target_slot, true);
            return Ok(ScsiTransferResult::Complete {
                transferred: byte_count,
                status,
            });
        }
        if let Some(ScsiTransaction {
            transfer:
                ScsiTransfer::StorageDataIn {
                    next_offset,
                    remaining: active_remaining,
                },
            ..
        }) = self.active_transaction.as_mut()
        {
            *next_offset += byte_count_u64;
            *active_remaining = remaining;
        }
        Ok(ScsiTransferResult::More {
            transferred: byte_count,
            remaining,
        })
    }

    fn complete_target_storage(&mut self, slot: usize, succeeded: bool) -> ScsiStatus {
        self.targets[slot]
            .as_mut()
            .map_or(ScsiStatus::CheckCondition, |attachment| {
                attachment.target.complete_storage(succeeded)
            })
    }
}

fn validate_snapshot_transaction(
    transaction: &ScsiTransaction,
    targets: &TargetRegistry,
) -> Result<(), ScsiSnapshotError> {
    let attachment = targets
        .get(transaction.target_slot)
        .and_then(Option::as_ref)
        .ok_or(ScsiSnapshotError)?;
    match &transaction.transfer {
        ScsiTransfer::ImmediateDataIn {
            data, next_offset, ..
        } if *next_offset < data.len() => Ok(()),
        ScsiTransfer::StorageDataIn {
            next_offset,
            remaining,
        }
        | ScsiTransfer::StorageDataOut {
            next_offset,
            remaining,
        } if *remaining != 0
            && next_offset
                .checked_add(*remaining)
                .is_some_and(|end| end <= attachment.storage.size_bytes()) =>
        {
            Ok(())
        }
        _ => Err(ScsiSnapshotError),
    }
}

impl Default for ScsiBus {
    fn default() -> Self {
        Self::new()
    }
}

const fn target_slot(target_id: u8, lun: u8) -> Option<usize> {
    if target_id < TARGET_COUNT as u8 && lun < LUN_COUNT as u8 {
        Some(target_id as usize * LUN_COUNT + lun as usize)
    } else {
        None
    }
}

const fn address_for_slot(slot: usize) -> (u8, u8) {
    ((slot / LUN_COUNT) as u8, (slot % LUN_COUNT) as u8)
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct SenseData {
    key: u8,
    asc: u8,
    ascq: u8,
}

impl SenseData {
    pub(crate) const NONE: Self = Self::new(0, 0, 0);
    pub(crate) const UNSUPPORTED_OPCODE: Self = Self::new(5, 0x20, 0);
    pub(crate) const INVALID_CDB_FIELD: Self = Self::new(5, 0x24, 0);
    pub(crate) const LBA_OUT_OF_RANGE: Self = Self::new(5, 0x21, 0);
    pub(crate) const WRITE_PROTECTED: Self = Self::new(7, 0x27, 0);
    pub(crate) const HOST_IO_ERROR: Self = Self::new(4, 0x44, 0);
    pub(crate) const NOT_READY: Self = Self::new(2, 0x04, 0x02);

    const fn new(key: u8, asc: u8, ascq: u8) -> Self {
        Self { key, asc, ascq }
    }

    pub(crate) fn fixed_response(self, allocation_length: u8) -> Vec<u8> {
        let mut data = vec![0; FIXED_SENSE_BYTES];
        data[0] = 0x70;
        data[2] = self.key;
        data[7] = 10;
        data[12] = self.asc;
        data[13] = self.ascq;
        data.truncate(usize::from(allocation_length));
        data
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::sync::{Arc, Mutex};

    use crate::scsi_disk::ScsiDisk;
    use crate::storage::BlockStorage;

    use super::{
        ScsiAttachError, ScsiBus, ScsiBusError, ScsiCommandPlan, ScsiCommandStart,
        ScsiDataDirection, ScsiStatus, ScsiTarget, ScsiTransferResult,
    };

    struct TestTarget {
        storage_bytes: u64,
    }

    impl ScsiTarget for TestTarget {
        fn storage_size_bytes(&self) -> u64 {
            self.storage_bytes
        }

        fn execute(&mut self, cdb: &[u8]) -> ScsiCommandPlan {
            match cdb.first().copied() {
                Some(1) => ScsiCommandPlan::Complete {
                    status: ScsiStatus::Good,
                    data_in: vec![1, 2, 3, 4],
                },
                Some(2) => ScsiCommandPlan::ReadStorage {
                    offset: 1,
                    byte_count: 4,
                },
                Some(3) => ScsiCommandPlan::ReadStorage {
                    offset: self.storage_bytes,
                    byte_count: 1,
                },
                Some(4) => ScsiCommandPlan::WriteStorage {
                    offset: 2,
                    byte_count: 4,
                },
                Some(5) => ScsiCommandPlan::WriteStorage {
                    offset: self.storage_bytes,
                    byte_count: 1,
                },
                Some(6) => ScsiCommandPlan::WriteStorage {
                    offset: 0,
                    byte_count: 0,
                },
                Some(7) => ScsiCommandPlan::WriteStorage {
                    offset: u64::MAX,
                    byte_count: 2,
                },
                _ => ScsiCommandPlan::Complete {
                    status: ScsiStatus::Good,
                    data_in: Vec::new(),
                },
            }
        }

        fn complete_storage(&mut self, succeeded: bool) -> ScsiStatus {
            if succeeded {
                ScsiStatus::Good
            } else {
                ScsiStatus::CheckCondition
            }
        }
    }

    struct TestStorage {
        bytes: Arc<Mutex<Vec<u8>>>,
        fail_reads: bool,
        fail_writes: bool,
    }

    impl BlockStorage for TestStorage {
        fn size_bytes(&self) -> u64 {
            self.bytes.lock().unwrap().len() as u64
        }

        fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
            if self.fail_reads {
                return Err(io::Error::other("injected storage failure"));
            }
            let offset = usize::try_from(offset)
                .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "offset overflow"))?;
            let end = offset
                .checked_add(buffer.len())
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "range overflow"))?;
            let bytes = self.bytes.lock().unwrap();
            let source = bytes
                .get(offset..end)
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short storage"))?;
            buffer.copy_from_slice(source);
            Ok(())
        }

        fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
            if self.fail_writes {
                return Err(io::Error::other("injected storage failure"));
            }
            let offset = usize::try_from(offset)
                .map_err(|_| io::Error::new(io::ErrorKind::UnexpectedEof, "offset overflow"))?;
            let end = offset
                .checked_add(data.len())
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "range overflow"))?;
            let mut bytes = self.bytes.lock().unwrap();
            let destination = bytes
                .get_mut(offset..end)
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short storage"))?;
            destination.copy_from_slice(data);
            Ok(())
        }
    }

    fn attach_test_target(
        bus: &mut ScsiBus,
        fail_reads: bool,
        fail_writes: bool,
    ) -> Arc<Mutex<Vec<u8>>> {
        let bytes = Arc::new(Mutex::new((0..8).collect()));
        bus.attach(
            1,
            0,
            Box::new(TestTarget { storage_bytes: 8 }),
            Box::new(TestStorage {
                bytes: Arc::clone(&bytes),
                fail_reads,
                fail_writes,
            }),
        )
        .unwrap();
        bytes
    }

    #[test]
    fn attachment_validates_address_occupancy_and_capacity() {
        let mut bus = ScsiBus::new();
        let target = || Box::new(TestTarget { storage_bytes: 8 });
        let storage = |len| {
            Box::new(TestStorage {
                bytes: Arc::new(Mutex::new(vec![0; len])),
                fail_reads: false,
                fail_writes: false,
            }) as Box<dyn BlockStorage>
        };

        assert_eq!(
            bus.attach(8, 0, target(), storage(8)),
            Err(ScsiAttachError::InvalidAddress {
                target_id: 8,
                lun: 0,
            })
        );
        assert_eq!(
            bus.attach(1, 0, target(), storage(7)),
            Err(ScsiAttachError::StorageSizeMismatch {
                target_bytes: 8,
                storage_bytes: 7,
            })
        );
        bus.attach(1, 0, target(), storage(8)).unwrap();
        assert_eq!(
            bus.attach(1, 0, target(), storage(8)),
            Err(ScsiAttachError::AddressOccupied {
                target_id: 1,
                lun: 0,
            })
        );
    }

    #[test]
    fn absent_target_is_a_selection_timeout() {
        assert_eq!(
            ScsiBus::new().start_command(4, 0, &[0]),
            Ok(ScsiCommandStart::SelectionTimeout)
        );
    }

    #[test]
    fn rejected_immediate_data_does_not_advance() {
        let mut bus = ScsiBus::new();
        attach_test_target(&mut bus, false, false);
        assert_eq!(
            bus.start_command(1, 0, &[1]),
            Ok(ScsiCommandStart::DataIn { byte_count: 4 })
        );
        assert_eq!(
            bus.transfer_data_in(2, |bytes| {
                assert_eq!(bytes, [1, 2]);
                false
            }),
            Ok(ScsiTransferResult::Rejected)
        );
        assert_eq!(
            bus.transfer_data_in(3, |bytes| {
                assert_eq!(bytes, [1, 2, 3]);
                true
            }),
            Ok(ScsiTransferResult::More {
                transferred: 3,
                remaining: 1,
            })
        );
        assert_eq!(
            bus.transfer_data_in(3, |bytes| {
                assert_eq!(bytes, [4]);
                true
            }),
            Ok(ScsiTransferResult::Complete {
                transferred: 1,
                status: ScsiStatus::Good,
            })
        );
    }

    #[test]
    fn storage_data_is_chunked_and_completed_by_the_target() {
        let mut bus = ScsiBus::new();
        attach_test_target(&mut bus, false, false);
        assert_eq!(
            bus.start_command(1, 0, &[2]),
            Ok(ScsiCommandStart::DataIn { byte_count: 4 })
        );
        assert_eq!(bus.active_address(), Some((1, 0)));
        assert_eq!(
            bus.transfer_data_in(2, |bytes| {
                assert_eq!(bytes, [1, 2]);
                true
            }),
            Ok(ScsiTransferResult::More {
                transferred: 2,
                remaining: 2,
            })
        );
        assert_eq!(
            bus.transfer_data_in(8, |bytes| {
                assert_eq!(bytes, [3, 4]);
                true
            }),
            Ok(ScsiTransferResult::Complete {
                transferred: 2,
                status: ScsiStatus::Good,
            })
        );
        assert_eq!(bus.active_address(), None);
    }

    #[test]
    fn snapshot_restores_an_active_transfer_without_replacing_storage() {
        let bytes = Arc::new(Mutex::new(
            (0..512).map(|index| index as u8).collect::<Vec<_>>(),
        ));
        let mut original = ScsiBus::new();
        original
            .attach(
                1,
                0,
                Box::new(ScsiDisk::try_new(512).unwrap()),
                Box::new(TestStorage {
                    bytes: Arc::clone(&bytes),
                    fail_reads: false,
                    fail_writes: false,
                }),
            )
            .unwrap();
        assert_eq!(
            original.start_command(1, 0, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0]),
            Ok(ScsiCommandStart::DataIn { byte_count: 512 })
        );
        assert_eq!(
            original.transfer_data_in(100, |chunk| {
                assert_eq!(chunk, &bytes.lock().unwrap()[..100]);
                true
            }),
            Ok(ScsiTransferResult::More {
                transferred: 100,
                remaining: 412,
            })
        );
        let snapshot = original.snapshot().unwrap();

        let replacement_bytes = Arc::new(Mutex::new(
            (0..512)
                .map(|index| (index as u8).wrapping_add(1))
                .collect::<Vec<_>>(),
        ));
        let mut restored = ScsiBus::new();
        restored
            .attach(
                1,
                0,
                Box::new(ScsiDisk::try_new(512).unwrap()),
                Box::new(TestStorage {
                    bytes: Arc::clone(&replacement_bytes),
                    fail_reads: false,
                    fail_writes: false,
                }),
            )
            .unwrap();
        restored.restore_snapshot(snapshot).unwrap();

        assert_eq!(restored.active_address(), Some((1, 0)));
        assert_eq!(
            restored.transfer_data_in(512, |chunk| {
                assert_eq!(chunk, &replacement_bytes.lock().unwrap()[100..]);
                true
            }),
            Ok(ScsiTransferResult::Complete {
                transferred: 412,
                status: ScsiStatus::Good,
            })
        );
    }

    #[test]
    fn storage_failure_completes_without_calling_the_consumer() {
        let mut bus = ScsiBus::new();
        attach_test_target(&mut bus, true, false);
        bus.start_command(1, 0, &[2]).unwrap();
        assert_eq!(
            bus.transfer_data_in(4, |_| panic!(
                "consumer must not run after a storage failure"
            )),
            Ok(ScsiTransferResult::Complete {
                transferred: 0,
                status: ScsiStatus::CheckCondition,
            })
        );
        assert_eq!(bus.active_address(), None);
    }

    #[test]
    fn storage_data_out_uses_exact_chunks_and_advances_after_writes() {
        let mut bus = ScsiBus::new();
        let bytes = attach_test_target(&mut bus, false, false);
        assert_eq!(
            bus.start_command(1, 0, &[4]),
            Ok(ScsiCommandStart::DataOut { byte_count: 4 })
        );
        assert_eq!(bus.active_data_direction(), Some(ScsiDataDirection::Out));

        assert_eq!(
            bus.transfer_data_out(2, |buffer| {
                assert_eq!(buffer.len(), 2);
                buffer.copy_from_slice(&[9, 8]);
                true
            }),
            Ok(ScsiTransferResult::More {
                transferred: 2,
                remaining: 2,
            })
        );
        assert_eq!(
            bus.transfer_data_out(8, |buffer| {
                assert_eq!(buffer.len(), 2);
                buffer.copy_from_slice(&[7, 6]);
                true
            }),
            Ok(ScsiTransferResult::Complete {
                transferred: 2,
                status: ScsiStatus::Good,
            })
        );
        assert_eq!(*bytes.lock().unwrap(), [0, 1, 9, 8, 7, 6, 6, 7]);
        assert_eq!(bus.active_data_direction(), None);
    }

    #[test]
    fn rejected_data_out_does_not_write_or_advance() {
        let mut bus = ScsiBus::new();
        let bytes = attach_test_target(&mut bus, false, false);
        bus.start_command(1, 0, &[4]).unwrap();

        assert_eq!(
            bus.transfer_data_out(3, |buffer| {
                buffer.fill(0xff);
                false
            }),
            Ok(ScsiTransferResult::Rejected)
        );
        assert_eq!(*bytes.lock().unwrap(), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(bus.active_address(), Some((1, 0)));

        assert_eq!(
            bus.transfer_data_out(4, |buffer| {
                buffer.copy_from_slice(&[4, 3, 2, 1]);
                true
            }),
            Ok(ScsiTransferResult::Complete {
                transferred: 4,
                status: ScsiStatus::Good,
            })
        );
        assert_eq!(*bytes.lock().unwrap(), [0, 1, 4, 3, 2, 1, 6, 7]);
    }

    #[test]
    fn data_out_storage_failure_reports_zero_and_clears_the_transaction() {
        let mut bus = ScsiBus::new();
        let bytes = attach_test_target(&mut bus, false, true);
        bus.start_command(1, 0, &[4]).unwrap();

        assert_eq!(
            bus.transfer_data_out(4, |buffer| {
                buffer.fill(0xff);
                true
            }),
            Ok(ScsiTransferResult::Complete {
                transferred: 0,
                status: ScsiStatus::CheckCondition,
            })
        );
        assert_eq!(*bytes.lock().unwrap(), [0, 1, 2, 3, 4, 5, 6, 7]);
        assert_eq!(bus.active_address(), None);
    }

    #[test]
    fn invalid_storage_range_becomes_target_check_condition() {
        let mut bus = ScsiBus::new();
        attach_test_target(&mut bus, false, false);
        for cdb in [3, 5, 7] {
            assert_eq!(
                bus.start_command(1, 0, &[cdb]),
                Ok(ScsiCommandStart::Complete {
                    status: ScsiStatus::CheckCondition,
                })
            );
        }
        assert_eq!(
            bus.start_command(1, 0, &[6]),
            Ok(ScsiCommandStart::Complete {
                status: ScsiStatus::Good,
            })
        );
    }

    #[test]
    fn active_transaction_blocks_new_commands_and_can_be_cancelled() {
        let mut bus = ScsiBus::new();
        attach_test_target(&mut bus, false, false);
        bus.start_command(1, 0, &[1]).unwrap();
        assert_eq!(
            bus.transfer_data_in(0, |_| true),
            Err(ScsiBusError::EmptyDataBuffer)
        );
        assert_eq!(
            bus.start_command(2, 0, &[0]),
            Err(ScsiBusError::TransactionActive {
                target_id: 1,
                lun: 0,
            })
        );
        bus.cancel_transaction();
        assert_eq!(bus.active_address(), None);
        assert_eq!(
            bus.transfer_data_in(1, |_| true),
            Err(ScsiBusError::NoDataInTransaction)
        );
    }

    #[test]
    fn transfer_direction_is_enforced_and_cancel_preserves_committed_chunks() {
        let mut bus = ScsiBus::new();
        let bytes = attach_test_target(&mut bus, false, false);
        bus.start_command(1, 0, &[4]).unwrap();
        assert_eq!(
            bus.transfer_data_in(1, |_| true),
            Err(ScsiBusError::NoDataInTransaction)
        );
        assert_eq!(
            bus.transfer_data_out(2, |buffer| {
                buffer.copy_from_slice(&[0xaa, 0xbb]);
                true
            }),
            Ok(ScsiTransferResult::More {
                transferred: 2,
                remaining: 2,
            })
        );
        bus.cancel_transaction();
        assert_eq!(*bytes.lock().unwrap(), [0, 1, 0xaa, 0xbb, 4, 5, 6, 7]);
        assert_eq!(
            bus.transfer_data_out(1, |_| true),
            Err(ScsiBusError::NoDataOutTransaction)
        );

        bus.start_command(1, 0, &[2]).unwrap();
        assert_eq!(bus.active_data_direction(), Some(ScsiDataDirection::In));
        assert_eq!(
            bus.transfer_data_out(1, |_| true),
            Err(ScsiBusError::NoDataOutTransaction)
        );
    }
}
