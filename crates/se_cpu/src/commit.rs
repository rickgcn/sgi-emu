//! Represents normal retirement as a bounded architectural write-set.
//!
//! Instruction handlers construct [`CpuCommit`] without mutating live CPU state.
//! A commit carries at most one GPR write and exactly one [`PcEffect`]. Memory and
//! device effects are not represented here.

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

/// Holds CPU architectural writes pending normal retirement.
///
/// The write-set contains at most one GPR write and exactly one [`PcEffect`].
/// Constructing a commit does not modify live CPU state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CpuCommit {
    gpr: Option<GprWrite>,
    pc: PcEffect,
}

impl CpuCommit {
    pub(crate) const fn new(pc: PcEffect) -> Self {
        Self { gpr: None, pc }
    }

    /// Sets the pending GPR write, replacing any previously selected write.
    pub(crate) const fn with_gpr_write(mut self, destination: Reg, value: u64) -> Self {
        self.gpr = Some(GprWrite::new(destination, value));
        self
    }

    pub(crate) const fn into_parts(self) -> (Option<GprWrite>, PcEffect) {
        (self.gpr, self.pc)
    }
}
