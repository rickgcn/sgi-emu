//! MIPS R3000 processor model.

mod alu;
mod decode;
mod mmu;
mod state;

use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

use self::{decode::decode, state::State};

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

    /// The fetched instruction is not supported by this processor model.
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
    /// General-purpose registers without architecturally defined reset values
    /// are initialized to zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: State::new(),
        }
    }

    /// Restores the architecturally defined core reset state.
    ///
    /// General-purpose registers other than register zero are preserved
    /// because their reset values are architecturally unspecified.
    pub fn reset(&mut self) {
        self.state.reset();
    }

    /// Fetches and executes one instruction.
    ///
    /// A successful step advances the program counter by four bytes. If an
    /// error occurs, the architectural processor state remains unchanged.
    ///
    /// # Errors
    ///
    /// Returns [`StepError`] when the instruction address is unsupported, the
    /// physical bus rejects the fetch, or the fetched instruction is not
    /// supported.
    pub fn step(&mut self, bus: &mut dyn PhysicalBus) -> Result<(), StepError> {
        let pc = self.state.pc();
        let word = fetch_instruction(pc, bus)?;
        let instruction = decode(word).ok_or(StepError::UnsupportedInstruction {
            pc,
            instruction: word,
        })?;

        alu::execute(&mut self.state, instruction);
        self.state.advance_pc();

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

    fn snapshot(processor: &R3000) -> ([u32; 32], u32) {
        (
            std::array::from_fn(|index| processor.state.read_gpr(index)),
            processor.state.pc(),
        )
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
    fn step_preserves_processor_state_for_unsupported_instruction() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1234_5678);
        processor.state.write_gpr(31, 0x89ab_cdef);
        let before = snapshot(&processor);
        let mut bus = TestBus::new([0x20, 0x01, 0x00, 0x01]);

        let error = processor
            .step(&mut bus)
            .expect_err("ADDI should not be supported");

        assert_eq!(
            error,
            StepError::UnsupportedInstruction {
                pc: 0xbfc0_0000,
                instruction: 0x2001_0001,
            }
        );
        assert_eq!(snapshot(&processor), before);
    }
}
