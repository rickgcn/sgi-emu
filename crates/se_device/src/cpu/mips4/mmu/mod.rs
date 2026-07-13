//! Generic MIPS IV memory-management unit helpers.
//!
//! This module classifies virtual address spaces and combines address-space
//! rules with the generic TLB translation primitives. It does not update CP0
//! exception registers, select exception vectors, execute TLB instructions,
//! manage TLB replacement policy, perform cache lookup, or initiate bus
//! transactions.

use crate::cpu::mips4::cache::{Mips4CacheCoherenceAlgorithm, Mips4MemoryAccessType};
use crate::cpu::mips4::config::Mips4AddressConfig;
use crate::cpu::mips4::cp0::{Mips4Cp0KernelUserMode, Mips4Cp0Status};
use crate::cpu::mips4::exception::Mips4Exception;
use crate::cpu::mips4::tlb::{
    Mips4Tlb, Mips4TlbAccessKind, Mips4TlbAddressMode, Mips4TlbAsid, Mips4TlbEntry,
    Mips4TlbPageHalf, Mips4TlbPageSize, Mips4TlbProbeResult, Mips4TlbTranslationResult,
};

const USEG_END: u64 = 0x0000_0000_7fff_ffff;
const XUSEG_END: u64 = 0x0000_00ff_ffff_ffff;
const XSSEG_START: u64 = 0x4000_0000_0000_0000;
const XSSEG_END: u64 = 0x4000_00ff_ffff_ffff;
const XKSEG_START: u64 = 0xc000_0000_0000_0000;
const XKSEG_END: u64 = 0xc000_00ff_7fff_ffff;
const KSEG0_START: u64 = 0xffff_ffff_8000_0000;
const KSEG0_END: u64 = 0xffff_ffff_9fff_ffff;
const KSEG1_START: u64 = 0xffff_ffff_a000_0000;
const KSEG1_END: u64 = 0xffff_ffff_bfff_ffff;
const KSSEG_START: u64 = 0xffff_ffff_c000_0000;
const KSSEG_END: u64 = 0xffff_ffff_dfff_ffff;
const KSEG3_START: u64 = 0xffff_ffff_e000_0000;
const KSEG3_END: u64 = 0xffff_ffff_ffff_ffff;
const XKPHYS_RESERVED_ADDRESS_BITS: u64 = 0x07ff_fff0_0000_0000;
const XKPHYS_PHYSICAL_ADDRESS_MASK: u64 = 0x0000_000f_ffff_ffff;
const XKPHYS_CCA_SHIFT: u8 = 59;

/// Generic MIPS IV MMU configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4MmuConfig {
    /// Implemented virtual and physical address widths.
    pub address: Mips4AddressConfig,

    /// Cache-coherence algorithm used for `kseg0` and `ckseg0` direct maps.
    pub kseg0_cache_coherence_algorithm: Mips4CacheCoherenceAlgorithm,
}

impl Mips4MmuConfig {
    /// Creates a generic MMU configuration.
    pub const fn new(
        address: Mips4AddressConfig,
        kseg0_cache_coherence_algorithm: Mips4CacheCoherenceAlgorithm,
    ) -> Self {
        Self {
            address,
            kseg0_cache_coherence_algorithm,
        }
    }
}

/// Effective privilege mode used by address translation.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4MmuPrivilegeMode {
    /// Kernel mode.
    Kernel,

    /// Supervisor mode.
    Supervisor,

    /// User mode.
    User,
}

impl Mips4MmuPrivilegeMode {
    /// Derives the effective privilege mode from CP0 `Status`.
    pub const fn from_status(status: Mips4Cp0Status) -> Option<Self> {
        if status.error_level() || status.exception_level() {
            return Some(Self::Kernel);
        }

        match status.kernel_user_mode() {
            Mips4Cp0KernelUserMode::Kernel => Some(Self::Kernel),
            Mips4Cp0KernelUserMode::Supervisor => Some(Self::Supervisor),
            Mips4Cp0KernelUserMode::User => Some(Self::User),
            Mips4Cp0KernelUserMode::Reserved => None,
        }
    }
}

