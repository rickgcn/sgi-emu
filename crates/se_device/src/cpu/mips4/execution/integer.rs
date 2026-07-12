//! Integer and control-flow instruction semantics.

use crate::cpu::mips4::alu::Mips4Alu;
use crate::cpu::mips4::branch::{Mips4Branch, Mips4BranchDecision};
use crate::cpu::mips4::exception::{
    Mips4Exception, Mips4TrapDecision, teq, teqi, tge, tgei, tgeiu, tgeu, tlt, tlti, tltiu, tltu,
    tne, tnei,
};
use crate::cpu::mips4::gpr::{Mips4GprIndex, is_sign_extended_word};
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::Mips4CpuInstruction;

use super::policy::{Mips4ExecutionPolicy, Mips4NotWordValuePolicy};
use super::state::Mips4ExecutionState;

pub(super) enum Mips4CpuExecution {
    Retire,
    Branch(Mips4BranchDecision),
    Memory(Mips4CpuInstruction),
    Exception(Mips4Exception),
}

pub(super) fn execute_cpu(
    state: &mut Mips4ExecutionState,
    policy: &impl Mips4ExecutionPolicy,
    raw: Mips4Instruction,
    instruction: Mips4CpuInstruction,
) -> Mips4CpuExecution {
    if !word_operands_valid(state, raw, instruction)
        && matches!(
            policy.not_word_value_policy(instruction),
            Mips4NotWordValuePolicy::NoOperation
        )
    {
        return Mips4CpuExecution::Retire;
    }

    let rs = read(state, raw.rs());
    let rt = read(state, raw.rt());
    let immediate = raw.signed_immediate();

    match instruction {
        Mips4CpuInstruction::Add => write_result(state, raw.rd(), Mips4Alu::add(rs, rt)),
        Mips4CpuInstruction::Addi => {
            write_result(state, raw.rt(), Mips4Alu::add_immediate(rs, immediate))
        }
        Mips4CpuInstruction::Addiu => {
            write(state, raw.rt(), Mips4Alu::addiu(rs, immediate));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Addu => {
            write(state, raw.rd(), Mips4Alu::addu(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::And => {
            write(state, raw.rd(), Mips4Alu::and(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Andi => {
            write(state, raw.rt(), Mips4Alu::andi(rs, raw.immediate()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dadd => write_result(state, raw.rd(), Mips4Alu::dadd(rs, rt)),
        Mips4CpuInstruction::Daddi => write_result(state, raw.rt(), Mips4Alu::daddi(rs, immediate)),
        Mips4CpuInstruction::Daddiu => {
            write(state, raw.rt(), Mips4Alu::daddiu(rs, immediate));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Daddu => {
            write(state, raw.rd(), Mips4Alu::daddu(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsub => write_result(state, raw.rd(), Mips4Alu::dsub(rs, rt)),
        Mips4CpuInstruction::Dsubu => {
            write(state, raw.rd(), Mips4Alu::dsubu(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Lui => {
            write(state, raw.rt(), Mips4Alu::lui(raw.immediate()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Nor => {
            write(state, raw.rd(), Mips4Alu::nor(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Or => {
            write(state, raw.rd(), Mips4Alu::or(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Ori => {
            write(state, raw.rt(), Mips4Alu::ori(rs, raw.immediate()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Slt => {
            write(state, raw.rd(), Mips4Alu::slt(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Slti => {
            write(state, raw.rt(), Mips4Alu::slti(rs, immediate));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Sltiu => {
            write(state, raw.rt(), Mips4Alu::sltiu(rs, immediate));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Sltu => {
            write(state, raw.rd(), Mips4Alu::sltu(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Sub => write_result(state, raw.rd(), Mips4Alu::sub(rs, rt)),
        Mips4CpuInstruction::Subu => {
            write(state, raw.rd(), Mips4Alu::subu(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Xor => {
            write(state, raw.rd(), Mips4Alu::xor(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Xori => {
            write(state, raw.rt(), Mips4Alu::xori(rs, raw.immediate()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Sll => {
            write(state, raw.rd(), Mips4Alu::sll(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Sllv => {
            write(state, raw.rd(), Mips4Alu::sllv(rt, rs));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Sra => {
            write(state, raw.rd(), Mips4Alu::sra(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Srav => {
            write(state, raw.rd(), Mips4Alu::srav(rt, rs));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Srl => {
            write(state, raw.rd(), Mips4Alu::srl(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Srlv => {
            write(state, raw.rd(), Mips4Alu::srlv(rt, rs));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsll => {
            write(state, raw.rd(), Mips4Alu::dsll(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsll32 => {
            write(state, raw.rd(), Mips4Alu::dsll32(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsllv => {
            write(state, raw.rd(), Mips4Alu::dsllv(rt, rs));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsra => {
            write(state, raw.rd(), Mips4Alu::dsra(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsra32 => {
            write(state, raw.rd(), Mips4Alu::dsra32(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsrav => {
            write(state, raw.rd(), Mips4Alu::dsrav(rt, rs));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsrl => {
            write(state, raw.rd(), Mips4Alu::dsrl(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsrl32 => {
            write(state, raw.rd(), Mips4Alu::dsrl32(rt, raw.shamt()));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dsrlv => {
            write(state, raw.rd(), Mips4Alu::dsrlv(rt, rs));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Mult => {
            write_hilo(state, Mips4Alu::mult(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Multu => {
            write_hilo(state, Mips4Alu::multu(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dmult => {
            write_hilo(state, Mips4Alu::dmult(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Dmultu => {
            write_hilo(state, Mips4Alu::dmultu(rs, rt));
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Div => {
            if let Some(result) = Mips4Alu::div(rs, rt) {
                write_hilo(state, result);
            }
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Divu => {
            if let Some(result) = Mips4Alu::divu(rs, rt) {
                write_hilo(state, result);
            }
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Ddiv => {
            if let Some(result) = Mips4Alu::ddiv(rs, rt) {
                write_hilo(state, result);
            }
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Ddivu => {
            if let Some(result) = Mips4Alu::ddivu(rs, rt) {
                write_hilo(state, result);
            }
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Mfhi => {
            write(state, raw.rd(), state.hi);
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Mflo => {
            write(state, raw.rd(), state.lo);
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Mthi => {
            state.hi = rs;
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Mtlo => {
            state.lo = rs;
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Movn => {
            if let Some(value) = Mips4Alu::movn(rs, rt) {
                write(state, raw.rd(), value);
            }
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Movz => {
            if let Some(value) = Mips4Alu::movz(rs, rt) {
                write(state, raw.rd(), value);
            }
            Mips4CpuExecution::Retire
        }
        Mips4CpuInstruction::Beq => branch(Mips4Branch::beq(state.pc, rs, rt, immediate)),
        Mips4CpuInstruction::Beql => branch(Mips4Branch::beql(state.pc, rs, rt, immediate)),
        Mips4CpuInstruction::Bne => branch(Mips4Branch::bne(state.pc, rs, rt, immediate)),
        Mips4CpuInstruction::Bnel => branch(Mips4Branch::bnel(state.pc, rs, rt, immediate)),
        Mips4CpuInstruction::Bgez => branch(Mips4Branch::bgez(state.pc, rs, immediate)),
        Mips4CpuInstruction::Bgezl => branch(Mips4Branch::bgezl(state.pc, rs, immediate)),
        Mips4CpuInstruction::Bgtz => branch(Mips4Branch::bgtz(state.pc, rs, immediate)),
        Mips4CpuInstruction::Bgtzl => branch(Mips4Branch::bgtzl(state.pc, rs, immediate)),
        Mips4CpuInstruction::Blez => branch(Mips4Branch::blez(state.pc, rs, immediate)),
        Mips4CpuInstruction::Blezl => branch(Mips4Branch::blezl(state.pc, rs, immediate)),
        Mips4CpuInstruction::Bltz => branch(Mips4Branch::bltz(state.pc, rs, immediate)),
        Mips4CpuInstruction::Bltzl => branch(Mips4Branch::bltzl(state.pc, rs, immediate)),
        Mips4CpuInstruction::Bgezal => {
            let linked = Mips4Branch::bgezal(state.pc, rs, immediate);
            write(state, 31, linked.link_value);
            branch(linked.decision)
        }
        Mips4CpuInstruction::Bgezall => {
            let linked = Mips4Branch::bgezall(state.pc, rs, immediate);
            write(state, 31, linked.link_value);
            branch(linked.decision)
        }
        Mips4CpuInstruction::Bltzal => {
            let linked = Mips4Branch::bltzal(state.pc, rs, immediate);
            write(state, 31, linked.link_value);
            branch(linked.decision)
        }
        Mips4CpuInstruction::Bltzall => {
            let linked = Mips4Branch::bltzall(state.pc, rs, immediate);
            write(state, 31, linked.link_value);
            branch(linked.decision)
        }
        Mips4CpuInstruction::J => branch(Mips4Branch::j(state.pc, raw.target())),
        Mips4CpuInstruction::Jal => {
            let linked = Mips4Branch::jal(state.pc, raw.target());
            write(state, 31, linked.link_value);
            branch(linked.decision)
        }
        Mips4CpuInstruction::Jr => match Mips4Branch::jr(rs) {
            Ok(decision) => branch(decision),
            Err(exception) => Mips4CpuExecution::Exception(exception),
        },
        Mips4CpuInstruction::Jalr => match Mips4Branch::jalr(state.pc, rs) {
            Ok(linked) => {
                write(state, raw.rd(), linked.link_value);
                branch(linked.decision)
            }
            Err(exception) => Mips4CpuExecution::Exception(exception),
        },
        Mips4CpuInstruction::Break | Mips4CpuInstruction::Syscall => {
            Mips4CpuExecution::Exception(instruction.system_exception().unwrap().exception())
        }
        Mips4CpuInstruction::Teq => trap(teq(rs, rt)),
        Mips4CpuInstruction::Teqi => trap(teqi(rs, immediate)),
        Mips4CpuInstruction::Tge => trap(tge(rs, rt)),
        Mips4CpuInstruction::Tgei => trap(tgei(rs, immediate)),
        Mips4CpuInstruction::Tgeiu => trap(tgeiu(rs, immediate)),
        Mips4CpuInstruction::Tgeu => trap(tgeu(rs, rt)),
        Mips4CpuInstruction::Tlt => trap(tlt(rs, rt)),
        Mips4CpuInstruction::Tlti => trap(tlti(rs, immediate)),
        Mips4CpuInstruction::Tltiu => trap(tltiu(rs, immediate)),
        Mips4CpuInstruction::Tltu => trap(tltu(rs, rt)),
        Mips4CpuInstruction::Tne => trap(tne(rs, rt)),
        Mips4CpuInstruction::Tnei => trap(tnei(rs, immediate)),
        Mips4CpuInstruction::Sync | Mips4CpuInstruction::Pref => Mips4CpuExecution::Retire,
        Mips4CpuInstruction::Lb
        | Mips4CpuInstruction::Lbu
        | Mips4CpuInstruction::Ld
        | Mips4CpuInstruction::Ldl
        | Mips4CpuInstruction::Ldr
        | Mips4CpuInstruction::Lh
        | Mips4CpuInstruction::Lhu
        | Mips4CpuInstruction::Ll
        | Mips4CpuInstruction::Lld
        | Mips4CpuInstruction::Lw
        | Mips4CpuInstruction::Lwl
        | Mips4CpuInstruction::Lwr
        | Mips4CpuInstruction::Lwu
        | Mips4CpuInstruction::Sb
        | Mips4CpuInstruction::Sc
        | Mips4CpuInstruction::Scd
        | Mips4CpuInstruction::Sd
        | Mips4CpuInstruction::Sdl
        | Mips4CpuInstruction::Sdr
        | Mips4CpuInstruction::Sh
        | Mips4CpuInstruction::Sw
        | Mips4CpuInstruction::Swl
        | Mips4CpuInstruction::Swr => Mips4CpuExecution::Memory(instruction),
    }
}

fn write_result(
    state: &mut Mips4ExecutionState,
    register: u8,
    result: Result<u64, Mips4Exception>,
) -> Mips4CpuExecution {
    match result {
        Ok(value) => {
            write(state, register, value);
            Mips4CpuExecution::Retire
        }
        Err(exception) => Mips4CpuExecution::Exception(exception),
    }
}

fn write_hilo(state: &mut Mips4ExecutionState, result: crate::cpu::mips4::alu::Mips4HiLoResult) {
    state.hi = result.hi;
    state.lo = result.lo;
}

fn read(state: &Mips4ExecutionState, register: u8) -> u64 {
    state.gpr.read(Mips4GprIndex::from_u8(register).unwrap())
}

fn write(state: &mut Mips4ExecutionState, register: u8, value: u64) {
    state
        .gpr
        .write(Mips4GprIndex::from_u8(register).unwrap(), value);
}

fn branch(decision: Mips4BranchDecision) -> Mips4CpuExecution {
    Mips4CpuExecution::Branch(decision)
}

fn trap(decision: Mips4TrapDecision) -> Mips4CpuExecution {
    if decision.should_trap() {
        Mips4CpuExecution::Exception(Mips4Exception::Trap)
    } else {
        Mips4CpuExecution::Retire
    }
}

fn word_operands_valid(
    state: &Mips4ExecutionState,
    raw: Mips4Instruction,
    instruction: Mips4CpuInstruction,
) -> bool {
    let rs = read(state, raw.rs());
    let rt = read(state, raw.rt());
    match instruction {
        Mips4CpuInstruction::Add
        | Mips4CpuInstruction::Addu
        | Mips4CpuInstruction::Sub
        | Mips4CpuInstruction::Subu
        | Mips4CpuInstruction::Mult
        | Mips4CpuInstruction::Multu
        | Mips4CpuInstruction::Div
        | Mips4CpuInstruction::Divu => is_sign_extended_word(rs) && is_sign_extended_word(rt),
        Mips4CpuInstruction::Addi | Mips4CpuInstruction::Addiu => is_sign_extended_word(rs),
        Mips4CpuInstruction::Sll
        | Mips4CpuInstruction::Sllv
        | Mips4CpuInstruction::Sra
        | Mips4CpuInstruction::Srav
        | Mips4CpuInstruction::Srl
        | Mips4CpuInstruction::Srlv => is_sign_extended_word(rt),
        _ => true,
    }
}
