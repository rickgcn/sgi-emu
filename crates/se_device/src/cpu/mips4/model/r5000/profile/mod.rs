//! Configurable R5000-compatible processor profile data.
//!
//! The profile records documented R5000 identity constants and caller-supplied
//! cache and clock configuration. It does not encode O2/IP32 board policy or
//! construct implementation-specific CP0 Config reset bits.

use crate::cpu::mips4::config::{
    Mips4AddressConfig, Mips4CacheConfig, Mips4Config, Mips4CoprocessorConfig, Mips4Endianness,
};
use crate::cpu::mips4::model::r5000::revision::{R5000Revision, r5000_fcr0, r5000_processor_id};

/// Implemented physical address width.
pub const R5000_PHYSICAL_ADDRESS_BITS: u8 = 36;

/// Implemented virtual address width in 64-bit addressing mode.
pub const R5000_VIRTUAL_ADDRESS_BITS: u8 = 40;

/// Number of R5000 TLB entries.
pub const R5000_TLB_ENTRY_COUNT: u8 = 48;

/// Reset upper bound for the CP0 `Random` register.
pub const R5000_TLB_RANDOM_UPPER_BOUND: u8 = R5000_TLB_ENTRY_COUNT - 1;

/// Standard R5000 primary instruction cache size.
pub const R5000_PRIMARY_INSTRUCTION_CACHE_SIZE_BYTES: u32 = 32 * 1024;

/// Standard R5000 primary data cache size.
pub const R5000_PRIMARY_DATA_CACHE_SIZE_BYTES: u32 = 32 * 1024;

/// Standard R5000 primary cache line size.
pub const R5000_PRIMARY_CACHE_LINE_SIZE_BYTES: u32 = 32;

/// Configurable R5000-compatible processor profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct R5000Profile {
    /// Processor memory byte order.
    pub endianness: Mips4Endianness,

    /// Processor and FPU revision bits.
    pub revision: R5000Revision,

    /// Processor frequency in hertz.
    pub processor_frequency_hz: u64,

    /// Instruction cache configuration.
    pub instruction_cache: Mips4CacheConfig,

    /// Data cache configuration.
    pub data_cache: Mips4CacheConfig,

    /// Secondary cache configuration.
    pub secondary_cache: Mips4CacheConfig,
}

impl R5000Profile {
    /// Creates a configurable R5000-compatible profile.
    pub const fn new(
        endianness: Mips4Endianness,
        revision: R5000Revision,
        processor_frequency_hz: u64,
        instruction_cache: Mips4CacheConfig,
        data_cache: Mips4CacheConfig,
        secondary_cache: Mips4CacheConfig,
    ) -> Self {
        Self {
            endianness,
            revision,
            processor_frequency_hz,
            instruction_cache,
            data_cache,
            secondary_cache,
        }
    }

    /// Returns the raw CP0 `PRId` value for this profile.
    pub const fn processor_id(self) -> u32 {
        r5000_processor_id(self.revision)
    }

    /// Returns the raw CP1 `FCR0` value for this profile.
    pub const fn fcr0(self) -> u32 {
        r5000_fcr0(self.revision)
    }

    /// Returns the number of implemented TLB entries.
    pub const fn tlb_entry_count(self) -> u8 {
        R5000_TLB_ENTRY_COUNT
    }

    /// Returns the CP0 `Random` upper bound for reset and `Wired` writes.
    pub const fn tlb_random_upper_bound(self) -> u8 {
        R5000_TLB_RANDOM_UPPER_BOUND
    }

    /// Creates the architecture-level MIPS IV configuration for this profile.
    pub const fn to_mips4_config(self) -> Mips4Config {
        Mips4Config::new(
            self.endianness,
            self.processor_id(),
            Mips4AddressConfig::new(R5000_PHYSICAL_ADDRESS_BITS, R5000_VIRTUAL_ADDRESS_BITS),
            self.instruction_cache,
            self.data_cache,
            self.secondary_cache,
            Mips4CoprocessorConfig::new(true, false),
        )
    }
}

#[cfg(test)]
mod tests;
