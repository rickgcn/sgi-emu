//! Maps typed instructions and immutable CPU pre-state to retirement candidates.
//!
//! Handlers receive `&Cpu`, so they cannot mutate live architectural state. The
//! error channel reports architecturally undefined or unpredictable cases; it does
//! not represent guest exceptions.

mod branch;
mod integer;

use crate::commit::CpuCommit;
use crate::cpu::Cpu;
use crate::decode::Instruction;

/// Identifies a non-guest condition for which no normal commit can be produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecuteError {
    /// A control transfer occurs in a delay slot, where its behavior is unpredictable.
    UnpredictableControlFlow { instruction_pc: u64, branch_pc: u64 },
    /// The instruction result is undefined for the observed operands.
    UndefinedResult { instruction: Instruction },
}

/// Computes a normal-retirement write-set from immutable architectural pre-state.
///
/// Failure leaves [`Cpu`] unchanged and represents an emulator stop rather than a
/// guest exception.
///
/// # Errors
///
/// Returns [`ExecuteError::UnpredictableControlFlow`] for a delayed transfer in a
/// delay slot, or [`ExecuteError::UndefinedResult`] when operand values make the
/// instruction result undefined.
pub(crate) fn execute(cpu: &Cpu, instruction: Instruction) -> Result<CpuCommit, ExecuteError> {
    match instruction {
        Instruction::Sll { rd, rt, shift } => Ok(integer::execute_sll(cpu, rd, rt, shift)),
        Instruction::Addiu { rt, rs, immediate } => integer::execute_addiu(cpu, rt, rs, immediate),
        Instruction::Or { rd, rs, rt } => Ok(integer::execute_or(cpu, rd, rs, rt)),
        Instruction::Ori { rt, rs, immediate } => Ok(integer::execute_ori(cpu, rt, rs, immediate)),
        Instruction::Lui { rt, immediate } => Ok(integer::execute_lui(rt, immediate)),
        Instruction::Beq { rs, rt, offset } => branch::execute_beq(cpu, rs, rt, offset),
        Instruction::Bne { rs, rt, offset } => branch::execute_bne(cpu, rs, rt, offset),
        Instruction::J { index } => branch::execute_j(cpu, index),
    }
}
