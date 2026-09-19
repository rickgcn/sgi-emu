//! Runtime-facing dispatch across supported machine models.

use std::error::Error;
use std::fmt;

use se_core::storage::StorageMedium;
use se_core::time::VirtualDuration;
use se_cpu::mips1::r3000::StepError;
use serde::{Deserialize, Serialize};

use crate::debug::{DebugRequest, DebugResponse};
use crate::endpoint::{EndpointCatalog, EndpointKind};
use crate::indigo::ip12::snapshot::Ip12Snapshot;
use crate::indigo::ip12::{Ip12, Ip12NonvolatileState, Ip12SnapshotError};
use crate::input::MachineInput;
use crate::media::{MachineMediaError, MediaCatalog, MediaSlotKey};
use crate::output::MachineOutput;

/// A configured emulated machine.
pub enum Machine {
    /// An SGI Indigo IP12.
    IndigoIp12(Ip12),
}

/// State retained while a configured machine is powered off.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MachineNonvolatileState {
    /// Battery-backed and nonvolatile state of an SGI Indigo IP12.
    IndigoIp12(Ip12NonvolatileState),
}

/// Complete restorable execution state of one configured machine.
///
/// The value intentionally excludes construction-time configuration and host
/// storage objects. It can only be restored into a matching cold-constructed
/// machine.
#[derive(Clone, Deserialize, Serialize)]
pub struct MachineSnapshot {
    state: MachineSnapshotState,
}

#[derive(Clone, Deserialize, Serialize)]
enum MachineSnapshotState {
    IndigoIp12(Ip12Snapshot),
}

/// A machine snapshot that cannot be captured or restored.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineSnapshotError {
    /// Snapshot and cold-constructed machine models differ.
    IncompatibleMachineModel,
    /// The Indigo IP12 snapshot cannot preserve its configured topology.
    IndigoIp12(Ip12SnapshotError),
}

impl fmt::Display for MachineSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompatibleMachineModel => {
                formatter.write_str("machine snapshot model does not match the configured machine")
            }
            Self::IndigoIp12(error) => error.fmt(formatter),
        }
    }
}

impl Error for MachineSnapshotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IncompatibleMachineModel => None,
            Self::IndigoIp12(error) => Some(error),
        }
    }
}

impl From<Ip12SnapshotError> for MachineSnapshotError {
    fn from(error: Ip12SnapshotError) -> Self {
        Self::IndigoIp12(error)
    }
}

/// An error encountered while executing one machine instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    /// An Indigo IP12 processor step failed.
    IndigoIp12(StepError),
}

/// Result of one valid machine input at the current execution boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineInputResult {
    /// The machine accepted the complete input.
    Consumed,
    /// The destination cannot accept the input at this boundary.
    WouldBlock,
}

/// An invalid endpoint or payload in a machine input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MachineInputError {
    /// The endpoint is not present in this machine topology.
    UnknownEndpoint,
    /// The endpoint does not accept host input.
    OutputOnlyEndpoint,
    /// The payload type does not match the endpoint kind.
    PayloadKindMismatch,
    /// The semantic keyboard key is outside the supported frontend set.
    UnsupportedKeyboardKey,
}

impl fmt::Display for MachineInputError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownEndpoint => formatter.write_str("unknown machine input endpoint"),
            Self::OutputOnlyEndpoint => {
                formatter.write_str("machine endpoint does not accept input")
            }
            Self::PayloadKindMismatch => {
                formatter.write_str("machine input payload does not match endpoint kind")
            }
            Self::UnsupportedKeyboardKey => {
                formatter.write_str("unsupported frontend keyboard key")
            }
        }
    }
}

impl Error for MachineInputError {}

impl fmt::Display for ExecutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IndigoIp12(error) => error.fmt(formatter),
        }
    }
}

impl Error for ExecutionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IndigoIp12(error) => Some(error),
        }
    }
}

impl Machine {
    /// Returns the configured machine's frontend I/O capabilities.
    #[must_use]
    pub fn endpoint_catalog(&self) -> EndpointCatalog {
        match self {
            Self::IndigoIp12(machine) => machine.endpoint_catalog(),
        }
    }

    /// Samples the configured machine's removable-media slots.
    ///
    /// Slot identity is opaque to callers: it may be built from a machine's
    /// own addressing information, but callers compare it for equality instead
    /// of parsing or depending on that representation. The sample carries no
    /// host resource or host path information, and it names no device model.
    #[must_use]
    pub fn media_catalog(&self) -> MediaCatalog {
        match self {
            Self::IndigoIp12(machine) => machine.media_catalog(),
        }
    }

    /// Installs one prepared medium in the addressed removable slot.
    ///
    /// # Errors
    ///
    /// Returns [`MachineMediaError`] when the active machine has no such slot
    /// or the slot cannot accept the medium at this boundary.
    pub fn insert_media(
        &mut self,
        slot: &MediaSlotKey,
        medium: Box<dyn StorageMedium>,
    ) -> Result<(), MachineMediaError> {
        match self {
            Self::IndigoIp12(machine) => machine.insert_media(slot, medium),
        }
    }

