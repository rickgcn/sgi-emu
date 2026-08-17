//! Prepares CPU word accesses and translates compatibility-kernel direct addresses.
//!
//! This module classifies guest virtual addresses before constructing
//! [`PhysAddr`] values. It has no knowledge of machine bus routes, device-local
//! addresses, or event scheduling. Address classification assumes a
//! kernel-compatible boot/direct-access context and does not validate `Status.KSU`
//! or `KX`/`SX`/`UX`; callers must not treat it as general-mode translation.

use se_core::address::PhysAddr;

use crate::cpu::Cpu;
use crate::decode::Instruction;
use crate::exception::ExceptionRequest;
use crate::execute::ExecuteError;
use crate::gpr::Reg;

const KSEG0_START: u32 = 0x8000_0000;
const KSEG1_START: u32 = 0xa000_0000;
const KSEG2_START: u32 = 0xc000_0000;

/// Identifies a virtual address path outside compatibility-kernel direct translation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TranslationError {
    /// A canonical 32-bit-compatible mapped segment requires TLB translation.
    TlbRequired { virtual_address: u64 },
    /// The address does not belong to the supported compatibility-space subset.
    AddressSpaceUnimplemented { virtual_address: u64 },
}

/// Holds a validated virtual word access awaiting address translation and bus I/O.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MemoryRequest {
    /// Reads a guest-big-endian word and writes its sign-extended value to a GPR.
    LoadWord {
        /// Destination general-purpose register.
        destination: Reg,
        /// Aligned guest virtual byte address.
        virtual_address: u64,
    },
    /// Writes the captured low 32 bits of a GPR as one guest-big-endian word.
    StoreWord {
        /// Word value captured before the physical transaction.
        value: u32,
        /// Aligned guest virtual byte address.
        virtual_address: u64,
    },
}

/// Separates a pre-Bus guest exception from an aligned timed memory request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum MemoryPreparation {
    /// Completes as a guest exception without issuing a data bus transaction.
    Exception(ExceptionRequest),
    /// Requires translation followed by a timed physical bus transaction.
    Access(MemoryRequest),
}

/// Prepares an `LW` without performing address translation or bus I/O.
///
/// A misaligned effective address produces
/// [`MemoryPreparation::Exception`]. Otherwise the returned request retains the
/// guest virtual address and destination register for timed completion.
///
/// # Errors
///
/// Returns [`ExecuteError::UndefinedResult`] when either low bit of `immediate`
/// is set, as required by the architectural instruction restriction.
pub(crate) fn prepare_lw(
    cpu: &Cpu,
    destination: Reg,
    base: Reg,
    immediate: i16,
) -> Result<MemoryPreparation, ExecuteError> {
    let instruction = Instruction::Lw {
        rt: destination,
        base,
        immediate,
    };
    validate_word_offset(instruction, immediate)?;
    let virtual_address = effective_address(cpu.read_gpr(base), immediate);
    if !virtual_address.is_multiple_of(4) {
        return Ok(MemoryPreparation::Exception(
            ExceptionRequest::AddressErrorLoad {
                bad_vaddr: virtual_address,
            },
        ));
    }
    Ok(MemoryPreparation::Access(MemoryRequest::LoadWord {
        destination,
        virtual_address,
    }))
}

/// Prepares an `SW` without performing address translation or bus I/O.
///
/// The request captures the source GPR's low 32 bits. A misaligned effective
/// address instead produces [`MemoryPreparation::Exception`] without a data bus
/// transaction.
///
/// # Errors
///
/// Returns [`ExecuteError::UndefinedResult`] when either low bit of `immediate`
/// is set, as required by the architectural instruction restriction.
pub(crate) fn prepare_sw(
    cpu: &Cpu,
    source: Reg,
    base: Reg,
    immediate: i16,
) -> Result<MemoryPreparation, ExecuteError> {
    let instruction = Instruction::Sw {
        rt: source,
        base,
        immediate,
    };
    validate_word_offset(instruction, immediate)?;
    let virtual_address = effective_address(cpu.read_gpr(base), immediate);
    if !virtual_address.is_multiple_of(4) {
        return Ok(MemoryPreparation::Exception(
            ExceptionRequest::AddressErrorStore {
                bad_vaddr: virtual_address,
            },
        ));
    }
    Ok(MemoryPreparation::Access(MemoryRequest::StoreWord {
        value: cpu.read_gpr(source) as u32,
        virtual_address,
    }))
}

