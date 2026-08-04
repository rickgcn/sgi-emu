use std::collections::BTreeMap;

use se_core::role::{BusControllerRole, BusDeviceRole, BusRole};
use se_float::backend::native::NativeFloatBackend;

use crate::cpu::execution::protocol::{ExecutionAction, ExecutionTransaction};
use crate::cpu::mips4::config::{Mips4CacheConfig, Mips4Endianness};
use crate::cpu::mips4::execution::block::{
    Mips4Block, Mips4BlockGuard, Mips4BlockInstruction, Mips4BlockInstructionMetadata,
    Mips4BlockOperation, Mips4BlockRetire, Mips4BlockRuntime, interpret_block_with_runtime,
};
use crate::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};
use crate::cpu::mips4::execution::port::{
    Mips4BlockExecutionResult, Mips4BlockProbe, Mips4BlockSource, Mips4ExecutionPort,
    Mips4ReusableBlockExecution,
};
use crate::cpu::mips4::model::r5000::revision::R5000Revision;

use super::*;

#[derive(Default)]
struct InterpreterPort {
    blocks: Vec<Mips4Block>,
}

impl Mips4ExecutionPort for InterpreterPort {
    type Error = core::convert::Infallible;

    fn probe<R>(
        &mut self,
        key: Mips4BlockKey,
        source: Mips4BlockSource,
        runtime: &R,
    ) -> Mips4BlockProbe
    where
        R: Mips4BlockRuntime + ?Sized,
    {
        let source_matches = self
            .blocks
            .iter()
            .find(|block| block.key() == key)
            .is_some_and(|block| match source {
                Mips4BlockSource::InstructionCache => {
                    !block.guard().lines().is_empty() && block.guard().code_source().is_none()
                }
                Mips4BlockSource::DynamicFetch => {
                    block.guard().lines().is_empty() && block.guard().code_source().is_none()
                }
                Mips4BlockSource::Stable(guard) => block.guard().code_source() == Some(guard),
            });
        let ready = source_matches
            && self
                .blocks
                .iter()
                .find(|block| block.key() == key)
                .is_some_and(|block| runtime.block_guard_valid(block.guard()));
        if ready {
            Mips4BlockProbe::Ready {
                counter_barrier: false,
            }
        } else {
            if source_matches {
                self.blocks.retain(|block| block.key() != key);
            }
            Mips4BlockProbe::Missing
        }
    }

    fn install(&mut self, block: Mips4Block, _source: Mips4BlockSource) -> Result<(), Self::Error> {
        self.blocks.retain(|cached| cached.key() != block.key());
        self.blocks.push(block);
        Ok(())
    }

    fn execute<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
    ) -> Result<Mips4BlockExecutionResult, Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        let block = self
            .blocks
            .iter()
            .find(|block| block.key() == key)
            .expect("the CPU probes or installs before execution");
        let exit = interpret_block_with_runtime(block, frame, runtime);
        Ok(Mips4BlockExecutionResult {
            exit,
            counter_barrier: false,
            operations_executed: frame.operations_executed(),
        })
    }

    fn execute_reusable<R>(
        &mut self,
        key: Mips4BlockKey,
        frame: &mut Mips4BlockFrame,
        runtime: &mut R,
        _counters_dirty: bool,
    ) -> Result<Mips4ReusableBlockExecution, Self::Error>
    where
        R: Mips4BlockRuntime,
    {
        if !matches!(
            self.probe(key, Mips4BlockSource::InstructionCache, runtime),
            Mips4BlockProbe::Ready { .. }
        ) {
            return Ok(Mips4ReusableBlockExecution::Missing);
        }
        self.execute(key, frame, runtime)
            .map(Mips4ReusableBlockExecution::Executed)
    }
}

fn profile() -> R5000Profile {
    R5000Profile::new(
        Mips4Endianness::Big,
        R5000Revision::from_bits(0x21),
        200_000_000,
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::disabled(),
    )
}

