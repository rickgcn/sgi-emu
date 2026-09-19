//! Host-path acquisition of live machine media.
//!
//! This module is the only owner of the host-file side of a live media change.
//! It turns a frontend-supplied path into an owned storage capability, hands
//! that capability to the runtime, and keeps no record of the path afterwards.

use std::error::Error;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use se_core::storage::StorageMedium;
use se_machine::media::MediaKind;
use se_runtime::control::RuntimeStatus;
use se_runtime::media::RuntimeMediaHandle;
use se_runtime::runtime::{RuntimeError, RuntimeHandle};

use crate::file_storage::HostFileStorage;

/// A live medium could not be opened or installed.
#[derive(Debug)]
pub enum SessionMediaError {
    /// The host medium could not be opened with the required access.
    OpenMedium {
        /// The host path supplied by the frontend.
        path: PathBuf,
        /// The underlying host I/O error.
        source: io::Error,
    },
    /// The runtime rejected the prepared medium, the handle, or the mode.
    Runtime(RuntimeError),
}

impl fmt::Display for SessionMediaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenMedium { path, source } => {
                write!(formatter, "failed to open '{}': {source}", path.display())
            }
            Self::Runtime(error) => error.fmt(formatter),
        }
    }
}

impl Error for SessionMediaError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::OpenMedium { source, .. } => Some(source),
            Self::Runtime(error) => Some(error),
        }
    }
}

