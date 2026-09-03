//! Application entry point.

mod config;
mod persistence;
mod storage;

use std::error::Error;
use std::fs;
use std::io;

use se_cli::Arguments;
use se_float::backend::Backend;
use se_machine::indigo::ip12::Ip12;
use se_machine::machine::Machine;
use se_runtime::runtime::Runtime;
use se_ui::bridge::ffi::MachineConfiguration;
use se_ui::session::UiSession;

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
        match build_machine(&machine_configuration) {
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
    let session = UiSession::new(runtime, Box::new(build_machine));
    let exit = session.run(&startup);
    if let Some(state) = session.shutdown()? {
        persistence::save(&state)?;
    }

    application_config.apply_ui_exit_state(exit);
    config::save(&config_path, &application_config)?;

    Ok(())
}

fn build_machine(configuration: &MachineConfiguration) -> Result<Machine, String> {
    match configuration.machine_model.as_str() {
        "indigo-ip12" => {
            let backend = match configuration.float_backend.as_str() {
                "softfloat" => Backend::SoftFloat,
                "native" => Backend::Native,
                _ => {
                    return Err(format!(
                        "unsupported floating-point backend: {}",
                        configuration.float_backend
                    ));
                }
            };
            let raw_prom = fs::read(&configuration.prom_path).map_err(|error| {
                format!(
                    "failed to read PROM image '{}': {error}",
                    configuration.prom_path
                )
            })?;
            let disk_storage = if configuration.disk_path.is_empty() {
                None
            } else {
                Some(
                    storage::FileBlockStorage::open_read_write(&configuration.disk_path)
                        .map_err(|error| {
                            format!(
                                "failed to open disk image '{}': {error}",
                                configuration.disk_path
                            )
                        })?
                        .boxed(),
                )
            };
            let cdrom_storage = if configuration.cdrom_path.is_empty() {
                None
            } else {
                Some(
                    storage::FileBlockStorage::open_read_only(&configuration.cdrom_path)
                        .map_err(|error| {
                            format!(
                                "failed to open CD-ROM image '{}': {error}",
                                configuration.cdrom_path
                            )
                        })?
                        .boxed(),
                )
            };
            let mut machine = Ip12::new(raw_prom, backend, disk_storage, cdrom_storage)
                .map(Machine::IndigoIp12)
                .map_err(|error| error.to_string())?;
            if let Some(restored) = persistence::load(&configuration.machine_model)
                .map_err(|error| error.to_string())?
            {
                machine.restore_nonvolatile_state(restored.state, restored.offline_milliseconds);
            }
            Ok(machine)
        }
        _ => Err(format!(
            "unsupported machine model: {}",
            configuration.machine_model
        )),
    }
}
