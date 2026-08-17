//! Computes normal write-sets and guest exception requests for integer instructions.
//!
//! Word operations use the low 32 bits and sign-extend their results to 64 bits.
//! Full-width logical operations preserve all 64 bits. Word additions report
//! [`ExecuteError::UndefinedResult`] when an input is not a sign-extended word;
//! signed `ADD` overflow produces a guest exception request instead.

use crate::commit::CpuCommit;
use crate::cpu::Cpu;
use crate::decode::Instruction;
use crate::exception::ExceptionRequest;
use crate::execute::{ExecuteError, InstructionOutcome};
use crate::gpr::Reg;
use crate::pc::PcEffect;

pub(super) fn execute_sll(cpu: &Cpu, rd: Reg, rt: Reg, shift: u8) -> CpuCommit {
    let word = (cpu.read_gpr(rt) as u32) << shift;
    CpuCommit::new(PcEffect::Sequential).with_gpr_write(rd, sign_extend_word(word))
}

pub(super) fn execute_add(
    cpu: &Cpu,
    rd: Reg,
    rs: Reg,
    rt: Reg,
) -> Result<InstructionOutcome, ExecuteError> {
    let left = cpu.read_gpr(rs);
    let right = cpu.read_gpr(rt);
    if !is_sign_extended_word(left) || !is_sign_extended_word(right) {
        return Err(ExecuteError::UndefinedResult {
            instruction: Instruction::Add { rd, rs, rt },
        });
    }

    let Some(result) = (left as u32 as i32).checked_add(right as u32 as i32) else {
        return Ok(InstructionOutcome::Exception(
            ExceptionRequest::IntegerOverflow,
        ));
    };

    Ok(InstructionOutcome::Commit(
        CpuCommit::new(PcEffect::Sequential).with_gpr_write(rd, sign_extend_word(result as u32)),
    ))
}

pub(super) fn execute_addiu(
    cpu: &Cpu,
    rt: Reg,
    rs: Reg,
    immediate: i16,
) -> Result<CpuCommit, ExecuteError> {
    let source = cpu.read_gpr(rs);
    if !is_sign_extended_word(source) {
        return Err(ExecuteError::UndefinedResult {
            instruction: Instruction::Addiu { rt, rs, immediate },
        });
    }

    let immediate = (i32::from(immediate)) as u32;
    let word = (source as u32).wrapping_add(immediate);
    Ok(CpuCommit::new(PcEffect::Sequential).with_gpr_write(rt, sign_extend_word(word)))
}

pub(super) fn execute_or(cpu: &Cpu, rd: Reg, rs: Reg, rt: Reg) -> CpuCommit {
    let value = cpu.read_gpr(rs) | cpu.read_gpr(rt);
    CpuCommit::new(PcEffect::Sequential).with_gpr_write(rd, value)
}

pub(super) fn execute_ori(cpu: &Cpu, rt: Reg, rs: Reg, immediate: u16) -> CpuCommit {
    let value = cpu.read_gpr(rs) | u64::from(immediate);
    CpuCommit::new(PcEffect::Sequential).with_gpr_write(rt, value)
}

pub(super) fn execute_lui(rt: Reg, immediate: u16) -> CpuCommit {
    let word = u32::from(immediate) << 16;
    CpuCommit::new(PcEffect::Sequential).with_gpr_write(rt, sign_extend_word(word))
}

fn is_sign_extended_word(value: u64) -> bool {
    value == sign_extend_word(value as u32)
}

fn sign_extend_word(value: u32) -> u64 {
    i64::from(value as i32) as u64
}

#[cfg(test)]
mod tests {
    use super::{execute_add, execute_addiu, execute_lui, execute_or, execute_ori, execute_sll};
    use crate::cp0::Cp0;
    use crate::cpu::Cpu;
    use crate::decode::Instruction;
    use crate::exception::ExceptionRequest;
    use crate::execute::{ExecuteError, InstructionOutcome};
    use crate::gpr::{GprFile, Reg};
    use crate::pc::PcState;
    use crate::timing::ProcessorClock;

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    fn cpu_with(initial: &[(Reg, u64)]) -> Cpu {
        let mut gpr = GprFile::new();
        for &(register, value) in initial {
            gpr.write(register, value);
        }
        Cpu::from_parts(
            gpr,
            PcState::new(0x1000),
            Cp0::synthetic_test_state(false),
            ProcessorClock::new(1_000_000_000).unwrap(),
        )
    }

