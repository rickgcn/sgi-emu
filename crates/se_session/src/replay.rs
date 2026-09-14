//! Cold Replay composition from the recorded machine and exact host identities.

use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use se_config::draft::MachineDraft;
use se_core::storage::StorageAccess;
use se_machine::indigo::ip12::builder::{self, Ip12AssemblyError};
use se_machine::indigo::ip12::definition::Ip12Definition;
use se_machine::indigo::ip12::plan::Ip12CompileError;
use se_machine::machine::Machine;
use se_machine::resource::{
    PrepareResourcesError, PreparedResource, ResourceId, ResourceKind, ResourceRequirements,
};
use se_runtime::record::{MediaIdentity, RecordError, RecordedResource, Replayer};
use se_runtime::runtime::RuntimeConfiguration;

use crate::file_storage::HostFileStorage;
use crate::normal::HostResourceError;
use crate::replay_storage::ReplayStorageMedium;

/// Failure to prepare one recorded host resource.
#[derive(Debug)]
pub enum ReplayResourceError {
    /// Reading or hashing the selected host file failed.
    Host(HostResourceError),
    /// The selected file differs from its recorded content identity.
    IdentityMismatch {
        /// Selected host path.
        path: PathBuf,
    },
}

impl fmt::Display for ReplayResourceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Host(error) => error.fmt(formatter),
            Self::IdentityMismatch { path } => {
                write!(
                    formatter,
                    "Replay resource content mismatch for '{}'",
                    path.display()
                )
            }
        }
    }
}

impl Error for ReplayResourceError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Host(error) => Some(error),
            Self::IdentityMismatch { .. } => None,
        }
    }
}

/// Failure to construct a Replay machine.
#[derive(Debug)]
pub enum ReplayBuildError {
    /// The Record or snapshot could not be opened.
    Open(RecordError),
    /// The recorded machine cannot be compiled.
    RecordedMachine(Ip12CompileError),
    /// The manifest resource set differs from its recorded machine plan.
    ResourceContract(String),
    /// A recorded resource could not be prepared or verified.
    Prepare(PrepareResourcesError<ReplayResourceError>),
    /// Validated resources could not be assembled.
    Assemble(Ip12AssemblyError),
}

impl fmt::Display for ReplayBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open(error) => error.fmt(formatter),
            Self::RecordedMachine(error) => write!(
                formatter,
                "invalid recorded machine: {}",
                error
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; ")
            ),
            Self::ResourceContract(error) => formatter.write_str(error),
            Self::Prepare(error) => error.fmt(formatter),
            Self::Assemble(error) => error.fmt(formatter),
        }
    }
}

impl Error for ReplayBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Open(error) => Some(error),
            Self::RecordedMachine(error) => Some(error),
            Self::ResourceContract(_) => None,
            Self::Prepare(error) => Some(error),
            Self::Assemble(error) => Some(error),
        }
    }
}

/// Builds the recorded machine using current settings only for exact resource path hints.
///
/// # Errors
///
/// Returns the failing Record, resource, identity, or machine assembly error.
pub fn build_configuration(
    current_draft: MachineDraft,
    record_path: PathBuf,
    snapshot_id: Option<String>,
) -> Result<RuntimeConfiguration, ReplayBuildError> {
    let replayer = match snapshot_id.as_deref() {
        Some(id) => Replayer::open_snapshot(record_path, id),
        None => Replayer::open(record_path),
    }
    .map_err(ReplayBuildError::Open)?;
    let manifest = replayer.manifest();
    let plan = Ip12Definition
        .compile(manifest.machine())
        .map_err(ReplayBuildError::RecordedMachine)?;
    validate_contract(plan.resources(), manifest.resources())?;
    let current = Ip12Definition
        .compile(&current_draft)
        .ok()
        .map(|plan| plan.resources().clone());
    let prepared = plan
        .prepare_with(|id, requirement| {
            let recorded = &manifest.resources()[id];
            let path = current
                .as_ref()
                .and_then(|requirements| requirements.get(id))
                .filter(|current| current.kind == requirement.kind)
                .map_or_else(
                    || PathBuf::from(&recorded.identity.path_hint),
                    |current| current.path.clone(),
                );
            prepare_requirement(id, requirement.kind, recorded, &path, &replayer)
        })
        .map_err(ReplayBuildError::Prepare)?;
    let mut machine =
        Machine::IndigoIp12(builder::build(prepared).map_err(ReplayBuildError::Assemble)?);
    machine.restore_nonvolatile_state(manifest.nonvolatile_state().clone(), 0);
    Ok(RuntimeConfiguration::replaying(machine, replayer))
}

