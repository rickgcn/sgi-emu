//! Persistent application configuration.

use std::env;
use std::error::Error;
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use se_cli::Arguments;
use se_network::config::{NatConfig, PortForwardRule, TransportProtocol};
use se_ui::bridge::ffi::{
    MachineConfiguration, NetworkConfiguration, NetworkForwardRule, UiExitState, UiStartupState,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ApplicationConfig {
    machine: MachineConfig,
    network: NatConfig,
    ui: UiConfig,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(default)]
struct MachineConfig {
    model: String,
    memory_bank_a_simm_mib: u8,
    memory_bank_b_simm_mib: u8,
    memory_bank_c_simm_mib: u8,
    prom_path: String,
    disk_path: String,
    cdrom_path: String,
    float_backend: FloatBackend,
}

impl Default for MachineConfig {
    fn default() -> Self {
        Self {
            model: String::from("indigo-ip12"),
            memory_bank_a_simm_mib: 2,
            memory_bank_b_simm_mib: 0,
            memory_bank_c_simm_mib: 0,
            prom_path: String::new(),
            disk_path: String::new(),
            cdrom_path: String::new(),
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

    pub fn machine_configuration(&self) -> MachineConfiguration {
        MachineConfiguration {
            machine_model: self.machine.model.clone(),
            memory_bank_a_simm_mib: self.machine.memory_bank_a_simm_mib,
            memory_bank_b_simm_mib: self.machine.memory_bank_b_simm_mib,
            memory_bank_c_simm_mib: self.machine.memory_bank_c_simm_mib,
            prom_path: self.machine.prom_path.clone(),
            disk_path: self.machine.disk_path.clone(),
            cdrom_path: self.machine.cdrom_path.clone(),
            float_backend: String::from(self.machine.float_backend.identifier()),
            network: network_configuration_dto(&self.network),
        }
    }

    pub fn ui_startup_state(&self, startup_error: String) -> UiStartupState {
        UiStartupState {
            machine: self.machine_configuration(),
            window_geometry: self.ui.window_geometry.clone(),
            window_state: self.ui.window_state.clone(),
            startup_error,
        }
    }

    pub fn apply_ui_exit_state(&mut self, exit: UiExitState) -> Result<(), String> {
        let network = parse_network_configuration(&exit.machine.network)?;
        self.network = network;
        self.machine.model = exit.machine.machine_model;
        self.machine.memory_bank_a_simm_mib = exit.machine.memory_bank_a_simm_mib;
        self.machine.memory_bank_b_simm_mib = exit.machine.memory_bank_b_simm_mib;
        self.machine.memory_bank_c_simm_mib = exit.machine.memory_bank_c_simm_mib;
        self.machine.prom_path = exit.machine.prom_path;
        self.machine.disk_path = exit.machine.disk_path;
        self.machine.cdrom_path = exit.machine.cdrom_path;
        self.machine.float_backend = FloatBackend::from_identifier(&exit.machine.float_backend);
        self.ui.window_geometry = exit.window_geometry;
        self.ui.window_state = exit.window_state;
        Ok(())
    }
}

/// Converts editable text into a validated host NAT configuration.
pub fn parse_network_configuration(
    configuration: &NetworkConfiguration,
) -> Result<NatConfig, String> {
    let address = |value: &str, field: &str| {
        value
            .parse()
            .map_err(|_| format!("{field} must be an IPv4 address"))
    };
    let port = |value: &str| {
        value
            .parse()
            .map_err(|_| String::from("Ports must be integers from 1 to 65535"))
    };
    let config = NatConfig {
        subnet: configuration.subnet.clone(),
        gateway: address(&configuration.gateway, "Gateway")?,
        dns: address(&configuration.dns, "DNS proxy")?,
        dhcp_start: address(&configuration.dhcp_start, "DHCP start")?,
        forwards: configuration
            .forwards
            .iter()
            .map(|rule| {
                Ok(PortForwardRule {
                    protocol: match rule.protocol.as_str() {
                        "tcp" => TransportProtocol::Tcp,
                        "udp" => TransportProtocol::Udp,
                        _ => return Err(String::from("Forwarding protocol must be TCP or UDP")),
                    },
                    host_address: address(&rule.host_address, "Host address")?,
                    host_port: port(&rule.host_port)?,
                    guest_address: address(&rule.guest_address, "Guest address")?,
                    guest_port: port(&rule.guest_port)?,
                })
            })
            .collect::<Result<_, String>>()?,
    };
    config.validate().map_err(|error| error.to_string())?;
    Ok(config)
}

/// Formats persistent settings without discarding invalid values from a configuration file.
fn network_configuration_dto(config: &NatConfig) -> NetworkConfiguration {
    NetworkConfiguration {
        subnet: config.subnet.clone(),
        gateway: config.gateway.to_string(),
        dns: config.dns.to_string(),
        dhcp_start: config.dhcp_start.to_string(),
        forwards: config
            .forwards
            .iter()
            .map(|rule| NetworkForwardRule {
                protocol: match rule.protocol {
                    TransportProtocol::Tcp => "tcp",
                    TransportProtocol::Udp => "udp",
                }
                .into(),
                host_address: rule.host_address.to_string(),
                host_port: rule.host_port.to_string(),
                guest_address: rule.guest_address.to_string(),
                guest_port: rule.guest_port.to_string(),
            })
            .collect(),
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

    use se_network::config::NatConfig;
    use se_ui::bridge::ffi::{NetworkForwardRule, UiExitState};

    use super::{
        ApplicationConfig, FloatBackend, load, network_configuration_dto,
        parse_network_configuration, save,
    };

    #[test]
    fn editable_invalid_ports_and_subnets_are_not_silently_changed() {
        let mut dto = network_configuration_dto(&NatConfig::default());
        dto.forwards.push(NetworkForwardRule {
            protocol: "tcp".into(),
            host_address: "127.0.0.1".into(),
            host_port: "65536".into(),
            guest_address: "10.0.2.15".into(),
            guest_port: "22".into(),
        });
        assert!(parse_network_configuration(&dto).is_err());
        dto.forwards[0].host_port = "2222".into();
        assert!(parse_network_configuration(&dto).is_ok());
        dto.subnet = "10.0.3.0/24".into();
        assert!(parse_network_configuration(&dto).is_err());
    }

    #[test]
    fn default_configuration_uses_indigo_and_softfloat() {
        let config = ApplicationConfig::default();

        assert_eq!(config.machine.model, "indigo-ip12");
        assert_eq!(config.machine.memory_bank_a_simm_mib, 2);
        assert_eq!(config.machine.memory_bank_b_simm_mib, 0);
        assert_eq!(config.machine.memory_bank_c_simm_mib, 0);
        assert!(config.machine.prom_path.is_empty());
        assert!(config.machine.disk_path.is_empty());
        assert!(config.machine.cdrom_path.is_empty());
        assert!(matches!(
            config.machine.float_backend,
            FloatBackend::SoftFloat
        ));
    }

    #[test]
    fn network_settings_round_trip_through_toml_and_ui() {
        let mut config = ApplicationConfig::default();
        config
            .network
            .forwards
            .push(se_network::config::PortForwardRule {
                protocol: se_network::config::TransportProtocol::Udp,
                host_address: "127.0.0.1".parse().unwrap(),
                host_port: 5300,
                guest_address: "10.0.2.20".parse().unwrap(),
                guest_port: 53,
            });
        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("[[network.forwards]]"));
        let loaded: ApplicationConfig = toml::from_str(&serialized).unwrap();
        let mut applied = ApplicationConfig::default();
        applied
            .apply_ui_exit_state(se_ui::bridge::ffi::UiExitState {
                machine: loaded.machine_configuration(),
                window_geometry: String::new(),
                window_state: String::new(),
            })
            .unwrap();
        assert_eq!(applied.network, config.network);
    }

    #[test]
    fn invalid_exit_network_reports_an_error_without_partially_updating_settings() {
        let mut config = ApplicationConfig::default();
        let original = toml::to_string(&config).unwrap();
        let mut machine = config.machine_configuration();
        machine.prom_path = String::from("replacement.bin");
        machine.network.gateway = String::from("invalid-address");

        let error = config
            .apply_ui_exit_state(UiExitState {
                machine,
                window_geometry: String::from("replacement-geometry"),
                window_state: String::from("replacement-state"),
            })
            .unwrap_err();

        assert_eq!(error, "Gateway must be an IPv4 address");
        assert_eq!(toml::to_string(&config).unwrap(), original);
    }

    #[test]
    fn unknown_fields_do_not_prevent_loading() {
        let config: ApplicationConfig = toml::from_str(
            r#"
                future_value = true

                [machine]
                model = "indigo-ip12"
                memory_bank_a_simm_mib = 8
                memory_bank_b_simm_mib = 0
                memory_bank_c_simm_mib = 4
                prom_path = "prom.bin"
                disk_path = "disk.img"
                cdrom_path = "disc.iso"
                float_backend = "native"
                another_future_value = 7
            "#,
        )
        .unwrap();

        assert_eq!(config.machine.memory_bank_a_simm_mib, 8);
        assert_eq!(config.machine.memory_bank_b_simm_mib, 0);
        assert_eq!(config.machine.memory_bank_c_simm_mib, 4);
        assert_eq!(config.machine.prom_path, "prom.bin");
        assert_eq!(config.machine.disk_path, "disk.img");
        assert_eq!(config.machine.cdrom_path, "disc.iso");
        assert!(matches!(config.machine.float_backend, FloatBackend::Native));
    }

    #[test]
    fn older_configuration_uses_default_values_for_new_machine_settings() {
        let config: ApplicationConfig = toml::from_str(
            r#"
                [machine]
                model = "indigo-ip12"
                prom_path = "prom.bin"
                disk_path = "disk.img"
                float_backend = "native"
            "#,
        )
        .unwrap();

        assert!(config.machine.cdrom_path.is_empty());
        assert!(config.machine_configuration().cdrom_path.is_empty());
        assert_eq!(config.machine.memory_bank_a_simm_mib, 2);
        assert_eq!(config.machine.memory_bank_b_simm_mib, 0);
        assert_eq!(config.machine.memory_bank_c_simm_mib, 0);
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
        config.machine.cdrom_path = String::from("disc.iso");
        config.machine.memory_bank_a_simm_mib = 0;
        config.machine.memory_bank_b_simm_mib = 8;
        config.machine.memory_bank_c_simm_mib = 4;
        save(&path, &config).unwrap();

        let loaded = load(&path).unwrap();
        assert_eq!(loaded.machine.prom_path, "replacement.bin");
        assert_eq!(loaded.machine.disk_path, "disk.img");
        assert_eq!(loaded.machine.cdrom_path, "disc.iso");
        assert_eq!(loaded.machine.memory_bank_a_simm_mib, 0);
        assert_eq!(loaded.machine.memory_bank_b_simm_mib, 8);
        assert_eq!(loaded.machine.memory_bank_c_simm_mib, 4);

        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }
}
