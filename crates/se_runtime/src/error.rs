//! Implements error formatting and source chaining for runtime operations.

use std::error::Error;
use std::fmt;

use se_core::machine::{MachineCreateError, MachineError};
use se_core::snapshot::SnapshotError;

use crate::RuntimeError;

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Machine(error) => write!(formatter, "runtime machine operation failed: {error}"),
            Self::MachineCreate(error) => {
                write!(formatter, "runtime machine creation failed: {error}")
            }
            Self::Snapshot(error) => {
                write!(formatter, "runtime snapshot operation failed: {error}")
            }
            Self::TargetBeforeNow { now, target } => write!(
                formatter,
                "run target {target} precedes current virtual time {now}"
            ),
            Self::InvalidState { operation, state } => {
                write!(
                    formatter,
                    "operation {operation} is invalid while runtime is {state:?}"
                )
            }
        }
    }
}

impl Error for RuntimeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Machine(error) => Some(error),
            Self::MachineCreate(error) => Some(error),
            Self::Snapshot(error) => Some(error),
            Self::TargetBeforeNow { .. } | Self::InvalidState { .. } => None,
        }
    }
}

impl From<MachineError> for RuntimeError {
    fn from(error: MachineError) -> Self {
        Self::Machine(error)
    }
}

impl From<MachineCreateError> for RuntimeError {
    fn from(error: MachineCreateError) -> Self {
        Self::MachineCreate(error)
    }
}

impl From<SnapshotError> for RuntimeError {
    fn from(error: SnapshotError) -> Self {
        Self::Snapshot(error)
    }
}
