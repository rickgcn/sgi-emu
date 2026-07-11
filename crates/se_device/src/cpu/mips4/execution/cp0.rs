//! Privileged MIPS IV CP0 and TLB instruction semantics.

use crate::cpu::mips4::cp0::{Mips4Cp0EntryHi, Mips4Cp0Register, Mips4Cp0Status};
use crate::cpu::mips4::exception::{Mips4CoprocessorNumber, Mips4Exception};
use crate::cpu::mips4::gpr::{Mips4GprIndex, sign_extend_word};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::memory::ll_sc::Mips4LlBit;
use crate::cpu::mips4::mmu::Mips4MmuPrivilegeMode;
use crate::cpu::mips4::tlb::{Mips4TlbAddressMode, Mips4TlbAsid, Mips4TlbEntry, Mips4TlbEntryHi};

use super::policy::{
    Mips4Cp0DoublewordTransferDirection, Mips4Cp0DoublewordTransferPolicy, Mips4Cp0WaitPolicy,
    Mips4ExecutionPolicy,
};
use super::state::Mips4ExecutionState;

const COP0_OPCODE: u8 = 0x10;
const COP0_MFC0: u8 = 0x00;
const COP0_DMFC0: u8 = 0x01;
const COP0_MTC0: u8 = 0x04;
const COP0_DMTC0: u8 = 0x05;
const COP0_CO: u8 = 0x10;

const COP0_TLBR: u8 = 0x01;
const COP0_TLBWI: u8 = 0x02;
const COP0_TLBWR: u8 = 0x06;
const COP0_TLBP: u8 = 0x08;
const COP0_ERET: u8 = 0x18;
const COP0_WAIT: u8 = 0x20;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Mips4Cp0Execution {
    Retire,
    Standby,
    SetPc(u64),
    Exception(Mips4Exception),
}

pub(super) fn execute_cp0(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    instruction: Mips4Instruction,
) -> Mips4Cp0Execution {
    if let Err(exception) = check_cp0_access(state.cp0.status()) {
        return Mips4Cp0Execution::Exception(exception);
    }
    if instruction.opcode() != COP0_OPCODE {
        return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
    }

    match instruction.rs() {
        COP0_MFC0 => transfer_from(state, policy, instruction, false),
        COP0_DMFC0 => transfer_from(state, policy, instruction, true),
        COP0_MTC0 => transfer_to(state, policy, instruction, false),
        COP0_DMTC0 => transfer_to(state, policy, instruction, true),
        COP0_CO if instruction.bits() & 0x03ff_ffc0 == 0x0200_0000 => {
            execute_co_function(state, policy, instruction.funct())
        }
        _ => Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction),
    }
}

pub(super) fn check_cp0_access(status: Mips4Cp0Status) -> Result<(), Mips4Exception> {
    let kernel = matches!(
        Mips4MmuPrivilegeMode::from_status(status),
        Some(Mips4MmuPrivilegeMode::Kernel)
    );
    if kernel || status.coprocessor_usable(Mips4CoprocessorNumber::Cp0) {
        Ok(())
    } else {
        Err(Mips4Exception::CoprocessorUnusable {
            coprocessor: Mips4CoprocessorNumber::Cp0,
        })
    }
}

fn transfer_from(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    instruction: Mips4Instruction,
    doubleword: bool,
) -> Mips4Cp0Execution {
    if instruction.bits() & 0x7ff != 0 {
        return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
    }
    let Some(register) = Mips4Cp0Register::from_u8(instruction.rd()) else {
        return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
    };
    if doubleword {
        match policy.cp0_doubleword_transfer_policy(
            Mips4Cp0DoublewordTransferDirection::FromCp0,
            state.cp0.status(),
            register,
        ) {
            Mips4Cp0DoublewordTransferPolicy::Execute => {}
            Mips4Cp0DoublewordTransferPolicy::NoOperation => {
                return Mips4Cp0Execution::Retire;
            }
            Mips4Cp0DoublewordTransferPolicy::ReservedInstruction => {
                return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
            }
        }
    }
    let raw = state.cp0.read(register);
    let value = if doubleword {
        raw
    } else {
        sign_extend_word(raw as u32)
    };
    write_gpr(state, instruction.rt(), value);
    Mips4Cp0Execution::Retire
}

