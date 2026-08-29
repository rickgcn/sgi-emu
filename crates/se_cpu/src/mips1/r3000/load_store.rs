use se_core::bus::{PhysAddr, PhysicalBus};

use super::{
    ExecutionOutcome, StepError,
    cp0::Exception,
    decode::MemoryInstruction,
    mmu::{AccessType, Translation},
    state::{InstructionEffect, LoadKind, State, TranslationError},
};

pub(super) fn execute(
    state: &mut State,
    instruction: MemoryInstruction,
    bus: &mut dyn PhysicalBus,
) -> Result<ExecutionOutcome<Option<InstructionEffect>>, StepError> {
    let effect = match instruction {
        MemoryInstruction::Lb { base, rt, offset } => {
            let mut bytes = [0; 1];
            match load(state, base, offset, &mut bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: (bytes[0] as i8 as i32) as u32,
                load_merge_bypass: true,
            }
        }
        MemoryInstruction::Lbu { base, rt, offset } => {
            let mut bytes = [0; 1];
            match load(state, base, offset, &mut bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from(bytes[0]),
                load_merge_bypass: true,
            }
        }
        MemoryInstruction::Lh { base, rt, offset } => {
            let mut bytes = [0; 2];
            match load(state, base, offset, &mut bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: (i16::from_be_bytes(bytes) as i32) as u32,
                load_merge_bypass: true,
            }
        }
        MemoryInstruction::Lhu { base, rt, offset } => {
            let mut bytes = [0; 2];
            match load(state, base, offset, &mut bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from(u16::from_be_bytes(bytes)),
                load_merge_bypass: true,
            }
        }
        MemoryInstruction::Lwl { base, rt, offset } => {
            let virtual_address = effective_address(state, base, offset);
            let byte = (virtual_address & 3) as usize;
            let translation = match translate_address(state, virtual_address, AccessType::Load)? {
                ExecutionOutcome::Completed(translation) => translation,
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            };
            let length = 4 - byte;
            let mut memory = [0; 4];
            if state
                .load_memory(LoadKind::Data, translation, &mut memory[..length], bus)
                .is_err()
            {
                return Ok(ExecutionOutcome::Exception(Exception::DataBusError));
            }

            let mut bytes = state.read_gpr_for_load_merge(rt).to_be_bytes();
            bytes[..length].copy_from_slice(&memory[..length]);
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from_be_bytes(bytes),
                load_merge_bypass: true,
            }
        }
        MemoryInstruction::Lw { base, rt, offset } => {
            let mut bytes = [0; 4];
            match load(state, base, offset, &mut bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from_be_bytes(bytes),
                load_merge_bypass: true,
            }
        }
        MemoryInstruction::Lwr { base, rt, offset } => {
            let virtual_address = effective_address(state, base, offset);
            let byte = (virtual_address & 3) as usize;
            let mut translation = match translate_address(state, virtual_address, AccessType::Load)?
            {
                ExecutionOutcome::Completed(translation) => translation,
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            };
            translation.address = PhysAddr::new(translation.address.get() - byte as u64);
            let length = byte + 1;
            let mut memory = [0; 4];
            if state
                .load_memory(LoadKind::Data, translation, &mut memory[..length], bus)
                .is_err()
            {
                return Ok(ExecutionOutcome::Exception(Exception::DataBusError));
            }

            let mut bytes = state.read_gpr_for_load_merge(rt).to_be_bytes();
            bytes[4 - length..].copy_from_slice(&memory[..length]);
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from_be_bytes(bytes),
                load_merge_bypass: true,
            }
        }
        MemoryInstruction::Sb { base, rt, offset } => {
            let bytes = [state.read_gpr(rt) as u8];
            match store(state, base, offset, &bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            return Ok(ExecutionOutcome::Completed(None));
        }
        MemoryInstruction::Sh { base, rt, offset } => {
            let bytes = (state.read_gpr(rt) as u16).to_be_bytes();
            match store(state, base, offset, &bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            return Ok(ExecutionOutcome::Completed(None));
        }
        MemoryInstruction::Swl { base, rt, offset } => {
            let virtual_address = effective_address(state, base, offset);
            let byte = (virtual_address & 3) as usize;
            let translation = match translate_address(state, virtual_address, AccessType::Store)? {
                ExecutionOutcome::Completed(translation) => translation,
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            };
            let physical_address = translation.address;
            let bytes = state.read_gpr(rt).to_be_bytes();
            state
                .store_memory(translation, &bytes[..4 - byte], bus)
                .map_err(|fault| StepError::BusFault {
                    address: physical_address,
                    fault,
                })?;
            return Ok(ExecutionOutcome::Completed(None));
        }
        MemoryInstruction::Sw { base, rt, offset } => {
            let bytes = state.read_gpr(rt).to_be_bytes();
            match store(state, base, offset, &bytes, bus)? {
                ExecutionOutcome::Completed(()) => {}
                ExecutionOutcome::Exception(exception) => {
                    return Ok(ExecutionOutcome::Exception(exception));
                }
            }
            return Ok(ExecutionOutcome::Completed(None));
        }
        MemoryInstruction::Swr { base, rt, offset } => {
            let virtual_address = effective_address(state, base, offset);
            let byte = (virtual_address & 3) as usize;
            let mut translation =
                match translate_address(state, virtual_address, AccessType::Store)? {
                    ExecutionOutcome::Completed(translation) => translation,
                    ExecutionOutcome::Exception(exception) => {
                        return Ok(ExecutionOutcome::Exception(exception));
                    }
                };
            translation.address = PhysAddr::new(translation.address.get() - byte as u64);
            let physical_address = translation.address;
            let bytes = state.read_gpr(rt).to_be_bytes();
            state
                .store_memory(translation, &bytes[3 - byte..], bus)
                .map_err(|fault| StepError::BusFault {
                    address: physical_address,
                    fault,
                })?;
            return Ok(ExecutionOutcome::Completed(None));
        }
    };

    Ok(ExecutionOutcome::Completed(Some(effect)))
}

