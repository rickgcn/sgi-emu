//! Runtime-facing dispatch across supported machine models.

use std::error::Error;
use std::fmt;

use se_core::time::VirtualDuration;
use se_cpu::mips1::r3000::StepError;
use se_device::scsi::ScsiSnapshotError;
use se_float::backend::Backend;
use serde::{Deserialize, Serialize};

use crate::debug::{DebugRequest, DebugResponse};
use crate::indigo::ip12::snapshot::Ip12Snapshot;
use crate::indigo::ip12::{Ip12, Ip12MemoryConfiguration, Ip12NonvolatileState};
use crate::output::MachineOutput;
use crate::serial::SerialPort;

/// Construction-time configuration for a supported machine model.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum MachineStartupConfiguration {
    /// Configuration for an SGI Indigo IP12.
    IndigoIp12 {
        /// Floating-point implementation selected for the R3010.
        #[serde(with = "BackendDefinition")]
        floating_point_backend: Backend,
        /// Installed IP12 memory banks.
        memory: Ip12MemoryConfiguration,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(remote = "Backend")]
enum BackendDefinition {
    SoftFloat,
    Native,
}

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
    /// The SCSI topology cannot preserve its attached storage objects.
    Scsi(ScsiSnapshotError),
}

impl fmt::Display for MachineSnapshotError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::IncompatibleMachineModel => {
                formatter.write_str("machine snapshot model does not match the configured machine")
            }
            Self::Scsi(error) => error.fmt(formatter),
        }
    }
}

impl Error for MachineSnapshotError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::IncompatibleMachineModel => None,
            Self::Scsi(error) => Some(error),
        }
    }
}

impl From<ScsiSnapshotError> for MachineSnapshotError {
    fn from(error: ScsiSnapshotError) -> Self {
        Self::Scsi(error)
    }
}

/// An error encountered while executing one machine instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionError {
    /// An Indigo IP12 processor step failed.
    IndigoIp12(StepError),
}

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

    /// Supplies host bytes to one external serial receiver.
    ///
    /// Returns the number of bytes consumed by the selected machine.
    pub fn receive_serial(&mut self, port: SerialPort, bytes: &[u8]) -> usize {
        match self {
            Self::IndigoIp12(machine) => machine.receive_serial(port, bytes),
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