fn cpu() -> R5000Cpu<NativeFloatBackend> {
    R5000Cpu::with_float_backend(
        ComponentId::new(7),
        "cpu0",
        profile(),
        R5000BootMode::from_low_bits(0).unwrap(),
        NativeFloatBackend::new(),
    )
    .unwrap()
}

fn big_endian_word(bits: u32) -> u64 {
    let bytes = bits.to_be_bytes();
    u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], 0, 0, 0, 0])
}

fn retire_instruction(cpu: &mut R5000Cpu<NativeFloatBackend>, bits: u32) -> Mips4ExecutionBoundary {
    let ExecutionAction::Transaction(fetch) = cpu.poll().unwrap() else {
        panic!("expected fetch");
    };
    assert_eq!(
        fetch.payload,
        Mips4ExecutionTransaction::Read {
            physical_address: 0x1fc0_0000 + (cpu.state().pc() & 0x0fff),
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::InstructionFetch,
            access_type: crate::cpu::mips4::cache::Mips4MemoryAccessType::Uncached,
        }
    );
    BusControllerRole::complete(
        cpu,
        ExecutionCompletion {
            id: fetch.id,
            payload: Mips4ExecutionCompletion::ReadData(big_endian_word(bits)),
        },
    );
    let ExecutionAction::Boundary(boundary) = cpu.poll().unwrap() else {
        panic!("expected retired boundary");
    };
    boundary
}

fn retire_nop(cpu: &mut R5000Cpu<NativeFloatBackend>) {
    let boundary = retire_instruction(cpu, 0);
    assert!(matches!(boundary, Mips4ExecutionBoundary::Retired { .. }));
}

#[test]
fn component_identity_and_reset_image_are_visible() {
    let mut cpu = cpu();
    assert_eq!(cpu.id(), ComponentId::new(7));
    assert_eq!(cpu.name(), "cpu0");
    assert_eq!(cpu.state().pc(), 0xffff_ffff_bfc0_0000);
    assert_eq!(cpu.state().cp0().processor_id().bits(), 0x2321);
    assert_eq!(cpu.state().cp1().fcr0().bits(), 0x2321);

    retire_nop(&mut cpu);
    cpu.reset();
    assert_eq!(cpu.state().pc(), 0xffff_ffff_bfc0_0000);
}

#[test]
fn performance_statistics_are_cumulative_across_reset() {
    let mut cpu = cpu();
    retire_nop(&mut cpu);
    assert_eq!(
        cpu.statistics(),
        R5000CpuStatistics {
            retired_instructions: 1,
            exceptions: 0,
            transactions: 1,
        }
    );

    cpu.reset();
    assert_eq!(cpu.statistics().retired_instructions, 1);
    assert_eq!(cpu.statistics().transactions, 1);
}

#[test]
fn decode_cache_keys_include_raw_bits_and_do_not_skip_uncached_fetches() {
    let mut cpu = cpu();
    retire_nop(&mut cpu);
    cpu.reset();

    let boundary = retire_instruction(&mut cpu, 0x2408_0001);
    assert!(matches!(boundary, Mips4ExecutionBoundary::Retired { .. }));
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(8).unwrap()),
        1
    );
    assert_eq!(cpu.statistics().transactions, 2);
}

