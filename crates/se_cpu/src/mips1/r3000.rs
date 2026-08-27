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
    mmu::AccessType,
    state::{InstructionEffect, State, TranslationError},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ExecutionError {
    Exception(Exception),
    TlbShutdown,
}

/// An error encountered while executing one processor step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StepError {
    /// The translation buffer is in shutdown state after a duplicate tag
    /// match.
    TlbShutdown,

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
    cp0_condition: bool,
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
            cp0_condition: false,
        }
    }

    /// Restores the architecturally defined core reset state.
    ///
    /// General-purpose registers other than register zero and the HI/LO
    /// registers are preserved because their reset values are architecturally
    /// unspecified. The external CP0 condition input is preserved because it
    /// is driven by the containing machine. Main translation-buffer entries
    /// are preserved, while pending instruction-translation visibility is
    /// discarded and synchronized with the main entries.
    pub fn reset(&mut self) {
        self.state.reset();
    }

    /// Sets the external CP0 condition input sampled by CP0 branches.
    pub fn set_cp0_condition(&mut self, condition: bool) {
        self.cp0_condition = condition;
    }

    /// Executes one architectural processor step.
    ///
    /// A successful step either completes one instruction or enters a guest
    /// exception. Guest exceptions are architectural state transitions and do
    /// not produce [`StepError`]. Bus faults and unsupported instructions leave
    /// the architectural processor state unchanged. A newly detected
    /// translation-buffer shutdown changes only the CP0 shutdown state.
    ///
    /// # Errors
    ///
    /// Returns [`StepError`] when the translation buffer is shut down, the
    /// physical bus rejects the fetch, or a valid R3000 instruction is not
    /// implemented by this processor model.
    pub fn step(&mut self, bus: &mut dyn PhysicalBus) -> Result<(), StepError> {
        if self.state.is_tlb_shutdown() {
            return Err(StepError::TlbShutdown);
        }

        let pc = self.state.pc();
        if pc & 3 != 0 {
            self.state
                .take_exception(Exception::InstructionAddressError { address: pc });
            return Ok(());
        }

        let address = match self.state.translate_address(pc, AccessType::Instruction) {
            Ok(address) => address,
            Err(TranslationError::Exception(exception)) => {
                self.state.take_exception(exception);
                return Ok(());
            }
            Err(TranslationError::TlbShutdown) => return Err(StepError::TlbShutdown),
        };
        let word = fetch_instruction(address, bus)?;
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

        let outcome: Result<(Option<u32>, Option<InstructionEffect>), ExecutionError> =
            match instruction {
                Instruction::Alu(instruction) => alu::execute(&mut self.state, instruction)
                    .map(|()| (None, None))
                    .map_err(ExecutionError::Exception),
                Instruction::Control(instruction) => {
                    Ok((Some(control::execute(&mut self.state, instruction)), None))
                }
                Instruction::Cp0(instruction) => {
                    cp0::execute(&mut self.state, instruction, self.cp0_condition)
                }
                Instruction::Syscall => Err(ExecutionError::Exception(Exception::Syscall)),
                Instruction::Breakpoint => Err(ExecutionError::Exception(Exception::Breakpoint)),
            };

        match outcome {
            Ok((delayed_resume_pc, effect)) => {
                self.state.complete_instruction(delayed_resume_pc, effect);
            }
            Err(ExecutionError::Exception(exception)) => self.state.take_exception(exception),
            Err(ExecutionError::TlbShutdown) => return Err(StepError::TlbShutdown),
        }

        Ok(())
    }
}

