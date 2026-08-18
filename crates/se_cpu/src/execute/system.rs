//! Implements system exceptions, bounded CP0 moves, indexed TLB writes, and exception return.
//!
//! `SYSCALL` and `BREAK` retain their encoded software code in the typed instruction
//! and produce guest exception requests. CP0 moves expose only the registers used
//! by the refill path, and `TLBWI` captures an immutable indexed-write decision.
//! `ERET` validates placement and CP0 usability before producing its staged
//! return. Handlers do not mutate live CPU state or map guest `BREAK` to host
//! debugger behavior.

use crate::commit::CpuCommit;
use crate::cp0::Cp0Effect;
use crate::cpu::Cpu;
use crate::decode::{Cp0Register, Instruction};
use crate::exception::ExceptionRequest;
use crate::execute::{ExecuteError, InstructionOutcome};
use crate::gpr::Reg;
use crate::pc::PcEffect;

pub(super) const fn execute_syscall(_code: u32) -> InstructionOutcome {
    InstructionOutcome::Exception(ExceptionRequest::Syscall)
}

pub(super) const fn execute_break(_code: u32) -> InstructionOutcome {
    InstructionOutcome::Exception(ExceptionRequest::Breakpoint)
}

pub(super) fn execute_mfc0(
    cpu: &Cpu,
    destination: Reg,
    register: Cp0Register,
) -> Result<InstructionOutcome, ExecuteError> {
    if !cpu.cp0().cp0_usable() {
        return Ok(cp0_unusable());
    }

    let value = match register {
        Cp0Register::CONTEXT => cpu.cp0().mfc0_context(),
        _ => {
            return Err(ExecuteError::Cp0RegisterImplementationGap {
                instruction: Instruction::Mfc0 {
                    rt: destination,
                    register,
                },
            });
        }
    };
    Ok(InstructionOutcome::Commit(
        CpuCommit::new(PcEffect::Sequential).with_gpr_write(destination, value),
    ))
}

pub(super) fn execute_mtc0(
    cpu: &Cpu,
    source: Reg,
    register: Cp0Register,
) -> Result<InstructionOutcome, ExecuteError> {
    if !cpu.cp0().cp0_usable() {
        return Ok(cp0_unusable());
    }

    let value = cpu.read_gpr(source);
    let effect = match register {
        Cp0Register::INDEX => Cp0Effect::write_index(value),
        Cp0Register::ENTRY_LO0 => Cp0Effect::write_entry_lo0(value),
        Cp0Register::ENTRY_LO1 => Cp0Effect::write_entry_lo1(value),
        _ => {
            return Err(ExecuteError::Cp0RegisterImplementationGap {
                instruction: Instruction::Mtc0 {
                    rt: source,
                    register,
                },
            });
        }
    };
    Ok(InstructionOutcome::Commit(
        CpuCommit::new(PcEffect::Sequential).with_cp0_effect(effect),
    ))
}

pub(super) fn execute_tlbwi(cpu: &Cpu) -> InstructionOutcome {
    if !cpu.cp0().cp0_usable() {
        return cp0_unusable();
    }
    InstructionOutcome::Commit(CpuCommit::tlb_write(cpu.tlbwi_decision()))
}

pub(super) fn execute_eret(cpu: &Cpu) -> Result<InstructionOutcome, ExecuteError> {
    if let Some(branch_pc) = cpu.pc_state().delay_slot_of() {
        return Err(ExecuteError::UnpredictableControlFlow {
            instruction_pc: cpu.pc_state().current(),
            branch_pc,
        });
    }

    if !cpu.cp0().cp0_usable() {
        return Ok(cp0_unusable());
    }

    Ok(InstructionOutcome::Commit(CpuCommit::exception_return(
        cpu.cp0().exception_return_decision(),
    )))
}

const fn cp0_unusable() -> InstructionOutcome {
    InstructionOutcome::Exception(ExceptionRequest::CoprocessorUnusable { coprocessor: 0 })
}

#[cfg(test)]
mod tests {
    use se_core::address::PhysAddr;