fn transfer_to(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    instruction: Mips4Instruction,
    doubleword: bool,
) -> Mips4Cp0Execution {
    if instruction.bits() & 0x7ff != 0 {
        return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
    }
    let Some(register) = Mips4Cp0Register::from_u8(instruction.rd()) else {
        return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
    };
    if doubleword {
        match policy.cp0_doubleword_transfer_policy(
            Mips4Cp0DoublewordTransferDirection::ToCp0,
            state.cp0.status(),
            register,
        ) {
            Mips4Cp0DoublewordTransferPolicy::Execute => {}
            Mips4Cp0DoublewordTransferPolicy::NoOperation => {
                return Mips4Cp0Execution::Retire;
            }
            Mips4Cp0DoublewordTransferPolicy::ReservedInstruction => {
                return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
            }
        }
    }
    let value = read_gpr(state, instruction.rt());
    let requested = if doubleword {
        value
    } else {
        value as u32 as u64
    };
    let value = policy.cp0_write_value(register, state.cp0.read(register), requested);
    let _ = state.cp0.write(register, value);
    Mips4Cp0Execution::Retire
}

fn execute_co_function(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    function: u8,
) -> Mips4Cp0Execution {
    match function {
        COP0_TLBR => {
            read_tlb_entry(state);
            Mips4Cp0Execution::Retire
        }
        COP0_TLBWI => {
            write_tlb_entry(state, state.cp0.index().index() as usize);
            Mips4Cp0Execution::Retire
        }
        COP0_TLBWR => {
            write_tlb_entry(state, state.cp0.random().index() as usize);
            Mips4Cp0Execution::Retire
        }
        COP0_TLBP => {
            probe_tlb(state);
            Mips4Cp0Execution::Retire
        }
        COP0_ERET => {
            state.llbit = Mips4LlBit::Clear;
            Mips4Cp0Execution::SetPc(state.cp0.return_from_exception())
        }
        COP0_WAIT => match policy.cp0_wait_policy() {
            Mips4Cp0WaitPolicy::ReservedInstruction => {
                Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction)
            }
            Mips4Cp0WaitPolicy::NoOperation => Mips4Cp0Execution::Retire,
            Mips4Cp0WaitPolicy::Standby => Mips4Cp0Execution::Standby,
        },
        _ => Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction),
    }
}

fn read_tlb_entry(state: &mut Mips4ExecutionState) {
    let index = state.cp0.index().index() as usize;
    let Some(entry) = state.tlb_entries.get(index).copied() else {
        return;
    };
    let _ = state.cp0.write(
        Mips4Cp0Register::PageMask,
        u64::from(entry.page_mask().bits()),
    );
    let _ = state.cp0.write(
        Mips4Cp0Register::EntryHi,
        Mips4Cp0EntryHi::from_tlb_entry_hi(entry.entry_hi()).bits(),
    );
    let _ = state
        .cp0
        .write(Mips4Cp0Register::EntryLo0, entry.even_page().bits());
    let _ = state
        .cp0
        .write(Mips4Cp0Register::EntryLo1, entry.odd_page().bits());
}

fn write_tlb_entry(state: &mut Mips4ExecutionState, index: usize) {
    let Some(slot) = state.tlb_entries.get_mut(index) else {
        return;
    };
    let Some(page_mask) = state.cp0.page_mask().to_tlb_page_mask() else {
        return;
    };
    let Some(entry_hi) = state.cp0.entry_hi().to_tlb_entry_hi() else {
        return;
    };
    let Some(entry_lo0) = state.cp0.entry_lo0().to_tlb_entry_lo() else {
        return;
    };
    let Some(entry_lo1) = state.cp0.entry_lo1().to_tlb_entry_lo() else {
        return;
    };
    *slot = Mips4TlbEntry::new(page_mask, entry_hi, entry_lo0, entry_lo1);
}

fn probe_tlb(state: &mut Mips4ExecutionState) {
    let entry_hi = state.cp0.entry_hi();
    let address = entry_hi_address(entry_hi.to_tlb_entry_hi().unwrap());
    let asid = Mips4TlbAsid::new(entry_hi.address_space_identifier());
    let hit = state.tlb_entries.iter().position(|entry| {
        entry.matches_virtual_address(address, asid, Mips4TlbAddressMode::Bits64)
    });
    let index = hit.map_or(1_u64 << 31, |index| index as u64);
    let _ = state.cp0.write(Mips4Cp0Register::Index, index);
}