/// MIPS IV virtual-address segment.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4MmuSegment {
    /// 32-bit user segment as viewed from user mode.
    Useg,

    /// 64-bit user segment as viewed from user mode.
    Xuseg,

    /// 32-bit user segment as viewed from supervisor mode.
    Suseg,

    /// 64-bit user segment as viewed from supervisor mode.
    Xsuseg,

    /// 32-bit supervisor segment.
    Sseg,

    /// 64-bit current supervisor segment.
    Xsseg,

    /// 64-bit supervisor compatibility segment.
    Csseg,

    /// 32-bit user segment as viewed from kernel mode.
    Kuseg,

    /// 64-bit user segment as viewed from kernel mode.
    Xkuseg,

    /// 64-bit supervisor segment as viewed from kernel mode.
    Xksseg,

    /// 64-bit unmapped physical kernel segment.
    Xkphys,

    /// 64-bit kernel mapped segment.
    Xkseg,

    /// 32-bit cached direct-mapped kernel segment.
    Kseg0,

    /// 32-bit uncached direct-mapped kernel segment.
    Kseg1,

    /// 32-bit supervisor segment as viewed from kernel mode.
    Ksseg,

    /// 32-bit kernel mapped segment.
    Kseg3,

    /// 64-bit cached direct-mapped kernel compatibility segment.
    Ckseg0,

    /// 64-bit uncached direct-mapped kernel compatibility segment.
    Ckseg1,

    /// 64-bit supervisor mapped kernel compatibility segment.
    Cksseg,

    /// 64-bit kernel mapped compatibility segment.
    Ckseg3,
}

/// Cache attribute selected by MMU translation.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4MmuCacheAttribute {
    /// Access bypasses the cache hierarchy.
    Uncached,

    /// Access uses a raw cache-coherence algorithm value.
    CacheCoherenceAlgorithm(Mips4CacheCoherenceAlgorithm),
}

impl Mips4MmuCacheAttribute {
    /// Returns whether this attribute is uncached.
    pub const fn is_uncached(self) -> bool {
        matches!(self, Self::Uncached)
    }

    /// Returns the raw cache-coherence algorithm value when present.
    pub const fn cache_coherence_algorithm(self) -> Option<Mips4CacheCoherenceAlgorithm> {
        match self {
            Self::Uncached => None,
            Self::CacheCoherenceAlgorithm(cache_coherence_algorithm) => {
                Some(cache_coherence_algorithm)
            }
        }
    }

    /// Converts this cache attribute to an architecture-level memory access type.
    ///
    /// The `Uncached` attribute resolves directly. A raw cache-coherence
    /// algorithm resolves to [`Mips4MemoryAccessType::ImplementationSpecific`]
    /// because the mapping from a raw 3-bit CCA value to a cached access type
    /// is processor-specific (MIPS IV manual section A.3). A processor model
    /// must refine such cached attributes to `CachedNoncoherent` or
    /// `CachedCoherent` before using them for access-type decisions.
    pub const fn memory_access_type(self) -> Mips4MemoryAccessType {
        match self {
            Self::Uncached => Mips4MemoryAccessType::Uncached,
            Self::CacheCoherenceAlgorithm(_) => Mips4MemoryAccessType::ImplementationSpecific,
        }
    }
}

/// Virtual-address classification before TLB lookup.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4MmuAddressClassification {
    /// Address belongs to a mapped segment.
    Mapped {
        /// Segment selected by the virtual address.
        segment: Mips4MmuSegment,

        /// TLB address comparison mode for this segment.
        address_mode: Mips4TlbAddressMode,
    },

    /// Address belongs to an unmapped direct segment.
    Unmapped {
        /// Segment selected by the virtual address.
        segment: Mips4MmuSegment,

        /// Directly translated physical address.
        physical_address: u64,

        /// Cache attribute selected by the segment.
        cache_attribute: Mips4MmuCacheAttribute,
    },

    /// Address is not valid for the current access mode.
    AddressError {
        /// Segment associated with the invalid address, when known.
        segment: Option<Mips4MmuSegment>,
    },

    /// CP0 `Status` encodes no valid effective privilege mode.
    InvalidMode,
}

/// Source of a successful MMU translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4MmuTranslationSource {
    /// Translation came from a mapped segment and a TLB hit.
    Mapped {
        /// TLB address comparison mode used for the lookup.
        address_mode: Mips4TlbAddressMode,

        /// Page size selected by the matching TLB entry.
        page_size: Mips4TlbPageSize,

        /// Selected page half within the odd/even pair.
        page_half: Mips4TlbPageHalf,
    },

    /// Translation came from an unmapped direct segment.
    Unmapped,
}

