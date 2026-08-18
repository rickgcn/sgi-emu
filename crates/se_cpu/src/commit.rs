//! Represents instruction commits as bounded architectural write-sets.
//!
//! Instruction handlers construct [`CpuCommit`] without mutating live CPU state.
//! A commit carries at most one GPR write, an optional bounded CP0 effect, and
//! exactly one program-counter effect. Memory and device effects are not
//! represented here.

use crate::cp0::{Cp0Effect, ExceptionReturnDecision};
use crate::gpr::Reg;
use crate::pc::PcEffect;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct GprWrite {
    destination: Reg,
    value: u64,
}

impl GprWrite {
    const fn new(destination: Reg, value: u64) -> Self {
        Self { destination, value }
    }

    pub(crate) const fn into_parts(self) -> (Reg, u64) {
        (self.destination, self.value)
    }
}

/// Describes the program-counter mutation carried by a CPU commit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PcCommitEffect {
    /// Applies an ordinary sequential or delayed-transfer effect.
    Normal(PcEffect),
    /// Replaces control-flow state with an exception-return target.
    ExceptionReturn { target: u64 },
}

/// Carries one instruction's bounded architectural write-set.
///
/// The write-set contains at most one GPR write, an optional CP0 effect, and
/// exactly one program-counter effect. Constructing a commit does not modify live
/// CPU state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CpuCommit {
    gpr: Option<GprWrite>,
    cp0: Option<Cp0Effect>,
    pc: PcCommitEffect,
}

impl CpuCommit {
    pub(crate) const fn new(pc: PcEffect) -> Self {
        Self {
            gpr: None,
            cp0: None,
            pc: PcCommitEffect::Normal(pc),
        }
    }

    /// Constructs the inseparable CP0 and PC effects of an exception return.
    pub(crate) const fn exception_return(decision: ExceptionReturnDecision) -> Self {
        Self {
            gpr: None,
            cp0: Some(Cp0Effect::ExceptionReturn {
                level: decision.level(),
            }),
            pc: PcCommitEffect::ExceptionReturn {
                target: decision.target(),
            },
        }
    }

    /// Sets the pending GPR write, replacing any previously selected write.
    pub(crate) const fn with_gpr_write(mut self, destination: Reg, value: u64) -> Self {
        self.gpr = Some(GprWrite::new(destination, value));
        self
    }

    pub(crate) const fn into_parts(self) -> (Option<GprWrite>, Option<Cp0Effect>, PcCommitEffect) {
        (self.gpr, self.cp0, self.pc)
    }
}
