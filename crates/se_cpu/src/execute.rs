//! Maps typed instructions and immutable CPU pre-state to architectural outcomes.
//!
//! Handlers receive `&Cpu`, so they cannot mutate live architectural state. The
//! error channel reports architecturally undefined or unpredictable cases; it does
//! not represent guest exceptions.

mod branch;
mod integer;
mod system;

use crate::commit::CpuCommit;
use crate::cpu::Cpu;
use crate::decode::Instruction;
use crate::exception::ExceptionRequest;

/// Represents one instruction's mutually exclusive normal or guest-exception outcome.
///
/// Execution computes either variant from immutable [`Cpu`] pre-state; applying
/// the result is a separate mutation step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstructionOutcome {
    Commit(CpuCommit),
    Exception(ExceptionRequest),
}

/// Identifies a non-guest condition for which no normal commit can be produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecuteError {
    /// A control transfer occurs in a delay slot, where its behavior is unpredictable.
    UnpredictableControlFlow { instruction_pc: u64, branch_pc: u64 },
    /// The instruction result is undefined for the observed operands.
    UndefinedResult { instruction: Instruction },
}

/// Computes one architectural outcome from immutable pre-state.
///
/// A commit and an exception request are mutually exclusive. Failure leaves
/// [`Cpu`] unchanged and represents an emulator stop rather than a guest exception.
///
/// # Errors
///
/// Returns [`ExecuteError::UnpredictableControlFlow`] for a delayed transfer in a
/// delay slot, or [`ExecuteError::UndefinedResult`] when operand values make the
/// instruction result undefined.
pub(crate) fn execute(
    cpu: &Cpu,
    instruction: Instruction,
) -> Result<InstructionOutcome, ExecuteError> {
    match instruction {
        Instruction::Sll { rd, rt, shift } => Ok(InstructionOutcome::Commit(integer::execute_sll(
            cpu, rd, rt, shift,
        ))),
        Instruction::Add { rd, rs, rt } => integer::execute_add(cpu, rd, rs, rt),
        Instruction::Addiu { rt, rs, immediate } => {
            integer::execute_addiu(cpu, rt, rs, immediate).map(InstructionOutcome::Commit)
        }
        Instruction::Or { rd, rs, rt } => Ok(InstructionOutcome::Commit(integer::execute_or(
            cpu, rd, rs, rt,
        ))),
        Instruction::Ori { rt, rs, immediate } => Ok(InstructionOutcome::Commit(
            integer::execute_ori(cpu, rt, rs, immediate),
        )),
        Instruction::Lui { rt, immediate } => Ok(InstructionOutcome::Commit(integer::execute_lui(
            rt, immediate,
        ))),
        Instruction::Beq { rs, rt, offset } => {
            branch::execute_beq(cpu, rs, rt, offset).map(InstructionOutcome::Commit)
        }
        Instruction::Bne { rs, rt, offset } => {
            branch::execute_bne(cpu, rs, rt, offset).map(InstructionOutcome::Commit)
        }
        Instruction::J { index } => branch::execute_j(cpu, index).map(InstructionOutcome::Commit),
        Instruction::Syscall { code } => Ok(system::execute_syscall(code)),
        Instruction::Break { code } => Ok(system::execute_break(code)),
    }
}
