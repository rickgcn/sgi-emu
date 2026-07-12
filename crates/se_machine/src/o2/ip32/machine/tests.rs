use se_core::role::BusDeviceRole;
use se_core::tracing::{TraceRecord, TraceSink, TraceValue};
use se_device::bus::irq::IrqTransaction;
use se_device::bus::media::{MediaPayload, MediaPort};
use se_device::chipset::crime::config::{CrimeAccessPolicy, CrimeConfigError, CrimeSdramBankSize};
use se_device::chipset::crime::iou::{CrimeCgiBus, CrimeCmiBus};
use se_device::chipset::crime::memory::CrimeSdram;
use se_device::chipset::crime::memory::bus::CrimeMemoryBus;
use se_device::chipset::crime::protocol::{
    CrimeCompletionPayload, CrimeMemoryBankSelect, CrimeMemoryClient, CrimeMemoryTransaction,
    CrimeTransactionId, CrimeTransfer,
};
use se_device::chipset::crime::registers;
use se_device::chipset::mace::Mace;
use se_device::cpu::mips4::gpr::Mips4GprIndex;
use se_device::memory::flash::ReadArrayFlash;
use se_device::serial::uart16550::Uart16550;

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
const LD_R3_R1: u32 = 0xdc23_0000;
const SD_R2_R1_CONTROL: u32 = 0xfc22_0008;
const SD_R2_R1_INTERRUPT_ENABLE: u32 = 0xfc22_0018;
const SD_R2_R1_SOFTWARE_INTERRUPT: u32 = 0xfc22_0020;
const WAIT: u32 = 0x4200_0020;

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

    assert_eq!(registry.len(), 23);
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
            .get_typed::<Ip32GbeEndpoint>(component_ids::GBE)
            .is_ok()
    );
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
            transfer: CrimeTransfer::Read { length: 4 },
        });
    assert_eq!(
        completion.result.unwrap().payload,
        CrimeCompletionPayload::ReadData(vec![0, 0, 0x12, 0x34])
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
            transfer: CrimeTransfer::Read { length: 44 },
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
fn crime_piu_requires_doubleword_access_through_the_cpu_path() {
    let config = config_with_program(&[(0, LUI_R1_CRIME), (4, LD_R3_R1), (8, WAIT)]);
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
            transfer: CrimeTransfer::Read { length: 4 },
        });
    assert_eq!(
        completion.result.unwrap().payload,
        CrimeCompletionPayload::ReadData(vec![0, 0, 0x12, 0x34])
    );
}

#[derive(Default)]
struct PromAcceptanceSink {
    failed_addresses: Vec<u64>,
}

impl TraceSink for PromAcceptanceSink {
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
    const ACCEPTED_UNIMPLEMENTED_BOUNDARIES: &[u64] = &[0x1400_0050];

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
        !exception_loop
            || failed
                .last()
                .is_some_and(|address| ACCEPTED_UNIMPLEMENTED_BOUNDARIES.contains(address)),
        "PROM remained in the exception loop at {pc:#018x}; EPC={epc:#018x}, exception={exception_code}, failed accesses: {failed:#x?}"
    );
    assert!(
        !failed.contains(&0x5000_0000),
        "PROM bank probing incorrectly received a SysAD bus error at 0x50000000"
    );

    assert!(
        failed.iter().all(|address| {
            !(registers::CRIME_BASE..registers::CRIME_REGISTER_END).contains(address)
                || ACCEPTED_UNIMPLEMENTED_BOUNDARIES.contains(address)
        }),
        "a modeled CRIME access returned a bus error"
    );
}
