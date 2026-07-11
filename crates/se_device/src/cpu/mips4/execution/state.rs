//! Architectural state owned by functional MIPS IV execution.

use crate::cpu::mips4::cache::Mips4CacheCoherenceAlgorithm;
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

/// Complete architectural state required by functional MIPS IV execution.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4ExecutionState {
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
}

impl Mips4ExecutionState {
    /// Creates reset architectural state for one processor policy.
    pub fn new(policy: &impl Mips4ExecutionPolicy) -> Self {
        let pc = policy.reset_pc();
        Self {
            pc,
            next_pc: pc.wrapping_add(4),
            delay_slot_branch_pc: None,
            gpr: Mips4GprFile::new(),
            hi: 0,
            lo: 0,
            cp0: Mips4Cp0::new(
                policy.processor_id(),
                policy.cp0_config(),
                policy.tlb_random_upper_bound(),
            ),
            cp1: Mips4Cp1::new(policy.fcr0()),
            tlb_entries: (0..policy.tlb_entry_count())
                .map(invalid_tlb_entry)
                .collect(),
            llbit: Mips4LlBit::Clear,
            external_interrupts: 0,
        }
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
