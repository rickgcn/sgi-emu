//! Application entry point.

mod config;
mod persistence;
mod storage;

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use se_config::draft::MachineDraft;
use se_config::id::{NodeId, PropertyId};
use se_config::value::PropertyValue;
use se_core::storage::StorageMedium;
use se_device::gio::{GioBus, GioSlot};
use se_device::lg1::Lg1;
use se_machine::indigo::GraphicsBoard;
use se_machine::indigo::ip12::Ip12;
use se_machine::indigo::ip12::builder;
use se_machine::indigo::ip12::definition::Ip12Definition;
use se_machine::indigo::ip12::plan::{Ip12BuildPlan, ScsiDevice};
use se_machine::machine::{Machine, MachineStartupConfiguration};
use se_machine::resource::ResourceRequirement;
use se_runtime::record::{MediaIdentity, RecordManifest, Recorder, Replayer};
use se_runtime::runtime::{Runtime, RuntimeConfiguration};
use se_session::normal::prepare_ip12;
use se_ui::bridge::ffi::NetworkConfiguration;
use se_ui::session::{LegacyBuildRequest, UiSession};
use std::sync::Arc;

fn main() -> Result<(), Box<dyn Error>> {
    let config_path = config::config_path()?;
    let mut application_config = config::load(&config_path)?;
    let machine_draft = application_config.machine_draft();
    let network = application_config.network_configuration();
    let runtime = Runtime::new_unconfigured()?;
    let startup_error = if firmware_unconfigured(&machine_draft) {
        String::new()
    } else {
        build_normal_configuration(machine_draft.clone(), &network)
            .and_then(|configuration| {
                runtime
                    .configure_with(configuration)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            })
            .err()
            .unwrap_or_default()
    };

    let startup = application_config.ui_startup_state(startup_error);
    let session = UiSession::new(
        runtime,
        machine_draft,
        Arc::new(Ip12Definition),
        Box::new(build_normal_configuration),
        Box::new(build_legacy_configuration),
        Box::new(|configuration| config::parse_network_configuration(configuration).map(|_| ())),
    );
    let exit = session.run(&startup);
    let committed = session.machine_draft_snapshot();
    if let Some(state) = session.shutdown()? {
        persistence::save(&state)?;
    }

    application_config
        .apply_ui_exit_state(exit)
        .map_err(io::Error::other)?;
    application_config.set_machine_draft(committed);
    config::save(&config_path, &application_config)?;

    Ok(())
}

fn firmware_path(draft: &MachineDraft) -> &str {
    match draft
        .properties
        .get(&PropertyId(String::from("firmware.0.image-path")))
    {
        Some(PropertyValue::Text(path)) => path,
        _ => "",
    }
}

fn firmware_unconfigured(draft: &MachineDraft) -> bool {
    matches!(
        draft
            .properties
            .get(&PropertyId(String::from("firmware.0.image-path"))),
        Some(PropertyValue::Text(path)) if path.is_empty()
    )
}

