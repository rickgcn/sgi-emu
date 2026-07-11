//! Privileged MIPS IV CP0 and TLB instruction semantics.

use crate::cpu::mips4::cp0::{Mips4Cp0EntryHi, Mips4Cp0Register, Mips4Cp0Status};
use crate::cpu::mips4::exception::{Mips4CoprocessorNumber, Mips4Exception};
use crate::cpu::mips4::gpr::{Mips4GprIndex, sign_extend_word};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::memory::ll_sc::Mips4LlBit;
use crate::cpu::mips4::mmu::Mips4MmuPrivilegeMode;
use crate::cpu::mips4::tlb::{Mips4TlbAddressMode, Mips4TlbAsid, Mips4TlbEntry, Mips4TlbEntryHi};

use super::policy::Mips4ExecutionPolicy;
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

pub(super) enum Mips4Cp0Execution {
    Retire,
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
        COP0_MFC0 => transfer_from(state, instruction, false),
        COP0_DMFC0 => transfer_from(state, instruction, true),
        COP0_MTC0 => transfer_to(state, policy, instruction, false),
        COP0_DMTC0 => transfer_to(state, policy, instruction, true),
        COP0_CO if instruction.bits() & 0x03ff_ffc0 == 0x0200_0000 => {
            execute_co_function(state, instruction.funct())
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
    instruction: Mips4Instruction,
    doubleword: bool,
) -> Mips4Cp0Execution {
    if instruction.bits() & 0x7ff != 0 {
        return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
    }
    let Some(register) = Mips4Cp0Register::from_u8(instruction.rd()) else {
        return Mips4Cp0Execution::Exception(Mips4Exception::ReservedInstruction);
    };
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

fn execute_co_function(state: &mut Mips4ExecutionState, function: u8) -> Mips4Cp0Execution {
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
