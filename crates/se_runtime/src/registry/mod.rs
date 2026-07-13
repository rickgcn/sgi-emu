//! Component ownership registry.
//!
//! The registry owns component instances and indexes them by stable
//! [`ComponentId`]. Components should refer to each other by identifiers, not by
//! Rust references, so the runtime can borrow and dispatch components without
//! creating ownership cycles.

use core::any::type_name;
use core::fmt;
use core::marker::PhantomData;

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

    /// The registry topology changed after a slot was resolved.
    StaleSlot {
        /// Component identifier held by the stale slot.
        id: ComponentId,
    },
}

impl fmt::Display for RegistryLookupError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingComponent { id } => write!(f, "missing component {id}"),
            Self::TypeMismatch { id, expected } => {
                write!(f, "component {id} is not of type {expected}")
            }
            Self::StaleSlot { id } => write!(f, "component slot for {id} is stale"),
        }
    }
}

impl std::error::Error for RegistryLookupError {}

/// Ordered component storage owned by the runtime.
pub struct ComponentRegistry {
    components: Vec<Box<dyn Component>>,
    topology_generation: u64,
}

impl Default for ComponentRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Type-checked position in one immutable registry topology.
pub struct ComponentSlot<T> {
    index: usize,
    id: ComponentId,
    generation: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T> Copy for ComponentSlot<T> {}

impl<T> Clone for ComponentSlot<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> fmt::Debug for ComponentSlot<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ComponentSlot")
            .field("index", &self.index)
            .field("id", &self.id)
            .field("generation", &self.generation)
            .finish()
    }
}

impl<T> PartialEq for ComponentSlot<T> {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.id == other.id && self.generation == other.generation
    }
}

impl<T> Eq for ComponentSlot<T> {}

impl<T> ComponentSlot<T> {
    /// Returns the stable component identifier represented by this slot.
    pub const fn id(self) -> ComponentId {
        self.id
    }
}

impl ComponentRegistry {
    /// Creates an empty component registry.
    pub const fn new() -> Self {
        Self {
            components: Vec::new(),
            topology_generation: 0,
        }
    }

    /// Inserts a component.
    ///
    /// Component identifiers must be unique. Replacing an existing component
    /// would make lifecycle and trace ordering ambiguous, so duplicates are
    /// rejected.
    pub fn insert(&mut self, component: Box<dyn Component>) -> Result<(), RegistryError> {
        let id = component.id();
        let index = match self
            .components
            .binary_search_by_key(&id, |component| component.id())
        {
            Ok(_) => return Err(RegistryError::DuplicateComponent { id }),
            Err(index) => index,
        };
        self.components.insert(index, component);
        self.topology_generation = self.topology_generation.wrapping_add(1);
        Ok(())
    }

    /// Returns an immutable component reference.
    pub fn get(&self, id: ComponentId) -> Option<&dyn Component> {
        self.index_of(id)
            .map(|index| self.components[index].as_ref())
    }

    /// Returns a mutable component reference.
    pub fn get_mut(&mut self, id: ComponentId) -> Option<&mut dyn Component> {
        let index = self.index_of(id)?;
        Some(self.components[index].as_mut())
    }

    /// Returns an immutable component reference with its concrete type checked.
    pub fn get_typed<T>(&self, id: ComponentId) -> Result<&T, RegistryLookupError>
    where
        T: Component,
    {
        let index = self
            .index_of(id)
            .ok_or(RegistryLookupError::MissingComponent { id })?;
        let component = &self.components[index];
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
        let index = self
            .index_of(id)
            .ok_or(RegistryLookupError::MissingComponent { id })?;
        let component = &mut self.components[index];
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
        let index = self.index_of(id)?;
        self.topology_generation = self.topology_generation.wrapping_add(1);
        Some(self.components.remove(index))
    }

    /// Returns whether the registry contains a component.
    pub fn contains(&self, id: ComponentId) -> bool {
        self.index_of(id).is_some()
    }

    /// Resets all components in stable component identifier order.
    pub fn reset_all(&mut self) {
        for component in &mut self.components {
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

    /// Resolves a typed component into a topology-checked slot.
    pub fn resolve<T>(&self, id: ComponentId) -> Result<ComponentSlot<T>, RegistryLookupError>
    where
        T: Component,
    {
        let index = self
            .index_of(id)
            .ok_or(RegistryLookupError::MissingComponent { id })?;
        let component: &dyn core::any::Any = self.components[index].as_ref();
        if !component.is::<T>() {
            return Err(RegistryLookupError::TypeMismatch {
                id,
                expected: type_name::<T>(),
            });
        }
        Ok(ComponentSlot {
            index,
            id,
            generation: self.topology_generation,
            marker: PhantomData,
        })
    }

    /// Returns an immutable component through a resolved slot.
    pub fn get_resolved<T>(&self, slot: ComponentSlot<T>) -> Result<&T, RegistryLookupError>
    where
        T: Component,
    {
        self.validate_slot(slot)?;
        let component: &dyn core::any::Any = self.components[slot.index].as_ref();
        component
            .downcast_ref::<T>()
            .ok_or(RegistryLookupError::TypeMismatch {
                id: slot.id,
                expected: type_name::<T>(),
            })
    }

    /// Returns a mutable component through a resolved slot.
    pub fn get_resolved_mut<T>(
        &mut self,
        slot: ComponentSlot<T>,
    ) -> Result<&mut T, RegistryLookupError>
    where
        T: Component,
    {
        self.validate_slot(slot)?;
        let component: &mut dyn core::any::Any = self.components[slot.index].as_mut();
        component
            .downcast_mut::<T>()
            .ok_or(RegistryLookupError::TypeMismatch {
                id: slot.id,
                expected: type_name::<T>(),
            })
    }

    fn index_of(&self, id: ComponentId) -> Option<usize> {
        self.components
            .binary_search_by_key(&id, |component| component.id())
            .ok()
    }

    fn validate_slot<T>(&self, slot: ComponentSlot<T>) -> Result<(), RegistryLookupError> {
        if slot.generation != self.topology_generation
            || self
                .components
                .get(slot.index)
                .is_none_or(|component| component.id() != slot.id)
        {
            return Err(RegistryLookupError::StaleSlot { id: slot.id });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
