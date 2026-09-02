//! Persistent application configuration.

use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use se_cli::Arguments;
use se_ui::bridge::ffi::{UiExitState, UiStartupState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ApplicationConfig {
    machine: MachineConfig,
    ui: UiConfig,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
struct MachineConfig {
    model: String,
    prom_path: String,
    disk_path: String,
    float_backend: FloatBackend,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            model: String::from("indigo-ip12"),
            prom_path: String::new(),
            disk_path: String::new(),
            float_backend: FloatBackend::SoftFloat,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum FloatBackend {
    #[default]
    SoftFloat,
    Native,
}

impl FloatBackend {
    const fn identifier(self) -> &'static str {
        match self {
            Self::SoftFloat => "softfloat",
            Self::Native => "native",
        }
    }

    fn from_identifier(identifier: &str) -> Self {
        match identifier {
            "native" => Self::Native,
            _ => Self::SoftFloat,
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
struct UiConfig {
    window_geometry: String,
    window_state: String,
}

impl ApplicationConfig {
    pub fn apply_environment(&mut self) {
        let prom_path = env::var_os("SE_INDIGO_IP12_PROM");
        self.apply_environment_prom(prom_path.as_deref());
    }

    fn apply_environment_prom(&mut self, prom_path: Option<&OsStr>) {
        if let Some(prom_path) = prom_path {
            self.machine.prom_path = prom_path.to_string_lossy().into_owned();
        }
    }

    pub fn apply_arguments(&mut self, arguments: &Arguments) {
        if let Some(model) = arguments.machine() {
            self.machine.model = String::from(model);
        }
        if let Some(prom_path) = arguments.prom() {
            self.machine.prom_path = prom_path.to_string_lossy().into_owned();
        }
        if let Some(float_backend) = arguments.float_backend() {
            self.machine.float_backend = FloatBackend::from_identifier(float_backend);
        }
    }

    pub fn machine_configuration(&self) -> (&str, &str, &str, &str) {
        (
            &self.machine.model,
            &self.machine.prom_path,
            &self.machine.disk_path,
            self.machine.float_backend.identifier(),
        )
    }

    pub fn ui_startup_state(&self, startup_error: String) -> UiStartupState {
        UiStartupState {
            machine_model: self.machine.model.clone(),
            prom_path: self.machine.prom_path.clone(),
            disk_path: self.machine.disk_path.clone(),
            float_backend: String::from(self.machine.float_backend.identifier()),
            window_geometry: self.ui.window_geometry.clone(),
            window_state: self.ui.window_state.clone(),
            startup_error,
        }
    }

    pub fn apply_ui_exit_state(&mut self, exit: UiExitState) {
        self.machine.model = exit.machine_model;
        self.machine.prom_path = exit.prom_path;
        self.machine.disk_path = exit.disk_path;
        self.machine.float_backend = FloatBackend::from_identifier(&exit.float_backend);
        self.ui.window_geometry = exit.window_geometry;
        self.ui.window_state = exit.window_state;
    }
}

pub fn config_path() -> io::Result<PathBuf> {
    BaseDirs::new()
        .map(|directories| directories.config_dir().join("sgi-emu/config.toml"))
        .ok_or_else(|| io::Error::other("the host configuration directory is unavailable"))
}

pub fn load(path: &Path) -> Result<ApplicationConfig, Box<dyn Error>> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(ApplicationConfig::default());
        }
        Err(error) => return Err(error.into()),
    };

    Ok(toml::from_str(&contents)?)
}

pub fn save(path: &Path, config: &ApplicationConfig) -> Result<(), Box<dyn Error>> {
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("configuration path has no parent directory"))?;
    fs::create_dir_all(parent)?;

    let temporary_path = path.with_extension("toml.tmp");
    let mut temporary_file = File::create(&temporary_path)?;
    temporary_file.write_all(toml::to_string_pretty(config)?.as_bytes())?;
    temporary_file.sync_all()?;
    drop(temporary_file);

    if let Err(error) = fs::rename(&temporary_path, path) {
        let _ = fs::remove_file(&temporary_path);
        return Err(error.into());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;
    use std::fs;

    use clap::Parser;
    use se_cli::Arguments;

    use super::{ApplicationConfig, FloatBackend, load, save};

    #[test]
    fn default_configuration_uses_indigo_and_softfloat() {
        let config = ApplicationConfig::default();

        assert_eq!(config.machine.model, "indigo-ip12");
        assert!(config.machine.prom_path.is_empty());
        assert!(config.machine.disk_path.is_empty());
        assert!(matches!(
            config.machine.float_backend,
            FloatBackend::SoftFloat
        ));
    }

    #[test]
    fn unknown_fields_do_not_prevent_loading() {
        let config: ApplicationConfig = toml::from_str(
            r#"
                future_value = true

                [machine]
                model = "indigo-ip12"
                prom_path = "prom.bin"
                disk_path = "disk.img"
                float_backend = "native"
                another_future_value = 7
            "#,
        )
        .unwrap();

        assert_eq!(config.machine.prom_path, "prom.bin");
        assert_eq!(config.machine.disk_path, "disk.img");
        assert!(matches!(config.machine.float_backend, FloatBackend::Native));
    }

    #[test]
    fn environment_and_arguments_override_saved_machine_configuration_in_order() {
        let mut config: ApplicationConfig = toml::from_str(
            r#"
                [machine]
                model = "indigo-ip12"
                prom_path = "saved.bin"
                float_backend = "soft-float"
            "#,
        )
        .unwrap();
        config.apply_environment_prom(Some(OsStr::new("environment.bin")));
        assert_eq!(config.machine.prom_path, "environment.bin");

        let arguments = Arguments::try_parse_from([
            "sgi-emu",
            "--prom",
            "command-line.bin",
            "--float-backend",
            "native",
        ])
        .unwrap();
        config.apply_arguments(&arguments);

        assert_eq!(config.machine.prom_path, "command-line.bin");
        assert!(matches!(config.machine.float_backend, FloatBackend::Native));
    }

    #[test]
    fn saving_replaces_an_existing_configuration() {
        let directory =
            std::env::temp_dir().join(format!("sgi-emu-config-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");

        let mut config = ApplicationConfig::default();
        save(&path, &config).unwrap();
        config.machine.prom_path = String::from("replacement.bin");
        config.machine.disk_path = String::from("disk.img");
        save(&path, &config).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.machine.prom_path, "replacement.bin");
        assert_eq!(loaded.machine.disk_path, "disk.img");

        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
