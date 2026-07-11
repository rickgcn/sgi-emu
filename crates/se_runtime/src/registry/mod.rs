//! Component ownership registry.
//!
//! The registry owns component instances and indexes them by stable
//! [`ComponentId`]. Components should refer to each other by identifiers, not by
//! Rust references, so the runtime can borrow and dispatch components without
//! creating ownership cycles.

use core::any::type_name;
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

/// Errors produced by typed component lookups.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegistryLookupError {
    /// No component is registered with the requested identifier.
    MissingComponent {
        /// Missing component identifier.
        id: ComponentId,
    },

    /// The registered component does not have the requested concrete type.
    TypeMismatch {
        /// Component identifier whose type did not match.
        id: ComponentId,

        /// Requested concrete Rust type.
        expected: &'static str,
    },
}

impl fmt::Display for RegistryLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingComponent { id } => write!(f, "missing component {id}"),
            Self::TypeMismatch { id, expected } => {
                write!(f, "component {id} is not of type {expected}")
            }
        }
    }
}

impl std::error::Error for RegistryLookupError {}

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
    pub fn get(&self, id: ComponentId) -> Option<&dyn Component> {
        self.components.get(&id).map(Box::as_ref)
    }

    /// Returns a mutable component reference.
    pub fn get_mut(&mut self, id: ComponentId) -> Option<&mut dyn Component> {
        match self.components.get_mut(&id) {
            Some(component) => Some(component.as_mut()),
            None => None,
        }
    }

    /// Returns an immutable component reference with its concrete type checked.
    pub fn get_typed<T>(&self, id: ComponentId) -> Result<&T, RegistryLookupError>
    where
        T: Component,
    {
        let component = self
            .components
            .get(&id)
            .ok_or(RegistryLookupError::MissingComponent { id })?;
        let component: &dyn core::any::Any = component.as_ref();
        component
            .downcast_ref::<T>()
            .ok_or(RegistryLookupError::TypeMismatch {
                id,
                expected: type_name::<T>(),
            })
    }

    /// Returns a mutable component reference with its concrete type checked.
    pub fn get_typed_mut<T>(&mut self, id: ComponentId) -> Result<&mut T, RegistryLookupError>
    where
        T: Component,
    {
        let component = self
            .components
            .get_mut(&id)
            .ok_or(RegistryLookupError::MissingComponent { id })?;
        let component: &mut dyn core::any::Any = component.as_mut();
        component
            .downcast_mut::<T>()
            .ok_or(RegistryLookupError::TypeMismatch {
                id,
                expected: type_name::<T>(),
            })
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
