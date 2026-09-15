use se_core::bus::PhysAddr;
use serde::{Deserialize, Serialize};
use serde_big_array::BigArray;

const TLB_ENTRY_COUNT: usize = 64;
const TLB_LOOKUP_CACHE_ENTRY_COUNT: usize = TLB_ENTRY_COUNT;
const TLB_LOOKUP_CACHE_HASH_MULTIPLIER: u32 = 0x9e37_79b1;
const TLB_LOOKUP_CACHE_INDEX_SHIFT: u32 = u32::BITS - TLB_LOOKUP_CACHE_ENTRY_COUNT.ilog2();

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

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct TlbEntry {
    entry_hi: u32,
    entry_lo: u32,
}

impl TlbEntry {
    const INITIAL_INVALID: Self = Self {
        entry_hi: KSEG0_START,
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

/// Selects the lowest matching slot, tolerating duplicates with identical EntryLo values.
///
/// This compatibility rule does not establish how physical R3000 hardware resolves
/// duplicate tags. Stored EntryLo values contain only PFN, N, D, V, and G bits.
/// Every matching tag participates, including invalid entries; any differing
/// EntryLo causes shutdown before validity or write permission is checked.
/// Global entries may match with different EntryHi ASIDs. The lookup is read-only
/// and leaves every duplicate slot intact.
fn match_entries(
    entries: &[TlbEntry; TLB_ENTRY_COUNT],
    virtual_page: u32,
    asid: u32,
) -> ProbeResult {
    let mut matching_index: Option<usize> = None;

    for (index, entry) in entries.iter().enumerate() {
        if entry.tag_matches(virtual_page, asid) {
            if let Some(first) = matching_index {
                if entry.entry_lo != entries[first].entry_lo {
                    return ProbeResult::Shutdown;
                }
            } else {
                matching_index = Some(index);
            }
        }
    }

    matching_index.map_or(ProbeResult::Miss, ProbeResult::Match)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CachedProbeResult {
    Miss,
    Match(u8),
    Shutdown,
}

impl CachedProbeResult {
    fn from_probe_result(result: ProbeResult) -> Self {
        match result {
            ProbeResult::Miss => Self::Miss,
            ProbeResult::Match(index) => Self::Match(
                index
                    .try_into()
                    .expect("a TLB lookup index must fit in one byte"),
            ),
            ProbeResult::Shutdown => Self::Shutdown,
        }
    }

    const fn probe_result(self) -> ProbeResult {
        match self {
            Self::Miss => ProbeResult::Miss,
            Self::Match(index) => ProbeResult::Match(index as usize),
            Self::Shutdown => ProbeResult::Shutdown,
        }
    }
}

#[derive(Clone, Copy)]
struct CachedLookup {
    generation: u32,
    key: u32,
    result: CachedProbeResult,
}

impl CachedLookup {
    const EMPTY: Self = Self {
        generation: 0,
        key: 0,
        result: CachedProbeResult::Miss,
    };

    const fn matches(self, generation: u32, key: u32) -> bool {
        self.generation == generation && self.key == key
    }
}

struct TlbLookupCache {
    generation: u32,
    hot: CachedLookup,
    entries: [CachedLookup; TLB_LOOKUP_CACHE_ENTRY_COUNT],
}

impl TlbLookupCache {
    const fn new() -> Self {
        Self {
            generation: 1,
            hot: CachedLookup::EMPTY,
            entries: [CachedLookup::EMPTY; TLB_LOOKUP_CACHE_ENTRY_COUNT],
        }
    }

    fn resolve(
        &mut self,
        entries: &[TlbEntry; TLB_ENTRY_COUNT],
        virtual_page: u32,
        asid: u32,
    ) -> ProbeResult {
        let key = lookup_key(virtual_page, asid);
        if self.hot.matches(self.generation, key) {
            return self.hot.result.probe_result();
        }

        let index = lookup_cache_index(key);
        let cached = self.entries[index];
        if cached.matches(self.generation, key) {
            self.hot = cached;
            return cached.result.probe_result();
        }

        let result = match_entries(entries, virtual_page, asid);
        let cached = CachedLookup {
            generation: self.generation,
            key,
            result: CachedProbeResult::from_probe_result(result),
        };
        self.hot = cached;
        self.entries[index] = cached;
        result
    }

    fn invalidate(&mut self) {
        if let Some(generation) = self.generation.checked_add(1) {
            self.generation = generation;
        } else {
            self.generation = 1;
            self.hot = CachedLookup::EMPTY;
            self.entries.fill(CachedLookup::EMPTY);
        }
    }
}

impl Default for TlbLookupCache {
    fn default() -> Self {
        Self::new()
    }
}

fn lookup_key(virtual_page: u32, asid: u32) -> u32 {
    debug_assert_eq!(virtual_page & !ENTRY_HI_VPN_MASK, 0);
    debug_assert_eq!(asid & !ENTRY_HI_ASID_MASK, 0);
    (virtual_page >> 12) | (asid << 14)
}

fn lookup_cache_index(key: u32) -> usize {
    debug_assert!(TLB_LOOKUP_CACHE_ENTRY_COUNT.is_power_of_two());
    (key.wrapping_mul(TLB_LOOKUP_CACHE_HASH_MULTIPLIER) >> TLB_LOOKUP_CACHE_INDEX_SHIFT) as usize
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct PendingTlbWrite {
    index: usize,
    entry: TlbEntry,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Mmu {
    #[serde(with = "BigArray")]
    entries: [TlbEntry; TLB_ENTRY_COUNT],
    #[serde(with = "BigArray")]
    instruction_entries: [TlbEntry; TLB_ENTRY_COUNT],
    pending_instruction_writes: [Option<PendingTlbWrite>; 2],
    #[serde(skip)]
    main_lookup_cache: TlbLookupCache,
    #[serde(skip)]
    instruction_lookup_cache: TlbLookupCache,
}

impl Clone for Mmu {
    fn clone(&self) -> Self {
        Self {
            entries: self.entries,
            instruction_entries: self.instruction_entries,
            pending_instruction_writes: self.pending_instruction_writes,
            main_lookup_cache: TlbLookupCache::new(),
            instruction_lookup_cache: TlbLookupCache::new(),
        }
    }
}

impl Mmu {
    pub(super) const fn new() -> Self {
        Self {
            entries: [TlbEntry::INITIAL_INVALID; TLB_ENTRY_COUNT],
            instruction_entries: [TlbEntry::INITIAL_INVALID; TLB_ENTRY_COUNT],
            pending_instruction_writes: [None; 2],
            main_lookup_cache: TlbLookupCache::new(),
            instruction_lookup_cache: TlbLookupCache::new(),
        }
    }

    pub(super) fn reset(&mut self) {
        self.instruction_entries = self.entries;
        self.pending_instruction_writes = [None; 2];
        self.main_lookup_cache.invalidate();
        self.instruction_lookup_cache.invalidate();
    }

    pub(super) fn translate(
        &mut self,
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

    pub(super) fn translate_readonly(
        &self,
        virtual_address: u32,
        asid: u8,
        kernel_mode: bool,
        access: AccessType,
    ) -> Result<Translation, TranslationFault> {
        match virtual_address {
            0..=KUSEG_END => self.translate_mapped_readonly(virtual_address, asid, access),
            KSEG0_START..=KSEG0_END if kernel_mode => Ok(Translation {
                address: PhysAddr::new(u64::from(virtual_address & PHYSICAL_ADDRESS_MASK)),
                cacheability: Cacheability::Cached,
            }),
            KSEG1_START..=KSEG1_END if kernel_mode => Ok(Translation {
                address: PhysAddr::new(u64::from(virtual_address & PHYSICAL_ADDRESS_MASK)),
                cacheability: Cacheability::Uncached,
            }),
            KSEG0_START..=KSEG1_END => Err(TranslationFault::AddressError),
            _ if kernel_mode => self.translate_mapped_readonly(virtual_address, asid, access),
            _ => Err(TranslationFault::AddressError),
        }
    }

    pub(super) fn read_indexed(&self, index: usize) -> (u32, u32) {
        let entry = self.entries[index];
        (entry.entry_hi, entry.entry_lo)
    }

    pub(super) fn debug_entries(&self, instruction: bool) -> [(u32, u32); TLB_ENTRY_COUNT] {
        let entries = if instruction {
            &self.instruction_entries
        } else {
            &self.entries
        };
        entries.map(|entry| (entry.entry_hi, entry.entry_lo))
    }

    pub(super) fn probe(&self, entry_hi: u32) -> ProbeResult {
        let virtual_page = entry_hi & ENTRY_HI_VPN_MASK;
        let asid = entry_hi & ENTRY_HI_ASID_MASK;
        match_entries(&self.entries, virtual_page, asid)
    }

    pub(super) fn advance_instruction_view(&mut self) {
        self.advance_instruction_view_with(None);
    }

    pub(super) fn complete_write(&mut self, index: usize, entry_hi: u32, entry_lo: u32) {
        let entry = TlbEntry::new(entry_hi, entry_lo);
        self.entries[index] = entry;
        self.main_lookup_cache.invalidate();
        self.advance_instruction_view_with(Some(PendingTlbWrite { index, entry }));
    }

    fn translate_mapped(
        &mut self,
        virtual_address: u32,
        asid: u8,
        access: AccessType,
    ) -> Result<Translation, TranslationFault> {
        let virtual_page = virtual_address & ENTRY_HI_VPN_MASK;
        let asid = (u32::from(asid) << 6) & ENTRY_HI_ASID_MASK;
        let result = match access {
            AccessType::Instruction => {
                self.instruction_lookup_cache
                    .resolve(&self.instruction_entries, virtual_page, asid)
            }
            AccessType::Load | AccessType::Store => {
                self.main_lookup_cache
                    .resolve(&self.entries, virtual_page, asid)
            }
        };
        let entries = match access {
            AccessType::Instruction => &self.instruction_entries,
            AccessType::Load | AccessType::Store => &self.entries,
        };
        Self::finish_mapped_translation(entries, virtual_address, access, result)
    }

    fn translate_mapped_readonly(
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
        let result = match_entries(entries, virtual_page, asid);
        Self::finish_mapped_translation(entries, virtual_address, access, result)
    }

    fn finish_mapped_translation(
        entries: &[TlbEntry; TLB_ENTRY_COUNT],
        virtual_address: u32,
        access: AccessType,
        result: ProbeResult,
    ) -> Result<Translation, TranslationFault> {
        let entry = match result {
            ProbeResult::Miss => return Err(TranslationFault::Miss),
            ProbeResult::Match(index) => entries[index],
            ProbeResult::Shutdown => return Err(TranslationFault::Shutdown),
        };
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
            self.instruction_lookup_cache.invalidate();
        }
        self.pending_instruction_writes[0] = self.pending_instruction_writes[1];
        self.pending_instruction_writes[1] = new_write;
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AccessType, Cacheability, ENTRY_HI_ASID_MASK, ENTRY_HI_VPN_MASK, ENTRY_LO_DIRTY,
        ENTRY_LO_GLOBAL, ENTRY_LO_NONCACHEABLE, ENTRY_LO_VALID, KSEG0_START, Mmu, ProbeResult,
        TLB_ENTRY_COUNT, TLB_LOOKUP_CACHE_ENTRY_COUNT, Translation, TranslationFault,
        lookup_cache_index, lookup_key,
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

    fn next_random(state: &mut u32) -> u32 {
        *state ^= *state << 13;
        *state ^= *state >> 17;
        *state ^= *state << 5;
        *state
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
    fn new_initializes_safe_invalid_entries() {
        let mut mmu = Mmu::new();

        for index in 0..TLB_ENTRY_COUNT {
            assert_eq!(mmu.read_indexed(index), (KSEG0_START, 0));
        }
        for access in [AccessType::Load, AccessType::Instruction] {
            assert_eq!(
                mmu.translate(0, 0, true, access),
                Err(TranslationFault::Miss)
            );
        }
        assert_eq!(mmu.probe(0), ProbeResult::Miss);
    }

    #[test]
    fn translates_kernel_direct_segments_and_rejects_user_access() {
        let mut mmu = Mmu::new();

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
    fn equivalent_duplicates_preserve_translation_permissions_and_slots() {
        let virtual_address = 0x4567_8abc;
        let high = entry_hi(virtual_address, ASID);
        for flags in [
            0,
            ENTRY_LO_VALID,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY | ENTRY_LO_NONCACHEABLE,
        ] {
            let mut mmu = Mmu::new();
            let low = entry_lo(0x1234_5000, flags);
            for index in [44, 2, 63] {
                // Reserved bits must not affect equivalence after a TLB write.
                mmu.complete_write(index, high, low | index as u32);
                mmu.advance_instruction_view();
                mmu.advance_instruction_view();
                for access in [AccessType::Load, AccessType::Store, AccessType::Instruction] {
                    let expected = if flags & ENTRY_LO_VALID == 0 {
                        Err(TranslationFault::Invalid)
                    } else if access == AccessType::Store && flags & ENTRY_LO_DIRTY == 0 {
                        Err(TranslationFault::Modified)
                    } else {
                        Ok(translation(
                            0x1234_5abc,
                            if flags & ENTRY_LO_NONCACHEABLE == 0 {
                                Cacheability::Cached
                            } else {
                                Cacheability::Uncached
                            },
                        ))
                    };
                    assert_eq!(mmu.translate(virtual_address, ASID, true, access), expected);
                }
                assert_eq!(
                    mmu.probe(high),
                    ProbeResult::Match(if index == 44 { 44 } else { 2 })
                );
            }
            for index in [2, 44, 63] {
                assert_eq!(mmu.read_indexed(index), (high, low));
            }
        }
    }

    #[test]
    fn conflicting_duplicates_check_every_entry_lo_field_and_match() {
        let virtual_address = 0x4567_8000;
        let high = entry_hi(virtual_address, ASID);
        let low = entry_lo(0x1234_5000, ENTRY_LO_VALID | ENTRY_LO_DIRTY);
        for difference in [
            0x1000,
            ENTRY_LO_NONCACHEABLE,
            ENTRY_LO_DIRTY,
            ENTRY_LO_VALID,
            ENTRY_LO_GLOBAL,
        ] {
            for conflict_index in [2, 44, 63] {
                let mut mmu = Mmu::new();
                for index in [2, 44, 63] {
                    mmu.complete_write(
                        index,
                        high,
                        if index == conflict_index {
                            low ^ difference
                        } else {
                            low
                        },
                    );
                }
                mmu.advance_instruction_view();
                mmu.advance_instruction_view();
                for access in [AccessType::Load, AccessType::Store, AccessType::Instruction] {
                    assert_eq!(
                        mmu.translate(virtual_address, ASID, true, access),
                        Err(TranslationFault::Shutdown)
                    );
                }
                assert_eq!(mmu.probe(high), ProbeResult::Shutdown);
            }
        }
    }

    #[test]
    fn conflicting_invalid_duplicates_shutdown_before_validity_checks() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x4567_8000;
        let high = entry_hi(virtual_address, ASID);

        mmu.complete_write(6, high, 0);
        mmu.complete_write(7, high, ENTRY_LO_DIRTY);

        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Load),
            Err(TranslationFault::Shutdown)
        );
        assert_eq!(mmu.probe(high), ProbeResult::Shutdown);
    }

    #[test]
    fn duplicate_matching_respects_asids_and_global_entries() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x4567_8000;
        let low = entry_lo(0x1234_5000, ENTRY_LO_VALID | ENTRY_LO_DIRTY);
        for (index, asid) in [(2, ASID), (44, ASID ^ 1)] {
            mmu.complete_write(
                index,
                entry_hi(virtual_address, asid),
                low ^ if index == 44 { 0x1000 } else { 0 },
            );
        }
        assert_eq!(
            mmu.probe(entry_hi(virtual_address, ASID)),
            ProbeResult::Match(2)
        );
        assert_eq!(
            mmu.probe(entry_hi(virtual_address, ASID ^ 1)),
            ProbeResult::Match(44)
        );
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Load),
            Ok(translation(0x1234_5000, Cacheability::Cached))
        );
        assert_eq!(
            mmu.translate(virtual_address, ASID ^ 1, true, AccessType::Load),
            Ok(translation(0x1234_4000, Cacheability::Cached))
        );

        for (index, asid) in [(2, ASID), (44, ASID ^ 1)] {
            mmu.complete_write(
                index,
                entry_hi(virtual_address, asid),
                low | ENTRY_LO_GLOBAL,
            );
        }
        mmu.advance_instruction_view();
        mmu.advance_instruction_view();
        for asid in [ASID, ASID ^ 1, ASID ^ 2] {
            assert_eq!(
                mmu.probe(entry_hi(virtual_address, asid)),
                ProbeResult::Match(2)
            );
            for access in [AccessType::Load, AccessType::Store, AccessType::Instruction] {
                assert_eq!(
                    mmu.translate(virtual_address, asid, true, access),
                    Ok(translation(0x1234_5000, Cacheability::Cached))
                );
            }
        }
    }

    #[test]
    fn main_lookup_cache_follows_misses_writes_conflicts_and_recovery() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x1234_5abc;
        let high = entry_hi(virtual_address, ASID);

        for _ in 0..2 {
            assert_eq!(
                mmu.translate(virtual_address, ASID, true, AccessType::Load),
                Err(TranslationFault::Miss)
            );
        }

        mmu.complete_write(
            11,
            high,
            entry_lo(0x1000_0000, ENTRY_LO_VALID | ENTRY_LO_DIRTY),
        );
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Load),
            Ok(translation(0x1000_0abc, Cacheability::Cached))
        );