fn build_normal_configuration(
    draft: MachineDraft,
    network: &NetworkConfiguration,
) -> Result<RuntimeConfiguration, String> {
    let network = config::parse_network_configuration(network)?;
    let plan = Ip12Definition.compile(&draft).map_err(|error| {
        error
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let prepared = prepare_ip12(plan).map_err(|error| error.to_string())?;
    let mut machine =
        Machine::IndigoIp12(builder::build(prepared).map_err(|error| error.to_string())?);
    restore_persisted_state(&mut machine, &draft.model.0)?;
    Ok(RuntimeConfiguration::normal_with_network(machine, network))
}

fn build_legacy_configuration(
    draft: MachineDraft,
    request: LegacyBuildRequest,
    network: Option<&NetworkConfiguration>,
) -> Result<RuntimeConfiguration, String> {
    match request {
        LegacyBuildRequest::Recording(path) => {
            let network = network.ok_or("Recording network settings are unavailable")?;
            build_recording_configuration(&legacy_recording_projection(&draft)?, network, path)
        }
        LegacyBuildRequest::Replaying { path, snapshot_id } => {
            build_replay_configuration(&draft, path, snapshot_id.as_deref())
        }
    }
}

struct LegacyRecordingProjection {
    machine_model: String,
    startup: MachineStartupConfiguration,
    prom_path: PathBuf,
    disk_path: Option<PathBuf>,
    cdrom_path: Option<PathBuf>,
}

fn legacy_recording_projection(draft: &MachineDraft) -> Result<LegacyRecordingProjection, String> {
    let plan = Ip12Definition.compile(draft).map_err(|error| {
        error
            .diagnostics()
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ")
    })?;
    let mut disk_path = None;
    let mut cdrom_path = None;
    for attachment in plan.scsi() {
        let path = required_path(&plan, &attachment.medium)?;
        match (attachment.target, attachment.lun, attachment.device) {
            (1, 0, ScsiDevice::Disk) => disk_path = Some(path),
            (4, 0, ScsiDevice::Cdrom) => cdrom_path = Some(path),
            _ => {
                return Err(String::from(
                    "current Record format does not support this machine topology",
                ));
            }
        }
    }
    let graphics = if plan.gio().is_empty() {
        None
    } else {
        Some(GraphicsBoard::Lg1)
    };
    Ok(LegacyRecordingProjection {
        machine_model: draft.model.0.clone(),
        startup: MachineStartupConfiguration::IndigoIp12 {
            floating_point_backend: plan.floating_point_backend(),
            memory: *plan.memory(),
            graphics,
        },
        prom_path: required_path(&plan, plan.firmware())?,
        disk_path,
        cdrom_path,
    })
}

fn required_path(
    plan: &Ip12BuildPlan,
    id: &se_machine::resource::ResourceId,
) -> Result<PathBuf, String> {
    plan.resources()
        .get(id)
        .map(|ResourceRequirement { path, .. }| path.clone())
        .ok_or_else(|| format!("missing resource requirement {}", id.as_str()))
}

fn build_recording_configuration(
    configuration: &LegacyRecordingProjection,
    network: &NetworkConfiguration,
    path: PathBuf,
) -> Result<RuntimeConfiguration, String> {
    let network = config::parse_network_configuration(network)?;
    let startup_configuration = configuration.startup;
    let prom_path = configuration.prom_path.as_path();
    let raw_prom = read_prom(prom_path)?;
    let prom_identity = MediaIdentity::from_bytes(prom_path, &raw_prom);
    let mut disk = open_optional_storage(configuration.disk_path.as_deref())?;
    let mut cdrom = open_optional_storage(configuration.cdrom_path.as_deref())?;
    let disk_identity = identity_for_storage(&mut disk, configuration.disk_path.as_deref())?;
    let cdrom_identity = identity_for_storage(&mut cdrom, configuration.cdrom_path.as_deref())?;
    let recorder = Recorder::create_or_replace(path).map_err(|error| error.to_string())?;
    let disk = disk.map(|storage| storage.recording(recorder.disk()).boxed());
    let cdrom = cdrom.map(storage::FileBlockStorage::boxed);
    let mut machine = build_machine_from_parts(startup_configuration, raw_prom, disk, cdrom)?;
    restore_persisted_state(&mut machine, &configuration.machine_model)?;
    let nonvolatile_state = machine.nonvolatile_state();
    let manifest = RecordManifest::new(
        startup_configuration,
        prom_identity,
        disk_identity,
        cdrom_identity,
        nonvolatile_state,
    );
    recorder
        .start(&manifest)
        .map_err(|error| error.to_string())?;
    Ok(RuntimeConfiguration::recording_with_network(
        machine, recorder, network,
    ))
}

fn build_replay_configuration(
    draft: &MachineDraft,
    path: PathBuf,
    snapshot_id: Option<&str>,
) -> Result<RuntimeConfiguration, String> {
    let replayer = match snapshot_id {
        Some(snapshot_id) => {
            Replayer::open_snapshot(path, snapshot_id).map_err(|error| error.to_string())?
        }
        None => Replayer::open(path).map_err(|error| error.to_string())?,
    };
    let manifest = replayer.manifest().clone();
    let startup_configuration = *manifest.machine();
    let nonvolatile_state = manifest.nonvolatile_state().clone();
    let prom_path = selected_or_hint(firmware_path(draft), &manifest.prom().path_hint);
    let raw_prom = read_prom(&prom_path)?;
    ensure_identity(
        "PROM",
        manifest.prom(),
        &MediaIdentity::from_bytes(&prom_path, &raw_prom),
    )?;

    let disk = match manifest.disk() {
        None => None,
        Some(expected) => {
            let path = selected_or_hint(
                legacy_medium_hint(draft, 1, 0, "scsi.disk"),
                &expected.path_hint,
            );
            let mut storage =
                storage::FileBlockStorage::open_read_only(&path).map_err(|error| {
                    format!("failed to open Replay disk '{}': {error}", path.display())
                })?;
            let identity = storage.identity(&path).map_err(|error| {
                format!(
                    "failed to validate Replay disk '{}': {error}",
                    path.display()
                )
            })?;
            ensure_identity("disk", expected, &identity)?;
            Some(storage.replay(replayer.disk()).boxed())
        }
    };
    let cdrom = match manifest.cdrom() {
        None => None,
        Some(expected) => {
            let path = selected_or_hint(
                legacy_medium_hint(draft, 4, 0, "scsi.cdrom"),
                &expected.path_hint,
            );
            let mut storage =
                storage::FileBlockStorage::open_read_only(&path).map_err(|error| {
                    format!("failed to open Replay CD-ROM '{}': {error}", path.display())
                })?;
            let identity = storage.identity(&path).map_err(|error| {
                format!(
                    "failed to validate Replay CD-ROM '{}': {error}",
                    path.display()
                )
            })?;
            ensure_identity("CD-ROM", expected, &identity)?;
            Some(storage.boxed())
        }
    };
    let mut machine = build_machine_from_parts(startup_configuration, raw_prom, disk, cdrom)?;
    machine.restore_nonvolatile_state(nonvolatile_state, 0);
    Ok(RuntimeConfiguration::replaying(machine, replayer))
}

fn build_machine_from_parts(
    startup_configuration: MachineStartupConfiguration,
    raw_prom: Vec<u8>,
    disk: Option<Box<dyn StorageMedium>>,
    cdrom: Option<Box<dyn StorageMedium>>,
) -> Result<Machine, String> {
    match startup_configuration {
        MachineStartupConfiguration::IndigoIp12 {
            floating_point_backend,
            memory,
            graphics,
        } => {
            let mut gio = GioBus::new();
            if let Some(GraphicsBoard::Lg1) = graphics {
                gio.attach(GioSlot::Graphics, Box::new(Lg1::new()))
                    .map_err(|error| error.to_string())?;
            }
            Ip12::new_with_memory(raw_prom, floating_point_backend, memory, gio, disk, cdrom)
                .map(Machine::IndigoIp12)
                .map_err(|error| error.to_string())
        }
    }
}

fn legacy_medium_hint<'a>(draft: &'a MachineDraft, target: u8, lun: u8, device: &str) -> &'a str {
    let slot = NodeId(format!("scsi.0.target.{target}.lun.{lun}"));
    if draft
        .attachments
        .get(&slot)
        .is_none_or(|attached| attached.0 != device)
    {
        return "";
    }
    let id = PropertyId(format!("scsi.0.target.{target}.lun.{lun}.medium-path"));
    match draft.properties.get(&id) {
        Some(PropertyValue::Text(path)) => path,
        _ => "",
    }
}

