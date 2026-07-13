//! Generic MIPS IV TLB entry and translation helpers.
//!
//! This module models PageMask encodings, EntryHi and EntryLo fields,
//! ASID/global matching, page-pair selection, TLB probing, and pure translation
//! result classification. It does not own CP0 registers, implement Random or
//! Wired replacement policy, validate virtual address permissions, translate
//! unmapped segments, enter exceptions, or define an implementation-specific TLB
//! size.

use crate::cpu::mips4::cache::Mips4CacheCoherenceAlgorithm;
use crate::cpu::mips4::exception::Mips4Exception;

const ENTRY_HI_VPN2_SHIFT: u8 = 13;
const ENTRY_HI_VPN2_32_BITS: u8 = 19;
const ENTRY_HI_VPN2_64_BITS: u8 = 27;
const ENTRY_HI_VPN2_32_MASK: u64 = (1 << ENTRY_HI_VPN2_32_BITS) - 1;
const ENTRY_HI_VPN2_64_MASK: u64 = (1 << ENTRY_HI_VPN2_64_BITS) - 1;
const ENTRY_LO_PFN_BITS: u8 = 24;
const ENTRY_LO_PFN_MASK: u64 = (1 << ENTRY_LO_PFN_BITS) - 1;
const ENTRY_LO_CCA_SHIFT: u8 = 3;
const ENTRY_LO_DIRTY_BIT: u64 = 1 << 2;
const ENTRY_LO_VALID_BIT: u64 = 1 << 1;
const ENTRY_LO_GLOBAL_BIT: u64 = 1;
const ENTRY_LO_DEFINED_BITS: u64 = (1 << 30) - 1;
const PAGE_MASK_SHIFT: u8 = 13;
const REGION_BITS_SHIFT: u8 = 62;
const REGION_BITS_MASK: u8 = 0x03;

/// TLB address comparison mode.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4TlbAddressMode {
    /// 32-bit virtual address comparison.
    Bits32,

    /// 64-bit virtual address comparison.
    Bits64,
}

impl Mips4TlbAddressMode {
    const fn vpn2_mask(self) -> u64 {
        match self {
            Self::Bits32 => ENTRY_HI_VPN2_32_MASK,
            Self::Bits64 => ENTRY_HI_VPN2_64_MASK,
        }
    }

    const fn uses_region_bits(self) -> bool {
        matches!(self, Self::Bits64)
    }
}

/// TLB ASID field.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4TlbAsid(u8);

impl Mips4TlbAsid {
    /// Creates an 8-bit ASID.
    pub const fn new(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the raw ASID bits.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// MIPS IV TLB page size.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4TlbPageSize {
    /// 4 KiB page.
    Size4KiB,

    /// 16 KiB page.
    Size16KiB,

    /// 64 KiB page.
    Size64KiB,

    /// 256 KiB page.
    Size256KiB,

    /// 1 MiB page.
    Size1MiB,

    /// 4 MiB page.
    Size4MiB,

    /// 16 MiB page.
    Size16MiB,
}

impl Mips4TlbPageSize {
    /// Creates a page size from its byte count.
    pub const fn from_bytes(bytes: u64) -> Option<Self> {
        match bytes {
            0x0000_1000 => Some(Self::Size4KiB),
            0x0000_4000 => Some(Self::Size16KiB),
            0x0001_0000 => Some(Self::Size64KiB),
            0x0004_0000 => Some(Self::Size256KiB),
            0x0010_0000 => Some(Self::Size1MiB),
            0x0040_0000 => Some(Self::Size4MiB),
            0x0100_0000 => Some(Self::Size16MiB),
            _ => None,
        }
    }

    /// Returns the page size in bytes.
    pub const fn bytes(self) -> u64 {
        1_u64 << self.shift()
    }

    /// Returns the page offset width in bits.
    pub const fn shift(self) -> u8 {
        match self {
            Self::Size4KiB => 12,
            Self::Size16KiB => 14,
            Self::Size64KiB => 16,
            Self::Size256KiB => 18,
            Self::Size1MiB => 20,
            Self::Size4MiB => 22,
            Self::Size16MiB => 24,
        }
    }

    const fn page_mask_bits(self) -> u32 {
        match self {
            Self::Size4KiB => 0x0000_0000,
            _ => ((1_u32 << (self.shift() + 1)) - 1) & !((1_u32 << PAGE_MASK_SHIFT) - 1),
        }
    }
}

/// MIPS IV PageMask register value for a defined page size.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct Mips4TlbPageMask {
    bits: u32,
    page_size: Mips4TlbPageSize,
}

