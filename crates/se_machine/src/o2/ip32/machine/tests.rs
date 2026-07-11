use std::cell::RefCell;
use std::rc::Rc;

use se_core::tracing::{TraceRecord, TraceValue};
use se_device::cpu::mips4::gpr::Mips4GprIndex;

use super::*;

const LUI_R1_LINEAR_RAM: u32 = 0x3c01_4000;
const LUI_R1_CRIME: u32 = 0x3c01_1400;
const ADDIU_R2_1234: u32 = 0x2402_1234;
const SW_R2_R1: u32 = 0xac22_0000;
const LW_R2_R1: u32 = 0x8c22_0000;
const LW_R3_R1: u32 = 0x8c23_0000;
const WAIT: u32 = 0x4200_0020;

fn config_with_program(words: &[(usize, u32)]) -> Ip32MachineConfig {
    let mut config = Ip32MachineConfig {
        ram_size_bytes: 1024 * 1024,
        ..Ip32MachineConfig::default()
    };
    for &(offset, word) in words {
        config.prom_image[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
    }
    config
}

#[test]
fn default_config_matches_the_o2_r5000sc_baseline() {
    let config = Ip32MachineConfig::default();

    assert_eq!(config.processor.endianness, Mips4Endianness::Big);
    assert_eq!(config.processor.revision.bits(), 0x21);
    assert_eq!(config.processor.processor_frequency_hz, 180_000_000);
    assert_eq!(
        config.processor.instruction_cache.size_bytes(),
        Some(32 * 1024)
    );
    assert_eq!(config.processor.data_cache.size_bytes(), Some(32 * 1024));
    assert_eq!(
        config.processor.secondary_cache.size_bytes(),
        Some(512 * 1024)
    );
    assert_eq!(config.ram_size_bytes, 64 * 1024 * 1024);
    assert_eq!(config.prom_image.len(), IP32_PROM_IMAGE_SIZE_BYTES);
    assert_eq!(
        config.unimplemented_access_policy,
        Ip32UnimplementedAccessPolicy::Strict
    );
}

#[test]
fn construction_registers_only_real_board_components() {
    let machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();
    let registry = machine.runtime().registry();

    assert_eq!(registry.len(), 8);
    assert!(registry.get_typed::<R5000Cpu>(component_ids::CPU0).is_ok());
    assert!(
        registry
            .get_typed::<Ip32CpuAddressBus>(component_ids::CPU_SYSAD_BUS)
            .is_ok()
    );
    assert!(registry.get_typed::<Ram>(component_ids::RAM).is_ok());
    assert!(registry.get_typed::<Rom>(component_ids::PROM).is_ok());
    for id in [
        component_ids::CRIME,
        component_ids::MACE,
        component_ids::GBE,
        component_ids::VICE,
    ] {
        assert!(registry.get_typed::<Ip32MmioStub>(id).is_ok());
    }
    for id in [
        component_ids::FPU0,
        component_ids::ICACHE0,
        component_ids::DCACHE0,
        component_ids::SCACHE0,
    ] {
        assert!(!registry.contains(id));
    }
}

#[test]
fn invalid_machine_config_is_rejected_before_component_construction() {
    let mut config = config_with_program(&[]);
    config.ram_size_bytes = 0;
    assert_eq!(
        Ip32Machine::from_config(config).err(),
        Some(Ip32MachineBuildError::InvalidRamSize { size_bytes: 0 })
    );

    let mut config = config_with_program(&[]);
    config.prom_image.pop();
    assert_eq!(
        Ip32Machine::from_config(config).err(),
        Some(Ip32MachineBuildError::InvalidPromSize {
            size_bytes: IP32_PROM_IMAGE_SIZE_BYTES - 1
        })
    );

    let mut config = config_with_program(&[]);
    config.processor.processor_frequency_hz = IP32_TIMEBASE_HZ + 1;
    assert_eq!(
        Ip32Machine::from_config(config).err(),
        Some(Ip32MachineBuildError::InvalidProcessorFrequency {
            frequency_hz: IP32_TIMEBASE_HZ + 1
        })
    );
}

#[test]
fn power_on_executes_prom_and_closes_the_ram_alias_loop() {
    let config = config_with_program(&[
        (0, LUI_R1_LINEAR_RAM),
        (4, ADDIU_R2_1234),
        (8, SW_R2_R1),
        (12, LW_R3_R1),
        (16, WAIT),
    ]);
    let mut machine = Ip32Machine::from_config(config).unwrap();

    machine.schedule_power_on().unwrap();
    let status = machine.run_until_time(SimTime::new(100)).unwrap();

    assert_eq!(status, RunStatus::Idle);
    assert_eq!(machine.runtime().now(), SimTime::new(100));
    assert!(machine.runtime().scheduler().is_empty());
    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(
        cpu.state().gpr().read(Mips4GprIndex::from_u8(3).unwrap()),
        0x1234
    );
    let ram_size = machine
        .runtime()
        .registry()
        .get_typed::<Ram>(component_ids::RAM)
        .unwrap()
        .len() as u64;
    assert_eq!(
        &machine
            .runtime()
            .registry()
            .get_typed::<Ram>(component_ids::RAM)
            .unwrap()
            .bytes()[..4],
        &[0x00, 0x00, 0x12, 0x34]
    );

    let mut low_alias_bus =
        Ip32CpuAddressBus::new(component_ids::CPU_SYSAD_BUS, "alias probe", ram_size);
    let probe = ExecutionTransaction {
        id: ExecutionTransactionId::new(0xfeed),
        payload: Mips4ExecutionTransaction::Read {
            physical_address: 0,
            size: se_device::cpu::mips4::execution::bus::Mips4ExecutionTransferSize::Word,
            kind: se_device::cpu::mips4::execution::bus::Mips4ExecutionAccessKind::DataLoad,
            access_type: se_device::cpu::mips4::cache::Mips4MemoryAccessType::Uncached,
        },
    };
    let route = low_alias_bus.route(probe);
    assert!(matches!(
        route,
        Ip32BusRoute::Memory {
            target: component_ids::RAM,
            offset: 0,
            ..
        }
    ));
    let Ip32BusRoute::Memory { offset, .. } = route else {
        unreachable!();
    };
    assert_eq!(
        machine
            .runtime_mut()
            .registry_mut()
            .get_typed_mut::<Ram>(component_ids::RAM)
            .unwrap()
            .accept(MemoryTransaction::Read { offset, size: 4 }),
        MemoryResponse::ReadData(0x3412_0000)
    );
}

#[test]
fn reset_invalidates_cpu_steps_from_the_previous_generation() {
    let config = config_with_program(&[(0, WAIT)]);
    let mut machine = Ip32Machine::from_config(config).unwrap();

    machine.schedule_power_on().unwrap();
    machine.schedule_reset().unwrap();
    machine.run_until_time(SimTime::new(20)).unwrap();

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().cp0().random().bits(), 46);
    assert!(machine.runtime().scheduler().is_empty());
}

