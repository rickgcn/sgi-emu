//! Architectural state owned by functional MIPS IV execution.

use core::fmt;

use crate::cpu::mips4::cache::Mips4CacheCoherenceAlgorithm;
use crate::cpu::mips4::cache::hierarchy::{Mips4CacheConfigError, Mips4CacheHierarchy};
use crate::cpu::mips4::config::Mips4Config;
use crate::cpu::mips4::cp0::Mips4Cp0;
use crate::cpu::mips4::cp1::Mips4Cp1;
use crate::cpu::mips4::gpr::Mips4GprFile;
use crate::cpu::mips4::memory::ll_sc::Mips4LlBit;
use crate::cpu::mips4::mmu::{Mips4Mmu, Mips4MmuAddressClassification};
use crate::cpu::mips4::tlb::{
    Mips4TlbAsid, Mips4TlbEntry, Mips4TlbEntryHi, Mips4TlbEntryLo, Mips4TlbPageMask,
    Mips4TlbPageSize,
};

use super::policy::Mips4ExecutionPolicy;

/// Invalid configuration supplied to functional MIPS IV execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4ExecutionConfigError {
    /// The implemented physical address width is outside the MIPS IV range.
    InvalidPhysicalAddressWidth {
        /// Invalid width in bits.
        bits: u8,
    },

    /// The implemented virtual address width is outside the MIPS IV range.
    InvalidVirtualAddressWidth {
        /// Invalid width in bits.
        bits: u8,
    },

    /// The configured functional cache hierarchy is invalid.
    Cache(Mips4CacheConfigError),
}

impl fmt::Display for Mips4ExecutionConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPhysicalAddressWidth { bits } => {
                write!(f, "invalid MIPS IV physical address width: {bits}")
            }
            Self::InvalidVirtualAddressWidth { bits } => {
                write!(f, "invalid MIPS IV virtual address width: {bits}")
            }
            Self::Cache(error) => error.fmt(f),
        }
    }
}

impl std::error::Error for Mips4ExecutionConfigError {}

/// Complete architectural state required by functional MIPS IV execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4ExecutionState {
    pub(super) config: Mips4Config,
    pub(super) pc: u64,
    pub(super) next_pc: u64,
    pub(super) delay_slot_branch_pc: Option<u64>,
    pub(super) gpr: Mips4GprFile,
    pub(super) hi: u64,
    pub(super) lo: u64,
    pub(super) cp0: Mips4Cp0,
    pub(super) cp1: Mips4Cp1,
    pub(super) tlb_entries: Vec<Mips4TlbEntry>,
    pub(super) llbit: Mips4LlBit,
    pub(super) external_interrupts: u8,
    pub(super) standby: bool,
    pub(super) cache: Mips4CacheHierarchy,
}

impl Mips4ExecutionState {
    /// Creates reset architectural state for one processor policy.
    pub fn new(policy: &impl Mips4ExecutionPolicy) -> Result<Self, Mips4ExecutionConfigError> {
        let config = policy.architecture_config();
        validate_architecture_config(config)?;
        let pc = policy.reset_pc();
        Ok(Self {
            config,
            pc,
            next_pc: pc.wrapping_add(4),
            delay_slot_branch_pc: None,
            gpr: Mips4GprFile::new(),
            hi: 0,
            lo: 0,
            cp0: Mips4Cp0::new(
                config.processor_id,
                policy.cp0_config(),
                policy.tlb_random_upper_bound(),
            ),
            cp1: Mips4Cp1::new(policy.fcr0()),
            tlb_entries: (0..policy.tlb_entry_count())
                .map(invalid_tlb_entry)
                .collect(),
            llbit: Mips4LlBit::Clear,
            external_interrupts: 0,
            standby: false,
            cache: Mips4CacheHierarchy::new(policy.cache_config())
                .map_err(Mips4ExecutionConfigError::Cache)?,
        })
    }

    /// Returns the architectural processor configuration.
    pub const fn config(&self) -> Mips4Config {
        self.config
    }

    /// Returns the current program counter.
    pub const fn pc(&self) -> u64 {
        self.pc
    }

    /// Returns the next program counter.
    pub const fn next_pc(&self) -> u64 {
        self.next_pc
    }

    /// Returns the branch instruction owning the current delay slot, if any.
    pub const fn delay_slot_branch_pc(&self) -> Option<u64> {
        self.delay_slot_branch_pc
    }

    /// Returns the general-purpose register file.
    pub const fn gpr(&self) -> &Mips4GprFile {
        &self.gpr
    }

