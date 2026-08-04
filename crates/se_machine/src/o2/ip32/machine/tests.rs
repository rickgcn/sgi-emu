#[cfg(feature = "jit")]
use std::time::Instant;

use crate::o2::ip32::component_ids;
use se_core::role::BusDeviceRole;
use se_core::scheduler::SimTime;
use se_device::bus::isa::{IsaDeviceResponse, IsaTransaction, IsaTransactionId, IsaTransfer};
use se_device::cpu::mips4::model::r5000::cpu::R5000Cpu;
use se_device::memory::flash::SystemFlash;

use super::*;

const HOT_LOOP_OFFSET: usize = 0x20;

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

fn hot_loop_config(jit_enabled: bool) -> Ip32RuntimeConfig {
    let loop_address = 0xffff_ffff_9fc0_0000_u64 + HOT_LOOP_OFFSET as u64;
    let jump_loop = (2_u32 << 26) | (((loop_address as u32) >> 2) & 0x03ff_ffff);
    let program = [
        (0x00, i_type(0x0f, 0, 1, 0x9fc0)),
        (0x04, i_type(0x0d, 1, 1, HOT_LOOP_OFFSET as u16)),
        (0x08, i_type(0x0d, 0, 2, 3)),
        (0x0c, (0x10_u32 << 26) | (4 << 21) | (2 << 16) | (16 << 11)),
        (0x10, r_type(1, 0, 0, 0, 0x08)),
        (0x14, 0),
        (HOT_LOOP_OFFSET, i_type(0x09, 2, 2, 1)),
        (HOT_LOOP_OFFSET + 4, jump_loop),
        (HOT_LOOP_OFFSET + 8, 0),
    ];
    let mut config = Ip32RuntimeConfig {
        jit_enabled,
        ..Ip32RuntimeConfig::default()
    };
    for (offset, instruction) in program {
        config.machine.prom_image[offset..offset + 4].copy_from_slice(&instruction.to_be_bytes());
    }
    config
}

fn run_to_retired(runtime: &mut Ip32Machine, target: u64) {
    if runtime.runtime().scheduler().is_empty() {
        runtime.schedule_power_on().unwrap();
    }
    while runtime.performance_snapshot().cpu.retired_instructions < target {
        let retired = runtime.performance_snapshot().cpu.retired_instructions;
        runtime.control.cpu_continuation_quantum =
            usize::try_from((target - retired).min(DEFAULT_CPU_CONTINUATION_QUANTUM as u64))
                .unwrap();
        match runtime.run_steps(1).unwrap() {
            RunStatus::Dispatched | RunStatus::StepLimitReached => {}
            status => panic!("runtime stopped before {target} instructions: {status:?}"),
        }
    }
    assert_eq!(
        runtime.performance_snapshot().cpu.retired_instructions,
        target
    );
}

fn encoded_state(runtime: &Ip32Machine) -> Vec<u8> {
    postcard::to_stdvec(&runtime.save_state().unwrap()).unwrap()
}

#[test]
fn default_machine_starts_at_zero_with_fixed_topology() {
    let runtime = Ip32Machine::new();

    assert_eq!(runtime.runtime().now(), SimTime::ZERO);
    assert!(
        runtime
            .runtime()
            .registry()
            .get_typed::<R5000Cpu>(component_ids::CPU0)
            .is_ok()
    );
    assert!(
        runtime
            .runtime()
            .registry()
            .get(component_ids::CRIME_MEMORY_DOMAIN)
            .is_none()
    );
}

#[test]
fn functional_loop_does_not_schedule_bus_phase_events() {
    let mut runtime = Ip32Machine::from_config(hot_loop_config(false)).unwrap();
    run_to_retired(&mut runtime, 20_000);

    let performance = runtime.performance_snapshot();
    assert!(performance.sysad_transactions > 0);
    assert!(performance.cmi_transactions > 0);
    assert!(performance.runtime.dispatched_events < 64);
    assert!(performance.runtime.scheduled_events < 64);
}

#[test]
fn scalar_execution_is_reproducible() {
    let config = hot_loop_config(false);
    let mut first = Ip32Machine::from_config(config.clone()).unwrap();
    let mut second = Ip32Machine::from_config(config).unwrap();

    run_to_retired(&mut first, 50_000);
    run_to_retired(&mut second, 50_000);

    assert_eq!(encoded_state(&first), encoded_state(&second));
}

#[test]
fn exact_state_round_trip_continues_deterministically() {
    let config = hot_loop_config(false);
    let mut reference = Ip32Machine::from_config(config.clone()).unwrap();
    run_to_retired(&mut reference, 20_000);
    let state = reference.save_state().unwrap();
    let encoded = postcard::to_stdvec(&state).unwrap();
    let decoded: Ip32MachineState = postcard::from_bytes(&encoded).unwrap();
    let mut restored =
        Ip32Machine::from_state_with_trace_sink(config, decoded, se_core::tracing::NoopTraceSink)
            .unwrap();

    run_to_retired(&mut reference, 40_000);
    run_to_retired(&mut restored, 40_000);

    assert_eq!(encoded_state(&reference), encoded_state(&restored));
}