fn read_prom(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path)
        .map_err(|error| format!("failed to read PROM image '{}': {error}", path.display()))
}

fn open_optional_storage(path: Option<&Path>) -> Result<Option<storage::FileBlockStorage>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    storage::FileBlockStorage::open_read_only(path)
        .map(Some)
        .map_err(|error| format!("failed to open storage image '{}': {error}", path.display()))
}

fn identity_for_storage(
    storage: &mut Option<storage::FileBlockStorage>,
    path: Option<&Path>,
) -> Result<Option<MediaIdentity>, String> {
    match (storage.as_mut(), path) {
        (Some(storage), Some(path)) => storage
            .identity(path)
            .map(Some)
            .map_err(|error| format!("failed to hash storage image '{}': {error}", path.display())),
        (None, None) => Ok(None),
        _ => Err(String::from("storage path and file presence disagree")),
    }
}

fn restore_persisted_state(machine: &mut Machine, machine_model: &str) -> Result<(), String> {
    if let Some(restored) = persistence::load(machine_model).map_err(|error| error.to_string())? {
        machine.restore_nonvolatile_state(restored.state, restored.offline_milliseconds);
    }
    Ok(())
}

fn ensure_identity(
    name: &str,
    expected: &MediaIdentity,
    actual: &MediaIdentity,
) -> Result<(), String> {
    if expected.size_bytes == actual.size_bytes && expected.sha256 == actual.sha256 {
        Ok(())
    } else {
        Err(format!(
            "Replay {name} content mismatch for '{}'",
            actual.path_hint
        ))
    }
}

