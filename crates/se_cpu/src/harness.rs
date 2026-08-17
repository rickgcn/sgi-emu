//! Drives raw words through decode, execute, and normal retirement in tests.
//!
//! The harness constructs synthetic GPR and PC pre-state. Reserved encodings
//! produce guest exception requests, while decoder gaps and execution stops remain
//! distinct errors.

use crate::cpu::Cpu;
use crate::decode::{DecodeGap, DecodeOutcome, decode};
use crate::exception::ExceptionRequest;
use crate::execute::{ExecuteError, execute};
use crate::gpr::{GprFile, Reg};
use crate::pc::PcState;

pub(crate) struct SemanticHarness {
    cpu: Cpu,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HarnessOutcome {
    Retired,
    ExceptionRequested(ExceptionRequest),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum HarnessError {
    Decode(DecodeGap),
    Execute(ExecuteError),
}

impl SemanticHarness {
    pub(crate) fn new(entry_pc: u64) -> Self {
        Self::with_gprs(entry_pc, &[])
    }

    pub(crate) fn with_gprs(entry_pc: u64, initial: &[(Reg, u64)]) -> Self {
        let mut gpr = GprFile::new();
        for &(register, value) in initial {
            gpr.write(register, value);
        }
        Self {
            cpu: Cpu::from_parts(gpr, PcState::new(entry_pc)),
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

    pub(crate) fn step(&mut self, raw: u32) -> Result<HarnessOutcome, HarnessError> {
        let instruction = match decode(raw) {
            DecodeOutcome::Instruction(instruction) => instruction,
            DecodeOutcome::ReservedEncoding { .. } => {
                return Ok(HarnessOutcome::ExceptionRequested(
                    ExceptionRequest::ReservedInstruction,
                ));
            }
            DecodeOutcome::ImplementationGap(gap) => return Err(HarnessError::Decode(gap)),
        };

        let commit = execute(&self.cpu, instruction).map_err(HarnessError::Execute)?;
        self.cpu.apply_commit(commit);
        Ok(HarnessOutcome::Retired)
    }
}

#[cfg(test)]
mod tests {
    use super::{HarnessError, HarnessOutcome, SemanticHarness};
    use crate::decode::DecodeGap;
    use crate::exception::ExceptionRequest;
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
    fn reserved_encoding_requests_guest_exception_without_retirement() {
        let mut harness = SemanticHarness::new(0x1000);
        let raw = 0x1c_u32 << 26;

        assert_eq!(
            harness.step(raw),
            Ok(HarnessOutcome::ExceptionRequested(
                ExceptionRequest::ReservedInstruction
            ))
        );
        assert_eq!(harness.current_pc(), 0x1000);
        assert_eq!(harness.next_pc(), 0x1004);
    }

    #[test]
    fn valid_unimplemented_instruction_is_not_a_guest_exception() {
        let mut harness = SemanticHarness::new(0x1000);
        let raw = encode_r(1, 2, 3, 0, 0x21);

        assert_eq!(
            harness.step(raw),
            Err(HarnessError::Decode(DecodeGap::ValidButUnimplemented {
                raw
            }))
        );
        assert_eq!(harness.current_pc(), 0x1000);
    }

    #[test]
    fn unclassified_encoding_is_not_a_guest_exception() {
        let mut harness = SemanticHarness::new(0x1000);
        let raw = 0x10_u32 << 26;

        assert_eq!(
            harness.step(raw),
            Err(HarnessError::Decode(DecodeGap::UnclassifiedEncoding {
                raw
            }))
        );
        assert_eq!(harness.current_pc(), 0x1000);
    }
}
