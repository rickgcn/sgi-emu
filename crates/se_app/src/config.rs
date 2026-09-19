//! Persistent application configuration.

use std::error::Error;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use directories::BaseDirs;
use se_config::draft::MachineDraft;
use se_network::config::{NatConfig, PortForwardRule, TransportProtocol};
use se_session::machine::default_machine_draft;
use se_ui::bridge::ffi::{NetworkConfiguration, NetworkForwardRule, UiExitState, UiStartupState};
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize)]
pub struct ApplicationConfig {
    machine: MachineDraft,
    network: NatConfig,
    ui: UiConfig,
}

impl Default for ApplicationConfig {
    fn default() -> Self {
        Self {
            machine: default_machine_draft(),
            network: NatConfig::default(),
            ui: UiConfig::default(),
        }
    }
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct UiConfig {
    window_geometry: String,
    window_state: String,
}

impl ApplicationConfig {
    pub fn machine_draft(&self) -> MachineDraft {
        self.machine.clone()
    }

    pub fn set_machine_draft(&mut self, machine: MachineDraft) {
        self.machine = machine;
    }

    pub fn network_configuration(&self) -> NetworkConfiguration {
        network_configuration_dto(&self.network)
    }

    pub fn ui_startup_state(&self, startup_error: String) -> UiStartupState {
        UiStartupState {
            network: self.network_configuration(),
            window_geometry: self.ui.window_geometry.clone(),
            window_state: self.ui.window_state.clone(),
            startup_error,
        }
    }

    pub fn apply_ui_exit_state(&mut self, exit: UiExitState) -> Result<(), String> {
        let network = parse_network_configuration(&exit.network)?;
        self.network = network;
        self.ui.window_geometry = exit.window_geometry;
        self.ui.window_state = exit.window_state;
        Ok(())
    }
}

/// Converts one optional network boot field between editable text and storage.
///
/// Empty text means the setting is absent. Any other text is kept exactly as
/// entered, including surrounding whitespace, because a host path may end in a
/// space.
fn network_boot_setting(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_owned())
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
        tftp_root: network_boot_setting(&configuration.tftp_root),
        bootfile: network_boot_setting(&configuration.bootfile),
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

/// Checks that the host resources required by a validated network configuration exist.
///
/// Only the TFTP root names a host directory. The boot filename is a guest file
/// name that the built-in server resolves against that root for each request, so
/// it is never probed here.
///
/// This is a readiness check. Served files stay dynamic host resources, so a
/// root that passes here may still disappear before it is used, and the built-in
/// server answers each request from its own host access result.
///
/// # Errors
///
/// Returns an actionable message when a configured TFTP root is missing, is not
/// a directory, or cannot be examined.
pub fn preflight_network_config(config: &NatConfig) -> Result<(), String> {
    let Some(root) = config.tftp_root.as_deref() else {
        return Ok(());
    };
    match fs::metadata(root) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(format!("TFTP root is not a directory: {root}")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Err(format!("TFTP root does not exist: {root}"))
        }
        Err(error) => Err(format!("Cannot access TFTP root {root}: {error}")),
    }
}

/// Converts editable network text into a configuration whose host resources are ready.
///
/// # Errors
///
/// Returns the semantic error of [`parse_network_configuration`], or the host
/// error of [`preflight_network_config`].
pub fn prepare_network_configuration(
    configuration: &NetworkConfiguration,
) -> Result<NatConfig, String> {
    let config = parse_network_configuration(configuration)?;
    preflight_network_config(&config)?;
    Ok(config)
}

