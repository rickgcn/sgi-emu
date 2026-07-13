//! Processor-specific policy required by functional MIPS IV execution.

use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::cache::hierarchy::{Mips4CacheAccessPolicy, Mips4CacheHierarchyConfig};
use crate::cpu::mips4::config::Mips4Config;
use crate::cpu::mips4::cp0::{Mips4Cp0Config, Mips4Cp0Register, Mips4Cp0Status};
use crate::cpu::mips4::exception::{Mips4ErrorException, Mips4ExceptionImage};
use crate::cpu::mips4::instruction::decode::Mips4CpuInstruction;
use crate::cpu::mips4::mmu::{Mips4MmuCacheAttribute, Mips4MmuConfig};
use crate::cpu::mips4::tlb::Mips4TlbAddressMode;

/// Processor decision for the implementation-specific `WAIT` instruction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4Cp0WaitPolicy {
    /// Treat the encoding as a reserved instruction.
    ReservedInstruction,
    /// Retire the instruction without entering a low-power state.
    NoOperation,
    /// Retire the instruction and enter functional standby.
    Standby,
}

/// Direction of a CP0 doubleword register transfer.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4Cp0DoublewordTransferDirection {
    /// `DMFC0` transfers from CP0 to a general-purpose register.
    FromCp0,
    /// `DMTC0` transfers from a general-purpose register to CP0.
    ToCp0,
}

/// Processor decision for `DMFC0` or `DMTC0` after generic decoding checks.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4Cp0DoublewordTransferPolicy {
    /// Execute the full 64-bit transfer.
    Execute,
    /// Retire without modifying the destination.
    NoOperation,
    /// Raise a reserved-instruction exception.
    ReservedInstruction,
}

/// Processor decision for architectural prefetch instructions.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4PrefetchPolicy {
    /// Execute the prefetch through the functional cache hierarchy.
    Execute,
    /// Retire the prefetch without address translation or a memory transaction.
    NoOperation,
}

/// Processor decision for a word instruction with a `NotWordValue` operand.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4NotWordValuePolicy {
    /// Execute the instruction using the low 32 bits of each word operand.
    ExecuteLowWord,
    /// Retire the instruction without modifying architectural state.
    NoOperation,
}

/// Processor decision for transfers involving a reserved CP1 control register.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4ReservedCp1ControlPolicy {
    /// Return zero from `CFC1` and ignore `CTC1` writes.
    ReadZeroWriteIgnore,
    /// Raise the floating-point Unimplemented Operation exception.
    FloatingPointUnimplemented,
}

/// Processor implementation policy used by the generic MIPS IV execution target.
pub trait Mips4ExecutionPolicy {
    /// Returns the reset program counter.
    fn reset_pc(&self) -> u64;

    /// Returns the architectural processor configuration.
    fn architecture_config(&self) -> Mips4Config;

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

    /// Selects processor behavior for the implementation-specific `WAIT` instruction.
    fn cp0_wait_policy(&self) -> Mips4Cp0WaitPolicy;

    /// Validates a CP0 doubleword transfer for the current processor and mode.
    fn cp0_doubleword_transfer_policy(
        &self,
        direction: Mips4Cp0DoublewordTransferDirection,
        status: Mips4Cp0Status,
        register: Mips4Cp0Register,
    ) -> Mips4Cp0DoublewordTransferPolicy;

    /// Selects processor behavior for `PREF` and `PREFX`.
    fn prefetch_policy(&self) -> Mips4PrefetchPolicy {
        Mips4PrefetchPolicy::Execute
    }

    /// Selects processor behavior for an instruction with a `NotWordValue` operand.
    fn not_word_value_policy(&self, _instruction: Mips4CpuInstruction) -> Mips4NotWordValuePolicy {
        Mips4NotWordValuePolicy::NoOperation
    }

    /// Selects behavior for `CFC1` and `CTC1` naming a reserved control register.
    fn reserved_cp1_control_policy(&self, _register: u8) -> Mips4ReservedCp1ControlPolicy {
        Mips4ReservedCp1ControlPolicy::FloatingPointUnimplemented
    }

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

    /// Selects the vector for an exception that enters CP0 error level.
    fn error_exception_vector(
        &self,
        status_before_exception: Mips4Cp0Status,
        reason: Mips4ErrorException,
    ) -> u64;
}
