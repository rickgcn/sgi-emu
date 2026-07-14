//! Byte-programmable system flash for ISA-style buses.

use core::fmt;

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;

use crate::bus::isa::{
    IsaBusError, IsaCompletion, IsaCompletionPayload, IsaDeviceResponse, IsaTransaction,
    IsaTransferView,
};
use crate::state::DeviceStateError;

/// One contiguous range whose current contents differ from the base image.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SystemFlashChange {
    offset: u64,
    bytes: Vec<u8>,
}

impl SystemFlashChange {
    /// Returns the byte offset of the first changed byte.
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    /// Returns the replacement bytes in ascending address order.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// Application-persistent flash contents relative to an immutable base image.
///
/// This state describes the flash device only. Battery-backed RTC/NVRAM state
/// belongs to its physical RTC device and must be persisted separately.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SystemFlashPersistentState {
    image_size: u64,
    revision: u64,
    changes: Vec<SystemFlashChange>,
}

impl SystemFlashPersistentState {
    /// Returns the required base-image size.
    pub const fn image_size(&self) -> u64 {
        self.image_size
    }

    /// Returns the revision of guest-visible flash contents.
    pub const fn revision(&self) -> u64 {
        self.revision
    }

    /// Returns the canonical changed ranges.
    pub fn changes(&self) -> &[SystemFlashChange] {
        &self.changes
    }
}

/// Exact flash component state stored in an IP32 machine state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct SystemFlashState {
    id: ComponentId,
    persistent: SystemFlashPersistentState,
}

/// Invalid serialized system-flash state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SystemFlashStateError {
    /// State belongs to another fixed-topology component.
    Device(DeviceStateError),
    /// The state was created for a differently sized base image.
    ImageSizeMismatch { expected: u64, actual: u64 },
    /// A changed range contains no bytes.
    EmptyChange { index: usize },
    /// Changed ranges are not strictly ordered and disjoint.
    UnorderedOrOverlappingChange { index: usize },
    /// A changed range extends beyond the base image.
    ChangeOutOfRange { index: usize },
    /// A changed range redundantly stores an unmodified base byte.
    NonCanonicalChange { index: usize },
}

impl fmt::Display for SystemFlashStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Device(error) => error.fmt(formatter),
            Self::ImageSizeMismatch { expected, actual } => write!(
                formatter,
                "system flash state image size mismatch: expected {expected}, got {actual}"
            ),
            Self::EmptyChange { index } => {
                write!(formatter, "system flash change {index} is empty")
            }
            Self::UnorderedOrOverlappingChange { index } => write!(
                formatter,
                "system flash change {index} is unordered or overlaps a previous change"
            ),
            Self::ChangeOutOfRange { index } => {
                write!(
                    formatter,
                    "system flash change {index} is outside the image"
                )
            }
            Self::NonCanonicalChange { index } => write!(
                formatter,
                "system flash change {index} contains an unchanged base byte"
            ),
        }
    }
}

impl std::error::Error for SystemFlashStateError {}

/// System flash with byte programming and an immutable base image.
///
/// The base image is never modified. Guest programming changes only the
/// current readable image and can be persisted as a canonical overlay.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SystemFlash {
    id: ComponentId,
    name: String,
    base_image: Vec<u8>,
    bytes: Vec<u8>,
    persistence_revision: u64,
}

impl SystemFlash {
    /// Creates flash initialized from one immutable base image.
    pub fn new(id: ComponentId, name: impl Into<String>, base_image: Vec<u8>) -> Self {
        Self {
            id,
            name: name.into(),
            bytes: base_image.clone(),
            base_image,
            persistence_revision: 0,
        }
    }

    /// Returns the physical flash capacity.
    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    /// Returns whether the flash contains no bytes.
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    /// Returns the current readable flash contents.
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Returns the revision of guest-visible flash contents.
    pub const fn persistence_revision(&self) -> u64 {
        self.persistence_revision
    }

    /// Captures exact component state without copying unchanged base-image bytes.
    pub fn save_state(&self) -> SystemFlashState {
        SystemFlashState {
            id: self.id,
            persistent: self.persistent_state(),
        }
    }

    /// Restores exact component state after validating topology identity and ranges.
    pub fn restore_state(&mut self, state: SystemFlashState) -> Result<(), SystemFlashStateError> {
        if state.id != self.id {
            return Err(SystemFlashStateError::Device(
                DeviceStateError::ComponentIdMismatch {
                    expected: self.id,
                    actual: state.id,
                },
            ));
        }
        self.restore_persistent_state(&state.persistent)
    }

