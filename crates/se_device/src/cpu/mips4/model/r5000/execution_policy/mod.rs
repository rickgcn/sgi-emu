//! R5000 functional execution policy.

use crate::cpu::mips4::cache::hierarchy::{
    Mips4CacheAccessPolicy, Mips4CacheConfigError, Mips4CacheGeometry, Mips4CacheHierarchyConfig,
};
use crate::cpu::mips4::cache::{Mips4CacheCoherenceAlgorithm, Mips4MemoryAccessType};
use crate::cpu::mips4::config::Mips4CacheConfig;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::{Mips4Cp0Config, Mips4Cp0Register, Mips4Cp0Status};
use crate::cpu::mips4::exception::{Mips4ErrorException, Mips4ExceptionImage};
use crate::cpu::mips4::execution::policy::{
    Mips4Cp0DoublewordTransferDirection, Mips4Cp0DoublewordTransferPolicy, Mips4Cp0WaitPolicy,
    Mips4ExecutionPolicy, Mips4NotWordValuePolicy, Mips4PrefetchPolicy,
    Mips4ReservedCp1ControlPolicy,
};
use crate::cpu::mips4::instruction::decode::Mips4CpuInstruction;
use crate::cpu::mips4::mmu::{Mips4MmuCacheAttribute, Mips4MmuConfig, Mips4MmuPrivilegeMode};
use crate::cpu::mips4::tlb::Mips4TlbAddressMode;

use super::boot_mode::R5000BootMode;
use super::profile::R5000Profile;

const RESET_PC: u64 = 0xffff_ffff_bfc0_0000;
const NORMAL_VECTOR_BASE: u64 = 0xffff_ffff_8000_0000;
const BOOT_VECTOR_BASE: u64 = 0xffff_ffff_bfc0_0200;
const NORMAL_CACHE_ERROR_VECTOR: u64 = 0xffff_ffff_a000_0100;

const CONFIG_EC_SHIFT: u8 = 28;
const CONFIG_EP_SHIFT: u8 = 24;
const CONFIG_SB_SHIFT: u8 = 22;
const CONFIG_SS_SHIFT: u8 = 20;
const CONFIG_SC: u32 = 1 << 17;
const CONFIG_FIXED_ONE: u32 = 1 << 16;
const CONFIG_BE: u32 = 1 << 15;
const CONFIG_EM: u32 = 1 << 14;
const CONFIG_EB: u32 = 1 << 13;
const CONFIG_IC_SHIFT: u8 = 9;
const CONFIG_DC_SHIFT: u8 = 6;
const CONFIG_IB: u32 = 1 << 5;
const CONFIG_DB: u32 = 1 << 4;
const CONFIG_WRITABLE_MASK: u64 = (1 << 12) | 0x0f;

/// R5000 model decisions consumed by the generic MIPS IV execution target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct R5000ExecutionPolicy {
    profile: R5000Profile,
    boot_mode: R5000BootMode,
}

impl R5000ExecutionPolicy {
    /// Creates execution policy from the processor profile and sampled boot mode.
    pub const fn new(profile: R5000Profile, boot_mode: R5000BootMode) -> Self {
        Self { profile, boot_mode }
    }

    /// Returns the processor profile.
    pub const fn profile(self) -> R5000Profile {
        self.profile
    }

    /// Returns the sampled boot mode.
    pub const fn boot_mode(self) -> R5000BootMode {
        self.boot_mode
    }

    /// Validates processor-specific cache geometry and sampled boot settings.
    pub fn validate_cache_config(self) -> Result<(), Mips4CacheConfigError> {
        for cache in [self.profile.instruction_cache, self.profile.data_cache] {
            if cache != Mips4CacheConfig::present(32 * 1024, 32) {
                return Err(Mips4CacheConfigError::InvalidR5000PrimaryGeometry);
            }
        }
        match self.profile.secondary_cache {
            Mips4CacheConfig::Disabled => {
                if self.boot_mode.secondary_cache_enabled() {
                    return Err(Mips4CacheConfigError::R5000SecondaryBootConflict);
                }
            }
            Mips4CacheConfig::Present {
                size_bytes,
                line_size_bytes,
            } => {
                if line_size_bytes != 32 || !matches!(size_bytes, 524_288 | 1_048_576 | 2_097_152) {
                    return Err(Mips4CacheConfigError::InvalidR5000SecondaryGeometry);
                }
                if !self.boot_mode.secondary_cache_enabled()
                    || size_bytes != self.boot_mode.secondary_cache_size().size_bytes()
                {
                    return Err(Mips4CacheConfigError::R5000SecondaryBootConflict);
                }
            }
        }
        Ok(())
    }
}