fn fetch_instruction(address: PhysAddr, bus: &mut dyn PhysicalBus) -> Result<u32, StepError> {
    let mut bytes = [0; 4];
    bus.read(address, &mut bytes)
        .map_err(|fault| StepError::BusFault { address, fault })?;

    Ok(u32::from_be_bytes(bytes))
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

    use super::{R3000, StepError, fetch_instruction};
    use super::{mmu::AccessType, state::InstructionEffect};

    const BOOT_GENERAL_EXCEPTION_VECTOR: u32 = 0xbfc0_0180;
    const BOOT_TLB_REFILL_EXCEPTION_VECTOR: u32 = 0xbfc0_0100;
    const ENTRY_LO_DIRTY: u32 = 1 << 10;
    const ENTRY_LO_VALID: u32 = 1 << 9;
    const STATUS_BEV: u32 = 1 << 22;
    const STATUS_TS: u32 = 1 << 21;
    const STATUS_KUC: u32 = 1 << 1;

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

    struct AddressBus {
        words: Vec<(PhysAddr, u32)>,
        read_addresses: Vec<PhysAddr>,
    }

    impl AddressBus {
        fn new(words: &[(u64, u32)]) -> Self {
            Self {
                words: words
                    .iter()
                    .map(|&(address, word)| (PhysAddr::new(address), word))
                    .collect(),
                read_addresses: Vec::new(),
            }
        }
    }

    impl PhysicalBus for AddressBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            if data.len() != 4 {
                return Err(BusFault::UnsupportedAccess);
            }

            let word = self
                .words
                .iter()
                .find_map(|&(candidate, word)| (candidate == address).then_some(word))
                .ok_or(BusFault::Unmapped)?;
            self.read_addresses.push(address);
            data.copy_from_slice(&word.to_be_bytes());
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

    fn install_and_sync_tlb_entry(
        processor: &mut R3000,
        index: usize,
        virtual_address: u32,
        physical_address: u32,
        flags: u32,
    ) {
        processor.state.complete_instruction(
            None,
            Some(InstructionEffect::TlbWrite {
                index,
                entry_hi: virtual_address & 0xffff_f000,
                entry_lo: (physical_address & 0xffff_f000) | flags,
            }),
        );
        processor.state.complete_instruction(None, None);
        processor.state.complete_instruction(None, None);
    }

    fn set_cp0_register(processor: &mut R3000, index: usize, value: u32) {
        processor.state.complete_instruction(
            None,
            Some(InstructionEffect::DelayedCp0Write { index, value }),
        );
        processor.state.complete_instruction(None, None);
    }

    fn jump_to(processor: &mut R3000, target: u32) {
        processor.state.write_gpr(3, target);
        step_with_word(processor, encode_register(3, 0, 0, 0x08)).expect("JR should succeed");
        step_with_word(processor, 0).expect("delay slot should succeed");
        assert_eq!(processor.state.pc(), target);
    }

    fn cp0_snapshot(processor: &R3000) -> [u32; 16] {
        std::array::from_fn(|index| processor.state.read_cp0(index))
    }

    fn encode_register(rs: u32, rt: u32, rd: u32, function: u32) -> u32 {
        (rs << 21) | (rt << 16) | (rd << 11) | function
    }

    fn encode_cp0_transfer(selector: u32, rt: u32, rd: u32) -> u32 {
        (0x10 << 26) | (selector << 21) | (rt << 16) | (rd << 11)
    }

    fn encode_cp0_branch(condition: u32, offset: u16) -> u32 {
        (0x10 << 26) | (0x08 << 21) | (condition << 16) | u32::from(offset)
    }

    #[test]
    fn processor_can_be_constructed_and_reset() {
        let mut processor = R3000::new();

        processor.reset();
    }

    #[test]
    fn fetch_reads_big_endian_word_from_physical_address() {
        let mut bus = TestBus::new([0x24, 0x01, 0xff, 0xff]);

        let word =
            fetch_instruction(PhysAddr::new(0x1fc0_0000), &mut bus).expect("fetch should succeed");

        assert_eq!(word, 0x2401_ffff);
        assert_eq!(bus.read_address, Some(PhysAddr::new(0x1fc0_0000)));
        assert_eq!(bus.read_length, Some(4));
    }

    #[test]
    fn step_fetches_from_both_direct_mapped_segments() {
        let mut processor = R3000::new();
        let mut kseg1_bus = TestBus::new([0; 4]);

        processor
            .step(&mut kseg1_bus)
            .expect("kseg1 fetch should succeed");

        assert_eq!(kseg1_bus.read_address, Some(PhysAddr::new(0x1fc0_0000)));

        jump_to(&mut processor, 0x8000_0000);
        let mut kseg0_bus = TestBus::new([0; 4]);

        processor
            .step(&mut kseg0_bus)
            .expect("kseg0 fetch should succeed");

        assert_eq!(kseg0_bus.read_address, Some(PhysAddr::new(0)));
    }

    #[test]
    fn step_fetches_from_configured_kuseg_and_kseg2_entries() {
        for (virtual_address, physical_address) in
            [(0x0040_0000, 0x0010_0000), (0xc040_0000, 0x0020_0000)]
        {
            let mut processor = R3000::new();
            install_and_sync_tlb_entry(
                &mut processor,
                5,
                virtual_address,
                physical_address,
                ENTRY_LO_VALID | ENTRY_LO_DIRTY,
            );
            jump_to(&mut processor, virtual_address);
            let mut bus = TestBus::new(0x2401_0001_u32.to_be_bytes());

            processor
                .step(&mut bus)
                .expect("mapped fetch should succeed");

            assert_eq!(
                bus.read_address,
                Some(PhysAddr::new(u64::from(physical_address)))
            );
            assert_eq!(processor.state.read_gpr(1), 1);
        }
    }

    #[test]
    fn instruction_miss_and_invalid_select_distinct_boot_vectors() {
        let virtual_address = 0x0040_0000;

        let mut miss_processor = R3000::new();
        jump_to(&mut miss_processor, virtual_address);
        let mut miss_bus = TestBus::new([0; 4]);

        miss_processor
            .step(&mut miss_bus)
            .expect("TLB miss should enter a guest exception");

        assert_eq!(miss_bus.read_address, None);
        assert_eq!(miss_processor.state.pc(), BOOT_TLB_REFILL_EXCEPTION_VECTOR);
        assert_eq!(miss_processor.state.read_cp0(8), virtual_address);
        assert_eq!((miss_processor.state.read_cp0(13) >> 2) & 0x1f, 2);

        let mut invalid_processor = R3000::new();
        install_and_sync_tlb_entry(&mut invalid_processor, 5, virtual_address, 0x0010_0000, 0);
        jump_to(&mut invalid_processor, virtual_address);
        let mut invalid_bus = TestBus::new([0; 4]);

        invalid_processor
            .step(&mut invalid_bus)
            .expect("invalid TLB entry should enter a guest exception");

        assert_eq!(invalid_bus.read_address, None);
        assert_eq!(invalid_processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);
        assert_eq!(invalid_processor.state.read_cp0(8), virtual_address);
        assert_eq!((invalid_processor.state.read_cp0(13) >> 2) & 0x1f, 2);
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
    fn delay_slot_instruction_miss_records_branch_epc_and_bd() {
        let branch_pc = 0x0040_0ffc;
        let mut processor = R3000::new();
        install_and_sync_tlb_entry(
            &mut processor,
            5,
            branch_pc,
            0x0010_0000,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY,
        );
        jump_to(&mut processor, branch_pc);

        step_with_word(&mut processor, 0x1000_0001).expect("BEQ should succeed");
        let mut bus = TestBus::new([0; 4]);

        processor
            .step(&mut bus)
            .expect("delay-slot miss should enter a guest exception");

        assert_eq!(bus.read_address, None);
        assert_eq!(processor.state.pc(), BOOT_TLB_REFILL_EXCEPTION_VECTOR);
        assert_eq!(processor.state.read_cp0(14), branch_pc);
        assert_ne!(processor.state.read_cp0(13) & (1 << 31), 0);
        assert_eq!((processor.state.read_cp0(13) >> 2) & 0x1f, 2);
    }

    #[test]
    fn misaligned_register_jump_target_faults_after_delay_slot() {
        let mut processor = R3000::new();
        let target = 1;
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
        assert_eq!(processor.state.read_cp0(12) & STATUS_TS, 0);
    }

    #[test]
    fn user_mode_kernel_segment_fetch_takes_address_error() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, STATUS_BEV | STATUS_KUC);
        processor.state.write_gpr(3, 0x8000_0000);

        step_with_word(&mut processor, encode_register(3, 0, 0, 0x08)).expect("JR should succeed");
        step_with_word(&mut processor, encode_cp0_transfer(0x04, 1, 12))
            .expect("delay-slot MTC0 should succeed");
        step_with_word(&mut processor, 0).expect("target instruction should succeed");
        assert_eq!(processor.state.pc(), 0x8000_0004);

        let mut bus = TestBus::new([0; 4]);
        processor
            .step(&mut bus)
            .expect("address error should enter a guest exception");

        assert_eq!(bus.read_address, None);
        assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);
        assert_eq!(processor.state.read_cp0(8), 0x8000_0004);
        assert_eq!((processor.state.read_cp0(13) >> 2) & 0x1f, 4);
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

    #[test]
    fn cp0_transfers_observe_their_instruction_delay() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1111_1111);
        processor.state.write_gpr(2, 0x1234_5678);

        step_with_word(&mut processor, encode_cp0_transfer(0x04, 2, 14))
            .expect("MTC0 should succeed");
        assert_eq!(processor.state.read_cp0(14), 0);

        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 14))
            .expect("MFC0 should succeed");
        assert_eq!(processor.state.read_cp0(14), 0x1234_5678);
        assert_eq!(processor.state.read_gpr(1), 0x1111_1111);

        step_with_word(&mut processor, 0x2423_0001).expect("ADDIU should succeed");
        assert_eq!(processor.state.read_gpr(1), 0);
        assert_eq!(processor.state.read_gpr(3), 0x1111_1112);

        processor.state.write_gpr(2, 0x89ab_cdef);
        step_with_word(&mut processor, encode_cp0_transfer(0x06, 2, 14))
            .expect("CTC0 should succeed");
        step_with_word(&mut processor, 0).expect("intervening instruction should succeed");
        assert_eq!(processor.state.read_cp0(14), 0x89ab_cdef);

        step_with_word(&mut processor, encode_cp0_transfer(0x02, 4, 14))
            .expect("CFC0 should succeed");
        assert_eq!(processor.state.read_gpr(4), 0);
        step_with_word(&mut processor, 0).expect("intervening instruction should succeed");
        assert_eq!(processor.state.read_gpr(4), 0x89ab_cdef);
    }

    #[test]
    fn tlb_write_changes_instruction_fetch_after_two_completed_instructions() {
        const VIRTUAL_ADDRESS: u32 = 0x0040_0000;
        const OLD_PHYSICAL_ADDRESS: u32 = 0x0010_0000;
        const NEW_PHYSICAL_ADDRESS: u32 = 0x0020_0000;

        let mut processor = R3000::new();
        install_and_sync_tlb_entry(
            &mut processor,
            5,
            VIRTUAL_ADDRESS,
            OLD_PHYSICAL_ADDRESS,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY,
        );
        set_cp0_register(&mut processor, 0, 5 << 8);
        set_cp0_register(&mut processor, 10, VIRTUAL_ADDRESS);
        set_cp0_register(
            &mut processor,
            2,
            NEW_PHYSICAL_ADDRESS | ENTRY_LO_VALID | ENTRY_LO_DIRTY,
        );
        jump_to(&mut processor, VIRTUAL_ADDRESS);

        let mut bus = AddressBus::new(&[
            (u64::from(OLD_PHYSICAL_ADDRESS), 0x4200_0002),
            (u64::from(OLD_PHYSICAL_ADDRESS + 4), 0),
            (u64::from(OLD_PHYSICAL_ADDRESS + 8), 0),
            (u64::from(NEW_PHYSICAL_ADDRESS + 12), 0x2401_0001),
        ]);

        for _ in 0..4 {
            processor
                .step(&mut bus)
                .expect("instruction should succeed");
        }

        assert_eq!(
            bus.read_addresses,
            [
                PhysAddr::new(u64::from(OLD_PHYSICAL_ADDRESS)),
                PhysAddr::new(u64::from(OLD_PHYSICAL_ADDRESS + 4)),
                PhysAddr::new(u64::from(OLD_PHYSICAL_ADDRESS + 8)),
                PhysAddr::new(u64::from(NEW_PHYSICAL_ADDRESS + 12)),
            ]
        );
        assert_eq!(processor.state.read_gpr(1), 1);
    }

    #[test]
    fn tlbwr_writes_the_pre_advance_random_entry() {
        let mut processor = R3000::new();
        let entry_hi = 0x1234_5000;
        let entry_lo = 0x3456_7000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY;
        set_cp0_register(&mut processor, 10, entry_hi);
        set_cp0_register(&mut processor, 2, entry_lo);
        let random_index = (processor.state.read_cp0(1) >> 8) as usize;

        step_with_word(&mut processor, 0x4200_0006).expect("TLBWR should succeed");
        set_cp0_register(&mut processor, 0, (random_index as u32) << 8);

        assert_eq!(
            processor.state.tlbr_effect(),
            InstructionEffect::DelayedTlbRead { entry_hi, entry_lo }
        );
    }

    #[test]
    fn translation_shutdown_sets_only_ts_and_stalls_the_processor() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1111_1111);
        processor.state.write_gpr(3, 0);
        step_with_word(&mut processor, encode_register(3, 0, 0, 0x08)).expect("JR should succeed");
        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 15))
            .expect("delay-slot MFC0 should succeed");
        assert_eq!(processor.state.pc(), 0);

        let before_core = snapshot(&processor);
        let before_cp0 = cp0_snapshot(&processor);
        let mut bus = TestBus::new([0; 4]);

        assert_eq!(processor.step(&mut bus), Err(StepError::TlbShutdown));

        let mut expected_cp0 = before_cp0;
        expected_cp0[12] |= STATUS_TS;
        assert_eq!(snapshot(&processor), before_core);
        assert_eq!(cp0_snapshot(&processor), expected_cp0);
        assert_eq!(processor.state.read_gpr(1), 0x1111_1111);
        assert_eq!(bus.read_address, None);

        assert_eq!(processor.step(&mut bus), Err(StepError::TlbShutdown));
        assert_eq!(snapshot(&processor), before_core);
        assert_eq!(cp0_snapshot(&processor), expected_cp0);

        processor.reset();
        assert_eq!(processor.state.read_cp0(12) & STATUS_TS, 0);
        step_with_word(&mut processor, 0).expect("reset vector should run after reset");
    }

    #[test]
    fn tlbp_duplicate_shutdown_does_not_complete_pending_transfer() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1111_1111);
        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 15))
            .expect("MFC0 should succeed");
        let pc = processor.state.pc();
        let random = processor.state.read_cp0(1);
        let index = processor.state.read_cp0(0);

        assert_eq!(
            step_with_word(&mut processor, 0x4200_0008),
            Err(StepError::TlbShutdown)
        );

        assert_eq!(processor.state.pc(), pc);
        assert_eq!(processor.state.read_cp0(1), random);
        assert_eq!(processor.state.read_cp0(0), index);
        assert_eq!(processor.state.read_gpr(1), 0x1111_1111);
        assert_ne!(processor.state.read_cp0(12) & STATUS_TS, 0);
    }

    #[test]
    fn direct_gpr_result_overrides_delayed_mfc0_result() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 7);

        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 15))
            .expect("MFC0 should succeed");
        step_with_word(&mut processor, 0x2421_0001).expect("ADDIU should succeed");

        assert_eq!(processor.state.read_gpr(1), 8);
    }

    #[test]
    fn mfc0_random_captures_the_predecrement_value() {
        let mut processor = R3000::new();

        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 1))
            .expect("MFC0 Random should succeed");
        assert_eq!(processor.state.read_cp0(1), 62 << 8);
        assert_eq!(processor.state.read_gpr(1), 0);

        step_with_word(&mut processor, 0).expect("intervening instruction should succeed");

        assert_eq!(processor.state.read_cp0(1), 61 << 8);
        assert_eq!(processor.state.read_gpr(1), 63 << 8);
    }

    #[test]
    fn step_error_stalls_pending_transfer_and_random() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1111_1111);

        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 15))
            .expect("MFC0 should succeed");
        let stalled_pc = processor.state.pc();
        let stalled_random = processor.state.read_cp0(1);

        let error =
            step_with_word(&mut processor, 0x8c02_0000).expect_err("LW should remain unsupported");

        assert_eq!(
            error,
            StepError::UnsupportedInstruction {
                pc: stalled_pc,
                instruction: 0x8c02_0000,
            }
        );
        assert_eq!(processor.state.pc(), stalled_pc);
        assert_eq!(processor.state.read_cp0(1), stalled_random);
        assert_eq!(processor.state.read_gpr(1), 0x1111_1111);

        step_with_word(&mut processor, 0).expect("retry should succeed");

        assert_eq!(processor.state.read_gpr(1), 0x0000_0230);
        assert_eq!(processor.state.read_cp0(1), stalled_random - (1 << 8));
    }

    #[test]
    fn step_error_stalls_pending_instruction_translation() {
        let mut processor = R3000::new();
        let virtual_address = 0x0060_0000;
        install_and_sync_tlb_entry(
            &mut processor,
            6,
            virtual_address,
            0x0010_0000,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY,
        );
        processor.state.complete_instruction(
            None,
            Some(InstructionEffect::TlbWrite {
                index: 6,
                entry_hi: virtual_address,
                entry_lo: 0x0020_0000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY,
            }),
        );

        step_with_word(&mut processor, 0x8c01_0000).expect_err("LW should remain unsupported");
        assert_eq!(
            processor
                .state
                .translate_address(virtual_address, AccessType::Instruction),
            Ok(PhysAddr::new(0x0010_0000))
        );

        step_with_word(&mut processor, 0).expect("first completion should succeed");
        assert_eq!(
            processor
                .state
                .translate_address(virtual_address, AccessType::Instruction),
            Ok(PhysAddr::new(0x0010_0000))
        );

        step_with_word(&mut processor, 0).expect("second completion should succeed");
        assert_eq!(
            processor
                .state
                .translate_address(virtual_address, AccessType::Instruction),
            Ok(PhysAddr::new(0x0020_0000))
        );
    }

    #[test]
    fn bus_fault_stalls_pending_transfer_and_random() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1111_1111);
        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 15))
            .expect("MFC0 should succeed");
        let stalled_pc = processor.state.pc();
        let stalled_random = processor.state.read_cp0(1);
        let mut bus = TestBus::new([0; 4]);
        bus.fault = Some(BusFault::Unmapped);

        let error = processor.step(&mut bus).expect_err("fetch should fault");

        assert_eq!(
            error,
            StepError::BusFault {
                address: PhysAddr::new(0x1fc0_0004),
                fault: BusFault::Unmapped,
            }
        );
        assert_eq!(processor.state.pc(), stalled_pc);
        assert_eq!(processor.state.read_cp0(1), stalled_random);
        assert_eq!(processor.state.read_gpr(1), 0x1111_1111);

        step_with_word(&mut processor, 0).expect("retry should succeed");
        assert_eq!(processor.state.read_gpr(1), 0x0000_0230);
    }

    #[test]
    fn cp0_condition_drives_taken_and_not_taken_branches() {
        let mut processor = R3000::new();

        processor.set_cp0_condition(false);
        step_with_word(&mut processor, encode_cp0_branch(0, 2)).expect("BC0F should succeed");
        step_with_word(&mut processor, 0).expect("delay slot should succeed");
        assert_eq!(processor.state.pc(), 0xbfc0_000c);

        processor.reset();
        processor.set_cp0_condition(false);
        step_with_word(&mut processor, encode_cp0_branch(1, 2)).expect("BC0T should succeed");
        step_with_word(&mut processor, 0).expect("delay slot should succeed");
        assert_eq!(processor.state.pc(), 0xbfc0_0008);

        processor.reset();
        processor.set_cp0_condition(true);
        step_with_word(&mut processor, encode_cp0_branch(1, 2)).expect("BC0T should succeed");
        step_with_word(&mut processor, 0).expect("delay slot should succeed");
        assert_eq!(processor.state.pc(), 0xbfc0_000c);
    }

    #[test]
    fn rfe_restores_status_without_loading_epc() {
        const STATUS_BEV: u32 = 1 << 22;

        let mut processor = R3000::new();
        processor.state.write_gpr(1, STATUS_BEV | 0x0c);

        step_with_word(&mut processor, encode_cp0_transfer(0x04, 1, 12))
            .expect("MTC0 should succeed");
        step_with_word(&mut processor, 0).expect("intervening instruction should succeed");
        let rfe_pc = processor.state.pc();

        step_with_word(&mut processor, 0x4200_0010).expect("RFE should succeed");

        assert_eq!(processor.state.read_cp0(12) & 0x3f, 0x03);
        assert_eq!(processor.state.read_cp0(14), 0);
        assert_eq!(processor.state.pc(), rfe_pc + 4);
    }

    #[test]
    fn user_mode_cp0_access_takes_coprocessor_unusable() {
        const STATUS_BEV: u32 = 1 << 22;
        const STATUS_KUC: u32 = 1 << 1;
        const USER_PC: u32 = 0x0040_0000;

        let mut processor = R3000::new();
        processor.state.write_gpr(1, STATUS_BEV | STATUS_KUC);
        processor.state.write_gpr(3, USER_PC);
        processor.state.complete_instruction(
            None,
            Some(InstructionEffect::TlbWrite {
                index: 0,
                entry_hi: USER_PC,
                entry_lo: 1 << 9,
            }),
        );
        processor.state.complete_instruction(None, None);
        processor.state.complete_instruction(None, None);

        step_with_word(&mut processor, encode_register(3, 0, 0, 0x08)).expect("JR should succeed");
        step_with_word(&mut processor, encode_cp0_transfer(0x04, 1, 12))
            .expect("delay-slot MTC0 should succeed");
        step_with_word(&mut processor, 0).expect("mapped user instruction should succeed");
        let fault_pc = processor.state.pc();

        step_with_word(&mut processor, encode_cp0_transfer(0x00, 2, 15))
            .expect("guest exception should succeed");

        assert_eq!(processor.state.pc(), BOOT_GENERAL_EXCEPTION_VECTOR);
        assert_eq!(processor.state.read_cp0(14), fault_pc);
        assert_eq!((processor.state.read_cp0(13) >> 2) & 0x1f, 11);
        assert_eq!((processor.state.read_cp0(13) >> 28) & 3, 0);
        assert_eq!(processor.state.read_gpr(2), 0);
    }

    #[test]
    fn reset_clears_transfer_and_preserves_cp0_condition() {
        let mut processor = R3000::new();
        processor.state.write_gpr(1, 0x1111_1111);
        processor.set_cp0_condition(true);

        step_with_word(&mut processor, encode_cp0_transfer(0x00, 1, 15))
            .expect("MFC0 should succeed");
        processor.reset();

        assert_eq!(processor.state.read_cp0(1), 63 << 8);
        step_with_word(&mut processor, encode_cp0_branch(1, 2)).expect("BC0T should succeed");
        step_with_word(&mut processor, 0).expect("delay slot should succeed");

        assert_eq!(processor.state.read_gpr(1), 0x1111_1111);
        assert_eq!(processor.state.pc(), 0xbfc0_000c);
    }
}
