//! Logical host-resource requirements for machine build plans.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::PathBuf;

use se_device::storage::BlockStorage;

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
    /// Prepares every requirement with a caller-supplied capability provider.
    ///
    /// Prepared capabilities are owned by the result. If the provider fails or
    /// returns a different kind or access mode, all earlier capabilities are
    /// dropped and no partial result is returned.
    ///
    /// # Errors
    ///
    /// Returns [`PrepareResourcesError`] with the resource role that failed.
    pub fn prepare_with<E>(
        &self,
        mut prepare: impl FnMut(&ResourceId, &ResourceRequirement) -> Result<PreparedResource, E>,
    ) -> Result<PreparedResources, PrepareResourcesError<E>> {
        let mut resources = BTreeMap::new();
        for (id, requirement) in &self.resources {
            let prepared =
                prepare(id, requirement).map_err(|source| PrepareResourcesError::Provider {
                    id: id.clone(),
                    source,
                })?;
            let provided = prepared.kind();
            if provided != requirement.kind {
                return Err(PrepareResourcesError::KindMismatch {
                    id: id.clone(),
                    required: requirement.kind,
                    provided,
                });
            }
            resources.insert(id.clone(), prepared);
        }
        Ok(PreparedResources { resources })
    }

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

/// A prepared capability ready for direct machine assembly.
///
/// Host paths are not retained after preparation.
pub enum PreparedResource {
    /// A complete byte image, such as firmware.
    Bytes(Vec<u8>),
    /// An owned block-storage capability with its exact access mode.
    BlockStorage {
        /// Access granted to the machine.
        access: BlockStorageAccess,
        /// Storage used directly by an attached device.
        storage: Box<dyn BlockStorage>,
    },
}

impl PreparedResource {
    /// Returns the capability kind and access mode.
    #[must_use]
    pub fn kind(&self) -> ResourceKind {
        match self {
            Self::Bytes(_) => ResourceKind::Bytes,
            Self::BlockStorage { access, .. } => ResourceKind::BlockStorage { access: *access },
        }
    }
}

/// An owned, complete set of prepared machine capabilities.
///
/// This value can only be produced by successfully preparing every item in a
/// [`ResourceRequirements`] collection.
pub struct PreparedResources {
    resources: BTreeMap<ResourceId, PreparedResource>,
}

impl PreparedResources {
    pub(crate) fn take_bytes(&mut self, id: &ResourceId) -> Vec<u8> {
        match self.resources.remove(id) {
            Some(PreparedResource::Bytes(bytes)) => bytes,
            _ => panic!("prepared byte resource must match the build plan"),
        }
    }

    pub(crate) fn take_block_storage(&mut self, id: &ResourceId) -> Box<dyn BlockStorage> {
        match self.resources.remove(id) {
            Some(PreparedResource::BlockStorage { storage, .. }) => storage,
            _ => panic!("prepared block storage must match the build plan"),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.resources.is_empty()
    }
}

/// A provider failure or incompatible prepared capability.
#[derive(Debug)]
pub enum PrepareResourcesError<E> {
    /// The provider could not prepare one required resource.
    Provider {
        /// The resource role that failed.
        id: ResourceId,
        /// The provider's error.
        source: E,
    },
    /// The provider returned a different kind or access mode.
    KindMismatch {
        /// The resource role with the incompatible capability.
        id: ResourceId,
        /// The requested kind and access mode.
        required: ResourceKind,
        /// The returned kind and access mode.
        provided: ResourceKind,
    },
}

impl<E> PrepareResourcesError<E> {
    /// Returns the resource role that could not be prepared.
    #[must_use]
    pub fn resource_id(&self) -> &ResourceId {
        match self {
            Self::Provider { id, .. } | Self::KindMismatch { id, .. } => id,
        }
    }
}

impl<E: fmt::Display> fmt::Display for PrepareResourcesError<E> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Provider { id, source } => {
                write!(
                    formatter,
                    "failed to prepare resource {}: {source}",
                    id.as_str()
                )
            }
            Self::KindMismatch {
                id,
                required,
                provided,
            } => write!(
                formatter,
                "resource {} requires {required:?}, got {provided:?}",
                id.as_str()
            ),
        }
    }
}

