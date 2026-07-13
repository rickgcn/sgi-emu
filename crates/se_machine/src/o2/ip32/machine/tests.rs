use se_core::role::BusDeviceRole;
use se_core::tracing::{TraceInterest, TraceRecord, TraceSink, TraceSource, TraceValue};
use se_device::bus::irq::{IrqBus, IrqTransaction};
use se_device::bus::media::{MediaPayload, MediaPort};
use se_device::bus::one_wire::OneWireBus;
use se_device::chipset::crime::config::{CrimeAccessPolicy, CrimeConfigError, CrimeSdramBankSize};
use se_device::chipset::crime::iou::{CrimeCgiBus, CrimeCmiBus};
use se_device::chipset::crime::memory::CrimeSdram;
use se_device::chipset::crime::memory::bus::CrimeMemoryBus;
use se_device::chipset::crime::protocol::{
    CrimeCompletionPayload, CrimeMemoryBankSelect, CrimeMemoryClient, CrimeMemoryTransaction,
    CrimeTransactionId, CrimeTransfer,
};
use se_device::chipset::crime::registers;
use se_device::chipset::gbe::Gbe;
use se_device::chipset::mace::Mace;
use se_device::cpu::mips4::gpr::Mips4GprIndex;
use se_device::memory::ds2502::Ds2502;
use se_device::memory::flash::ReadArrayFlash;
use se_device::serial::uart16550::Uart16550;
use std::time::{Duration, Instant};

use super::*;

const LUI_R1_LINEAR_RAM: u32 = 0x3c01_4000;
const LUI_R1_CRIME: u32 = 0x3c01_1400;
const LUI_R1_MACE_ISA: u32 = 0x3c01_1f3a;
const ORI_R1_RTC_37: u32 = 0x3421_3707;
const LBU_R3_R1: u32 = 0x9023_0000;
const ADDIU_R2_1234: u32 = 0x2402_1234;
const ADDIU_R2_1: u32 = 0x2402_0001;
const ADDIU_R2_SOFT_RESET: u32 = 0x2402_0400;
const SW_R2_R1: u32 = 0xac22_0000;
const LW_R3_R1: u32 = 0x8c23_0000;
const LW_R3_R1_LOW_WORD: u32 = 0x8c23_0004;
const SD_R2_R1_CONTROL: u32 = 0xfc22_0008;
const SD_R2_R1_INTERRUPT_ENABLE: u32 = 0xfc22_0018;
const SD_R2_R1_SOFTWARE_INTERRUPT: u32 = 0xfc22_0020;
const WAIT: u32 = 0x4200_0020;

#[derive(Default, serde::Deserialize, serde::Serialize)]
struct SchedulerCaptureSink {
    records: Vec<(u64, SimTime, String)>,
}

impl TraceSink for SchedulerCaptureSink {
    fn interest(&self, source: TraceSource) -> TraceInterest {
        if matches!(source, TraceSource::Scheduler) {
            TraceInterest::All
        } else {
            TraceInterest::None
        }
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        self.records
            .push((record.sequence, record.time, record.event.to_owned()));
    }
}

#[derive(Default)]
struct ComponentCaptureSink {
    records: Vec<(u64, SimTime, TraceSource, String, String)>,
}

impl TraceSink for ComponentCaptureSink {
    fn interest(&self, source: TraceSource) -> TraceInterest {
        if matches!(source, TraceSource::Component(_)) {
            TraceInterest::All
        } else {
            TraceInterest::None
        }
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        self.records.push((
            record.sequence,
            record.time,
            record.source,
            record.target.to_owned(),
            record.event.to_owned(),
        ));
    }
}

