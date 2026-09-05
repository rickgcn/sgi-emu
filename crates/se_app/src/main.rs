//! Application entry point.

mod config;
mod persistence;
mod storage;

use std::error::Error;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use se_cli::Arguments;
use se_float::backend::Backend;
use se_machine::indigo::ip12::Ip12;
use se_machine::machine::Machine;
use se_runtime::record::{MediaIdentity, RecordManifest, Recorder, Replayer};
use se_runtime::runtime::{Runtime, RuntimeConfiguration};
use se_ui::bridge::ffi::MachineConfiguration;
use se_ui::session::{MachineBuildRequest, UiSession};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments = Arguments::parse_process();
    let config_path = config::config_path()?;
    let mut application_config = config::load(&config_path)?;
    application_config.apply_environment();
    application_config.apply_arguments(&arguments);

    let machine_configuration = application_config.machine_configuration();
    if arguments.headless() && machine_configuration.prom_path.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "headless mode requires an Indigo IP12 PROM; use --prom or SE_INDIGO_IP12_PROM",
        )
        .into());
    }
    let (machine, startup_error) = if machine_configuration.prom_path.is_empty() {
        (None, String::new())
    } else {
        match build_normal_machine(&machine_configuration) {
            Ok(machine) => (Some(machine), String::new()),
            Err(error) => (None, error),
        }
    };

    if arguments.headless() {
        let machine = machine.ok_or_else(|| io::Error::other(startup_error))?;
        let runtime = Runtime::new(Some(machine))?;
        let frontend_result = se_cli::headless::run(&runtime);
        if let Some(state) = runtime.shutdown()? {
            persistence::save(&state)?;
        }
        return frontend_result.map_err(|error| Box::new(error) as Box<dyn Error>);
    }

    let startup = application_config.ui_startup_state(startup_error);
    let runtime = Runtime::new(machine)?;
    let session = UiSession::new(runtime, Box::new(build_runtime_configuration));
    let exit = session.run(&startup);
    if let Some(state) = session.shutdown()? {
        persistence::save(&state)?;
    }

    application_config.apply_ui_exit_state(exit);
    config::save(&config_path, &application_config)?;

    Ok(())
}

fn build_normal_machine(configuration: &MachineConfiguration) -> Result<Machine, String> {
    build_ip12(
        &configuration.machine_model,
        &configuration.float_backend,
        Path::new(&configuration.prom_path),
        optional_path(&configuration.disk_path),
        optional_path(&configuration.cdrom_path),
        true,
    )
}

fn build_runtime_configuration(
    configuration: &MachineConfiguration,
    request: MachineBuildRequest,
) -> Result<RuntimeConfiguration, String> {
    match request {
        MachineBuildRequest::Normal => {
            build_normal_machine(configuration).map(RuntimeConfiguration::normal)
        }
        MachineBuildRequest::Recording(path) => build_recording_configuration(configuration, path),
        MachineBuildRequest::Replaying { path, snapshot_id } => {
            build_replay_configuration(configuration, path, snapshot_id.as_deref())
        }
    }
}

fn build_recording_configuration(
    configuration: &MachineConfiguration,
    path: PathBuf,
) -> Result<RuntimeConfiguration, String> {
    validate_machine_and_backend(&configuration.machine_model, &configuration.float_backend)?;
    let prom_path = Path::new(&configuration.prom_path);
    let raw_prom = read_prom(prom_path)?;
    let prom_identity = MediaIdentity::from_bytes(prom_path, &raw_prom);
    let mut disk = open_optional_storage(optional_path(&configuration.disk_path), false)?;
    let mut cdrom = open_optional_storage(optional_path(&configuration.cdrom_path), true)?;
    let disk_identity = identity_for_storage(&mut disk, optional_path(&configuration.disk_path))?;
    let cdrom_identity =
        identity_for_storage(&mut cdrom, optional_path(&configuration.cdrom_path))?;
    let recorder = Recorder::create_or_replace(path).map_err(|error| error.to_string())?;
    let disk = disk.map(|storage| storage.recording(recorder.disk()).boxed());
    let cdrom = cdrom.map(storage::FileBlockStorage::boxed);
    let mut machine = build_ip12_from_parts(
        &configuration.machine_model,
        &configuration.float_backend,
        raw_prom,
        disk,
        cdrom,
    )?;
    restore_persisted_state(&mut machine, &configuration.machine_model)?;
    let nonvolatile_state = machine.nonvolatile_state();
    let manifest = RecordManifest::new(
        configuration.machine_model.clone(),
        configuration.float_backend.clone(),
        prom_identity,
        disk_identity,
        cdrom_identity,
        nonvolatile_state,
    );
    recorder
        .start(&manifest)
        .map_err(|error| error.to_string())?;
    Ok(RuntimeConfiguration::recording(machine, recorder))
}

