//! Pure MIPS IV branch and jump target helpers.
//!
//! Branch helpers take the address of the branch or jump instruction itself.
//! They compute targets relative to the delay-slot address where required, but
//! they do not update CPU state or model branch delay execution.

use crate::cpu::mips4::exception::Mips4Exception;

/// Result of evaluating a branch or jump.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4BranchDecision {
    /// Control continues through the delay slot without taking the branch.
    NotTaken {
        /// Whether the delay-slot instruction is nullified.
        nullify_delay_slot: bool,
    },

    /// Control transfers to the target after the delay slot.
    Taken {
        /// Target address.
        target: u64,

        /// Whether the delay-slot instruction is nullified.
        nullify_delay_slot: bool,
    },
}

impl Mips4BranchDecision {
    const fn taken(target: u64) -> Self {
        Self::Taken {
            target,
            nullify_delay_slot: false,
        }
    }

    const fn not_taken(nullify_delay_slot: bool) -> Self {
        Self::NotTaken { nullify_delay_slot }
    }

    /// Returns whether control transfers to a target after the delay slot.
    pub const fn is_taken(self) -> bool {
        matches!(self, Self::Taken { .. })
    }

    /// Returns the target address when the branch or jump is taken.
    pub const fn target(self) -> Option<u64> {
        match self {
            Self::NotTaken { .. } => None,
            Self::Taken { target, .. } => Some(target),
        }
    }

    /// Returns whether the delay-slot instruction is nullified.
    pub const fn nullify_delay_slot(self) -> bool {
        match self {
            Self::NotTaken { nullify_delay_slot }
            | Self::Taken {
                nullify_delay_slot, ..
            } => nullify_delay_slot,
        }
    }
}

/// Result of evaluating a branch or jump that writes a link register.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4LinkedBranchDecision {
    /// Branch or jump decision.
    pub decision: Mips4BranchDecision,

    /// Value written to the link register by the instruction.
    pub link_value: u64,
}

/// Stateless MIPS IV branch helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mips4Branch;