fn assert_machine_architecture_equal<A, B>(reference: &Ip32Machine<A>, optimized: &Ip32Machine<B>) {
    macro_rules! assert_component_eq {
        ($type:ty, $id:expr) => {
            assert_eq!(
                reference
                    .runtime()
                    .registry()
                    .get_typed::<$type>($id)
                    .unwrap(),
                optimized
                    .runtime()
                    .registry()
                    .get_typed::<$type>($id)
                    .unwrap()
            );
        };
    }

    assert_eq!(reference.runtime().now(), optimized.runtime().now());
    assert_eq!(
        reference.runtime().scheduler().peek_next_time(),
        optimized.runtime().scheduler().peek_next_time()
    );
    assert_eq!(
        reference
            .runtime()
            .registry()
            .get_typed::<R5000Cpu>(component_ids::CPU0)
            .unwrap()
            .state(),
        optimized
            .runtime()
            .registry()
            .get_typed::<R5000Cpu>(component_ids::CPU0)
            .unwrap()
            .state()
    );
    assert_component_eq!(Ip32SysAdBus, component_ids::CPU_SYSAD_BUS);
    assert_component_eq!(CrimeMemoryBus, component_ids::CRIME_MEMORY_DOMAIN);
    assert_component_eq!(CrimeCmiBus, component_ids::CRIME_MACE_LINK);
    assert_component_eq!(CrimeCgiBus, component_ids::CRIME_GBE_LINK);
    assert_component_eq!(IsaBus, component_ids::ISA_BUS);
    assert_component_eq!(CrimeSdram, component_ids::RAM);
    assert_component_eq!(Crime, component_ids::CRIME);
    assert_component_eq!(Mace, component_ids::MACE);
    assert_component_eq!(IrqBus, component_ids::CPU_IRQ_BUS);
    assert_component_eq!(IrqBus, component_ids::MACE_IRQ_BUS);
    assert_component_eq!(OneWireBus, component_ids::ONE_WIRE_BUS);
    assert_component_eq!(Ds2502, component_ids::NIC_IDENTITY);
    assert_eq!(
        reference.control.cpu_generation,
        optimized.control.cpu_generation
    );
    assert_eq!(reference.control.cpu_clock, optimized.control.cpu_clock);
    assert_eq!(
        reference.control.host_generation,
        optimized.control.host_generation
    );
    assert_eq!(
        reference.control.host_reservations,
        optimized.control.host_reservations
    );
    assert_eq!(
        reference.control.host_outputs,
        optimized.control.host_outputs
    );
    assert_eq!(
        reference.control.host_output_units,
        optimized.control.host_output_units
    );
    assert_eq!(
        reference.control.host_dropped_output_bytes,
        optimized.control.host_dropped_output_bytes
    );
}

#[test]
fn isa_post_delivery_drains_only_the_addressed_peripheral() {
    assert_eq!(
        isa_post_delivery(component_ids::PROM),
        IsaPostDelivery::None
    );
    assert_eq!(isa_post_delivery(component_ids::RTC), IsaPostDelivery::Rtc);
    assert_eq!(
        isa_post_delivery(component_ids::SERIAL0),
        IsaPostDelivery::Serial(component_ids::SERIAL0)
    );
    assert_eq!(
        isa_post_delivery(component_ids::SERIAL1),
        IsaPostDelivery::Serial(component_ids::SERIAL1)
    );
    assert_eq!(
        isa_post_delivery(component_ids::PARALLEL_PORT),
        IsaPostDelivery::Parallel
    );
}

const fn i_type(opcode: u8, rs: u8, rt: u8, immediate: u16) -> u32 {
    (opcode as u32) << 26 | (rs as u32) << 21 | (rt as u32) << 16 | immediate as u32
}

const fn r_type(rs: u8, rt: u8, rd: u8, shift: u8, function: u8) -> u32 {
    (rs as u32) << 21
        | (rt as u32) << 16
        | (rd as u32) << 11
        | (shift as u32) << 6
        | function as u32
}

fn config_with_program(words: &[(usize, u32)]) -> Ip32MachineConfig {
    let mut config = Ip32MachineConfig::default();
    for &(offset, word) in words {
        config.prom_image[offset..offset + 4].copy_from_slice(&word.to_be_bytes());
    }
    config
}

fn drive_crime_irq(machine: &mut Ip32Machine, asserted: bool) {
    let registry = machine.runtime_mut().registry_mut();
    registry
        .get_typed_mut::<IrqBus>(component_ids::CPU_IRQ_BUS)
        .unwrap()
        .route(IrqTransaction {
            source: IrqSource {
                component: component_ids::CRIME,
                output: CRIME_IRQ_OUTPUT,
            },
            asserted,
        })
        .unwrap();
    drain_irq_bus(registry).unwrap();
}

#[test]
fn default_config_matches_the_o2_r5000sc_and_crime_baseline() {
    let config = Ip32MachineConfig::default();

    assert_eq!(config.processor.endianness, Mips4Endianness::Big);
    assert_eq!(config.processor.revision.bits(), 0x21);
    assert_eq!(config.processor.processor_frequency_hz, 180_000_000);
    assert_eq!(config.crime.memory.total_size_bytes(), 64 * 1024 * 1024);
    assert_eq!(
        config.crime.memory.banks[0].unwrap().size,
        CrimeSdramBankSize::MiB32
    );
    assert_eq!(
        config.crime.unimplemented_access_policy,
        CrimeAccessPolicy::Strict
    );
    assert_eq!(config.prom_image.len(), IP32_PROM_IMAGE_SIZE_BYTES);
}

#[test]
fn host_input_reservations_enforce_configured_capacity() {
    let mut config = Ip32MachineConfig::default();
    config.mace.ports.byte_stream_bytes = 1;
    let mut machine = Ip32Machine::from_config(config).unwrap();
    let input = Ip32HostInput {
        port: MediaPort::Keyboard,
        payload: MediaPayload::Bytes(vec![0xaa]),
    };
    machine
        .schedule_host_input(SimTime::new(1), input.clone())
        .unwrap();
    assert_eq!(
        machine.schedule_host_input(SimTime::new(2), input),
        Err(Ip32HostInputError::QueueFull(MediaPort::Keyboard))
    );
}