#[test]
fn protocol_boundary_refreshes_the_reusable_block_frame() {
    let mut cpu = cpu();
    let mut engine = InterpreterPort::default();
    let mut bus = FakeBus {
        ram: FakeRam::new(),
    };
    bus.ram.load_word_be(0x1fc0_0000, 0x3c01_a000);
    bus.ram.load_word_be(0x1fc0_0004, 0x8c28_0000);
    bus.ram.load_word_be(0, 0x1234_5678);

    let R5000ExecutionSliceAction::Transaction(fetch) =
        cpu.run_slice(&mut engine, 1).unwrap().action
    else {
        panic!("expected first instruction fetch");
    };
    BusControllerRole::complete(&mut cpu, bus.route(fetch));
    let first = cpu.run_slice(&mut engine, 1).unwrap();
    assert_eq!(first.retired_instructions, 1);

    let R5000ExecutionSliceAction::Transaction(fetch) =
        cpu.run_slice(&mut engine, 1).unwrap().action
    else {
        panic!("expected second instruction fetch");
    };
    BusControllerRole::complete(&mut cpu, bus.route(fetch));
    let R5000ExecutionSliceAction::Transaction(load) =
        cpu.run_slice(&mut engine, 1).unwrap().action
    else {
        panic!("expected data load");
    };
    assert!(cpu.reusable_block_frame.0.is_some());
    BusControllerRole::complete(&mut cpu, bus.route(load));

    let completed = cpu.run_slice(&mut engine, 1).unwrap();

    assert_eq!(completed.retired_instructions, 1);
    let frame = cpu.reusable_block_frame.0.as_ref().unwrap();
    assert_eq!(frame.pc(), cpu.state().pc());
    assert_eq!(frame.next_pc(), cpu.state().next_pc());
    assert_eq!(frame.read_gpr(8), 0x1234_5678);
}

#[test]
fn reusable_slice_ends_after_nonpersistent_fallback_progress() {
    #[derive(Default)]
    struct MissingFallbackPort {
        direct_executions: u64,
        reusable_attempts: u64,
    }

    impl MissingFallbackPort {
        fn block(key: Mips4BlockKey) -> Mips4Block {
            let mut block = Mips4Block::new(key, Mips4BlockGuard::new());
            block
                .push(Mips4BlockInstruction {
                    metadata: Mips4BlockInstructionMetadata {
                        pc: key.pc,
                        instruction: 0,
                        delay_slot_branch_pc: key.delay_slot_branch_pc,
                    },
                    operation: Mips4BlockOperation::NoOperation,
                    retire: Mips4BlockRetire { pc: key.pc },
                })
                .unwrap();
            block.terminate_dispatch().unwrap();
            block
        }
    }

    impl Mips4ExecutionPort for MissingFallbackPort {
        type Error = core::convert::Infallible;

        fn probe<R>(
            &mut self,
            _key: Mips4BlockKey,
            _source: Mips4BlockSource,
            _runtime: &R,
        ) -> Mips4BlockProbe
        where
            R: Mips4BlockRuntime + ?Sized,
        {
            Mips4BlockProbe::Ready {
                counter_barrier: false,
            }
        }

        fn install(
            &mut self,
            _block: Mips4Block,
            _source: Mips4BlockSource,
        ) -> Result<(), Self::Error> {
            Ok(())
        }

        fn execute<R>(
            &mut self,
            key: Mips4BlockKey,
            frame: &mut Mips4BlockFrame,
            runtime: &mut R,
        ) -> Result<Mips4BlockExecutionResult, Self::Error>
        where
            R: Mips4BlockRuntime,
        {
            self.direct_executions += 1;
            let exit = interpret_block_with_runtime(&Self::block(key), frame, runtime);
            Ok(Mips4BlockExecutionResult {
                exit,
                counter_barrier: false,
                operations_executed: frame.operations_executed(),
            })
        }

        fn execute_reusable<R>(
            &mut self,
            key: Mips4BlockKey,
            frame: &mut Mips4BlockFrame,
            runtime: &mut R,
            _counters_dirty: bool,
        ) -> Result<Mips4ReusableBlockExecution, Self::Error>
        where
            R: Mips4BlockRuntime,
        {
            self.reusable_attempts += 1;
            if self.reusable_attempts == 1 {
                return Ok(Mips4ReusableBlockExecution::Missing);
            }
            self.execute(key, frame, runtime)
                .map(Mips4ReusableBlockExecution::Executed)
        }
    }

    let mut cpu = cpu();
    let mut port = MissingFallbackPort::default();

    let slice = cpu.run_reusable_slice(&mut port, 8).unwrap().unwrap();

    assert_eq!(slice.boundaries, 1);
    assert_eq!(port.direct_executions, 1);
    assert_eq!(port.reusable_attempts, 1);
}