fn validate_contract(
    requirements: &ResourceRequirements,
    recorded: &std::collections::BTreeMap<ResourceId, RecordedResource>,
) -> Result<(), ReplayBuildError> {
    for (id, requirement) in requirements.iter() {
        match recorded.get(id) {
            None => {
                return Err(ReplayBuildError::ResourceContract(format!(
                    "recorded resource {} is missing",
                    id.as_str()
                )));
            }
            Some(value) if value.kind != requirement.kind => {
                return Err(ReplayBuildError::ResourceContract(format!(
                    "recorded resource {} has the wrong kind",
                    id.as_str()
                )));
            }
            Some(_) => {}
        }
    }
    for id in recorded.keys() {
        if requirements.get(id).is_none() {
            return Err(ReplayBuildError::ResourceContract(format!(
                "recorded resource {} is not required by the machine",
                id.as_str()
            )));
        }
    }
    Ok(())
}

fn prepare_requirement(
    id: &ResourceId,
    kind: ResourceKind,
    recorded: &RecordedResource,
    path: &Path,
    replayer: &Replayer,
) -> Result<PreparedResource, ReplayResourceError> {
    let (prepared, actual) = match kind {
        ResourceKind::Bytes => {
            let bytes = fs::read(path).map_err(|source| {
                ReplayResourceError::Host(HostResourceError::ReadBytes {
                    path: path.to_path_buf(),
                    source,
                })
            })?;
            let identity = MediaIdentity::from_bytes(path, &bytes);
            (PreparedResource::Bytes(bytes), identity)
        }
        ResourceKind::Storage { access } => {
            let mut base = HostFileStorage::open_read_only(path).map_err(|source| {
                ReplayResourceError::Host(HostResourceError::OpenStorage {
                    path: path.to_path_buf(),
                    access: StorageAccess::ReadOnly,
                    source,
                })
            })?;
            let identity = MediaIdentity::from_storage(path, &mut base).map_err(|source| {
                ReplayResourceError::Host(HostResourceError::HashStorage {
                    path: path.to_path_buf(),
                    source,
                })
            })?;
            let medium: Box<dyn se_core::storage::StorageMedium> = match access {
                StorageAccess::ReadOnly => Box::new(base),
                StorageAccess::ReadWrite => {
                    Box::new(ReplayStorageMedium::new(base, replayer.storage(id.clone())))
                }
            };
            (PreparedResource::Storage { access, medium }, identity)
        }
    };
    if actual.size_bytes != recorded.identity.size_bytes
        || actual.sha256 != recorded.identity.sha256
    {
        return Err(ReplayResourceError::IdentityMismatch {
            path: path.to_path_buf(),
        });
    }
    Ok(prepared)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use se_config::definition::MachineDefinition;
    use se_config::draft::Edit;
    use se_config::id::PropertyId;
    use se_config::value::PropertyValue;
    use se_core::storage::StorageAccess;
    use se_machine::indigo::ip12::definition::Ip12Definition;
    use se_machine::resource::{ResourceId, ResourceKind};
    use se_runtime::record::{MediaIdentity, RecordedResource};

    use super::validate_contract;

    #[test]
    fn replay_contract_rejects_missing_extra_and_wrong_kinds() {
        let mut draft = Ip12Definition.default_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("firmware.0.image-path")),
            value: PropertyValue::Text(String::from("prom.bin")),
        });
        let plan = Ip12Definition.compile(&draft).unwrap();
        let id = ResourceId::new("firmware.0.image");
        let identity = MediaIdentity::from_bytes(Path::new("prom.bin"), &[0]);
        let mut resources = BTreeMap::new();
        assert!(validate_contract(plan.resources(), &resources).is_err());
        resources.insert(
            id.clone(),
            RecordedResource {
                kind: ResourceKind::Storage {
                    access: StorageAccess::ReadOnly,
                },
                identity: identity.clone(),
            },
        );
        assert!(validate_contract(plan.resources(), &resources).is_err());
        resources.get_mut(&id).unwrap().kind = ResourceKind::Bytes;
        assert!(validate_contract(plan.resources(), &resources).is_ok());
        resources.insert(
            ResourceId::new("unexpected"),
            RecordedResource {
                kind: ResourceKind::Bytes,
                identity,
            },
        );
        assert!(validate_contract(plan.resources(), &resources).is_err());
    }
}
