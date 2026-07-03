//! Component ownership registry.
//!
//! The registry owns component instances and indexes them by stable
//! [`ComponentId`]. Components should refer to each other by identifiers, not by
//! Rust references, so the runtime can borrow and dispatch components without
//! creating ownership cycles.

use core::fmt;
use std::collections::BTreeMap;

use se_core::component::{Component, ComponentId};

/// Errors produced by component registry operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryError {
    /// A component with the same identifier already exists.
    DuplicateComponent {
        /// Duplicate component identifier.
        id: ComponentId,
    },
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateComponent { id } => write!(f, "duplicate component {id}"),
        }
    }
}

impl std::error::Error for RegistryError {}

/// Ordered component storage owned by the runtime.
#[derive(Default)]
pub struct ComponentRegistry {
    components: BTreeMap<ComponentId, Box<dyn Component>>,
}

impl ComponentRegistry {
    /// Creates an empty component registry.
    pub const fn new() -> Self {
        Self {
            components: BTreeMap::new(),
        }
    }

    /// Inserts a component.
    ///
    /// Component identifiers must be unique. Replacing an existing component
    /// would make lifecycle and trace ordering ambiguous, so duplicates are
    /// rejected.
    pub fn insert(&mut self, component: Box<dyn Component>) -> Result<(), RegistryError> {
        let id = component.id();
        if self.components.contains_key(&id) {
            return Err(RegistryError::DuplicateComponent { id });
        }

        self.components.insert(id, component);
        Ok(())
    }

    /// Returns an immutable component reference.
    pub fn get(&self, id: ComponentId) -> Option<&(dyn Component + '_)> {
        self.components.get(&id).map(Box::as_ref)
    }

    /// Returns a mutable component reference.
    pub fn get_mut(&mut self, id: ComponentId) -> Option<&mut (dyn Component + '_)> {
        match self.components.get_mut(&id) {
            Some(component) => Some(component.as_mut()),
            None => None,
        }
    }

    /// Removes and returns a component.
    pub fn remove(&mut self, id: ComponentId) -> Option<Box<dyn Component>> {
        self.components.remove(&id)
    }

    /// Returns whether the registry contains a component.
    pub fn contains(&self, id: ComponentId) -> bool {
        self.components.contains_key(&id)
    }

    /// Resets all components in stable component identifier order.
    pub fn reset_all(&mut self) {
        for component in self.components.values_mut() {
            component.reset();
        }
    }

    /// Returns the number of registered components.
    pub fn len(&self) -> usize {
        self.components.len()
    }

    /// Returns whether the registry contains no components.
    pub fn is_empty(&self) -> bool {
        self.components.is_empty()
    }
}

#[cfg(test)]
mod tests;
