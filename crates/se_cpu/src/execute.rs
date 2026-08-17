//! Maps typed instructions and immutable CPU pre-state to context-free dispositions.
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
use crate::memory::{MemoryPreparation, MemoryRequest};

/// Represents one instruction's mutually exclusive normal or guest-exception outcome.
///
/// Execution computes either variant from immutable [`Cpu`] pre-state; applying
/// the result is a separate mutation step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstructionOutcome {
    Commit(CpuCommit),
    Exception(ExceptionRequest),
}

/// Separates complete context-free semantics from a prepared timed memory access.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InstructionDisposition {
    Architectural(InstructionOutcome),
    Memory(MemoryRequest),
}

/// Identifies a non-guest condition for which no normal commit can be produced.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ExecuteError {
    /// A control transfer occurs in a delay slot, where its behavior is unpredictable.
    UnpredictableControlFlow { instruction_pc: u64, branch_pc: u64 },
    /// The instruction result is undefined for the observed operands.
    UndefinedResult { instruction: Instruction },
}

/// Computes one context-free instruction disposition from immutable pre-state.
///
/// A complete architectural outcome and a prepared memory request are mutually
/// exclusive. Failure leaves [`Cpu`] unchanged and represents an emulator stop
/// rather than a guest exception.
///
/// # Errors
///
/// Returns [`ExecuteError::UnpredictableControlFlow`] for a delayed transfer in a
/// delay slot, or [`ExecuteError::UndefinedResult`] when operand values make the
/// instruction result undefined.
pub(crate) fn execute(
    cpu: &Cpu,
    instruction: Instruction,
) -> Result<InstructionDisposition, ExecuteError> {
    match instruction {
        Instruction::Sll { rd, rt, shift } => Ok(architectural(InstructionOutcome::Commit(
            integer::execute_sll(cpu, rd, rt, shift),
        ))),
        Instruction::Add { rd, rs, rt } => integer::execute_add(cpu, rd, rs, rt).map(architectural),
        Instruction::Addiu { rt, rs, immediate } => integer::execute_addiu(cpu, rt, rs, immediate)
            .map(InstructionOutcome::Commit)
            .map(architectural),
        Instruction::Or { rd, rs, rt } => Ok(architectural(InstructionOutcome::Commit(
            integer::execute_or(cpu, rd, rs, rt),
        ))),
        Instruction::Ori { rt, rs, immediate } => Ok(architectural(InstructionOutcome::Commit(
            integer::execute_ori(cpu, rt, rs, immediate),
        ))),
        Instruction::Lui { rt, immediate } => Ok(architectural(InstructionOutcome::Commit(
            integer::execute_lui(rt, immediate),
        ))),
        Instruction::Beq { rs, rt, offset } => branch::execute_beq(cpu, rs, rt, offset)
            .map(InstructionOutcome::Commit)
            .map(architectural),
        Instruction::Bne { rs, rt, offset } => branch::execute_bne(cpu, rs, rt, offset)
            .map(InstructionOutcome::Commit)
            .map(architectural),
        Instruction::J { index } => branch::execute_j(cpu, index)
            .map(InstructionOutcome::Commit)
            .map(architectural),
        Instruction::Lw {
            rt,
            base,
            immediate,
        } => crate::memory::prepare_lw(cpu, rt, base, immediate).map(memory_disposition),
        Instruction::Sw {
            rt,
            base,
            immediate,
        } => crate::memory::prepare_sw(cpu, rt, base, immediate).map(memory_disposition),
        Instruction::Syscall { code } => Ok(architectural(system::execute_syscall(code))),
        Instruction::Break { code } => Ok(architectural(system::execute_break(code))),
    }
}

const fn architectural(outcome: InstructionOutcome) -> InstructionDisposition {
    InstructionDisposition::Architectural(outcome)
}

const fn memory_disposition(preparation: MemoryPreparation) -> InstructionDisposition {
    match preparation {
        MemoryPreparation::Exception(request) => {
            architectural(InstructionOutcome::Exception(request))
        }
        MemoryPreparation::Access(request) => InstructionDisposition::Memory(request),
    }
}