pub(super) fn load(
    state: &mut State,
    base: usize,
    offset: u16,
    data: &mut [u8],
    bus: &mut dyn PhysicalBus,
) -> Result<ExecutionOutcome<()>, StepError> {
    let address = effective_address(state, base, offset);
    let misaligned = match data.len() {
        1 => false,
        2 => address & 1 != 0,
        4 => address & 3 != 0,
        _ => unreachable!("aligned R3000 loads use one, two, or four bytes"),
    };
    if misaligned {
        return Ok(ExecutionOutcome::Exception(Exception::LoadAddressError {
            address,
        }));
    }

    let translation = match translate_address(state, address, AccessType::Load)? {
        ExecutionOutcome::Completed(translation) => translation,
        ExecutionOutcome::Exception(exception) => {
            return Ok(ExecutionOutcome::Exception(exception));
        }
    };

    if state
        .load_memory(LoadKind::Data, translation, data, bus)
        .is_err()
    {
        return Ok(ExecutionOutcome::Exception(Exception::DataBusError));
    }

    Ok(ExecutionOutcome::Completed(()))
}

pub(super) fn store(
    state: &mut State,
    base: usize,
    offset: u16,
    data: &[u8],
    bus: &mut dyn PhysicalBus,
) -> Result<ExecutionOutcome<()>, StepError> {
    let address = effective_address(state, base, offset);
    let misaligned = match data.len() {
        1 => false,
        2 => address & 1 != 0,
        4 => address & 3 != 0,
        _ => unreachable!("aligned R3000 stores use one, two, or four bytes"),
    };
    if misaligned {
        return Ok(ExecutionOutcome::Exception(Exception::StoreAddressError {
            address,
        }));
    }

    let translation = match translate_address(state, address, AccessType::Store)? {
        ExecutionOutcome::Completed(translation) => translation,
        ExecutionOutcome::Exception(exception) => {
            return Ok(ExecutionOutcome::Exception(exception));
        }
    };
    let physical_address = translation.address;

    state
        .store_memory(translation, data, bus)
        .map_err(|fault| StepError::BusFault {
            address: physical_address,
            fault,
        })?;

    Ok(ExecutionOutcome::Completed(()))
}