impl Mips4TlbPageMask {
    /// Creates a PageMask value from raw register bits.
    pub const fn from_bits(bits: u32) -> Option<Self> {
        match bits {
            0x0000_0000 => Some(Self::from_page_size(Mips4TlbPageSize::Size4KiB)),
            0x0000_6000 => Some(Self::from_page_size(Mips4TlbPageSize::Size16KiB)),
            0x0001_e000 => Some(Self::from_page_size(Mips4TlbPageSize::Size64KiB)),
            0x0007_e000 => Some(Self::from_page_size(Mips4TlbPageSize::Size256KiB)),
            0x001f_e000 => Some(Self::from_page_size(Mips4TlbPageSize::Size1MiB)),
            0x007f_e000 => Some(Self::from_page_size(Mips4TlbPageSize::Size4MiB)),
            0x01ff_e000 => Some(Self::from_page_size(Mips4TlbPageSize::Size16MiB)),
            _ => None,
        }
    }

    /// Creates a PageMask value for a page size.
    pub const fn from_page_size(page_size: Mips4TlbPageSize) -> Self {
        Self {
            bits: page_size.page_mask_bits(),
            page_size,
        }
    }

    /// Returns the raw PageMask register bits.
    pub const fn bits(self) -> u32 {
        self.bits
    }

    /// Returns the page size selected by this PageMask value.
    pub const fn page_size(self) -> Mips4TlbPageSize {
        self.page_size
    }

    /// Returns the byte mask for the page offset.
    pub const fn page_offset_mask(self) -> u64 {
        self.page_size.bytes() - 1
    }

    /// Returns the byte mask for the two-page pair covered by one TLB entry.
    pub const fn page_pair_offset_mask(self) -> u64 {
        (self.page_size.bytes() * 2) - 1
    }

    const fn vpn2_mask_bits(self) -> u64 {
        (self.bits >> PAGE_MASK_SHIFT) as u64
    }
}

/// MIPS IV TLB EntryHi fields.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct Mips4TlbEntryHi {
    vpn2: u64,
    asid: Mips4TlbAsid,
    region_bits: u8,
}

impl Mips4TlbEntryHi {
    /// Creates EntryHi fields from raw field values.
    pub const fn from_parts(vpn2: u64, asid: Mips4TlbAsid, region_bits: u8) -> Option<Self> {
        if vpn2 <= ENTRY_HI_VPN2_64_MASK && region_bits <= REGION_BITS_MASK {
            Some(Self {
                vpn2,
                asid,
                region_bits,
            })
        } else {
            None
        }
    }

    /// Creates canonical EntryHi fields for a virtual address and page mask.
    pub const fn from_virtual_address(
        address: u64,
        asid: Mips4TlbAsid,
        page_mask: Mips4TlbPageMask,
        address_mode: Mips4TlbAddressMode,
    ) -> Self {
        let vpn2 = ((address >> ENTRY_HI_VPN2_SHIFT) & address_mode.vpn2_mask())
            & !page_mask.vpn2_mask_bits();
        let region_bits = if address_mode.uses_region_bits() {
            ((address >> REGION_BITS_SHIFT) as u8) & REGION_BITS_MASK
        } else {
            0
        };

        Self {
            vpn2,
            asid,
            region_bits,
        }
    }

    /// Returns the raw VPN2 field.
    pub const fn vpn2(self) -> u64 {
        self.vpn2
    }

    /// Returns the ASID field.
    pub const fn asid(self) -> Mips4TlbAsid {
        self.asid
    }

    /// Returns the raw region field bits.
    pub const fn region_bits(self) -> u8 {
        self.region_bits
    }

    /// Returns whether the VPN fits an implemented virtual address width.
    pub const fn fits_virtual_address_bits(self, bits: u8) -> bool {
        let vpn_bits = bits.saturating_sub(ENTRY_HI_VPN2_SHIFT);
        vpn_bits >= 64 || self.vpn2 < (1_u64 << vpn_bits)
    }

    /// Returns whether this EntryHi value matches a virtual address.
    pub const fn matches_virtual_address(
        self,
        address: u64,
        page_mask: Mips4TlbPageMask,
        address_mode: Mips4TlbAddressMode,
    ) -> bool {
        let address_vpn2 = (address >> ENTRY_HI_VPN2_SHIFT) & address_mode.vpn2_mask();
        let masked_compare_bits = !page_mask.vpn2_mask_bits() & address_mode.vpn2_mask();
        let vpn_matches = (self.vpn2 ^ address_vpn2) & masked_compare_bits == 0;
        let region_matches = !address_mode.uses_region_bits()
            || self.region_bits == (((address >> REGION_BITS_SHIFT) as u8) & REGION_BITS_MASK);

        vpn_matches && region_matches
    }
}

/// MIPS IV TLB EntryLo fields.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub struct Mips4TlbEntryLo {
    pfn: u64,
    cache_coherence_algorithm: Mips4CacheCoherenceAlgorithm,
    dirty: bool,
    valid: bool,
    global: bool,
}

