use se_core::bus::PhysAddr;

const TLB_ENTRY_COUNT: usize = 64;

const KUSEG_END: u32 = 0x7fff_ffff;
const KSEG0_START: u32 = 0x8000_0000;
const KSEG0_END: u32 = 0x9fff_ffff;
const KSEG1_START: u32 = 0xa000_0000;
const KSEG1_END: u32 = 0xbfff_ffff;
const PHYSICAL_ADDRESS_MASK: u32 = 0x1fff_ffff;

const ENTRY_HI_VPN_MASK: u32 = 0xffff_f000;
const ENTRY_HI_ASID_MASK: u32 = 0x0000_0fc0;
const ENTRY_LO_PFN_MASK: u32 = 0xffff_f000;
const ENTRY_LO_NONCACHEABLE: u32 = 1 << 11;
const ENTRY_LO_DIRTY: u32 = 1 << 10;
const ENTRY_LO_VALID: u32 = 1 << 9;
const ENTRY_LO_GLOBAL: u32 = 1 << 8;
const ENTRY_LO_MASK: u32 =
    ENTRY_LO_PFN_MASK | ENTRY_LO_NONCACHEABLE | ENTRY_LO_DIRTY | ENTRY_LO_VALID | ENTRY_LO_GLOBAL;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AccessType {
    Instruction,
    #[allow(
        dead_code,
        reason = "The MMU contract includes data-load translation semantics"
    )]
    Load,
    Store,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Cacheability {
    Cached,
    Uncached,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Translation {
    pub(super) address: PhysAddr,
    pub(super) cacheability: Cacheability,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum TranslationFault {
    AddressError,
    Miss,
    Invalid,
    Modified,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProbeResult {
    Miss,
    Match(usize),
    Shutdown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TlbEntry {
    entry_hi: u32,
    entry_lo: u32,
}

impl TlbEntry {
    const ZERO: Self = Self {
        entry_hi: 0,
        entry_lo: 0,
    };

    const fn new(entry_hi: u32, entry_lo: u32) -> Self {
        Self {
            entry_hi: entry_hi & (ENTRY_HI_VPN_MASK | ENTRY_HI_ASID_MASK),
            entry_lo: entry_lo & ENTRY_LO_MASK,
        }
    }

    fn tag_matches(self, virtual_page: u32, asid: u32) -> bool {
        self.entry_hi & ENTRY_HI_VPN_MASK == virtual_page
            && (self.entry_lo & ENTRY_LO_GLOBAL != 0 || self.entry_hi & ENTRY_HI_ASID_MASK == asid)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingTlbWrite {
    index: usize,
    entry: TlbEntry,
}

pub(super) struct Mmu {
    entries: [TlbEntry; TLB_ENTRY_COUNT],
    instruction_entries: [TlbEntry; TLB_ENTRY_COUNT],
    pending_instruction_writes: [Option<PendingTlbWrite>; 2],
}

impl Mmu {
    pub(super) const fn new() -> Self {
        Self {
            entries: [TlbEntry::ZERO; TLB_ENTRY_COUNT],
            instruction_entries: [TlbEntry::ZERO; TLB_ENTRY_COUNT],
            pending_instruction_writes: [None; 2],
        }
    }

    pub(super) fn reset(&mut self) {
        self.instruction_entries = self.entries;
        self.pending_instruction_writes = [None; 2];
    }

    pub(super) fn translate(
        &self,
        virtual_address: u32,
        asid: u8,
        kernel_mode: bool,
        access: AccessType,
    ) -> Result<Translation, TranslationFault> {
        match virtual_address {
            0..=KUSEG_END => self.translate_mapped(virtual_address, asid, access),
            KSEG0_START..=KSEG0_END if kernel_mode => Ok(Translation {
                address: PhysAddr::new(u64::from(virtual_address & PHYSICAL_ADDRESS_MASK)),
                cacheability: Cacheability::Cached,
            }),
            KSEG1_START..=KSEG1_END if kernel_mode => Ok(Translation {
                address: PhysAddr::new(u64::from(virtual_address & PHYSICAL_ADDRESS_MASK)),
                cacheability: Cacheability::Uncached,
            }),
            KSEG0_START..=KSEG1_END => Err(TranslationFault::AddressError),
            _ if kernel_mode => self.translate_mapped(virtual_address, asid, access),
            _ => Err(TranslationFault::AddressError),
        }
    }

    pub(super) fn read_indexed(&self, index: usize) -> (u32, u32) {
        let entry = self.entries[index];
        (entry.entry_hi, entry.entry_lo)
    }

    pub(super) fn probe(&self, entry_hi: u32) -> ProbeResult {
        let virtual_page = entry_hi & ENTRY_HI_VPN_MASK;
        let asid = entry_hi & ENTRY_HI_ASID_MASK;
        let mut matching_index = None;

        for (index, entry) in self.entries.iter().copied().enumerate() {
            if entry.tag_matches(virtual_page, asid) {
                if matching_index.is_some() {
                    return ProbeResult::Shutdown;
                }
                matching_index = Some(index);
            }
        }

        matching_index.map_or(ProbeResult::Miss, ProbeResult::Match)
    }

    pub(super) fn advance_instruction_view(&mut self) {
        self.advance_instruction_view_with(None);
    }

    pub(super) fn complete_write(&mut self, index: usize, entry_hi: u32, entry_lo: u32) {
        let entry = TlbEntry::new(entry_hi, entry_lo);
        self.entries[index] = entry;
        self.advance_instruction_view_with(Some(PendingTlbWrite { index, entry }));
    }

    fn translate_mapped(
        &self,
        virtual_address: u32,
        asid: u8,
        access: AccessType,
    ) -> Result<Translation, TranslationFault> {
        let entries = match access {
            AccessType::Instruction => &self.instruction_entries,
            AccessType::Load | AccessType::Store => &self.entries,
        };
        let virtual_page = virtual_address & ENTRY_HI_VPN_MASK;
        let asid = (u32::from(asid) << 6) & ENTRY_HI_ASID_MASK;
        let mut matching_entry = None;

        for entry in entries.iter().copied() {
            if entry.tag_matches(virtual_page, asid) {
                if matching_entry.is_some() {
                    return Err(TranslationFault::Shutdown);
                }
                matching_entry = Some(entry);
            }
        }

        let entry = matching_entry.ok_or(TranslationFault::Miss)?;
        if entry.entry_lo & ENTRY_LO_VALID == 0 {
            return Err(TranslationFault::Invalid);
        }
        if access == AccessType::Store && entry.entry_lo & ENTRY_LO_DIRTY == 0 {
            return Err(TranslationFault::Modified);
        }

        let physical_address =
            (entry.entry_lo & ENTRY_LO_PFN_MASK) | (virtual_address & !ENTRY_HI_VPN_MASK);
        let cacheability = if entry.entry_lo & ENTRY_LO_NONCACHEABLE == 0 {
            Cacheability::Cached
        } else {
            Cacheability::Uncached
        };
        Ok(Translation {
            address: PhysAddr::new(u64::from(physical_address)),
            cacheability,
        })
    }

    fn advance_instruction_view_with(&mut self, new_write: Option<PendingTlbWrite>) {
        if let Some(write) = self.pending_instruction_writes[0] {
            self.instruction_entries[write.index] = write.entry;
        }
        self.pending_instruction_writes[0] = self.pending_instruction_writes[1];
        self.pending_instruction_writes[1] = new_write;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccessType, Cacheability, ENTRY_HI_ASID_MASK, ENTRY_HI_VPN_MASK, ENTRY_LO_DIRTY,
        ENTRY_LO_GLOBAL, ENTRY_LO_NONCACHEABLE, ENTRY_LO_VALID, Mmu, ProbeResult, TLB_ENTRY_COUNT,
        Translation, TranslationFault,
    };
    use se_core::bus::PhysAddr;

    const ASID: u8 = 0x15;

    fn entry_hi(virtual_address: u32, asid: u8) -> u32 {
        (virtual_address & ENTRY_HI_VPN_MASK) | (u32::from(asid) << 6)
    }

    fn entry_lo(physical_address: u32, flags: u32) -> u32 {
        (physical_address & 0xffff_f000) | flags
    }

    fn translation(address: u64, cacheability: Cacheability) -> Translation {
        Translation {
            address: PhysAddr::new(address),
            cacheability,
        }
    }

    fn complete_and_sync(
        mmu: &mut Mmu,
        index: usize,
        virtual_address: u32,
        physical_address: u32,
        flags: u32,
    ) {
        mmu.complete_write(
            index,
            entry_hi(virtual_address, ASID),
            entry_lo(physical_address, flags),
        );
        mmu.advance_instruction_view();
        mmu.advance_instruction_view();
    }

    #[test]
    fn new_initializes_zero_entries_and_duplicate_zero_tags() {
        let mmu = Mmu::new();

        for index in 0..TLB_ENTRY_COUNT {
            assert_eq!(mmu.read_indexed(index), (0, 0));
        }
        assert_eq!(
            mmu.translate(0, 0, true, AccessType::Load),
            Err(TranslationFault::Shutdown)
        );
        assert_eq!(mmu.probe(0), ProbeResult::Shutdown);
    }

    #[test]
    fn translates_kernel_direct_segments_and_rejects_user_access() {
        let mmu = Mmu::new();

        for (virtual_address, physical_address, cacheability) in [
            (0x8000_0000, 0, Cacheability::Cached),
            (0x9fff_ffff, 0x1fff_ffff, Cacheability::Cached),
            (0xa000_0000, 0, Cacheability::Uncached),
            (0xbfc0_0000, 0x1fc0_0000, Cacheability::Uncached),
            (0xbfff_ffff, 0x1fff_ffff, Cacheability::Uncached),
        ] {
            assert_eq!(
                mmu.translate(virtual_address, 0, true, AccessType::Instruction),
                Ok(translation(physical_address, cacheability))
            );
        }

        for (virtual_address, access) in [
            (0x8000_0000, AccessType::Instruction),
            (0xa000_0000, AccessType::Load),
            (0xc000_0000, AccessType::Store),
            (0xffff_ffff, AccessType::Instruction),
        ] {
            assert_eq!(
                mmu.translate(virtual_address, 0, false, access),
                Err(TranslationFault::AddressError)
            );
        }
    }

    #[test]
    fn translates_mapped_segments_with_asid_global_and_page_offset() {
        let mut mmu = Mmu::new();
        let kuseg_address = 0x1234_5abc;
        let kseg2_address = 0xc234_5def;

        mmu.complete_write(
            3,
            entry_hi(kuseg_address, ASID),
            entry_lo(0x89ab_c000, ENTRY_LO_VALID | ENTRY_LO_DIRTY),
        );
        mmu.complete_write(
            4,
            entry_hi(kseg2_address, 0),
            entry_lo(
                0x7654_3000,
                ENTRY_LO_VALID | ENTRY_LO_DIRTY | ENTRY_LO_GLOBAL | ENTRY_LO_NONCACHEABLE,
            ),
        );

        assert_eq!(
            mmu.translate(kuseg_address, ASID, true, AccessType::Load),
            Ok(translation(0x89ab_cabc, Cacheability::Cached))
        );
        assert_eq!(
            mmu.translate(kuseg_address, ASID ^ 1, true, AccessType::Load),
            Err(TranslationFault::Miss)
        );
        assert_eq!(
            mmu.translate(kseg2_address, 0x3f, true, AccessType::Store),
            Ok(translation(0x7654_3def, Cacheability::Uncached))
        );
        assert_eq!(
            mmu.translate(kseg2_address, ASID, false, AccessType::Load),
            Err(TranslationFault::AddressError)
        );
    }

    #[test]
    fn mapped_cacheability_is_shared_by_all_access_types() {
        let mut mmu = Mmu::new();
        let cached_address = 0x1234_5000;
        let uncached_address = 0x2345_6000;
        complete_and_sync(
            &mut mmu,
            5,
            cached_address,
            0x1000_0000,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY,
        );
        complete_and_sync(
            &mut mmu,
            6,
            uncached_address,
            0x2000_0000,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY | ENTRY_LO_NONCACHEABLE,
        );

        for access in [AccessType::Instruction, AccessType::Load, AccessType::Store] {
            assert_eq!(
                mmu.translate(cached_address, ASID, true, access),
                Ok(translation(0x1000_0000, Cacheability::Cached))
            );
            assert_eq!(
                mmu.translate(uncached_address, ASID, true, access),
                Ok(translation(0x2000_0000, Cacheability::Uncached))
            );
        }
    }

    #[test]
    fn translates_mapped_segment_boundaries() {
        let mut mmu = Mmu::new();
        let cases = [
            (0, 0x1000_0000),
            (0x7fff_ffff, 0x2000_0fff),
            (0xc000_0000, 0x3000_0000),
            (0xffff_ffff, 0x4000_0fff),
        ];

        for (index, (virtual_address, physical_address)) in cases.into_iter().enumerate() {
            mmu.complete_write(
                index,
                entry_hi(virtual_address, ASID),
                entry_lo(physical_address, ENTRY_LO_VALID | ENTRY_LO_DIRTY),
            );
            assert_eq!(
                mmu.translate(virtual_address, ASID, true, AccessType::Load),
                Ok(translation(
                    u64::from(physical_address),
                    Cacheability::Cached
                ))
            );
        }
    }

    #[test]
    fn distinguishes_invalid_and_modified_and_ignores_dirty_for_reads() {
        let mut mmu = Mmu::new();
        let invalid_address = 0x2345_6000;
        let clean_address = 0x3456_7000;

        mmu.complete_write(
            1,
            entry_hi(invalid_address, ASID),
            entry_lo(0x1000_0000, ENTRY_LO_DIRTY),
        );
        complete_and_sync(&mut mmu, 2, clean_address, 0x2000_0000, ENTRY_LO_VALID);

        assert_eq!(
            mmu.translate(invalid_address, ASID, true, AccessType::Load),
            Err(TranslationFault::Invalid)
        );
        assert_eq!(
            mmu.translate(clean_address, ASID, true, AccessType::Load),
            Ok(translation(0x2000_0000, Cacheability::Cached))
        );
        assert_eq!(
            mmu.translate(clean_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x2000_0000, Cacheability::Cached))
        );
        assert_eq!(
            mmu.translate(clean_address, ASID, true, AccessType::Store),
            Err(TranslationFault::Modified)
        );
    }

    #[test]
    fn duplicate_tags_shutdown_before_validity_checks() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x4567_8000;
        let high = entry_hi(virtual_address, ASID);

        mmu.complete_write(6, high, 0);
        mmu.complete_write(7, high, 0);

        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Load),
            Err(TranslationFault::Shutdown)
        );
        assert_eq!(mmu.probe(high), ProbeResult::Shutdown);
    }

    #[test]
    fn probe_uses_main_tags_and_ignores_validity() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x5678_9000;
        let high = entry_hi(virtual_address, ASID);

        assert_eq!(mmu.probe(high), ProbeResult::Miss);

        mmu.complete_write(37, high, ENTRY_LO_NONCACHEABLE);

        assert_eq!(mmu.probe(high), ProbeResult::Match(37));
        assert_eq!(
            mmu.probe(entry_hi(virtual_address, ASID ^ 1)),
            ProbeResult::Miss
        );

        mmu.complete_write(37, high, ENTRY_LO_NONCACHEABLE | ENTRY_LO_GLOBAL);
        assert_eq!(
            mmu.probe(entry_hi(virtual_address, ASID ^ 1)),
            ProbeResult::Match(37)
        );
    }

    #[test]
    fn indexed_write_masks_reserved_bits() {
        let mut mmu = Mmu::new();

        mmu.complete_write(63, u32::MAX, u32::MAX);

        assert_eq!(
            mmu.read_indexed(63),
            (ENTRY_HI_VPN_MASK | ENTRY_HI_ASID_MASK, 0xffff_ff00)
        );
    }

    #[test]
    fn instruction_view_observes_two_completed_instruction_delay() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x6789_a123;
        let high = entry_hi(virtual_address, ASID);
        let old_low = entry_lo(0x1111_1000, ENTRY_LO_VALID | ENTRY_LO_DIRTY);
        let new_low = entry_lo(0x2222_2000, ENTRY_LO_VALID | ENTRY_LO_DIRTY);

        mmu.complete_write(8, high, old_low);
        mmu.advance_instruction_view();
        mmu.advance_instruction_view();
        mmu.complete_write(8, high, new_low);

        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Load),
            Ok(translation(0x2222_2123, Cacheability::Cached))
        );
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x1111_1123, Cacheability::Cached))
        );

        mmu.advance_instruction_view();
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x1111_1123, Cacheability::Cached))
        );

        mmu.advance_instruction_view();
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x2222_2123, Cacheability::Cached))
        );
    }

    #[test]
    fn consecutive_writes_reach_instruction_view_in_order() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x789a_b000;
        let high = entry_hi(virtual_address, ASID);

        complete_and_sync(&mut mmu, 9, virtual_address, 0x1000_0000, ENTRY_LO_VALID);
        mmu.complete_write(9, high, entry_lo(0x2000_0000, ENTRY_LO_VALID));
        mmu.complete_write(9, high, entry_lo(0x3000_0000, ENTRY_LO_VALID));

        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x1000_0000, Cacheability::Cached))
        );
        mmu.advance_instruction_view();
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x2000_0000, Cacheability::Cached))
        );
        mmu.advance_instruction_view();
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x3000_0000, Cacheability::Cached))
        );
    }

    #[test]
    fn reset_preserves_main_entries_and_synchronizes_instruction_view() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x7abc_d000;
        let high = entry_hi(virtual_address, ASID);
        let low = entry_lo(0x4000_0000, ENTRY_LO_VALID | ENTRY_LO_DIRTY);

        mmu.complete_write(10, high, low);
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Err(TranslationFault::Miss)
        );

        mmu.reset();

        assert_eq!(mmu.read_indexed(10), (high, low));
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
            Ok(translation(0x4000_0000, Cacheability::Cached))
        );
        assert_eq!(mmu.pending_instruction_writes, [None; 2]);
    }
}