    /// Removes the medium from the addressed removable slot and releases it.
    ///
    /// # Errors
    ///
    /// Returns [`MachineMediaError`] when the active machine has no such slot
    /// or the slot cannot release its medium at this boundary.
    pub fn eject_media(
        &mut self,
        slot: &MediaSlotKey,
        force: bool,
    ) -> Result<(), MachineMediaError> {
        match self {
            Self::IndigoIp12(machine) => machine.eject_media(slot, force),
        }
    }

    /// Captures complete execution state without construction-time resources.
    ///
    /// # Errors
    ///
    /// Returns [`MachineSnapshotError`] when an attached device cannot expose
    /// restorable state.
    pub fn snapshot(&self) -> Result<MachineSnapshot, MachineSnapshotError> {
        let state = match self {
            Self::IndigoIp12(machine) => MachineSnapshotState::IndigoIp12(machine.snapshot()?),
        };
        Ok(MachineSnapshot { state })
    }

    /// Restores execution state into a matching cold-constructed machine.
    ///
    /// # Errors
    ///
    /// Returns [`MachineSnapshotError`] when the machine model or attached
    /// storage topology differs.
    pub fn restore_snapshot(
        &mut self,
        snapshot: MachineSnapshot,
    ) -> Result<(), MachineSnapshotError> {
        match (self, snapshot.state) {
            (Self::IndigoIp12(machine), MachineSnapshotState::IndigoIp12(snapshot)) => {
                machine.restore_snapshot(snapshot)?;
            }
        }
        Ok(())
    }

    /// Returns state that survives machine reconstruction and application
    /// sessions.
    #[must_use]
    pub fn nonvolatile_state(&self) -> MachineNonvolatileState {
        match self {
            Self::IndigoIp12(machine) => {
                MachineNonvolatileState::IndigoIp12(machine.nonvolatile_state())
            }
        }
    }

    /// Restores retained state and advances battery-backed clocks by elapsed
    /// offline milliseconds.
    pub fn restore_nonvolatile_state(
        &mut self,
        state: MachineNonvolatileState,
        offline_milliseconds: u64,
    ) {
        match (self, state) {
            (Self::IndigoIp12(machine), MachineNonvolatileState::IndigoIp12(state)) => {
                machine.restore_nonvolatile_state(state, offline_milliseconds);
            }
        }
    }

    /// Returns the configured processor clock frequency in hertz.
    #[must_use]
    pub const fn cpu_frequency_hz(&self) -> u64 {
        match self {
            Self::IndigoIp12(machine) => machine.cpu_frequency_hz(),
        }
    }

    /// Restores the selected machine's reset state.
    pub fn reset(&mut self) {
        match self {
            Self::IndigoIp12(machine) => machine.reset(),
        }
    }

    /// Executes one architectural processor instruction.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutionError`] when the selected machine cannot complete
    /// the instruction.
    pub fn execute_instruction(&mut self) -> Result<(), ExecutionError> {
        match self {
            Self::IndigoIp12(machine) => machine
                .execute_instruction()
                .map_err(ExecutionError::IndigoIp12),
        }
    }

    /// Advances timed devices and appends frontend-visible output.
    pub fn advance_time(&mut self, elapsed: VirtualDuration, output: &mut MachineOutput) {
        match self {
            Self::IndigoIp12(machine) => machine.advance_time(elapsed, output),
        }
    }

    /// Publishes current-state outputs without advancing virtual time.
    pub fn publish_current_outputs(&self, output: &mut MachineOutput) {
        match self {
            Self::IndigoIp12(machine) => machine.publish_current_outputs(output),
        }
    }

    /// Attempts to accept one frontend-neutral input at the current machine
    /// boundary.
    ///
    /// Keyboard and mouse inputs are accepted even when they describe a
    /// duplicate state or zero motion. Serial character arrivals are handled
    /// immediately, while Ethernet inputs report link readiness.
    pub fn try_receive_input(
        &mut self,
        input: &MachineInput,
    ) -> Result<MachineInputResult, MachineInputError> {
        let catalog = self.endpoint_catalog();
        let descriptor = catalog
            .get(input.endpoint())
            .ok_or(MachineInputError::UnknownEndpoint)?;
        if !descriptor.direction().accepts_input() {
            return Err(MachineInputError::OutputOnlyEndpoint);
        }
        if descriptor.kind() != input.payload().kind() {
            return Err(MachineInputError::PayloadKindMismatch);
        }
        debug_assert_ne!(descriptor.kind(), EndpointKind::Video);
        match self {
            Self::IndigoIp12(machine) => machine.try_receive_input(input),
        }
    }

    /// Returns the virtual address of the next instruction to execute.
    #[must_use]
    pub fn execution_address(&self) -> u32 {
        match self {
            Self::IndigoIp12(machine) => machine.execution_address(),
        }
    }

    /// Performs one side-effect-free debugger query.
    #[must_use]
    pub fn debug(&self, request: DebugRequest) -> DebugResponse {
        match (self, request) {
            (Self::IndigoIp12(machine), DebugRequest::MachineStateFingerprint) => {
                DebugResponse::MachineStateFingerprint(machine.machine_state_fingerprint())
            }
            (Self::IndigoIp12(machine), DebugRequest::IndigoIp12(request)) => {
                DebugResponse::IndigoIp12(machine.debug(request))
            }
        }
    }
}
