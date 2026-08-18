//! Implements system-instruction exception requests and exception return.
//!
//! `SYSCALL` and `BREAK` retain their encoded software code in the typed instruction
//! and produce guest exception requests. `ERET` validates placement and CP0
//! usability, then produces a staged commit from immutable pre-state. Handlers do
//! not mutate live `CP0` state or map guest `BREAK` to host debugger behavior.

use crate::commit::CpuCommit;
use crate::cpu::Cpu;
use crate::exception::ExceptionRequest;
use crate::execute::{ExecuteError, InstructionOutcome};

pub(super) const fn execute_syscall(_code: u32) -> InstructionOutcome {
    InstructionOutcome::Exception(ExceptionRequest::Syscall)
}

pub(super) const fn execute_break(_code: u32) -> InstructionOutcome {
    InstructionOutcome::Exception(ExceptionRequest::Breakpoint)
}

pub(super) fn execute_eret(cpu: &Cpu) -> Result<InstructionOutcome, ExecuteError> {
    if let Some(branch_pc) = cpu.pc_state().delay_slot_of() {
        return Err(ExecuteError::UnpredictableControlFlow {
            instruction_pc: cpu.pc_state().current(),
            branch_pc,
        });
    }

    if !cpu.cp0().cp0_usable() {
        return Ok(InstructionOutcome::Exception(
            ExceptionRequest::CoprocessorUnusable { coprocessor: 0 },
        ));
    }

    Ok(InstructionOutcome::Commit(CpuCommit::exception_return(
        cpu.cp0().exception_return_decision(),
    )))
}

#[cfg(test)]
mod tests {
    use super::{execute_break, execute_eret, execute_syscall};
    use crate::commit::CpuCommit;
    use crate::cp0::{Cp0, OperatingMode, SyntheticCp0State};
    use crate::cpu::Cpu;
    use crate::exception::{ExceptionCode, ExceptionRequest};
    use crate::execute::{ExecuteError, InstructionOutcome};
    use crate::gpr::GprFile;
    use crate::pc::{PcEffect, PcState};
    use crate::timing::ProcessorClock;

    fn cpu_with(state: SyntheticCp0State) -> Cpu {
        Cpu::from_parts(
            GprFile::new(),
            PcState::new(0x8000),
            Cp0::synthetic_test_state_with(state),
            ProcessorClock::new(1_000_000_000).unwrap(),
        )
    }

    fn apply_eret(cpu: &mut Cpu) {
        let outcome = execute_eret(cpu).expect("valid ERET placement must execute");
        let InstructionOutcome::Commit(commit) = outcome else {
            panic!("usable CP0 must produce an exception-return commit");
        };
        cpu.apply_commit(commit);
    }

    #[test]
    fn syscall_requests_a_guest_system_call_exception() {
        assert_eq!(
            execute_syscall(0xabcde),
            InstructionOutcome::Exception(ExceptionRequest::Syscall)
        );
    }

    #[test]
    fn break_requests_a_guest_breakpoint_exception() {
        assert_eq!(
            execute_break(0x54321),
            InstructionOutcome::Exception(ExceptionRequest::Breakpoint)
        );
    }

    #[test]
    fn eret_uses_epc_and_clears_exl() {
        let mut cpu = cpu_with(
            SyntheticCp0State::new(false)
                .with_exception_levels(true, false)
                .with_return_addresses(0x1000, 0x2000),
        );

        apply_eret(&mut cpu);

        assert!(!cpu.cp0().exl());
        assert!(!cpu.cp0().erl());
        assert_eq!(cpu.pc_state().current(), 0x1000);
        assert_eq!(cpu.pc_state().next(), 0x1004);
        assert_eq!(cpu.pc_state().delay_slot_of(), None);
    }

    #[test]
    fn eret_uses_error_epc_before_a_second_return_uses_epc() {
        let mut cpu = cpu_with(
            SyntheticCp0State::new(false)
                .with_exception_levels(true, true)
                .with_return_addresses(0x1000, u64::MAX - 3),
        );

        apply_eret(&mut cpu);

        assert!(cpu.cp0().exl());
        assert!(!cpu.cp0().erl());
        assert_eq!(cpu.pc_state().current(), u64::MAX - 3);
        assert_eq!(cpu.pc_state().next(), 0);

        apply_eret(&mut cpu);

        assert!(!cpu.cp0().exl());
        assert_eq!(cpu.pc_state().current(), 0x1000);
        assert_eq!(cpu.pc_state().next(), 0x1004);
    }

    #[test]
    fn eret_without_an_active_level_still_uses_epc() {
        let mut cpu = cpu_with(
            SyntheticCp0State::new(false)
                .with_exception_levels(false, false)
                .with_return_addresses(0x3000, 0x4000),
        );

        apply_eret(&mut cpu);

        assert!(!cpu.cp0().exl());
        assert!(!cpu.cp0().erl());
        assert_eq!(cpu.pc_state().current(), 0x3000);
    }

    #[test]
    fn unavailable_cp0_requests_coprocessor_unusable_with_ce_zero() {
        let mut cpu = cpu_with(
            SyntheticCp0State::new(false)
                .with_operating_mode(OperatingMode::User, false)
                .with_return_addresses(0x3000, 0x4000),
        );

        let outcome = execute_eret(&cpu).unwrap();

        assert_eq!(
            outcome,
            InstructionOutcome::Exception(ExceptionRequest::CoprocessorUnusable { coprocessor: 0 })
        );
        assert_eq!(cpu.pc_state().current(), 0x8000);

        let InstructionOutcome::Exception(request) = outcome else {
            unreachable!();
        };
        cpu.apply_exception(request);
        assert_eq!(
            cpu.cp0().exception_code(),
            ExceptionCode::CoprocessorUnusable
        );
        assert_eq!(cpu.cp0().coprocessor_error(), 0);
        assert_eq!(cpu.cp0().epc(), 0x8000);
    }

    #[test]
    fn cu0_allows_eret_in_user_mode() {
        let mut cpu = cpu_with(
            SyntheticCp0State::new(false)
                .with_operating_mode(OperatingMode::User, true)
                .with_return_addresses(0x3000, 0x4000),
        );

        apply_eret(&mut cpu);

        assert_eq!(cpu.pc_state().current(), 0x3000);
    }

    #[test]
    fn delay_slot_rejection_precedes_cp0_usability_and_preserves_state() {
        let mut cpu = cpu_with(
            SyntheticCp0State::new(false)
                .with_operating_mode(OperatingMode::User, false)
                .with_return_addresses(0x3000, 0x4000),
        );
        cpu.apply_commit(CpuCommit::new(PcEffect::DelayedTransfer {
            after_delay_slot: 0x9000,
        }));
        let before = cpu.clone();

        assert_eq!(
            execute_eret(&cpu),
            Err(ExecuteError::UnpredictableControlFlow {
                instruction_pc: 0x8004,
                branch_pc: 0x8000,
            })
        );
        assert_eq!(cpu, before);
    }
}