fn selected_or_hint(selected: &str, hint: &str) -> PathBuf {
    if selected.is_empty() {
        PathBuf::from(hint)
    } else {
        PathBuf::from(selected)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use se_config::definition::MachineDefinition;
    use se_config::draft::Edit;
    use se_config::id::{DeviceKindId, NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_float::backend::Backend;
    use se_machine::indigo::ip12::Ip12MemoryConfiguration;
    use se_machine::indigo::ip12::Ip12SnapshotError;
    use se_machine::indigo::ip12::definition::Ip12Definition;
    use se_machine::machine::MachineSnapshotError;
    use se_machine::output::VideoOutput;

    use super::{
        GraphicsBoard, MachineStartupConfiguration, build_machine_from_parts,
        build_normal_configuration, config, firmware_unconfigured, legacy_recording_projection,
    };

    const PROM_BYTES: usize = 0x40000;

    fn startup_configuration(graphics: Option<GraphicsBoard>) -> MachineStartupConfiguration {
        MachineStartupConfiguration::IndigoIp12 {
            floating_point_backend: Backend::SoftFloat,
            memory: Ip12MemoryConfiguration::default(),
            graphics,
        }
    }

    #[test]
    fn application_composition_installs_the_selected_graphics_board() {
        let lg1 = build_machine_from_parts(
            startup_configuration(Some(GraphicsBoard::Lg1)),
            vec![0; PROM_BYTES],
            None,
            None,
        )
        .unwrap();
        let graphics_free =
            build_machine_from_parts(startup_configuration(None), vec![0; PROM_BYTES], None, None)
                .unwrap();

        assert!(matches!(lg1.video_output(), VideoOutput::NoSignal));
        assert!(matches!(
            graphics_free.video_output(),
            VideoOutput::NoGraphicsBoard
        ));
    }

    #[test]
    fn machine_snapshot_error_preserves_the_ip12_and_gio_boundaries() {
        let lg1 = build_machine_from_parts(
            startup_configuration(Some(GraphicsBoard::Lg1)),
            vec![0; PROM_BYTES],
            None,
            None,
        )
        .unwrap();
        let snapshot = lg1.snapshot().unwrap();
        let mut graphics_free =
            build_machine_from_parts(startup_configuration(None), vec![0; PROM_BYTES], None, None)
                .unwrap();

        assert!(matches!(
            graphics_free.restore_snapshot(snapshot),
            Err(MachineSnapshotError::IndigoIp12(Ip12SnapshotError::Gio(_)))
        ));
        assert!(matches!(
            graphics_free.video_output(),
            VideoOutput::NoGraphicsBoard
        ));
    }

    #[test]
    fn normal_builder_accepts_arbitrary_scsi_targets_and_luns() {
        let directory =
            std::env::temp_dir().join(format!("sgi-emu-normal-build-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let prom = directory.join("prom.bin");
        fs::write(&prom, vec![0; PROM_BYTES]).unwrap();
        let mut draft = Ip12Definition.default_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("firmware.0.image-path")),
            value: PropertyValue::Text(prom.to_string_lossy().into_owned()),
        });
        for (target, lun, kind, name) in [
            (2, 3, "scsi.disk", "disk-a.img"),
            (5, 1, "scsi.disk", "disk-b.img"),
            (6, 4, "scsi.cdrom", "disc.iso"),
        ] {
            let path = directory.join(name);
            fs::write(&path, vec![0; 8192]).unwrap();
            draft.apply(Edit::SetAttachment {
                slot: NodeId(format!("scsi.0.target.{target}.lun.{lun}")),
                device: Some(DeviceKindId(String::from(kind))),
            });
            draft.apply(Edit::SetProperty {
                property: PropertyId(format!("scsi.0.target.{target}.lun.{lun}.medium-path")),
                value: PropertyValue::Text(path.to_string_lossy().into_owned()),
            });
        }
        if let Err(error) = build_normal_configuration(
            draft,
            &config::ApplicationConfig::default().network_configuration(),
        ) {
            panic!("{error}");
        }
        for name in ["prom.bin", "disk-a.img", "disk-b.img", "disc.iso"] {
            fs::remove_file(directory.join(name)).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn legacy_recording_rejects_unrepresentable_scsi_topology() {
        let mut draft = Ip12Definition.default_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("firmware.0.image-path")),
            value: PropertyValue::Text(String::from("prom.bin")),
        });
        draft.apply(Edit::SetAttachment {
            slot: NodeId(String::from("scsi.0.target.2.lun.0")),
            device: Some(DeviceKindId(String::from("scsi.disk"))),
        });
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("scsi.0.target.2.lun.0.medium-path")),
            value: PropertyValue::Text(String::from("disk.img")),
        });
        assert_eq!(
            legacy_recording_projection(&draft).err().as_deref(),
            Some("current Record format does not support this machine topology")
        );
    }

    #[test]
    fn only_explicitly_empty_firmware_skips_initial_build() {
        let mut draft = Ip12Definition.default_draft();
        assert!(firmware_unconfigured(&draft));
        let firmware = PropertyId(String::from("firmware.0.image-path"));
        draft.properties.remove(&firmware);
        assert!(!firmware_unconfigured(&draft));
        draft.apply(Edit::SetProperty {
            property: firmware,
            value: PropertyValue::Integer(0),
        });
        assert!(!firmware_unconfigured(&draft));
    }
}
