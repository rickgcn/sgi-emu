use se_core::bus::PhysicalBus;

use super::{
    ExecutionOutcome, InstructionResult,
    control::branch_resume_pc,
    cp0::Exception,
    decode::Cp1Instruction,
    load_store,
    state::{InstructionEffect, State},
};

const IMPLEMENTATION_REVISION: u32 = 0x0000_0300;
const CONTROL_STATUS_WRITABLE_MASK: u32 = 0x0083_ffff;
const CONTROL_STATUS_UNIMPLEMENTED: u32 = 1 << 17;
const CONTROL_STATUS_CAUSE_MASK: u32 = 0x0001_f000;
const CONTROL_STATUS_ENABLE_MASK: u32 = 0x0000_0f80;
const CONTROL_STATUS_CONDITION: u32 = 1 << 23;

#[derive(Debug, Eq, PartialEq)]
pub(super) struct Cp1 {
    registers: [u32; 32],
    exception_instruction: u32,
    control_status: u32,
}

impl Cp1 {
    pub(super) const fn new() -> Self {
        Self {
            registers: [0; 32],
            exception_instruction: 0,
            control_status: 0,
        }
    }

    pub(super) fn read_general_register(&self, index: usize) -> u32 {
        self.registers[index]
    }

    pub(super) fn write_general_register(&mut self, index: usize, value: u32) {
        self.registers[index] = value;
    }

    pub(super) fn read_control_register(&self, index: usize) -> u32 {
        match index {
            0 => IMPLEMENTATION_REVISION,
            30 => self.exception_instruction,
            31 => self.control_status,
            _ => 0,
        }
    }

    pub(super) fn write_control_register(&mut self, index: usize, value: u32) {
        match index {
            30 => self.exception_instruction = value,
            31 => self.write_control_status(value),
            _ => {}
        }
    }

    pub(super) fn condition(&self) -> bool {
        self.control_status & CONTROL_STATUS_CONDITION != 0
    }

    pub(super) fn interrupt_asserted(&self) -> bool {
        let enabled_causes = (self.control_status & CONTROL_STATUS_CAUSE_MASK) >> 5
            & self.control_status
            & CONTROL_STATUS_ENABLE_MASK;

        self.control_status & CONTROL_STATUS_UNIMPLEMENTED != 0 || enabled_causes != 0
    }

    fn write_control_status(&mut self, value: u32) {
        self.control_status = value & CONTROL_STATUS_WRITABLE_MASK;
    }
}

