//! Pure MIPS I branch and jump target helpers.
//!
//! Branch helpers take the address of the branch or jump instruction itself.
//! They compute targets relative to the delay-slot address where required, but
//! they do not update CPU state or model branch delay execution.

use crate::cpu::mips1::exception::Mips1Exception;

/// Result of evaluating a branch or jump.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips1BranchDecision {
    /// Control continues through the delay slot without taking the branch.
    NotTaken,

    /// Control transfers to the target after the delay slot.
    Taken {
        /// Target address.
        target: u32,
    },
}

impl Mips1BranchDecision {
    const fn taken(target: u32) -> Self {
        Self::Taken { target }
    }

    const fn not_taken() -> Self {
        Self::NotTaken
    }

    /// Returns whether control transfers to a target after the delay slot.
    pub const fn is_taken(self) -> bool {
        matches!(self, Self::Taken { .. })
    }

    /// Returns the target address when the branch or jump is taken.
    pub const fn target(self) -> Option<u32> {
        match self {
            Self::NotTaken => None,
            Self::Taken { target } => Some(target),
        }
    }
}

/// Result of evaluating a branch or jump that writes a link register.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips1LinkedBranchDecision {
    /// Branch or jump decision.
    pub decision: Mips1BranchDecision,

    /// Value written to the link register by the instruction.
    pub link_value: u32,
}

/// Stateless MIPS I branch helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips1Branch;

impl Mips1Branch {
    /// Evaluates `BEQ`.
    pub const fn beq(branch_pc: u32, lhs: u32, rhs: u32, offset: i16) -> Mips1BranchDecision {
        conditional_branch(branch_pc, lhs == rhs, offset)
    }

    /// Evaluates `BNE`.
    pub const fn bne(branch_pc: u32, lhs: u32, rhs: u32, offset: i16) -> Mips1BranchDecision {
        conditional_branch(branch_pc, lhs != rhs, offset)
    }

    /// Evaluates `BLTZ`.
    pub const fn bltz(branch_pc: u32, value: u32, offset: i16) -> Mips1BranchDecision {
        conditional_branch(branch_pc, (value as i32) < 0, offset)
    }

    /// Evaluates `BGEZ`.
    pub const fn bgez(branch_pc: u32, value: u32, offset: i16) -> Mips1BranchDecision {
        conditional_branch(branch_pc, (value as i32) >= 0, offset)
    }

    /// Evaluates `BLEZ`.
    pub const fn blez(branch_pc: u32, value: u32, offset: i16) -> Mips1BranchDecision {
        conditional_branch(branch_pc, (value as i32) <= 0, offset)
    }

    /// Evaluates `BGTZ`.
    pub const fn bgtz(branch_pc: u32, value: u32, offset: i16) -> Mips1BranchDecision {
        conditional_branch(branch_pc, (value as i32) > 0, offset)
    }

    /// Evaluates `BLTZAL`.
    pub const fn bltzal(branch_pc: u32, value: u32, offset: i16) -> Mips1LinkedBranchDecision {
        linked_branch(branch_pc, Self::bltz(branch_pc, value, offset))
    }

    /// Evaluates `BGEZAL`.
    pub const fn bgezal(branch_pc: u32, value: u32, offset: i16) -> Mips1LinkedBranchDecision {
        linked_branch(branch_pc, Self::bgez(branch_pc, value, offset))
    }

    /// Evaluates `J`.
    pub const fn j(branch_pc: u32, target: u32) -> Mips1BranchDecision {
        Mips1BranchDecision::taken(jump_target(branch_pc, target))
    }

    /// Evaluates `JAL`.
    pub const fn jal(branch_pc: u32, target: u32) -> Mips1LinkedBranchDecision {
        linked_branch(branch_pc, Self::j(branch_pc, target))
    }

    /// Evaluates `JR`.
    pub const fn jr(target: u32) -> Result<Mips1BranchDecision, Mips1Exception> {
        match aligned_target(target) {
            Ok(()) => Ok(Mips1BranchDecision::taken(target)),
            Err(error) => Err(error),
        }
    }

    /// Evaluates `JALR`.
    pub const fn jalr(
        branch_pc: u32,
        target: u32,
    ) -> Result<Mips1LinkedBranchDecision, Mips1Exception> {
        match Self::jr(target) {
            Ok(decision) => Ok(linked_branch(branch_pc, decision)),
            Err(error) => Err(error),
        }
    }
}

const fn conditional_branch(branch_pc: u32, taken: bool, offset: i16) -> Mips1BranchDecision {
    if taken {
        Mips1BranchDecision::taken(branch_target(branch_pc, offset))
    } else {
        Mips1BranchDecision::not_taken()
    }
}

const fn linked_branch(branch_pc: u32, decision: Mips1BranchDecision) -> Mips1LinkedBranchDecision {
    Mips1LinkedBranchDecision {
        decision,
        link_value: branch_pc.wrapping_add(8),
    }
}

const fn branch_target(branch_pc: u32, offset: i16) -> u32 {
    branch_pc
        .wrapping_add(4)
        .wrapping_add((offset as i32 as u32).wrapping_shl(2))
}

const fn jump_target(branch_pc: u32, target: u32) -> u32 {
    (branch_pc.wrapping_add(4) & 0xf000_0000) | ((target & 0x03ff_ffff) << 2)
}

const fn aligned_target(target: u32) -> Result<(), Mips1Exception> {
    if target & 0x3 == 0 {
        Ok(())
    } else {
        Err(Mips1Exception::AddressErrorLoad)
    }
}

#[cfg(test)]
mod tests;