#[test]
fn run_steps_exposes_the_runtime_event_limit() {
    let config = config_with_program(&[(0, WAIT)]);
    let mut machine = Ip32Machine::from_config(config).unwrap();
    machine.schedule_power_on().unwrap();

    assert_eq!(machine.run_steps(1).unwrap(), RunStatus::StepLimitReached);
    assert_eq!(machine.control.generation, 1);
    assert!(!machine.runtime().scheduler().is_empty());
}

#[derive(Clone, Default)]
struct SequenceSink(Rc<RefCell<Vec<u64>>>);

impl TraceSink for SequenceSink {
    fn record(&mut self, record: TraceRecord<'_>) {
        self.0.borrow_mut().push(record.sequence);
    }
}

#[test]
fn hard_reset_dispatches_its_exact_event_and_preserves_trace_sequence() {
    let sink = SequenceSink::default();
    let sequences = Rc::clone(&sink.0);
    let config = config_with_program(&[(0, WAIT)]);
    let mut machine = Ip32Machine::from_config_with_trace_sink(config, sink).unwrap();
    machine.schedule_power_on().unwrap();

    machine.hard_reset().unwrap();

    assert_eq!(machine.runtime().now(), SimTime::ZERO);
    assert_eq!(machine.control.generation, 2);
    let captured = sequences.borrow();
    assert!(!captured.is_empty());
    assert!(
        captured
            .windows(2)
            .all(|pair| pair[1] == pair[0].checked_add(1).unwrap())
    );
}

