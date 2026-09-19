//! Generation-safe runtime control of live machine media slots.

use std::error::Error;
use std::fmt;

use se_machine::media::{MediaKind, MediaSlotKey, MediaSlotState};

/// A media slot identity bound to one installed machine instance.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeMediaHandle {
    generation: u64,
    key: MediaSlotKey,
}

impl RuntimeMediaHandle {
    pub(crate) const fn new(generation: u64, key: MediaSlotKey) -> Self {
        Self { generation, key }
    }

    /// Returns the installed machine generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns the opaque machine media-slot identity.
    #[must_use]
    pub const fn key(&self) -> &MediaSlotKey {
        &self.key
    }
}

/// One media slot visible to a frontend at a coherent runtime boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeMediaSlotDescriptor {
    handle: RuntimeMediaHandle,
    label: String,
    kind: MediaKind,
    state: MediaSlotState,
}

impl RuntimeMediaSlotDescriptor {
    pub(crate) fn new(
        handle: RuntimeMediaHandle,
        label: &str,
        kind: MediaKind,
        state: MediaSlotState,
    ) -> Self {
        Self {
            handle,
            label: label.into(),
            kind,
            state,
        }
    }

    /// Returns the live media-slot handle.
    #[must_use]
    pub const fn handle(&self) -> &RuntimeMediaHandle {
        &self.handle
    }

    /// Returns the user-visible slot label.
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

/// Atomically sampled media slots of the active machine generation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeMediaCatalog {
    generation: u64,
    slots: Vec<RuntimeMediaSlotDescriptor>,
}

impl RuntimeMediaCatalog {
    pub(crate) const fn new(generation: u64, slots: Vec<RuntimeMediaSlotDescriptor>) -> Self {
        Self { generation, slots }
    }

    /// Returns the active machine generation.
    #[must_use]
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns slots in stable machine-defined order.
    #[must_use]
    pub fn slots(&self) -> &[RuntimeMediaSlotDescriptor] {
        &self.slots
    }
}

/// Typed frontend outcome of one runtime media command.
///
/// The variants mirror the machine-visible outcomes without naming a device
/// model, a SCSI address, or a host resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeMediaError {
    /// The handle refers to a previous machine instance.
    StaleHandle,
    /// The active machine does not contain the referenced slot.
    UnknownSlot,
    /// The current runtime mode does not accept host media changes.
    MutationUnavailable,
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

impl fmt::Display for RuntimeMediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StaleHandle => formatter.write_str("stale runtime media handle"),
            Self::UnknownSlot => formatter.write_str("unknown runtime media slot"),
            Self::MutationUnavailable => {
                formatter.write_str("host media changes are unavailable in the current mode")
            }
            Self::MediumAlreadyPresent => {
                formatter.write_str("runtime media slot already holds a medium")
            }
            Self::MediumNotPresent => formatter.write_str("runtime media slot holds no medium"),
            Self::MediumRemovalPrevented => {
                formatter.write_str("runtime media slot removal is prevented")
            }
            Self::InvalidMedium => formatter.write_str("invalid medium for runtime media slot"),
            Self::Busy => formatter.write_str("runtime media slot is busy"),
        }
    }
}

impl Error for RuntimeMediaError {}