fn effective_address(state: &State, base: usize, offset: u16) -> u32 {
    state
        .read_gpr(base)
        .wrapping_add((offset as i16 as i32) as u32)
}

fn translate_address(
    state: &mut State,
    virtual_address: u32,
    access: AccessType,
) -> Result<ExecutionOutcome<Translation>, StepError> {
    match state.translate_address(virtual_address, access) {
        Ok(translation) => Ok(ExecutionOutcome::Completed(translation)),
        Err(TranslationError::Exception(exception)) => Ok(ExecutionOutcome::Exception(exception)),
        Err(TranslationError::TlbShutdown) => Err(StepError::TlbShutdown),
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

    use super::{
        Exception, ExecutionOutcome, InstructionEffect, MemoryInstruction, State, StepError,
        execute,
    };
    use crate::mips1::r3000::{TEST_CONFIG, cp0::TlbFaultKind};

    const ENTRY_LO_DIRTY: u32 = 1 << 10;
    const ENTRY_LO_VALID: u32 = 1 << 9;

    fn completed<T>(value: T) -> Result<ExecutionOutcome<T>, StepError> {
        Ok(ExecutionOutcome::Completed(value))
    }

    fn guest_exception<T>(value: Exception) -> Result<ExecutionOutcome<T>, StepError> {
        Ok(ExecutionOutcome::Exception(value))
    }

    struct TestBus {
        read_data: [u8; 4],
        reads: Vec<(PhysAddr, usize)>,
        writes: Vec<(PhysAddr, Vec<u8>)>,
        read_fault: Option<BusFault>,
        write_fault: Option<BusFault>,
    }

    impl TestBus {
        fn new(read_data: [u8; 4]) -> Self {
            Self {
                read_data,
                reads: Vec::new(),
                writes: Vec::new(),
                read_fault: None,
                write_fault: None,
            }
        }
    }

    impl PhysicalBus for TestBus {
        fn read(&mut self, address: PhysAddr, data: &mut [u8]) -> Result<(), BusFault> {
            self.reads.push((address, data.len()));
            if let Some(fault) = self.read_fault {
                return Err(fault);
            }

            data.copy_from_slice(&self.read_data[..data.len()]);
            Ok(())
        }

        fn write(&mut self, address: PhysAddr, data: &[u8]) -> Result<(), BusFault> {
            self.writes.push((address, data.to_vec()));
            if let Some(fault) = self.write_fault {
                return Err(fault);
            }

            Ok(())
        }
    }

    fn install_tlb_entry(state: &mut State, virtual_address: u32, flags: u32) {
        state.complete_instruction(
            None,
            Some(InstructionEffect::TlbWrite {
                index: 1,
                entry_hi: virtual_address,
                entry_lo: 0x0010_0000 | flags,
            }),
        );
    }

    #[test]
    fn loads_convert_big_endian_bytes_and_return_delayed_writes() {
        let cases = [
            (
                MemoryInstruction::Lb {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                [0x80, 0x01, 0x23, 0x45],
                1,
                0xffff_ff80,
            ),
            (
                MemoryInstruction::Lbu {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                [0x80, 0x01, 0x23, 0x45],
                1,
                0x0000_0080,
            ),
            (
                MemoryInstruction::Lh {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                [0x80, 0x01, 0x23, 0x45],
                2,
                0xffff_8001,
            ),
            (
                MemoryInstruction::Lhu {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                [0x80, 0x01, 0x23, 0x45],
                2,
                0x0000_8001,
            ),
            (
                MemoryInstruction::Lw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                [0x89, 0xab, 0xcd, 0xef],
                4,
                0x89ab_cdef,
            ),
        ];

        for (instruction, read_data, length, value) in cases {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 0xa000_0100);
            state.write_gpr(2, 0x1111_1111);
            let mut bus = TestBus::new(read_data);

            assert_eq!(
                execute(&mut state, instruction, &mut bus),
                completed(Some(InstructionEffect::DelayedGprWrite {
                    index: 2,
                    value,
                    load_merge_bypass: true,
                }))
            );
            assert_eq!(state.read_gpr(2), 0x1111_1111);
            assert_eq!(bus.reads, [(PhysAddr::new(0x100), length)]);
            assert!(bus.writes.is_empty());
        }
    }

    #[test]
    fn stores_write_low_register_bytes_in_big_endian_order() {
        let cases: [(MemoryInstruction, u64, &[u8]); 3] = [
            (
                MemoryInstruction::Sb {
                    base: 1,
                    rt: 2,
                    offset: 3,
                },
                0x103,
                &[0xef],
            ),
            (
                MemoryInstruction::Sh {
                    base: 1,
                    rt: 2,
                    offset: 2,
                },
                0x102,
                &[0xcd, 0xef],
            ),
            (
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                0x100,
                &[0x89, 0xab, 0xcd, 0xef],
            ),
        ];

        for (instruction, address, bytes) in cases {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 0xa000_0100);
            state.write_gpr(2, 0x89ab_cdef);
            let mut bus = TestBus::new([0; 4]);

            assert_eq!(execute(&mut state, instruction, &mut bus), completed(None));
            assert!(bus.reads.is_empty());
            assert_eq!(bus.writes, [(PhysAddr::new(address), bytes.to_vec())]);
        }
    }

    #[test]
    fn partial_loads_merge_all_offsets_and_issue_exact_transactions() {
        let memory = [0x11, 0x22, 0x33, 0x44];
        let old_value = 0xaabb_ccdd;
        let lwl_results = [0x1122_3344, 0x2233_44dd, 0x3344_ccdd, 0x44bb_ccdd];
        let lwr_results = [0xaabb_cc11, 0xaabb_1122, 0xaa11_2233, 0x1122_3344];

        for (byte, expected) in lwl_results.into_iter().enumerate() {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 0xa000_0100 + byte as u32);
            state.write_gpr(2, old_value);
            let length = 4 - byte;
            let mut read_data = [0; 4];
            read_data[..length].copy_from_slice(&memory[byte..]);
            let mut bus = TestBus::new(read_data);

            assert_eq!(
                execute(
                    &mut state,
                    MemoryInstruction::Lwl {
                        base: 1,
                        rt: 2,
                        offset: 0,
                    },
                    &mut bus,
                ),
                completed(Some(InstructionEffect::DelayedGprWrite {
                    index: 2,
                    value: expected,
                    load_merge_bypass: true,
                }))
            );
            assert_eq!(bus.reads, [(PhysAddr::new(0x100 + byte as u64), length)]);
            assert!(bus.writes.is_empty());
        }

        for (byte, expected) in lwr_results.into_iter().enumerate() {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 0xa000_0100 + byte as u32);
            state.write_gpr(2, old_value);
            let length = byte + 1;
            let mut bus = TestBus::new(memory);

            assert_eq!(
                execute(
                    &mut state,
                    MemoryInstruction::Lwr {
                        base: 1,
                        rt: 2,
                        offset: 0,
                    },
                    &mut bus,
                ),
                completed(Some(InstructionEffect::DelayedGprWrite {
                    index: 2,
                    value: expected,
                    load_merge_bypass: true,
                }))
            );
            assert_eq!(bus.reads, [(PhysAddr::new(0x100), length)]);
            assert!(bus.writes.is_empty());
        }
    }

    #[test]
    fn partial_stores_select_all_big_endian_slices_and_addresses() {
        let bytes = 0xaabb_ccdd_u32.to_be_bytes();

        for byte in 0..4 {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 0xa000_0100 + byte as u32);
            state.write_gpr(2, u32::from_be_bytes(bytes));
            let mut bus = TestBus::new([0; 4]);

            assert_eq!(
                execute(
                    &mut state,
                    MemoryInstruction::Swl {
                        base: 1,
                        rt: 2,
                        offset: 0,
                    },
                    &mut bus,
                ),
                completed(None)
            );
            assert!(bus.reads.is_empty());
            assert_eq!(
                bus.writes,
                [(
                    PhysAddr::new(0x100 + byte as u64),
                    bytes[..4 - byte].to_vec(),
                )]
            );
        }

        for byte in 0..4 {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 0xa000_0100 + byte as u32);
            state.write_gpr(2, u32::from_be_bytes(bytes));
            let mut bus = TestBus::new([0; 4]);

            assert_eq!(
                execute(
                    &mut state,
                    MemoryInstruction::Swr {
                        base: 1,
                        rt: 2,
                        offset: 0,
                    },
                    &mut bus,
                ),
                completed(None)
            );
            assert!(bus.reads.is_empty());
            assert_eq!(
                bus.writes,
                [(PhysAddr::new(0x100), bytes[3 - byte..].to_vec(),)]
            );
        }
    }

    #[test]
    fn partial_accesses_sign_extend_wrap_and_never_raise_alignment_errors() {
        let mut state = State::new(TEST_CONFIG);
        state.write_gpr(1, 0xa000_0104);
        state.write_gpr(2, 0xaabb_ccdd);
        let mut bus = TestBus::new([0x22, 0x33, 0x44, 0]);

        assert_eq!(
            execute(
                &mut state,
                MemoryInstruction::Lwl {
                    base: 1,
                    rt: 2,
                    offset: 0xfffd,
                },
                &mut bus,
            ),
            completed(Some(InstructionEffect::DelayedGprWrite {
                index: 2,
                value: 0x2233_44dd,
                load_merge_bypass: true,
            }))
        );
        assert_eq!(bus.reads, [(PhysAddr::new(0x101), 3)]);

        state.write_gpr(1, 0);
        let mut wrapping_bus = TestBus::new([0; 4]);
        assert_eq!(
            execute(
                &mut state,
                MemoryInstruction::Lwr {
                    base: 1,
                    rt: 2,
                    offset: 0xffff,
                },
                &mut wrapping_bus,
            ),
            guest_exception(Exception::TlbLoad {
                address: u32::MAX,
                fault: TlbFaultKind::Miss,
            })
        );
        assert!(wrapping_bus.reads.is_empty());
    }

    #[test]
    fn effective_addresses_sign_extend_and_wrap() {
        let mut state = State::new(TEST_CONFIG);
        state.write_gpr(1, 0xa000_0104);
        let mut bus = TestBus::new([0x5a, 0, 0, 0]);

        assert_eq!(
            execute(
                &mut state,
                MemoryInstruction::Lbu {
                    base: 1,
                    rt: 2,
                    offset: 0xfffc,
                },
                &mut bus,
            ),
            completed(Some(InstructionEffect::DelayedGprWrite {
                index: 2,
                value: 0x5a,
                load_merge_bypass: true,
            }))
        );
        assert_eq!(bus.reads, [(PhysAddr::new(0x100), 1)]);

        state.write_gpr(1, 0);
        let mut wrapping_bus = TestBus::new([0; 4]);
        assert_eq!(
            execute(
                &mut state,
                MemoryInstruction::Lbu {
                    base: 1,
                    rt: 2,
                    offset: 0xffff,
                },
                &mut wrapping_bus,
            ),
            guest_exception(Exception::TlbLoad {
                address: u32::MAX,
                fault: TlbFaultKind::Miss,
            })
        );
        assert!(wrapping_bus.reads.is_empty());
    }

    #[test]
    fn alignment_errors_precede_translation_and_bus_access() {
        let cases = [
            (
                MemoryInstruction::Lh {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::LoadAddressError { address: 1 },
            ),
            (
                MemoryInstruction::Lhu {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::LoadAddressError { address: 1 },
            ),
            (
                MemoryInstruction::Lw {
                    base: 1,
                    rt: 2,
                    offset: 1,
                },
                Exception::LoadAddressError { address: 2 },
            ),
            (
                MemoryInstruction::Sh {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::StoreAddressError { address: 1 },
            ),
            (
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 2,
                    offset: 1,
                },
                Exception::StoreAddressError { address: 2 },
            ),
        ];

        for (instruction, exception) in cases {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 1);
            state.write_gpr(2, 0x1234_5678);
            let mut bus = TestBus::new([0; 4]);

            assert_eq!(
                execute(&mut state, instruction, &mut bus),
                guest_exception(exception)
            );
            assert!(bus.reads.is_empty());
            assert!(bus.writes.is_empty());
        }

        let mut state = State::new(TEST_CONFIG);
        state.write_gpr(1, 0xa000_0101);
        let mut bus = TestBus::new([0x7f, 0, 0, 0]);
        assert_eq!(
            execute(
                &mut state,
                MemoryInstruction::Lb {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            completed(Some(InstructionEffect::DelayedGprWrite {
                index: 2,
                value: 0x7f,
                load_merge_bypass: true,
            }))
        );
        assert_eq!(bus.reads, [(PhysAddr::new(0x101), 1)]);
    }

    #[test]
    fn translation_uses_load_store_and_modified_faults() {
        let virtual_address = 0x1234_5000;
        let cases = [
            (
                MemoryInstruction::Lw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::TlbLoad {
                    address: virtual_address,
                    fault: TlbFaultKind::Miss,
                },
            ),
            (
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::TlbStore {
                    address: virtual_address,
                    fault: TlbFaultKind::Miss,
                },
            ),
        ];

        for (instruction, exception) in cases {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, virtual_address);
            let mut bus = TestBus::new([0; 4]);
            assert_eq!(
                execute(&mut state, instruction, &mut bus),
                guest_exception(exception)
            );
        }

        for (instruction, exception) in [
            (
                MemoryInstruction::Lw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::TlbLoad {
                    address: virtual_address,
                    fault: TlbFaultKind::Invalid,
                },
            ),
            (
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::TlbStore {
                    address: virtual_address,
                    fault: TlbFaultKind::Invalid,
                },
            ),
        ] {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, virtual_address);
            install_tlb_entry(&mut state, virtual_address, 0);
            let mut bus = TestBus::new([0; 4]);
            assert_eq!(
                execute(&mut state, instruction, &mut bus),
                guest_exception(exception)
            );
        }

        let mut state = State::new(TEST_CONFIG);
        state.write_gpr(1, virtual_address);
        state.write_gpr(2, 0x1234_5678);
        install_tlb_entry(&mut state, virtual_address, ENTRY_LO_VALID);
        let mut bus = TestBus::new([0; 4]);
        assert_eq!(
            execute(
                &mut state,
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut bus,
            ),
            guest_exception(Exception::TlbModified {
                address: virtual_address,
            })
        );

        let mut writable_state = State::new(TEST_CONFIG);
        writable_state.write_gpr(1, virtual_address);
        writable_state.write_gpr(2, 0x1234_5678);
        install_tlb_entry(
            &mut writable_state,
            virtual_address,
            ENTRY_LO_VALID | ENTRY_LO_DIRTY,
        );
        let mut writable_bus = TestBus::new([0; 4]);
        assert_eq!(
            execute(
                &mut writable_state,
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut writable_bus,
            ),
            completed(None)
        );
    }

    #[test]
    fn bus_faults_distinguish_load_exception_and_store_step_error() {
        let mut load_state = State::new(TEST_CONFIG);
        load_state.write_gpr(1, 0xa000_0100);
        let mut load_bus = TestBus::new([0; 4]);
        load_bus.read_fault = Some(BusFault::Unmapped);

        assert_eq!(
            execute(
                &mut load_state,
                MemoryInstruction::Lw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut load_bus,
            ),
            guest_exception(Exception::DataBusError)
        );

        let mut store_state = State::new(TEST_CONFIG);
        store_state.write_gpr(1, 0xa000_0100);
        store_state.write_gpr(2, 0x1234_5678);
        let mut store_bus = TestBus::new([0; 4]);
        store_bus.write_fault = Some(BusFault::UnsupportedAccess);

        assert_eq!(
            execute(
                &mut store_state,
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut store_bus,
            ),
            Err(StepError::BusFault {
                address: PhysAddr::new(0x100),
                fault: BusFault::UnsupportedAccess,
            })
        );
    }

    #[test]
    fn partial_access_faults_preserve_original_translation_and_request_addresses() {
        let virtual_address = 0x1234_5002;
        for (instruction, exception) in [
            (
                MemoryInstruction::Lwr {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::TlbLoad {
                    address: virtual_address,
                    fault: TlbFaultKind::Miss,
                },
            ),
            (
                MemoryInstruction::Swr {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                Exception::TlbStore {
                    address: virtual_address,
                    fault: TlbFaultKind::Miss,
                },
            ),
        ] {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, virtual_address);
            let mut bus = TestBus::new([0; 4]);

            assert_eq!(
                execute(&mut state, instruction, &mut bus),
                guest_exception(exception)
            );
            assert!(bus.reads.is_empty());
            assert!(bus.writes.is_empty());
        }

        let mut modified_state = State::new(TEST_CONFIG);
        modified_state.write_gpr(1, virtual_address);
        modified_state.write_gpr(2, 0x1234_5678);
        install_tlb_entry(&mut modified_state, virtual_address, ENTRY_LO_VALID);
        let mut modified_bus = TestBus::new([0; 4]);
        assert_eq!(
            execute(
                &mut modified_state,
                MemoryInstruction::Swr {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut modified_bus,
            ),
            guest_exception(Exception::TlbModified {
                address: virtual_address,
            })
        );

        let mut load_state = State::new(TEST_CONFIG);
        load_state.write_gpr(1, 0xa000_0102);
        let mut load_bus = TestBus::new([0; 4]);
        load_bus.read_fault = Some(BusFault::Unmapped);
        assert_eq!(
            execute(
                &mut load_state,
                MemoryInstruction::Lwr {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut load_bus,
            ),
            guest_exception(Exception::DataBusError)
        );
        assert_eq!(load_bus.reads, [(PhysAddr::new(0x100), 3)]);

        let mut store_state = State::new(TEST_CONFIG);
        store_state.write_gpr(1, 0xa000_0102);
        store_state.write_gpr(2, 0x1234_5678);
        let mut store_bus = TestBus::new([0; 4]);
        store_bus.write_fault = Some(BusFault::UnsupportedAccess);
        assert_eq!(
            execute(
                &mut store_state,
                MemoryInstruction::Swr {
                    base: 1,
                    rt: 2,
                    offset: 0,
                },
                &mut store_bus,
            ),
            Err(StepError::BusFault {
                address: PhysAddr::new(0x100),
                fault: BusFault::UnsupportedAccess,
            })
        );
        assert_eq!(
            store_bus.writes,
            [(
                PhysAddr::new(0x100),
                0x1234_5678_u32.to_be_bytes()[1..].to_vec(),
            )]
        );
    }

    #[test]
    fn partial_accesses_report_duplicate_translation_shutdown() {
        for instruction in [
            MemoryInstruction::Lwl {
                base: 1,
                rt: 2,
                offset: 0,
            },
            MemoryInstruction::Swr {
                base: 1,
                rt: 2,
                offset: 0,
            },
        ] {
            let mut state = State::new(TEST_CONFIG);
            state.write_gpr(1, 0x1234_5002);
            state.complete_instruction(
                None,
                Some(InstructionEffect::TlbWrite {
                    index: 1,
                    entry_hi: 0x1234_5000,
                    entry_lo: 0x0010_0000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY,
                }),
            );
            state.complete_instruction(
                None,
                Some(InstructionEffect::TlbWrite {
                    index: 2,
                    entry_hi: 0x1234_5000,
                    entry_lo: 0x0020_0000 | ENTRY_LO_VALID | ENTRY_LO_DIRTY,
                }),
            );
            let mut bus = TestBus::new([0; 4]);

            assert_eq!(
                execute(&mut state, instruction, &mut bus),
                Err(StepError::TlbShutdown)
            );
            assert!(state.is_tlb_shutdown());
            assert!(bus.reads.is_empty());
            assert!(bus.writes.is_empty());
        }
    }
}
