//! Owns the live GPR and program-counter state used by instruction semantics.
//!
//! Execution borrows [`Cpu`] immutably and produces a [`CpuCommit`]. For ordinary
//! instruction normal retirement, [`Cpu::apply_commit`] is the sole mutation path,
//! so handlers cannot expose a partially applied write-set.

use crate::commit::CpuCommit;
use crate::gpr::{GprFile, Reg};
use crate::pc::PcState;

/// Holds live architectural state consumed by instruction semantics.
///
/// Typed execution only borrows this state. Normal retirement applies a validated
/// [`CpuCommit`] through [`Self::apply_commit`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Cpu {
    gpr: GprFile,
    pc: PcState,
}

impl Cpu {
    pub(crate) const fn from_parts(gpr: GprFile, pc: PcState) -> Self {
        Self { gpr, pc }
    }

    pub(crate) fn read_gpr(&self, reg: Reg) -> u64 {
        self.gpr.read(reg)
    }

    pub(crate) const fn pc_state(&self) -> &PcState {
        &self.pc
    }

    /// Applies a validated write-set for one ordinary instruction's normal retirement.
    ///
    /// This is the only mutation path for ordinary instruction normal retirement.
    ///
    /// # Panics
    ///
    /// Panics if the commit contains a delayed transfer while the current
    /// instruction already occupies a delay slot.
    pub(crate) fn apply_commit(&mut self, commit: CpuCommit) {
        let (gpr, pc) = commit.into_parts();
        self.pc.apply(pc);
        if let Some(write) = gpr {
            let (destination, value) = write.into_parts();
            self.gpr.write(destination, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Cpu;
    use crate::decode::Instruction;
    use crate::execute::execute;
    use crate::gpr::{GprFile, Reg};
    use crate::pc::PcState;

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    #[test]
    fn execute_builds_a_candidate_before_normal_retirement() {
        let source = reg(1);
        let destination = reg(2);
        let mut gpr = GprFile::new();
        gpr.write(source, 0x1234);
        let mut cpu = Cpu::from_parts(gpr, PcState::new(0x1000));
        let instruction = Instruction::Ori {
            rt: destination,
            rs: source,
            immediate: 0x00f0,
        };

        let commit = execute(&cpu, instruction).expect("ORI must execute normally");

        assert_eq!(cpu.read_gpr(destination), 0);
        assert_eq!(cpu.pc_state().current(), 0x1000);

        cpu.apply_commit(commit);

        assert_eq!(cpu.read_gpr(destination), 0x12f4);
        assert_eq!(cpu.pc_state().current(), 0x1004);
    }
}
