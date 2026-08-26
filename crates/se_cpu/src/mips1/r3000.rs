//! MIPS R3000 processor model.

mod alu;
mod control;
mod cp0;
mod decode;
mod mmu;
mod state;

use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

use self::{
    cp0::Exception,
    decode::{DecodeResult, Instruction, decode},
    state::State,
};

/// An error encountered while executing one processor step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepError {
    /// The instruction address cannot be translated by the supported mapping.
    UnsupportedInstructionAddress {
        /// The virtual address used for the instruction fetch.
        address: u32,
    },

    /// The physical bus rejected the instruction fetch.
    BusFault {
        /// The translated physical address used for the instruction fetch.
        address: PhysAddr,

        /// The fault reported by the physical bus.
        fault: BusFault,
    },

    /// The instruction is valid for the R3000 but is not implemented by this
    /// processor model.
    UnsupportedInstruction {
        /// The virtual address of the instruction.
        pc: u32,

        /// The raw instruction word.
        instruction: u32,
    },
}

/// An architectural R3000 processor.
pub struct R3000 {
    state: State,
}

#[expect(
    clippy::new_without_default,
    reason = "Processor construction has explicit reset semantics"
)]
impl R3000 {
    /// Creates a processor at the reset vector.
    ///
    /// General-purpose registers and the HI/LO registers without
    /// architecturally defined reset values are initialized to zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::new(),
        }
    }

    /// Restores the architecturally defined core reset state.
    ///
    /// General-purpose registers other than register zero and the HI/LO
    /// registers are preserved because their reset values are architecturally
    /// unspecified.
    pub fn reset(&mut self) {
        self.state.reset();
    }

    /// Executes one architectural processor step.
    ///
    /// A successful step either completes one instruction or enters a guest
    /// exception. Guest exceptions are architectural state transitions and do
    /// not produce [`StepError`]. If an error occurs, the architectural
    /// processor state remains unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`StepError`] when the instruction address is unsupported, the
    /// physical bus rejects the fetch, or a valid R3000 instruction is not
    /// implemented by this processor model.
    pub fn step(&mut self, bus: &mut dyn PhysicalBus) -> Result<(), StepError> {
        let pc = self.state.pc();
        if pc & 3 != 0 {
            self.state
                .take_exception(Exception::InstructionAddressError { address: pc });
            return Ok(());
        }

        let word = fetch_instruction(pc, bus)?;
        let instruction = match decode(word) {
            DecodeResult::Implemented(instruction) => instruction,
            DecodeResult::Unimplemented => {
                return Err(StepError::UnsupportedInstruction {
                    pc,
                    instruction: word,
                });
            }
            DecodeResult::Reserved => {
                self.state.take_exception(Exception::ReservedInstruction);
                return Ok(());
            }
        };

        let outcome: Result<Option<u32>, Exception> = match instruction {
            Instruction::Alu(instruction) => {
                alu::execute(&mut self.state, instruction).map(|()| None)
            }
            Instruction::Control(instruction) => {
                Ok(Some(control::execute(&mut self.state, instruction)))
            }
            Instruction::Syscall => Err(Exception::Syscall),
            Instruction::Breakpoint => Err(Exception::Breakpoint),
        };

        match outcome {
            Ok(delayed_resume_pc) => self.state.complete_instruction(delayed_resume_pc),
            Err(exception) => self.state.take_exception(exception),
        }

        Ok(())
    }
}