impl<E: Error + 'static> Error for PrepareResourcesError<E> {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Provider { source, .. } => Some(source),
            Self::KindMismatch { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::PathBuf;
    use std::sync::mpsc::{self, Sender};

    use se_device::storage::BlockStorage;

    use super::{
        BlockStorageAccess, PrepareResourcesError, PreparedResource, ResourceId, ResourceKind,
        ResourceRequirement, ResourceRequirements,
    };

    struct TestStorage {
        dropped: Option<Sender<()>>,
    }

    impl Drop for TestStorage {
        fn drop(&mut self) {
            if let Some(sender) = &self.dropped {
                sender.send(()).expect("the drop observer must still exist");
            }
        }
    }

    impl BlockStorage for TestStorage {
        fn size_bytes(&self) -> u64 {
            512
        }

        fn read_exact_at(&mut self, _offset: u64, _buffer: &mut [u8]) -> io::Result<()> {
            Err(io::Error::other("test storage does not perform I/O"))
        }

        fn write_all_at(&mut self, _offset: u64, _data: &[u8]) -> io::Result<()> {
            Err(io::Error::other("test storage does not perform I/O"))
        }
    }

    fn requirement(kind: ResourceKind) -> ResourceRequirement {
        ResourceRequirement {
            path: PathBuf::from("unused-host-path"),
            kind,
        }
    }

    #[test]
    fn complete_preparation_owns_each_capability() {
        let mut requirements = ResourceRequirements::default();
        let bytes = ResourceId::new("a.bytes");
        let block = ResourceId::new("b.block");
        requirements.insert(bytes.clone(), requirement(ResourceKind::Bytes));
        requirements.insert(
            block.clone(),
            requirement(ResourceKind::BlockStorage {
                access: BlockStorageAccess::ReadOnly,
            }),
        );
        let mut prepared = requirements
            .prepare_with(|_, item| {
                Ok::<_, io::Error>(match item.kind {
                    ResourceKind::Bytes => PreparedResource::Bytes(vec![1, 2, 3]),
                    ResourceKind::BlockStorage { access } => PreparedResource::BlockStorage {
                        access,
                        storage: Box::new(TestStorage { dropped: None }),
                    },
                })
            })
            .expect("all required capabilities must prepare");
        assert_eq!(prepared.take_bytes(&bytes), [1, 2, 3]);
        assert_eq!(prepared.take_block_storage(&block).size_bytes(), 512);
        assert!(prepared.is_empty());
    }

    #[test]
    fn provider_failure_drops_earlier_capabilities() {
        let mut requirements = ResourceRequirements::default();
        requirements.insert(
            ResourceId::new("a.block"),
            requirement(ResourceKind::BlockStorage {
                access: BlockStorageAccess::ReadWrite,
            }),
        );
        requirements.insert(ResourceId::new("b.bytes"), requirement(ResourceKind::Bytes));
        let (sender, receiver) = mpsc::channel();
        let error = match requirements.prepare_with(|id, _| {
            if id.as_str() == "a.block" {
                Ok(PreparedResource::BlockStorage {
                    access: BlockStorageAccess::ReadWrite,
                    storage: Box::new(TestStorage {
                        dropped: Some(sender.clone()),
                    }),
                })
            } else {
                Err(io::Error::other("provider failed"))
            }
        }) {
            Ok(_) => panic!("partial preparation must fail"),
            Err(error) => error,
        };
        assert_eq!(error.resource_id().as_str(), "b.bytes");
        assert!(matches!(error, PrepareResourcesError::Provider { .. }));
        receiver
            .try_recv()
            .expect("the first storage must be dropped on failure");
    }

    #[test]
    fn wrong_resource_kind_reports_its_role() {
        let mut requirements = ResourceRequirements::default();
        requirements.insert(
            ResourceId::new("firmware"),
            requirement(ResourceKind::Bytes),
        );
        let error = match requirements.prepare_with(|_, _| {
            Ok::<_, io::Error>(PreparedResource::BlockStorage {
                access: BlockStorageAccess::ReadOnly,
                storage: Box::new(TestStorage { dropped: None }),
            })
        }) {
            Ok(_) => panic!("block storage cannot satisfy a byte image"),
            Err(error) => error,
        };
        assert_eq!(error.resource_id().as_str(), "firmware");
        assert!(matches!(
            error,
            PrepareResourcesError::KindMismatch {
                required: ResourceKind::Bytes,
                provided: ResourceKind::BlockStorage { .. },
                ..
            }
        ));
    }

    #[test]
    fn block_storage_access_requires_an_exact_match() {
        let mut requirements = ResourceRequirements::default();
        requirements.insert(
            ResourceId::new("medium"),
            requirement(ResourceKind::BlockStorage {
                access: BlockStorageAccess::ReadOnly,
            }),
        );
        let error = match requirements.prepare_with(|_, _| {
            Ok::<_, io::Error>(PreparedResource::BlockStorage {
                access: BlockStorageAccess::ReadWrite,
                storage: Box::new(TestStorage { dropped: None }),
            })
        }) {
            Ok(_) => panic!("write access must not silently satisfy read-only access"),
            Err(error) => error,
        };
        assert_eq!(error.resource_id().as_str(), "medium");
        assert!(matches!(
            error,
            PrepareResourcesError::KindMismatch {
                required: ResourceKind::BlockStorage {
                    access: BlockStorageAccess::ReadOnly
                },
                provided: ResourceKind::BlockStorage {
                    access: BlockStorageAccess::ReadWrite
                },
                ..
            }
        ));
    }
}