impl Mips4TlbEntryLo {
    /// Creates EntryLo fields from raw field values.
    pub const fn from_parts(
        pfn: u64,
        cache_coherence_algorithm: Mips4CacheCoherenceAlgorithm,
        dirty: bool,
        valid: bool,
        global: bool,
    ) -> Option<Self> {
        if pfn <= ENTRY_LO_PFN_MASK {
            Some(Self {
                pfn,
                cache_coherence_algorithm,
                dirty,
                valid,
                global,
            })
        } else {
            None
        }
    }

    /// Creates EntryLo fields from raw register bits.
    pub const fn from_bits(bits: u64) -> Option<Self> {
        if bits & !ENTRY_LO_DEFINED_BITS != 0 {
            return None;
        }

        let Some(cache_coherence_algorithm) =
            Mips4CacheCoherenceAlgorithm::from_bits(((bits >> ENTRY_LO_CCA_SHIFT) & 0x07) as u8)
        else {
            return None;
        };

        Some(Self {
            pfn: (bits >> 6) & ENTRY_LO_PFN_MASK,
            cache_coherence_algorithm,
            dirty: bits & ENTRY_LO_DIRTY_BIT != 0,
            valid: bits & ENTRY_LO_VALID_BIT != 0,
            global: bits & ENTRY_LO_GLOBAL_BIT != 0,
        })
    }

    /// Returns the raw EntryLo register bits.
    pub const fn bits(self) -> u64 {
        (self.pfn << 6)
            | ((self.cache_coherence_algorithm.bits() as u64) << ENTRY_LO_CCA_SHIFT)
            | if self.dirty { ENTRY_LO_DIRTY_BIT } else { 0 }
            | if self.valid { ENTRY_LO_VALID_BIT } else { 0 }
            | if self.global { ENTRY_LO_GLOBAL_BIT } else { 0 }
    }

    /// Returns the page frame number.
    pub const fn pfn(self) -> u64 {
        self.pfn
    }

    /// Returns the raw cache-coherence algorithm value.
    pub const fn cache_coherence_algorithm(self) -> Mips4CacheCoherenceAlgorithm {
        self.cache_coherence_algorithm
    }

    /// Returns whether this page is writable.
    pub const fn dirty(self) -> bool {
        self.dirty
    }

    /// Returns whether this page is valid.
    pub const fn valid(self) -> bool {
        self.valid
    }

    /// Returns whether this EntryLo half has the global bit set.
    pub const fn global(self) -> bool {
        self.global
    }

    /// Returns whether the PFN fits an implemented physical address width.
    pub const fn fits_physical_address_bits(self, bits: u8) -> bool {
        let pfn_bits = bits.saturating_sub(12);
        pfn_bits >= 64 || self.pfn < (1_u64 << pfn_bits)
    }

    /// Returns the physical page base for the supplied page mask.
    pub const fn physical_page_base(self, page_mask: Mips4TlbPageMask) -> u64 {
        (self.pfn << 12) & !page_mask.page_offset_mask()
    }
}

/// Page half selected within a TLB odd/even page pair.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4TlbPageHalf {
    /// Even virtual page.
    Even,

    /// Odd virtual page.
    Odd,
}

/// TLB access type for exception classification.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4TlbAccessKind {
    /// Instruction fetch.
    InstructionFetch,

    /// Data load.
    Load,

    /// Data store.
    Store,
}

impl Mips4TlbAccessKind {
    const fn invalid_or_miss_exception(self) -> Mips4Exception {
        match self {
            Self::InstructionFetch | Self::Load => Mips4Exception::TlbLoad,
            Self::Store => Mips4Exception::TlbStore,
        }
    }
}

/// Successful TLB translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4TlbTranslation {
    /// Translated physical address.
    pub physical_address: u64,

    /// Cache-coherence algorithm selected by the matching TLB entry.
    pub cache_coherence_algorithm: Mips4CacheCoherenceAlgorithm,

    /// Page size selected by the matching TLB entry.
    pub page_size: Mips4TlbPageSize,

    /// Selected page half within the odd/even pair.
    pub page_half: Mips4TlbPageHalf,
}

/// Pure TLB translation result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4TlbTranslationResult {
    /// No matching TLB entry was found.
    Miss {
        /// Exception that a CPU pipeline would raise for this access.
        exception: Mips4Exception,
    },

    /// A matching TLB entry was found, but its valid bit was clear.
    Invalid {
        /// Exception that a CPU pipeline would raise for this access.
        exception: Mips4Exception,
    },

    /// A store matched a valid entry whose dirty bit was clear.
    Modified {
        /// Exception that a CPU pipeline would raise for this access.
        exception: Mips4Exception,
    },

    /// Translation succeeded.
    Hit(Mips4TlbTranslation),
}

