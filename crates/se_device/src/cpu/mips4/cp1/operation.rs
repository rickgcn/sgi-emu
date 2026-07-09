//! CP1 memory and prefetch operation request/result shapes.
//!
//! This module composes the generic memory operation with the CP1 floating-point
//! register target for the COP1 offset load/store instructions
//! `LWC1`/`SWC1`/`LDC1`/`SDC1` and the COP1X indexed load/store instructions
//! `LWXC1`/`LDXC1`/`SWXC1`/`SDXC1` (MIPS IV manual section B.6), and provides the
//! COP1X indexed prefetch `PREFX` request. It computes the effective address,
//! performs alignment and translation, and bundles the resolved access with the
//! destination FGR. It does not access memory, write FGR state, or decide
//! cacheability.

use super::Mips4Cp1FgrIndex;
use super::decode::{Mips4Cp1IndexedMemoryOperation, Mips4Cp1OffsetMemoryOperation};
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::Mips4Cp0Status;
use crate::cpu::mips4::memory::operation::{
    Mips4MemoryAccess, Mips4MemoryAccessError, Mips4Prefetch, Mips4PrefetchHint,
    Mips4PrefetchResult,
};
use crate::cpu::mips4::memory::{Mips4Memory, Mips4MemoryAccessKind, Mips4MemoryAccessSize};
use crate::cpu::mips4::mmu::Mips4MmuConfig;
use crate::cpu::mips4::tlb::{Mips4TlbAsid, Mips4TlbEntry};

/// A resolved COP1X indexed load or store operation ready for the execution layer.
///
/// The caller must first verify CP1 usability with
/// [`crate::cpu::mips4::exception::check_coprocessor_access`] using
/// [`crate::cpu::mips4::exception::Mips4CoprocessorNumber::Cp1`]. The `rd` field
/// of a COP1X indexed memory instruction must be zero; a non-zero value is
/// UNPREDICTABLE and is the caller's responsibility, not checked here.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4Cp1IndexedMemoryAccess {
    /// Resolved memory access for the indexed operation.
    pub access: Mips4MemoryAccess,

    /// Destination floating-point general register selected by the `fd` field.
    pub fgr: Mips4Cp1FgrIndex,
}

impl Mips4Cp1IndexedMemoryAccess {
    /// Resolves a COP1X indexed memory operation from its base and index GPR values.
    ///
    /// `base` and `index` are the contents of the base and index GPRs. The region
    /// bits of the effective address must come from `base` (manual restriction);
    /// a region mismatch raises [`Mips4MemoryAccessError::AddressError`].
    /// `fgr` is the destination register selected by the instruction's `fd` field.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        operation: Mips4Cp1IndexedMemoryOperation,
        base: u64,
        index: u64,
        fgr: Mips4Cp1FgrIndex,
        endianness: Mips4Endianness,
        mmu_config: Mips4MmuConfig,
        status: Mips4Cp0Status,
        asid: Mips4TlbAsid,
        tlb_entries: &[Mips4TlbEntry],
    ) -> Result<Self, Mips4MemoryAccessError> {
        let is_store = matches!(
            operation,
            Mips4Cp1IndexedMemoryOperation::StoreWordIndexed
                | Mips4Cp1IndexedMemoryOperation::StoreDoublewordIndexed
        );
        let virtual_address = Mips4Memory::indexed_effective_address(base, index, is_store)
            .map_err(|exception| Mips4MemoryAccessError::AddressError {
                exception,
                virtual_address: base.wrapping_add(index),
            })?;
        let kind = cp1_indexed_kind(operation);
        let access = Mips4MemoryAccess::prepare(
            virtual_address,
            kind,
            endianness,
            mmu_config,
            status,
            asid,
            tlb_entries,
        )?;
        Ok(Self { access, fgr })
    }
}

const fn cp1_indexed_kind(operation: Mips4Cp1IndexedMemoryOperation) -> Mips4MemoryAccessKind {
    match operation {
        Mips4Cp1IndexedMemoryOperation::LoadWordIndexed => Mips4MemoryAccessKind::Load {
            size: Mips4MemoryAccessSize::Word,
            signed: false,
        },
        Mips4Cp1IndexedMemoryOperation::LoadDoublewordIndexed => Mips4MemoryAccessKind::Load {
            size: Mips4MemoryAccessSize::Doubleword,
            signed: false,
        },
        Mips4Cp1IndexedMemoryOperation::StoreWordIndexed => Mips4MemoryAccessKind::Store {
            size: Mips4MemoryAccessSize::Word,
        },
        Mips4Cp1IndexedMemoryOperation::StoreDoublewordIndexed => Mips4MemoryAccessKind::Store {
            size: Mips4MemoryAccessSize::Doubleword,
        },
    }
}

