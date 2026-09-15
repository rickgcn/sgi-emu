//! End-to-end cold Recording and Replay composition tests.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use se_config::definition::MachineDefinition;
use se_config::draft::{Edit, MachineDraft};
use se_config::id::{DeviceKindId, NodeId, PropertyId};
use se_config::value::PropertyValue;
use se_machine::indigo::ip12::definition::Ip12Definition;
use se_machine::resource::ResourceId;
use se_network::config::NatConfig;
use se_runtime::control::RuntimeMode;
use se_runtime::record::Replayer;
use se_runtime::runtime::Runtime;
use se_session::{recording, replay};

const PROM_BYTES: usize = 0x40000;
static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

struct TemporaryFiles {
    directory: PathBuf,
}

impl TemporaryFiles {
    fn new(name: &str) -> Self {
        let id = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let directory = std::env::temp_dir().join(format!(
            "sgi-emu-record-replay-{name}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir(&directory).unwrap();
        Self { directory }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.directory.join(name)
    }

    fn write(&self, name: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}

impl Drop for TemporaryFiles {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.directory).unwrap();
    }
}

fn set_path(draft: &mut MachineDraft, property: impl Into<String>, path: &Path) {
    draft.apply(Edit::SetProperty {
        property: PropertyId(property.into()),
        value: PropertyValue::Text(path.to_string_lossy().into_owned()),
    });
}

fn attach(draft: &mut MachineDraft, target: u8, lun: u8, kind: &str, path: &Path) {
    draft.apply(Edit::SetAttachment {
        slot: NodeId(format!("scsi.0.target.{target}.lun.{lun}")),
        device: Some(DeviceKindId(kind.into())),
    });
    set_path(
        draft,
        format!("scsi.0.target.{target}.lun.{lun}.medium-path"),
        path,
    );
}

fn set_vt100(draft: &mut MachineDraft, port: &str, attached: bool) {
    draft.apply(Edit::SetAttachment {
        slot: NodeId(String::from(port)),
        device: attached.then(|| DeviceKindId(String::from("terminal.vt100"))),
    });
}

fn multi_scsi_draft(files: &TemporaryFiles) -> MachineDraft {
    let prom = files.write("prom.bin", vec![0; PROM_BYTES]);
    let mut draft = Ip12Definition.default_draft();
    set_path(&mut draft, "firmware.0.image-path", &prom);
    for (target, lun, kind, name, size) in [
        (2, 0, "scsi.disk", "disk-a.img", 512),
        (3, 5, "scsi.disk", "disk-b.img", 1024),
        (5, 1, "scsi.disk", "disk-c.img", 1536),
        (6, 4, "scsi.cdrom", "disc.iso", 2048),
    ] {
        let path = files.write(name, vec![target; size]);
        attach(&mut draft, target, lun, kind, &path);
    }
    draft
}

fn complete_record(draft: MachineDraft, path: PathBuf) {
    let (configuration, _) = recording::build_configuration(draft, NatConfig::default(), path)
        .unwrap()
        .into_parts();
    let runtime = Runtime::new_unconfigured().unwrap();
    runtime.configure_with(configuration).unwrap();
    assert_eq!(runtime.step().unwrap().mode, RuntimeMode::Recording);
    assert_eq!(
        runtime.stop_recording().unwrap().mode,
        RuntimeMode::RecordCompleted
    );
    runtime.shutdown().unwrap();
}

#[test]
fn failed_recording_build_discards_partial_and_can_retry() {
    let files = TemporaryFiles::new("record-retry");
    let mut draft = multi_scsi_draft(&files);
    let missing_medium = files.path("missing.iso");
    set_path(
        &mut draft,
        "scsi.0.target.6.lun.4.medium-path",
        &missing_medium,
    );
    let record_path = files.path("retry.serec");
    let partial_path = files.path("retry.serec.partial");

    let error = match recording::build_configuration(
        draft.clone(),
        NatConfig::default(),
        record_path.clone(),
    ) {
        Ok(_) => panic!("missing medium must fail resource preparation"),
        Err(error) => error,
    };
    assert!(matches!(error, recording::RecordingBuildError::Prepare(_)));
    assert!(!partial_path.exists());

    fs::write(missing_medium, vec![0; 2048]).unwrap();
    complete_record(draft, record_path.clone());
    assert!(record_path.exists());
}

#[test]
fn multi_scsi_record_replays_recorded_topology_and_relocated_resource() {
    let files = TemporaryFiles::new("multi-scsi");
    let draft = multi_scsi_draft(&files);
    let path = files.path("multi.serec");
    complete_record(draft.clone(), path.clone());

    let recorded = Replayer::open(&path).unwrap();
    assert_eq!(recorded.manifest().machine(), &draft);
    assert_eq!(recorded.manifest().resources().len(), 5);
    for role in [
        "scsi.0.target.2.lun.0.medium",
        "scsi.0.target.3.lun.5.medium",
        "scsi.0.target.5.lun.1.medium",
        "scsi.0.target.6.lun.4.medium",
    ] {
        assert!(
            recorded
                .manifest()
                .resources()
                .contains_key(&ResourceId::new(role))
        );
    }
    drop(recorded);

    let relocated = files.path("moved-a.img");
    fs::rename(files.path("disk-a.img"), &relocated).unwrap();
    let mut current = draft.clone();
    set_path(
        &mut current,
        "scsi.0.target.2.lun.0.medium-path",
        &relocated,
    );
    current.apply(Edit::SetProperty {
        property: PropertyId(String::from("memory.bank.a.simm-mib")),
        value: PropertyValue::Integer(8),
    });
    current.apply(Edit::SetAttachment {
        slot: NodeId(String::from("scsi.0.target.5.lun.1")),
        device: None,
    });
    let (configuration, _) = replay::build_configuration(current, path, None)
        .unwrap()
        .into_parts();
    let runtime = Runtime::new_unconfigured().unwrap();
    runtime.configure_with(configuration).unwrap();
    assert_eq!(runtime.step().unwrap().mode, RuntimeMode::ReplayCompleted);
    runtime.shutdown().unwrap();
}

#[test]
fn changed_relocated_resource_fails_before_replay_execution() {
    let files = TemporaryFiles::new("identity");
    let draft = multi_scsi_draft(&files);
    let path = files.path("identity.serec");
    complete_record(draft.clone(), path.clone());

    let relocated = files.path("changed-a.img");
    fs::rename(files.path("disk-a.img"), &relocated).unwrap();
    fs::write(&relocated, vec![0xff; 512]).unwrap();
    let mut current = draft;
    set_path(
        &mut current,
        "scsi.0.target.2.lun.0.medium-path",
        &relocated,
    );
    let error = match replay::build_configuration(current, path, None) {
        Ok(_) => panic!("changed content must not enter the runtime"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("scsi.0.target.2.lun.0.medium"));
    assert!(error.to_string().contains("content mismatch"));
}

#[test]
fn invalid_current_draft_does_not_override_recorded_machine() {
    let files = TemporaryFiles::new("invalid-current");
    let draft = multi_scsi_draft(&files);
    let path = files.path("record.serec");
    complete_record(draft.clone(), path.clone());
    let mut current = draft;
    current
        .properties
        .remove(&PropertyId(String::from("firmware.0.image-path")));
    current.apply(Edit::SetAttachment {
        slot: NodeId(String::from("scsi.0.target.2.lun.0")),
        device: None,
    });
    let (configuration, _) = replay::build_configuration(current, path, None)
        .unwrap()
        .into_parts();
    let runtime = Runtime::new_unconfigured().unwrap();
    runtime.configure_with(configuration).unwrap();
    assert_eq!(runtime.step().unwrap().mode, RuntimeMode::ReplayCompleted);
    runtime.shutdown().unwrap();
}

#[test]
fn current_resource_with_same_id_but_wrong_kind_is_not_a_replay_hint() {
    let files = TemporaryFiles::new("wrong-kind-hint");
    let draft = multi_scsi_draft(&files);
    let path = files.path("record.serec");
    complete_record(draft.clone(), path.clone());
    let different_medium = files.write("current-disc.iso", vec![0xff; 2048]);
    let mut current = draft;
    attach(&mut current, 2, 0, "scsi.cdrom", &different_medium);
    let (configuration, _) = replay::build_configuration(current, path, None)
        .unwrap()
        .into_parts();
    let runtime = Runtime::new_unconfigured().unwrap();
    runtime.configure_with(configuration).unwrap();
    assert_eq!(runtime.step().unwrap().mode, RuntimeMode::ReplayCompleted);
    runtime.shutdown().unwrap();
}

#[test]
fn recording_and_replay_frontend_plans_follow_the_recorded_machine() {
    let files = TemporaryFiles::new("frontend-authority");
    let mut recorded_draft = multi_scsi_draft(&files);
    set_vt100(&mut recorded_draft, "serial.1.channel.b.port", false);
    let path = files.path("frontend.serec");

    let (recording_configuration, recording_frontend) =
        recording::build_configuration(recorded_draft.clone(), NatConfig::default(), path.clone())
            .unwrap()
            .into_parts();
    assert_eq!(
        recording_frontend
            .serial_console_endpoints()
            .iter()
            .map(|endpoint| endpoint.as_str())
            .collect::<Vec<_>>(),
        ["serial.external.a"]
    );
    let recording_runtime = Runtime::new_unconfigured().unwrap();
    recording_runtime
        .configure_with(recording_configuration)
        .unwrap();
    recording_runtime.step().unwrap();
    recording_runtime.stop_recording().unwrap();
    recording_runtime.shutdown().unwrap();

    let mut current_draft = recorded_draft;
    set_vt100(&mut current_draft, "serial.1.channel.a.port", false);
    set_vt100(&mut current_draft, "serial.1.channel.b.port", true);
    let (replay_configuration, replay_frontend) =
        replay::build_configuration(current_draft, path, None)
            .unwrap()
            .into_parts();
    assert_eq!(
        replay_frontend
            .serial_console_endpoints()
            .iter()
            .map(|endpoint| endpoint.as_str())
            .collect::<Vec<_>>(),
        ["serial.external.a"]
    );
    let replay_runtime = Runtime::new_unconfigured().unwrap();
    replay_runtime.configure_with(replay_configuration).unwrap();
    assert_eq!(
        replay_runtime.step().unwrap().mode,
        RuntimeMode::ReplayCompleted
    );
    replay_runtime.shutdown().unwrap();
}