fn entry_hi_address(entry_hi: Mips4TlbEntryHi) -> u64 {
    (u64::from(entry_hi.region_bits()) << 62) | (entry_hi.vpn2() << 13)
}

fn read_gpr(state: &Mips4ExecutionState, register: u8) -> u64 {
    state.gpr.read(Mips4GprIndex::from_u8(register).unwrap())
}

fn write_gpr(state: &mut Mips4ExecutionState, register: u8, value: u64) {
    state
        .gpr
        .write(Mips4GprIndex::from_u8(register).unwrap(), value);
}

#[cfg(test)]
mod tests {
    use crate::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
    use crate::cpu::mips4::model::r5000::boot_mode::R5000BootMode;
    use crate::cpu::mips4::model::r5000::execution_policy::R5000ExecutionPolicy;
    use crate::cpu::mips4::model::r5000::profile::R5000Profile;
    use crate::cpu::mips4::model::r5000::revision::R5000Revision;

    use super::*;

    fn policy() -> R5000ExecutionPolicy {
        R5000ExecutionPolicy::new(
            R5000Profile::new(
                Mips4Endianness::Big,
                R5000Revision::from_bits(0x21),
                200_000_000,
                Mips4CacheConfig::present(32 * 1024, 32),
                Mips4CacheConfig::present(32 * 1024, 32),
                Mips4CacheConfig::disabled(),
            ),
            R5000BootMode::from_low_bits(0).unwrap(),
        )
    }

    fn state(policy: &R5000ExecutionPolicy) -> Mips4ExecutionState {
        Mips4ExecutionState::new(policy).unwrap()
    }

    #[test]
    fn r5000_doubleword_transfers_execute_for_64_bit_cp0_registers() {
        let policy = policy();
        let mut state = state(&policy);
        let value = 0x1234_5678_9abc_def0;
        write_gpr(&mut state, 1, value);

        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x40a1_7000),
            ),
            Mips4Cp0Execution::Retire
        );
        assert_eq!(state.cp0.epc().bits(), value);
        write_gpr(&mut state, 2, 0);
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4022_7000),
            ),
            Mips4Cp0Execution::Retire
        );
        assert_eq!(read_gpr(&state, 2), value);
    }

    #[test]
    fn r5000_doubleword_transfers_to_32_bit_cp0_registers_have_no_effect() {
        let policy = policy();
        let mut state = state(&policy);
        let marker = 0xfeed_face_cafe_beef;
        write_gpr(&mut state, 1, marker);

        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4021_6000),
            ),
            Mips4Cp0Execution::Retire
        );
        assert_eq!(read_gpr(&state, 1), marker);
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x40a1_6000),
            ),
            Mips4Cp0Execution::Retire
        );
        assert_eq!(state.cp0.status().bits(), (1 << 22) | (1 << 2));
    }

    #[test]
    fn r5000_doubleword_transfer_checks_mode_after_cp0_usability() {
        let policy = policy();
        let mut state = state(&policy);
        let supervisor_cu0 = (1 << 28) | (1 << 3);
        let _ = state.cp0.write(Mips4Cp0Register::Status, supervisor_cu0);
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4021_7000),
            ),
            Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction)
        );

        let _ = state.cp0.write(Mips4Cp0Register::Status, 1 << 3);
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4021_7000),
            ),
            Mips4Cp0Execution::Exception(Mips4Exception::CoprocessorUnusable {
                coprocessor: Mips4CoprocessorNumber::Cp0,
            })
        );

        let _ = state
            .cp0
            .write(Mips4Cp0Register::Status, (2 << 3) | (1 << 1));
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4021_7000),
            ),
            Mips4Cp0Execution::Retire
        );
    }

    #[test]
    fn r5000_wait_requires_exact_encoding_and_cp0_access() {
        let policy = policy();
        let mut state = state(&policy);
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4200_0020),
            ),
            Mips4Cp0Execution::Standby
        );
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4200_0060),
            ),
            Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction)
        );
        let _ = state.cp0.write(Mips4Cp0Register::Status, 2 << 3);
        assert_eq!(
            execute_cp0(
                &mut state,
                &policy,
                Mips4Instruction::from_bits(0x4200_0020),
            ),
            Mips4Cp0Execution::Exception(Mips4Exception::CoprocessorUnusable {
                coprocessor: Mips4CoprocessorNumber::Cp0,
            })
        );
    }
}