#[test]
fn fake_port_reports_architectural_exceptions() {
    let mut cpu = cpu();
    let mut port = InterpreterPort::default();
    let R5000ExecutionSliceAction::Transaction(fetch) = cpu
        .run_slice_with_code_window(&mut port, 1, None)
        .unwrap()
        .action
    else {
        panic!("expected instruction fetch");
    };
    BusControllerRole::complete(
        &mut cpu,
        ExecutionCompletion {
            id: fetch.id,
            payload: Mips4ExecutionCompletion::ReadData(big_endian_word(0x0000_000c)),
        },
    );
    let execution = cpu.run_slice_with_code_window(&mut port, 1, None).unwrap();
    assert!(matches!(
        execution.exception_boundary,
        Some(Mips4ExecutionBoundary::Exception { .. })
    ));
}

#[test]
fn component_state_drops_the_reusable_block_frame() {
    let mut cpu = R5000Cpu::new(
        ComponentId::new(7),
        "cpu0",
        profile(),
        R5000BootMode::from_low_bits(0).unwrap(),
    )
    .unwrap();
    cpu.reusable_block_frame.0 = Some(cpu.executor.target().block_frame(1));

    let state = cpu.save_state();
    cpu.restore_state(state).unwrap();
    assert!(cpu.reusable_block_frame.0.is_none());
}

#[test]
fn component_state_preserves_name_and_rejects_profile_and_invalid_remainder_atomically() {
    let id = ComponentId::new(7);
    let boot_mode = R5000BootMode::from_low_bits(0).unwrap();
    let mut source = R5000Cpu::new(id, "source", profile(), boot_mode).unwrap();
    source.half_pclock_remainder = 1;
    let mut renamed = R5000Cpu::new(id, "target", profile(), boot_mode).unwrap();
    renamed.restore_state(source.save_state()).unwrap();
    assert_eq!(renamed.name(), "target");
    assert_eq!(renamed.half_pclock_remainder, 1);

    let mismatched_profile = R5000Profile::new(
        Mips4Endianness::Big,
        R5000Revision::from_bits(0x21),
        180_000_000,
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::disabled(),
    );
    let mismatched = R5000Cpu::new(id, "source", mismatched_profile, boot_mode)
        .unwrap()
        .save_state();
    let mut target = R5000Cpu::new(id, "target", profile(), boot_mode).unwrap();
    let before = postcard::to_stdvec(&target.save_state()).unwrap();
    assert!(matches!(
        target.restore_state(mismatched),
        Err(ComponentStateError::ConfigurationMismatch { .. })
    ));
    assert_eq!(postcard::to_stdvec(&target.save_state()).unwrap(), before);

    let mut invalid = target.save_state();
    invalid.half_pclock_remainder = 2;
    assert!(matches!(
        target.restore_state(invalid),
        Err(ComponentStateError::InvalidState { .. })
    ));
    assert_eq!(postcard::to_stdvec(&target.save_state()).unwrap(), before);
}

#[test]
fn count_uses_half_pclock_remainder_without_drift() {
    let mut cpu = cpu();
    assert_eq!(cpu.state().cp0().count().bits(), 0);
    retire_nop(&mut cpu);
    assert_eq!(cpu.state().cp0().count().bits(), 0);
    retire_nop(&mut cpu);
    assert_eq!(cpu.state().cp0().count().bits(), 1);

    cpu.advance_pclocks(5);
    assert_eq!(cpu.state().cp0().count().bits(), 3);
    cpu.advance_pclocks(1);
    assert_eq!(cpu.state().cp0().count().bits(), 4);
}