impl Mips4Branch {
    /// Evaluates `BEQ`.
    pub const fn beq(branch_pc: u64, lhs: u64, rhs: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, lhs == rhs, offset, false)
    }

    /// Evaluates `BEQL`.
    pub const fn beql(branch_pc: u64, lhs: u64, rhs: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, lhs == rhs, offset, true)
    }

    /// Evaluates `BNE`.
    pub const fn bne(branch_pc: u64, lhs: u64, rhs: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, lhs != rhs, offset, false)
    }

    /// Evaluates `BNEL`.
    pub const fn bnel(branch_pc: u64, lhs: u64, rhs: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, lhs != rhs, offset, true)
    }

    /// Evaluates `BLTZ`.
    pub const fn bltz(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) < 0, offset, false)
    }

    /// Evaluates `BLTZL`.
    pub const fn bltzl(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) < 0, offset, true)
    }

    /// Evaluates `BGEZ`.
    pub const fn bgez(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) >= 0, offset, false)
    }

    /// Evaluates `BGEZL`.
    pub const fn bgezl(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) >= 0, offset, true)
    }

    /// Evaluates `BLEZ`.
    pub const fn blez(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) <= 0, offset, false)
    }

    /// Evaluates `BLEZL`.
    pub const fn blezl(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) <= 0, offset, true)
    }

    /// Evaluates `BGTZ`.
    pub const fn bgtz(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) > 0, offset, false)
    }

    /// Evaluates `BGTZL`.
    pub const fn bgtzl(branch_pc: u64, value: u64, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, (value as i64) > 0, offset, true)
    }

    /// Evaluates `BLTZAL`.
    pub const fn bltzal(branch_pc: u64, value: u64, offset: i16) -> Mips4LinkedBranchDecision {
        linked_branch(branch_pc, Self::bltz(branch_pc, value, offset))
    }

    /// Evaluates `BLTZALL`.
    pub const fn bltzall(branch_pc: u64, value: u64, offset: i16) -> Mips4LinkedBranchDecision {
        linked_branch(branch_pc, Self::bltzl(branch_pc, value, offset))
    }

    /// Evaluates `BGEZAL`.
    pub const fn bgezal(branch_pc: u64, value: u64, offset: i16) -> Mips4LinkedBranchDecision {
        linked_branch(branch_pc, Self::bgez(branch_pc, value, offset))
    }

    /// Evaluates `BGEZALL`.
    pub const fn bgezall(branch_pc: u64, value: u64, offset: i16) -> Mips4LinkedBranchDecision {
        linked_branch(branch_pc, Self::bgezl(branch_pc, value, offset))
    }

    /// Evaluates `BC1F`.
    ///
    /// `fcc` is the floating-point condition code bit selected by the instruction
    /// (`FCC[cc]`); the branch is taken when the condition code is false.
    pub const fn bc1f(branch_pc: u64, fcc: bool, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, !fcc, offset, false)
    }

    /// Evaluates `BC1T`.
    ///
    /// `fcc` is the floating-point condition code bit selected by the instruction
    /// (`FCC[cc]`); the branch is taken when the condition code is true.
    pub const fn bc1t(branch_pc: u64, fcc: bool, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, fcc, offset, false)
    }

    /// Evaluates `BC1FL`, nullifying the delay slot when the branch is not taken.
    pub const fn bc1fl(branch_pc: u64, fcc: bool, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, !fcc, offset, true)
    }

    /// Evaluates `BC1TL`, nullifying the delay slot when the branch is not taken.
    pub const fn bc1tl(branch_pc: u64, fcc: bool, offset: i16) -> Mips4BranchDecision {
        conditional_branch(branch_pc, fcc, offset, true)
    }

    /// Evaluates `J`.
    pub const fn j(branch_pc: u64, target: u32) -> Mips4BranchDecision {
        Mips4BranchDecision::taken(jump_target(branch_pc, target))
    }

    /// Evaluates `JAL`.
    pub const fn jal(branch_pc: u64, target: u32) -> Mips4LinkedBranchDecision {
        linked_branch(branch_pc, Self::j(branch_pc, target))
    }

    /// Evaluates `JR`.
    pub const fn jr(target: u64) -> Result<Mips4BranchDecision, Mips4Exception> {
        match aligned_target(target) {
            Ok(()) => Ok(Mips4BranchDecision::taken(target)),
            Err(error) => Err(error),
        }
    }

    /// Evaluates `JALR`.
    pub const fn jalr(
        branch_pc: u64,
        target: u64,
    ) -> Result<Mips4LinkedBranchDecision, Mips4Exception> {
        match Self::jr(target) {
            Ok(decision) => Ok(linked_branch(branch_pc, decision)),
            Err(error) => Err(error),
        }
    }
}

const fn conditional_branch(
    branch_pc: u64,
    taken: bool,
    offset: i16,
    likely: bool,
) -> Mips4BranchDecision {
    if taken {
        Mips4BranchDecision::taken(branch_target(branch_pc, offset))
    } else {
        Mips4BranchDecision::not_taken(likely)
    }
}

const fn linked_branch(branch_pc: u64, decision: Mips4BranchDecision) -> Mips4LinkedBranchDecision {
    Mips4LinkedBranchDecision {
        decision,
        link_value: branch_pc.wrapping_add(8),
    }
}

const fn branch_target(branch_pc: u64, offset: i16) -> u64 {
    branch_pc
        .wrapping_add(4)
        .wrapping_add((offset as i64 as u64).wrapping_shl(2))
}

const fn jump_target(branch_pc: u64, target: u32) -> u64 {
    (branch_pc.wrapping_add(4) & !0x0fff_ffff) | (((target & 0x03ff_ffff) as u64) << 2)
}

const fn aligned_target(target: u64) -> Result<(), Mips4Exception> {
    if target & 0x3 == 0 {
        Ok(())
    } else {
        Err(Mips4Exception::AddressErrorLoad)
    }
}

#[cfg(test)]
mod tests;
