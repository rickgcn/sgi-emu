//! Runtime-facing dispatch across supported machine models.

use std::error::Error;
use std::fmt;

use se_cpu::mips1::r3000::StepError;

use crate::debug::{DebugRequest, DebugResponse};
use crate::indigo::ip12::Ip12;

/// A configured emulated machine.
pub enum Machine {
    /// An SGI Indigo IP12.
    IndigoIp12(Ip12),
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
            (Self::IndigoIp12(machine), DebugRequest::IndigoIp12(request)) => {
                DebugResponse::IndigoIp12(machine.debug(request))
            }
        }
    }
}