impl Mips4ExecutionPolicy for R5000ExecutionPolicy {
    fn reset_pc(&self) -> u64 {
        RESET_PC
    }

    fn architecture_config(&self) -> crate::cpu::mips4::config::Mips4Config {
        self.profile.to_mips4_config()
    }

    fn cp0_config(&self) -> u32 {
        reset_config(self.profile, self.boot_mode)
    }

    fn fcr0(&self) -> u32 {
        self.profile.fcr0()
    }

    fn tlb_entry_count(&self) -> usize {
        self.profile.tlb_entry_count() as usize
    }

    fn tlb_random_upper_bound(&self) -> u8 {
        self.profile.tlb_random_upper_bound()
    }

    fn mmu_config(&self, config: Mips4Cp0Config) -> Mips4MmuConfig {
        let kseg0 = Mips4CacheCoherenceAlgorithm::from_bits((config.bits() & 0x07) as u8).unwrap();
        Mips4MmuConfig::new(self.profile.to_mips4_config().address, kseg0)
    }

    fn cp0_write_value(&self, register: Mips4Cp0Register, current: u64, requested: u64) -> u64 {
        let mask = match register {
            Mips4Cp0Register::Config => CONFIG_WRITABLE_MASK,
            _ => u64::MAX,
        };
        (current & !mask) | (requested & mask)
    }

    fn cp0_wait_policy(&self) -> Mips4Cp0WaitPolicy {
        Mips4Cp0WaitPolicy::Standby
    }

    fn cp0_doubleword_transfer_policy(
        &self,
        _direction: Mips4Cp0DoublewordTransferDirection,
        status: Mips4Cp0Status,
        register: Mips4Cp0Register,
    ) -> Mips4Cp0DoublewordTransferPolicy {
        match Mips4MmuPrivilegeMode::from_status(status) {
            Some(Mips4MmuPrivilegeMode::Kernel) => {}
            Some(Mips4MmuPrivilegeMode::Supervisor) if status.supervisor_64_bit_addressing() => {}
            Some(Mips4MmuPrivilegeMode::User) if status.user_64_bit_addressing() => {}
            Some(Mips4MmuPrivilegeMode::Supervisor | Mips4MmuPrivilegeMode::User) | None => {
                return Mips4Cp0DoublewordTransferPolicy::ReservedInstruction;
            }
        }
        if matches!(
            register,
            Mips4Cp0Register::EntryLo0
                | Mips4Cp0Register::EntryLo1
                | Mips4Cp0Register::Context
                | Mips4Cp0Register::BadVaddr
                | Mips4Cp0Register::EntryHi
                | Mips4Cp0Register::Epc
                | Mips4Cp0Register::XContext
                | Mips4Cp0Register::ErrorEpc
        ) {
            Mips4Cp0DoublewordTransferPolicy::Execute
        } else {
            Mips4Cp0DoublewordTransferPolicy::NoOperation
        }
    }

    fn prefetch_policy(&self) -> Mips4PrefetchPolicy {
        Mips4PrefetchPolicy::NoOperation
    }

    fn not_word_value_policy(&self, instruction: Mips4CpuInstruction) -> Mips4NotWordValuePolicy {
        match instruction {
            Mips4CpuInstruction::Addiu | Mips4CpuInstruction::Addu => {
                Mips4NotWordValuePolicy::ExecuteLowWord
            }
            _ => Mips4NotWordValuePolicy::NoOperation,
        }
    }

    fn reserved_cp1_control_policy(&self, _register: u8) -> Mips4ReservedCp1ControlPolicy {
        Mips4ReservedCp1ControlPolicy::ReadZeroWriteIgnore
    }

