//! Manual-shaped MIPS IV memory operation request/result shapes.
//!
//! This module composes the pure memory helpers with address translation to
//! describe a load, store, or prefetch as an immutable operation ready for the
//! execution layer. It mirrors the manual common functions `LoadMemory`,
//! `StoreMemory`, and `Prefetch` (MIPS IV manual section A.5.3.2) without
//! performing bus access, cache fill, store commit, or exception entry.
//!
//! The operations here only classify and translate. They do not read or write
//! CPU, FPU, or CP0 state.

use super::{Mips4Memory, Mips4MemoryAccessKind};
use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::Mips4Cp0Status;
use crate::cpu::mips4::exception::Mips4Exception;
use crate::cpu::mips4::mmu::{
    Mips4Mmu, Mips4MmuCacheAttribute, Mips4MmuConfig, Mips4MmuFault, Mips4MmuSegment,
    Mips4MmuTranslation, Mips4MmuTranslationResult,
};
use crate::cpu::mips4::tlb::{
    Mips4TlbAccessKind, Mips4TlbAddressMode, Mips4TlbAsid, Mips4TlbEntry,
};

/// Prefetch hint supplied by the `PREF` and `PREFX` hint field.
///
/// Defined hint values follow the manual tables A-32 and B-22. Values not yet
/// defined by the architecture are preserved as [`Self::Undefined`] so the raw
/// field round-trips; the manual recommends implementations treat an undefined
/// hint as the `load` action or as a no-op.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4PrefetchHint {
    /// `load` (0): data is expected to be loaded, not modified.
    Load,

    /// `store` (1): data is expected to be stored or modified.
    Store,

    /// `load_streamed` (4): loaded but not reused extensively.
    LoadStreamed,

    /// `store_streamed` (5): stored or modified but not reused extensively.
    StoreStreamed,

    /// `load_retained` (6): loaded and reused extensively.
    LoadRetained,

    /// `store_retained` (7): stored or modified and reused extensively.
    StoreRetained,

    /// A hint value not yet defined by the architecture (2, 3, and 8 through 31).
    Undefined(u8),
}

impl Mips4PrefetchHint {
    /// Creates a prefetch hint from the raw 5-bit hint field value.
    pub const fn from_bits(bits: u8) -> Self {
        match bits {
            0 => Self::Load,
            1 => Self::Store,
            4 => Self::LoadStreamed,
            5 => Self::StoreStreamed,
            6 => Self::LoadRetained,
            7 => Self::StoreRetained,
            other => Self::Undefined(other),
        }
    }

    /// Returns the raw hint field value.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Load => 0,
            Self::Store => 1,
            Self::LoadStreamed => 4,
            Self::StoreStreamed => 5,
            Self::LoadRetained => 6,
            Self::StoreRetained => 7,
            Self::Undefined(bits) => bits,
        }
    }

    /// Returns whether the architecture defines this hint value.
    pub const fn is_defined(self) -> bool {
        !matches!(self, Self::Undefined(_))
    }
}

/// Failure of a memory operation before the execution layer receives it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4MemoryAccessError {
    /// Architectural alignment or address-space error before translation.
    AddressError {
        /// Exception a CPU pipeline would raise.
        exception: Mips4Exception,

        /// Virtual address that failed the access.
        virtual_address: u64,
    },

    /// Address translation faulted (TLB miss, invalid, modified, or address error).
    TranslationFault(Mips4MmuFault),

    /// More than one TLB entry matched a mapped address (undefined result).
    UndefinedMultipleTlbMatch {
        /// Segment selected by the virtual address.
        segment: Mips4MmuSegment,

        /// TLB address comparison mode used for the lookup.
        address_mode: Mips4TlbAddressMode,
    },
}

