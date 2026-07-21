//! Component identity and lifecycle.
//!
//! A component is the unit of ownership in the emulator.
//! It represents a hardware object with identity, private state, and lifecycle,
//! but it does not define how the object communicates with other hardware.
//!
//! Observable hardware behavior is expressed by roles and protocols instead of
//! the base component abstraction. This keeps component identity separate from
//! bus topology and protocol-specific behavior.
//!
//! A single component may participate in multiple roles at the same time. For
//! example, a bridge can be a device on an upstream bus and a bus controller or
//! bus for a downstream communication domain.
//!
//! This module should contain only component identity, naming, hierarchy, and
//! lifecycle concepts. It should not contain bus routing, protocol semantics,
//! or scheduler policy.
use core::any::Any;
use core::fmt;

/// Failure while restoring one component's serialized state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentStateError {
    /// Serialized state belongs to a different topology component.
    ComponentIdMismatch {
        /// Stable identifier required by the rebuilt topology.
        expected: ComponentId,
        /// Identifier stored in the serialized component state.
        actual: ComponentId,
    },
    /// Serialized state was captured for a different immutable configuration.
    ConfigurationMismatch {
        /// Component whose configuration did not match.
        component: ComponentId,
        /// Immutable field or configuration group that differed.
        field: &'static str,
    },
    /// Serialized dynamic state violates a component invariant.
    InvalidState {
        /// Component whose serialized state is invalid.
        component: ComponentId,
        /// Invariant rejected by the component.
        invariant: &'static str,
    },
}

impl fmt::Display for ComponentStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentIdMismatch { expected, actual } => write!(
                formatter,
                "component state identifier mismatch: expected {expected}, got {actual}"
            ),
            Self::ConfigurationMismatch { component, field } => write!(
                formatter,
                "component state configuration mismatch for {component}: {field}"
            ),
            Self::InvalidState {
                component,
                invariant,
            } => write!(
                formatter,
                "invalid serialized state for {component}: {invariant}"
            ),
        }
    }
}

impl std::error::Error for ComponentStateError {}

/// Stable identifier for a component.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct ComponentId(u64);

impl ComponentId {
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for ComponentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "component:{}", self.0)
    }
}

/// Common identity and lifecycle interface for all emulator components.
pub trait Component: Any {
    /// Returns the stable component identifier.
    fn id(&self) -> ComponentId;

    /// Returns the human-readable component name.
    fn name(&self) -> &str;

    /// Resets the component to its deterministic initial state.
    fn reset(&mut self);
}

/// Validates the topology identity stored in one explicit component state.
pub fn validate_component_state_id(
    expected: ComponentId,
    actual: ComponentId,
) -> Result<(), ComponentStateError> {
    if actual == expected {
        Ok(())
    } else {
        Err(ComponentStateError::ComponentIdMismatch { expected, actual })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_state_identity_accepts_a_match() {
        let id = ComponentId::new(1);
        assert_eq!(validate_component_state_id(id, id), Ok(()));
    }

    #[test]
    fn component_state_identity_rejects_a_mismatch() {
        assert_eq!(
            validate_component_state_id(ComponentId::new(1), ComponentId::new(2)),
            Err(ComponentStateError::ComponentIdMismatch {
                expected: ComponentId::new(1),
                actual: ComponentId::new(2),
            })
        );
    }
}