/// Formats persistent settings without discarding invalid values from a configuration file.
fn network_configuration_dto(config: &NatConfig) -> NetworkConfiguration {
    NetworkConfiguration {
        subnet: config.subnet.clone(),
        gateway: config.gateway.to_string(),
        dns: config.dns.to_string(),
        dhcp_start: config.dhcp_start.to_string(),
        tftp_root: config.tftp_root.clone().unwrap_or_default(),
        bootfile: config.bootfile.clone().unwrap_or_default(),
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
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    use se_config::draft::Edit;
    use se_config::id::PropertyId;
    use se_config::value::PropertyValue;
    use se_network::config::NatConfig;
    use se_ui::bridge::ffi::{NetworkForwardRule, UiExitState};

    use super::{
        ApplicationConfig, load, network_configuration_dto, parse_network_configuration,
        preflight_network_config, prepare_network_configuration, save,
    };

    static NEXT_DIRECTORY_ID: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory(name: &str) -> PathBuf {
        let id = NEXT_DIRECTORY_ID.fetch_add(1, Ordering::Relaxed);
        let directory =
            std::env::temp_dir().join(format!("sgi-emu-config-{name}-{}-{id}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn default_configuration_uses_the_session_draft() {
        let config = ApplicationConfig::default();
        assert_eq!(config.machine, se_session::machine::default_machine_draft());
    }

    #[test]
    fn machine_draft_round_trips_without_a_ui_projection() {
        let mut config = ApplicationConfig::default();
        config.machine.apply(Edit::SetProperty {
            property: PropertyId(String::from("test.opaque")),
            value: PropertyValue::Text(String::from("prom.bin")),
        });
        let serialized = toml::to_string(&config).unwrap();
        let restored: ApplicationConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.machine, config.machine);
    }

    #[test]
    fn old_machine_schema_is_rejected_without_migration() {
        let old = r#"
            [machine]
            model = "indigo-ip12"
            memory_bank_a_simm_mib = 2
            prom_path = "prom.bin"
            [network]
            subnet = "10.0.2.0/24"
            gateway = "10.0.2.2"
            dns = "10.0.2.3"
            dhcp_start = "10.0.2.15"
            forwards = []
            [ui]
            window_geometry = ""
            window_state = ""
        "#;
        assert!(toml::from_str::<ApplicationConfig>(old).is_err());
    }

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
    fn invalid_exit_network_does_not_partially_update_settings() {
        let mut config = ApplicationConfig::default();
        let original = toml::to_string(&config).unwrap();
        let mut network = config.network_configuration();
        network.gateway = String::from("invalid-address");
        let error = config
            .apply_ui_exit_state(UiExitState {
                network,
                window_geometry: String::from("replacement-geometry"),
                window_state: String::from("replacement-state"),
            })
            .unwrap_err();
        assert_eq!(error, "Gateway must be an IPv4 address");
        assert_eq!(toml::to_string(&config).unwrap(), original);
    }

    #[test]
    fn saving_replaces_an_existing_configuration() {
        let directory =
            std::env::temp_dir().join(format!("sgi-emu-config-test-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("config.toml");
        let mut config = ApplicationConfig::default();
        save(&path, &config).unwrap();
        config.machine.apply(Edit::SetProperty {
            property: PropertyId(String::from("test.opaque")),
            value: PropertyValue::Text(String::from("replacement.bin")),
        });
        save(&path, &config).unwrap();
        assert_eq!(load(&path).unwrap().machine, config.machine);
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn network_boot_text_round_trips_without_normalization() {
        let defaults = network_configuration_dto(&NatConfig::default());
        assert_eq!(defaults.tftp_root, "");
        assert_eq!(defaults.bootfile, "");

        let mut dto = defaults;
        dto.tftp_root = String::from("/srv/sgi ");
        dto.bootfile = String::from("stand/sa");
        let config = parse_network_configuration(&dto).unwrap();
        assert_eq!(config.tftp_root.as_deref(), Some("/srv/sgi "));
        assert_eq!(config.bootfile.as_deref(), Some("stand/sa"));
        let restored = network_configuration_dto(&config);
        assert_eq!(restored.tftp_root, "/srv/sgi ");
        assert_eq!(restored.bootfile, "stand/sa");
        assert_eq!(parse_network_configuration(&restored).unwrap(), config);

        let mut cleared = dto;
        cleared.tftp_root = String::new();
        cleared.bootfile = String::from("   ");
        let config = parse_network_configuration(&cleared).unwrap();
        assert_eq!(config.tftp_root, None);
        assert_eq!(config.bootfile.as_deref(), Some("   "));
        assert_eq!(network_configuration_dto(&config).tftp_root, "");
    }

    #[test]
    fn network_boot_defaults_stay_absent_and_load_as_unset() {
        let serialized = toml::to_string(&ApplicationConfig::default()).unwrap();
        assert!(!serialized.contains("tftp_root"));
        assert!(!serialized.contains("bootfile"));
        // Without the optional keys this text is exactly an older configuration.
        let restored: ApplicationConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.network.tftp_root, None);
        assert_eq!(restored.network.bootfile, None);
    }

    #[test]
    fn network_boot_settings_survive_the_persistent_file() {
        let mut config = ApplicationConfig::default();
        config.network.tftp_root = Some(String::from("/srv/sgi"));
        config.network.bootfile = Some(String::from("stand/sa"));

        let serialized = toml::to_string(&config).unwrap();
        assert!(serialized.contains("tftp_root = \"/srv/sgi\""));
        assert!(serialized.contains("bootfile = \"stand/sa\""));
        let restored: ApplicationConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(restored.network, config.network);
        assert_eq!(restored.network_configuration().tftp_root, "/srv/sgi");
        assert_eq!(restored.network_configuration().bootfile, "stand/sa");
    }

    #[test]
    fn invalid_network_boot_text_is_rejected_by_the_shared_semantics() {
        let mut dto = network_configuration_dto(&NatConfig::default());
        dto.bootfile = "b".repeat(128);
        assert_eq!(
            parse_network_configuration(&dto).unwrap_err(),
            "Boot filename must be shorter than 128 bytes"
        );
        dto.bootfile = String::from("stand\0sa");
        assert_eq!(
            parse_network_configuration(&dto).unwrap_err(),
            "Boot filename must not contain NUL bytes"
        );
    }

    #[test]
    fn host_preflight_accepts_a_directory_and_an_unset_root() {
        let directory = temporary_directory("preflight-valid");
        let config = NatConfig {
            tftp_root: Some(directory.to_string_lossy().into_owned()),
            bootfile: Some(String::from("stand/sa")),
            ..NatConfig::default()
        };

        assert_eq!(preflight_network_config(&NatConfig::default()), Ok(()));
        assert_eq!(preflight_network_config(&config), Ok(()));
        assert_eq!(
            prepare_network_configuration(&network_configuration_dto(&config)),
            Ok(config)
        );
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn host_preflight_reports_missing_and_non_directory_roots() {
        let directory = temporary_directory("preflight-invalid");
        let missing = directory.join("missing");
        let missing_config = NatConfig {
            tftp_root: Some(missing.to_string_lossy().into_owned()),
            ..NatConfig::default()
        };
        assert_eq!(
            preflight_network_config(&missing_config).unwrap_err(),
            format!("TFTP root does not exist: {}", missing.display())
        );

        let file = directory.join("prom.bin");
        fs::write(&file, [0]).unwrap();
        let file_config = NatConfig {
            tftp_root: Some(file.to_string_lossy().into_owned()),
            ..NatConfig::default()
        };
        assert_eq!(
            preflight_network_config(&file_config).unwrap_err(),
            format!("TFTP root is not a directory: {}", file.display())
        );
        assert!(prepare_network_configuration(&network_configuration_dto(&file_config)).is_err());
        fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn exit_state_saves_network_boot_settings_that_are_not_yet_available() {
        let directory = temporary_directory("exit-state");
        let root = directory.join("unmounted");
        let mut config = ApplicationConfig::default();
        let mut network = config.network_configuration();
        network.tftp_root = root.to_string_lossy().into_owned();
        network.bootfile = String::from("stand/sa");

        config
            .apply_ui_exit_state(UiExitState {
                network,
                window_geometry: String::from("geometry"),
                window_state: String::from("state"),
            })
            .unwrap();
        assert_eq!(
            config.network.tftp_root.as_deref(),
            Some(root.to_string_lossy().as_ref())
        );
        assert!(prepare_network_configuration(&config.network_configuration()).is_err());
        fs::remove_dir_all(&directory).unwrap();
    }
}