/// A resolved load or store operation ready for the execution layer.
///
/// This is the manual `LoadMemory`/`StoreMemory` request shape after effective
/// address calculation, architectural alignment, and address translation. It
/// carries the translated physical address, the cache attribute, the access
/// kind, and the effective endianness, but does not perform the memory access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4MemoryAccess {
    /// Effective virtual address of the access.
    pub virtual_address: u64,

    /// Successful address translation carrying the physical address and cache attribute.
    pub translation: Mips4MmuTranslation,

    /// Load, store, or partial-word access kind selected by the instruction.
    pub kind: Mips4MemoryAccessKind,

    /// Effective byte order for selecting bytes within the memory element.
    pub endianness: Mips4Endianness,
}

impl Mips4MemoryAccess {
    /// Resolves a load or store operation from its effective address.
    ///
    /// `virtual_address` is the already-computed effective address. The caller
    /// computes it with [`Mips4Memory::effective_address`] or
    /// [`Mips4Memory::indexed_effective_address`]. This helper performs
    /// architectural alignment (manual order: align before translate) and
    /// address translation, then bundles the result. It does not access memory.
    pub fn prepare(
        virtual_address: u64,
        kind: Mips4MemoryAccessKind,
        endianness: Mips4Endianness,
        mmu_config: Mips4MmuConfig,
        status: Mips4Cp0Status,
        asid: Mips4TlbAsid,
        tlb_entries: &[Mips4TlbEntry],
    ) -> Result<Self, Mips4MemoryAccessError> {
        if let Err(exception) = Mips4Memory::check_alignment(virtual_address, kind) {
            return Err(Mips4MemoryAccessError::AddressError {
                exception,
                virtual_address,
            });
        }

        match Mips4Mmu::translate(
            mmu_config,
            status,
            asid,
            tlb_entries,
            virtual_address,
            tlb_access_kind(kind),
        ) {
            Mips4MmuTranslationResult::Hit(translation) => Ok(Self {
                virtual_address,
                translation,
                kind,
                endianness,
            }),
            Mips4MmuTranslationResult::Fault(fault) => {
                Err(Mips4MemoryAccessError::TranslationFault(fault))
            }
            Mips4MmuTranslationResult::UndefinedMultipleTlbMatch {
                segment,
                address_mode,
            } => Err(Mips4MemoryAccessError::UndefinedMultipleTlbMatch {
                segment,
                address_mode,
            }),
        }
    }

    /// Returns the translated physical address.
    pub const fn physical_address(&self) -> u64 {
        self.translation.physical_address
    }

    /// Returns the cache attribute selected by translation.
    pub const fn cache_attribute(&self) -> Mips4MmuCacheAttribute {
        self.translation.cache_attribute
    }

    /// Returns the architecture-level memory access type for this access.
    ///
    /// Cached cache-coherence algorithms resolve to
    /// [`Mips4MemoryAccessType::ImplementationSpecific`] at this base layer; a
    /// processor model must refine them before using the result for
    /// access-type decisions.
    pub const fn memory_access_type(&self) -> Mips4MemoryAccessType {
        self.cache_attribute().memory_access_type()
    }
}

/// A resolved prefetch operation ready for the execution layer.
///
/// This is the manual `Prefetch` request shape after address translation. It
/// carries the translated physical address, the prefetch hint, and the cache
/// attribute. Prefetch is advisory and never changes architecturally-visible
/// state, so the result only describes the request; whether the implementation
/// acts on it is implementation-specific.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4Prefetch {
    /// Effective virtual address of the prefetch.
    pub virtual_address: u64,

    /// Translated physical address.
    pub physical_address: u64,

    /// Prefetch hint selected by the instruction.
    pub hint: Mips4PrefetchHint,

    /// Cache attribute selected by translation.
    pub cache_attribute: Mips4MmuCacheAttribute,
}