    use super::{
        execute_break, execute_eret, execute_mfc0, execute_mtc0, execute_syscall, execute_tlbwi,
    };
    use crate::commit::CpuCommit;
    use crate::cp0::{Cp0, OperatingMode, SyntheticCp0State};
    use crate::cpu::Cpu;
    use crate::decode::{Cp0Register, Instruction};
    use crate::exception::{ExceptionCode, ExceptionRequest};
    use crate::execute::{ExecuteError, InstructionOutcome};
    use crate::gpr::{GprFile, Reg};
    use crate::memory::AccessKind;
    use crate::pc::{PcEffect, PcState};
    use crate::timing::ProcessorClock;
    use crate::tlb::{TlbFault, TlbFaultReason, TlbTranslation};

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    fn cpu_with(state: SyntheticCp0State) -> Cpu {
        cpu_with_gprs(state, &[])
    }

    fn cpu_with_gprs(state: SyntheticCp0State, initial_gprs: &[(Reg, u64)]) -> Cpu {
        let mut gpr = GprFile::new();
        for &(register, value) in initial_gprs {
            gpr.write(register, value);
        }
        Cpu::from_parts(
            gpr,
            PcState::new(0x8000),
            Cp0::synthetic_test_state_with(state),
            ProcessorClock::new(1_000_000_000).unwrap(),
        )
    }

    fn apply_commit(cpu: &mut Cpu, outcome: InstructionOutcome) {
        let InstructionOutcome::Commit(commit) = outcome else {
            panic!("usable CP0 operation must produce a commit");
        };
        cpu.apply_commit(commit);
    }

    fn apply_mtc0(cpu: &mut Cpu, source: Reg, register: Cp0Register) {
        let outcome = execute_mtc0(cpu, source, register)
            .expect("implemented CP0 register write must execute");
        apply_commit(cpu, outcome);
    }

    fn apply_tlbwi(cpu: &mut Cpu) {
        let outcome = execute_tlbwi(cpu);
        apply_commit(cpu, outcome);
    }

