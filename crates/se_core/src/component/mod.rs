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
use core::fmt;

/// Stable identifier for a component.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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
pub trait Component {
    /// Returns the stable component identifier.
    fn id(&self) -> ComponentId;

    /// Returns the human-readable component name.
    fn name(&self) -> &str;

    /// Resets the component to its deterministic initial state.
    fn reset(&mut self);
}
