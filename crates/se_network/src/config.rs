//! Shared NAT configuration and validation used by application and UI callers.

use std::fmt;
use std::net::Ipv4Addr;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// Number of contiguous DHCP client addresses in libslirp 4.9.4.
pub const DHCP_CLIENTS: u32 = 16;

/// Size of the BOOTP reply filename field in libslirp 4.9.4.
const BOOTP_FILE_BYTES: usize = 128;

/// An IPv4 network with a canonical network address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4Subnet {
    network: u32,
    prefix: u8,
}

impl FromStr for Ipv4Subnet {
    type Err = ConfigError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = value
            .split_once('/')
            .ok_or_else(|| error("Subnet must include an IPv4 CIDR prefix"))?;
        let address: Ipv4Addr = address
            .parse()
            .map_err(|_| error("Subnet must contain an IPv4 address"))?;
        let prefix: u8 = prefix.parse().map_err(|_| error("Invalid subnet prefix"))?;
        if prefix > 30 {
            return Err(error("Subnet prefix must be between 0 and 30"));
        }
        let network = u32::from(address);
        let mask = u32::MAX.checked_shl(u32::from(32 - prefix)).unwrap_or(0);
        if network & !mask != 0 {
            return Err(error("Subnet must use its network address"));
        }
        Ok(Self { network, prefix })
    }
}

impl Ipv4Subnet {
    /// Returns the subnet network address.
    #[must_use]
    pub fn network(self) -> Ipv4Addr {
        self.network.into()
    }
    /// Returns the subnet mask.
    #[must_use]
    pub fn mask(self) -> Ipv4Addr {
        u32::MAX
            .checked_shl(u32::from(32 - self.prefix))
            .unwrap_or(0)
            .into()
    }
    /// Reports whether an address is a usable unicast host in this subnet.
    #[must_use]
    pub fn contains_host(self, address: Ipv4Addr) -> bool {
        let value = u32::from(address);
        let mask = u32::from(self.mask());
        value & mask == self.network
            && value != self.network
            && value != self.network | !mask
            && !address.is_unspecified()
            && !address.is_loopback()
            && !address.is_multicast()
            && address.octets()[0] != 0
            && address.octets()[0] < 240
    }
}

/// Transport protocol for one host port forward.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum TransportProtocol {
    /// TCP listener.
    Tcp,
    /// UDP listener.
    Udp,
}

/// One explicitly configured host-to-guest listener.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PortForwardRule {
    /// TCP or UDP.
    pub protocol: TransportProtocol,
    /// IPv4 address bound on the host; unspecified means every interface.
    pub host_address: Ipv4Addr,
    /// Nonzero host listening port.
    pub host_port: u16,
    /// Guest address in the configured subnet.
    pub guest_address: Ipv4Addr,
    /// Nonzero guest destination port.
    pub guest_port: u16,
}

/// Persistent, host-session IPv4 NAT settings.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(default)]
pub struct NatConfig {
    /// Canonical IPv4 network and CIDR prefix.
    pub subnet: String,
    /// Virtual gateway exposed to the guest.
    pub gateway: Ipv4Addr,
    /// Virtual DNS proxy, not an upstream resolver address.
    pub dns: Ipv4Addr,
    /// First address of the contiguous DHCP client pool.
    pub dhcp_start: Ipv4Addr,
    /// Explicit TCP/UDP host listeners.
    pub forwards: Vec<PortForwardRule>,
    /// Host directory served by the built-in TFTP server; absent disables it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tftp_root: Option<String>,
    /// Boot filename advertised in BOOTP replies; absent leaves the field empty.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootfile: Option<String>,
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            subnet: "10.0.2.0/24".into(),
            gateway: Ipv4Addr::new(10, 0, 2, 2),
            dns: Ipv4Addr::new(10, 0, 2, 3),
            dhcp_start: Ipv4Addr::new(10, 0, 2, 15),
            forwards: Vec::new(),
            tftp_root: None,
            bootfile: None,
        }
    }
}