    /// Captures application-persistent contents relative to the base image.
    pub fn persistent_state(&self) -> SystemFlashPersistentState {
        let mut changes = Vec::new();
        let mut offset = 0;
        while offset < self.bytes.len() {
            if self.bytes[offset] == self.base_image[offset] {
                offset += 1;
                continue;
            }
            let start = offset;
            while offset < self.bytes.len() && self.bytes[offset] != self.base_image[offset] {
                offset += 1;
            }
            changes.push(SystemFlashChange {
                offset: start as u64,
                bytes: self.bytes[start..offset].to_vec(),
            });
        }
        SystemFlashPersistentState {
            image_size: self.base_image.len() as u64,
            revision: self.persistence_revision,
            changes,
        }
    }

    /// Applies application-persistent contents atomically to the base image.
    pub fn restore_persistent_state(
        &mut self,
        state: &SystemFlashPersistentState,
    ) -> Result<(), SystemFlashStateError> {
        let expected = self.base_image.len() as u64;
        if state.image_size != expected {
            return Err(SystemFlashStateError::ImageSizeMismatch {
                expected,
                actual: state.image_size,
            });
        }
        let mut restored = self.base_image.clone();
        let mut previous_end = 0_usize;
        for (index, change) in state.changes.iter().enumerate() {
            if change.bytes.is_empty() {
                return Err(SystemFlashStateError::EmptyChange { index });
            }
            let Ok(start) = usize::try_from(change.offset) else {
                return Err(SystemFlashStateError::ChangeOutOfRange { index });
            };
            if index != 0 && start < previous_end {
                return Err(SystemFlashStateError::UnorderedOrOverlappingChange { index });
            }
            let Some(end) = start.checked_add(change.bytes.len()) else {
                return Err(SystemFlashStateError::ChangeOutOfRange { index });
            };
            if end > restored.len() {
                return Err(SystemFlashStateError::ChangeOutOfRange { index });
            }
            if change
                .bytes
                .iter()
                .zip(&self.base_image[start..end])
                .any(|(current, base)| current == base)
            {
                return Err(SystemFlashStateError::NonCanonicalChange { index });
            }
            restored[start..end].copy_from_slice(&change.bytes);
            previous_end = end;
        }
        self.bytes = restored;
        self.persistence_revision = state.revision;
        Ok(())
    }
}

impl Component for SystemFlash {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {}
}

impl BusDeviceRole<IsaTransaction> for SystemFlash {
    type Response = IsaDeviceResponse;