fn validate_word_offset(instruction: Instruction, immediate: i16) -> Result<(), ExecuteError> {
    // MIPS IV defines LW/SW with either low offset bit set as undefined, even
    // when the complete effective address would otherwise be word-aligned.
    if (immediate as u16) & 0b11 != 0 {
        return Err(ExecuteError::UndefinedResult { instruction });
    }
    Ok(())
}

fn effective_address(base: u64, immediate: i16) -> u64 {
    base.wrapping_add(i64::from(immediate) as u64)
}

/// Translates sign-extended compatibility-kernel direct addresses to physical space.
///
/// The inclusive virtual ranges `0xffff_ffff_8000_0000..=0xffff_ffff_9fff_ffff`
/// and `0xffff_ffff_a000_0000..=0xffff_ffff_bfff_ffff` map to physical
/// `0x0000_0000..=0x1fff_ffff`. The caller supplies a kernel-compatible
/// boot/direct-access context; this function does not validate privilege or
/// extended-address mode bits.
///
/// # Errors
///
/// Returns [`TranslationError::TlbRequired`] for canonical 32-bit-compatible
/// mapped segments. Returns [`TranslationError::AddressSpaceUnimplemented`] for
/// zero-extended kernel aliases and other unsupported 64-bit address forms.
pub(crate) fn translate_compat_kernel_direct(
    virtual_address: u64,
) -> Result<PhysAddr, TranslationError> {
    let upper = virtual_address >> 32;
    let low = virtual_address as u32;

    if upper == 0 && low < KSEG0_START {
        return Err(TranslationError::TlbRequired { virtual_address });
    }
    if upper != u64::from(u32::MAX) {
        return Err(TranslationError::AddressSpaceUnimplemented { virtual_address });
    }

    match low {
        KSEG0_START..KSEG1_START => Ok(PhysAddr::new(u64::from(low - KSEG0_START))),
        KSEG1_START..KSEG2_START => Ok(PhysAddr::new(u64::from(low - KSEG1_START))),
        KSEG2_START..=u32::MAX => Err(TranslationError::TlbRequired { virtual_address }),
        _ => Err(TranslationError::AddressSpaceUnimplemented { virtual_address }),
    }
}

#[cfg(test)]
mod tests {
    use se_core::address::PhysAddr;

    use super::{TranslationError, translate_compat_kernel_direct};

    #[test]
    fn compatibility_kernel_segments_translate_only_after_positive_classification() {
        assert_eq!(
            translate_compat_kernel_direct(0xffff_ffff_8000_0180),
            Ok(PhysAddr::new(0x180))
        );
        assert_eq!(
            translate_compat_kernel_direct(0xffff_ffff_bfc0_0000),
            Ok(PhysAddr::new(0x1fc0_0000))
        );
        assert_eq!(
            translate_compat_kernel_direct(0xffff_ffff_bfff_ffff),
            Ok(PhysAddr::new(0x1fff_ffff))
        );
    }

    #[test]
    fn mapped_compatibility_segments_require_the_deferred_tlb() {
        for virtual_address in [0x1000, 0xffff_ffff_c000_0000, u64::MAX] {
            assert_eq!(
                translate_compat_kernel_direct(virtual_address),
                Err(TranslationError::TlbRequired { virtual_address })
            );
        }
    }

    #[test]
    fn zero_extended_kernel_aliases_and_other_64_bit_spaces_are_not_invented() {
        for virtual_address in [
            0x0000_0000_8000_0000,
            0x0000_0000_bfc0_0000,
            0x8000_0000_0000_0000,
        ] {
            assert_eq!(
                translate_compat_kernel_direct(virtual_address),
                Err(TranslationError::AddressSpaceUnimplemented { virtual_address })
            );
        }
    }
}