/// Successful MMU translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4MmuTranslation {
    /// Translated physical address.
    pub physical_address: u64,

    /// Segment selected by the virtual address.
    pub segment: Mips4MmuSegment,

    /// Translation source.
    pub source: Mips4MmuTranslationSource,

    /// Cache attribute selected by the segment or TLB entry.
    pub cache_attribute: Mips4MmuCacheAttribute,
}

/// MMU translation fault.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4MmuFault {
    /// Exception that a CPU pipeline would raise for this access.
    pub exception: Mips4Exception,

    /// Virtual address that failed translation.
    pub bad_virtual_address: u64,

    /// Segment selected by the virtual address, when known.
    pub segment: Option<Mips4MmuSegment>,

    /// TLB address comparison mode used before the fault, when applicable.
    pub address_mode: Option<Mips4TlbAddressMode>,
}

/// MMU translation result.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4MmuTranslationResult {
    /// Translation succeeded.
    Hit(Mips4MmuTranslation),

    /// Translation failed with an architectural exception.
    Fault(Mips4MmuFault),

    /// More than one TLB entry matched the mapped address.
    UndefinedMultipleTlbMatch {
        /// Segment selected by the virtual address.
        segment: Mips4MmuSegment,

        /// TLB address comparison mode used for the lookup.
        address_mode: Mips4TlbAddressMode,
    },
}

/// Stateless MIPS IV MMU helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4Mmu;

impl Mips4Mmu {
    /// Classifies a virtual address for the supplied CP0 `Status` value.
    pub const fn classify_virtual_address(
        config: Mips4MmuConfig,
        status: Mips4Cp0Status,
        address: u64,
    ) -> Mips4MmuAddressClassification {
        let Some(privilege_mode) = Mips4MmuPrivilegeMode::from_status(status) else {
            return Mips4MmuAddressClassification::InvalidMode;
        };

        if status.error_level() && address <= USEG_END {
            return Mips4MmuAddressClassification::Unmapped {
                segment: if status.user_64_bit_addressing() {
                    Mips4MmuSegment::Xkuseg
                } else {
                    Mips4MmuSegment::Kuseg
                },
                physical_address: address,
                cache_attribute: Mips4MmuCacheAttribute::Uncached,
            };
        }

        match privilege_mode {
            Mips4MmuPrivilegeMode::User => classify_user(config, status, address),
            Mips4MmuPrivilegeMode::Supervisor => classify_supervisor(config, status, address),
            Mips4MmuPrivilegeMode::Kernel => classify_kernel(config, status, address),
        }
    }

    /// Translates a virtual address using address-space rules and TLB entries.
    pub fn translate(
        config: Mips4MmuConfig,
        status: Mips4Cp0Status,
        asid: Mips4TlbAsid,
        tlb_entries: &[Mips4TlbEntry],
        address: u64,
        access_kind: Mips4TlbAccessKind,
    ) -> Mips4MmuTranslationResult {
        match Self::classify_virtual_address(config, status, address) {
            Mips4MmuAddressClassification::Mapped {
                segment,
                address_mode,
            } => translate_mapped(
                segment,
                address_mode,
                asid,
                tlb_entries,
                address,
                access_kind,
                config.address.physical_address_bits,
            ),
            Mips4MmuAddressClassification::Unmapped {
                segment,
                physical_address,
                cache_attribute,
            } => Mips4MmuTranslationResult::Hit(Mips4MmuTranslation {
                physical_address,
                segment,
                source: Mips4MmuTranslationSource::Unmapped,
                cache_attribute,
            }),
            Mips4MmuAddressClassification::AddressError { segment } => {
                fault(address_error_exception(access_kind), address, segment, None)
            }
            Mips4MmuAddressClassification::InvalidMode => {
                fault(address_error_exception(access_kind), address, None, None)
            }
        }
    }
}

const fn classify_user(
    config: Mips4MmuConfig,
    status: Mips4Cp0Status,
    address: u64,
) -> Mips4MmuAddressClassification {
    if status.user_64_bit_addressing() {
        if address <= XUSEG_END && extended_offset_valid(config, address) {
            mapped(Mips4MmuSegment::Xuseg, Mips4TlbAddressMode::Bits64)
        } else {
            address_error(None)
        }
    } else if address <= USEG_END {
        mapped(Mips4MmuSegment::Useg, Mips4TlbAddressMode::Bits32)
    } else {
        address_error(None)
    }
}