#[test]
fn rtc_and_system_flash_persistence_round_trip_independently() {
    let mut nvram = se_device::rtc::ds1687::Ds1687Config::default().nvram;
    nvram[0x40] = 0xa5;
    let source_config = Ip32RuntimeConfig {
        machine: Ip32MachineConfig {
            rtc: se_device::rtc::ds1687::Ds1687Config {
                initial_unix_seconds: 1_700_000_000,
                nvram,
            },
            ..Ip32MachineConfig::default()
        },
        ..Ip32RuntimeConfig::default()
    };
    let mut source = Ip32Machine::from_config(source_config).unwrap();
    let now = source.runtime.now();
    let response = source
        .runtime
        .registry_mut()
        .get_typed_mut::<SystemFlash>(component_ids::PROM)
        .unwrap()
        .accept(IsaTransaction {
            id: IsaTransactionId::new(1),
            time: now,
            controller: component_ids::ISA_BUS,
            target: component_ids::PROM,
            address: 0x100,
            transfer: IsaTransfer::write([0x5a].into(), [true].into()),
        });
    assert!(matches!(
        response,
        IsaDeviceResponse::Complete(completion) if completion.result.is_ok()
    ));

    let rtc = source.rtc_persistent_state().unwrap();
    let flash = source.system_flash_persistent_state().unwrap();
    assert_eq!(flash.changes().len(), 1);

    let mut restored = Ip32Machine::new();
    restored.restore_rtc_persistent_state(&rtc).unwrap();
    restored
        .restore_system_flash_persistent_state(&flash)
        .unwrap();

    assert_eq!(restored.rtc_persistent_state().unwrap(), rtc);
    assert_eq!(restored.system_flash_persistent_state().unwrap(), flash);
}

#[cfg(feature = "jit")]
#[test]
fn jit_and_scalar_share_the_same_architectural_path() {
    let scalar_config = hot_loop_config(false);
    let jit_config = hot_loop_config(true);
    let mut scalar = Ip32Machine::from_config(scalar_config).unwrap();
    let mut jit = Ip32Machine::from_config(jit_config).unwrap();

    run_to_retired(&mut scalar, 100_000);
    run_to_retired(&mut jit, 500);
    jit.control
        .jit_engine
        .as_mut()
        .unwrap()
        .finish_compilations()
        .unwrap();
    run_to_retired(&mut jit, 100_000);

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
    let scalar_state = scalar_cpu.state();
    let jit_state = jit_cpu.state();
    assert_eq!(scalar.runtime().now(), jit.runtime().now());
    assert_eq!(scalar_state.pc(), jit_state.pc());
    assert_eq!(scalar_state.next_pc(), jit_state.next_pc());
    assert_eq!(
        scalar_state.delay_slot_branch_pc(),
        jit_state.delay_slot_branch_pc()
    );
    assert_eq!(scalar_state.gpr(), jit_state.gpr());
    assert_eq!(scalar_state.hi(), jit_state.hi());
    assert_eq!(scalar_state.lo(), jit_state.lo());
    assert_eq!(scalar_state.cp0(), jit_state.cp0());
    assert_eq!(scalar_state.cp1(), jit_state.cp1());
    assert_eq!(scalar_state.tlb_entries(), jit_state.tlb_entries());
    assert_eq!(scalar_state.llbit(), jit_state.llbit());
    assert_eq!(
        scalar_state.external_interrupts(),
        jit_state.external_interrupts()
    );
    let scalar_performance = scalar.performance_snapshot();
    let jit_performance = jit.performance_snapshot();
    assert_eq!(scalar_performance.cpu, jit_performance.cpu);
    assert!(jit_performance.jit.native_operations + jit_performance.jit.region_operations > 0);
}

#[test]
fn invalid_prom_size_is_rejected() {
    let mut prom_image = Ip32MachineConfig::default().prom_image;
    prom_image.pop();
    let config = Ip32RuntimeConfig {
        machine: Ip32MachineConfig {
            prom_image,
            ..Ip32MachineConfig::default()
        },
        ..Ip32RuntimeConfig::default()
    };

    assert!(matches!(
        Ip32Machine::from_config(config),
        Err(Ip32RuntimeBuildError::InvalidPromSize { .. })
    ));
}

#[cfg(not(feature = "jit"))]
#[test]
fn unavailable_jit_is_rejected() {
    let config = Ip32RuntimeConfig {
        jit_enabled: true,
        ..Ip32RuntimeConfig::default()
    };

    assert!(matches!(
        Ip32Machine::from_config(config),
        Err(Ip32RuntimeBuildError::JitUnavailable)
    ));
}

#[cfg(feature = "jit")]
#[test]
#[ignore = "requires a local IP32 PROM image and a release build"]
fn local_prom_instruction_throughput() {
    if cfg!(debug_assertions) {
        panic!("the throughput check must run with --release");
    }
    let path = std::env::var("IP32_PROM_PATH").expect("IP32_PROM_PATH must name an IP32 PROM");
    let target = std::env::var("IP32_PROM_INSTRUCTIONS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(1_000_000);
    let config = Ip32RuntimeConfig {
        machine: Ip32MachineConfig {
            prom_image: std::fs::read(path).unwrap(),
            ..Ip32MachineConfig::default()
        },
        jit_enabled: true,
    };
    let mut runtime = Ip32Machine::from_config(config).unwrap();

    let started = Instant::now();
    run_to_retired(&mut runtime, target);
    let elapsed = started.elapsed();
    let performance = runtime.performance_snapshot();
    let instructions_per_second = target as f64 / elapsed.as_secs_f64();
    eprintln!(
        "elapsed={elapsed:?}, instructions/s={instructions_per_second:.0}, events={}, sysad={}, memory={}, cmi={}, cgi={}, jit={:?}",
        performance.runtime.dispatched_events,
        performance.sysad_transactions,
        performance.memory_transactions,
        performance.cmi_transactions,
        performance.cgi_transactions,
        performance.jit,
    );
}