    fn resolve_access_type(
        &self,
        cache_attribute: Mips4MmuCacheAttribute,
    ) -> Mips4MemoryAccessType {
        let Some(algorithm) = cache_attribute.cache_coherence_algorithm() else {
            return Mips4MemoryAccessType::Uncached;
        };
        match algorithm.bits() {
            0 | 1 | 3 => Mips4MemoryAccessType::CachedNoncoherent,
            _ => Mips4MemoryAccessType::Uncached,
        }
    }

    fn cache_config(&self) -> Mips4CacheHierarchyConfig {
        let secondary = match self.profile.secondary_cache {
            Mips4CacheConfig::Disabled => None,
            Mips4CacheConfig::Present {
                size_bytes,
                line_size_bytes,
            } => Some(Mips4CacheGeometry::new(size_bytes, line_size_bytes, 1)),
        };
        Mips4CacheHierarchyConfig::new(
            Some(Mips4CacheGeometry::new(32 * 1024, 32, 2)),
            Some(Mips4CacheGeometry::new(32 * 1024, 32, 2)),
            secondary,
        )
    }

    fn resolve_cache_policy(
        &self,
        cache_attribute: Mips4MmuCacheAttribute,
    ) -> Mips4CacheAccessPolicy {
        let Some(algorithm) = cache_attribute.cache_coherence_algorithm() else {
            return Mips4CacheAccessPolicy::Uncached;
        };
        match algorithm.bits() {
            0 => Mips4CacheAccessPolicy::WriteThroughNoWriteAllocate,
            1 => Mips4CacheAccessPolicy::WriteThroughWriteAllocate,
            3 => Mips4CacheAccessPolicy::WriteBackWriteAllocate,
            _ => Mips4CacheAccessPolicy::Uncached,
        }
    }

    fn exception_vector(
        &self,
        status_before_exception: Mips4Cp0Status,
        _image: Mips4ExceptionImage,
        refill_address_mode: Option<Mips4TlbAddressMode>,
    ) -> u64 {
        let base = if status_before_exception.boot_exception_vectors() {
            BOOT_VECTOR_BASE
        } else {
            NORMAL_VECTOR_BASE
        };
        let offset = if status_before_exception.exception_level() {
            0x180
        } else {
            match refill_address_mode {
                Some(Mips4TlbAddressMode::Bits32) => 0,
                Some(Mips4TlbAddressMode::Bits64) => 0x80,
                None => 0x180,
            }
        };
        base + offset
    }

    fn error_exception_vector(
        &self,
        status_before_exception: Mips4Cp0Status,
        reason: Mips4ErrorException,
    ) -> u64 {
        match reason {
            Mips4ErrorException::SoftReset | Mips4ErrorException::NonMaskableInterrupt => RESET_PC,
            Mips4ErrorException::CacheError => {
                if status_before_exception.boot_exception_vectors() {
                    BOOT_VECTOR_BASE + 0x100
                } else {
                    NORMAL_CACHE_ERROR_VECTOR
                }
            }
        }
    }
}

const fn reset_config(profile: R5000Profile, boot_mode: R5000BootMode) -> u32 {
    let mut bits = ((boot_mode.sys_clock_ratio_bits() as u32) << CONFIG_EC_SHIFT)
        | ((boot_mode.transmit_data_pattern_bits() as u32) << CONFIG_EP_SHIFT)
        | (1 << CONFIG_SB_SHIFT)
        | ((boot_mode.secondary_cache_size_bits() as u32) << CONFIG_SS_SHIFT)
        | CONFIG_FIXED_ONE
        | CONFIG_EM
        | CONFIG_EB
        | (3 << CONFIG_IC_SHIFT)
        | (3 << CONFIG_DC_SHIFT)
        | CONFIG_IB
        | CONFIG_DB
        | 2;
    if boot_mode.secondary_cache_enabled() {
        bits |= 1 << 12;
    }
    if !profile.secondary_cache.is_present() {
        bits |= CONFIG_SC;
    }
    if matches!(profile.endianness, Mips4Endianness::Big) {
        bits |= CONFIG_BE;
    }
    bits
}

#[cfg(test)]
mod tests;