fn build_replay_configuration(
    configuration: &MachineConfiguration,
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
    validate_machine_and_backend(manifest.machine_model(), manifest.float_backend())?;
    let nonvolatile_state = manifest.nonvolatile_state().clone();
    let prom_path = selected_or_hint(&configuration.prom_path, &manifest.prom().path_hint);
    let raw_prom = read_prom(&prom_path)?;
    ensure_identity(
        "PROM",
        manifest.prom(),
        &MediaIdentity::from_bytes(&prom_path, &raw_prom),
    )?;

    let disk = match manifest.disk() {
        None => None,
        Some(expected) => {
            let path = selected_or_hint(&configuration.disk_path, &expected.path_hint);
            let mut storage = storage::FileBlockStorage::open_read_only(&path)
                .map_err(|error| {
                    format!("failed to open Replay disk '{}': {error}", path.display())
                })?
                .replay(replayer.disk());
            let identity = storage.replay_initial_identity(&path).map_err(|error| {
                format!(
                    "failed to validate Replay disk '{}': {error}",
                    path.display()
                )
            })?;
            ensure_identity("disk", expected, &identity)?;
            Some(storage.boxed())
        }
    };
    let cdrom = match manifest.cdrom() {
        None => None,
        Some(expected) => {
            let path = selected_or_hint(&configuration.cdrom_path, &expected.path_hint);
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
    let mut machine = build_ip12_from_parts(
        manifest.machine_model(),
        manifest.float_backend(),
        raw_prom,
        disk,
        cdrom,
    )?;
    machine.restore_nonvolatile_state(nonvolatile_state, 0);
    Ok(RuntimeConfiguration::replaying(machine, replayer))
}

fn build_ip12(
    machine_model: &str,
    float_backend: &str,
    prom_path: &Path,
    disk_path: Option<&Path>,
    cdrom_path: Option<&Path>,
    restore_persistence: bool,
) -> Result<Machine, String> {
    let raw_prom = read_prom(prom_path)?;
    let disk = open_optional_storage(disk_path, false)?.map(storage::FileBlockStorage::boxed);
    let cdrom = open_optional_storage(cdrom_path, true)?.map(storage::FileBlockStorage::boxed);
    let mut machine = build_ip12_from_parts(machine_model, float_backend, raw_prom, disk, cdrom)?;
    if restore_persistence {
        restore_persisted_state(&mut machine, machine_model)?;
    }
    Ok(machine)
}

fn build_ip12_from_parts(
    machine_model: &str,
    float_backend: &str,
    raw_prom: Vec<u8>,
    disk: Option<Box<dyn se_device::storage::BlockStorage>>,
    cdrom: Option<Box<dyn se_device::storage::BlockStorage>>,
) -> Result<Machine, String> {
    validate_machine_and_backend(machine_model, float_backend)?;
    let backend = match float_backend {
        "softfloat" => Backend::SoftFloat,
        "native" => Backend::Native,
        _ => unreachable!("backend was validated"),
    };
    Ip12::new(raw_prom, backend, disk, cdrom)
        .map(Machine::IndigoIp12)
        .map_err(|error| error.to_string())
}

fn validate_machine_and_backend(machine_model: &str, float_backend: &str) -> Result<(), String> {
    if machine_model != "indigo-ip12" {
        return Err(format!("unsupported machine model: {machine_model}"));
    }
    if !matches!(float_backend, "softfloat" | "native") {
        return Err(format!(
            "unsupported floating-point backend: {float_backend}"
        ));
    }
    Ok(())
}

fn read_prom(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path)
        .map_err(|error| format!("failed to read PROM image '{}': {error}", path.display()))
}

fn open_optional_storage(
    path: Option<&Path>,
    read_only: bool,
) -> Result<Option<storage::FileBlockStorage>, String> {
    let Some(path) = path else {
        return Ok(None);
    };
    let result = if read_only {
        storage::FileBlockStorage::open_read_only(path)
    } else {
        storage::FileBlockStorage::open_read_write(path)
    };
    result
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

fn optional_path(path: &str) -> Option<&Path> {
    (!path.is_empty()).then(|| Path::new(path))
}

fn selected_or_hint(selected: &str, hint: &str) -> PathBuf {
    if selected.is_empty() {
        PathBuf::from(hint)
    } else {
        PathBuf::from(selected)
    }
}