#[test]
fn irq_bus_deliveries_update_external_interrupt_lines() {
    let mut cpu = cpu();
    for input in [
        R5000_IRQ_IP2,
        R5000_IRQ_IP3,
        R5000_IRQ_IP4,
        R5000_IRQ_IP5,
        R5000_IRQ_IP6,
    ] {
        BusDeviceRole::accept(
            &mut cpu,
            IrqDelivery {
                input,
                asserted: true,
            },
        )
        .unwrap();
    }
    assert_eq!(cpu.state().external_interrupts(), 0x7c);
    assert_eq!(cpu.state().cp0().cause().interrupt_pending() & 0x7c, 0x7c);

    BusDeviceRole::accept(
        &mut cpu,
        IrqDelivery {
            input: R5000_IRQ_IP4,
            asserted: false,
        },
    )
    .unwrap();
    assert_eq!(cpu.state().external_interrupts(), 0x6c);
}

#[test]
fn irq_bus_deliveries_reject_non_external_inputs() {
    for input in [IrqInput::new(0), IrqInput::new(1), IrqInput::new(7)] {
        let mut cpu = cpu();
        assert_eq!(
            BusDeviceRole::accept(
                &mut cpu,
                IrqDelivery {
                    input,
                    asserted: true,
                },
            ),
            Err(R5000IrqError::UnsupportedInput(input))
        );
        assert_eq!(cpu.state().external_interrupts(), 0);
    }
}

#[test]
fn soft_reset_preserves_irq_inputs_and_component_reset_clears_them() {
    let mut cpu = cpu();
    BusDeviceRole::accept(
        &mut cpu,
        IrqDelivery {
            input: R5000_IRQ_IP2,
            asserted: true,
        },
    )
    .unwrap();

    BusDeviceRole::accept(&mut cpu, R5000CpuSignal::SoftReset);
    assert_eq!(cpu.state().external_interrupts(), 0x04);

    Component::reset(&mut cpu);
    assert_eq!(cpu.state().external_interrupts(), 0);
}

#[test]
fn bus_device_signals_deliver_r5000_error_level_exceptions() {
    let mut nmi_cpu = cpu();
    BusDeviceRole::accept(&mut nmi_cpu, R5000CpuSignal::NonMaskableInterrupt);
    assert!(matches!(
        nmi_cpu.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException {
            image: crate::cpu::mips4::exception::Mips4ErrorExceptionImage {
                reason: crate::cpu::mips4::exception::Mips4ErrorException::NonMaskableInterrupt,
                ..
            },
            vector: 0xffff_ffff_bfc0_0000,
            ..
        })
    ));

    let mut cache_cpu = cpu();
    let cache_error = crate::cpu::mips4::cp0::Mips4Cp0CacheErr::from_bits(0xb300_1231);
    BusDeviceRole::accept(&mut cache_cpu, R5000CpuSignal::CacheError(cache_error));
    assert!(matches!(
        cache_cpu.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException {
            image: crate::cpu::mips4::exception::Mips4ErrorExceptionImage {
                reason: crate::cpu::mips4::exception::Mips4ErrorException::CacheError,
                cache_error: Some(0xb300_1231),
                ..
            },
            vector: 0xffff_ffff_bfc0_0300,
            ..
        })
    ));
    assert_eq!(cache_cpu.state().cp0().cache_err(), cache_error);

    let mut reset_cpu = cpu();
    assert!(matches!(
        reset_cpu.poll().unwrap(),
        ExecutionAction::Transaction(_)
    ));
    BusDeviceRole::accept(&mut reset_cpu, R5000CpuSignal::SoftReset);
    assert!(matches!(
        reset_cpu.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException {
            image: crate::cpu::mips4::exception::Mips4ErrorExceptionImage {
                reason: crate::cpu::mips4::exception::Mips4ErrorException::SoftReset,
                ..
            },
            vector: 0xffff_ffff_bfc0_0000,
            ..
        })
    ));
}