/// Result of resolving a prefetch operation.
///
/// This only reflects whether address translation succeeded; it does not decide
/// cacheability. The manual states that `PREF` never generates a memory
/// operation for an uncached location, but whether a raw cache-coherence
/// algorithm resolves to uncached is processor-specific (see
/// [`Mips4MmuCacheAttribute::memory_access_type`]). The caller must resolve the
/// request's cache attribute to a [`Mips4MemoryAccessType`] using a
/// processor-specific CCA policy and skip the prefetch when that resolves to
/// uncached.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4PrefetchResult {
    /// Translation succeeded. The caller resolves the cache attribute to a
    /// memory access type and performs the advisory prefetch only when the
    /// location is not uncached.
    Request(Mips4Prefetch),

    /// No prefetch occurs because translation faulted or multiple TLB entries
    /// matched. Addressing-related exceptions are ignored, so no exception is
    /// raised.
    NoOperation,
}

impl Mips4Prefetch {
    /// Resolves a prefetch operation from its effective address.
    ///
    /// `virtual_address` is the already-computed effective address. Translation
    /// uses the `LOAD` access kind, matching the manual `PREF` operation. No
    /// alignment check is performed because `PREF` ignores addressing
    /// exceptions; a translation fault or multiple TLB match yields
    /// [`Mips4PrefetchResult::NoOperation`]. This helper does not decide
    /// cacheability: a successful translation always produces a
    /// [`Mips4PrefetchResult::Request`] carrying the cache attribute, and the
    /// caller resolves it and skips the prefetch for an uncached location.
    pub fn prepare(
        virtual_address: u64,
        hint: Mips4PrefetchHint,
        mmu_config: Mips4MmuConfig,
        status: Mips4Cp0Status,
        asid: Mips4TlbAsid,
        tlb_entries: &[Mips4TlbEntry],
    ) -> Mips4PrefetchResult {
        match Mips4Mmu::translate(
            mmu_config,
            status,
            asid,
            tlb_entries,
            virtual_address,
            Mips4TlbAccessKind::Load,
        ) {
            Mips4MmuTranslationResult::Hit(translation) => Mips4PrefetchResult::Request(Self {
                virtual_address,
                physical_address: translation.physical_address,
                hint,
                cache_attribute: translation.cache_attribute,
            }),
            Mips4MmuTranslationResult::Fault(_)
            | Mips4MmuTranslationResult::UndefinedMultipleTlbMatch { .. } => {
                Mips4PrefetchResult::NoOperation
            }
        }
    }
}

