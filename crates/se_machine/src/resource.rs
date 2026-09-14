//! Logical host-resource requirements for machine build plans.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// A stable resource role within one build plan.
///
/// The role identifies a topology location, not a host file or a device
/// instance. Moving a device to another location may change its resource ID.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ResourceId(String);

impl ResourceId {
    /// Creates a logical resource role.
    #[must_use]
    pub fn new(role: impl Into<String>) -> Self {
        Self(role.into())
    }

    /// Returns the role identifier.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The access needed for a block-storage resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BlockStorageAccess {
    /// The medium is read without writes.
    ReadOnly,
    /// The medium may be read and written.
    ReadWrite,
}

/// The kind of host resource a later preparation step must provide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResourceKind {
    /// A complete byte image, such as firmware.
    Bytes,
    /// A block-storage medium with the requested access.
    BlockStorage {
        /// The access requested by the attached device.
        access: BlockStorageAccess,
    },
}

/// A resource path and the form in which it will be needed.
///
/// The path is preserved from the draft; compilation does not inspect it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResourceRequirement {
    /// The user-supplied host path.
    pub path: PathBuf,
    /// The required resource form.
    pub kind: ResourceKind,
}

/// Resource roles and requirements in deterministic ID order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ResourceRequirements {
    resources: BTreeMap<ResourceId, ResourceRequirement>,
}

impl ResourceRequirements {
    /// Returns the requirement for a logical resource role.
    #[must_use]
    pub fn get(&self, id: &ResourceId) -> Option<&ResourceRequirement> {
        self.resources.get(id)
    }

    /// Iterates over roles and requirements in ID order.
    pub fn iter(&self) -> impl Iterator<Item = (&ResourceId, &ResourceRequirement)> {
        self.resources.iter()
    }

    /// Returns the number of required resources.
    #[must_use]
    pub fn len(&self) -> usize {
        self.resources.len()
    }

    /// Reports whether the plan needs no host resources.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }

    pub(crate) fn insert(&mut self, id: ResourceId, requirement: ResourceRequirement) {
        self.resources.insert(id, requirement);
    }
}