#[test]
fn wait_idle_does_not_advance_random_or_count() {
    let mut cpu = cpu();
    let boundary = retire_instruction(&mut cpu, 0x4200_0020);
    assert!(matches!(
        boundary,
        Mips4ExecutionBoundary::Retired {
            instruction: 0x4200_0020,
            ..
        }
    ));
    let random = cpu.state().cp0().random().bits();
    let count = cpu.state().cp0().count().bits();

    assert_eq!(cpu.poll().unwrap(), ExecutionAction::Idle);
    assert_eq!(cpu.poll().unwrap(), ExecutionAction::Idle);
    assert_eq!(cpu.state().cp0().random().bits(), random);
    assert_eq!(cpu.state().cp0().count().bits(), count);

    BusDeviceRole::accept(
        &mut cpu,
        IrqDelivery {
            input: R5000_IRQ_IP2,
            asserted: true,
        },
    )
    .unwrap();
    assert!(matches!(
        cpu.poll().unwrap(),
        ExecutionAction::Transaction(_)
    ));
}

struct FakeRam {
    bytes: BTreeMap<u64, u8>,
}

impl FakeRam {
    fn new() -> Self {
        Self {
            bytes: BTreeMap::new(),
        }
    }

    fn load_word_be(&mut self, address: u64, word: u32) {
        for (offset, byte) in word.to_be_bytes().into_iter().enumerate() {
            self.bytes.insert(address + offset as u64, byte);
        }
    }

    fn read_word_be(&self, address: u64) -> u32 {
        u32::from_be_bytes([
            *self.bytes.get(&address).unwrap_or(&0),
            *self.bytes.get(&(address + 1)).unwrap_or(&0),
            *self.bytes.get(&(address + 2)).unwrap_or(&0),
            *self.bytes.get(&(address + 3)).unwrap_or(&0),
        ])
    }
}

impl BusDeviceRole<Mips4ExecutionTransaction> for FakeRam {
    type Response = Mips4ExecutionCompletion;

    fn accept(&mut self, transaction: Mips4ExecutionTransaction) -> Self::Response {
        match transaction {
            Mips4ExecutionTransaction::Read {
                physical_address,
                size,
                ..
            } => {
                let mut data = 0;
                for offset in 0..size.bytes() {
                    data |= u64::from(
                        *self
                            .bytes
                            .get(&(physical_address + u64::from(offset)))
                            .unwrap_or(&0),
                    ) << (offset * 8);
                }
                Mips4ExecutionCompletion::ReadData(data)
            }
            Mips4ExecutionTransaction::Write {
                physical_address,
                size,
                data,
                byte_enable,
                ..
            } => {
                for offset in 0..size.bytes() {
                    if byte_enable & (1 << offset) != 0 {
                        self.bytes.insert(
                            physical_address + u64::from(offset),
                            (data >> (offset * 8)) as u8,
                        );
                    }
                }
                Mips4ExecutionCompletion::WriteComplete
            }
        }
    }
}

struct FakeBus {
    ram: FakeRam,
}

impl BusRole<ExecutionTransaction<Mips4ExecutionTransaction>> for FakeBus {
    type Response = ExecutionCompletion<Mips4ExecutionCompletion>;

    fn route(
        &mut self,
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    ) -> Self::Response {
        ExecutionCompletion {
            id: transaction.id,
            payload: self.ram.accept(transaction.payload),
        }
    }
}

#[test]
fn delayed_bus_completion_keeps_the_cpu_waiting_on_the_same_id() {
    let mut cpu = cpu();
    let mut bus = FakeBus {
        ram: FakeRam::new(),
    };
    bus.ram.load_word_be(0x1fc0_0000, 0);

    let ExecutionAction::Transaction(fetch) = cpu.poll().unwrap() else {
        panic!("expected fetch");
    };
    let ExecutionAction::Waiting { transaction_id } = cpu.poll().unwrap() else {
        panic!("expected wait state");
    };
    assert_eq!(transaction_id, fetch.id);

    let completion = bus.route(fetch);
    BusControllerRole::complete(&mut cpu, completion);
    assert!(matches!(
        cpu.poll().unwrap(),
        ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. })
    ));
}