/// A resolved CP1 offset load or store operation ready for the execution layer.
///
/// This is the manual `LWC1`/`SWC1`/`LDC1`/`SDC1` request shape (MIPS IV manual
/// section B.6) after base + sign-extended-offset effective address calculation,
/// alignment, and translation. It bundles the resolved access with the
/// destination FGR selected by the `ft` field. It does not access memory or
/// write FGR state.
///
/// The caller must first verify CP1 usability with
/// [`crate::cpu::mips4::exception::check_coprocessor_access`] using
/// [`crate::cpu::mips4::exception::Mips4CoprocessorNumber::Cp1`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4Cp1OffsetMemoryAccess {
    /// Resolved memory access for the offset operation.
    pub access: Mips4MemoryAccess,

    /// Load destination or store source floating-point general register selected
    /// by the `ft` field.
    pub fgr: Mips4Cp1FgrIndex,
}

impl Mips4Cp1OffsetMemoryAccess {
    /// Resolves a CP1 offset memory operation from its base GPR value and offset.
    ///
    /// `base` is the contents of the base GPR and `offset` is the instruction's
    /// signed 16-bit offset. The effective address is `base + sign_extend(offset)`
    /// with no region-bit restriction, matching the manual (region bits are only
    /// constrained for the indexed form). `fgr` is the load destination or store
    /// source register selected by the instruction's `ft` field. Alignment and
    /// translation are delegated to [`Mips4MemoryAccess::prepare`].
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        operation: Mips4Cp1OffsetMemoryOperation,
        base: u64,
        offset: i16,
        fgr: Mips4Cp1FgrIndex,
        endianness: Mips4Endianness,
        mmu_config: Mips4MmuConfig,
        status: Mips4Cp0Status,
        asid: Mips4TlbAsid,
        tlb_entries: &[Mips4TlbEntry],
    ) -> Result<Self, Mips4MemoryAccessError> {
        let virtual_address = Mips4Memory::effective_address(base, offset);
        let kind = cp1_offset_kind(operation);
        let access = Mips4MemoryAccess::prepare(
            virtual_address,
            kind,
            endianness,
            mmu_config,
            status,
            asid,
            tlb_entries,
        )?;
        Ok(Self { access, fgr })
    }
}

const fn cp1_offset_kind(operation: Mips4Cp1OffsetMemoryOperation) -> Mips4MemoryAccessKind {
    match operation {
        Mips4Cp1OffsetMemoryOperation::LoadWord => Mips4MemoryAccessKind::Load {
            size: Mips4MemoryAccessSize::Word,
            signed: false,
        },
        Mips4Cp1OffsetMemoryOperation::LoadDoubleword => Mips4MemoryAccessKind::Load {
            size: Mips4MemoryAccessSize::Doubleword,
            signed: false,
        },
        Mips4Cp1OffsetMemoryOperation::StoreWord => Mips4MemoryAccessKind::Store {
            size: Mips4MemoryAccessSize::Word,
        },
        Mips4Cp1OffsetMemoryOperation::StoreDoubleword => Mips4MemoryAccessKind::Store {
            size: Mips4MemoryAccessSize::Doubleword,
        },
    }
}

/// Stateless COP1X indexed prefetch (`PREFX`) request helper.
///
/// This mirrors the manual `PREFX` operation (MIPS IV manual section B.6) with
/// GPR+GPR addressing: the effective address is `GPR[base] + GPR[index]`,
/// translated with the `LOAD` access kind, and the result carries the prefetch
/// hint and cache attribute. `PREFX` is advisory and ignores addressing-related
/// exceptions, so a region-bit mismatch from
/// [`Mips4Memory::indexed_effective_address`] or a translation fault yields
/// [`Mips4PrefetchResult::NoOperation`]. This helper does not decide
/// cacheability; a successful translation always produces a
/// [`Mips4PrefetchResult::Request`] carrying the cache attribute, and the caller
/// resolves it and skips the prefetch for an uncached location, exactly as for
/// `PREF`. The `PREFX` hint field occupies the `rd` position (bits 15..11) of the
/// instruction word, distinct from `PREF` whose hint occupies the `rt` position;
/// extract it with `Mips4Cp1Instruction::prefetch_hint`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mips4Cp1IndexedPrefetch;