#[test]
fn serial_host_input_routes_only_to_the_selected_uart() {
    let mut machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();
    machine
        .schedule_host_input(
            SimTime::ZERO,
            Ip32HostInput {
                port: MediaPort::Serial0,
                payload: MediaPayload::Bytes(vec![0x41]),
            },
        )
        .unwrap();

    let _ = machine.run_steps(1).unwrap();

    let registry = machine.runtime().registry();
    assert_eq!(
        registry
            .get_typed::<Uart16550>(component_ids::SERIAL0)
            .unwrap()
            .external_receive_len(),
        1
    );
    assert_eq!(
        registry
            .get_typed::<Uart16550>(component_ids::SERIAL1)
            .unwrap()
            .external_receive_len(),
        0
    );
}

#[test]
fn hard_reset_invalidates_future_serial_host_input() {
    let mut machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();
    machine
        .schedule_host_input(
            SimTime::new(100),
            Ip32HostInput {
                port: MediaPort::Serial1,
                payload: MediaPayload::Bytes(vec![0x42]),
            },
        )
        .unwrap();

    machine.hard_reset().unwrap();
    let _ = machine.run_until_time(SimTime::new(100)).unwrap();

    assert_eq!(
        machine
            .runtime()
            .registry()
            .get_typed::<Uart16550>(component_ids::SERIAL1)
            .unwrap()
            .external_receive_len(),
        0
    );
}

#[test]
fn synthetic_prom_transmits_through_uart_and_media_bus() {
    let config = config_with_program(&[
        (0, i_type(0x0f, 0, 1, 0xbf39)),
        (4, i_type(0x0d, 1, 1, 0x0007)),
        (8, i_type(0x09, 0, 2, 0x0080)),
        (12, i_type(0x28, 1, 2, 0x0300)),
        (16, i_type(0x09, 0, 2, 48)),
        (20, i_type(0x28, 1, 2, 0x0000)),
        (24, i_type(0x28, 1, 0, 0x0100)),
        (28, i_type(0x09, 0, 2, 3)),
        (32, i_type(0x28, 1, 2, 0x0700)),
        (36, i_type(0x09, 0, 2, 3)),
        (40, i_type(0x28, 1, 2, 0x0300)),
        (44, i_type(0x09, 0, 2, u16::from(b'>'))),
        (48, i_type(0x28, 1, 2, 0x0000)),
        (52, WAIT),
    ]);
    let mut machine = Ip32Machine::from_config(config).unwrap();
    machine.schedule_power_on().unwrap();

    let _ = machine.run_until_time(SimTime::new(2_000_000)).unwrap();

    assert_eq!(
        machine.poll_host_output(),
        Some(Ip32HostOutput {
            port: MediaPort::Serial0,
            payload: MediaPayload::Bytes(vec![b'>']),
        })
    );
}

#[test]
fn construction_registers_the_role_oriented_ip32_topology() {
    let machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();
    let registry = machine.runtime().registry();

    assert_eq!(registry.len(), 25);
    assert!(registry.get_typed::<R5000Cpu>(component_ids::CPU0).is_ok());
    assert!(
        registry
            .get_typed::<IrqBus>(component_ids::CPU_IRQ_BUS)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<Ip32SysAdBus>(component_ids::CPU_SYSAD_BUS)
            .is_ok()
    );
    assert!(registry.get_typed::<Crime>(component_ids::CRIME).is_ok());
    assert!(
        registry
            .get_typed::<CrimeMemoryBus>(component_ids::CRIME_MEMORY_DOMAIN)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<CrimeCmiBus>(component_ids::CRIME_MACE_LINK)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<CrimeCgiBus>(component_ids::CRIME_GBE_LINK)
            .is_ok()
    );
    assert!(registry.get_typed::<CrimeSdram>(component_ids::RAM).is_ok());
    assert!(registry.get_typed::<Mace>(component_ids::MACE).is_ok());
    assert!(
        registry
            .get_typed::<OneWireBus>(component_ids::ONE_WIRE_BUS)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<Ds2502>(component_ids::NIC_IDENTITY)
            .is_ok()
    );
    assert!(registry.get_typed::<Gbe>(component_ids::GBE).is_ok());
    assert!(
        registry
            .get_typed::<Ip32StubEndpoint>(component_ids::VICE)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<ReadArrayFlash>(component_ids::PROM)
            .is_ok()
    );
}

#[test]
fn crime_irq_output_reaches_r5000_ip2_only_through_the_irq_bus() {
    let mut machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();

    drive_crime_irq(&mut machine, true);
    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().external_interrupts(), 0x04);
    assert_eq!(cpu.state().cp0().cause().interrupt_pending() & 0x04, 0x04);

    drive_crime_irq(&mut machine, false);
    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().external_interrupts(), 0);
    assert_eq!(cpu.state().cp0().cause().interrupt_pending() & 0x04, 0);
}

