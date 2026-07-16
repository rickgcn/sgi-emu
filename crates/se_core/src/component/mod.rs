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
}

impl fmt::Display for ComponentStateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ComponentIdMismatch { expected, actual } => write!(
                formatter,
                "component state identifier mismatch: expected {expected}, got {actual}"
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

/// Defines a serializable deterministic state wrapper for a component.
#[macro_export]
macro_rules! component_state {
    ($state:ident, $component:ty) => {
        #[doc = "Serializable deterministic component state."]
        #[derive(Clone, serde::Deserialize, serde::Serialize)]
        pub struct $state($component);

        impl $component {
            #[doc = "Captures all hardware-visible and in-flight component state."]
            pub fn save_state(&self) -> $state {
                $state(self.clone())
            }

            #[doc = "Restores validated component state without changing topology identity."]
            pub fn restore_state(
                &mut self,
                state: $state,
            ) -> Result<(), $crate::component::ComponentStateError> {
                let expected = $crate::component::Component::id(self);
                let actual = $crate::component::Component::id(&state.0);
                if actual != expected {
                    return Err(
                        $crate::component::ComponentStateError::ComponentIdMismatch {
                            expected,
                            actual,
                        },
                    );
                }
                *self = state.0;
                Ok(())
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
    struct TestComponent {
        id: ComponentId,
        name: String,
        value: u64,
    }

    crate::component_state!(TestComponentState, TestComponent);

    impl TestComponent {
        fn new(id: u64, name: &str, value: u64) -> Self {
            Self {
                id: ComponentId::new(id),
                name: name.to_owned(),
                value,
            }
        }
    }

    impl Component for TestComponent {
        fn id(&self) -> ComponentId {
            self.id
        }

        fn name(&self) -> &str {
            &self.name
        }

        fn reset(&mut self) {
            self.value = 0;
        }
    }

    #[test]
    fn component_state_restores_matching_identity() {
        let source = TestComponent::new(1, "source", 42);
        let mut target = TestComponent::new(1, "target", 7);

        target.restore_state(source.save_state()).unwrap();

        assert_eq!(target, source);
    }

    #[test]
    fn component_state_rejects_mismatched_identity_without_modification() {
        let source = TestComponent::new(2, "source", 42);
        let mut target = TestComponent::new(1, "target", 7);
        let original = target.clone();

        assert_eq!(
            target.restore_state(source.save_state()),
            Err(ComponentStateError::ComponentIdMismatch {
                expected: ComponentId::new(1),
                actual: ComponentId::new(2),
            })
        );
        assert_eq!(target, original);
    }
}
