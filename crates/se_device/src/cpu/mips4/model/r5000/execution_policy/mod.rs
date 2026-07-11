//! R5000 functional execution policy.

use crate::cpu::mips4::cache::{Mips4CacheCoherenceAlgorithm, Mips4MemoryAccessType};
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::{Mips4Cp0Config, Mips4Cp0Register, Mips4Cp0Status};
use crate::cpu::mips4::exception::Mips4ExceptionImage;
use crate::cpu::mips4::execution::policy::Mips4ExecutionPolicy;
use crate::cpu::mips4::mmu::{Mips4MmuCacheAttribute, Mips4MmuConfig};
use crate::cpu::mips4::tlb::Mips4TlbAddressMode;

use super::boot_mode::R5000BootMode;
use super::profile::R5000Profile;

const RESET_PC: u64 = 0xffff_ffff_bfc0_0000;
const NORMAL_VECTOR_BASE: u64 = 0xffff_ffff_8000_0000;
const BOOT_VECTOR_BASE: u64 = 0xffff_ffff_bfc0_0200;

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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
}

impl Mips4ExecutionPolicy for R5000ExecutionPolicy {
    fn reset_pc(&self) -> u64 {
        RESET_PC
    }

    fn endianness(&self) -> Mips4Endianness {
        self.profile.endianness
    }

    fn processor_id(&self) -> u32 {
        self.profile.processor_id()
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
        Mips4MmuConfig::new(kseg0)
    }

    fn cp0_write_value(&self, register: Mips4Cp0Register, current: u64, requested: u64) -> u64 {
        let mask = match register {
            Mips4Cp0Register::Config => CONFIG_WRITABLE_MASK,
            _ => u64::MAX,
        };
        (current & !mask) | (requested & mask)
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