#[test]
fn processor_clock_accumulator_has_no_long_term_drift() {
    let mut clock = CpuClock::new(180_000_000);
    let delays: Vec<u64> = (0..18).map(|_| clock.next_pclock_delay().get()).collect();

    assert!(delays.iter().all(|delay| matches!(delay, 5 | 6)));
    assert_eq!(delays.iter().sum::<u64>(), 100);
}

#[test]
fn strict_stub_access_raises_a_precise_data_bus_error() {
    let config = config_with_program(&[(0, LUI_R1_CRIME), (4, LW_R2_R1), (0x380, WAIT)]);
    let mut machine = Ip32Machine::from_config(config).unwrap();

    machine.schedule_power_on().unwrap();
    assert_eq!(
        machine.run_until_time(SimTime::new(100)).unwrap(),
        RunStatus::Idle
    );

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().cp0().cause().exception_code(), 7);
}

#[test]
fn permissive_stub_reads_zero_and_execution_continues() {
    let mut config = config_with_program(&[(0, LUI_R1_CRIME), (4, LW_R2_R1), (8, WAIT)]);
    config.unimplemented_access_policy = Ip32UnimplementedAccessPolicy::Permissive;
    let mut machine = Ip32Machine::from_config(config).unwrap();

    machine.schedule_power_on().unwrap();
    assert_eq!(
        machine.run_until_time(SimTime::new(100)).unwrap(),
        RunStatus::Idle
    );

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(
        cpu.state().gpr().read(Mips4GprIndex::from_u8(2).unwrap()),
        0
    );
    assert_eq!(cpu.state().cp0().cause().exception_code(), 0);
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CapturedAccess {
    level: TraceLevel,
    physical_address: Option<u64>,
}

#[derive(Clone, Default)]
struct CapturingSink(Rc<RefCell<Vec<CapturedAccess>>>);

impl TraceSink for CapturingSink {
    fn record(&mut self, record: TraceRecord<'_>) {
        if record.target != "ip32.sysad" || record.event != "access" {
            return;
        }
        let physical_address = record
            .fields
            .iter()
            .find_map(|field| (field.key == "physical_address").then_some(field.value));
        self.0.borrow_mut().push(CapturedAccess {
            level: record.level,
            physical_address: match physical_address {
                Some(TraceValue::Hex64(value)) => Some(value),
                _ => None,
            },
        });
    }
}

#[test]
fn successful_and_stub_bus_accesses_are_traced() {
    let sink = CapturingSink::default();
    let captured = Rc::clone(&sink.0);
    let mut config = config_with_program(&[(0, LUI_R1_CRIME), (4, LW_R2_R1), (0x380, WAIT)]);
    config.unimplemented_access_policy = Ip32UnimplementedAccessPolicy::Strict;
    let mut machine = Ip32Machine::from_config_with_trace_sink(config, sink).unwrap();

    machine.schedule_power_on().unwrap();
    machine.run_until_time(SimTime::new(100)).unwrap();

    let accesses = captured.borrow();
    assert!(accesses.iter().any(|access| {
        access.level == TraceLevel::Trace && access.physical_address == Some(0x1fc0_0000)
    }));
    assert!(accesses.iter().any(|access| {
        access.level == TraceLevel::Warn && access.physical_address == Some(0x1400_0000)
    }));
}

#[test]
fn schedule_power_on_queues_zero_time_event() {
    let mut machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();

    machine.schedule_power_on().unwrap();
    let event = machine.runtime_mut().scheduler_mut().pop_next().unwrap();

    assert_eq!(event.time, SimTime::ZERO);
    assert_eq!(event.target, component_ids::MACHINE);
    assert_eq!(event.payload, Ip32Event::PowerOn);
}

#[test]
fn run_until_time_advances_when_queue_is_empty() {
    let mut machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();

    let status = machine.run_until_time(SimTime::new(42)).unwrap();

    assert_eq!(status, RunStatus::Idle);
    assert_eq!(machine.runtime().now(), SimTime::new(42));
}