/// MIPS IV odd/even TLB entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4TlbEntry {
    page_mask: Mips4TlbPageMask,
    entry_hi: Mips4TlbEntryHi,
    even_page: Mips4TlbEntryLo,
    odd_page: Mips4TlbEntryLo,
}

impl Mips4TlbEntry {
    /// Creates a TLB entry from its component fields.
    pub const fn new(
        page_mask: Mips4TlbPageMask,
        entry_hi: Mips4TlbEntryHi,
        even_page: Mips4TlbEntryLo,
        odd_page: Mips4TlbEntryLo,
    ) -> Self {
        Self {
            page_mask,
            entry_hi,
            even_page,
            odd_page,
        }
    }

    /// Returns the PageMask value.
    pub const fn page_mask(self) -> Mips4TlbPageMask {
        self.page_mask
    }

    /// Returns the EntryHi fields.
    pub const fn entry_hi(self) -> Mips4TlbEntryHi {
        self.entry_hi
    }

    /// Returns the even EntryLo fields.
    pub const fn even_page(self) -> Mips4TlbEntryLo {
        self.even_page
    }

    /// Returns the odd EntryLo fields.
    pub const fn odd_page(self) -> Mips4TlbEntryLo {
        self.odd_page
    }

    /// Returns whether both EntryLo halves are global.
    pub const fn global(self) -> bool {
        self.even_page.global() && self.odd_page.global()
    }

    /// Returns the selected page half for a virtual address.
    pub const fn page_half(self, address: u64) -> Mips4TlbPageHalf {
        if address & self.page_mask.page_size().bytes() == 0 {
            Mips4TlbPageHalf::Even
        } else {
            Mips4TlbPageHalf::Odd
        }
    }

    /// Returns the selected EntryLo half for a virtual address.
    pub const fn selected_entry_lo(self, address: u64) -> Mips4TlbEntryLo {
        match self.page_half(address) {
            Mips4TlbPageHalf::Even => self.even_page,
            Mips4TlbPageHalf::Odd => self.odd_page,
        }
    }

    /// Returns whether this entry matches a virtual address and ASID.
    pub const fn matches_virtual_address(
        self,
        address: u64,
        asid: Mips4TlbAsid,
        address_mode: Mips4TlbAddressMode,
    ) -> bool {
        self.entry_hi
            .matches_virtual_address(address, self.page_mask, address_mode)
            && (self.global() || self.entry_hi.asid().bits() == asid.bits())
    }

    /// Translates a virtual address through this single TLB entry.
    pub const fn translate(
        self,
        address: u64,
        asid: Mips4TlbAsid,
        access_kind: Mips4TlbAccessKind,
        address_mode: Mips4TlbAddressMode,
    ) -> Mips4TlbTranslationResult {
        let exception = access_kind.invalid_or_miss_exception();

        if !self.matches_virtual_address(address, asid, address_mode) {
            return Mips4TlbTranslationResult::Miss { exception };
        }

        let entry_lo = self.selected_entry_lo(address);
        if !entry_lo.valid() {
            return Mips4TlbTranslationResult::Invalid { exception };
        }

        if matches!(access_kind, Mips4TlbAccessKind::Store) && !entry_lo.dirty() {
            return Mips4TlbTranslationResult::Modified {
                exception: Mips4Exception::TlbModification,
            };
        }

        let page_offset = address & self.page_mask.page_offset_mask();
        Mips4TlbTranslationResult::Hit(Mips4TlbTranslation {
            physical_address: entry_lo.physical_page_base(self.page_mask) | page_offset,
            cache_coherence_algorithm: entry_lo.cache_coherence_algorithm(),
            page_size: self.page_mask.page_size(),
            page_half: self.page_half(address),
        })
    }
}

/// Result of probing a TLB entry slice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4TlbProbeResult {
    /// No matching entry was found.
    Miss,

    /// Exactly one matching entry was found.
    Hit {
        /// Index of the matching entry.
        index: usize,
    },

    /// More than one matching entry was found.
    Multiple,
}

/// Stateless TLB helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4Tlb;

impl Mips4Tlb {
    /// Probes a slice of TLB entries for a virtual address and ASID.
    pub fn probe(
        entries: &[Mips4TlbEntry],
        address: u64,
        asid: Mips4TlbAsid,
        address_mode: Mips4TlbAddressMode,
    ) -> Mips4TlbProbeResult {
        let mut hit_index = None;

        for (index, entry) in entries.iter().enumerate() {
            if entry.matches_virtual_address(address, asid, address_mode) {
                if hit_index.is_some() {
                    return Mips4TlbProbeResult::Multiple;
                }

                hit_index = Some(index);
            }
        }

        match hit_index {
            Some(index) => Mips4TlbProbeResult::Hit { index },
            None => Mips4TlbProbeResult::Miss,
        }
    }
}

#[cfg(test)]
mod tests;