    fn apply_commit(cpu: &mut Cpu, outcome: InstructionOutcome) {
        let InstructionOutcome::Commit(commit) = outcome else {
            panic!("test instruction must produce a normal commit");
        };
        cpu.apply_commit(commit);
    }

    #[test]
    fn sll_uses_the_low_word_and_sign_extends_the_result() {
        let source = reg(1);
        let destination = reg(2);
        let mut cpu = cpu_with(&[(source, 0x0123_4567_8000_0001)]);

        let commit = execute_sll(&cpu, destination, source, 0);
        cpu.apply_commit(commit);

        assert_eq!(cpu.read_gpr(destination), 0xffff_ffff_8000_0001);
    }

    #[test]
    fn addiu_wraps_a_word_and_sign_extends_the_result() {
        let source = reg(1);
        let destination = reg(2);
        let mut cpu = cpu_with(&[(source, 0x0000_0000_7fff_ffff)]);

        let commit = execute_addiu(&cpu, destination, source, 1)
            .expect("a canonical word operand must produce a result");
        cpu.apply_commit(commit);

        assert_eq!(cpu.read_gpr(destination), 0xffff_ffff_8000_0000);
    }

    #[test]
    fn add_sign_extends_a_negative_word_result() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut cpu = cpu_with(&[(left, u64::MAX - 2), (right, 1)]);

        let outcome = execute_add(&cpu, destination, left, right)
            .expect("canonical ADD operands must have defined semantics");
        apply_commit(&mut cpu, outcome);

        assert_eq!(cpu.read_gpr(destination), u64::MAX - 1);
    }

    #[test]
    fn add_reports_positive_and_negative_word_overflow() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let positive = cpu_with(&[(left, 0x7fff_ffff), (right, 1)]);
        let negative = cpu_with(&[(left, 0xffff_ffff_8000_0000), (right, u64::MAX)]);

        for cpu in [&positive, &negative] {
            assert_eq!(
                execute_add(cpu, destination, left, right),
                Ok(InstructionOutcome::Exception(
                    ExceptionRequest::IntegerOverflow
                ))
            );
        }
    }

    #[test]
    fn add_rejects_either_noncanonical_word_operand() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let cases = [
            cpu_with(&[(left, 0x0000_0001_0000_0000), (right, 1)]),
            cpu_with(&[(left, 1), (right, 0x0000_0001_0000_0000)]),
        ];

        for cpu in cases {
            assert_eq!(
                execute_add(&cpu, destination, left, right),
                Err(ExecuteError::UndefinedResult {
                    instruction: Instruction::Add {
                        rd: destination,
                        rs: left,
                        rt: right,
                    },
                })
            );
        }
    }

    #[test]
    fn addiu_rejects_a_noncanonical_word_operand() {
        let source = reg(1);
        let destination = reg(2);
        let cpu = cpu_with(&[(source, 0x0000_0001_0000_0000)]);

        assert_eq!(
            execute_addiu(&cpu, destination, source, 1),
            Err(ExecuteError::UndefinedResult {
                instruction: Instruction::Addiu {
                    rt: destination,
                    rs: source,
                    immediate: 1,
                },
            })
        );
    }

    #[test]
    fn or_combines_full_register_width() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut cpu = cpu_with(&[
            (left, 0x8000_0000_0000_0001),
            (right, 0x0000_0001_0000_0010),
        ]);

        let commit = execute_or(&cpu, destination, left, right);
        cpu.apply_commit(commit);

        assert_eq!(cpu.read_gpr(destination), 0x8000_0001_0000_0011);
    }

    #[test]
    fn ori_zero_extends_the_immediate() {
        let source = reg(1);
        let destination = reg(2);
        let mut cpu = cpu_with(&[(source, 0x8000_0000_0000_0000)]);

        let commit = execute_ori(&cpu, destination, source, 0x8001);
        cpu.apply_commit(commit);

        assert_eq!(cpu.read_gpr(destination), 0x8000_0000_0000_8001);
    }

    #[test]
    fn lui_sign_extends_the_constructed_word() {
        let destination = reg(2);
        let mut cpu = cpu_with(&[]);

        let commit = execute_lui(destination, 0x8001);
        cpu.apply_commit(commit);

        assert_eq!(cpu.read_gpr(destination), 0xffff_ffff_8001_0000);
    }
}