impl NatConfig {
    /// Validates subnet membership, the entire DHCP pool, listener conflicts, and
    /// the network boot strings. Actual host binding is checked during session
    /// creation.
    ///
    /// # Errors
    /// Returns an actionable configuration error for an invalid field or rule.
    pub fn validate(&self) -> Result<Ipv4Subnet, ConfigError> {
        let subnet: Ipv4Subnet = self.subnet.parse()?;
        for (label, address) in [("Gateway", self.gateway), ("DNS proxy", self.dns)] {
            if !subnet.contains_host(address) {
                return Err(error(format!(
                    "{label} must be a usable address in the subnet"
                )));
            }
        }
        if self.gateway == self.dns {
            return Err(error("Gateway and DNS proxy must be different addresses"));
        }
        for offset in 0..DHCP_CLIENTS {
            let address = u32::from(self.dhcp_start)
                .checked_add(offset)
                .map(Ipv4Addr::from)
                .ok_or_else(|| error("DHCP pool overflows IPv4 addresses"))?;
            if !subnet.contains_host(address) || address == self.gateway || address == self.dns {
                return Err(error(
                    "All 16 DHCP addresses must be usable hosts and must exclude gateway and DNS",
                ));
            }
        }
        for (index, rule) in self.forwards.iter().enumerate() {
            if rule.host_port == 0 || rule.guest_port == 0 {
                return Err(error(format!(
                    "Forward {} ports must be between 1 and 65535",
                    index + 1
                )));
            }
            if rule.host_address.is_multicast() || rule.host_address.is_broadcast() {
                return Err(error(format!(
                    "Forward {} host address cannot be multicast or broadcast",
                    index + 1
                )));
            }
            if !subnet.contains_host(rule.guest_address)
                || rule.guest_address == self.gateway
                || rule.guest_address == self.dns
            {
                return Err(error(format!(
                    "Forward {} guest address must be a usable non-reserved subnet host",
                    index + 1
                )));
            }
            if self.forwards[..index].iter().any(|other| {
                other.protocol == rule.protocol
                    && other.host_port == rule.host_port
                    && (other.host_address == rule.host_address
                        || other.host_address.is_unspecified()
                        || rule.host_address.is_unspecified())
            }) {
                return Err(error(format!(
                    "Forward {} overlaps another host listener",
                    index + 1
                )));
            }
        }
        if let Some(root) = &self.tftp_root {
            if root.is_empty() {
                return Err(error("TFTP root must not be empty"));
            }
            if root.contains('\0') {
                return Err(error("TFTP root must not contain NUL bytes"));
            }
        }
        if let Some(bootfile) = &self.bootfile {
            if bootfile.contains('\0') {
                return Err(error("Boot filename must not contain NUL bytes"));
            }
            if bootfile.len() >= BOOTP_FILE_BYTES {
                return Err(error(format!(
                    "Boot filename must be shorter than {BOOTP_FILE_BYTES} bytes"
                )));
            }
        }
        Ok(subnet)
    }
}

/// Invalid NAT configuration with a user-facing explanation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigError(String);
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for ConfigError {}
fn error(message: impl Into<String>) -> ConfigError {
    ConfigError(message.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn validates_the_whole_dhcp_pool_without_silent_normalization() {
        let mut config = NatConfig::default();
        assert!(config.validate().is_ok());
        config.dhcp_start = Ipv4Addr::new(10, 0, 2, 245);
        assert!(config.validate().is_err());
        config.dhcp_start = config.gateway;
        assert!(config.validate().is_err());
        config = NatConfig::default();
        config.subnet = "10.0.2.1/24".into();
        assert!(config.validate().is_err());
    }
    #[test]
    fn wildcard_conflicts_are_protocol_specific() {
        let mut config = NatConfig::default();
        let rule = PortForwardRule {
            protocol: TransportProtocol::Tcp,
            host_address: Ipv4Addr::LOCALHOST,
            host_port: 8022,
            guest_address: config.dhcp_start,
            guest_port: 22,
        };
        config.forwards.push(rule.clone());
        let mut second = rule;
        second.host_address = Ipv4Addr::UNSPECIFIED;
        config.forwards.push(second);
        assert!(config.validate().is_err());
        config.forwards[1].protocol = TransportProtocol::Udp;
        assert!(config.validate().is_ok());
    }
    #[test]
    fn network_boot_defaults_expose_no_host_directory_or_bootfile() {
        let config = NatConfig::default();
        assert_eq!(config.tftp_root, None);
        assert_eq!(config.bootfile, None);
        assert!(config.validate().is_ok());
    }
    #[test]
    fn boot_filename_is_limited_to_the_native_field_in_bytes() {
        let validate = |bootfile: String| {
            NatConfig {
                bootfile: Some(bootfile),
                ..NatConfig::default()
            }
            .validate()
            .map_err(|error| error.to_string())
        };
        assert!(validate("b".repeat(BOOTP_FILE_BYTES - 1)).is_ok());
        assert_eq!(
            validate("b".repeat(BOOTP_FILE_BYTES)).unwrap_err(),
            "Boot filename must be shorter than 128 bytes"
        );
        assert!(validate("é".repeat(BOOTP_FILE_BYTES / 2 - 1)).is_ok());
        assert!(validate("é".repeat(BOOTP_FILE_BYTES / 2)).is_err());
    }
    #[test]
    fn network_boot_strings_reject_unrepresentable_values() {
        let with_bootfile = |bootfile: &str| NatConfig {
            bootfile: Some(bootfile.into()),
            ..NatConfig::default()
        };
        let with_root = |root: &str| NatConfig {
            tftp_root: Some(root.into()),
            ..NatConfig::default()
        };
        assert_eq!(
            with_bootfile("stand\0sa")
                .validate()
                .unwrap_err()
                .to_string(),
            "Boot filename must not contain NUL bytes"
        );
        assert_eq!(
            with_root("/srv/tftp\0").validate().unwrap_err().to_string(),
            "TFTP root must not contain NUL bytes"
        );
        assert_eq!(
            with_root("").validate().unwrap_err().to_string(),
            "TFTP root must not be empty"
        );
        assert!(with_root("/srv/tftp").validate().is_ok());
    }
}