const fn classify_supervisor(
    config: Mips4MmuConfig,
    status: Mips4Cp0Status,
    address: u64,
) -> Mips4MmuAddressClassification {
    if status.user_64_bit_addressing() {
        if address <= XUSEG_END && extended_offset_valid(config, address) {
            return mapped(Mips4MmuSegment::Xsuseg, Mips4TlbAddressMode::Bits64);
        }
    } else if address <= USEG_END {
        return mapped(Mips4MmuSegment::Suseg, Mips4TlbAddressMode::Bits32);
    }

    if status.supervisor_64_bit_addressing()
        && in_range(address, XSSEG_START, XSSEG_END)
        && extended_offset_valid(config, address.wrapping_sub(XSSEG_START))
    {
        return mapped(Mips4MmuSegment::Xsseg, Mips4TlbAddressMode::Bits64);
    }

    if in_range(address, KSSEG_START, KSSEG_END) {
        let segment = if status.supervisor_64_bit_addressing() {
            Mips4MmuSegment::Csseg
        } else {
            Mips4MmuSegment::Sseg
        };

        return mapped(segment, Mips4TlbAddressMode::Bits32);
    }

    address_error(None)
}

const fn classify_kernel(
    config: Mips4MmuConfig,
    status: Mips4Cp0Status,
    address: u64,
) -> Mips4MmuAddressClassification {
    if status.user_64_bit_addressing() {
        if address <= XUSEG_END && extended_offset_valid(config, address) {
            return mapped(Mips4MmuSegment::Xkuseg, Mips4TlbAddressMode::Bits64);
        }
    } else if address <= USEG_END {
        return mapped(Mips4MmuSegment::Kuseg, Mips4TlbAddressMode::Bits32);
    }

    if status.supervisor_64_bit_addressing()
        && in_range(address, XSSEG_START, XSSEG_END)
        && extended_offset_valid(config, address.wrapping_sub(XSSEG_START))
    {
        return mapped(Mips4MmuSegment::Xksseg, Mips4TlbAddressMode::Bits64);
    }

    if status.kernel_64_bit_addressing() {
        if address >> 62 == 2 {
            return classify_xkphys(config, address);
        }

        if in_range(address, XKSEG_START, XKSEG_END)
            && extended_offset_valid(config, address.wrapping_sub(XKSEG_START))
        {
            return mapped(Mips4MmuSegment::Xkseg, Mips4TlbAddressMode::Bits64);
        }
    }

    if in_range(address, KSEG0_START, KSEG0_END) {
        return unmapped(
            if status.kernel_64_bit_addressing() {
                Mips4MmuSegment::Ckseg0
            } else {
                Mips4MmuSegment::Kseg0
            },
            address.wrapping_sub(KSEG0_START),
            Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(config.kseg0_cache_coherence_algorithm),
        );
    }

    if in_range(address, KSEG1_START, KSEG1_END) {
        return unmapped(
            if status.kernel_64_bit_addressing() {
                Mips4MmuSegment::Ckseg1
            } else {
                Mips4MmuSegment::Kseg1
            },
            address.wrapping_sub(KSEG1_START),
            Mips4MmuCacheAttribute::Uncached,
        );
    }

    if in_range(address, KSSEG_START, KSSEG_END) {
        return mapped(
            if status.kernel_64_bit_addressing() {
                Mips4MmuSegment::Cksseg
            } else {
                Mips4MmuSegment::Ksseg
            },
            Mips4TlbAddressMode::Bits32,
        );
    }

    if in_range(address, KSEG3_START, KSEG3_END) {
        return mapped(
            if status.kernel_64_bit_addressing() {
                Mips4MmuSegment::Ckseg3
            } else {
                Mips4MmuSegment::Kseg3
            },
            Mips4TlbAddressMode::Bits32,
        );
    }

    address_error(None)
}

const fn classify_xkphys(config: Mips4MmuConfig, address: u64) -> Mips4MmuAddressClassification {
    let physical_mask = low_mask(config.address.physical_address_bits);
    let reserved_mask =
        XKPHYS_RESERVED_ADDRESS_BITS | (XKPHYS_PHYSICAL_ADDRESS_MASK & !physical_mask);
    if address & reserved_mask != 0 {
        return address_error(Some(Mips4MmuSegment::Xkphys));
    }

    let cache_coherence_algorithm_bits = ((address >> XKPHYS_CCA_SHIFT) & 0x07) as u8;
    let Some(cache_coherence_algorithm) =
        Mips4CacheCoherenceAlgorithm::from_bits(cache_coherence_algorithm_bits)
    else {
        return address_error(Some(Mips4MmuSegment::Xkphys));
    };

    unmapped(
        Mips4MmuSegment::Xkphys,
        address & physical_mask,
        Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cache_coherence_algorithm),
    )
}

