//! Prepares CPU word accesses and classifies canonical 32-bit virtual addresses.
//!
//! Address classification validates the complete 64-bit representation and the
//! effective operating mode before choosing a direct physical route or a mapped
//! TLB route. It has no knowledge of TLB contents, machine bus routes,
//! device-local addresses, or event scheduling.

use se_core::address::PhysAddr;

use crate::cp0::OperatingMode;
use crate::cpu::Cpu;
use crate::decode::Instruction;
use crate::exception::ExceptionRequest;
use crate::execute::ExecuteError;
use crate::gpr::Reg;

const KSEG0_START: u32 = 0x8000_0000;
const KSEG1_START: u32 = 0xa000_0000;
const KSEG2_START: u32 = 0xc000_0000;
const KSEG3_START: u32 = 0xe000_0000;

/// Identifies the CPU operation presenting a virtual address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AccessKind {
    /// Instruction fetch.
    Fetch,
    /// Data load.
    Load,
    /// Data store.
    Store,
}

/// Selects the next stage after 32-bit address legality checks.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AddressRoute {
    /// Uses the contained physical byte address without a TLB lookup.
    Direct(PhysAddr),
    /// Presents the contained canonical guest virtual byte address to the TLB.
    Mapped { virtual_address: u64 },
}

/// Holds an aligned virtual word access awaiting address classification and bus I/O.
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
/// The effective address is the base GPR plus the sign-extended `immediate`
/// modulo 2^64. A misaligned effective address produces
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
/// The effective address is the base GPR plus the sign-extended `immediate`
/// modulo 2^64. The request captures the source GPR's low 32 bits. A misaligned
/// effective address instead produces [`MemoryPreparation::Exception`] without a
/// data bus transaction.
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

/// Classifies one guest virtual byte address under canonical 32-bit rules.
///
/// A valid address equals the sign extension of its low 32 bits. User mode maps
/// `useg` (`0x0000_0000_0000_0000..=0x0000_0000_7fff_ffff`). Supervisor mode
/// additionally maps `sseg`
/// (`0xffff_ffff_c000_0000..=0xffff_ffff_dfff_ffff`), and Kernel mode additionally
/// maps `kseg3` (`0xffff_ffff_e000_0000..=0xffff_ffff_ffff_ffff`). In Kernel mode,
/// `kseg0` (`0xffff_ffff_8000_0000..=0xffff_ffff_9fff_ffff`) and `kseg1`
/// (`0xffff_ffff_a000_0000..=0xffff_ffff_bfff_ffff`) each map to physical
/// `0x0000_0000..=0x1fff_ffff`. With `mode` set to Kernel and `erl` set, virtual
/// `0x0000_0000..=0x7fff_ffff` maps directly to the same physical byte address.
/// Direct routes still require the caller to use the physical bus.
///
/// # Errors
///
/// Returns the access-appropriate Address Error request, containing the original
/// offending value, when the address is noncanonical or inaccessible in `mode`.
pub(crate) fn classify_32_bit_address(
    virtual_address: u64,
    mode: OperatingMode,
    erl: bool,
    access: AccessKind,
) -> Result<AddressRoute, ExceptionRequest> {
    let low = virtual_address as u32;
    let canonical = i64::from(low as i32) as u64;
    if virtual_address != canonical {
        return Err(address_error(access, virtual_address));
    }

    let mapped = || AddressRoute::Mapped { virtual_address };

    match mode {
        OperatingMode::User if low < KSEG0_START => Ok(mapped()),
        OperatingMode::User => Err(address_error(access, virtual_address)),
        OperatingMode::Supervisor if low < KSEG0_START => Ok(mapped()),
        OperatingMode::Supervisor if (KSEG2_START..KSEG3_START).contains(&low) => Ok(mapped()),
        OperatingMode::Supervisor => Err(address_error(access, virtual_address)),
        OperatingMode::Kernel if erl && low < KSEG0_START => {
            Ok(AddressRoute::Direct(PhysAddr::new(u64::from(low))))
        }
        OperatingMode::Kernel if low < KSEG0_START => Ok(mapped()),
        OperatingMode::Kernel if low < KSEG1_START => Ok(AddressRoute::Direct(PhysAddr::new(
            u64::from(low - KSEG0_START),
        ))),
        OperatingMode::Kernel if low < KSEG2_START => Ok(AddressRoute::Direct(PhysAddr::new(
            u64::from(low - KSEG1_START),
        ))),
        OperatingMode::Kernel => Ok(mapped()),
    }
}