    fn accept(&mut self, transaction: IsaTransaction) -> Self::Response {
        let result = match transaction.transfer.view() {
            IsaTransferView::Read { length } if (1..=8).contains(&length) => {
                let start = transaction.address as usize;
                let length = usize::from(length);
                if start
                    .checked_add(length)
                    .is_some_and(|end| end <= self.bytes.len())
                {
                    Ok(IsaCompletionPayload::ReadData(
                        self.bytes[start..start + length].iter().copied().collect(),
                    ))
                } else {
                    Err(IsaBusError::Address)
                }
            }
            IsaTransferView::Read { .. } => Err(IsaBusError::Access),
            IsaTransferView::Write { data, byte_enable }
                if data.len() == 1 && byte_enable.len() == 1 =>
            {
                let Some(enabled) = byte_enable.is_enabled(0) else {
                    unreachable!("the validated byte-enable view contains one lane")
                };
                let address = transaction.address as usize;
                if address >= self.bytes.len() {
                    Err(IsaBusError::Address)
                } else {
                    if enabled && self.bytes[address] != data[0] {
                        self.bytes[address] = data[0];
                        self.persistence_revision = self.persistence_revision.wrapping_add(1);
                    }
                    Ok(IsaCompletionPayload::WriteComplete)
                }
            }
            IsaTransferView::Write { .. } => Err(IsaBusError::Access),
        };
        IsaDeviceResponse::Complete(IsaCompletion {
            id: transaction.id,
            result,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::isa::{IsaByteEnable, IsaData, IsaTransactionId, IsaTransfer};
    use se_core::scheduler::SimTime;

    fn transaction(address: u32, transfer: IsaTransfer) -> IsaTransaction {
        IsaTransaction {
            id: IsaTransactionId::new(1),
            time: SimTime::ZERO,
            controller: ComponentId::new(2),
            target: ComponentId::new(1),
            address,
            transfer,
        }
    }

    fn result(response: IsaDeviceResponse) -> Result<IsaCompletionPayload, IsaBusError> {
        let IsaDeviceResponse::Complete(completion) = response else {
            panic!("system flash deferred a synchronous access")
        };
        completion.result
    }

    #[test]
    fn reads_and_programs_one_enabled_byte() {
        let mut flash = SystemFlash::new(ComponentId::new(1), "flash", vec![1, 2, 3, 4]);
        assert_eq!(
            result(flash.accept(transaction(1, IsaTransfer::read(2)))),
            Ok(IsaCompletionPayload::ReadData([2, 3].into()))
        );
        assert_eq!(
            result(flash.accept(transaction(
                2,
                IsaTransfer::write([0x5a].into(), [true].into()),
            ))),
            Ok(IsaCompletionPayload::WriteComplete)
        );
        assert_eq!(flash.bytes(), &[1, 2, 0x5a, 4]);
        assert_eq!(flash.persistence_revision(), 1);

        let _ = flash.accept(transaction(
            2,
            IsaTransfer::write([0x5a].into(), [true].into()),
        ));
        let _ = flash.accept(transaction(
            3,
            IsaTransfer::write([0xaa].into(), [false].into()),
        ));
        assert_eq!(flash.bytes(), &[1, 2, 0x5a, 4]);
        assert_eq!(flash.persistence_revision(), 1);
    }

    #[test]
    fn rejects_invalid_access_shapes_and_ranges() {
        let mut flash = SystemFlash::new(ComponentId::new(1), "flash", vec![0; 8]);
        assert_eq!(
            result(flash.accept(transaction(0, IsaTransfer::read(9)))),
            Err(IsaBusError::Access)
        );
        assert_eq!(
            result(flash.accept(transaction(
                0,
                IsaTransfer::write(IsaData::from([1, 2]), IsaByteEnable::from([true, true])),
            ))),
            Err(IsaBusError::Access)
        );
        assert_eq!(
            result(flash.accept(transaction(
                0,
                IsaTransfer::write(IsaData::from([1]), IsaByteEnable::from([true, false])),
            ))),
            Err(IsaBusError::Access)
        );
        assert_eq!(
            result(flash.accept(transaction(
                8,
                IsaTransfer::write([1].into(), [true].into()),
            ))),
            Err(IsaBusError::Address)
        );
    }

    #[test]
    fn reset_and_state_round_trip_preserve_programmed_bytes() {
        let mut flash = SystemFlash::new(ComponentId::new(1), "flash", vec![0xff; 8]);
        let _ = flash.accept(transaction(
            1,
            IsaTransfer::write([0x11].into(), [true].into()),
        ));
        let _ = flash.accept(transaction(
            2,
            IsaTransfer::write([0x22].into(), [true].into()),
        ));
        flash.reset();
        assert_eq!(&flash.bytes()[1..3], &[0x11, 0x22]);

        let state = flash.save_state();
        let persistent = flash.persistent_state();
        assert_eq!(persistent.revision(), 2);
        assert_eq!(persistent.changes().len(), 1);
        assert_eq!(persistent.changes()[0].offset(), 1);
        assert_eq!(persistent.changes()[0].bytes(), &[0x11, 0x22]);

        let mut restored = SystemFlash::new(ComponentId::new(1), "flash", vec![0xff; 8]);
        restored.restore_state(state).unwrap();
        assert_eq!(restored.bytes(), flash.bytes());
        assert_eq!(restored.persistence_revision(), 2);
    }

    #[test]
    fn invalid_persistent_state_does_not_partially_modify_flash() {
        let mut flash = SystemFlash::new(ComponentId::new(1), "flash", vec![0xff; 4]);
        let invalid = SystemFlashPersistentState {
            image_size: 4,
            revision: 9,
            changes: vec![
                SystemFlashChange {
                    offset: 0,
                    bytes: vec![0x11],
                },
                SystemFlashChange {
                    offset: 4,
                    bytes: vec![0x22],
                },
            ],
        };
        assert!(matches!(
            flash.restore_persistent_state(&invalid),
            Err(SystemFlashStateError::ChangeOutOfRange { index: 1 })
        ));
        assert_eq!(flash.bytes(), &[0xff; 4]);
        assert_eq!(flash.persistence_revision(), 0);
    }
}