    const fn entry_lo(pfn: u32, valid: bool, dirty: bool, global: bool) -> u64 {
        (pfn as u64) << 6 | (dirty as u64) << 2 | (valid as u64) << 1 | global as u64
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

    #[test]
    fn cp0_unusable_precedes_move_surface_and_tlb_write() {
        let cpu =
            cpu_with(SyntheticCp0State::new(false).with_operating_mode(OperatingMode::User, false));
        let expected =
            InstructionOutcome::Exception(ExceptionRequest::CoprocessorUnusable { coprocessor: 0 });

        assert_eq!(
            execute_mfc0(&cpu, reg(2), Cp0Register::STATUS),
            Ok(expected)
        );
        assert_eq!(
            execute_mtc0(&cpu, reg(3), Cp0Register::STATUS),
            Ok(expected)
        );
        assert_eq!(execute_tlbwi(&cpu), expected);
    }

    #[test]
    fn unsupported_defined_cp0_moves_are_explicit_execution_gaps() {
        let cpu = cpu_with(SyntheticCp0State::new(false));

        assert_eq!(
            execute_mfc0(&cpu, reg(2), Cp0Register::STATUS),
            Err(ExecuteError::Cp0RegisterImplementationGap {
                instruction: Instruction::Mfc0 {
                    rt: reg(2),
                    register: Cp0Register::STATUS,
                },
            })
        );
        assert_eq!(
            execute_mtc0(&cpu, reg(3), Cp0Register::STATUS),
            Err(ExecuteError::Cp0RegisterImplementationGap {
                instruction: Instruction::Mtc0 {
                    rt: reg(3),
                    register: Cp0Register::STATUS,
                },
            })
        );
    }

    #[test]
    fn mfc0_context_sign_extends_the_diagnostic_low_word() {
        let mut cpu = cpu_with(
            SyntheticCp0State::new(false)
                .with_entry_hi_asid(0x42)
                .with_context_pte_base(0xffff_ffff_a000_0000),
        );
        cpu.apply_exception(ExceptionRequest::Tlb(TlbFault::new_for_test(
            TlbFaultReason::Refill,
            AccessKind::Load,
            0x0040_0000,
        )));

        let outcome = execute_mfc0(&cpu, reg(2), Cp0Register::CONTEXT)
            .expect("implemented Context read must execute");
        apply_commit(&mut cpu, outcome);

        assert_eq!(cpu.read_gpr(reg(2)), 0xffff_ffff_a000_2000);
        assert_eq!(cpu.cp0().entry_hi().asid(), 0x42);
    }

    #[test]
    fn mtc0_masks_index_and_parses_full_width_entry_lo_values() {
        let index_source = reg(1);
        let even_source = reg(2);
        let odd_source = reg(3);
        let even_value = (1_u64 << 62) | (0x0abc_def0_u64 << 6) | 0x3f;
        let odd_value = (1_u64 << 61) | (0x0123_4567_u64 << 6) | (1 << 1);
        let mut cpu = cpu_with_gprs(
            SyntheticCp0State::new(false),
            &[
                (index_source, 0xffff_ffff_ffff_ffff),
                (even_source, even_value),
                (odd_source, odd_value),
            ],
        );

        apply_mtc0(&mut cpu, index_source, Cp0Register::INDEX);
        apply_mtc0(&mut cpu, even_source, Cp0Register::ENTRY_LO0);
        apply_mtc0(&mut cpu, odd_source, Cp0Register::ENTRY_LO1);

        assert_eq!(cpu.cp0().index(), 63);
        assert_eq!(cpu.cp0().entry_lo0().pfn(), 0x0abc_def0);
        assert!(cpu.cp0().entry_lo0().valid());
        assert!(cpu.cp0().entry_lo0().dirty());
        assert!(cpu.cp0().entry_lo0().global());
        assert_eq!(cpu.cp0().entry_lo1().pfn(), 0x0123_4567);
        assert!(cpu.cp0().entry_lo1().valid());
        assert!(!cpu.cp0().entry_lo1().dirty());
        assert!(!cpu.cp0().entry_lo1().global());
        let TlbTranslation::Fault(fault) = cpu
            .translate_mapped_address(0x0040_0000, AccessKind::Load)
            .expect("empty authoritative TLB must be unambiguous")
        else {
            panic!("staging writes must not install an authoritative mapping");
        };
        assert_eq!(fault.reason(), TlbFaultReason::Refill);
    }

    #[test]
    fn tlbwi_commits_staging_and_refreshes_shutdown_from_conflicts() {
        let index_one = reg(1);
        let first_even = reg(2);
        let first_odd = reg(3);
        let index_two = reg(4);
        let second_even = reg(5);
        let second_odd = reg(6);
        let index_three = reg(7);
        let mut cpu = cpu_with_gprs(
            SyntheticCp0State::new(false).with_entry_hi_asid(7),
            &[
                (index_one, 1),
                (first_even, entry_lo(4, true, true, true)),
                (first_odd, entry_lo(5, true, true, false)),
                (index_two, 2),
                (second_even, entry_lo(6, true, true, true)),
                (second_odd, entry_lo(7, true, true, true)),
                (index_three, 3),
            ],
        );
        cpu.apply_exception(ExceptionRequest::Tlb(TlbFault::new_for_test(
            TlbFaultReason::Refill,
            AccessKind::Load,
            0x0040_0123,
        )));

        apply_mtc0(&mut cpu, index_one, Cp0Register::INDEX);
        apply_mtc0(&mut cpu, first_even, Cp0Register::ENTRY_LO0);
        apply_mtc0(&mut cpu, first_odd, Cp0Register::ENTRY_LO1);
        apply_tlbwi(&mut cpu);

        assert!(!cpu.cp0().tlb_shutdown());
        assert_eq!(
            cpu.translate_mapped_address(0x0040_0123, AccessKind::Load),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x4123)))
        );

        apply_mtc0(&mut cpu, index_two, Cp0Register::INDEX);
        apply_mtc0(&mut cpu, second_even, Cp0Register::ENTRY_LO0);
        apply_mtc0(&mut cpu, second_odd, Cp0Register::ENTRY_LO1);
        apply_tlbwi(&mut cpu);

        assert!(cpu.cp0().tlb_shutdown());
        assert_eq!(
            cpu.translate_mapped_address(0x0040_0123, AccessKind::Load),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x6123)))
        );

        cpu.apply_exception(ExceptionRequest::Tlb(TlbFault::new_for_test(
            TlbFaultReason::Refill,
            AccessKind::Load,
            0x0080_0123,
        )));
        apply_mtc0(&mut cpu, index_three, Cp0Register::INDEX);
        apply_tlbwi(&mut cpu);

        assert!(!cpu.cp0().tlb_shutdown());
        assert_eq!(
            cpu.translate_mapped_address(0x0080_0123, AccessKind::Load),
            Ok(TlbTranslation::Translated(PhysAddr::new(0x6123)))
        );
    }
}
