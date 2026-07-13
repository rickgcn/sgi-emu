//! Common validation failures for explicit device-state DTOs.

use core::fmt;

use se_core::component::ComponentId;

/// Failure while restoring one component's serialized state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceStateError {
    /// Serialized state belongs to a different topology component.
    ComponentIdMismatch {
        /// Stable identifier required by the rebuilt topology.
        expected: ComponentId,
        /// Identifier stored in the serialized component state.
        actual: ComponentId,
    },
}

impl fmt::Display for DeviceStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentIdMismatch { expected, actual } => write!(
                formatter,
                "component state identifier mismatch: expected {expected}, got {actual}"
            ),
        }
    }
}

impl std::error::Error for DeviceStateError {}