fn translate_mapped(
    segment: Mips4MmuSegment,
    address_mode: Mips4TlbAddressMode,
    asid: Mips4TlbAsid,
    tlb_entries: &[Mips4TlbEntry],
    address: u64,
    access_kind: Mips4TlbAccessKind,
    physical_address_bits: u8,
) -> Mips4MmuTranslationResult {
    match Mips4Tlb::probe(tlb_entries, address, asid, address_mode) {
        Mips4TlbProbeResult::Miss => fault(
            invalid_or_miss_exception(access_kind),
            address,
            Some(segment),
            Some(address_mode),
        ),
        Mips4TlbProbeResult::Multiple => Mips4MmuTranslationResult::UndefinedMultipleTlbMatch {
            segment,
            address_mode,
        },
        Mips4TlbProbeResult::Hit { index } => {
            match tlb_entries[index].translate(address, asid, access_kind, address_mode) {
                Mips4TlbTranslationResult::Miss { exception }
                | Mips4TlbTranslationResult::Invalid { exception }
                | Mips4TlbTranslationResult::Modified { exception } => {
                    fault(exception, address, Some(segment), Some(address_mode))
                }
                Mips4TlbTranslationResult::Hit(translation) => {
                    if translation.physical_address > low_mask(physical_address_bits) {
                        return fault(
                            address_error_exception(access_kind),
                            address,
                            Some(segment),
                            Some(address_mode),
                        );
                    }
                    Mips4MmuTranslationResult::Hit(Mips4MmuTranslation {
                        physical_address: translation.physical_address,
                        segment,
                        source: Mips4MmuTranslationSource::Mapped {
                            address_mode,
                            page_size: translation.page_size,
                            page_half: translation.page_half,
                        },
                        cache_attribute: Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(
                            translation.cache_coherence_algorithm,
                        ),
                    })
                }
            }
        }
    }
}

const fn mapped(
    segment: Mips4MmuSegment,
    address_mode: Mips4TlbAddressMode,
) -> Mips4MmuAddressClassification {
    Mips4MmuAddressClassification::Mapped {
        segment,
        address_mode,
    }
}

const fn unmapped(
    segment: Mips4MmuSegment,
    physical_address: u64,
    cache_attribute: Mips4MmuCacheAttribute,
) -> Mips4MmuAddressClassification {
    Mips4MmuAddressClassification::Unmapped {
        segment,
        physical_address,
        cache_attribute,
    }
}

const fn address_error(segment: Option<Mips4MmuSegment>) -> Mips4MmuAddressClassification {
    Mips4MmuAddressClassification::AddressError { segment }
}

const fn in_range(address: u64, start: u64, end: u64) -> bool {
    address >= start && address <= end
}

const fn extended_offset_valid(config: Mips4MmuConfig, offset: u64) -> bool {
    offset <= low_mask(config.address.virtual_address_bits)
}

const fn low_mask(bits: u8) -> u64 {
    if bits >= 64 {
        u64::MAX
    } else {
        (1_u64 << bits) - 1
    }
}

const fn address_error_exception(access_kind: Mips4TlbAccessKind) -> Mips4Exception {
    match access_kind {
        Mips4TlbAccessKind::InstructionFetch | Mips4TlbAccessKind::Load => {
            Mips4Exception::AddressErrorLoad
        }
        Mips4TlbAccessKind::Store => Mips4Exception::AddressErrorStore,
    }
}

const fn invalid_or_miss_exception(access_kind: Mips4TlbAccessKind) -> Mips4Exception {
    match access_kind {
        Mips4TlbAccessKind::InstructionFetch | Mips4TlbAccessKind::Load => Mips4Exception::TlbLoad,
        Mips4TlbAccessKind::Store => Mips4Exception::TlbStore,
    }
}

const fn fault(
    exception: Mips4Exception,
    bad_virtual_address: u64,
    segment: Option<Mips4MmuSegment>,
    address_mode: Option<Mips4TlbAddressMode>,
) -> Mips4MmuTranslationResult {
    Mips4MmuTranslationResult::Fault(Mips4MmuFault {
        exception,
        bad_virtual_address,
        segment,
        address_mode,
    })
}

#[cfg(test)]
mod tests;
