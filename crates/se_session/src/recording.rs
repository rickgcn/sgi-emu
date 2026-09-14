//! Cold Recording composition from a validated draft and host resources.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::PathBuf;

use se_config::draft::MachineDraft;
use se_core::storage::StorageAccess;
use se_machine::indigo::ip12::builder::{self, Ip12AssemblyError};
use se_machine::indigo::ip12::definition::Ip12Definition;
use se_machine::indigo::ip12::plan::Ip12CompileError;
use se_machine::machine::Machine;
use se_machine::resource::{
    PrepareResourcesError, PreparedResource, ResourceId, ResourceKind, ResourceRequirement,
};
use se_network::config::NatConfig;
use se_runtime::record::{MediaIdentity, RecordError, RecordManifest, RecordedResource, Recorder};
use se_runtime::runtime::RuntimeConfiguration;

use crate::file_storage::HostFileStorage;
use crate::normal::HostResourceError;
use crate::persistence;
use crate::recording_storage::RecordingStorageMedium;

/// Failure to construct a cold Recording machine.
#[derive(Debug)]
pub enum RecordingBuildError {
    /// The draft has semantic errors.
    Compile(Ip12CompileError),
    /// The Record writer could not be created.
    Create(RecordError),
    /// A host resource could not be prepared.
    Prepare(PrepareResourcesError<HostResourceError>),
    /// Validated resources could not be assembled.
    Assemble(Ip12AssemblyError),
    /// Retained machine state could not be loaded.
    Persistence(Box<dyn Error>),
    /// The manifest could not be written.
    Start(RecordError),
    /// A failed cold build's partial Record could not be discarded.
    Discard {
        /// The original cold-build failure.
        build: Box<RecordingBuildError>,
        /// The partial-file cleanup failure.
        source: RecordError,
    },
}

impl fmt::Display for RecordingBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compile(error) => formatter.write_str(
                &error
                    .diagnostics()
                    .iter()
                    .map(|diagnostic| diagnostic.message.as_str())
                    .collect::<Vec<_>>()
                    .join("; "),
            ),
            Self::Create(error) | Self::Start(error) => error.fmt(formatter),
            Self::Prepare(error) => error.fmt(formatter),
            Self::Assemble(error) => error.fmt(formatter),
            Self::Persistence(error) => error.fmt(formatter),
            Self::Discard { build, source } => {
                write!(
                    formatter,
                    "{build}; failed to discard partial record: {source}"
                )
            }
        }
    }
}

impl Error for RecordingBuildError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Compile(error) => Some(error),
            Self::Create(error) | Self::Start(error) => Some(error),
            Self::Prepare(error) => Some(error),
            Self::Assemble(error) => Some(error),
            Self::Persistence(error) => Some(error.as_ref()),
            Self::Discard { build, .. } => Some(build.as_ref()),
        }
    }
}

/// Builds a cold Recording machine and starts its Record manifest.
///
/// # Errors
///
/// Returns the failing semantic, resource, assembly, persistence, or Record error.
pub fn build_configuration(
    draft: MachineDraft,
    network: NatConfig,
    record_path: PathBuf,
) -> Result<RuntimeConfiguration, RecordingBuildError> {
    let plan = Ip12Definition
        .compile(&draft)
        .map_err(RecordingBuildError::Compile)?;
    let recorder = Recorder::create_or_replace(record_path).map_err(RecordingBuildError::Create)?;
    let machine = (|| {
        let mut resources = BTreeMap::new();
        let prepared = plan
            .prepare_with(|id, requirement| {
                prepare_requirement(id, requirement, &recorder, &mut resources)
            })
            .map_err(RecordingBuildError::Prepare)?;
        let mut machine =
            Machine::IndigoIp12(builder::build(prepared).map_err(RecordingBuildError::Assemble)?);
        if let Some(restored) =
            persistence::load(&draft.model.0).map_err(RecordingBuildError::Persistence)?
        {
            machine.restore_nonvolatile_state(restored.state, restored.offline_milliseconds);
        }
        let manifest = RecordManifest::new(draft, resources, machine.nonvolatile_state());
        recorder
            .start(&manifest)
            .map_err(RecordingBuildError::Start)?;
        Ok(machine)
    })();
    match machine {
        Ok(machine) => Ok(RuntimeConfiguration::recording_with_network(
            machine, recorder, network,
        )),
        Err(build) => match recorder.discard_unstarted() {
            Ok(()) => Err(build),
            Err(source) => Err(RecordingBuildError::Discard {
                build: Box::new(build),
                source,
            }),
        },
    }
}

fn prepare_requirement(
    id: &ResourceId,
    requirement: &ResourceRequirement,
    recorder: &Recorder,
    resources: &mut BTreeMap<ResourceId, RecordedResource>,
) -> Result<PreparedResource, HostResourceError> {
    let path = &requirement.path;
    let (prepared, identity) = match requirement.kind {
        ResourceKind::Bytes => {
            let bytes = fs::read(path).map_err(|source| HostResourceError::ReadBytes {
                path: path.clone(),
                source,
            })?;
            let identity = MediaIdentity::from_bytes(path, &bytes);
            (PreparedResource::Bytes(bytes), identity)
        }
        ResourceKind::Storage { access } => {
            let mut base = HostFileStorage::open_read_only(path).map_err(|source| {
                HostResourceError::OpenStorage {
                    path: path.clone(),
                    access: StorageAccess::ReadOnly,
                    source,
                }
            })?;
            let identity = MediaIdentity::from_storage(path, &mut base).map_err(|source| {
                HostResourceError::HashStorage {
                    path: path.clone(),
                    source,
                }
            })?;
            let medium: Box<dyn se_core::storage::StorageMedium> = match access {
                StorageAccess::ReadOnly => Box::new(base),
                StorageAccess::ReadWrite => Box::new(RecordingStorageMedium::new(
                    base,
                    recorder.storage(id.clone()),
                )),
            };
            (PreparedResource::Storage { access, medium }, identity)
        }
    };
    resources.insert(
        id.clone(),
        RecordedResource {
            kind: requirement.kind,
            identity,
        },
    );
    Ok(prepared)
}