    /// Returns `HI`.
    pub const fn hi(&self) -> u64 {
        self.hi
    }

    /// Returns `LO`.
    pub const fn lo(&self) -> u64 {
        self.lo
    }

    /// Returns CP0 state.
    pub const fn cp0(&self) -> &Mips4Cp0 {
        &self.cp0
    }

    /// Returns CP1 state.
    pub const fn cp1(&self) -> &Mips4Cp1 {
        &self.cp1
    }

    /// Returns the modeled TLB entries.
    pub fn tlb_entries(&self) -> &[Mips4TlbEntry] {
        &self.tlb_entries
    }

    /// Returns LLbit state.
    pub const fn llbit(&self) -> Mips4LlBit {
        self.llbit
    }

    /// Returns raw external interrupt lines as Cause IP field bits.
    pub const fn external_interrupts(&self) -> u8 {
        self.external_interrupts
    }

    pub(super) fn deterministic_tlb_entries(
        &self,
        policy: &impl Mips4ExecutionPolicy,
        address: u64,
    ) -> &[Mips4TlbEntry] {
        let Mips4MmuAddressClassification::Mapped { address_mode, .. } =
            Mips4Mmu::classify_virtual_address(
                policy.mmu_config(self.cp0.config()),
                self.cp0.status(),
                address,
            )
        else {
            return &self.tlb_entries;
        };
        let asid = Mips4TlbAsid::new(self.cp0.entry_hi().address_space_identifier());
        let first = self
            .tlb_entries
            .iter()
            .position(|entry| entry.matches_virtual_address(address, asid, address_mode));
        match first {
            Some(index) => &self.tlb_entries[..=index],
            None => &self.tlb_entries,
        }
    }
}

fn validate_architecture_config(config: Mips4Config) -> Result<(), Mips4ExecutionConfigError> {
    if !(32..=36).contains(&config.address.physical_address_bits) {
        return Err(Mips4ExecutionConfigError::InvalidPhysicalAddressWidth {
            bits: config.address.physical_address_bits,
        });
    }
    if !(32..=40).contains(&config.address.virtual_address_bits) {
        return Err(Mips4ExecutionConfigError::InvalidVirtualAddressWidth {
            bits: config.address.virtual_address_bits,
        });
    }
    Ok(())
}

fn invalid_tlb_entry(index: usize) -> Mips4TlbEntry {
    let page_mask = Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size4KiB);
    let entry_hi = Mips4TlbEntryHi::from_parts(index as u64, Mips4TlbAsid::new(0xff), 0).unwrap();
    let entry_lo = Mips4TlbEntryLo::from_parts(
        0,
        Mips4CacheCoherenceAlgorithm::from_bits(0).unwrap(),
        false,
        false,
        false,
    )
    .unwrap();
    Mips4TlbEntry::new(page_mask, entry_hi, entry_lo, entry_lo)
}

#[cfg(test)]
mod tests {
    use crate::cpu::mips4::config::{
        Mips4AddressConfig, Mips4CacheConfig, Mips4CoprocessorConfig, Mips4Endianness,
    };

    use super::*;

    fn config(physical_address_bits: u8, virtual_address_bits: u8) -> Mips4Config {
        Mips4Config::new(
            Mips4Endianness::Big,
            0x2300,
            Mips4AddressConfig::new(physical_address_bits, virtual_address_bits),
            Mips4CacheConfig::disabled(),
            Mips4CacheConfig::disabled(),
            Mips4CacheConfig::disabled(),
            Mips4CoprocessorConfig::new(true, false),
        )
    }

    #[test]
    fn architecture_address_widths_are_validated_at_execution_construction() {
        assert_eq!(validate_architecture_config(config(32, 32)), Ok(()));
        assert_eq!(validate_architecture_config(config(36, 40)), Ok(()));
        assert_eq!(
            validate_architecture_config(config(31, 40)),
            Err(Mips4ExecutionConfigError::InvalidPhysicalAddressWidth { bits: 31 })
        );
        assert_eq!(
            validate_architecture_config(config(37, 40)),
            Err(Mips4ExecutionConfigError::InvalidPhysicalAddressWidth { bits: 37 })
        );
        assert_eq!(
            validate_architecture_config(config(36, 31)),
            Err(Mips4ExecutionConfigError::InvalidVirtualAddressWidth { bits: 31 })
        );
        assert_eq!(
            validate_architecture_config(config(36, 41)),
            Err(Mips4ExecutionConfigError::InvalidVirtualAddressWidth { bits: 41 })
        );
    }
}