#[test]
fn hand_written_rom_runs_through_cpu_bus_and_ram_roles() {
    const ROM: [u32; 16] = [
        0x2401_0005,
        0x2402_0007,
        0x0022_1821,
        0x3c04_8000,
        0xac83_0000,
        0x8c85_0000,
        0x10a3_0001,
        0x2406_0001,
        0x3c07_2440,
        0x34e7_0004,
        0x4087_6000,
        0x3c08_3f80,
        0x4488_1000,
        0x4602_1100,
        0x4409_2000,
        0x0000_000d,
    ];

    let mut cpu = cpu();
    let mut bus = FakeBus {
        ram: FakeRam::new(),
    };
    for (index, instruction) in ROM.into_iter().enumerate() {
        bus.ram
            .load_word_be(0x1fc0_0000 + (index as u64 * 4), instruction);
    }

    let mut boundaries = 0;
    loop {
        match cpu.poll().unwrap() {
            ExecutionAction::Transaction(transaction) => {
                let completion = bus.route(transaction);
                BusControllerRole::complete(&mut cpu, completion);
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) => {
                boundaries += 1;
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { image, .. }) => {
                assert_eq!(
                    image.reason,
                    crate::cpu::mips4::exception::Mips4Exception::Breakpoint
                );
                break;
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException { .. }) => {
                panic!("test program unexpectedly entered error level")
            }
            ExecutionAction::Waiting { .. } => panic!("immediate bus must not remain waiting"),
            ExecutionAction::Idle => panic!("test program must not enter standby"),
        }
    }

    assert_eq!(boundaries, 15);
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(3).unwrap()),
        12
    );
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(5).unwrap()),
        12
    );
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(6).unwrap()),
        1
    );
    assert_eq!(
        cpu.state()
            .gpr()
            .read(crate::cpu::mips4::gpr::Mips4GprIndex::from_u8(9).unwrap()),
        2.0f32.to_bits() as u64
    );
    assert_eq!(bus.ram.read_word_be(0), 12);
}

#[test]
fn instruction_tlb_miss_selects_the_boot_refill_vector() {
    const ROM: [u32; 5] = [
        0x2401_0000,
        0x4081_7000,
        0x3c01_0040,
        0x4081_6000,
        0x4200_0018,
    ];

    let mut cpu = cpu();
    let mut bus = FakeBus {
        ram: FakeRam::new(),
    };
    for (index, instruction) in ROM.into_iter().enumerate() {
        bus.ram
            .load_word_be(0x1fc0_0000 + (index as u64 * 4), instruction);
    }

    let mut retired = 0;
    while retired != ROM.len() {
        match cpu.poll().unwrap() {
            ExecutionAction::Transaction(transaction) => {
                let completion = bus.route(transaction);
                BusControllerRole::complete(&mut cpu, completion);
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Retired { .. }) => retired += 1,
            ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { .. }) => {
                panic!("setup instruction unexpectedly trapped");
            }
            ExecutionAction::Boundary(Mips4ExecutionBoundary::ErrorException { .. }) => {
                panic!("setup unexpectedly entered error level");
            }
            ExecutionAction::Waiting { .. } => unreachable!(),
            ExecutionAction::Idle => unreachable!(),
        }
    }

    let ExecutionAction::Boundary(Mips4ExecutionBoundary::Exception { image, vector, .. }) =
        cpu.poll().unwrap()
    else {
        panic!("expected instruction TLB miss");
    };
    assert_eq!(
        image.reason,
        crate::cpu::mips4::exception::Mips4Exception::TlbLoad
    );
    assert_eq!(image.bad_virtual_address, Some(0));
    assert_eq!(vector, 0xffff_ffff_bfc0_0200);
}
