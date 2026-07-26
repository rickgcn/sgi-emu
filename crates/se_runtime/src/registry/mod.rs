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

    /// Two mutable lookups selected the same component.
    AliasedMutableAccess {
        /// Component identifier selected more than once.
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
            Self::AliasedMutableAccess { id } => {
                write!(f, "component {id} cannot be mutably borrowed twice")
            }
        }
    }
}

impl std::error::Error for RegistryLookupError {}

/// Ordered component storage owned by the runtime.
pub struct ComponentRegistry {
    components: Vec<Box<dyn Component>>,
    topology_generation: u64,
    identity: Box<u8>,
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
    registry_identity: usize,
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
    pub fn new() -> Self {
        Self {
            components: Vec::new(),
            topology_generation: 0,
            identity: Box::new(0),
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
        self.advance_topology_generation();
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
        self.advance_topology_generation();
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
            registry_identity: self.identity.as_ref() as *const u8 as usize,
            marker: PhantomData,
        })
    }

    /// Returns an immutable component through a resolved slot.
    pub fn get_resolved<T>(&self, slot: ComponentSlot<T>) -> Result<&T, RegistryLookupError>
    where
        T: Component,
    {
        self.validate_slot(slot)?;
        let component = self.components[slot.index].as_ref();
        debug_assert_eq!(component.id(), slot.id);
        let component: &dyn core::any::Any = component;
        debug_assert!(component.is::<T>());
        // SAFETY: `resolve` established the concrete type at this index. The
        // registry identity and topology generation prove that neither another
        // registry nor a topology mutation can reuse the resolved slot.
        Ok(unsafe { &*(component as *const dyn core::any::Any as *const T) })
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
        let component = self.components[slot.index].as_mut();
        debug_assert_eq!(component.id(), slot.id);
        let component: &mut dyn core::any::Any = component;
        debug_assert!(component.is::<T>());
        // SAFETY: `resolve` established the concrete type at this index. The
        // registry identity and topology generation prove that neither another
        // registry nor a topology mutation can reuse the resolved slot.
        Ok(unsafe { &mut *(component as *mut dyn core::any::Any as *mut T) })
    }

    /// Returns two distinct mutable components through resolved slots.
    pub fn get_resolved_pair_mut<T, U>(
        &mut self,
        first: ComponentSlot<T>,
        second: ComponentSlot<U>,
    ) -> Result<(&mut T, &mut U), RegistryLookupError>
    where
        T: Component,
        U: Component,
    {
        self.validate_slot(first)?;
        self.validate_slot(second)?;
        if first.index == second.index {
            return Err(RegistryLookupError::AliasedMutableAccess { id: first.id });
        }
        let (first_component, second_component) = if first.index < second.index {
            let (before_second, from_second) = self.components.split_at_mut(second.index);
            (before_second[first.index].as_mut(), from_second[0].as_mut())
        } else {
            let (before_first, from_first) = self.components.split_at_mut(first.index);
            (from_first[0].as_mut(), before_first[second.index].as_mut())
        };
        debug_assert_eq!(first_component.id(), first.id);
        debug_assert_eq!(second_component.id(), second.id);
        let first_component: &mut dyn core::any::Any = first_component;
        let second_component: &mut dyn core::any::Any = second_component;
        debug_assert!(first_component.is::<T>());
        debug_assert!(second_component.is::<U>());
        // SAFETY: Both slots were resolved against this registry and validated
        // against its unchanged topology. The split above also proves that the
        // two resulting mutable references do not alias.
        let first_component =
            unsafe { &mut *(first_component as *mut dyn core::any::Any as *mut T) };
        // SAFETY: The same slot and split invariants apply to the second type.
        let second_component =
            unsafe { &mut *(second_component as *mut dyn core::any::Any as *mut U) };
        Ok((first_component, second_component))
    }

    fn advance_topology_generation(&mut self) {
        self.topology_generation = self
            .topology_generation
            .checked_add(1)
            .expect("component registry topology generation exhausted");
    }

    fn index_of(&self, id: ComponentId) -> Option<usize> {
        self.components
            .binary_search_by_key(&id, |component| component.id())
            .ok()
    }

    fn validate_slot<T>(&self, slot: ComponentSlot<T>) -> Result<(), RegistryLookupError> {
        if slot.registry_identity != self.identity.as_ref() as *const u8 as usize
            || slot.generation != self.topology_generation
            || slot.index >= self.components.len()
        {
            return Err(RegistryLookupError::StaleSlot { id: slot.id });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
