//! Stable identities and live state of a configured machine's removable media.
//!
//! A media slot is the boundary between a drive that belongs to the machine
//! topology and a medium that belongs to the host. The machine routes slot
//! identity and reports state; the device model behind it remains the only
//! source of truth for what the guest currently observes.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

/// Stable identity of one removable-media slot in a machine topology.
///
/// The identity is opaque: it carries no SCSI address, no host path, and no
/// index into any collection that can be reordered.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MediaSlotKey(String);

impl MediaSlotKey {
    pub(crate) fn new(value: &str) -> Self {
        Self(value.into())
    }

    /// Returns the opaque identity for transport and display equality checks.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Frontend-visible medium family of a removable slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKind {
    /// A read-only optical disc.
    OpticalDisc,
}

/// Current guest-visible state of one removable slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MediaSlotState {
    medium_size_bytes: Option<u64>,
    removal_prevented: bool,
}

impl MediaSlotState {
    pub(crate) const fn new(medium_size_bytes: Option<u64>, removal_prevented: bool) -> Self {
        Self {
            medium_size_bytes,
            removal_prevented,
        }
    }

    /// Reports whether the slot holds a medium.
    #[must_use]
    pub const fn medium_present(&self) -> bool {
        self.medium_size_bytes.is_some()
    }

    /// Returns the capacity of the installed medium, or `None` when the slot
    /// holds no medium.
    #[must_use]
    pub const fn medium_size_bytes(&self) -> Option<u64> {
        self.medium_size_bytes
    }

    /// Reports whether the guest locked the slot against removal.
    #[must_use]
    pub const fn removal_prevented(&self) -> bool {
        self.removal_prevented
    }
}

/// One removable slot of a configured machine.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaSlotDescriptor {
    key: MediaSlotKey,
    label: String,
    kind: MediaKind,
    state: MediaSlotState,
}

impl MediaSlotDescriptor {
    pub(crate) fn new(
        key: MediaSlotKey,
        label: &str,
        kind: MediaKind,
        state: MediaSlotState,
    ) -> Self {
        Self {
            key,
            label: label.into(),
            kind,
            state,
        }
    }

    /// Returns the stable machine slot identity.
    #[must_use]
    pub const fn key(&self) -> &MediaSlotKey {
        &self.key
    }

    /// Returns the user-visible slot name.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Returns the medium family the slot accepts.
    #[must_use]
    pub const fn kind(&self) -> MediaKind {
        self.kind
    }

    /// Returns the state sampled with this descriptor.
    #[must_use]
    pub const fn state(&self) -> MediaSlotState {
        self.state
    }
}

/// Duplicate media-slot identity in one machine catalog.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicateMediaSlotKey(MediaSlotKey);

impl fmt::Display for DuplicateMediaSlotKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "duplicate machine media slot key: {}",
            self.0.as_str()
        )
    }
}

impl Error for DuplicateMediaSlotKey {}

/// Live removable-media slots of one machine topology.
///
/// The catalog holds the state observed when it was sampled. Callers re-sample
/// it instead of caching presence or lock state, because the guest can change
/// both with its own commands.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MediaCatalog {
    slots: Vec<MediaSlotDescriptor>,
}

impl MediaCatalog {
    /// Validates unique keys while retaining machine-defined slot order.
    ///
    /// # Errors
    ///
    /// Returns [`DuplicateMediaSlotKey`] if one key occurs more than once.
    pub fn try_new(slots: Vec<MediaSlotDescriptor>) -> Result<Self, DuplicateMediaSlotKey> {
        let mut seen = BTreeSet::new();
        for slot in &slots {
            if !seen.insert(slot.key.clone()) {
                return Err(DuplicateMediaSlotKey(slot.key.clone()));
            }
        }
        Ok(Self { slots })
    }

    /// Returns descriptors in stable machine-defined order.
    #[must_use]
    pub fn slots(&self) -> &[MediaSlotDescriptor] {
        &self.slots
    }

    /// Finds one slot by exact opaque identity.
    #[must_use]
    pub fn get(&self, key: &MediaSlotKey) -> Option<&MediaSlotDescriptor> {
        self.slots.iter().find(|slot| slot.key() == key)
    }
}

/// A caller error encountered while changing machine media.
///
/// The machine reports slot-level outcomes so that no caller needs to know
/// which device model implements the slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineMediaError {
    /// The active machine has no removable slot with this identity.
    UnknownSlot,
    /// The addressed slot already holds a medium.
    MediumAlreadyPresent,
    /// The addressed slot holds no medium.
    MediumNotPresent,
    /// The guest prevented removal of the installed medium.
    MediumRemovalPrevented,
    /// The supplied medium does not satisfy the slot's capacity contract.
    InvalidMedium,
    /// The slot cannot change media while a bus connection or transaction is
    /// in progress.
    Busy,
}

impl fmt::Display for MachineMediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownSlot => formatter.write_str("unknown machine media slot"),
            Self::MediumAlreadyPresent => {
                formatter.write_str("machine media slot already holds a medium")
            }
            Self::MediumNotPresent => formatter.write_str("machine media slot holds no medium"),
            Self::MediumRemovalPrevented => {
                formatter.write_str("machine media slot removal is prevented")
            }
            Self::InvalidMedium => formatter.write_str("invalid medium for machine media slot"),
            Self::Busy => formatter.write_str("machine media slot is busy"),
        }
    }
}

impl Error for MachineMediaError {}