/// Opens one host file as a live medium and installs it in the addressed slot.
///
/// The session owns the complete host-resource path: it opens the file with the
/// access the medium family requires, transfers the capability to the runtime,
/// and forgets the path. Contents and capacity are not validated here, because
/// the device behind the addressed slot owns that contract.
///
/// A failed open leaves the slot untouched, and a runtime rejection drops the
/// capability instead of installing a partial medium.
///
/// # Errors
///
/// Returns [`SessionMediaError::OpenMedium`] when the host file cannot be
/// opened, or [`SessionMediaError::Runtime`] when the runtime rejects the
/// medium, the handle, or the current runtime mode.
pub fn insert_media_from_path(
    runtime: &RuntimeHandle,
    handle: RuntimeMediaHandle,
    kind: MediaKind,
    path: impl AsRef<Path>,
) -> Result<RuntimeStatus, SessionMediaError> {
    let path = path.as_ref();
    let medium: Box<dyn StorageMedium> = match kind {
        // A read-only medium family never receives host write access.
        MediaKind::OpticalDisc => {
            Box::new(HostFileStorage::open_read_only(path).map_err(|source| {
                SessionMediaError::OpenMedium {
                    path: path.to_path_buf(),
                    source,
                }
            })?)
        }
    };
    runtime
        .insert_media(handle, medium)
        .map_err(SessionMediaError::Runtime)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use se_config::draft::{Edit, MachineDraft};
    use se_config::id::{DeviceKindId, NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_machine::media::MediaKind;
    use se_network::config::NatConfig;
    use se_runtime::media::{RuntimeMediaError, RuntimeMediaHandle};
    use se_runtime::runtime::{Runtime, RuntimeError};

    use super::{SessionMediaError, insert_media_from_path};
    use crate::normal::build_configuration;

    static NEXT_MEDIUM_ID: AtomicU64 = AtomicU64::new(0);

    struct TemporaryFile {
        path: PathBuf,
    }

    impl TemporaryFile {
        fn new(name: &str, bytes: usize) -> Self {
            let id = NEXT_MEDIUM_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "sgi-emu-session-media-{name}-{}-{id}.img",
                std::process::id()
            ));
            fs::write(&path, vec![0; bytes]).unwrap();
            Self { path }
        }
    }

    impl Drop for TemporaryFile {
        fn drop(&mut self) {
            fs::remove_file(&self.path).unwrap();
        }
    }

    /// A Normal runtime holding one empty CD-ROM drive at target 4.
    struct EmptyCdromMachine {
        runtime: Runtime,
        draft: MachineDraft,
        /// Kept alive for the whole test so every rebuild can read the PROM.
        _firmware: TemporaryFile,
    }

    impl EmptyCdromMachine {
        fn new() -> Self {
            let firmware = TemporaryFile::new("prom", 0x40000);
            let mut draft = crate::machine::default_machine_draft();
            draft.apply(Edit::SetProperty {
                property: PropertyId(String::from("firmware.0.image-path")),
                value: PropertyValue::Text(firmware.path.to_string_lossy().into_owned()),
            });
            draft.apply(Edit::SetAttachment {
                slot: NodeId(String::from("scsi.0.target.4.lun.0")),
                device: Some(DeviceKindId(String::from("scsi.cdrom"))),
            });
            let mut machine = Self {
                runtime: Runtime::new_unconfigured().unwrap(),
                draft,
                _firmware: firmware,
            };
            machine.install();
            machine
        }

        /// Cold-constructs and installs a fresh machine instance.
        fn install(&mut self) {
            let build = build_configuration(self.draft.clone(), NatConfig::default())
                .expect("an empty CD-ROM drive is a complete machine");
            self.runtime
                .configure_with(build.into_parts().0)
                .expect("the runtime accepts its own configuration");
        }

        fn handle(&self) -> RuntimeMediaHandle {
            self.runtime.media_catalog().unwrap().slots()[0]
                .handle()
                .clone()
        }

        fn medium_present(&self) -> bool {
            self.runtime.media_catalog().unwrap().slots()[0]
                .state()
                .medium_present()
        }
    }

    #[test]
    fn an_existing_host_file_loads_an_empty_slot() {
        let machine = EmptyCdromMachine::new();
        assert_eq!(
            machine.runtime.media_catalog().unwrap().slots().len(),
            1,
            "the empty drive stays in the machine topology"
        );
        assert!(!machine.medium_present());
        let medium = TemporaryFile::new("valid", 2048);

        insert_media_from_path(
            &machine.runtime.handle(),
            machine.handle(),
            MediaKind::OpticalDisc,
            &medium.path,
        )
        .expect("a readable image is a valid host medium");

        let catalog = machine.runtime.media_catalog().unwrap();
        let state = catalog.slots()[0].state();
        assert!(state.medium_present());
        assert_eq!(state.medium_size_bytes(), Some(2048));
        machine.runtime.shutdown().unwrap();
    }

    #[test]
    fn a_missing_host_file_reports_its_path_and_keeps_the_slot_empty() {
        let machine = EmptyCdromMachine::new();
        let missing = std::env::temp_dir().join("sgi-emu-session-media-absent.iso");

        let error = insert_media_from_path(
            &machine.runtime.handle(),
            machine.handle(),
            MediaKind::OpticalDisc,
            &missing,
        )
        .expect_err("an absent host file cannot become a medium");
        assert!(matches!(
            &error,
            SessionMediaError::OpenMedium { path, source }
                if path == &missing && source.kind() == io::ErrorKind::NotFound
        ));
        assert!(
            error
                .to_string()
                .contains("sgi-emu-session-media-absent.iso"),
            "the message must name the host path that failed"
        );
        assert!(!machine.medium_present());
        machine.runtime.shutdown().unwrap();
    }

    #[test]
    fn an_unaligned_host_file_is_rejected_by_the_device_contract() {
        let machine = EmptyCdromMachine::new();
        let medium = TemporaryFile::new("unaligned", 1000);

        let error = insert_media_from_path(
            &machine.runtime.handle(),
            machine.handle(),
            MediaKind::OpticalDisc,
            &medium.path,
        )
        .expect_err("the device rejects a capacity it cannot address");
        assert!(matches!(
            error,
            SessionMediaError::Runtime(RuntimeError::Media(RuntimeMediaError::InvalidMedium))
        ));
        assert!(!machine.medium_present());
        machine.runtime.shutdown().unwrap();
    }

    #[test]
    fn a_stale_handle_never_reaches_a_replaced_machine() {
        let mut machine = EmptyCdromMachine::new();
        let stale = machine.handle();
        machine.install();
        let medium = TemporaryFile::new("stale", 2048);

        let error = insert_media_from_path(
            &machine.runtime.handle(),
            stale,
            MediaKind::OpticalDisc,
            &medium.path,
        )
        .expect_err("a handle from a replaced machine cannot address the new one");
        assert!(matches!(
            error,
            SessionMediaError::Runtime(RuntimeError::Media(RuntimeMediaError::StaleHandle))
        ));
        assert!(!machine.medium_present());
        machine.runtime.shutdown().unwrap();
    }

    #[test]
    fn a_rejected_medium_leaves_the_slot_ready_for_another_attempt() {
        let machine = EmptyCdromMachine::new();
        let unaligned = TemporaryFile::new("rejected", 1000);
        let valid = TemporaryFile::new("accepted", 4096);

        insert_media_from_path(
            &machine.runtime.handle(),
            machine.handle(),
            MediaKind::OpticalDisc,
            &unaligned.path,
        )
        .expect_err("an unaligned image is not a medium");
        insert_media_from_path(
            &machine.runtime.handle(),
            machine.handle(),
            MediaKind::OpticalDisc,
            &valid.path,
        )
        .expect("the rejected attempt left the slot empty");

        assert_eq!(
            machine.runtime.media_catalog().unwrap().slots()[0]
                .state()
                .medium_size_bytes(),
            Some(4096)
        );
        machine.runtime.shutdown().unwrap();
    }
}