fn fetch_instruction(virtual_address: u32, bus: &mut dyn PhysicalBus) -> Result<u32, StepError> {
    let address = mmu::translate_instruction_address(virtual_address).ok_or(
        StepError::UnsupportedInstructionAddress {
            address: virtual_address,
        },
    )?;
    let mut bytes = [0; 4];
    bus.read(address, &mut bytes)
        .map_err(|fault| StepError::BusFault { address, fault })?;

    Ok(u32::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

    use super::{R3000, StepError, fetch_instruction};

    const BOOT_GENERAL_EXCEPTION_VECTOR: u32 = 0xbfc0_0180;

    struct TestBus {
        bytes: [u8; 4],
        fault: Option<BusFault>,
        read_address: Option<PhysAddr>,
        read_length: Option<usize>,
    }

    impl TestBus {
        fn new(bytes: [u8; 4]) -> Self {
            Self {
                bytes,
                fault: None,
                read_address: None,
                read_length: None,
            }
        }
    }

    impl PhysicalBus for TestBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            self.read_address = Some(address);
            self.read_length = Some(data.len());

            if let Some(fault) = self.fault {
                return Err(fault);
            }
            if data.len() != self.bytes.len() {
                return Err(BusFault::UnsupportedAccess);
            }

            data.copy_from_slice(&self.bytes);
            Ok(())
        }

        fn write(&mut self, _address: PhysAddr, _data: &[u8]) -> Result<(), BusFault> {
            Err(BusFault::UnsupportedAccess)
        }
    }

    fn snapshot(processor: &R3000) -> ([u32; 32], u32, u32, u32) {
        (
            std::array::from_fn(|index| processor.state.read_gpr(index)),
            processor.state.read_hi(),
            processor.state.read_lo(),
            processor.state.pc(),
        )
    }

    fn step_with_word(processor: &mut R3000, word: u32) -> Result<(), StepError> {
        let mut bus = TestBus::new(word.to_be_bytes());
        processor.step(&mut bus)
    }

    fn encode_register(rs: u32, rt: u32, rd: u32, function: u32) -> u32 {
        (rs << 21) | (rt << 16) | (rd << 11) | function
    }

    #[test]
    fn processor_can_be_constructed_and_reset() {
        let mut processor = R3000::new();

        processor.reset();
    }

    #[test]
    fn fetch_translates_reset_vector_and_reads_big_endian_word() {
        let mut bus = TestBus::new([0x24, 0x01, 0xff, 0xff]);

        let word = fetch_instruction(0xbfc0_0000, &mut bus).expect("fetch should succeed");

        assert_eq!(word, 0x2401_ffff);
        assert_eq!(bus.read_address, Some(PhysAddr::new(0x1fc0_0000)));
        assert_eq!(bus.read_length, Some(4));
    }

    #[test]
    fn fetch_rejects_unsupported_address_without_reading_bus() {
        let mut bus = TestBus::new([0; 4]);

        let error =
            fetch_instruction(0x8000_0000, &mut bus).expect_err("address should be rejected");

        assert_eq!(
            error,
            StepError::UnsupportedInstructionAddress {
                address: 0x8000_0000
            }
        );
        assert_eq!(bus.read_address, None);
        assert_eq!(bus.read_length, None);
    }

    #[test]
    fn step_preserves_processor_state_when_bus_faults() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1234_5678);
        processor.state.write_gpr(31, 0x89ab_cdef);
        processor.state.write_hi(0x1357_9bdf);
        processor.state.write_lo(0x2468_ace0);
        let before = snapshot(&processor);
        let mut bus = TestBus::new([0; 4]);
        bus.fault = Some(BusFault::Unmapped);

        let error = processor.step(&mut bus).expect_err("fetch should fault");

        assert_eq!(
            error,
            StepError::BusFault {
                address: PhysAddr::new(0x1fc0_0000),
                fault: BusFault::Unmapped,
            }
        );
        assert_eq!(snapshot(&processor), before);
    }

    #[test]
    fn step_executes_addiu_and_advances_program_counter() {
        let mut processor = R3000::new();
        let mut bus = TestBus::new([0x24, 0x01, 0xff, 0xff]);

        processor.step(&mut bus).expect("step should succeed");

        assert_eq!(processor.state.read_gpr(1), u32::MAX);
        assert_eq!(processor.state.pc(), 0xbfc0_0004);
    }

    #[test]
    fn step_executes_multiply_and_reads_both_results() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, (-2_i32) as u32);
        processor.state.write_gpr(2, 3);

        step_with_word(&mut processor, encode_register(1, 2, 0, 0x18))
            .expect("MULT should succeed");

        assert_eq!(processor.state.read_hi(), u32::MAX);
        assert_eq!(processor.state.read_lo(), 0xffff_fffa);
        assert_eq!(processor.state.pc(), 0xbfc0_0004);

        step_with_word(&mut processor, encode_register(0, 0, 3, 0x12))
            .expect("MFLO should succeed");

        assert_eq!(processor.state.read_gpr(3), 0xffff_fffa);
        assert_eq!(processor.state.pc(), 0xbfc0_0008);

        step_with_word(&mut processor, encode_register(0, 0, 4, 0x10))
            .expect("MFHI should succeed");

        assert_eq!(processor.state.read_gpr(4), u32::MAX);
        assert_eq!(processor.state.read_hi(), u32::MAX);
        assert_eq!(processor.state.read_lo(), 0xffff_fffa);
        assert_eq!(processor.state.pc(), 0xbfc0_000c);
    }

    #[test]
    fn step_preserves_processor_state_for_unsupported_instruction() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1234_5678);
        processor.state.write_gpr(31, 0x89ab_cdef);
        processor.state.write_hi(0x1357_9bdf);
        processor.state.write_lo(0x2468_ace0);
        let before = snapshot(&processor);
        let mut bus = TestBus::new([0x8c, 0x01, 0x00, 0x00]);

        let error = processor
            .step(&mut bus)
            .expect_err("LW should not be supported");

        assert_eq!(
            error,
            StepError::UnsupportedInstruction {
                pc: 0xbfc0_0000,
                instruction: 0x8c01_0000,
            }
        );
        assert_eq!(snapshot(&processor), before);
    }

    #[test]
    fn step_takes_explicit_instruction_exceptions() {
        for word in [0x0000_000c, 0x0000_000d] {
            let mut processor = R3000::new();

            step_with_word(&mut processor, word).expect("guest exception should succeed");

            assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);
        }
    }

    #[test]
    fn step_takes_reserved_instruction_exception() {
        let mut processor = R3000::new();

        step_with_word(&mut processor, 0x0000_0001)
            .expect("reserved instruction exception should succeed");

        assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);
    }

    #[test]
    fn step_takes_overflow_without_writing_destination() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, i32::MAX as u32);
        processor.state.write_gpr(2, 1);
        processor.state.write_gpr(3, 0xdead_beef);
        let add = encode_register(1, 2, 3, 0x20);

        step_with_word(&mut processor, add).expect("overflow exception should succeed");

        assert_eq!(processor.state.read_gpr(3), 0xdead_beef);
        assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);
    }

    #[test]
    fn taken_branch_executes_delay_slot_before_resuming_at_target() {
        let mut processor = R3000::new();

        step_with_word(&mut processor, 0x1000_0002).expect("BEQ should succeed");

        assert_eq!(processor.state.pc(), 0xbfc0_0004);
        assert_eq!(processor.state.read_gpr(1), 0);

        step_with_word(&mut processor, 0x2401_0001).expect("delay slot should succeed");

        assert_eq!(processor.state.read_gpr(1), 1);
        assert_eq!(processor.state.pc(), 0xbfc0_000c);
    }

    #[test]
    fn not_taken_branch_executes_delay_slot_before_falling_through() {
        let mut processor = R3000::new();

        step_with_word(&mut processor, 0x1400_0002).expect("BNE should succeed");

        assert_eq!(processor.state.pc(), 0xbfc0_0004);

        step_with_word(&mut processor, 0x2401_0001).expect("delay slot should succeed");

        assert_eq!(processor.state.read_gpr(1), 1);
        assert_eq!(processor.state.pc(), 0xbfc0_0008);
    }

    #[test]
    fn jump_and_link_writes_link_before_executing_delay_slot() {
        let mut processor = R3000::new();

        step_with_word(&mut processor, 0x0ff0_0010).expect("JAL should succeed");

        assert_eq!(processor.state.read_gpr(31), 0xbfc0_0008);
        assert_eq!(processor.state.pc(), 0xbfc0_0004);

        step_with_word(&mut processor, 0).expect("NOP delay slot should succeed");

        assert_eq!(processor.state.read_gpr(31), 0xbfc0_0008);
        assert_eq!(processor.state.pc(), 0xbfc0_0040);
    }

    #[test]
    fn bus_fault_in_delay_slot_preserves_pending_jump_and_link() {
        let mut processor = R3000::new();
        step_with_word(&mut processor, 0x0ff0_0010).expect("JAL should succeed");
        let before = snapshot(&processor);
        let mut bus = TestBus::new([0; 4]);
        bus.fault = Some(BusFault::Unmapped);

        let error = processor
            .step(&mut bus)
            .expect_err("delay-slot fetch should fault");

        assert_eq!(
            error,
            StepError::BusFault {
                address: PhysAddr::new(0x1fc0_0004),
                fault: BusFault::Unmapped,
            }
        );
        assert_eq!(snapshot(&processor), before);
        assert_eq!(processor.state.read_gpr(31), 0xbfc0_0008);

        step_with_word(&mut processor, 0).expect("retry should execute delay slot");

        assert_eq!(processor.state.pc(), 0xbfc0_0040);
        assert_eq!(processor.state.read_gpr(31), 0xbfc0_0008);
    }

    #[test]
    fn exception_in_delay_slot_preserves_link_and_cancels_resume() {
        let mut processor = R3000::new();
        step_with_word(&mut processor, 0x0ff0_0010).expect("JAL should succeed");

        step_with_word(&mut processor, 0x0000_000c).expect("delay-slot exception should succeed");

        assert_eq!(processor.state.read_gpr(31), 0xbfc0_0008);
        assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);

        step_with_word(&mut processor, 0).expect("exception vector instruction should succeed");

        assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR + 4);
    }

    #[test]
    fn misaligned_register_jump_target_faults_after_delay_slot() {
        let mut processor = R3000::new();
        let target = 0xbfc0_0041;
        processor.state.write_gpr(1, target);
        let jr = encode_register(1, 0, 0, 0x08);

        step_with_word(&mut processor, jr).expect("JR should succeed");
        assert_eq!(processor.state.pc(), 0xbfc0_0004);

        step_with_word(&mut processor, 0).expect("delay slot should succeed");
        assert_eq!(processor.state.pc(), target);

        let mut bus = TestBus::new([0; 4]);
        processor
            .step(&mut bus)
            .expect("address error exception should succeed");

        assert_eq!(bus.read_address, None);
        assert_eq!(bus.read_length, None);
        assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);
    }

    #[test]
    fn unsupported_instruction_in_delay_slot_preserves_pending_branch() {
        let mut processor = R3000::new();
        step_with_word(&mut processor, 0x1000_0002).expect("BEQ should succeed");
        let before = snapshot(&processor);

        let error =
            step_with_word(&mut processor, 0x8c01_0000).expect_err("LW should remain unsupported");

        assert_eq!(
            error,
            StepError::UnsupportedInstruction {
                pc: 0xbfc0_0004,
                instruction: 0x8c01_0000,
            }
        );
        assert_eq!(snapshot(&processor), before);

        step_with_word(&mut processor, 0).expect("retry should execute delay slot");

        assert_eq!(processor.state.pc(), 0xbfc0_000c);
    }
}
