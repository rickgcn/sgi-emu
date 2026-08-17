//! Computes delayed control-transfer continuations for `BEQ`, `BNE`, and `J`.
//!
//! Branch handlers resolve conditions and targets before constructing a
//! [`PcEffect::DelayedTransfer`]. A transfer encountered in a delay slot returns
//! [`ExecuteError::UnpredictableControlFlow`] without producing a commit.

use crate::commit::CpuCommit;
use crate::cpu::Cpu;
use crate::execute::ExecuteError;
use crate::gpr::Reg;
use crate::pc::PcEffect;

pub(super) fn execute_beq(
    cpu: &Cpu,
    rs: Reg,
    rt: Reg,
    offset: i16,
) -> Result<CpuCommit, ExecuteError> {
    let after_delay_slot = if cpu.read_gpr(rs) == cpu.read_gpr(rt) {
        branch_target(cpu.pc_state().current(), offset)
    } else {
        branch_fallthrough(cpu.pc_state().current())
    };
    delayed_transfer(cpu, after_delay_slot)
}

pub(super) fn execute_bne(
    cpu: &Cpu,
    rs: Reg,
    rt: Reg,
    offset: i16,
) -> Result<CpuCommit, ExecuteError> {
    let after_delay_slot = if cpu.read_gpr(rs) != cpu.read_gpr(rt) {
        branch_target(cpu.pc_state().current(), offset)
    } else {
        branch_fallthrough(cpu.pc_state().current())
    };
    delayed_transfer(cpu, after_delay_slot)
}

pub(super) fn execute_j(cpu: &Cpu, index: u32) -> Result<CpuCommit, ExecuteError> {
    delayed_transfer(cpu, jump_target(cpu.pc_state().current(), index))
}

fn delayed_transfer(cpu: &Cpu, after_delay_slot: u64) -> Result<CpuCommit, ExecuteError> {
    if cpu.pc_state().is_delay_slot() {
        let branch_pc = cpu
            .pc_state()
            .delay_slot_of()
            .expect("a delay-slot state must retain its control-transfer origin");
        return Err(ExecuteError::UnpredictableControlFlow {
            instruction_pc: cpu.pc_state().current(),
            branch_pc,
        });
    }

    Ok(CpuCommit::new(PcEffect::DelayedTransfer {
        after_delay_slot,
    }))
}

fn branch_target(current_pc: u64, offset: i16) -> u64 {
    let displacement = (i64::from(offset) << 2) as u64;
    current_pc.wrapping_add(4).wrapping_add(displacement)
}

fn branch_fallthrough(current_pc: u64) -> u64 {
    current_pc.wrapping_add(8)
}

fn jump_target(current_pc: u64, index: u32) -> u64 {
    let delay_slot_pc = current_pc.wrapping_add(4);
    (delay_slot_pc & !0x0fff_ffff) | (u64::from(index) << 2)
}

#[cfg(test)]
mod tests {
    use super::{execute_beq, execute_bne, execute_j};
    use crate::commit::CpuCommit;
    use crate::cp0::Cp0;
    use crate::cpu::Cpu;
    use crate::execute::ExecuteError;
    use crate::gpr::{GprFile, Reg};
    use crate::pc::{PcEffect, PcState};
    use crate::timing::ProcessorClock;

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    fn cpu_at(current: u64, initial: &[(Reg, u64)]) -> Cpu {
        let mut gpr = GprFile::new();
        for &(register, value) in initial {
            gpr.write(register, value);
        }
        Cpu::from_parts(
            gpr,
            PcState::new(current),
            Cp0::synthetic_test_state(false),
            ProcessorClock::new(1_000_000_000).unwrap(),
        )
    }

    #[test]
    fn beq_selects_taken_and_fallthrough_continuations() {
        let left = reg(1);
        let right = reg(2);
        let mut taken = cpu_at(0x1000, &[(left, 7), (right, 7)]);
        let mut not_taken = cpu_at(0x1000, &[(left, 7), (right, 8)]);

        let taken_commit = execute_beq(&taken, left, right, 3).expect("BEQ must create a transfer");
        let not_taken_commit = execute_beq(&not_taken, left, right, 3)
            .expect("BEQ must create a fallthrough transfer");
        taken.apply_commit(taken_commit);
        not_taken.apply_commit(not_taken_commit);

        assert_eq!(taken.pc_state().current(), 0x1004);
        assert_eq!(taken.pc_state().next(), 0x1010);
        assert_eq!(taken.pc_state().delay_slot_of(), Some(0x1000));
        assert_eq!(not_taken.pc_state().current(), 0x1004);
        assert_eq!(not_taken.pc_state().next(), 0x1008);
        assert_eq!(not_taken.pc_state().delay_slot_of(), Some(0x1000));
    }

    #[test]
    fn bne_selects_taken_and_fallthrough_continuations() {
        let left = reg(1);
        let right = reg(2);
        let mut taken = cpu_at(0x1000, &[(left, 7), (right, 8)]);
        let mut not_taken = cpu_at(0x1000, &[(left, 7), (right, 7)]);

        let taken_commit =
            execute_bne(&taken, left, right, -2).expect("BNE must create a transfer");
        let not_taken_commit = execute_bne(&not_taken, left, right, -2)
            .expect("BNE must create a fallthrough transfer");
        taken.apply_commit(taken_commit);
        not_taken.apply_commit(not_taken_commit);

        assert_eq!(taken.pc_state().next(), 0x0ffc);
        assert_eq!(taken.pc_state().delay_slot_of(), Some(0x1000));
        assert_eq!(not_taken.pc_state().next(), 0x1008);
        assert_eq!(not_taken.pc_state().delay_slot_of(), Some(0x1000));
    }

    #[test]
    fn jump_uses_the_region_containing_the_delay_slot() {
        let mut cpu = cpu_at(0x0fff_fffc, &[]);

        let commit = execute_j(&cpu, 1).expect("J must create a transfer");
        cpu.apply_commit(commit);

        assert_eq!(cpu.pc_state().current(), 0x1000_0000);
        assert_eq!(cpu.pc_state().next(), 0x1000_0004);
        assert_eq!(cpu.pc_state().delay_slot_of(), Some(0x0fff_fffc));
    }

    #[test]
    fn jump_preserves_the_high_64_bit_region() {
        let mut cpu = cpu_at(0xffff_ffff_8fff_fffc, &[]);

        let commit = execute_j(&cpu, 1).expect("J must create a transfer");
        cpu.apply_commit(commit);

        assert_eq!(cpu.pc_state().current(), 0xffff_ffff_9000_0000);
        assert_eq!(cpu.pc_state().next(), 0xffff_ffff_9000_0004);
        assert_eq!(cpu.pc_state().delay_slot_of(), Some(0xffff_ffff_8fff_fffc));
    }

    #[test]
    fn delayed_control_transfer_returns_an_explicit_stop() {
        let mut cpu = cpu_at(0x1000, &[]);
        cpu.apply_commit(CpuCommit::new(PcEffect::DelayedTransfer {
            after_delay_slot: 0x2000,
        }));

        assert_eq!(
            execute_j(&cpu, 0),
            Err(ExecuteError::UnpredictableControlFlow {
                instruction_pc: 0x1004,
                branch_pc: 0x1000,
            })
        );
    }
}
