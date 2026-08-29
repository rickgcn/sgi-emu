use se_core::bus::PhysicalBus;

use super::{
    ExecutionError,
    cp0::Exception,
    decode::MemoryInstruction,
    mmu::AccessType,
    state::{InstructionEffect, LoadKind, State, TranslationError},
};

pub(super) fn execute(
    state: &mut State,
    instruction: MemoryInstruction,
    bus: &mut dyn PhysicalBus,
) -> Result<Option<InstructionEffect>, ExecutionError> {
    let effect = match instruction {
        MemoryInstruction::Lb { base, rt, offset } => {
            let mut bytes = [0; 1];
            load(state, base, offset, &mut bytes, bus)?;
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: (bytes[0] as i8 as i32) as u32,
            }
        }
        MemoryInstruction::Lbu { base, rt, offset } => {
            let mut bytes = [0; 1];
            load(state, base, offset, &mut bytes, bus)?;
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from(bytes[0]),
            }
        }
        MemoryInstruction::Lh { base, rt, offset } => {
            let mut bytes = [0; 2];
            load(state, base, offset, &mut bytes, bus)?;
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: (i16::from_be_bytes(bytes) as i32) as u32,
            }
        }
        MemoryInstruction::Lhu { base, rt, offset } => {
            let mut bytes = [0; 2];
            load(state, base, offset, &mut bytes, bus)?;
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from(u16::from_be_bytes(bytes)),
            }
        }
        MemoryInstruction::Lw { base, rt, offset } => {
            let mut bytes = [0; 4];
            load(state, base, offset, &mut bytes, bus)?;
            InstructionEffect::DelayedGprWrite {
                index: rt,
                value: u32::from_be_bytes(bytes),
            }
        }
        MemoryInstruction::Sb { base, rt, offset } => {
            let bytes = [state.read_gpr(rt) as u8];
            store(state, base, offset, &bytes, bus)?;
            return Ok(None);
        }
        MemoryInstruction::Sh { base, rt, offset } => {
            let bytes = (state.read_gpr(rt) as u16).to_be_bytes();
            store(state, base, offset, &bytes, bus)?;
            return Ok(None);
        }
        MemoryInstruction::Sw { base, rt, offset } => {
            let bytes = state.read_gpr(rt).to_be_bytes();
            store(state, base, offset, &bytes, bus)?;
            return Ok(None);
        }
    };

    Ok(Some(effect))
}

fn load(
    state: &mut State,
    base: usize,
    offset: u16,
    data: &mut [u8],
    bus: &mut dyn PhysicalBus,
) -> Result<(), ExecutionError> {
    let address = state
        .read_gpr(base)
        .wrapping_add((offset as i16 as i32) as u32);
    let misaligned = match data.len() {
        1 => false,
        2 => address & 1 != 0,
        4 => address & 3 != 0,
        _ => unreachable!("aligned R3000 loads use one, two, or four bytes"),
    };
    if misaligned {
        return Err(ExecutionError::Exception(Exception::LoadAddressError {
            address,
        }));
    }

    let translation = match state.translate_address(address, AccessType::Load) {
        Ok(translation) => translation,
        Err(TranslationError::Exception(exception)) => {
            return Err(ExecutionError::Exception(exception));
        }
        Err(TranslationError::TlbShutdown) => return Err(ExecutionError::TlbShutdown),
    };

    state
        .load_memory(LoadKind::Data, translation, data, bus)
        .map_err(|_| ExecutionError::Exception(Exception::DataBusError))
}

fn store(
    state: &mut State,
    base: usize,
    offset: u16,
    data: &[u8],
    bus: &mut dyn PhysicalBus,
) -> Result<(), ExecutionError> {
    let address = state
        .read_gpr(base)
        .wrapping_add((offset as i16 as i32) as u32);
    let misaligned = match data.len() {
        1 => false,
        2 => address & 1 != 0,
        4 => address & 3 != 0,
        _ => unreachable!("aligned R3000 stores use one, two, or four bytes"),
    };
    if misaligned {
        return Err(ExecutionError::Exception(Exception::StoreAddressError {
            address,
        }));
    }

    let translation = match state.translate_address(address, AccessType::Store) {
        Ok(translation) => translation,
        Err(TranslationError::Exception(exception)) => {
            return Err(ExecutionError::Exception(exception));
        }
        Err(TranslationError::TlbShutdown) => return Err(ExecutionError::TlbShutdown),
    };
    let physical_address = translation.address;

    state
        .store_memory(translation, data, bus)
        .map_err(|fault| ExecutionError::BusFault {
            address: physical_address,
            fault,
        })
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, PhysAddr, PhysicalBus};

    use super::{Exception, ExecutionError, InstructionEffect, MemoryInstruction, State, execute};
    use crate::mips1::r3000::{TEST_CONFIG, cp0::TlbFaultKind};

    const ENTRY_LO_DIRTY: u32 = 1 << 10;
    const ENTRY_LO_VALID: u32 = 1 << 9;

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
                Ok(Some(InstructionEffect::DelayedGprWrite { index: 2, value }))
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

            assert_eq!(execute(&mut state, instruction, &mut bus), Ok(None));
            assert!(bus.reads.is_empty());
            assert_eq!(bus.writes, [(PhysAddr::new(address), bytes.to_vec())]);
        }
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
            Ok(Some(InstructionEffect::DelayedGprWrite {
                index: 2,
                value: 0x5a,
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
            Err(ExecutionError::Exception(Exception::TlbLoad {
                address: u32::MAX,
                fault: TlbFaultKind::Miss,
            }))
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
                Err(ExecutionError::Exception(exception))
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
            Ok(Some(InstructionEffect::DelayedGprWrite {
                index: 2,
                value: 0x7f,
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
                Err(ExecutionError::Exception(exception))
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
                Err(ExecutionError::Exception(exception))
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
            Err(ExecutionError::Exception(Exception::TlbModified {
                address: virtual_address,
            }))
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
            Ok(None)
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
            Err(ExecutionError::Exception(Exception::DataBusError))
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
            Err(ExecutionError::BusFault {
                address: PhysAddr::new(0x100),
                fault: BusFault::UnsupportedAccess,
            })
        );
    }
}
