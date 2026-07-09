//! COP1X indexed memory operation request/result.
//!
//! This module composes the generic memory operation with the CP1 floating-point
//! register target for the COP1X indexed load and store instructions
//! `LWXC1`/`LDXC1`/`SWXC1`/`SDXC1` (MIPS IV manual section B.6). It computes the
//! indexed effective address, performs alignment and translation, and bundles
//! the resolved access with the destination FGR. It does not access memory or
//! write FGR state.

use super::Mips4Cp1FgrIndex;
use super::decode::Mips4Cp1IndexedMemoryOperation;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::Mips4Cp0Status;
use crate::cpu::mips4::memory::operation::{Mips4MemoryAccess, Mips4MemoryAccessError};
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
}
