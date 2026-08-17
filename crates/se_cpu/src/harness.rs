//! Drives raw words through decode and one of the two architectural retirement paths.
//!
//! The harness constructs synthetic GPR, PC, and minimal CP0 pre-state. Reserved
//! encodings and instruction-generated exceptions are taken architecturally, while
//! decoder gaps and execution stops remain distinct errors.

use crate::cp0::Cp0;
use crate::cpu::Cpu;
use crate::decode::{DecodeGap, DecodeOutcome, decode};
use crate::exception::{ExceptionCode, ExceptionRequest};
use crate::execute::{ExecuteError, InstructionDisposition, InstructionOutcome, execute};
use crate::gpr::{GprFile, Reg};
use crate::pc::PcState;
use crate::timing::ProcessorClock;

pub(crate) struct SemanticHarness {
    cpu: Cpu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HarnessOutcome {
    Retired,
    ExceptionTaken(ExceptionRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HarnessError {
    Decode(DecodeGap),
    Execute(ExecuteError),
    TimedMemoryRequired {
        instruction: crate::decode::Instruction,
    },
}

impl SemanticHarness {
    pub(crate) fn new(entry_pc: u64) -> Self {
        Self::with_gprs(entry_pc, &[])
    }

    pub(crate) fn with_gprs(entry_pc: u64, initial: &[(Reg, u64)]) -> Self {
        Self::with_gprs_and_bev(entry_pc, initial, false)
    }

    pub(crate) fn with_gprs_and_bev(entry_pc: u64, initial: &[(Reg, u64)], bev: bool) -> Self {
        let mut gpr = GprFile::new();
        for &(register, value) in initial {
            gpr.write(register, value);
        }
        Self {
            cpu: Cpu::from_parts(
                gpr,
                PcState::new(entry_pc),
                Cp0::synthetic_test_state(bev),
                ProcessorClock::new(1_000_000_000).expect("test PClk must be representable"),
            ),
        }
    }

    pub(crate) fn read_gpr(&self, reg: Reg) -> u64 {
        self.cpu.read_gpr(reg)
    }

    pub(crate) fn current_pc(&self) -> u64 {
        self.cpu.pc_state().current()
    }

    pub(crate) fn next_pc(&self) -> u64 {
        self.cpu.pc_state().next()
    }

    pub(crate) fn delay_slot_of(&self) -> Option<u64> {
        self.cpu.pc_state().delay_slot_of()
    }

    pub(crate) fn exl(&self) -> bool {
        self.cpu.cp0().exl()
    }

    pub(crate) fn bev(&self) -> bool {
        self.cpu.cp0().bev()
    }

    pub(crate) fn exception_code(&self) -> ExceptionCode {
        self.cpu.cp0().exception_code()
    }

    pub(crate) fn branch_delay(&self) -> bool {
        self.cpu.cp0().branch_delay()
    }

    pub(crate) fn epc(&self) -> u64 {
        self.cpu.cp0().epc()
    }

    pub(crate) fn bad_vaddr(&self) -> u64 {
        self.cpu.cp0().bad_vaddr()
    }

    pub(crate) fn step(&mut self, raw: u32) -> Result<HarnessOutcome, HarnessError> {
        let instruction = match decode(raw) {
            DecodeOutcome::Instruction(instruction) => instruction,
            DecodeOutcome::ReservedEncoding { .. } => {
                let request = ExceptionRequest::ReservedInstruction;
                self.cpu.apply_exception(request);
                return Ok(HarnessOutcome::ExceptionTaken(request));
            }
            DecodeOutcome::ImplementationGap(gap) => return Err(HarnessError::Decode(gap)),
        };

        let outcome = match execute(&self.cpu, instruction).map_err(HarnessError::Execute)? {
            InstructionDisposition::Architectural(outcome) => outcome,
            InstructionDisposition::Memory(_) => {
                return Err(HarnessError::TimedMemoryRequired { instruction });
            }
        };

        match outcome {
            InstructionOutcome::Commit(commit) => {
                self.cpu.apply_commit(commit);
                Ok(HarnessOutcome::Retired)
            }
            InstructionOutcome::Exception(request) => {
                self.cpu.apply_exception(request);
                Ok(HarnessOutcome::ExceptionTaken(request))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{HarnessError, HarnessOutcome, SemanticHarness};
    use crate::decode::{DecodeGap, Instruction};
    use crate::exception::{ExceptionCode, ExceptionRequest};
    use crate::execute::ExecuteError;
    use crate::gpr::Reg;

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    fn encode_r(rs: u8, rt: u8, rd: u8, shift: u8, function: u8) -> u32 {
        (u32::from(rs) << 21)
            | (u32::from(rt) << 16)
            | (u32::from(rd) << 11)
            | (u32::from(shift) << 6)
            | u32::from(function)
    }

    fn encode_i(opcode: u8, rs: u8, rt: u8, immediate: u16) -> u32 {
        (u32::from(opcode) << 26)
            | (u32::from(rs) << 21)
            | (u32::from(rt) << 16)
            | u32::from(immediate)
    }

    fn encode_special_code(code: u32, function: u8) -> u32 {
        ((code & 0x000f_ffff) << 6) | u32::from(function)
    }

    fn assert_exception_state(
        harness: &SemanticHarness,
        code: ExceptionCode,
        epc: u64,
        branch_delay: bool,
        vector: u64,
    ) {
        assert!(harness.exl());
        assert_eq!(harness.exception_code(), code);
        assert_eq!(harness.epc(), epc);
        assert_eq!(harness.branch_delay(), branch_delay);
        assert_eq!(harness.current_pc(), vector);
        assert_eq!(harness.next_pc(), vector.wrapping_add(4));
        assert_eq!(harness.delay_slot_of(), None);
    }

    #[test]
    fn ordinary_instruction_retires_through_commit() {
        let source = reg(1);
        let destination = reg(2);
        let mut harness = SemanticHarness::with_gprs(0x1000, &[(source, 0x1200)]);

        let outcome = harness.step(encode_i(0x0d, 1, 2, 0x00f0));

        assert_eq!(outcome, Ok(HarnessOutcome::Retired));
        assert_eq!(harness.read_gpr(destination), 0x12f0);
        assert_eq!(harness.current_pc(), 0x1004);
        assert_eq!(harness.next_pc(), 0x1008);
    }

    #[test]
    fn zero_destination_remains_zero_after_retirement() {
        let source = reg(1);
        let mut harness = SemanticHarness::with_gprs(0x1000, &[(source, u64::MAX)]);

        assert_eq!(
            harness.step(encode_i(0x0d, 1, 0, 0xffff)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(harness.read_gpr(Reg::ZERO), 0);
    }

    #[test]
    fn taken_branch_executes_the_delay_slot_before_the_target() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut harness = SemanticHarness::with_gprs(0x1000, &[(left, 7), (right, 7)]);

        assert_eq!(
            harness.step(encode_i(0x04, 1, 2, 3)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(harness.current_pc(), 0x1004);
        assert_eq!(harness.next_pc(), 0x1010);
        assert_eq!(harness.delay_slot_of(), Some(0x1000));

        assert_eq!(
            harness.step(encode_i(0x0d, 0, 3, 1)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(harness.read_gpr(destination), 1);
        assert_eq!(harness.current_pc(), 0x1010);
        assert_eq!(harness.next_pc(), 0x1014);
        assert_eq!(harness.delay_slot_of(), None);
    }

    #[test]
    fn jump_executes_the_delay_slot_before_the_region_target() {
        let mut harness = SemanticHarness::new(0x0fff_fffc);
        let raw_jump = (0x02_u32 << 26) | 1;

        assert_eq!(harness.step(raw_jump), Ok(HarnessOutcome::Retired));
        assert_eq!(harness.current_pc(), 0x1000_0000);
        assert_eq!(harness.next_pc(), 0x1000_0004);
        assert_eq!(harness.delay_slot_of(), Some(0x0fff_fffc));

        assert_eq!(harness.step(0), Ok(HarnessOutcome::Retired));
        assert_eq!(harness.current_pc(), 0x1000_0004);
        assert_eq!(harness.next_pc(), 0x1000_0008);
        assert_eq!(harness.delay_slot_of(), None);
    }

    #[test]
    fn reserved_encoding_takes_guest_exception_without_retirement() {
        let mut harness = SemanticHarness::new(0x1000);
        let raw = 0x1c_u32 << 26;

        assert_eq!(
            harness.step(raw),
            Ok(HarnessOutcome::ExceptionTaken(
                ExceptionRequest::ReservedInstruction
            ))
        );
        assert_exception_state(
            &harness,
            ExceptionCode::ReservedInstruction,
            0x1000,
            false,
            0xffff_ffff_8000_0180,
        );
    }

    #[test]
    fn valid_unimplemented_instruction_is_not_a_guest_exception() {
        let mut harness = SemanticHarness::new(0x1000);
        let raw = encode_r(1, 2, 3, 0, 0x21);
        let before = harness.cpu.clone();

        assert_eq!(
            harness.step(raw),
            Err(HarnessError::Decode(DecodeGap::ValidButUnimplemented {
                raw
            }))
        );
        assert_eq!(harness.cpu, before);
    }

    #[test]
    fn unclassified_encoding_is_not_a_guest_exception() {
        let mut harness = SemanticHarness::new(0x1000);
        let raw = 0x10_u32 << 26;
        let before = harness.cpu.clone();

        assert_eq!(
            harness.step(raw),
            Err(HarnessError::Decode(DecodeGap::UnclassifiedEncoding {
                raw
            }))
        );
        assert_eq!(harness.cpu, before);
    }

    #[test]
    fn add_retires_positive_and_negative_results() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut positive = SemanticHarness::with_gprs(0x1000, &[(left, 2), (right, 3)]);
        let mut negative = SemanticHarness::with_gprs(0x1000, &[(left, u64::MAX - 2), (right, 1)]);

        assert_eq!(
            positive.step(encode_r(1, 2, 3, 0, 0x20)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(positive.read_gpr(destination), 5);
        assert_eq!(positive.current_pc(), 0x1004);

        assert_eq!(
            negative.step(encode_r(1, 2, 3, 0, 0x20)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(negative.read_gpr(destination), u64::MAX - 1);
        assert_eq!(negative.current_pc(), 0x1004);
    }

    #[test]
    fn add_write_to_zero_retires_without_changing_zero() {
        let left = reg(1);
        let right = reg(2);
        let mut harness = SemanticHarness::with_gprs(0x1000, &[(left, 2), (right, 3)]);

        assert_eq!(
            harness.step(encode_r(1, 2, 0, 0, 0x20)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(harness.read_gpr(Reg::ZERO), 0);
        assert_eq!(harness.current_pc(), 0x1004);
    }

    #[test]
    fn add_positive_overflow_takes_a_precise_exception() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut harness = SemanticHarness::with_gprs(
            0x1000,
            &[(left, 0x7fff_ffff), (right, 1), (destination, 42)],
        );

        assert_eq!(
            harness.step(encode_r(1, 2, 3, 0, 0x20)),
            Ok(HarnessOutcome::ExceptionTaken(
                ExceptionRequest::IntegerOverflow
            ))
        );
        assert_eq!(harness.read_gpr(destination), 42);
        assert_exception_state(
            &harness,
            ExceptionCode::IntegerOverflow,
            0x1000,
            false,
            0xffff_ffff_8000_0180,
        );
    }

    #[test]
    fn add_negative_overflow_takes_a_precise_exception() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut harness = SemanticHarness::with_gprs(
            0x1000,
            &[
                (left, 0xffff_ffff_8000_0000),
                (right, u64::MAX),
                (destination, 42),
            ],
        );

        assert_eq!(
            harness.step(encode_r(1, 2, 3, 0, 0x20)),
            Ok(HarnessOutcome::ExceptionTaken(
                ExceptionRequest::IntegerOverflow
            ))
        );
        assert_eq!(harness.read_gpr(destination), 42);
        assert_eq!(harness.exception_code(), ExceptionCode::IntegerOverflow);
    }

    #[test]
    fn add_noncanonical_operand_leaves_all_state_unchanged() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut harness = SemanticHarness::with_gprs(
            0x1000,
            &[(left, 0x0000_0001_0000_0000), (right, 1), (destination, 42)],
        );
        let raw = encode_r(1, 2, 3, 0, 0x20);
        let before = harness.cpu.clone();

        assert_eq!(
            harness.step(raw),
            Err(HarnessError::Execute(ExecuteError::UndefinedResult {
                instruction: Instruction::Add {
                    rd: destination,
                    rs: left,
                    rt: right,
                },
            }))
        );
        assert_eq!(harness.cpu, before);
    }

    #[test]
    fn syscall_and_break_take_distinct_guest_exceptions() {
        let cases = [
            (
                encode_special_code(0xabcde, 0x0c),
                ExceptionRequest::Syscall,
                ExceptionCode::Syscall,
            ),
            (
                encode_special_code(0x54321, 0x0d),
                ExceptionRequest::Breakpoint,
                ExceptionCode::Breakpoint,
            ),
        ];

        for (raw, request, code) in cases {
            let mut harness = SemanticHarness::new(0x1000);

            assert_eq!(
                harness.step(raw),
                Ok(HarnessOutcome::ExceptionTaken(request))
            );
            assert_exception_state(&harness, code, 0x1000, false, 0xffff_ffff_8000_0180);
        }
    }

    #[test]
    fn syscall_and_break_are_valid_in_a_branch_delay_slot() {
        let cases = [
            (
                encode_special_code(1, 0x0c),
                ExceptionRequest::Syscall,
                ExceptionCode::Syscall,
            ),
            (
                encode_special_code(2, 0x0d),
                ExceptionRequest::Breakpoint,
                ExceptionCode::Breakpoint,
            ),
        ];

        for (raw, request, code) in cases {
            let mut harness = SemanticHarness::new(0x1000);
            assert_eq!(
                harness.step(encode_i(0x04, 0, 0, 3)),
                Ok(HarnessOutcome::Retired)
            );

            assert_eq!(
                harness.step(raw),
                Ok(HarnessOutcome::ExceptionTaken(request))
            );
            assert_exception_state(&harness, code, 0x1000, true, 0xffff_ffff_8000_0180);
        }
    }

    #[test]
    fn bev_and_bad_vaddr_are_available_as_read_only_test_state() {
        let mut harness = SemanticHarness::with_gprs_and_bev(0x1000, &[], true);
        harness
            .cpu
            .apply_exception(ExceptionRequest::AddressErrorLoad { bad_vaddr: 0x123 });

        assert!(harness.bev());
        assert_eq!(harness.bad_vaddr(), 0x123);
        assert_exception_state(
            &harness,
            ExceptionCode::AddressErrorLoad,
            0x1000,
            false,
            0xffff_ffff_bfc0_0380,
        );
    }

    #[test]
    fn reserved_instruction_in_delay_slot_records_branch_origin() {
        let mut harness = SemanticHarness::new(0x1000);

        assert_eq!(
            harness.step(encode_i(0x04, 0, 0, 3)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(harness.delay_slot_of(), Some(0x1000));
        assert_eq!(
            harness.step(0x1c_u32 << 26),
            Ok(HarnessOutcome::ExceptionTaken(
                ExceptionRequest::ReservedInstruction
            ))
        );
        assert_exception_state(
            &harness,
            ExceptionCode::ReservedInstruction,
            0x1000,
            true,
            0xffff_ffff_8000_0180,
        );
    }

    #[test]
    fn add_overflow_in_delay_slot_is_the_m1_graduation_path() {
        let left = reg(1);
        let right = reg(2);
        let destination = reg(3);
        let mut harness = SemanticHarness::with_gprs(
            0x1000,
            &[(left, 0x7fff_ffff), (right, 1), (destination, 42)],
        );

        assert_eq!(
            harness.step(encode_i(0x04, 0, 0, 3)),
            Ok(HarnessOutcome::Retired)
        );
        assert_eq!(harness.current_pc(), 0x1004);
        assert_eq!(harness.delay_slot_of(), Some(0x1000));

        assert_eq!(
            harness.step(encode_r(1, 2, 3, 0, 0x20)),
            Ok(HarnessOutcome::ExceptionTaken(
                ExceptionRequest::IntegerOverflow
            ))
        );
        assert_eq!(harness.read_gpr(destination), 42);
        assert_exception_state(
            &harness,
            ExceptionCode::IntegerOverflow,
            0x1000,
            true,
            0xffff_ffff_8000_0180,
        );
    }

    #[test]
    fn delayed_control_flow_error_leaves_all_state_unchanged() {
        let mut harness = SemanticHarness::new(0x1000);
        assert_eq!(
            harness.step(encode_i(0x04, 0, 0, 3)),
            Ok(HarnessOutcome::Retired)
        );
        let before = harness.cpu.clone();
        let raw_jump = 0x02_u32 << 26;

        assert_eq!(
            harness.step(raw_jump),
            Err(HarnessError::Execute(
                ExecuteError::UnpredictableControlFlow {
                    instruction_pc: 0x1004,
                    branch_pc: 0x1000,
                }
            ))
        );
        assert_eq!(harness.cpu, before);
    }
}