impl Mips4Cp1IndexedPrefetch {
    /// Resolves a COP1X indexed prefetch from its base and index GPR values.
    ///
    /// `base` and `index` are the contents of the base and index GPRs. The region
    /// bits of the effective address must come from `base` (manual restriction);
    /// a region mismatch is an addressing exception, which `PREFX` ignores, so it
    /// yields [`Mips4PrefetchResult::NoOperation`] rather than an address error.
    /// `hint` is the prefetch hint extracted from the instruction's `rd` field.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        base: u64,
        index: u64,
        hint: Mips4PrefetchHint,
        mmu_config: Mips4MmuConfig,
        status: Mips4Cp0Status,
        asid: Mips4TlbAsid,
        tlb_entries: &[Mips4TlbEntry],
    ) -> Mips4PrefetchResult {
        let virtual_address = match Mips4Memory::indexed_effective_address(base, index, false) {
            Ok(address) => address,
            Err(_) => return Mips4PrefetchResult::NoOperation,
        };
        Mips4Prefetch::prepare(virtual_address, hint, mmu_config, status, asid, tlb_entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cpu::mips4::cache::Mips4CacheCoherenceAlgorithm;
    use crate::cpu::mips4::exception::Mips4Exception;

    const ASID: Mips4TlbAsid = Mips4TlbAsid::new(0x22);
    const KSEG0_BASE: u64 = 0xffff_ffff_8000_0000;

    fn mmu_config() -> Mips4MmuConfig {
        Mips4MmuConfig::new(Mips4CacheCoherenceAlgorithm::from_bits(3).unwrap())
    }

    fn kernel_status() -> Mips4Cp0Status {
        Mips4Cp0Status::from_bits(0)
    }

    fn fgr(number: u8) -> Mips4Cp1FgrIndex {
        Mips4Cp1FgrIndex::from_u8(number).unwrap()
    }

    #[test]
    fn prepare_resolves_aligned_word_indexed_load() {
        let access = Mips4Cp1IndexedMemoryAccess::prepare(
            Mips4Cp1IndexedMemoryOperation::LoadWordIndexed,
            KSEG0_BASE,
            0,
            fgr(5),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        )
        .unwrap();

        assert_eq!(access.fgr.number(), 5);
        assert_eq!(
            access.access.kind,
            Mips4MemoryAccessKind::Load {
                size: Mips4MemoryAccessSize::Word,
                signed: false,
            }
        );
        assert_eq!(access.access.virtual_address, KSEG0_BASE);
    }

    #[test]
    fn prepare_maps_store_operations_to_store_kind() {
        let access = Mips4Cp1IndexedMemoryAccess::prepare(
            Mips4Cp1IndexedMemoryOperation::StoreDoublewordIndexed,
            KSEG0_BASE,
            0,
            fgr(0),
            Mips4Endianness::Little,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        )
        .unwrap();

        assert_eq!(
            access.access.kind,
            Mips4MemoryAccessKind::Store {
                size: Mips4MemoryAccessSize::Doubleword,
            }
        );
    }

    #[test]
    fn prepare_reports_load_address_error_when_index_crosses_region() {
        let index: u64 = 0x4000_0000_0000_0000;
        let result = Mips4Cp1IndexedMemoryAccess::prepare(
            Mips4Cp1IndexedMemoryOperation::LoadWordIndexed,
            KSEG0_BASE,
            index,
            fgr(7),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        );

        assert_eq!(
            result,
            Err(Mips4MemoryAccessError::AddressError {
                exception: Mips4Exception::AddressErrorLoad,
                virtual_address: KSEG0_BASE.wrapping_add(index),
            })
        );
    }

    #[test]
    fn prepare_reports_store_address_error_when_index_crosses_region() {
        let index: u64 = 0x4000_0000_0000_0000;
        let result = Mips4Cp1IndexedMemoryAccess::prepare(
            Mips4Cp1IndexedMemoryOperation::StoreWordIndexed,
            KSEG0_BASE,
            index,
            fgr(7),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        );

        assert_eq!(
            result,
            Err(Mips4MemoryAccessError::AddressError {
                exception: Mips4Exception::AddressErrorStore,
                virtual_address: KSEG0_BASE.wrapping_add(index),
            })
        );
    }

    const STATUS_KSU_SHIFT: u8 = 3;
    const STATUS_UX: u32 = 1 << 5;

    fn user_status_64() -> Mips4Cp0Status {
        Mips4Cp0Status::from_bits((2 << STATUS_KSU_SHIFT) | STATUS_UX)
    }

    #[test]
    fn prepare_resolves_aligned_word_offset_load() {
        let access = Mips4Cp1OffsetMemoryAccess::prepare(
            Mips4Cp1OffsetMemoryOperation::LoadWord,
            KSEG0_BASE,
            0,
            fgr(5),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        )
        .unwrap();

        assert_eq!(access.fgr.number(), 5);
        assert_eq!(
            access.access.kind,
            Mips4MemoryAccessKind::Load {
                size: Mips4MemoryAccessSize::Word,
                signed: false,
            }
        );
        assert_eq!(access.access.virtual_address, KSEG0_BASE);
    }

    #[test]
    fn prepare_resolves_aligned_doubleword_offset_load() {
        let access = Mips4Cp1OffsetMemoryAccess::prepare(
            Mips4Cp1OffsetMemoryOperation::LoadDoubleword,
            KSEG0_BASE,
            0,
            fgr(9),
            Mips4Endianness::Little,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        )
        .unwrap();

        assert_eq!(
            access.access.kind,
            Mips4MemoryAccessKind::Load {
                size: Mips4MemoryAccessSize::Doubleword,
                signed: false,
            }
        );
    }

    #[test]
    fn prepare_maps_offset_store_operations_to_store_kind() {
        let access = Mips4Cp1OffsetMemoryAccess::prepare(
            Mips4Cp1OffsetMemoryOperation::StoreWord,
            KSEG0_BASE,
            0,
            fgr(0),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        )
        .unwrap();

        assert_eq!(
            access.access.kind,
            Mips4MemoryAccessKind::Store {
                size: Mips4MemoryAccessSize::Word,
            }
        );
    }

    #[test]
    fn prepare_reports_address_error_for_misaligned_word_offset_load() {
        let result = Mips4Cp1OffsetMemoryAccess::prepare(
            Mips4Cp1OffsetMemoryOperation::LoadWord,
            KSEG0_BASE,
            1,
            fgr(5),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        );

        assert_eq!(
            result,
            Err(Mips4MemoryAccessError::AddressError {
                exception: Mips4Exception::AddressErrorLoad,
                virtual_address: KSEG0_BASE.wrapping_add(1),
            })
        );
    }

    #[test]
    fn prepare_reports_address_error_for_misaligned_doubleword_offset_store() {
        let result = Mips4Cp1OffsetMemoryAccess::prepare(
            Mips4Cp1OffsetMemoryOperation::StoreDoubleword,
            KSEG0_BASE,
            2,
            fgr(0),
            Mips4Endianness::Big,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        );

        assert_eq!(
            result,
            Err(Mips4MemoryAccessError::AddressError {
                exception: Mips4Exception::AddressErrorStore,
                virtual_address: KSEG0_BASE.wrapping_add(2),
            })
        );
    }

    #[test]
    fn prepare_reports_translation_fault_for_unmapped_tlb_miss() {
        let address: u64 = 0x0000_0000_0000_1000;
        let result = Mips4Cp1OffsetMemoryAccess::prepare(
            Mips4Cp1OffsetMemoryOperation::LoadWord,
            address,
            0,
            fgr(5),
            Mips4Endianness::Little,
            mmu_config(),
            user_status_64(),
            ASID,
            &[],
        );

        assert!(matches!(
            result,
            Err(Mips4MemoryAccessError::TranslationFault(_))
        ));
    }

    #[test]
    fn prefetch_indexed_resolves_translated_address_and_hint() {
        let result = Mips4Cp1IndexedPrefetch::prepare(
            KSEG0_BASE,
            0,
            Mips4PrefetchHint::Load,
            mmu_config(),
            kernel_status(),
            ASID,
            &[],
        );
        let Mips4PrefetchResult::Request(prefetch) = result else {
            panic!("expected prefetch request for translated location");
        };

        assert_eq!(prefetch.virtual_address, KSEG0_BASE);
        assert_eq!(prefetch.hint, Mips4PrefetchHint::Load);
        assert!(!prefetch.cache_attribute.is_uncached());
    }

    #[test]
    fn prefetch_indexed_is_no_operation_when_index_crosses_region() {
        let index: u64 = 0x4000_0000_0000_0000;
        assert_eq!(
            Mips4Cp1IndexedPrefetch::prepare(
                KSEG0_BASE,
                index,
                Mips4PrefetchHint::Store,
                mmu_config(),
                kernel_status(),
                ASID,
                &[],
            ),
            Mips4PrefetchResult::NoOperation
        );
    }

    #[test]
    fn prefetch_indexed_is_no_operation_for_translation_fault() {
        assert_eq!(
            Mips4Cp1IndexedPrefetch::prepare(
                0x0000_0000_0000_1000,
                0,
                Mips4PrefetchHint::LoadRetained,
                mmu_config(),
                user_status_64(),
                ASID,
                &[],
            ),
            Mips4PrefetchResult::NoOperation
        );
    }
}
