use se_core::role::BusDeviceRole;
use se_core::tracing::{TraceRecord, TraceSink, TraceValue};
use se_device::bus::irq::IrqTransaction;
use se_device::bus::media::{MediaPayload, MediaPort};
use se_device::chipset::crime::config::{CrimeAccessPolicy, CrimeConfigError, CrimeSdramBankSize};
use se_device::chipset::crime::iou::{CrimeCgiBus, CrimeCmiBus};
use se_device::chipset::crime::memory::CrimeSdram;
use se_device::chipset::crime::memory::bus::CrimeMemoryBus;
use se_device::chipset::crime::protocol::{
    CrimeCompletionPayload, CrimeMemoryClient, CrimeMemoryTransaction, CrimeTransactionId,
    CrimeTransfer,
};
use se_device::chipset::crime::registers;
use se_device::chipset::mace::Mace;
use se_device::cpu::mips4::gpr::Mips4GprIndex;
use se_device::memory::flash::ReadArrayFlash;

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
            no_ecc: false,
            transfer: CrimeTransfer::Read { length: 4 },
        });
    assert_eq!(
        completion.result.unwrap().payload,
        CrimeCompletionPayload::ReadData(vec![0, 0, 0x12, 0x34])
    );
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
    assert!(
        !matches!(
            pc,
            0xffff_ffff_bfc0_0380
                | 0xffff_ffff_bfc0_0384
                | 0xffff_ffff_bfc0_0388
                | 0xffff_ffff_bfc0_038c
                | 0xffff_ffff_bfc0_03a0
                | 0xffff_ffff_bfc0_03a4
        ),
        "PROM remained in the exception loop at {pc:#018x}; EPC={epc:#018x}, exception={exception_code}, failed accesses: {failed:#x?}"
    );
    assert!(
        !failed.contains(&0x5000_0000),
        "PROM bank probing incorrectly received a SysAD bus error at 0x50000000"
    );

    assert!(
        failed.iter().all(|address| {
            !(registers::CRIME_BASE..registers::CRIME_REGISTER_END).contains(address)
        }),
        "a defined CRIME access returned a bus error"
    );
}
