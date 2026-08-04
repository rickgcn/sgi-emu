//! Construction settings for the SGI O2 IP32 machine profile.

use core::fmt;

use se_device::chipset::crime::config::{CrimeConfig, CrimeConfigError};
use se_device::chipset::gbe::protocol::GbeRasterMode;
use se_device::chipset::mace::config::MaceConfig;
use se_device::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use se_device::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
use se_device::cpu::mips4::model::r5000::profile::R5000Profile;
use se_device::cpu::mips4::model::r5000::revision::R5000Revision;
use se_device::memory::ds2502::Ds2502Config;
use se_device::rtc::ds1687::Ds1687Config;

use super::address_map::IP32_PROM_IMAGE_SIZE_BYTES;
use super::timing::IP32_TIMEBASE_HZ;

const DEFAULT_PROCESSOR_FREQUENCY_HZ: u64 = 180_000_000;
const PRIMARY_CACHE_SIZE_BYTES: u32 = 32 * 1024;
const SECONDARY_CACHE_SIZE_BYTES: u32 = 512 * 1024;
const CACHE_LINE_SIZE_BYTES: u32 = 32;
const SECONDARY_CACHE_ENABLE_BIT: u64 = 1 << 12;

/// Hardware construction input for one IP32 machine.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32MachineConfig {
    /// R5000 processor identity, byte order, clocks, and cache geometry.
    pub processor: R5000Profile,

    /// R5000 boot-mode serial stream sampled at reset.
    pub boot_mode: R5000BootMode,

    /// CRIME chipset and physical SDRAM topology.
    pub crime: CrimeConfig,

    /// MACE I/O ASIC configuration.
    pub mace: MaceConfig,

    /// Deterministic DS1687 RTC time and battery-backed register/NVRAM image.
    ///
    /// The IP32 PROM environment is not stored in this device.
    pub rtc: Ds1687Config,

    /// Deterministic board-identity ROM and EPROM image.
    pub nic_identity: Ds2502Config,

    /// Immutable base image for the 512 KiB byte-programmable System Flash.
    ///
    /// On IP32, the PROM environment resides in this System Flash and remains
    /// physically separate from the DS1687 RTC/NVRAM domain.
    pub prom_image: Vec<u8>,

    /// Selects how GBE display-frame content is produced.
    #[serde(default)]
    pub gbe_raster_mode: GbeRasterMode,
}

impl Default for Ip32MachineConfig {
    fn default() -> Self {
        Self {
            processor: R5000Profile::new(
                Mips4Endianness::Big,
                R5000Revision::from_bits(0x21),
                DEFAULT_PROCESSOR_FREQUENCY_HZ,
                Mips4CacheConfig::present(PRIMARY_CACHE_SIZE_BYTES, CACHE_LINE_SIZE_BYTES),
                Mips4CacheConfig::present(PRIMARY_CACHE_SIZE_BYTES, CACHE_LINE_SIZE_BYTES),
                Mips4CacheConfig::present(SECONDARY_CACHE_SIZE_BYTES, CACHE_LINE_SIZE_BYTES),
            ),
            boot_mode: R5000BootMode::from_low_bits(SECONDARY_CACHE_ENABLE_BIT)
                .expect("the default R5000 boot mode must be valid"),
            crime: CrimeConfig::default(),
            mace: MaceConfig::default(),
            rtc: Ds1687Config::default(),
            nic_identity: Ds2502Config::default(),
            prom_image: vec![0; IP32_PROM_IMAGE_SIZE_BYTES],
            gbe_raster_mode: GbeRasterMode::default(),
        }
    }
}

/// Construction settings that do not contain PROM or battery-backed bytes.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32PersistentConfig {
    processor: R5000Profile,
    boot_mode: R5000BootMode,
    crime: CrimeConfig,
    mace: MaceConfig,
    nic_identity: Ds2502Config,
}

impl Ip32PersistentConfig {
    /// Extracts persistent hardware settings from a complete machine configuration.
    pub fn from_machine_config(config: &Ip32MachineConfig) -> Self {
        Self {
            processor: config.processor,
            boot_mode: config.boot_mode,
            crime: config.crime,
            mace: config.mace,
            nic_identity: config.nic_identity.clone(),
        }
    }

    /// Returns the configured processor profile.
    pub const fn processor(&self) -> R5000Profile {
        self.processor
    }

    /// Returns the sampled boot-mode stream.
    pub const fn boot_mode(&self) -> R5000BootMode {
        self.boot_mode
    }

    /// Returns CRIME construction settings.
    pub const fn crime(&self) -> CrimeConfig {
        self.crime
    }

    /// Returns MACE construction settings.
    pub const fn mace(&self) -> MaceConfig {
        self.mace
    }

    /// Returns board-identity settings.
    pub const fn nic_identity(&self) -> &Ds2502Config {
        &self.nic_identity
    }

    /// Validates construction settings without requiring a PROM or battery image.
    pub fn validate(&self) -> Result<(), Ip32PersistentConfigError> {
        let frequency_hz = self.processor.processor_frequency_hz;
        if !(1..=IP32_TIMEBASE_HZ).contains(&frequency_hz) {
            return Err(Ip32PersistentConfigError::InvalidProcessorFrequency { frequency_hz });
        }
        self.crime
            .validate()
            .map_err(Ip32PersistentConfigError::Crime)
    }

    /// Creates machine construction input by adding session-specific PROM and RTC data.
    pub fn machine_config(
        &self,
        prom_image: Vec<u8>,
        rtc_unix_seconds: i64,
        rtc_nvram: Vec<u8>,
    ) -> Ip32MachineConfig {
        Ip32MachineConfig {
            processor: self.processor,
            boot_mode: self.boot_mode,
            crime: self.crime,
            mace: self.mace,
            rtc: Ds1687Config {
                initial_unix_seconds: rtc_unix_seconds,
                nvram: rtc_nvram,
            },
            nic_identity: self.nic_identity.clone(),
            prom_image,
            gbe_raster_mode: GbeRasterMode::default(),
        }
    }
}

impl Default for Ip32PersistentConfig {
    fn default() -> Self {
        Self::from_machine_config(&Ip32MachineConfig::default())
    }
}

/// Invalid persisted IP32 construction settings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip32PersistentConfigError {
    /// The processor frequency cannot be represented by the machine timebase.
    InvalidProcessorFrequency {
        /// Requested processor frequency in hertz.
        frequency_hz: u64,
    },

    /// CRIME construction settings are invalid.
    Crime(CrimeConfigError),
}

impl fmt::Display for Ip32PersistentConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidProcessorFrequency { frequency_hz } => write!(
                formatter,
                "invalid R5000 processor frequency {frequency_hz} Hz"
            ),
            Self::Crime(error) => error.fmt(formatter),
        }
    }
}

impl std::error::Error for Ip32PersistentConfigError {}