pub(super) fn execute(
    state: &mut State,
    instruction: Cp1Instruction,
    bus: &mut dyn PhysicalBus,
) -> InstructionResult {
    if !state.coprocessor_usable(1) {
        return Ok(ExecutionOutcome::Exception(
            Exception::CoprocessorUnusable { unit: 1 },
        ));
    }

    let completion = match instruction {
        Cp1Instruction::Mfc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: rt,
                value: state.read_cp1_general(rd),
                load_merge_bypass: false,
            }),
        ),
        Cp1Instruction::Cfc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedGprWrite {
                index: rt,
                value: state.read_cp1_control(rd),
                load_merge_bypass: false,
            }),
        ),
        Cp1Instruction::Mtc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedCp1GeneralWrite {
                index: rd,
                value: state.read_gpr(rt),
            }),
        ),
        Cp1Instruction::Ctc1 { rt, rd } => (
            None,
            Some(InstructionEffect::DelayedCp1ControlWrite {
                index: rd,
                value: state.read_gpr(rt),
            }),
        ),
        Cp1Instruction::Bc1f { offset } => (
            Some(branch_resume_pc(state.pc(), offset, !state.cp1_condition())),
            None,
        ),
        Cp1Instruction::Bc1t { offset } => (
            Some(branch_resume_pc(state.pc(), offset, state.cp1_condition())),
            None,
        ),
        Cp1Instruction::Lwc1 { base, ft, offset } => {
            let mut bytes = [0; 4];
            match load_store::load(state, base, offset, &mut bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }

            (
                None,
                Some(InstructionEffect::DelayedCp1GeneralWrite {
                    index: ft,
                    value: u32::from_be_bytes(bytes),
                }),
            )
        }
        Cp1Instruction::Swc1 { base, ft, offset } => {
            let bytes = state.read_cp1_general(ft).to_be_bytes();
            match load_store::store(state, base, offset, &bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }

            (None, None)
        }
    };

    Ok(ExecutionOutcome::Completed(completion))
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

    use super::{
        CONTROL_STATUS_CAUSE_MASK, CONTROL_STATUS_CONDITION, CONTROL_STATUS_ENABLE_MASK,
        CONTROL_STATUS_UNIMPLEMENTED, CONTROL_STATUS_WRITABLE_MASK, Cp1, Cp1Instruction, Exception,
        ExecutionOutcome, IMPLEMENTATION_REVISION, InstructionEffect, State, execute,
    };
    use crate::mips1::r3000::TEST_CONFIG;

    const STATUS_CU1: u32 = 1 << 29;

    struct TestBus {
        read_data: [u8; 4],
        reads: Vec<(PhysAddr, usize)>,
        writes: Vec<(PhysAddr, Vec<u8>)>,
    }

    impl TestBus {
        fn new(read_data: [u8; 4]) -> Self {
            Self {
                read_data,
                reads: Vec::new(),
                writes: Vec::new(),
            }
        }
    }

    impl PhysicalBus for TestBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            self.reads.push((address, data.len()));
            data.copy_from_slice(&self.read_data[..data.len()]);
            Ok(())
        }

        fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
            self.writes.push((address, data.to_vec()));
            Ok(())
        }
    }

    fn enabled_state() -> State {
        let mut state = State::new(TEST_CONFIG);
        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write {
                index: 12,
                value: state.read_cp0(12) | STATUS_CU1,
            }),
        );
        state.complete_instruction(None, None);
        state.complete_instruction(None, None);
        state
    }

    #[test]
    fn new_initializes_deterministic_state() {
        let cp1 = Cp1::new();

        for index in 0..32 {
            assert_eq!(cp1.read_general_register(index), 0);
        }
        assert_eq!(cp1.read_control_register(0), IMPLEMENTATION_REVISION);
        assert_eq!(cp1.read_control_register(30), 0);
        assert_eq!(cp1.read_control_register(31), 0);
        assert!(!cp1.condition());
        assert!(!cp1.interrupt_asserted());
    }

    #[test]
    fn general_registers_include_zero_and_thirty_one() {
        let mut cp1 = Cp1::new();

        cp1.write_general_register(0, 0x1234_5678);
        cp1.write_general_register(31, 0x89ab_cdef);

        assert_eq!(cp1.read_general_register(0), 0x1234_5678);
        assert_eq!(cp1.read_general_register(31), 0x89ab_cdef);
    }

    #[test]
    fn control_registers_apply_their_access_rules() {
        let mut cp1 = Cp1::new();

        cp1.write_control_register(0, u32::MAX);
        cp1.write_control_register(1, u32::MAX);
        cp1.write_control_register(29, u32::MAX);
        cp1.write_control_register(30, 0x1234_5678);
        cp1.write_control_register(31, u32::MAX);

        assert_eq!(cp1.read_control_register(0), IMPLEMENTATION_REVISION);
        for index in 1..30 {
            assert_eq!(cp1.read_control_register(index), 0);
        }
        assert_eq!(cp1.read_control_register(30), 0x1234_5678);
        assert_eq!(cp1.read_control_register(31), CONTROL_STATUS_WRITABLE_MASK);
    }

    #[test]
    fn condition_and_interrupt_are_derived_from_control_status() {
        let mut cp1 = Cp1::new();

        cp1.write_control_register(31, CONTROL_STATUS_CONDITION);
        assert!(cp1.condition());
        assert!(!cp1.interrupt_asserted());

        cp1.write_control_register(31, CONTROL_STATUS_UNIMPLEMENTED);
        assert!(!cp1.condition());
        assert!(cp1.interrupt_asserted());

        for cause in 0..5 {
            let cause_bit = 1 << (12 + cause);
            let enable_bit = 1 << (7 + cause);

            cp1.write_control_register(31, cause_bit);
            assert!(!cp1.interrupt_asserted());

            cp1.write_control_register(31, enable_bit);
            assert!(!cp1.interrupt_asserted());

            cp1.write_control_register(31, cause_bit | enable_bit);
            assert!(cp1.interrupt_asserted());
        }

        cp1.write_control_register(
            31,
            (CONTROL_STATUS_CAUSE_MASK & !(1 << 12)) | (CONTROL_STATUS_ENABLE_MASK & (1 << 7)),
        );
        assert!(!cp1.interrupt_asserted());
    }

    #[test]
    fn conditional_branches_read_the_committed_condition() {
        let mut state = enabled_state();
        let mut bus = TestBus::new([0; 4]);
        let pc = state.pc();

        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1f { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(12)),
                None
            )))
        );
        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1t { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(8)),
                None
            )))
        );

        state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp1ControlWrite {
                index: 31,
                value: CONTROL_STATUS_CONDITION,
            }),
        );
        state.complete_instruction(None, None);
        let pc = state.pc();

        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1f { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(8)),
                None
            )))
        );
        assert_eq!(
            execute(&mut state, Cp1Instruction::Bc1t { offset: 2 }, &mut bus,),
            Ok(ExecutionOutcome::Completed((
                Some(pc.wrapping_add(12)),
                None
            )))
        );
    }

    #[test]
    fn word_memory_uses_shared_big_endian_access_and_load_delay() {
        let mut state = enabled_state();
        state.write_gpr(1, 0xa000_0100);
        let mut bus = TestBus::new([0x12, 0x34, 0x56, 0x78]);
        let effect = InstructionEffect::DelayedCp1GeneralWrite {
            index: 5,
            value: 0x1234_5678,
        };

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Lwc1 {
                    base: 1,
                    ft: 5,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Completed((None, Some(effect))))
        );
        assert_eq!(bus.reads, [(PhysAddr::new(0x100), 4)]);
        assert_eq!(state.read_cp1_general(5), 0);

        state.complete_instruction(None, Some(effect));
        assert_eq!(state.read_cp1_general(5), 0);
        state.complete_instruction(None, None);
        assert_eq!(state.read_cp1_general(5), 0x1234_5678);

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Swc1 {
                    base: 1,
                    ft: 5,
                    offset: 4,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Completed((None, None)))
        );
        assert_eq!(
            bus.writes,
            [(PhysAddr::new(0x104), vec![0x12, 0x34, 0x56, 0x78])]
        );
    }

    #[test]
    fn word_memory_reuses_word_alignment_exceptions() {
        let mut state = enabled_state();
        state.write_gpr(1, 0xa000_0101);
        let mut bus = TestBus::new([0; 4]);

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Lwc1 {
                    base: 1,
                    ft: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Exception(Exception::LoadAddressError {
                address: 0xa000_0101,
            }))
        );
        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Swc1 {
                    base: 1,
                    ft: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Exception(Exception::StoreAddressError {
                address: 0xa000_0101,
            }))
        );
        assert!(bus.reads.is_empty());
        assert!(bus.writes.is_empty());
    }

    #[test]
    fn usability_gate_precedes_cp1_execution() {
        let mut state = State::new(TEST_CONFIG);
        state.write_gpr(1, 0xa000_0101);
        let mut bus = TestBus::new([0; 4]);

        assert_eq!(
            execute(
                &mut state,
                Cp1Instruction::Lwc1 {
                    base: 1,
                    ft: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            Ok(ExecutionOutcome::Exception(
                Exception::CoprocessorUnusable { unit: 1 }
            ))
        );
        assert!(bus.reads.is_empty());
        assert!(bus.writes.is_empty());
    }
}