pub(crate) const fn address_error(access: AccessKind, virtual_address: u64) -> ExceptionRequest {
    match access {
        AccessKind::Fetch | AccessKind::Load => ExceptionRequest::AddressErrorLoad {
            bad_vaddr: virtual_address,
        },
        AccessKind::Store => ExceptionRequest::AddressErrorStore {
            bad_vaddr: virtual_address,
        },
    }
}

#[cfg(test)]
mod tests {
    use se_core::address::PhysAddr;

    use super::{AccessKind, AddressRoute, classify_32_bit_address};
    use crate::cp0::OperatingMode;
    use crate::exception::ExceptionRequest;

    #[test]
    fn classifier_requires_the_canonical_sign_extended_representation() {
        let cases = [
            0x0000_0000_8000_0000,
            0xffff_ffff_7fff_ffff,
            0x8000_0000_0000_0000,
        ];

        for virtual_address in cases {
            assert_eq!(
                classify_32_bit_address(
                    virtual_address,
                    OperatingMode::Kernel,
                    false,
                    AccessKind::Load,
                ),
                Err(ExceptionRequest::AddressErrorLoad {
                    bad_vaddr: virtual_address,
                })
            );
        }
    }

    #[test]
    fn user_mode_maps_only_useg_and_selects_access_specific_address_errors() {
        assert_eq!(
            classify_32_bit_address(0x1234, OperatingMode::User, false, AccessKind::Fetch),
            Ok(AddressRoute::Mapped {
                virtual_address: 0x1234,
            })
        );
        assert_eq!(
            classify_32_bit_address(
                0xffff_ffff_c000_0000,
                OperatingMode::User,
                false,
                AccessKind::Store,
            ),
            Err(ExceptionRequest::AddressErrorStore {
                bad_vaddr: 0xffff_ffff_c000_0000,
            })
        );
    }

    #[test]
    fn supervisor_mode_maps_useg_and_sseg_but_rejects_kernel_spaces() {
        for virtual_address in [0x1234, 0xffff_ffff_c000_1234] {
            assert_eq!(
                classify_32_bit_address(
                    virtual_address,
                    OperatingMode::Supervisor,
                    false,
                    AccessKind::Load,
                ),
                Ok(AddressRoute::Mapped { virtual_address })
            );
        }

        for virtual_address in [
            0xffff_ffff_8000_0000,
            0xffff_ffff_a000_0000,
            0xffff_ffff_e000_0000,
        ] {
            assert_eq!(
                classify_32_bit_address(
                    virtual_address,
                    OperatingMode::Supervisor,
                    false,
                    AccessKind::Load,
                ),
                Err(ExceptionRequest::AddressErrorLoad {
                    bad_vaddr: virtual_address,
                })
            );
        }
    }

    #[test]
    fn kernel_direct_segments_select_the_expected_physical_addresses() {
        assert_eq!(
            classify_32_bit_address(
                0xffff_ffff_8000_0180,
                OperatingMode::Kernel,
                false,
                AccessKind::Fetch,
            ),
            Ok(AddressRoute::Direct(PhysAddr::new(0x180)))
        );
        assert_eq!(
            classify_32_bit_address(
                0xffff_ffff_bfc0_0000,
                OperatingMode::Kernel,
                false,
                AccessKind::Load,
            ),
            Ok(AddressRoute::Direct(PhysAddr::new(0x1fc0_0000)))
        );
    }

    #[test]
    fn kernel_mapped_segments_are_kept_distinct_from_direct_routes() {
        for virtual_address in [
            0x1000,
            0xffff_ffff_c000_0000,
            0xffff_ffff_e000_0000,
            u64::MAX,
        ] {
            assert_eq!(
                classify_32_bit_address(
                    virtual_address,
                    OperatingMode::Kernel,
                    false,
                    AccessKind::Load,
                ),
                Ok(AddressRoute::Mapped { virtual_address })
            );
        }
    }

    #[test]
    fn erl_routes_the_low_two_gibibytes_directly() {
        assert_eq!(
            classify_32_bit_address(0x1234_5678, OperatingMode::Kernel, true, AccessKind::Store,),
            Ok(AddressRoute::Direct(PhysAddr::new(0x1234_5678)))
        );
    }
}
