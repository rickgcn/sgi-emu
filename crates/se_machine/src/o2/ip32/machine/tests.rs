use se_core::role::BusDeviceRole;
use se_core::tracing::{TraceInterest, TraceRecord, TraceSink, TraceSource, TraceValue};
use se_device::bus::irq::{IrqBus, IrqTransaction};
use se_device::bus::media::{MediaPayload, MediaPort};
use se_device::bus::one_wire::OneWireBus;
use se_device::bus::two_wire::TwoWireBus;
use se_device::chipset::crime::config::{CrimeAccessPolicy, CrimeConfigError, CrimeSdramBankSize};
use se_device::chipset::crime::iou::{CrimeCgiBus, CrimeCmiBus};
use se_device::chipset::crime::memory::CrimeSdram;
use se_device::chipset::crime::memory::bus::CrimeMemoryBus;
#[cfg(feature = "jit")]
use se_device::chipset::crime::protocol::{
    CrimeCgiTransaction, CrimeLinkDeviceResponse, CrimeLinkOperation, CrimePioRequest,
};
use se_device::chipset::crime::protocol::{
    CrimeCompletionPayload, CrimeMemoryBankSelect, CrimeMemoryClient, CrimeMemoryTransaction,
    CrimeTransactionId, CrimeTransfer,
};
use se_device::chipset::crime::registers;
use se_device::chipset::gbe::Gbe;
#[cfg(feature = "jit")]
use se_device::chipset::gbe::protocol::{GbeExternalClock, GbeExternalInput};
use se_device::chipset::mace::Mace;
use se_device::cpu::mips4::gpr::Mips4GprIndex;
use se_device::input::ps2::{Ps2Keyboard, Ps2Mouse};
use se_device::memory::ds2502::Ds2502;
use se_device::memory::flash::SystemFlash;
use se_device::rtc::ds1687::Ds1687;
use se_device::serial::uart16550::Uart16550;
#[cfg(feature = "jit")]
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
    assert_component_eq!(TwoWireBus, component_ids::GBE_CRT_DDC_BUS);
    assert_component_eq!(TwoWireBus, component_ids::GBE_FLAT_PANEL_DDC_BUS);
    assert_component_eq!(TwoWireBus, component_ids::KEYBOARD_PS2_BUS);
    assert_component_eq!(TwoWireBus, component_ids::MOUSE_PS2_BUS);
    assert_component_eq!(Ps2Keyboard, component_ids::KEYBOARD);
    assert_component_eq!(Ps2Mouse, component_ids::MOUSE);
    assert_component_eq!(Gbe, component_ids::GBE);
    assert_component_eq!(Ds2502, component_ids::NIC_IDENTITY);
    assert_component_eq!(SystemFlash, component_ids::PROM);
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
    assert_eq!(
        reference.control.latest_display_frame,
        optimized.control.latest_display_frame
    );
    assert_eq!(
        reference.control.dropped_display_frames,
        optimized.control.dropped_display_frames
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

#[cfg(feature = "jit")]
#[test]
fn jit_hot_loop_matches_scalar_machine_state_and_scheduler() {
    let loop_address = 0xffff_ffff_9fc0_0020_u64;
    let jump_loop = (2_u32 << 26) | (((loop_address as u32) >> 2) & 0x03ff_ffff);
    let program = [
        (0x00, i_type(0x0f, 0, 1, 0x9fc0)),
        (0x04, i_type(0x0d, 1, 1, 0x0020)),
        (0x08, i_type(0x0d, 0, 2, 3)),
        (0x0c, (0x10_u32 << 26) | (4 << 21) | (2 << 16) | (16 << 11)),
        (0x10, r_type(1, 0, 0, 0, 0x08)),
        (0x14, 0),
        (0x20, i_type(0x09, 2, 2, 1)),
        (0x24, jump_loop),
        (0x28, 0),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config.clone()).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(2_000_000);
    assert_eq!(
        scalar.run_until_time(deadline).unwrap(),
        RunStatus::DeadlineReached
    );
    assert_eq!(
        jit.run_until_time(deadline).unwrap(),
        RunStatus::DeadlineReached
    );
    assert_machine_architecture_equal(&scalar, &jit);
    let scalar_performance = scalar.performance_snapshot();
    let jit_performance = jit.performance_snapshot();
    assert_eq!(scalar_performance.sim_time, jit_performance.sim_time);
    assert_eq!(
        scalar_performance.cpu.retired_instructions,
        jit_performance.cpu.retired_instructions
    );
    assert_eq!(
        scalar_performance.cpu.exceptions,
        jit_performance.cpu.exceptions
    );
    let jit_statistics = jit.control.jit_engine.as_ref().unwrap().statistics();
    assert!(
        jit_statistics.compiled_blocks > 0,
        "JIT did not compile the hot loop: {jit_statistics:?}"
    );

    let state = jit.save_state().unwrap();
    let restored =
        Ip32Machine::from_state_with_trace_sink(jit_config, state, NoopTraceSink).unwrap();
    assert_machine_architecture_equal(&jit, &restored);
    assert!(restored.control.jit_engine.as_ref().unwrap().is_empty());
}

#[cfg(feature = "jit")]
#[test]
fn jit_native_mace_ust_loop_matches_scalar() {
    let program = [
        (0x00, i_type(0x0f, 0, 8, 0xbf34)),
        (0x04, i_type(0x09, 0, 9, 512)),
        (0x08, i_type(0x37, 8, 10, 0)),
        (0x0c, i_type(0x09, 9, 9, u16::MAX)),
        (0x10, i_type(0x05, 9, 0, 0xfffd)),
        (0x14, 0),
        (0x18, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(20_000_000);
    scalar.run_until_time(deadline).unwrap();
    jit.run_until_time(deadline).unwrap();
    let scalar_cpu = scalar
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let jit_cpu = jit
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let register = Mips4GprIndex::from_u8(10).unwrap();
    assert_eq!(
        scalar_cpu.state().gpr().read(register),
        jit_cpu.state().gpr().read(register),
        "native UST value differs: scalar={:?}, JIT={:?}",
        scalar.performance_snapshot(),
        jit.performance_snapshot(),
    );
    assert_eq!(
        scalar.performance_snapshot().cpu.retired_instructions,
        jit.performance_snapshot().cpu.retired_instructions
    );
    assert!(
        jit.performance_snapshot().jit.fast_transaction_hits > 0,
        "native UST lowering did not run: {:?}",
        jit.performance_snapshot(),
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_native_crime_timer_loop_matches_scalar() {
    let program = [
        (0x00, i_type(0x0f, 0, 8, 0xb400)),
        (0x04, i_type(0x09, 0, 9, 512)),
        (0x08, i_type(0x37, 8, 10, 0x0038)),
        (0x0c, i_type(0x09, 9, 9, u16::MAX)),
        (0x10, i_type(0x05, 9, 0, 0xfffd)),
        (0x14, 0),
        (0x18, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(20_000_000);
    scalar.run_until_time(deadline).unwrap();
    jit.run_until_time(deadline).unwrap();
    let scalar_cpu = scalar
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let jit_cpu = jit
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let register = Mips4GprIndex::from_u8(10).unwrap();
    assert_eq!(
        scalar_cpu.state().gpr().read(register),
        jit_cpu.state().gpr().read(register),
        "native CRIME TIMER value differs: scalar={:?}, JIT={:?}",
        scalar.performance_snapshot(),
        jit.performance_snapshot(),
    );
    assert_eq!(
        scalar.performance_snapshot().cpu.retired_instructions,
        jit.performance_snapshot().cpu.retired_instructions
    );
    assert!(
        jit.performance_snapshot().jit.fast_transaction_hits > 0,
        "native CRIME TIMER lowering did not run: {:?}",
        jit.performance_snapshot(),
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_native_crime_timer_poll_matches_scalar() {
    let program = [
        (0x00, i_type(0x0f, 0, 8, 0xb400)),
        (0x04, i_type(0x37, 8, 10, 0x0038)),
        (0x08, i_type(0x09, 10, 10, 30_000)),
        (0x0c, i_type(0x37, 8, 11, 0x0038)),
        (0x10, r_type(11, 10, 12, 0, 0x2b)),
        (0x14, i_type(0x05, 12, 0, 0xfffd)),
        (0x18, 0),
        (0x1c, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(20_000_000);
    scalar.run_until_time(deadline).unwrap();
    jit.run_until_time(deadline).unwrap();
    let scalar_cpu = scalar
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let jit_cpu = jit
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    assert_eq!(
        scalar.performance_snapshot().cpu.retired_instructions,
        jit.performance_snapshot().cpu.retired_instructions,
        "CRIME TIMER polling retired a different path: scalar={:?}, JIT={:?}",
        scalar.performance_snapshot(),
        jit.performance_snapshot(),
    );
    for register in [10, 11, 12] {
        let register = Mips4GprIndex::from_u8(register).unwrap();
        assert_eq!(
            scalar_cpu.state().gpr().read(register),
            jit_cpu.state().gpr().read(register)
        );
    }
    assert!(jit.performance_snapshot().jit.fast_transaction_hits > 0);
}

#[cfg(feature = "jit")]
#[test]
fn jit_crime_timer_write_then_poll_reaches_wait_at_scalar_time() {
    let program = [
        (0x00, i_type(0x0f, 0, 8, 0xb400)),
        (0x04, i_type(0x3f, 8, 0, 0x0038)),
        (0x08, i_type(0x0f, 0, 10, 0x0030)),
        (0x0c, i_type(0x37, 8, 11, 0x0038)),
        (0x10, r_type(11, 10, 12, 0, 0x2b)),
        (0x14, i_type(0x05, 12, 0, 0xfffd)),
        (0x18, 0),
        (0x1c, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    scalar.run_until_time(SimTime::new(60_000_000)).unwrap();
    jit.run_until_time(SimTime::new(60_000_000)).unwrap();
    assert_eq!(
        jit.control.first_cpu_idle_time,
        scalar.control.first_cpu_idle_time
    );
    assert_machine_architecture_equal(&scalar, &jit);
    assert!(jit.performance_snapshot().jit.fast_transaction_hits > 0);
    assert!(
        jit.performance_snapshot().jit.region_entries > 0,
        "timer polling did not enter a Region: {:?}",
        jit.performance_snapshot()
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_sdram_refills_preserve_crime_timer_poll_timing() {
    let program = [
        (0x00, i_type(0x0f, 0, 8, 0xb400)),
        (0x04, i_type(0x3f, 8, 0, 0x0038)),
        (0x08, i_type(0x0f, 0, 10, 0x0030)),
        (0x0c, i_type(0x0f, 0, 1, 0x8000)),
        (0x10, i_type(0x2b, 1, 2, 0x0000)),
        (0x14, i_type(0x09, 1, 1, 0x0020)),
        (0x18, i_type(0x37, 8, 11, 0x0038)),
        (0x1c, r_type(11, 10, 12, 0, 0x2b)),
        (0x20, i_type(0x05, 12, 0, 0xfffb)),
        (0x24, 0),
        (0x28, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    scalar.run_until_time(SimTime::new(60_000_000)).unwrap();
    jit.run_until_time(SimTime::new(60_000_000)).unwrap();
    assert_eq!(
        jit.control.first_cpu_idle_time,
        scalar.control.first_cpu_idle_time
    );
    assert_machine_architecture_equal(&scalar, &jit);
    assert!(jit.performance_snapshot().jit.fast_transaction_hits > 0);
}

#[cfg(feature = "jit")]
#[test]
fn jit_direct_sdram_transactions_reach_wait_at_scalar_time() {
    let program = [
        (0x00, LUI_R1_LINEAR_RAM),
        (0x04, ADDIU_R2_1234),
        (0x08, SW_R2_R1),
        (0x0c, LW_R3_R1),
        (0x10, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    scalar.run_until_time(SimTime::new(1_000_000)).unwrap();
    jit.run_until_time(SimTime::new(1_000_000)).unwrap();
    assert_eq!(
        jit.control.first_cpu_idle_time,
        scalar.control.first_cpu_idle_time
    );
    assert_machine_architecture_equal(&scalar, &jit);
    assert!(jit.performance_snapshot().jit.fast_transaction_hits > 0);
}

#[cfg(feature = "jit")]
#[test]
fn jit_component_tracing_preserves_scheduled_transaction_path() {
    let program = [
        (0x00, LUI_R1_LINEAR_RAM),
        (0x04, ADDIU_R2_1234),
        (0x08, SW_R2_R1),
        (0x0c, LW_R3_R1),
        (0x10, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar =
        Ip32Machine::from_config_with_trace_sink(scalar_config, ComponentCaptureSink::default())
            .unwrap();
    let mut jit =
        Ip32Machine::from_config_with_trace_sink(jit_config, ComponentCaptureSink::default())
            .unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    scalar.run_until_time(SimTime::new(1_000_000)).unwrap();
    jit.run_until_time(SimTime::new(1_000_000)).unwrap();

    assert_machine_architecture_equal(&scalar, &jit);
    assert_eq!(
        scalar.runtime().trace_recorder().sink().records,
        jit.runtime().trace_recorder().sink().records
    );
    let performance = jit.performance_snapshot();
    assert_eq!(performance.jit.fast_transaction_hits, 0);
    assert!(performance.jit.fast_transaction_attempts > 0);
    assert_eq!(
        performance.jit.fast_transaction_fallbacks,
        performance.jit.fast_transaction_attempts
    );
}

#[cfg(feature = "jit")]
#[test]
fn jit_direct_sdram_transactions_respect_uart_completion() {
    let loop_address = 0xffff_ffff_9fc0_0014_u64;
    let branch_to_loop = ((loop_address as i64 - 0xffff_ffff_9fc0_0028_u64 as i64) / 4) as u16;
    let program = [
        (0x00, i_type(0x0f, 0, 1, 0xbf39)),
        (0x04, i_type(0x0d, 1, 1, 0x0007)),
        (0x08, i_type(0x09, 0, 2, b'A'.into())),
        (0x0c, i_type(0x28, 1, 2, 0x0000)),
        (0x10, i_type(0x0f, 0, 3, 0x8000)),
        (0x14, i_type(0x23, 3, 4, 0x0000)),
        (0x18, i_type(0x09, 3, 3, 0x0020)),
        (0x1c, i_type(0x24, 1, 5, 0x0500)),
        (0x20, i_type(0x0c, 5, 5, 0x0020)),
        (0x24, i_type(0x04, 5, 0, branch_to_loop)),
        (0x28, 0),
        (0x2c, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(10_000_000);
    scalar.run_until_time(deadline).unwrap();
    jit.run_until_time(deadline).unwrap();
    assert_eq!(
        jit.control.first_cpu_idle_time,
        scalar.control.first_cpu_idle_time
    );
    assert_machine_architecture_equal(&scalar, &jit);
    assert!(jit.performance_snapshot().jit.fast_transaction_hits > 0);
}

#[cfg(feature = "jit")]
#[test]
fn jit_native_timer_reads_respect_uart_completion() {
    let loop_address = 0xffff_ffff_9fc0_0014_u64;
    let branch_to_loop = ((loop_address as i64 - 0xffff_ffff_9fc0_0024_u64 as i64) / 4) as u16;
    let program = [
        (0x00, i_type(0x0f, 0, 1, 0xbf39)),
        (0x04, i_type(0x0d, 1, 1, 0x0007)),
        (0x08, i_type(0x09, 0, 2, b'A'.into())),
        (0x0c, i_type(0x28, 1, 2, 0x0000)),
        (0x10, i_type(0x0f, 0, 3, 0xb400)),
        (0x14, i_type(0x23, 3, 4, 0x003c)),
        (0x18, i_type(0x24, 1, 5, 0x0500)),
        (0x1c, i_type(0x0c, 5, 5, 0x0020)),
        (0x20, i_type(0x04, 5, 0, branch_to_loop)),
        (0x24, 0),
        (0x28, WAIT),
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(10_000_000);
    scalar.run_until_time(deadline).unwrap();
    jit.run_until_time(deadline).unwrap();
    assert_eq!(
        jit.control.first_cpu_idle_time,
        scalar.control.first_cpu_idle_time
    );
    assert_machine_architecture_equal(&scalar, &jit);
    assert!(jit.performance_snapshot().jit.fast_transaction_hits > 0);
}

#[cfg(feature = "jit")]
#[test]
fn jit_native_crime_timer_poll_from_ram_matches_scalar() {
    let ram_program = [
        i_type(0x0f, 0, 8, 0xb400),
        i_type(0x23, 8, 10, 0x003c),
        i_type(0x09, 10, 10, 30_000),
        i_type(0x23, 8, 11, 0x003c),
        r_type(11, 10, 12, 0, 0x2b),
        i_type(0x05, 12, 0, 0xfffd),
        0,
        WAIT,
    ];
    let mut loader = vec![
        (0x00, i_type(0x0f, 0, 8, 0xa000)),
        (0x04, i_type(0x0d, 8, 8, 0x1000)),
    ];
    let mut offset = 0x08;
    for (index, instruction) in ram_program.into_iter().enumerate() {
        loader.push((offset, i_type(0x0f, 0, 9, (instruction >> 16) as u16)));
        loader.push((offset + 4, i_type(0x0d, 9, 9, instruction as u16)));
        loader.push((offset + 8, i_type(0x2b, 8, 9, (index * 4) as u16)));
        offset += 12;
    }
    loader.push((offset, i_type(0x0f, 0, 7, 0x8000)));
    loader.push((offset + 4, i_type(0x0d, 7, 7, 0x1000)));
    loader.push((offset + 8, r_type(7, 0, 0, 0, 0x08)));
    loader.push((offset + 12, 0));

    let scalar_config = config_with_program(&loader);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(100_000_000);
    scalar.run_until_time(deadline).unwrap();
    jit.run_until_time(deadline).unwrap();
    let scalar_cpu = scalar
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let jit_cpu = jit
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let timer_registers = |cpu: &R5000Cpu| {
        [10, 11, 12].map(|register| {
            cpu.state()
                .gpr()
                .read(Mips4GprIndex::from_u8(register).unwrap())
        })
    };
    assert_eq!(
        scalar.performance_snapshot().cpu.retired_instructions,
        jit.performance_snapshot().cpu.retired_instructions,
        "RAM CRIME TIMER polling retired a different path: scalar_registers={:?}, JIT_registers={:?}, scalar={:?}, JIT={:?}",
        timer_registers(scalar_cpu),
        timer_registers(jit_cpu),
        scalar.performance_snapshot(),
        jit.performance_snapshot(),
    );
    for register in [10, 11, 12] {
        let register = Mips4GprIndex::from_u8(register).unwrap();
        assert_eq!(
            scalar_cpu.state().gpr().read(register),
            jit_cpu.state().gpr().read(register)
        );
    }
    assert!(jit.performance_snapshot().jit.fast_transaction_hits > 0);
    assert!(jit.performance_snapshot().jit.region_entries > 0);
}

#[cfg(not(feature = "jit"))]
#[test]
fn requesting_jit_without_the_feature_is_rejected() {
    let config = Ip32MachineConfig {
        jit_enabled: true,
        ..Ip32MachineConfig::default()
    };
    assert!(matches!(
        Ip32Machine::from_config(config),
        Err(Ip32MachineBuildError::JitUnavailable)
    ));
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
        port: MediaPort::Serial0,
        payload: MediaPayload::Bytes(vec![0xaa]),
    };
    machine
        .schedule_host_input(SimTime::new(1), input.clone())
        .unwrap();
    assert_eq!(
        machine.schedule_host_input(SimTime::new(2), input),
        Err(Ip32HostInputError::QueueFull(MediaPort::Serial0))
    );
}

#[test]
fn keyboard_and_mouse_reject_legacy_byte_injection() {
    let mut machine = Ip32Machine::from_config(Ip32MachineConfig::default()).unwrap();
    for port in [MediaPort::Keyboard, MediaPort::Mouse] {
        assert_eq!(
            machine.schedule_host_input(
                SimTime::ZERO,
                Ip32HostInput {
                    port,
                    payload: MediaPayload::Bytes(vec![0xaa]),
                },
            ),
            Err(Ip32HostInputError::UnsupportedPort(port))
        );
    }
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

#[cfg(feature = "jit")]
#[test]
fn jit_synthetic_uart_matches_scalar_output_time() {
    let program = [
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
    ];
    let scalar_config = config_with_program(&program);
    let mut jit_config = scalar_config.clone();
    jit_config.jit_enabled = true;
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();
    scalar.schedule_power_on().unwrap();
    jit.schedule_power_on().unwrap();

    let deadline = SimTime::new(2_000_000);
    let _ = scalar.run_until_time(deadline).unwrap();
    let _ = jit.run_until_time(deadline).unwrap();
    assert_eq!(
        scalar.control.first_serial_output_time,
        jit.control.first_serial_output_time
    );
    assert_eq!(scalar.poll_host_output(), jit.poll_host_output());
    assert_machine_architecture_equal(&scalar, &jit);
}

#[cfg(feature = "jit")]
#[test]
fn jit_boundary_budget_matches_iterated_pclock_reference() {
    fn reference(
        mut clock: CpuClock,
        now: SimTime,
        deadline: SimTime,
        next_event: Option<SimTime>,
        maximum: usize,
    ) -> usize {
        let mut time = now;
        for boundary in 1..=maximum {
            let delay = clock.next_pclock_delay();
            let Some(next_time) = time.checked_add(delay) else {
                return boundary;
            };
            if next_time > deadline || next_event.is_some_and(|event| event <= next_time) {
                return boundary;
            }
            time = next_time;
        }
        maximum
    }

    for remainder in [0, 1, 17, 89_999_999, 179_999_999] {
        let clock = CpuClock {
            frequency_hz: 180_000_000,
            remainder,
        };
        for now in [0, 1, 10_000, u64::MAX - 10_000] {
            for deadline_delta in [0, 1, 21, 22, 23, 1_000, 9_999] {
                let deadline = SimTime::new(now.saturating_add(deadline_delta));
                for event_delta in [None, Some(0), Some(1), Some(22), Some(999)] {
                    let next_event =
                        event_delta.map(|delta| SimTime::new(now.saturating_add(delta)));
                    for maximum in [0, 1, 2, 31, 32, 255, 256] {
                        assert_eq!(
                            clock.plan_boundary_budget(
                                SimTime::new(now),
                                deadline,
                                next_event,
                                maximum,
                            ),
                            reference(clock, SimTime::new(now), deadline, next_event, maximum,),
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn synthetic_prom_programs_system_flash_through_mace_and_isa() {
    let config = config_with_program(&[
        (0, i_type(0x0f, 0, 1, 0xbf31)),
        (4, i_type(0x09, 0, 2, 1)),
        (8, i_type(0x3f, 1, 2, 8)),
        (12, i_type(0x0f, 0, 1, 0xbfc0)),
        (16, i_type(0x09, 0, 2, 0x5a)),
        (20, i_type(0x28, 1, 2, 0x4000)),
        (24, WAIT),
    ]);
    let mut machine = Ip32Machine::from_config(config).unwrap();
    machine.schedule_power_on().unwrap();

    let _ = machine.run_steps(500).unwrap();
    let flash = machine
        .runtime()
        .registry()
        .get_typed::<SystemFlash>(component_ids::PROM)
        .unwrap();
    assert_eq!(flash.bytes()[0x4000], 0x5a);
    assert_eq!(flash.persistence_revision(), 1);

    machine.hard_reset().unwrap();
    let flash = machine
        .runtime()
        .registry()
        .get_typed::<SystemFlash>(component_ids::PROM)
        .unwrap();
    assert_eq!(flash.bytes()[0x4000], 0x5a);
}

#[test]
fn construction_registers_the_role_oriented_ip32_topology() {
    let machine = Ip32Machine::from_config(config_with_program(&[])).unwrap();
    let registry = machine.runtime().registry();

    assert_eq!(registry.len(), 31);
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
            .get_typed::<TwoWireBus>(component_ids::GBE_CRT_DDC_BUS)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<TwoWireBus>(component_ids::GBE_FLAT_PANEL_DDC_BUS)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<TwoWireBus>(component_ids::KEYBOARD_PS2_BUS)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<TwoWireBus>(component_ids::MOUSE_PS2_BUS)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<Ps2Keyboard>(component_ids::KEYBOARD)
            .is_ok()
    );
    assert!(registry.get_typed::<Ps2Mouse>(component_ids::MOUSE).is_ok());
    assert!(
        registry
            .get_typed::<Ip32StubEndpoint>(component_ids::VICE)
            .is_ok()
    );
    assert!(
        registry
            .get_typed::<SystemFlash>(component_ids::PROM)
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
        RunStatus::DeadlineReached
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
    reference.control.capture_logical_transitions = true;
    optimized.control.capture_logical_transitions = true;
    reference.schedule_power_on().unwrap();
    optimized.schedule_power_on().unwrap();

    assert_eq!(
        reference.run_until_time(SimTime::new(1_000_000)).unwrap(),
        RunStatus::DeadlineReached
    );
    assert_eq!(
        optimized.run_until_time(SimTime::new(1_000_000)).unwrap(),
        RunStatus::DeadlineReached
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
    machine.control.capture_logical_transitions = true;
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
        RunStatus::DeadlineReached
    );
}

#[test]
fn fusion_respects_deadlines_and_same_time_global_events() {
    for deadline in [119_999, 120_000, 120_014, 120_015, 120_016, 120_030] {
        let config = config_with_program(&[(0, ADDIU_R2_1), (4, WAIT)]);
        let mut reference = Ip32Machine::from_config(config.clone()).unwrap();
        reference.control.event_chain_policy = Ip32EventChainPolicy::disabled();
        let mut optimized = Ip32Machine::from_config(config).unwrap();
        reference.control.capture_logical_transitions = true;
        optimized.control.capture_logical_transitions = true;
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
        machine.control.capture_logical_transitions = true;
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
        RunStatus::DeadlineReached
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

#[derive(Default)]
struct PromEnvironmentSink {
    failed_addresses: Vec<u64>,
    flash_write_addresses: Vec<u64>,
}

impl TraceSink for PromEnvironmentSink {
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
        (target == "ip32.sysad" && event == "access")
            || (target == "ip32.mace.cmi" && event == "pio")
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        let value = |key| {
            record
                .fields
                .iter()
                .find_map(|field| (field.key == key).then_some(field.value))
        };
        if record.target == "ip32.sysad" && record.event == "access" {
            if matches!(value("bus_error"), Some(TraceValue::Bool(true)))
                && let Some(TraceValue::Hex64(address)) = value("physical_address")
            {
                self.failed_addresses.push(address);
            }
            return;
        }
        if record.target == "ip32.mace.cmi"
            && record.event == "pio"
            && matches!(value("write"), Some(TraceValue::Bool(true)))
            && let Some(TraceValue::Hex64(address)) = value("address")
            && (se_device::chipset::mace::registers::PROM_START
                ..se_device::chipset::mace::registers::PROM_END)
                .contains(&address)
        {
            self.flash_write_addresses.push(address);
        }
    }
}

fn drain_serial_one<S>(machine: &mut Ip32Machine<S>, terminal: &mut Vec<u8>) {
    while let Some(output) = machine.poll_serial_output() {
        if output.port == Ip32SerialPort::Serial1 {
            terminal.extend(output.bytes);
        }
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|candidate| candidate == needle)
}

fn run_until_serial_one_contains<S: TraceSink>(
    machine: &mut Ip32Machine<S>,
    terminal: &mut Vec<u8>,
    needle: &[u8],
    max_events: usize,
    context: &str,
) {
    const BATCH_SIZE: usize = 4_096;
    let mut dispatched = 0;
    while dispatched < max_events {
        let batch = (max_events - dispatched).min(BATCH_SIZE);
        let status = machine.run_steps(batch).unwrap();
        dispatched += batch;
        drain_serial_one(machine, terminal);
        if contains_bytes(terminal, needle) {
            return;
        }
        assert!(
            !matches!(status, RunStatus::Idle | RunStatus::Stopped),
            "IP32 became inactive while waiting for {context}"
        );
    }
    panic!("IP32 did not produce {context} within {max_events} events");
}

fn send_serial_one<S: TraceSink>(machine: &mut Ip32Machine<S>, bytes: &[u8]) {
    machine
        .schedule_serial_input(
            machine.runtime().now(),
            Ip32SerialPort::Serial1,
            bytes.to_vec(),
        )
        .unwrap();
}

fn enter_command_monitor<S: TraceSink>(
    machine: &mut Ip32Machine<S>,
    terminal: &mut Vec<u8>,
    max_events: usize,
) {
    run_until_serial_one_contains(
        machine,
        terminal,
        b"System Maintenance Menu",
        max_events,
        "the System Maintenance Menu",
    );
    terminal.clear();
    send_serial_one(machine, b"5\r");
    run_until_serial_one_contains(
        machine,
        terminal,
        b"> ",
        max_events,
        "the Command Monitor prompt",
    );
}

fn printenv_diagmode<S: TraceSink>(
    machine: &mut Ip32Machine<S>,
    terminal: &mut Vec<u8>,
    max_events: usize,
) {
    terminal.clear();
    send_serial_one(machine, b"printenv diagmode\r");
    run_until_serial_one_contains(
        machine,
        terminal,
        b"> ",
        max_events,
        "the prompt after printenv",
    );
    assert!(
        contains_bytes(terminal, b"diagmode=v"),
        "printenv did not return the value programmed by setenv"
    );
}

#[cfg(feature = "jit")]
#[derive(Default)]
struct PromDisplaySink {
    dma_events: std::collections::BTreeMap<String, u64>,
    render_writes: std::collections::BTreeMap<u64, (u64, u64)>,
    capture_render: bool,
}

#[cfg(feature = "jit")]
impl TraceSink for PromDisplaySink {
    fn interest(&self, source: TraceSource) -> TraceInterest {
        if source == TraceSource::Component(component_ids::GBE)
            || self.capture_render && source == TraceSource::Component(component_ids::CRIME)
        {
            TraceInterest::Filtered
        } else {
            TraceInterest::None
        }
    }

    fn enabled(
        &self,
        _source: TraceSource,
        _level: se_core::tracing::TraceLevel,
        target: &str,
        _event: &str,
    ) -> bool {
        target == "gbe.dma" || target == "ip32.crime.re"
    }

    fn record(&mut self, record: TraceRecord<'_>) {
        if record.target == "gbe.dma" {
            *self.dma_events.entry(record.event.to_owned()).or_default() += 1;
            return;
        }
        if record.event != "register_write" {
            return;
        }
        let value = |key| {
            record
                .fields
                .iter()
                .find_map(|field| (field.key == key).then_some(field.value))
        };
        if let (Some(TraceValue::Hex64(address)), Some(TraceValue::Hex64(value))) =
            (value("physical_address"), value("value"))
        {
            let entry = self.render_writes.entry(address).or_default();
            entry.0 = entry.0.saturating_add(1);
            entry.1 = value;
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
    assert!(
        failed
            .iter()
            .all(|address| !(0x1600_0000..0x1700_0000).contains(address)),
        "a PROM GBE initialization access returned a bus error"
    );
}

#[cfg(feature = "jit")]
#[test]
#[ignore = "requires a local proprietary IP32 PROM image"]
fn local_ip32_prom_reaches_gbe_display_output() {
    fn read_gbe<S: TraceSink>(machine: &mut Ip32Machine<S>, address: u64) -> u32 {
        let gbe = machine
            .runtime_mut()
            .registry_mut()
            .get_typed_mut::<Gbe>(component_ids::GBE)
            .unwrap();
        let response = gbe.accept(CrimeCgiTransaction {
            id: CrimeTransactionId::new(u128::MAX),
            controller: component_ids::CRIME,
            target: component_ids::GBE,
            operation: CrimeLinkOperation::Pio(CrimePioRequest {
                address,
                transfer: CrimeTransfer::read(4),
            }),
        });
        let CrimeLinkDeviceResponse::Complete(completion) = response else {
            panic!("diagnostic GBE read was unexpectedly deferred");
        };
        let CrimeCompletionPayload::ReadData(data) = completion.result.unwrap() else {
            panic!("diagnostic GBE read returned the wrong payload");
        };
        u32::from_be_bytes(data.as_ref().try_into().unwrap())
    }

    fn read_ram<S: TraceSink>(machine: &Ip32Machine<S>, address: u64, length: usize) -> Vec<u8> {
        let ram = machine
            .runtime()
            .registry()
            .get_typed::<CrimeSdram>(component_ids::RAM)
            .unwrap();
        let mut data = Vec::with_capacity(length);
        let mut offset = 0;
        while offset < length {
            let chunk = (length - offset).min(128);
            let (bytes, _) = ram
                .stable_code_window(address + offset as u64, chunk, true)
                .unwrap();
            data.extend_from_slice(&bytes);
            offset += chunk;
        }
        data
    }

    let path = std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name a local image");
    let config = Ip32MachineConfig {
        prom_image: std::fs::read(path).expect("the local PROM image must be readable"),
        jit_enabled: true,
        ..Ip32MachineConfig::default()
    };
    let mut machine =
        Ip32Machine::from_config_with_trace_sink(config, PromDisplaySink::default()).unwrap();
    machine.schedule_power_on().unwrap();
    machine
        .schedule_gbe_external_input(SimTime::ZERO, GbeExternalInput::SenseN(false))
        .unwrap();
    machine
        .schedule_gbe_external_input(
            SimTime::ZERO,
            GbeExternalInput::PixelClock {
                source: GbeExternalClock::Ttl,
                numerator_hz: 20_000_000,
                denominator: 1,
            },
        )
        .unwrap();

    let max_events = std::env::var("IP32_PROM_DISPLAY_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(100_000_000);
    let mut events = 0;
    let mut last_frame = None;
    let mut terminal = Vec::new();
    let diagnostic_frame_limit = std::env::var("IP32_PROM_DISPLAY_DIAGNOSTIC_FRAMES")
        .ok()
        .and_then(|value| value.parse::<u64>().ok());
    while events < max_events {
        let batch = (max_events - events).min(4_096);
        let status = machine.run_steps(batch).unwrap();
        events += batch;
        drain_serial_one(&mut machine, &mut terminal);
        if let Some(frame) = machine.take_display_frame() {
            assert!(frame.width != 0 && frame.height != 0);
            let has_visible_pixel = frame
                .rgba
                .chunks_exact(4)
                .any(|pixel| pixel[..3] != [0, 0, 0]);
            if has_visible_pixel {
                return;
            }
            last_frame = Some((events, frame.sequence, frame.width, frame.height));
            if diagnostic_frame_limit.is_some_and(|limit| frame.sequence >= limit) {
                break;
            }
        }
        assert!(!matches!(status, RunStatus::Idle | RunStatus::Stopped));
    }

    let cpu = machine
        .runtime()
        .registry()
        .get_typed::<R5000Cpu>(component_ids::CPU0)
        .unwrap();
    let pc = cpu.state().pc();
    let gpr = (0..32)
        .map(|index| {
            cpu.state()
                .gpr()
                .read(Mips4GprIndex::from_u8(index).unwrap())
        })
        .collect::<Vec<_>>();
    let code_address = (pc & 0x1fff_ffff).saturating_sub(32);
    let code = machine
        .runtime()
        .registry()
        .get_typed::<CrimeSdram>(component_ids::RAM)
        .unwrap()
        .stable_code_window(code_address, 96, true)
        .map(|(code, _)| code)
        .unwrap();
    let time = machine.runtime().now();
    let performance = machine.performance_snapshot();
    let dma_events = machine.runtime().trace_recorder().sink().dma_events.clone();
    let render_writes = machine
        .runtime()
        .trace_recorder()
        .sink()
        .render_writes
        .clone();
    let terminal_start = terminal.len().saturating_sub(2_048);
    let terminal = String::from_utf8_lossy(&terminal[terminal_start..]);
    let control_status = read_gbe(&mut machine, 0x1600_0000);
    let dot_clock = read_gbe(&mut machine, 0x1600_0004);
    let vt_xy = read_gbe(&mut machine, 0x1601_0000);
    let vt_xy_max = read_gbe(&mut machine, 0x1601_0004);
    let vt_intr01 = read_gbe(&mut machine, 0x1601_0020);
    let vt_intr23 = read_gbe(&mut machine, 0x1601_0024);
    let vt_hpixen = read_gbe(&mut machine, 0x1601_0034);
    let vt_vpixen = read_gbe(&mut machine, 0x1601_0038);
    let frame_size_tile = read_gbe(&mut machine, 0x1603_0000);
    let frame_size_pixel = read_gbe(&mut machine, 0x1603_0004);
    let frame_active = read_gbe(&mut machine, 0x1603_0008);
    let frame_shadow = read_gbe(&mut machine, 0x1603_000c);
    let did_active = read_gbe(&mut machine, 0x1604_0000);
    let wid_zero = read_gbe(&mut machine, 0x1604_8000);
    let color_zero = read_gbe(&mut machine, 0x1605_0000);
    let color_one = read_gbe(&mut machine, 0x1605_0004);
    let gamma_zero = read_gbe(&mut machine, 0x1606_0000);
    let gamma_one = read_gbe(&mut machine, 0x1606_0004);
    let gamma_max = read_gbe(&mut machine, 0x1606_03fc);
    let tile_columns = usize::from(((frame_size_tile >> 5) & 0xff) as u8)
        + usize::from(frame_size_tile & 0x1f != 0);
    let tile_rows = usize::try_from(frame_size_pixel >> 16)
        .unwrap()
        .div_ceil(128);
    let tile_pointer_count = tile_columns.saturating_mul(tile_rows);
    let tile_pointer_bytes = read_ram(
        &machine,
        u64::from(frame_active & !0x1f),
        tile_pointer_count.saturating_mul(2),
    );
    let tile_pages = tile_pointer_bytes
        .chunks_exact(2)
        .map(|bytes| u16::from_be_bytes([bytes[0], bytes[1]]))
        .collect::<Vec<_>>();
    let mut nonzero_tiles = Vec::new();
    for page in tile_pages.iter().copied().filter(|page| *page != 0) {
        let base = u64::from(page) << 16;
        let mut first_nonzero = None;
        for offset in (0..65_536).step_by(128) {
            let bytes = read_ram(&machine, base + offset as u64, 128);
            if let Some(index) = bytes.iter().position(|byte| *byte != 0) {
                first_nonzero = Some((offset + index, bytes[index]));
                break;
            }
        }
        if let Some(first_nonzero) = first_nonzero {
            nonzero_tiles.push((page, first_nonzero));
        }
    }
    panic!(
        "the PROM did not publish a non-black GBE frame within {max_events} events; last_frame={last_frame:?}; dma_events={dma_events:?}; render_writes={render_writes:016x?}; terminal={terminal:?}; tile_pages={tile_pages:04x?}; nonzero_tiles={nonzero_tiles:04x?}; time={time:?}; PERFORMANCE={performance:#?}; PC={pc:#018x}; GPR={gpr:#018x?}; CODE_ADDRESS={code_address:#010x}; CODE={code:02x?}; CTRLSTAT={control_status:#010x}; DOTCLOCK={dot_clock:#010x}; VT_XY={vt_xy:#010x}; VT_XY_MAX={vt_xy_max:#010x}; VT_INTR01={vt_intr01:#010x}; VT_INTR23={vt_intr23:#010x}; VT_HPIXEN={vt_hpixen:#010x}; VT_VPIXEN={vt_vpixen:#010x}; FRM_0={frame_size_tile:#010x}; FRM_1={frame_size_pixel:#010x}; FRM_2={frame_active:#010x}; FRM_3={frame_shadow:#010x}; DID={did_active:#010x}; WID_0={wid_zero:#010x}; CMAP_0={color_zero:#010x}; CMAP_1={color_one:#010x}; GMAP_0={gamma_zero:#010x}; GMAP_1={gamma_one:#010x}; GMAP_255={gamma_max:#010x}"
    );
}

#[cfg(feature = "jit")]
#[test]
#[ignore = "requires a local proprietary IP32 PROM image"]
fn local_ip32_prom_initializes_classic_ps2_keyboard() {
    use se_device::chipset::crime::{CrimeError, render::CrimeRenderError};
    use se_runtime::runtime::RunError;

    let path = std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name a local image");
    let config = Ip32MachineConfig {
        prom_image: std::fs::read(&path).expect("the local PROM image must be readable"),
        jit_enabled: true,
        ..Ip32MachineConfig::default()
    };
    let mut machine = Ip32Machine::from_config(config).unwrap();
    machine.schedule_power_on().unwrap();
    machine
        .schedule_gbe_external_input(SimTime::ZERO, GbeExternalInput::SenseN(false))
        .unwrap();
    machine
        .schedule_gbe_external_input(
            SimTime::ZERO,
            GbeExternalInput::PixelClock {
                source: GbeExternalClock::Ttl,
                numerator_hz: 20_000_000,
                denominator: 1,
            },
        )
        .unwrap();
    let max_events = std::env::var("IP32_PROM_INPUT_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(20_000_000);
    let mut terminal = Vec::new();
    let mut events = 0;
    while events < max_events {
        let batch = (max_events - events).min(4_096);
        match machine.run_steps(batch) {
            Ok(RunStatus::Idle | RunStatus::Stopped) => {
                panic!("PROM became inactive before PS/2 initialization: {path}")
            }
            Ok(_) => {}
            Err(RunError::Dispatch(Ip32MachineDispatchError::Crime(CrimeError::Render(
                CrimeRenderError::UnsupportedPixelCommand {
                    primitive: 0x0100_0020,
                    draw_mode: 0x0008_02f8,
                    ..
                },
            )))) => break,
            Err(error) => panic!("PROM failed before PS/2 initialization: {error}"),
        }
        events += batch;
        drain_serial_one(&mut machine, &mut terminal);
        let registry = machine.runtime().registry();
        let keyboard = registry
            .get_typed::<Ps2Keyboard>(component_ids::KEYBOARD)
            .unwrap();
        if keyboard.scan_set() == 3 {
            break;
        }
    }

    drain_serial_one(&mut machine, &mut terminal);
    let terminal = String::from_utf8_lossy(&terminal);
    assert!(
        !terminal.contains("Cannot connect to keyboard -- check the cable."),
        "PROM reported a disconnected keyboard: {terminal}"
    );
    let registry = machine.runtime().registry();
    assert_eq!(
        registry
            .get_typed::<Ps2Keyboard>(component_ids::KEYBOARD)
            .unwrap()
            .scan_set(),
        3,
        "PROM did not select keyboard scan-code set 3: {path}"
    );
}

#[test]
#[ignore = "requires a local proprietary IP32 PROM image"]
fn local_ip32_prom_environment_uses_system_flash() {
    const ENV_START: u64 = 0x1fc0_4000;
    const ENV_END: u64 = 0x1fc0_4400;
    let path = std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name a local image");
    let config = Ip32MachineConfig {
        prom_image: std::fs::read(path).expect("the local PROM image must be readable"),
        ..Ip32MachineConfig::default()
    };
    let max_events = std::env::var("IP32_PROM_INTERACTION_EVENTS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(120_000_000);
    let mut machine =
        Ip32Machine::from_config_with_trace_sink(config.clone(), PromEnvironmentSink::default())
            .unwrap();
    machine.schedule_power_on().unwrap();

    let mut terminal = Vec::new();
    enter_command_monitor(&mut machine, &mut terminal, max_events);
    let rtc_before = machine
        .runtime()
        .registry()
        .get_typed::<Ds1687>(component_ids::RTC)
        .unwrap()
        .nvram_snapshot()
        .to_vec();

    terminal.clear();
    send_serial_one(&mut machine, b"setenv diagmode v\r");
    run_until_serial_one_contains(
        &mut machine,
        &mut terminal,
        b"> ",
        max_events,
        "the prompt after setenv",
    );
    let rtc_after = machine
        .runtime()
        .registry()
        .get_typed::<Ds1687>(component_ids::RTC)
        .unwrap()
        .nvram_snapshot();
    assert_eq!(rtc_after.as_slice(), rtc_before);

    let sink = machine.runtime().trace_recorder().sink();
    assert!(
        sink.failed_addresses.is_empty(),
        "setenv produced failed SysAD accesses: {:#x?}",
        sink.failed_addresses
    );
    let mut unique_writes = sink.flash_write_addresses.clone();
    unique_writes.sort_unstable();
    unique_writes.dedup();
    assert_eq!(unique_writes, (ENV_START..ENV_END).collect::<Vec<_>>());
    let flash = machine.system_flash_persistent_state().unwrap();
    assert!(!flash.changes().is_empty());
    assert!(flash.changes().iter().all(|change| {
        let start = se_device::chipset::mace::registers::PROM_START + change.offset();
        let end = start + change.bytes().len() as u64;
        start >= ENV_START && end <= ENV_END
    }));
    printenv_diagmode(&mut machine, &mut terminal, max_events);

    let saved = machine.save_state().unwrap();
    machine.hard_reset().unwrap();
    terminal.clear();
    enter_command_monitor(&mut machine, &mut terminal, max_events);
    printenv_diagmode(&mut machine, &mut terminal, max_events);

    let mut restored =
        Ip32Machine::from_state_with_trace_sink(config, saved, PromEnvironmentSink::default())
            .unwrap();
    terminal.clear();
    printenv_diagmode(&mut restored, &mut terminal, max_events);
}

#[cfg(feature = "jit")]
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
        interpreted_blocks: u64,
        native_blocks: u64,
    }

    fn run_sample(
        prom: &[u8],
        jit_enabled: bool,
        quantum: usize,
        inline: bool,
        event_chain_policy: Ip32EventChainPolicy,
        limit: Limit,
    ) -> Sample {
        let mut machine = Ip32Machine::from_config(Ip32MachineConfig {
            prom_image: prom.to_vec(),
            jit_enabled,
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
        let engine_statistics = machine
            .control
            .jit_engine
            .as_ref()
            .map(|engine| engine.statistics())
            .unwrap_or_default();
        Sample {
            elapsed: started.elapsed(),
            performance: machine.performance_snapshot(),
            interpreted_blocks: engine_statistics.interpreted_blocks,
            native_blocks: engine_statistics.native_blocks,
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
            "{label}/{mode}: median_elapsed={:?}, simulated_seconds={simulated_seconds:.6}, rtf={:.4}, instructions/s={:.0}, events/s={:.0}, events/instruction={:.3}, sysad={}, memory={}, cmi={}, cgi={}, interpreted-blocks={}, native-blocks={}, native-operations/block={:.3}, jit={:?}",
            sample.elapsed,
            simulated_seconds / host_seconds,
            instructions as f64 / host_seconds,
            events as f64 / host_seconds,
            events as f64 / instructions.max(1) as f64,
            sample.performance.sysad_transactions,
            sample.performance.memory_transactions,
            sample.performance.cmi_transactions,
            sample.performance.cgi_transactions,
            sample.interpreted_blocks,
            sample.native_blocks,
            sample.performance.jit.native_operations as f64 / sample.native_blocks.max(1) as f64,
            sample.performance.jit,
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
    let selected_limit = std::env::var("IP32_PROM_BENCH_LIMIT").ok();
    let jit_enabled = std::env::var("IP32_PROM_JIT")
        .ok()
        .is_some_and(|value| matches!(value.as_str(), "1" | "true" | "yes"));

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
            if selected_limit.as_deref().is_some_and(|limit| limit != mode) {
                continue;
            }
            let samples = (0..runs)
                .map(|_| {
                    run_sample(
                        &prom,
                        jit_enabled,
                        quantum,
                        inline,
                        event_chain_policy,
                        limit,
                    )
                })
                .collect();
            print_median(label, mode, samples);
        }
    }
}

#[cfg(feature = "jit")]
#[test]
#[ignore = "requires local proprietary IP32 PROM images and a release build"]
fn local_ip32_prom_jit_matches_scalar_through_4_2_billion_ticks() {
    const DEADLINE: SimTime = SimTime::new(4_200_000_000);
    let paths = std::env::var("IP32_PROM_PATHS")
        .ok()
        .map(|paths| {
            paths
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|paths| !paths.is_empty())
        .unwrap_or_else(|| {
            vec![
                std::env::var("IP32_PROM_PATH")
                    .expect("IP32_PROM_PATHS or IP32_PROM_PATH must name local images"),
            ]
        });

    for path in paths {
        let prom = std::fs::read(&path).expect("the local PROM image must be readable");
        let scalar_config = Ip32MachineConfig {
            prom_image: prom,
            ..Ip32MachineConfig::default()
        };
        let mut jit_config = scalar_config.clone();
        jit_config.jit_enabled = true;
        let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
        let mut jit = Ip32Machine::from_config(jit_config).unwrap();
        scalar.schedule_power_on().unwrap();
        jit.schedule_power_on().unwrap();

        assert_eq!(
            scalar.run_until_time(DEADLINE).unwrap(),
            RunStatus::DeadlineReached
        );
        assert_eq!(
            jit.run_until_time(DEADLINE).unwrap(),
            RunStatus::DeadlineReached
        );
        assert_machine_architecture_equal(&scalar, &jit);
        assert_eq!(
            scalar.control.first_serial_output_time, jit.control.first_serial_output_time,
            "first serial output time differs for {path}"
        );
        let scalar_performance = scalar.performance_snapshot();
        let jit_performance = jit.performance_snapshot();
        assert_eq!(scalar_performance.sim_time, jit_performance.sim_time);
        assert_eq!(
            scalar_performance.cpu.retired_instructions, jit_performance.cpu.retired_instructions,
            "retired instruction count differs for {path}"
        );
        assert_eq!(
            scalar_performance.cpu.transactions, jit_performance.cpu.transactions,
            "CPU transaction count differs for {path}"
        );
        assert_eq!(
            scalar_performance.cpu.exceptions, jit_performance.cpu.exceptions,
            "exception count differs for {path}"
        );
        assert_eq!(
            scalar_performance.sysad_transactions, jit_performance.sysad_transactions,
            "SysAD transaction count differs for {path}"
        );
        assert_eq!(
            scalar_performance.memory_transactions, jit_performance.memory_transactions,
            "memory transaction count differs for {path}"
        );
        assert_eq!(
            scalar_performance.cmi_transactions, jit_performance.cmi_transactions,
            "CMI transaction count differs for {path}"
        );
        assert_eq!(
            scalar_performance.cgi_transactions, jit_performance.cgi_transactions,
            "CGI transaction count differs for {path}"
        );
    }
}

#[cfg(feature = "jit")]
#[test]
#[ignore = "requires local proprietary IP32 PROM images and a release build"]
fn local_ip32_prom_jit_first_serial_acceptance() {
    #[derive(Clone)]
    struct Sample {
        elapsed: Duration,
        simulated_time: SimTime,
        byte: u8,
        retired_instructions: u64,
        cpu_transactions: u64,
        jit: Ip32JitPerformanceSnapshot,
    }

    fn run(prom: &[u8], jit_enabled: bool, max_events: usize) -> Sample {
        let mut machine = Ip32Machine::from_config(Ip32MachineConfig {
            prom_image: prom.to_vec(),
            jit_enabled,
            ..Ip32MachineConfig::default()
        })
        .unwrap();
        machine.schedule_power_on().unwrap();
        let event_batch = std::env::var("IP32_PROM_EVENT_BATCH")
            .ok()
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(4_096)
            .max(1);
        let started = Instant::now();
        let mut events = 0;
        while events < max_events {
            let batch = event_batch.min(max_events - events);
            match machine.run_steps(batch).unwrap() {
                RunStatus::Dispatched | RunStatus::StepLimitReached => {}
                status => panic!("PROM became inactive before serial output: {status:?}"),
            }
            events += batch;
            while let Some(output) = machine.poll_serial_output() {
                if output.port == Ip32SerialPort::Serial1
                    && let Some(byte) = output.bytes.first().copied()
                {
                    let performance = machine.performance_snapshot();
                    return Sample {
                        elapsed: started.elapsed(),
                        simulated_time: machine
                            .control
                            .first_serial_output_time
                            .expect("serial output must record its simulation time"),
                        byte,
                        retired_instructions: performance.cpu.retired_instructions,
                        cpu_transactions: performance.cpu.transactions,
                        jit: performance.jit,
                    };
                }
            }
        }
        panic!("PROM produced no serial output within {max_events} events");
    }

    fn median(mut samples: Vec<Sample>) -> Sample {
        samples.sort_by_key(|sample| sample.elapsed);
        let middle = samples.len() / 2;
        samples.remove(middle)
    }

    if cfg!(debug_assertions) {
        panic!("the first-serial JIT acceptance must run with --release");
    }
    let paths = std::env::var("IP32_PROM_PATHS")
        .ok()
        .map(|paths| {
            paths
                .split(',')
                .map(str::trim)
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .filter(|paths| !paths.is_empty())
        .unwrap_or_else(|| {
            vec![
                std::env::var("IP32_PROM_PATH")
                    .expect("IP32_PROM_PATHS or IP32_PROM_PATH must name local images"),
            ]
        });
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
    let max_events = std::env::var("IP32_PROM_EVENTS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20_000_000);

    for path in paths {
        let prom = std::fs::read(&path).expect("the local PROM image must be readable");
        let _ = run(&prom, false, max_events);
        let _ = run(&prom, true, max_events);
        let scalar = median((0..runs).map(|_| run(&prom, false, max_events)).collect());
        let jit = median((0..runs).map(|_| run(&prom, true, max_events)).collect());
        let speedup = scalar.elapsed.as_secs_f64() / jit.elapsed.as_secs_f64();
        let simulated_seconds = jit.simulated_time.get() as f64 / IP32_TIMEBASE_HZ as f64;
        let rtf = simulated_seconds / jit.elapsed.as_secs_f64();
        let region_operations_per_entry =
            jit.jit.region_operations as f64 / jit.jit.region_entries.max(1) as f64;
        let guest_operations = jit
            .jit
            .interpreted_operations
            .saturating_add(jit.jit.native_operations)
            .saturating_add(jit.jit.region_operations);
        let runtime_calls_per_operation =
            jit.jit.runtime_calls as f64 / guest_operations.max(1) as f64;
        let region_side_exits = jit
            .jit
            .region_cold_side_exits
            .saturating_add(jit.jit.region_budget_side_exits)
            .saturating_add(jit.jit.region_runtime_side_exits);
        eprintln!(
            "{path}: scalar={:?}@{} ticks, jit={:?}@{} ticks, rtf={rtf:.3}, speedup={speedup:.3}x, retired={}/{}, cpu-transactions={}/{}, region-operations/entry={region_operations_per_entry:.3}, runtime-calls/operation={runtime_calls_per_operation:.5}, jit={:?}",
            scalar.elapsed,
            scalar.simulated_time.get(),
            jit.elapsed,
            jit.simulated_time.get(),
            scalar.retired_instructions,
            jit.retired_instructions,
            scalar.cpu_transactions,
            jit.cpu_transactions,
            jit.jit,
        );
        assert_eq!(
            jit.byte, scalar.byte,
            "first serial byte differs for {path}"
        );
        assert_eq!(
            jit.simulated_time, scalar.simulated_time,
            "first serial simulated time differs for {path}"
        );
        assert!(
            rtf >= 1.5,
            "first serial output for {path} reached only RTF {rtf:.3}"
        );
        assert!(
            region_operations_per_entry >= 16.0,
            "first serial output for {path} averaged only {region_operations_per_entry:.3} Region operations per entry"
        );
        assert!(
            runtime_calls_per_operation < 0.10,
            "first serial output for {path} used {runtime_calls_per_operation:.5} runtime calls per guest operation"
        );
        assert_eq!(
            region_side_exits, jit.jit.region_entries,
            "first serial output for {path} did not classify every Region exit"
        );
        assert_eq!(
            jit.jit
                .system_flash_fetches
                .saturating_add(jit.jit.sdram_fetches),
            jit.jit.fast_fetches,
            "first serial output for {path} did not classify every stable code fetch"
        );
    }
}

#[cfg(feature = "jit")]
#[test]
#[ignore = "requires the local rev4.3 IP32 PROM image and a release build"]
fn local_ip32_prom_jit_instruction_acceptance() {
    #[derive(Clone)]
    struct Sample {
        elapsed: Duration,
        performance: Ip32PerformanceSnapshot,
    }

    fn run(prom: &[u8], target: u64) -> Sample {
        let mut machine = Ip32Machine::from_config(Ip32MachineConfig {
            prom_image: prom.to_vec(),
            jit_enabled: true,
            ..Ip32MachineConfig::default()
        })
        .unwrap();
        machine.schedule_power_on().unwrap();
        let started = Instant::now();
        loop {
            let retired = machine.performance_snapshot().cpu.retired_instructions;
            if retired >= target {
                break;
            }
            let events = if target - retired > 1_000_000 {
                4_096
            } else {
                1
            };
            match machine.run_steps(events).unwrap() {
                RunStatus::Dispatched | RunStatus::StepLimitReached => {}
                status => panic!(
                    "PROM became inactive before reaching {target} retired instructions: {status:?}"
                ),
            }
        }
        Sample {
            elapsed: started.elapsed(),
            performance: machine.performance_snapshot(),
        }
    }

    fn median(mut samples: Vec<Sample>) -> Sample {
        samples.sort_by_key(|sample| sample.elapsed);
        let middle = samples.len() / 2;
        samples.remove(middle)
    }

    if cfg!(debug_assertions) {
        panic!("the instruction-count JIT acceptance must run with --release");
    }
    const TARGET: u64 = 120_000_000;
    let path =
        std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name the local rev4.3 image");
    let prom = std::fs::read(&path).expect("the local PROM image must be readable");
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

    let _ = run(&prom, TARGET);
    let sample = median((0..runs).map(|_| run(&prom, TARGET)).collect());
    let simulated_seconds = sample.performance.sim_time.get() as f64 / IP32_TIMEBASE_HZ as f64;
    let rtf = simulated_seconds / sample.elapsed.as_secs_f64();
    let mips = sample.performance.cpu.retired_instructions as f64
        / sample.elapsed.as_secs_f64()
        / 1_000_000.0;
    eprintln!(
        "{path}: elapsed={:?}, simulated-ticks={}, retired={}, rtf={rtf:.3}, throughput={mips:.3} MIPS, jit={:?}",
        sample.elapsed,
        sample.performance.sim_time.get(),
        sample.performance.cpu.retired_instructions,
        sample.performance.jit,
    );
    assert!(
        rtf >= 1.5,
        "120,000,000-instruction benchmark reached only RTF {rtf:.3}"
    );
}