const fn tlb_access_kind(kind: Mips4MemoryAccessKind) -> Mips4TlbAccessKind {
    if kind.is_store() {
        Mips4TlbAccessKind::Store
    } else {
        Mips4TlbAccessKind::Load
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::mips4::cache::Mips4CacheCoherenceAlgorithm;
    use crate::cpu::mips4::mmu::Mips4MmuSegment;
    use crate::cpu::mips4::tlb::{
        Mips4TlbAddressMode, Mips4TlbEntry, Mips4TlbEntryHi, Mips4TlbEntryLo, Mips4TlbPageMask,
        Mips4TlbPageSize,
    };

    const STATUS_KSU_SHIFT: u8 = 3;
    const STATUS_UX: u32 = 1 << 5;
    const ASID: Mips4TlbAsid = Mips4TlbAsid::new(0x22);

    const KSEG0_BASE: u64 = 0xffff_ffff_8000_0000;
    const KSEG1_BASE: u64 = 0xffff_ffff_a000_0000;

    fn cca(bits: u8) -> Mips4CacheCoherenceAlgorithm {
        Mips4CacheCoherenceAlgorithm::from_bits(bits).unwrap()
    }

    fn mmu_config() -> Mips4MmuConfig {
        Mips4MmuConfig::new(cca(3))
    }

    fn status(bits: u32) -> Mips4Cp0Status {
        Mips4Cp0Status::from_bits(bits)
    }

    fn kernel_status() -> Mips4Cp0Status {
        status(0)
    }

    fn user_status_64() -> Mips4Cp0Status {
        status((2 << STATUS_KSU_SHIFT) | STATUS_UX)
    }

    fn entry_lo(cca_bits: u8, valid: bool) -> Mips4TlbEntryLo {
        Mips4TlbEntryLo::from_parts(0x100, cca(cca_bits), true, valid, false).unwrap()
    }

    fn entry_for(
        addr: u64,
        mode: Mips4TlbAddressMode,
        even: Mips4TlbEntryLo,
        odd: Mips4TlbEntryLo,
    ) -> Mips4TlbEntry {
        let page_mask = Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size4KiB);
        let entry_hi = Mips4TlbEntryHi::from_virtual_address(addr, ASID, page_mask, mode);
        Mips4TlbEntry::new(page_mask, entry_hi, even, odd)
    }

    fn word_load(signed: bool) -> Mips4MemoryAccessKind {
        Mips4MemoryAccessKind::Load {
            size: crate::cpu::mips4::memory::Mips4MemoryAccessSize::Word,
            signed,
        }
    }

    fn word_store() -> Mips4MemoryAccessKind {
        Mips4MemoryAccessKind::Store {
            size: crate::cpu::mips4::memory::Mips4MemoryAccessSize::Word,
        }
    }

    #[test]
    fn cache_attribute_resolves_uncached_directly_and_caches_as_implementation_specific() {
        assert_eq!(
            Mips4MmuCacheAttribute::Uncached.memory_access_type(),
            Mips4MemoryAccessType::Uncached
        );
        for bits in 0..=7 {
            assert_eq!(
                Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(bits)).memory_access_type(),
                Mips4MemoryAccessType::ImplementationSpecific,
                "CCA {bits}"
            );
        }
    }

    #[test]
    fn prepare_resolves_aligned_kseg0_word_load() {
        let address = KSEG0_BASE;
        let access = Mips4MemoryAccess::prepare(
            address,
            word_load(true),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        )
        .unwrap();

        assert_eq!(access.virtual_address, address);
        assert_eq!(access.physical_address(), address.wrapping_sub(KSEG0_BASE));
        assert_eq!(access.kind, word_load(true));
        assert_eq!(access.endianness, Mips4Endianness::Big);
        assert_eq!(
            access.memory_access_type(),
            Mips4MemoryAccessType::ImplementationSpecific
        );
    }

    #[test]
    fn prepare_reports_address_error_for_misaligned_word_load_and_store() {
        let address = KSEG0_BASE | 0x1;
        assert_eq!(
            Mips4MemoryAccess::prepare(
                address,
                word_load(true),
                Mips4Endianness::Big,
                mmu_config(),
                kernel_status(),
                ASID,
                &[],
            ),
            Err(Mips4MemoryAccessError::AddressError {
                exception: Mips4Exception::AddressErrorLoad,
                virtual_address: address,
            })
        );
        assert_eq!(
            Mips4MemoryAccess::prepare(
                address,
                word_store(),
                Mips4Endianness::Big,
                mmu_config(),
                kernel_status(),
                ASID,
                &[],
            ),
            Err(Mips4MemoryAccessError::AddressError {
                exception: Mips4Exception::AddressErrorStore,
                virtual_address: address,
            })
        );
    }

    #[test]
    fn prepare_reports_translation_fault_for_unmapped_tlb_miss() {
        let address: u64 = 0x0000_0000_0000_1000;
        let result = Mips4MemoryAccess::prepare(
            address,
            word_load(true),
            Mips4Endianness::Little,
            mmu_config(),
            user_status_64(),
            ASID,
            &[],
        );

        assert_eq!(
            result,
            Err(Mips4MemoryAccessError::TranslationFault(Mips4MmuFault {
                exception: Mips4Exception::TlbLoad,
                bad_virtual_address: address,
                segment: Some(Mips4MmuSegment::Xuseg),
                address_mode: Some(Mips4TlbAddressMode::Bits64),
            }))
        );
    }

    #[test]
    fn prepare_reports_translation_fault_for_invalid_user_segment() {
        let address: u64 = 0x8000_0000;
        let result = Mips4MemoryAccess::prepare(
            address,
            word_load(true),
            Mips4Endianness::Little,
            mmu_config(),
            status(2 << STATUS_KSU_SHIFT),
            ASID,
            &[],
        );

        assert_eq!(
            result,
            Err(Mips4MemoryAccessError::TranslationFault(Mips4MmuFault {
                exception: Mips4Exception::AddressErrorLoad,
                bad_virtual_address: address,
                segment: None,
                address_mode: None,
            }))
        );
    }

    #[test]
    fn prepare_skips_alignment_for_partial_word_access() {
        let address = KSEG0_BASE | 0x1;
        let access = Mips4MemoryAccess::prepare(
            address,
            Mips4MemoryAccessKind::LoadWordLeft,
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        )
        .unwrap();

        assert_eq!(access.virtual_address, address);
        assert_eq!(access.kind, Mips4MemoryAccessKind::LoadWordLeft);
    }

    #[test]
    fn prepare_reports_undefined_multiple_tlb_match() {
        let address: u64 = 0x0000_0000_0000_1000;
        let even = entry_lo(3, true);
        let entries = [
            entry_for(
                address,
                Mips4TlbAddressMode::Bits64,
                even,
                entry_lo(3, false),
            ),
            entry_for(
                address,
                Mips4TlbAddressMode::Bits64,
                even,
                entry_lo(3, false),
            ),
        ];

        let result = Mips4MemoryAccess::prepare(
            address,
            word_load(true),
            Mips4Endianness::Little,
            mmu_config(),
            user_status_64(),
            ASID,
            &entries,
        );

        assert_eq!(
            result,
            Err(Mips4MemoryAccessError::UndefinedMultipleTlbMatch {
                segment: Mips4MmuSegment::Xuseg,
                address_mode: Mips4TlbAddressMode::Bits64,
            })
        );
    }

    #[test]
    fn prefetch_request_carries_translated_address_and_cache_attribute() {
        let request = Mips4Prefetch::prepare(
            KSEG0_BASE,
            Mips4PrefetchHint::Load,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        );
        let Mips4PrefetchResult::Request(prefetch) = request else {
            panic!("expected prefetch request for translated location");
        };
        assert_eq!(prefetch.virtual_address, KSEG0_BASE);
        assert_eq!(
            prefetch.physical_address,
            KSEG0_BASE.wrapping_sub(KSEG0_BASE)
        );
        assert_eq!(prefetch.hint, Mips4PrefetchHint::Load);
        // kseg0 carries a raw cache-coherence algorithm; the caller resolves it.
        assert!(!prefetch.cache_attribute.is_uncached());
    }

    #[test]
    fn prefetch_request_carries_uncached_attribute_without_filtering_it() {
        // kseg1 is architecturally uncached, but the base layer does not decide
        // cacheability; it hands the uncached attribute to the caller.
        let request = Mips4Prefetch::prepare(
            KSEG1_BASE,
            Mips4PrefetchHint::Store,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        );
        let Mips4PrefetchResult::Request(prefetch) = request else {
            panic!("expected prefetch request carrying the uncached attribute");
        };
        assert_eq!(prefetch.virtual_address, KSEG1_BASE);
        assert!(prefetch.cache_attribute.is_uncached());
    }

    #[test]
    fn prefetch_is_no_operation_for_translation_fault() {
        let address: u64 = 0x0000_0000_0000_1000;
        assert_eq!(
            Mips4Prefetch::prepare(
                address,
                Mips4PrefetchHint::LoadRetained,
                mmu_config(),
                user_status_64(),
                ASID,
                &[],
            ),
            Mips4PrefetchResult::NoOperation
        );
    }

    #[test]
    fn prefetch_hint_round_trips_defined_and_undefined_values() {
        for bits in 0..=31u8 {
            let hint = Mips4PrefetchHint::from_bits(bits);
            assert_eq!(hint.bits(), bits, "hint {bits}");
            assert_eq!(
                hint.is_defined(),
                matches!(bits, 0 | 1 | 4 | 5 | 6 | 7),
                "hint {bits}"
            );
        }
    }
}
