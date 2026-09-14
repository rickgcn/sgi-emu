//! Read-only host backing with a resource-specific Replay overlay.

use std::io;

use se_core::storage::StorageMedium;
use se_runtime::record::ReplayStorage;

use crate::file_storage::HostFileStorage;

pub(crate) struct ReplayStorageMedium {
    base: HostFileStorage,
    overlay: ReplayStorage,
}

impl ReplayStorageMedium {
    pub(crate) fn new(base: HostFileStorage, overlay: ReplayStorage) -> Self {
        Self { base, overlay }
    }
}

impl StorageMedium for ReplayStorageMedium {
    fn size_bytes(&self) -> u64 {
        self.base.size_bytes()
    }

    fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
        let result = self
            .base
            .read_exact_at(offset, buffer)
            .and_then(|()| self.overlay.overlay_read(offset, buffer));
        if let Err(error) = &result {
            self.overlay.report_storage_error(error);
        }
        result
    }

    fn write_all_at(&mut self, offset: u64, data: &[u8]) -> io::Result<()> {
        let result =
            self.overlay
                .write_all_at(offset, data, self.base.size_bytes(), |page_offset, page| {
                    self.base.read_exact_at(page_offset, page)
                });
        if let Err(error) = &result {
            self.overlay.report_storage_error(error);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use se_config::definition::MachineDefinition;
    use se_config::draft::Edit;
    use se_config::id::{DeviceKindId, NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_core::storage::StorageMedium;
    use se_machine::indigo::ip12::definition::Ip12Definition;
    use se_machine::resource::ResourceId;
    use se_network::config::NatConfig;
    use se_runtime::record::{Recorder, Replayer};
    use se_runtime::runtime::Runtime;

    use super::ReplayStorageMedium;
    use crate::file_storage::HostFileStorage;
    use crate::recording;
    use crate::recording_storage::RecordingStorageMedium;

    static NEXT_ID: AtomicU64 = AtomicU64::new(0);

    struct Files(PathBuf);

    impl Files {
        fn new() -> Self {
            let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir()
                .join(format!("sgi-emu-session-cow-{}-{id}", std::process::id()));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for Files {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    #[test]
    fn recording_wrappers_keep_two_host_images_read_only() {
        let files = Files::new();
        let base_a = vec![1; 4096];
        let base_b = vec![2; 4096];
        let path_a = files.write("a.img", &base_a);
        let path_b = files.write("b.img", &base_b);
        let recorder = Recorder::create(files.0.join("record.serec")).unwrap();
        let mut a = RecordingStorageMedium::new(
            HostFileStorage::open_read_only(&path_a).unwrap(),
            recorder.storage(ResourceId::new("a")),
        );
        let mut b = RecordingStorageMedium::new(
            HostFileStorage::open_read_only(&path_b).unwrap(),
            recorder.storage(ResourceId::new("b")),
        );
        a.write_all_at(7, &[0xa1]).unwrap();
        b.write_all_at(7, &[0xb2]).unwrap();
        let mut visible_a = vec![0; 4096];
        let mut visible_b = vec![0; 4096];
        a.read_exact_at(0, &mut visible_a).unwrap();
        b.read_exact_at(0, &mut visible_b).unwrap();
        assert_eq!(visible_a[7], 0xa1);
        assert_eq!(visible_b[7], 0xb2);
        assert_eq!(fs::read(path_a).unwrap(), base_a);
        assert_eq!(fs::read(path_b).unwrap(), base_b);
    }

    #[test]
    fn replay_wrappers_keep_two_host_images_read_only() {
        let files = Files::new();
        let base_a = vec![1; 4096];
        let base_b = vec![2; 4096];
        let path_a = files.write("a.img", &base_a);
        let path_b = files.write("b.img", &base_b);
        let prom = files.write("prom.bin", &vec![0; 0x40000]);
        let mut draft = Ip12Definition.default_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("firmware.0.image-path")),
            value: PropertyValue::Text(prom.to_string_lossy().into_owned()),
        });
        for (target, path) in [(2, &path_a), (3, &path_b)] {
            draft.apply(Edit::SetAttachment {
                slot: NodeId(format!("scsi.0.target.{target}.lun.0")),
                device: Some(DeviceKindId(String::from("scsi.disk"))),
            });
            draft.apply(Edit::SetProperty {
                property: PropertyId(format!("scsi.0.target.{target}.lun.0.medium-path")),
                value: PropertyValue::Text(path.to_string_lossy().into_owned()),
            });
        }
        let record_path = files.0.join("record.serec");
        let configuration =
            recording::build_configuration(draft, NatConfig::default(), record_path.clone())
                .unwrap();
        let runtime = Runtime::new_unconfigured().unwrap();
        runtime.configure_with(configuration).unwrap();
        runtime.step().unwrap();
        runtime.stop_recording().unwrap();
        runtime.shutdown().unwrap();

        let replayer = Replayer::open(record_path).unwrap();
        let mut a = ReplayStorageMedium::new(
            HostFileStorage::open_read_only(&path_a).unwrap(),
            replayer.storage(ResourceId::new("scsi.0.target.2.lun.0.medium")),
        );
        let mut b = ReplayStorageMedium::new(
            HostFileStorage::open_read_only(&path_b).unwrap(),
            replayer.storage(ResourceId::new("scsi.0.target.3.lun.0.medium")),
        );
        a.write_all_at(7, &[0xa1]).unwrap();
        b.write_all_at(7, &[0xb2]).unwrap();
        let mut visible_a = vec![0; 4096];
        let mut visible_b = vec![0; 4096];
        a.read_exact_at(0, &mut visible_a).unwrap();
        b.read_exact_at(0, &mut visible_b).unwrap();
        assert_eq!(visible_a[7], 0xa1);
        assert_eq!(visible_b[7], 0xb2);
        assert_eq!(fs::read(path_a).unwrap(), base_a);
        assert_eq!(fs::read(path_b).unwrap(), base_b);
    }
}
