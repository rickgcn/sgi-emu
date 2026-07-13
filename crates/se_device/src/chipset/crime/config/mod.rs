//! CRIME chipset configuration.

use core::fmt;

/// Capacity of one installed CRIME external SDRAM bank.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum CrimeSdramBankSize {
    /// A bank assembled from 16-Mbit SDRAM devices.
    MiB32,

    /// A bank assembled from 64-Mbit SDRAM devices.
    MiB128,
}

impl CrimeSdramBankSize {
    /// Returns the physical capacity in bytes.
    pub const fn bytes(self) -> u64 {
        match self {
            Self::MiB32 => 32 * 1024 * 1024,
            Self::MiB128 => 128 * 1024 * 1024,
        }
    }

    /// Returns the bank-control size bit used for this capacity.
    pub const fn control_bit(self) -> u16 {
        match self {
            Self::MiB32 => 0,
            Self::MiB128 => 1 << 8,
        }
    }
}

/// Physical population of one CRIME external SDRAM bank.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct CrimeSdramBankConfig {
    /// Installed capacity.
    pub size: CrimeSdramBankSize,
}

/// Physical SDRAM topology connected to CRIME.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeMemoryConfig {
    /// External banks in hardware priority order.
    #[serde(with = "bank_config_serde")]
    pub banks: [Option<CrimeSdramBankConfig>; 8],
}

mod bank_config_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use super::{CrimeSdramBankConfig, CrimeSdramBankSize};

    pub(super) fn serialize<S>(
        banks: &[Option<CrimeSdramBankConfig>; 8],
        serializer: S,
    ) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        banks
            .map(|bank| match bank.map(|bank| bank.size) {
                None => 0_u16,
                Some(CrimeSdramBankSize::MiB32) => 32,
                Some(CrimeSdramBankSize::MiB128) => 128,
            })
            .serialize(serializer)
    }

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> Result<[Option<CrimeSdramBankConfig>; 8], D::Error>
    where
        D: Deserializer<'de>,
    {
        let values = <[u16; 8]>::deserialize(deserializer)?;
        values
            .map(|value| match value {
                0 => Ok(None),
                32 => Ok(Some(CrimeSdramBankConfig {
                    size: CrimeSdramBankSize::MiB32,
                })),
                128 => Ok(Some(CrimeSdramBankConfig {
                    size: CrimeSdramBankSize::MiB128,
                })),
                value => Err(serde::de::Error::custom(format_args!(
                    "invalid CRIME SDRAM bank capacity {value} MiB"
                ))),
            })
            .into_iter()
            .collect::<Result<Vec<_>, D::Error>>()?
            .try_into()
            .map_err(|_| serde::de::Error::custom("invalid CRIME SDRAM bank count"))
    }
}

impl CrimeMemoryConfig {
    /// Creates an empty topology.
    pub const fn empty() -> Self {
        Self { banks: [None; 8] }
    }

    /// Returns the total installed physical capacity.
    pub const fn total_size_bytes(self) -> u64 {
        let mut total = 0;
        let mut index = 0;
        while index < self.banks.len() {
            if let Some(bank) = self.banks[index] {
                total += bank.size.bytes();
            }
            index += 1;
        }
        total
    }

    /// Validates topology requirements needed for deterministic reset mapping.
    pub const fn validate(self) -> Result<(), CrimeConfigError> {
        if self.banks[0].is_none() {
            return Err(CrimeConfigError::MissingBankZero);
        }
        Ok(())
    }
}

impl Default for CrimeMemoryConfig {
    fn default() -> Self {
        let bank = Some(CrimeSdramBankConfig {
            size: CrimeSdramBankSize::MiB32,
        });
        Self {
            banks: [bank, bank, None, None, None, None, None, None],
        }
    }
}

/// Behavior for mapped addresses whose target semantics are not implemented.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
pub enum CrimeAccessPolicy {
    /// Report a bus error.
    #[default]
    Strict,

    /// Read zero and ignore writes.
    Permissive,
}

/// Complete construction input for a CRIME 1.1 chipset.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeConfig {
    /// Installed SDRAM topology.
    pub memory: CrimeMemoryConfig,

    /// Behavior for mapped but unsupported peer devices.
    pub unimplemented_access_policy: CrimeAccessPolicy,
}

impl CrimeConfig {
    /// Validates the complete chipset configuration.
    pub const fn validate(self) -> Result<(), CrimeConfigError> {
        self.memory.validate()
    }
}

/// Invalid CRIME construction input.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeConfigError {
    /// Bank zero must be populated because it owns the reset mapping.
    MissingBankZero,
}

impl fmt::Display for CrimeConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBankZero => write!(f, "CRIME SDRAM bank zero is not populated"),
        }
    }
}

impl std::error::Error for CrimeConfigError {}

#[cfg(test)]
mod tests;