        mmu.complete_write(
            11,
            high,
            entry_lo(0x2000_0000, ENTRY_LO_VALID | ENTRY_LO_DIRTY),
        );
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Load),
            Ok(translation(0x2000_0abc, Cacheability::Cached))
        );

        mmu.complete_write(
            12,
            high,
            entry_lo(0x3000_0000, ENTRY_LO_VALID | ENTRY_LO_DIRTY),
        );
        for _ in 0..2 {
            assert_eq!(
                mmu.translate(virtual_address, ASID, true, AccessType::Load),
                Err(TranslationFault::Shutdown)
            );
        }

        mmu.complete_write(12, entry_hi(0x6000_0000, ASID), 0);
        assert_eq!(
            mmu.translate(virtual_address, ASID, true, AccessType::Load),
            Ok(translation(0x2000_0abc, Cacheability::Cached))
        );
    }

    #[test]
    fn direct_mapped_lookup_collisions_preserve_results() {
        let mut first_pages = [None; TLB_LOOKUP_CACHE_ENTRY_COUNT];
        let encoded_asid = u32::from(ASID) << 6;
        let mut collision = None;

        for page_number in 0..=TLB_LOOKUP_CACHE_ENTRY_COUNT as u32 {
            let virtual_page = page_number << 12;
            let index = lookup_cache_index(lookup_key(virtual_page, encoded_asid));
            if let Some(first_page) = first_pages[index] {
                collision = Some((first_page, virtual_page));
                break;
            }
            first_pages[index] = Some(virtual_page);
        }

        let (first_page, second_page) = collision.expect("a cache collision must exist");
        assert_eq!(
            lookup_cache_index(lookup_key(first_page, encoded_asid)),
            lookup_cache_index(lookup_key(second_page, encoded_asid))
        );

        let mut mmu = Mmu::new();
        mmu.complete_write(
            1,
            entry_hi(first_page, ASID),
            entry_lo(0x1000_0000, ENTRY_LO_VALID | ENTRY_LO_DIRTY),
        );
        mmu.complete_write(
            2,
            entry_hi(second_page, ASID),
            entry_lo(0x2000_0000, ENTRY_LO_VALID | ENTRY_LO_DIRTY),
        );

        for (virtual_page, physical_page) in [
            (first_page, 0x1000_0000),
            (second_page, 0x2000_0000),
            (first_page, 0x1000_0000),
            (second_page, 0x2000_0000),
        ] {
            assert_eq!(
                mmu.translate(virtual_page | 0xabc, ASID, true, AccessType::Load),
                Ok(translation(physical_page | 0xabc, Cacheability::Cached))
            );
        }
    }

    #[test]
    fn cached_translation_matches_canonical_scan_across_state_changes() {
        let mut mmu = Mmu::new();
        let mut random = 0x6d2b_79f5;

        for step in 0..8_192 {
            let operation = next_random(&mut random);
            match operation & 7 {
                0 | 1 => {
                    let virtual_page = ((operation >> 3) & 0x1f) << 12;
                    let virtual_address = if operation & (1 << 8) == 0 {
                        virtual_page
                    } else {
                        0xc000_0000 | virtual_page
                    };
                    let asid = ((operation >> 9) & 3) as u8;
                    let index = ((operation >> 11) as usize) & (TLB_ENTRY_COUNT - 1);
                    let flags = ((operation >> 17) & 0x0f) << 8;
                    let physical_address = next_random(&mut random) & 0xffff_f000;
                    mmu.complete_write(
                        index,
                        entry_hi(virtual_address, asid),
                        entry_lo(physical_address, flags),
                    );
                }
                2 | 3 => mmu.advance_instruction_view(),
                _ => {}
            }

            let query = next_random(&mut random);
            let virtual_page = ((query >> 8) & 0x1f) << 12;
            let page_offset = (query >> 20) & 0x0fff;
            let virtual_address = match query & 7 {
                0 => 0x8000_0000 | (query & 0x1fff_ffff),
                1 => 0xa000_0000 | (query & 0x1fff_ffff),
                2..=4 => 0xc000_0000 | virtual_page | page_offset,
                _ => virtual_page | page_offset,
            };
            let asid = ((query >> 3) & 3) as u8;
            let kernel_mode = query & (1 << 5) != 0;

            for access in [AccessType::Instruction, AccessType::Load, AccessType::Store] {
                let expected = mmu.translate_readonly(virtual_address, asid, kernel_mode, access);
                let actual = mmu.translate(virtual_address, asid, kernel_mode, access);
                assert_eq!(
                    actual, expected,
                    "translation mismatch at step {step}, address {virtual_address:#010x}, ASID {asid:#04x}, access {access:?}"
                );
            }
        }
    }

    #[test]
    fn cloning_discards_derived_lookup_history() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x3456_7abc;
        complete_and_sync(
            &mut mmu,
            5,
            virtual_address,
            0x1234_5000,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY,
        );
        for access in [AccessType::Instruction, AccessType::Load] {
            assert!(mmu.translate(virtual_address, ASID, true, access).is_ok());
        }
        assert_ne!(mmu.main_lookup_cache.hot.generation, 0);
        assert_ne!(mmu.instruction_lookup_cache.hot.generation, 0);

        let mut cloned = mmu.clone();

        assert_eq!(cloned.main_lookup_cache.hot.generation, 0);
        assert_eq!(cloned.instruction_lookup_cache.hot.generation, 0);
        assert!(
            cloned
                .main_lookup_cache
                .entries
                .iter()
                .all(|entry| entry.generation == 0)
        );
        assert!(
            cloned
                .instruction_lookup_cache
                .entries
                .iter()
                .all(|entry| entry.generation == 0)
        );
        for access in [AccessType::Instruction, AccessType::Load] {
            assert_eq!(
                cloned.translate(virtual_address, ASID, true, access),
                Ok(translation(0x1234_5abc, Cacheability::Cached))
            );
        }
    }

    #[test]
    fn duplicate_conflicts_follow_instruction_view_delay() {
        let mut mmu = Mmu::new();
        let virtual_address = 0x4567_8000;
        let high = entry_hi(virtual_address, ASID);
        let low = entry_lo(0x1234_5000, ENTRY_LO_VALID | ENTRY_LO_DIRTY);
        for index in [2, 44] {
            complete_and_sync(
                &mut mmu,
                index,
                virtual_address,
                0x1234_5000,
                ENTRY_LO_VALID | ENTRY_LO_DIRTY,
            );
        }
        for (new_low, main_result, instruction_result) in [
            (
                low ^ 0x1000,
                Err(TranslationFault::Shutdown),
                Ok(translation(0x1234_5000, Cacheability::Cached)),
            ),
            (
                low,
                Ok(translation(0x1234_5000, Cacheability::Cached)),
                Err(TranslationFault::Shutdown),
            ),
        ] {
            mmu.complete_write(44, high, new_low);
            for _ in 0..2 {
                assert_eq!(
                    mmu.translate(virtual_address, ASID, true, AccessType::Load),
                    main_result
                );
                assert_eq!(
                    mmu.probe(high),
                    if new_low == low {
                        ProbeResult::Match(2)
                    } else {
                        ProbeResult::Shutdown
                    }
                );
                assert_eq!(
                    mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
                    instruction_result
                );
                mmu.advance_instruction_view();
            }
            assert_eq!(
                mmu.translate(virtual_address, ASID, true, AccessType::Instruction),
                main_result
            );
        }
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