#[test]
fn hard_reset_clears_cpu_and_irq_bus_levels() {
    let mut machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();
    drive_crime_irq(&mut machine, true);

    machine.schedule_power_on().unwrap();
    let _ = machine.run_steps(1).unwrap();
    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().external_interrupts(), 0);

    drive_crime_irq(&mut machine, true);

    machine.hard_reset().unwrap();

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().external_interrupts(), 0);
    drive_crime_irq(&mut machine, true);
    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().external_interrupts(), 0x04);
}

#[test]
fn warm_reset_preserves_the_routed_crime_irq_level() {
    let config = config_with_program(&[
        (0, LUI_R1_CRIME),
        (4, ADDIU_R2_1),
        (8, SD_R2_R1_INTERRUPT_ENABLE),
        (12, SD_R2_R1_SOFTWARE_INTERRUPT),
        (16, ADDIU_R2_SOFT_RESET),
        (20, SD_R2_R1_CONTROL),
        (24, WAIT),
    ]);
    let mut machine = Ip32Machine::from_config(config).unwrap();
    machine.schedule_power_on().unwrap();

    let _ = machine.run_steps(100).unwrap();

    assert!(machine.control.cpu_generation > 1);
    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(cpu.state().external_interrupts(), 0x04);
}

#[test]
fn invalid_machine_configuration_is_rejected_before_construction() {
    let mut config = config_with_program(&[]);
    config.crime.memory.banks[0] = None;
    assert_eq!(
        Ip32Machine::from_config(config).err(),
        Some(Ip32MachineBuildError::Crime(CrimeError::Configuration(
            CrimeConfigError::MissingBankZero
        )))
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
fn cpu_prom_and_ram_accesses_cross_all_required_buses() {
    let config = config_with_program(&[
        (0, LUI_R1_LINEAR_RAM),
        (4, ADDIU_R2_1234),
        (8, SW_R2_R1),
        (12, LW_R3_R1),
        (16, WAIT),
    ]);
    let mut machine = Ip32Machine::from_config(config).unwrap();

    machine.schedule_power_on().unwrap();
    assert_eq!(
        machine.run_until_time(SimTime::new(1_000_000)).unwrap(),
        RunStatus::Idle
    );

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(
        cpu.state().gpr().read(Mips4GprIndex::from_u8(3).unwrap()),
        0x1234
    );

    let completion = machine
        .runtime_mut()
        .registry_mut()
        .get_typed_mut::<CrimeSdram>(component_ids::RAM)
        .unwrap()
        .accept(CrimeMemoryTransaction {
            id: CrimeTransactionId::new(0x100),
            time: SimTime::new(1_000_000),
            controller: component_ids::CRIME,
            client: CrimeMemoryClient::Cpu,
            address: 0,
            bank_select: CrimeMemoryBankSelect::Decode,
            no_ecc: false,
            transfer: CrimeTransfer::read(4),
        });
    assert_eq!(
        completion.result.unwrap().payload,
        CrimeCompletionPayload::ReadData(vec![0, 0, 0x12, 0x34].into())
    );
}

#[test]
fn inline_cpu_continuation_matches_single_step_reference_execution() {
    let config = config_with_program(&[
        (0, ADDIU_R2_1),
        (4, i_type(0x09, 2, 2, 1)),
        (8, LUI_R1_LINEAR_RAM),
        (12, SW_R2_R1),
        (16, LW_R3_R1),
        (20, WAIT),
    ]);
    let mut reference = Ip32Machine::from_config(config.clone()).unwrap();
    reference.control.cpu_continuation_quantum = 1;
    reference.control.inline_sysad_completion = false;
    reference.control.event_chain_policy = Ip32EventChainPolicy::disabled();
    let mut optimized = Ip32Machine::from_config(config).unwrap();
    reference.schedule_power_on().unwrap();
    optimized.schedule_power_on().unwrap();

    assert_eq!(
        reference.run_until_time(SimTime::new(1_000_000)).unwrap(),
        RunStatus::Idle
    );
    assert_eq!(
        optimized.run_until_time(SimTime::new(1_000_000)).unwrap(),
        RunStatus::Idle
    );
    assert_machine_architecture_equal(&reference, &optimized);
    assert_eq!(
        reference.control.logical_transitions,
        optimized.control.logical_transitions
    );
    assert_eq!(reference.poll_host_output(), optimized.poll_host_output());
}

#[test]
fn fusion_budget_materializes_the_next_logical_transition() {
    let config = config_with_program(&[(0, ADDIU_R2_1), (4, WAIT)]);
    let mut machine = Ip32Machine::from_config(config).unwrap();
    machine.control.event_chain_policy = Ip32EventChainPolicy {
        budget: 1,
        ..Ip32EventChainPolicy::all()
    };
    machine.schedule_power_on().unwrap();

    assert_eq!(machine.run_steps(2).unwrap(), RunStatus::StepLimitReached);
    assert_eq!(machine.control.logical_transitions.len(), 1);
    assert!(machine.runtime().scheduler().peek_next_time().is_some());
    assert_eq!(
        machine.run_until_time(SimTime::new(1_000_000)).unwrap(),
        RunStatus::Idle
    );
}

#[test]
fn fusion_respects_deadlines_and_same_time_global_events() {
    for deadline in [119_999, 120_000, 120_014, 120_015, 120_016, 120_030] {
        let config = config_with_program(&[(0, ADDIU_R2_1), (4, WAIT)]);
        let mut reference = Ip32Machine::from_config(config.clone()).unwrap();
        reference.control.event_chain_policy = Ip32EventChainPolicy::disabled();
        let mut optimized = Ip32Machine::from_config(config).unwrap();
        reference.schedule_power_on().unwrap();
        optimized.schedule_power_on().unwrap();

        reference.run_until_time(SimTime::new(deadline)).unwrap();
        optimized.run_until_time(SimTime::new(deadline)).unwrap();
        assert_machine_architecture_equal(&reference, &optimized);
        assert_eq!(
            reference.control.logical_transitions,
            optimized.control.logical_transitions
        );
    }

    let config = config_with_program(&[(0, ADDIU_R2_1), (4, WAIT)]);
    let mut reference = Ip32Machine::from_config(config.clone()).unwrap();
    reference.control.event_chain_policy = Ip32EventChainPolicy::disabled();
    let mut optimized = Ip32Machine::from_config(config).unwrap();
    for machine in [&mut reference, &mut optimized] {
        machine.schedule_power_on().unwrap();
        machine
            .schedule_host_input(
                SimTime::new(120_015),
                Ip32HostInput {
                    port: MediaPort::Serial0,
                    payload: MediaPayload::Bytes(vec![0x41]),
                },
            )
            .unwrap();
        machine.run_until_time(SimTime::new(120_030)).unwrap();
    }
    assert_machine_architecture_equal(&reference, &optimized);
    assert_eq!(
        reference.control.logical_transitions,
        optimized.control.logical_transitions
    );
}

#[test]
fn scheduler_capture_forces_reference_dispatch_and_component_trace_order_is_preserved() {
    let config = config_with_program(&[
        (0, LUI_R1_LINEAR_RAM),
        (4, ADDIU_R2_1234),
        (8, SW_R2_R1),
        (12, LW_R3_R1),
        (16, WAIT),
    ]);
    let mut scheduler_reference =
        Ip32Machine::from_config_with_trace_sink(config.clone(), SchedulerCaptureSink::default())
            .unwrap();
    scheduler_reference.control.event_chain_policy = Ip32EventChainPolicy::disabled();
    let mut scheduler_optimized =
        Ip32Machine::from_config_with_trace_sink(config.clone(), SchedulerCaptureSink::default())
            .unwrap();
    scheduler_reference.schedule_power_on().unwrap();
    scheduler_optimized.schedule_power_on().unwrap();
    scheduler_reference
        .run_until_time(SimTime::new(1_000_000))
        .unwrap();
    scheduler_optimized
        .run_until_time(SimTime::new(1_000_000))
        .unwrap();

    assert_machine_architecture_equal(&scheduler_reference, &scheduler_optimized);
    assert_eq!(
        scheduler_reference.runtime().statistics(),
        scheduler_optimized.runtime().statistics()
    );
    assert_eq!(
        scheduler_reference
            .runtime()
            .trace_recorder()
            .sink()
            .records,
        scheduler_optimized
            .runtime()
            .trace_recorder()
            .sink()
            .records
    );

    let mut component_reference =
        Ip32Machine::from_config_with_trace_sink(config.clone(), ComponentCaptureSink::default())
            .unwrap();
    component_reference.control.event_chain_policy = Ip32EventChainPolicy::disabled();
    let mut component_optimized =
        Ip32Machine::from_config_with_trace_sink(config, ComponentCaptureSink::default()).unwrap();
    component_reference.schedule_power_on().unwrap();
    component_optimized.schedule_power_on().unwrap();
    component_reference
        .run_until_time(SimTime::new(1_000_000))
        .unwrap();
    component_optimized
        .run_until_time(SimTime::new(1_000_000))
        .unwrap();

    assert_machine_architecture_equal(&component_reference, &component_optimized);
    assert_eq!(
        component_reference
            .runtime()
            .trace_recorder()
            .sink()
            .records,
        component_optimized
            .runtime()
            .trace_recorder()
            .sink()
            .records
    );
}

#[test]
fn synthetic_prom_clears_linear_a_range_through_the_render_memory_client() {
    let lui = |rt, immediate| i_type(0x0f, 0, rt, immediate);
    let ori = |rt, rs, immediate| i_type(0x0d, rs, rt, immediate);
    let addiu = |rt, rs, immediate| i_type(0x09, rs, rt, immediate);
    let sw = |rt, offset, base| i_type(0x2b, base, rt, offset);
    let lw = |rt, offset, base| i_type(0x23, base, rt, offset);
    let sd = |rt, offset, base| i_type(0x3f, base, rt, offset);
    let program = [
        lui(1, 0x1500),
        lui(3, 0x8000),
        r_type(0, 3, 3, 0, 0x3c),
        lui(4, 0x8000),
        r_type(0, 4, 4, 0, 0x3c),
        r_type(0, 4, 4, 0, 0x3e),
        ori(4, 4, 1),
        r_type(3, 4, 3, 0, 0x25),
        sd(3, 0x1700, 1),
        lui(5, 0x4000),
        lui(6, 0xa5a5),
        ori(6, 6, 0xa5a5),
        sw(6, 0x0fec, 5),
        sw(6, 0x0ff0, 5),
        sw(6, 0x1000, 5),
        sw(6, 0x1010, 5),
        sw(6, 0x1014, 5),
        addiu(2, 0, 0xffff),
        sw(2, 0x3008, 1),
        sw(0, 0x3018, 1),
        ori(2, 0, 0x0ff0),
        sw(2, 0x3030, 1),
        ori(2, 0, 0x1010),
        sw(2, 0x3038, 1),
        ori(2, 0, 0x0011),
        sw(2, 0x3800, 1),
        lui(8, 0x1000),
        lw(7, 0x4000, 1),
        r_type(7, 8, 9, 0, 0x24),
        i_type(0x04, 9, 0, 0xfffd),
        0,
        WAIT,
    ];
    let words = program
        .into_iter()
        .enumerate()
        .map(|(index, word)| (index * 4, word))
        .collect::<Vec<_>>();
    let mut machine = Ip32Machine::from_config(config_with_program(&words)).unwrap();

    machine.schedule_power_on().unwrap();
    assert_eq!(
        machine.run_until_time(SimTime::new(1_000_000)).unwrap(),
        RunStatus::Idle
    );

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_ne!(
        cpu.state().gpr().read(Mips4GprIndex::from_u8(7).unwrap()) & 0x1000_0000,
        0
    );

    let now = machine.runtime().now();
    let completion = machine
        .runtime_mut()
        .registry_mut()
        .get_typed_mut::<CrimeSdram>(component_ids::RAM)
        .unwrap()
        .accept(CrimeMemoryTransaction {
            id: CrimeTransactionId::new(0x102),
            time: now,
            controller: component_ids::CRIME,
            client: CrimeMemoryClient::Cpu,
            address: 0x0fec,
            bank_select: CrimeMemoryBankSelect::Decode,
            no_ecc: false,
            transfer: CrimeTransfer::read(44),
        });
    let CrimeCompletionPayload::ReadData(data) = completion.result.unwrap().payload else {
        panic!("expected SDRAM read data");
    };
    assert_eq!(&data[..4], &[0xa5; 4]);
    assert_eq!(&data[4..=36], &[0; 33]);
    assert_eq!(&data[37..], &[0xa5; 7]);
}

#[test]
fn dallas_nvram_access_crosses_cmi_and_isa() {
    let mut config = config_with_program(&[
        (0, LUI_R1_MACE_ISA),
        (4, ORI_R1_RTC_37),
        (8, LBU_R3_R1),
        (12, WAIT),
    ]);
    config.rtc.nvram[0x37] = 0xa5;
    let mut machine = Ip32Machine::from_config(config).unwrap();

    machine.schedule_power_on().unwrap();
    let _ = machine.run_steps(100).unwrap();

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(
        cpu.state().gpr().read(Mips4GprIndex::from_u8(3).unwrap()),
        0xa5
    );
}

#[test]
fn crime_piu_read_supports_prom_word_lane_selection_through_the_cpu_path() {
    let config = config_with_program(&[(0, LUI_R1_CRIME), (4, LW_R3_R1_LOW_WORD), (8, WAIT)]);
    let mut machine = Ip32Machine::from_config(config).unwrap();

    machine.schedule_power_on().unwrap();
    machine.run_until_time(SimTime::new(1_000_000)).unwrap();

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(
        cpu.state().gpr().read(Mips4GprIndex::from_u8(3).unwrap()),
        0xa1
    );
}

#[test]
fn hard_reset_preserves_sdram_and_advances_the_cpu_generation() {
    let config = config_with_program(&[
        (0, LUI_R1_LINEAR_RAM),
        (4, ADDIU_R2_1234),
        (8, SW_R2_R1),
        (12, WAIT),
    ]);
    let mut machine = Ip32Machine::from_config(config).unwrap();
    machine.schedule_power_on().unwrap();
    machine.run_until_time(SimTime::new(1_000_000)).unwrap();
    let generation = machine.control.cpu_generation;

    machine.hard_reset().unwrap();

    assert_eq!(machine.control.cpu_generation, generation + 1);
    let now = machine.runtime().now();
    let completion = machine
        .runtime_mut()
        .registry_mut()
        .get_typed_mut::<CrimeSdram>(component_ids::RAM)
        .unwrap()
        .accept(CrimeMemoryTransaction {
            id: CrimeTransactionId::new(0x101),
            time: now,
            controller: component_ids::CRIME,
            client: CrimeMemoryClient::Cpu,
            address: 0,
            bank_select: CrimeMemoryBankSelect::Decode,
            no_ecc: false,
            transfer: CrimeTransfer::read(4),
        });
    assert_eq!(
        completion.result.unwrap().payload,
        CrimeCompletionPayload::ReadData(vec![0, 0, 0x12, 0x34].into())
    );
}

#[derive(Default, serde::Deserialize, serde::Serialize)]
struct PromAcceptanceSink {
    failed_addresses: Vec<u64>,
}

impl TraceSink for PromAcceptanceSink {
    fn interest(&self, source: TraceSource) -> TraceInterest {
        if matches!(source, TraceSource::Scheduler) {
            TraceInterest::None
        } else {
            TraceInterest::Filtered
        }
    }

    fn enabled(
        &self,
        _source: TraceSource,
        _level: se_core::tracing::TraceLevel,
        target: &str,
        event: &str,
    ) -> bool {
        target == "ip32.sysad" && event == "access"
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        if record.target != "ip32.sysad" || record.event != "access" {
            return;
        }
        let failed = record
            .fields
            .iter()
            .any(|field| field.key == "bus_error" && matches!(field.value, TraceValue::Bool(true)));
        if !failed {
            return;
        }
        if let Some(address) = record
            .fields
            .iter()
            .find_map(|field| (field.key == "physical_address").then_some(field.value))
            && let TraceValue::Hex64(address) = address
        {
            self.failed_addresses.push(address);
        }
    }
}

#[test]
#[ignore = "requires a local proprietary IP32 PROM image"]
fn local_ip32_prom_reaches_only_an_explicit_unimplemented_boundary() {
    let path = std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name a local image");
    let mut config = Ip32MachineConfig {
        prom_image: std::fs::read(path).expect("the local PROM image must be readable"),
        ..Ip32MachineConfig::default()
    };
    config.crime.unimplemented_access_policy = CrimeAccessPolicy::Strict;
    let mut machine =
        Ip32Machine::from_config_with_trace_sink(config, PromAcceptanceSink::default()).unwrap();
    machine.schedule_power_on().unwrap();
    let max_events = std::env::var("IP32_PROM_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(200_000);
    let _ = machine.run_steps(max_events).unwrap();

    let mut terminal = Vec::new();
    while let Some(output) = machine.poll_serial_output() {
        if output.port == Ip32SerialPort::Serial1 {
            terminal.extend(output.bytes);
        }
    }
    let terminal = String::from_utf8_lossy(&terminal);
    assert!(
        !terminal.contains("ds2502_read_rom failed"),
        "the PROM could not read the configured DS2502 identity"
    );
    assert!(
        !terminal.contains("PANIC: Unexpected exception"),
        "the PROM entered its unexpected-exception panic path"
    );

    let pc = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap()
        .state()
        .pc();
    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let epc = cpu.state().cp0().epc().bits();
    let exception_code = cpu.state().cp0().cause().exception_code();
    let failed = &machine.runtime().trace_recorder().sink().failed_addresses;
    let exception_loop = matches!(
        pc,
        0xffff_ffff_bfc0_0380
            | 0xffff_ffff_bfc0_0384
            | 0xffff_ffff_bfc0_0388
            | 0xffff_ffff_bfc0_038c
            | 0xffff_ffff_bfc0_03a0
            | 0xffff_ffff_bfc0_03a4
    );
    assert!(
        !exception_loop,
        "PROM remained in the exception loop at {pc:#018x}; EPC={epc:#018x}, exception={exception_code}, failed accesses: {failed:#x?}"
    );
    assert!(
        !failed.contains(&0x5000_0000),
        "PROM bank probing incorrectly received a SysAD bus error at 0x50000000"
    );
    assert!(
        !failed.contains(&registers::CPU_RESERVED_WRITE_SINK),
        "the PROM-compatible CRIME reserved write sink returned a SysAD bus error"
    );

    assert!(
        failed.iter().all(
            |address| !(registers::CRIME_BASE..registers::CRIME_REGISTER_END).contains(address)
        ),
        "a modeled CRIME access returned a bus error"
    );
}

#[test]
#[ignore = "requires a local proprietary IP32 PROM image"]
fn local_ip32_prom_core_throughput_probe() {
    #[derive(Clone, Copy, serde::Deserialize, serde::Serialize)]
    enum Limit {
        Events(usize),
        SimTime(SimTime),
        Instructions(u64),
    }

    struct Sample {
        elapsed: Duration,
        performance: Ip32PerformanceSnapshot,
    }

    fn run_sample(
        prom: &[u8],
        quantum: usize,
        inline: bool,
        event_chain_policy: Ip32EventChainPolicy,
        limit: Limit,
    ) -> Sample {
        let mut machine = Ip32Machine::from_config(Ip32MachineConfig {
            prom_image: prom.to_vec(),
            ..Ip32MachineConfig::default()
        })
        .unwrap();
        machine.control.cpu_continuation_quantum = quantum;
        machine.control.inline_sysad_completion = inline;
        machine.control.event_chain_policy = event_chain_policy;
        machine.schedule_power_on().unwrap();

        let started = Instant::now();
        match limit {
            Limit::Events(max_events) => {
                machine.run_steps(max_events).unwrap();
            }
            Limit::SimTime(deadline) => {
                machine.run_until_time(deadline).unwrap();
            }
            Limit::Instructions(target) => {
                while machine.performance_snapshot().cpu.retired_instructions < target {
                    match machine.run_steps(4_096).unwrap() {
                        RunStatus::Dispatched | RunStatus::StepLimitReached => {}
                        status => panic!(
                            "PROM became inactive before reaching {target} instructions: {status:?}"
                        ),
                    }
                }
            }
        }
        Sample {
            elapsed: started.elapsed(),
            performance: machine.performance_snapshot(),
        }
    }

    fn print_median(label: &str, mode: &str, mut samples: Vec<Sample>) {
        samples.sort_by_key(|sample| sample.elapsed);
        let sample = &samples[samples.len() / 2];
        let host_seconds = sample.elapsed.as_secs_f64();
        let simulated_seconds = sample.performance.sim_time.get() as f64 / IP32_TIMEBASE_HZ as f64;
        let instructions = sample.performance.cpu.retired_instructions;
        let events = sample.performance.runtime.dispatched_events;
        eprintln!(
            "{label}/{mode}: median_elapsed={:?}, simulated_seconds={simulated_seconds:.6}, rtf={:.4}, instructions/s={:.0}, events/s={:.0}, events/instruction={:.3}, sysad={}, memory={}, cmi={}, cgi={}",
            sample.elapsed,
            simulated_seconds / host_seconds,
            instructions as f64 / host_seconds,
            events as f64 / host_seconds,
            events as f64 / instructions.max(1) as f64,
            sample.performance.sysad_transactions,
            sample.performance.memory_transactions,
            sample.performance.cmi_transactions,
            sample.performance.cgi_transactions,
        );
    }

    let path = std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name a local image");
    let prom = std::fs::read(path).expect("the local PROM image must be readable");
    let max_events = std::env::var("IP32_PROM_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(2_000_000);
    let simulated_ticks = std::env::var("IP32_PROM_SIM_TICKS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000_000);
    let retired_instructions = std::env::var("IP32_PROM_INSTRUCTIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let requested_runs = std::env::var("IP32_PROM_BENCH_RUNS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(5)
        .max(1);
    let runs = if requested_runs.is_multiple_of(2) {
        requested_runs + 1
    } else {
        requested_runs
    };
    let selected_mode = std::env::var("IP32_PROM_BENCH_MODE").ok();

    for (label, quantum, inline, event_chain_policy) in [
        ("reference", 1, false, Ip32EventChainPolicy::disabled()),
        (
            "optimized-no-fusion",
            DEFAULT_CPU_CONTINUATION_QUANTUM,
            true,
            Ip32EventChainPolicy::disabled(),
        ),
        (
            "sysad-fusion",
            DEFAULT_CPU_CONTINUATION_QUANTUM,
            true,
            Ip32EventChainPolicy {
                sysad: true,
                budget: 16,
                ..Ip32EventChainPolicy::disabled()
            },
        ),
        (
            "sysad-memory-fusion",
            DEFAULT_CPU_CONTINUATION_QUANTUM,
            true,
            Ip32EventChainPolicy {
                sysad: true,
                memory: true,
                budget: 16,
                ..Ip32EventChainPolicy::disabled()
            },
        ),
        (
            "all-fusion",
            DEFAULT_CPU_CONTINUATION_QUANTUM,
            true,
            Ip32EventChainPolicy::all(),
        ),
    ] {
        if selected_mode.as_deref().is_some_and(|mode| mode != label) {
            continue;
        }
        for (mode, limit) in [
            ("sim-time", Limit::SimTime(SimTime::new(simulated_ticks))),
            ("instructions", Limit::Instructions(retired_instructions)),
            ("events", Limit::Events(max_events)),
        ] {
            let samples = (0..runs)
                .map(|_| run_sample(&prom, quantum, inline, event_chain_policy, limit))
                .collect();
            print_median(label, mode, samples);
        }
    }
}
