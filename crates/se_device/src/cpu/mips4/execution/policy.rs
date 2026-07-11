//! Processor-specific policy required by functional MIPS IV execution.

use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::cache::hierarchy::{Mips4CacheAccessPolicy, Mips4CacheHierarchyConfig};
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::{Mips4Cp0Config, Mips4Cp0Register, Mips4Cp0Status};
use crate::cpu::mips4::exception::Mips4ExceptionImage;
use crate::cpu::mips4::mmu::{Mips4MmuCacheAttribute, Mips4MmuConfig};
use crate::cpu::mips4::tlb::Mips4TlbAddressMode;

/// Processor implementation policy used by the generic MIPS IV execution target.
pub trait Mips4ExecutionPolicy {
    /// Returns the reset program counter.
    fn reset_pc(&self) -> u64;

    /// Returns the configured processor byte order.
    fn endianness(&self) -> Mips4Endianness;

    /// Returns the initial CP0 processor identifier.
    fn processor_id(&self) -> u32;

    /// Returns the initial raw CP0 Config value.
    fn cp0_config(&self) -> u32;

    /// Returns the initial CP1 implementation/revision value.
    fn fcr0(&self) -> u32;

    /// Returns the implemented TLB entry count.
    fn tlb_entry_count(&self) -> usize;

    /// Returns the CP0 Random upper bound.
    fn tlb_random_upper_bound(&self) -> u8;

    /// Returns the current generic MMU configuration.
    fn mmu_config(&self, config: Mips4Cp0Config) -> Mips4MmuConfig;

    /// Applies processor-specific writable masks before a CP0 register write.
    fn cp0_write_value(&self, register: Mips4Cp0Register, current: u64, requested: u64) -> u64;

    /// Resolves an architecture cache attribute to a processor access type.
    fn resolve_access_type(&self, cache_attribute: Mips4MmuCacheAttribute)
    -> Mips4MemoryAccessType;

    /// Returns the processor's functional cache geometry.
    fn cache_config(&self) -> Mips4CacheHierarchyConfig;

    /// Resolves an architecture cache attribute to functional cache behavior.
    fn resolve_cache_policy(
        &self,
        cache_attribute: Mips4MmuCacheAttribute,
    ) -> Mips4CacheAccessPolicy;

    /// Selects the exception vector after CP0 state has been captured.
    fn exception_vector(
        &self,
        status_before_exception: Mips4Cp0Status,
        image: Mips4ExceptionImage,
        refill_address_mode: Option<Mips4TlbAddressMode>,
    ) -> u64;
}
