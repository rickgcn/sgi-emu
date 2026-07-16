use se_core::role::{BusControllerRole, BusDeviceRole};
use se_core::tracing::TraceInterest;

use super::*;
use crate::chipset::crime::memory::CrimeSdram;
use crate::chipset::crime::protocol::{
    CrimeByteEnable, CrimeData, CrimeMemoryDiagnostic, CrimeTransferView,
};

const CRIME: ComponentId = ComponentId::new(1);
const RAM: ComponentId = ComponentId::new(2);
const MACE: ComponentId = ComponentId::new(3);
const GBE: ComponentId = ComponentId::new(4);
const TIMEBASE_HZ: u64 = 1_000_000_000;

fn crime() -> Crime {
    Crime::new(
        CRIME,
        "CRIME",
        CrimeConfig::default(),
        TIMEBASE_HZ,
        RAM,
        MACE,
        GBE,
    )
    .unwrap()
}

fn read(id: u128, address: u64, size: u16) -> CrimeSysAdRequest {
    CrimeSysAdRequest {
        id: CrimeTransactionId::new(id),
        time: SimTime::ZERO,
        address,
        transfer: CrimeTransfer::read(size),
    }
}

fn write(id: u128, address: u64, size: u16, value: u64) -> CrimeSysAdRequest {
    let length = usize::from(size);
    let bytes = value.to_be_bytes();
    CrimeSysAdRequest {
        id: CrimeTransactionId::new(id),
        time: SimTime::ZERO,
        address,
        transfer: CrimeTransfer::write(
            bytes[bytes.len() - length..].iter().copied().collect(),
            CrimeByteEnable::enabled(length),
        ),
    }
}

fn read_completion(id: u128, data: impl Into<CrimeData>) -> CrimeAction {
    CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
        id: CrimeTransactionId::new(id),
        result: Ok(CrimeCompletionPayload::ReadData(data.into())),
    })
}

fn write_completion(id: u128) -> CrimeAction {
    CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
        id: CrimeTransactionId::new(id),
        result: Ok(CrimeCompletionPayload::WriteComplete),
    })
}

fn next_non_trace(crime: &mut Crime) -> CrimeAction {
    loop {
        let CrimePoll::Action(action) = crime.poll().unwrap() else {
            panic!("CRIME became idle before producing an action");
        };
        if !matches!(action, CrimeAction::Trace(_)) {
            return action;
        }
    }
}

fn clear_actions(crime: &mut Crime) {
    while matches!(crime.poll(), Ok(CrimePoll::Action(_))) {}
}

#[test]
fn uninterested_crime_does_not_construct_trace_actions() {
    let mut crime = crime();
    crime.set_trace_interest(TraceInterest::None);
    crime.accept_sysad(read(1, registers::ID, 8)).unwrap();

    let mut saw_hardware_action = false;
    while let CrimePoll::Action(action) = crime.poll().unwrap() {
        assert!(!matches!(action, CrimeAction::Trace(_)));
        saw_hardware_action = true;
    }
    assert!(saw_hardware_action);
}

#[test]
fn crime_trace_targets_are_device_local() {
    let mut crime = crime();
    crime.accept_sysad(read(1, registers::ID, 8)).unwrap();

    let CrimePoll::Action(CrimeAction::Trace(event)) = crime.poll().unwrap() else {
        panic!("CRIME must trace an accepted SysAD request first");
    };
    assert_eq!(event.target, "crime.piu");
    assert_eq!(event.event, "sysad_request");
    assert!(!event.target.starts_with("ip32."));
}

fn retire_render_write(crime: &mut Crime, address: u64, size: u8, value: u64) {
    let progress = crime.render.write(address, size, value).unwrap();
    crime.apply_render_progress(progress).unwrap();
    crime.handle_event(
        SimTime::ZERO,
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );
    clear_actions(crime);
}

fn enable_interrupt_bit(crime: &mut Crime, bit: u8) {
    let effects = crime
        .piu
        .write(
            registers::INTERRUPT_ENABLE,
            1_u64 << bit,
            SimTime::ZERO,
            TIMEBASE_HZ,
        )
        .effects;
    crime.apply_piu_effects(effects);
}

fn memory_transaction(id: u128, address: u64, transfer: CrimeTransfer) -> CrimeMemoryTransaction {
    CrimeMemoryTransaction {
        id: CrimeTransactionId::new(id),
        time: SimTime::ZERO,
        controller: CRIME,
        client: CrimeMemoryClient::Render,
        address,
        bank_select: CrimeMemoryBankSelect::Decode,
        no_ecc: false,
        transfer,
    }
}

#[test]
fn crime_implements_all_required_topological_roles() {
    fn sysad_device<T: BusDeviceRole<CrimeSysAdRequest>>() {}
    fn memory_controller<T: BusControllerRole<CrimeMemoryCompletion>>() {}
    fn cmi_controller<T: BusControllerRole<CrimeCmiCompletion>>() {}
    fn cgi_controller<T: BusControllerRole<CrimeCgiCompletion>>() {}
    fn cmi_device<T: BusDeviceRole<CrimeCmiTransaction>>() {}
    fn cgi_device<T: BusDeviceRole<CrimeCgiTransaction>>() {}

    sysad_device::<Crime>();
    memory_controller::<Crime>();
    cmi_controller::<Crime>();
    cgi_controller::<Crime>();
    cmi_device::<Crime>();
    cgi_device::<Crime>();
}

#[test]
fn mace_interrupt_posts_drive_the_crime_irq_output() {
    let mut crime = crime();
    enable_interrupt_bit(&mut crime, 3);
    assert!(matches!(
        BusDeviceRole::accept(
            &mut crime,
            CrimeCmiTransaction {
                id: CrimeTransactionId::new(1),
                controller: MACE,
                target: CRIME,
                operation: CrimeLinkOperation::InterruptPost(CrimeInterruptPost {
                    interrupt_bit: 3,
                    asserted: true,
                }),
            }
        ),
        CrimeLinkDeviceResponse::Complete(CrimeCmiCompletion {
            result: Ok(CrimeCompletionPayload::WriteComplete),
            ..
        })
    ));
    assert_eq!(
        next_non_trace(&mut crime),
        CrimeAction::SetIrq(IrqTransaction {
            source: IrqSource {
                component: CRIME,
                output: CRIME_IRQ_OUTPUT,
            },
            asserted: true,
        })
    );
}

#[test]
fn gbe_interrupt_posts_drive_the_crime_irq_output() {
    let mut crime = crime();
    enable_interrupt_bit(&mut crime, 7);
    assert!(matches!(
        BusDeviceRole::accept(
            &mut crime,
            CrimeCgiTransaction {
                id: CrimeTransactionId::new(2),
                controller: GBE,
                target: CRIME,
                operation: CrimeLinkOperation::InterruptPost(CrimeInterruptPost {
                    interrupt_bit: 7,
                    asserted: true,
                }),
            }
        ),
        CrimeLinkDeviceResponse::Complete(CrimeCgiCompletion {
            result: Ok(CrimeCompletionPayload::WriteComplete),
            ..
        })
    ));
    assert_eq!(
        next_non_trace(&mut crime),
        CrimeAction::SetIrq(IrqTransaction {
            source: IrqSource {
                component: CRIME,
                output: CRIME_IRQ_OUTPUT,
            },
            asserted: true,
        })
    );
}

#[test]
fn doubleword_id_read_returns_crime_11_identity_in_big_endian_lanes() {
    let mut crime = crime();
    crime.accept(read(1, registers::ID, 8)).unwrap();

    assert_eq!(
        next_non_trace(&mut crime),
        read_completion(1, [0, 0, 0, 0, 0, 0, 0, 0xa1])
    );
}

#[test]
fn word_reads_select_big_endian_lanes_from_the_sysad_doubleword() {
    let mut crime = crime();
    crime.accept(read(1, registers::ID, 4)).unwrap();

    assert_eq!(next_non_trace(&mut crime), read_completion(1, [0; 4]));

    crime.accept(read(2, registers::ID + 4, 4)).unwrap();

    assert_eq!(
        next_non_trace(&mut crime),
        read_completion(2, [0, 0, 0, 0xa1])
    );
}

#[test]
fn word_writes_to_piu_remain_precise_bus_errors() {
    let mut crime = crime();
    crime
        .accept(write(1, registers::CONTROL + 4, 4, 0))
        .unwrap();

    assert!(matches!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
            result: Err(CrimeBusError::Access),
            ..
        })
    ));
}

#[test]
fn reserved_prom_write_sink_accepts_only_complete_aligned_doubleword_writes() {
    let mut device = crime();
    device
        .accept(write(1, registers::CPU_RESERVED_WRITE_SINK, 8, u64::MAX))
        .unwrap();

    assert_eq!(next_non_trace(&mut device), write_completion(1));
    assert_eq!(
        device
            .piu
            .read(registers::CPU_ERROR_ADDRESS, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
    assert_eq!(
        device
            .piu
            .read(registers::CPU_ERROR_STATUS, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
    assert!(!device.piu.interrupt_output_asserted());

    for request in [
        read(2, registers::CPU_RESERVED_WRITE_SINK, 8),
        write(3, registers::CPU_RESERVED_WRITE_SINK, 4, 0),
        write(4, registers::CPU_RESERVED_WRITE_SINK + 4, 8, 0),
        write(5, registers::CPU_RESERVED_WRITE_SINK + 8, 8, 0),
    ] {
        let mut candidate = crime();
        candidate.accept(request).unwrap();
        assert!(matches!(
            next_non_trace(&mut candidate),
            CrimeAction::CompleteSysAd(CrimeSysAdCompletion { result: Err(_), .. })
        ));
    }

    let mut request = write(6, registers::CPU_RESERVED_WRITE_SINK, 8, 0);
    request.transfer = CrimeTransfer::write(
        [0; 8].into(),
        [true, true, true, true, false, false, false, false].into(),
    );
    let mut candidate = crime();
    candidate.accept(request).unwrap();
    assert!(matches!(
        next_non_trace(&mut candidate),
        CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
            result: Err(CrimeBusError::Access),
            ..
        })
    ));
}

#[test]
fn sysad_rejects_invalid_transfer_shapes() {
    let mut crime = crime();
    for (id, transfer) in [
        (7, CrimeTransfer::write([0; 8].into(), [true; 4].into())),
        (8, CrimeTransfer::read(3)),
    ] {
        crime
            .accept(CrimeSysAdRequest {
                id: CrimeTransactionId::new(id),
                time: SimTime::ZERO,
                address: registers::CONTROL,
                transfer,
            })
            .unwrap();

        assert!(matches!(
            next_non_trace(&mut crime),
            CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
                id: completion_id,
                result: Err(CrimeBusError::Access),
            }) if completion_id == CrimeTransactionId::new(id)
        ));
    }
}

#[test]
fn cpu_error_status_clear_semantics_are_not_aliased_to_the_reserved_write_sink() {
    let mut crime = crime();
    enable_interrupt_bit(&mut crime, 20);
    crime.record_cpu_error(0x1234_5678);
    clear_actions(&mut crime);

    crime
        .accept(write(1, registers::CPU_RESERVED_WRITE_SINK, 8, 0))
        .unwrap();
    assert!(matches!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
            result: Ok(CrimeCompletionPayload::WriteComplete),
            ..
        })
    ));
    assert_eq!(
        crime
            .piu
            .read(registers::CPU_ERROR_STATUS, SimTime::ZERO, TIMEBASE_HZ),
        Some(registers::CPU_ERROR_ILLEGAL_ADDRESS)
    );

    crime
        .accept(write(2, registers::CPU_ERROR_STATUS, 8, 0))
        .unwrap();
    assert!(matches!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
            result: Ok(CrimeCompletionPayload::WriteComplete),
            ..
        })
    ));
    assert_eq!(
        crime
            .piu
            .read(registers::CPU_ERROR_STATUS, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
    assert!(!crime.piu.interrupt_output_asserted());
}

#[test]
fn full_render_fifo_defers_sysad_until_a_slot_is_retired() {
    let mut crime = crime();
    for _ in 0..64 {
        crime
            .render
            .write(registers::CRIME_RENDER_BASE + 0x2000, 4, 0)
            .unwrap();
    }
    crime.actions.clear();

    crime
        .accept(write(31, registers::CRIME_RENDER_BASE + 0x2000, 4, 1))
        .unwrap();
    assert_eq!(crime.pending_sysad, Some(CrimeTransactionId::new(31)));
    assert!(crime.pending_render_write.is_some());
    assert!(!crime.actions.iter().any(|action| matches!(
        action,
        CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
            id,
            result: Err(_),
        }) if *id == CrimeTransactionId::new(31)
    )));

    crime.handle_event(
        SimTime::new(1),
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );
    assert_eq!(crime.pending_sysad, None);
    assert_eq!(crime.render.interface_level(), 64);
    assert!(crime.actions.iter().any(|action| matches!(
        action,
        CrimeAction::CompleteSysAd(CrimeSysAdCompletion {
            id,
            result: Ok(CrimeCompletionPayload::WriteComplete),
        }) if *id == CrimeTransactionId::new(31)
    )));
}

#[test]
fn render_memory_fault_uses_the_miu_re_source_without_cpu_bus_error() {
    let mut crime = crime();
    let render_base = registers::CRIME_RENDER_BASE;
    retire_render_write(
        &mut crime,
        render_base + 0x1700,
        8,
        u64::from(0x0000_0001_u32) << 32,
    );
    retire_render_write(&mut crime, render_base + 0x3008, 4, u32::MAX.into());
    retire_render_write(&mut crime, render_base + 0x3018, 4, 0);
    retire_render_write(&mut crime, render_base + 0x3030, 4, 0);
    retire_render_write(&mut crime, render_base + 0x3038, 4, 0);

    let progress = crime.render.write(render_base + 0x3800, 4, 0x11).unwrap();
    crime.apply_render_progress(progress).unwrap();
    crime.handle_event(
        SimTime::new(1),
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );
    let memory = crime
        .actions
        .iter()
        .find_map(|action| match action {
            CrimeAction::StartMemory(transaction) => Some(transaction.clone()),
            _ => None,
        })
        .expect("the MTE must issue a memory transaction");
    assert_eq!(memory.client, CrimeMemoryClient::Render);
    assert_eq!(memory.address, 0x1000);
    assert_eq!(
        memory.bank_select,
        CrimeMemoryBankSelect::Inhibited {
            reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
        }
    );
    assert!(matches!(
        memory.transfer.view(),
        CrimeTransferView::Write { data, byte_enable }
            if data == [0]
                && byte_enable.len() == 1
                && byte_enable.is_enabled(0) == Some(true)
    ));
    crime.actions.clear();

    crime.complete(CrimeMemoryCompletion {
        id: memory.id,
        result: Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::WriteComplete,
            Some(CrimeMemoryFault::Address),
            Some(CrimeMemoryDiagnostic {
                address: memory.address,
                syndrome: 0,
                check: 0,
                corrected: false,
                write: true,
                read_modify_write: false,
            }),
        )),
    });
    assert_eq!(
        crime.memory_error_status,
        (1 << 15) | registers::MEMORY_ERROR_INVALID_WRITE
    );
    assert_eq!(
        crime
            .piu
            .read(registers::CPU_ERROR_STATUS, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
    assert!(crime.terminal_error.is_none());
}

#[test]
fn diagnostic_x_line_reaches_the_crime_memory_domain() {
    let mut crime = crime();
    let render_base = registers::CRIME_RENDER_BASE;
    retire_render_write(
        &mut crime,
        render_base + 0x1200,
        8,
        u64::from(0x8001_u16) << 48,
    );
    for (offset, value) in [
        (0x2000, 0x0000_0628),
        (0x2008, 0x0000_0628),
        (0x2018, 0x0000_02f8),
        (0x2060, 0x0100_0020),
        (0x20d0, 0x1122_3344),
        (0x21b0, 3),
        (0x21b8, u32::MAX),
        (0x2070, 0),
        (0x2074, 7 << 16),
    ] {
        retire_render_write(&mut crime, render_base + offset, 4, value.into());
    }

    let progress = crime.render.write(render_base + 0x29f0, 4, 0).unwrap();
    crime.apply_render_progress(progress).unwrap();
    crime.handle_event(
        SimTime::new(1),
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );
    let memory = crime
        .actions
        .iter()
        .find_map(|action| match action {
            CrimeAction::StartMemory(transaction) => Some(transaction),
            _ => None,
        })
        .expect("the X line must issue a memory transaction");
    assert_eq!(memory.client, CrimeMemoryClient::Render);
    assert_eq!(memory.address, 0x1_0000);
    assert_eq!(memory.bank_select, CrimeMemoryBankSelect::Decode);
    assert!(!memory.no_ecc);
    assert!(matches!(
        memory.transfer.view(),
        CrimeTransferView::Write { data, byte_enable }
            if data == [0x11, 0x22, 0x33, 0x44].repeat(8)
                && byte_enable.iter().all(|enabled| enabled)
    ));
    assert!(crime.terminal_error.is_none());
}

#[test]
fn prom_ci8_zero_rectangle_reaches_the_crime_memory_domain() {
    let mut crime = crime();
    let render_base = registers::CRIME_RENDER_BASE;
    retire_render_write(
        &mut crime,
        render_base + 0x1000,
        8,
        u64::from(0x8002_u16) << 48,
    );
    for (offset, value) in [
        (0x20d0, 0),
        (0x2018, 0x0000_00f8),
        (0x21b8, u32::MAX),
        (0x2070, 0),
        (0x2074, 63 << 16 | 1),
        (0x2060, 0x0302_0000),
    ] {
        retire_render_write(&mut crime, render_base + offset, 4, value.into());
    }

    let progress = crime.render.write(render_base + 0x29f0, 4, 0).unwrap();
    crime.apply_render_progress(progress).unwrap();
    crime.handle_event(
        SimTime::new(1),
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );
    let memory = crime
        .actions
        .iter()
        .find_map(|action| match action {
            CrimeAction::StartMemory(transaction) => Some(transaction),
            _ => None,
        })
        .expect("the PROM rectangle must issue a memory transaction");
    assert_eq!(memory.client, CrimeMemoryClient::Render);
    assert_eq!(memory.address, 0x2_0000);
    assert_eq!(memory.bank_select, CrimeMemoryBankSelect::Decode);
    assert!(!memory.no_ecc);
    assert!(matches!(
        memory.transfer.view(),
        CrimeTransferView::Write { data, byte_enable }
            if data == [0; 32] && byte_enable.iter().all(|enabled| enabled)
    ));
    assert!(crime.terminal_error.is_none());
}

#[test]
fn prom_ci8_flat_rectangle_reaches_memory_with_the_color_index() {
    let mut crime = crime();
    let render_base = registers::CRIME_RENDER_BASE;
    retire_render_write(
        &mut crime,
        render_base + 0x1000,
        8,
        u64::from(0x8002_u16) << 48,
    );
    for (offset, value) in [
        (0x20d0, 0x7b7b_7b7b),
        (0x2018, 0x0000_02f8),
        (0x21b0, 3),
        (0x21b8, u32::MAX),
        (0x2070, 0),
        (0x2074, 31 << 16),
        (0x2060, 0x0302_0000),
    ] {
        retire_render_write(&mut crime, render_base + offset, 4, value.into());
    }

    let progress = crime.render.write(render_base + 0x29f0, 4, 0).unwrap();
    crime.apply_render_progress(progress).unwrap();
    crime.handle_event(
        SimTime::new(1),
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );
    let memory = crime
        .actions
        .iter()
        .find_map(|action| match action {
            CrimeAction::StartMemory(transaction) => Some(transaction),
            _ => None,
        })
        .expect("the PROM flat rectangle must issue a memory transaction");
    assert!(matches!(
        memory.transfer.view(),
        CrimeTransferView::Write { data, byte_enable }
            if data == [0x7b; 32] && byte_enable.iter().all(|enabled| enabled)
    ));
    assert!(crime.terminal_error.is_none());
}

#[test]
fn prom_linear_tlb_sequence_completes_through_miu_faults() {
    let config = CrimeConfig::default();
    let mut crime = Crime::new(CRIME, "CRIME", config, TIMEBASE_HZ, RAM, MACE, GBE).unwrap();
    let mut memory = CrimeSdram::new(RAM, "RAM", config.memory);

    for (index, address) in [0x1000, 0x3000, 0x5000, 0x7000].into_iter().enumerate() {
        let completion = memory.accept(memory_transaction(
            0x100 + index as u128,
            address,
            CrimeTransfer::write(vec![0xa5; 8].into(), vec![true; 8].into()),
        ));
        assert_eq!(completion.result.unwrap().fault, None);
    }

    let render_base = registers::CRIME_RENDER_BASE;
    for (index, entry) in [
        0xffff_ffff_8004_0001,
        0xffff_ffff_8004_0003,
        0xffff_ffff_8004_0005,
        0xffff_ffff_8004_0007,
    ]
    .into_iter()
    .enumerate()
    {
        retire_render_write(
            &mut crime,
            render_base + 0x1700 + index as u64 * 8,
            8,
            entry,
        );
    }
    retire_render_write(&mut crime, render_base + 0x3008, 4, u32::MAX.into());
    retire_render_write(&mut crime, render_base + 0x3018, 4, 0);
    retire_render_write(&mut crime, render_base + 0x3030, 4, 0x4000_0000);
    retire_render_write(&mut crime, render_base + 0x3038, 4, 0x4000_7fff);
    enable_interrupt_bit(&mut crime, 21);

    let progress = crime.render.write(render_base + 0x3800, 4, 0x11).unwrap();
    crime.apply_render_progress(progress).unwrap();
    crime.handle_event(
        SimTime::new(1),
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );

    for step in 0..1024 {
        let Some(action) = crime.actions.pop_front() else {
            assert_ne!(
                crime.render.read(render_base + 0x4000, 4).unwrap() as u32 & 0x1000_0000,
                0
            );
            break;
        };
        match action {
            CrimeAction::StartMemory(transaction) => {
                let completion = memory.accept(transaction);
                crime.complete(completion);
            }
            CrimeAction::Schedule { event, .. } => {
                crime.handle_event(SimTime::new(step + 2), event);
            }
            CrimeAction::Trace(_) | CrimeAction::SetIrq(_) => {}
            action => panic!("unexpected CRIME action while driving MTE: {action:?}"),
        }
    }

    assert!(crime.terminal_error.is_none());
    assert_eq!(
        crime.memory_error_status,
        (1 << 15) | registers::MEMORY_ERROR_INVALID_WRITE
    );
    assert_eq!(crime.memory_error_address, 0x3fff_f000);
    assert_eq!(
        crime
            .piu
            .read(registers::CPU_ERROR_STATUS, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
    assert_ne!(
        crime
            .piu
            .read(registers::INTERRUPT_STATUS, SimTime::ZERO, TIMEBASE_HZ)
            .unwrap()
            & u64::from(registers::INTERRUPT_MEMORY_ERROR),
        0
    );

    for (index, address) in [0x1000, 0x3000, 0x5000, 0x7000].into_iter().enumerate() {
        let completion = memory.accept(memory_transaction(
            0x200 + index as u128,
            address,
            CrimeTransfer::read(8),
        ));
        assert_eq!(
            completion.result.unwrap().payload,
            CrimeCompletionPayload::ReadData(vec![0; 8].into())
        );
    }
}

#[test]
fn render_memory_transport_failure_is_terminal_and_traced() {
    let mut crime = crime();
    let render_base = registers::CRIME_RENDER_BASE;
    retire_render_write(
        &mut crime,
        render_base + 0x1700,
        8,
        u64::from(0x8000_0001_u32) << 32,
    );
    retire_render_write(&mut crime, render_base + 0x3008, 4, u32::MAX.into());
    retire_render_write(&mut crime, render_base + 0x3018, 4, 0);
    retire_render_write(&mut crime, render_base + 0x3030, 4, 0);
    retire_render_write(&mut crime, render_base + 0x3038, 4, 0);
    let progress = crime.render.write(render_base + 0x3800, 4, 0x11).unwrap();
    crime.apply_render_progress(progress).unwrap();
    crime.handle_event(
        SimTime::new(1),
        CrimeEvent::RenderStep {
            epoch: crime.render.epoch(),
        },
    );
    let memory = crime
        .actions
        .iter()
        .find_map(|action| match action {
            CrimeAction::StartMemory(transaction) => Some(transaction.clone()),
            _ => None,
        })
        .unwrap();
    crime.actions.clear();

    crime.complete(CrimeMemoryCompletion {
        id: memory.id,
        result: Err(CrimeBusError::Timeout),
    });
    assert!(matches!(
        crime.actions.front(),
        Some(CrimeAction::Trace(event))
            if event.target == "crime.re" && event.event == "render_error"
    ));
    clear_actions(&mut crime);
    assert_eq!(
        crime.poll(),
        Err(CrimeError::Render(CrimeRenderError::MemoryTransport(
            CrimeBusError::Timeout
        )))
    );
}

#[test]
fn memory_request_is_deferred_until_the_matching_bus_completion() {
    let mut crime = crime();
    crime.accept(read(9, LINEAR_MEMORY_START, 4)).unwrap();
    let CrimeAction::StartMemory(transaction) = next_non_trace(&mut crime) else {
        panic!("expected a memory transaction");
    };
    assert_eq!(crime.poll().unwrap(), CrimePoll::Idle);

    crime.complete(CrimeMemoryCompletion {
        id: transaction.id,
        result: Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(vec![1, 2, 3, 4].into()),
            None,
            None,
        )),
    });
    assert_eq!(next_non_trace(&mut crime), read_completion(9, [1, 2, 3, 4]));
}

#[test]
fn memory_address_fault_updates_miu_and_completes_sysad_without_cpu_error() {
    let mut crime = crime();
    enable_interrupt_bit(&mut crime, 21);
    crime.accept(read(11, LINEAR_MEMORY_START, 8)).unwrap();
    let CrimeAction::StartMemory(transaction) = next_non_trace(&mut crime) else {
        panic!("expected a memory transaction");
    };
    crime.complete(CrimeMemoryCompletion {
        id: transaction.id,
        result: Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(vec![0; 8].into()),
            Some(CrimeMemoryFault::Address),
            Some(CrimeMemoryDiagnostic {
                address: 0x1000_0000,
                syndrome: 0,
                check: 0,
                corrected: false,
                write: false,
                read_modify_write: false,
            }),
        )),
    });

    assert!(matches!(next_non_trace(&mut crime), CrimeAction::SetIrq(_)));
    assert_eq!(next_non_trace(&mut crime), read_completion(11, [0; 8]));
    assert_eq!(
        crime.memory_error_status,
        registers::MEMORY_ERROR_CPU_ACCESS | registers::MEMORY_ERROR_INVALID_READ
    );
    assert_eq!(crime.memory_error_address, 0x1000_0000);
    assert_eq!(
        crime
            .piu
            .read(registers::CPU_ERROR_STATUS, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
    assert_eq!(
        crime
            .piu
            .read(registers::CPU_ERROR_ADDRESS, SimTime::ZERO, TIMEBASE_HZ),
        Some(0)
    );
}

#[test]
fn dma_memory_faults_are_separate_from_cmi_and_cgi_transport_results() {
    let mut crime = crime();
    let request = CrimeDmaRequest {
        address: 0x2000_0000,
        transfer: CrimeTransfer::read(8),
    };

    assert_eq!(
        BusDeviceRole::accept(
            &mut crime,
            CrimeCmiTransaction {
                id: CrimeTransactionId::new(21),
                controller: MACE,
                target: CRIME,
                operation: CrimeLinkOperation::Dma(request.clone()),
            }
        ),
        CrimeLinkDeviceResponse::Deferred
    );
    let CrimeAction::StartMemory(cmi_memory) = next_non_trace(&mut crime) else {
        panic!("expected CMI memory transaction");
    };
    crime.complete(CrimeMemoryCompletion {
        id: cmi_memory.id,
        result: Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(vec![0; 8].into()),
            Some(CrimeMemoryFault::Address),
            Some(CrimeMemoryDiagnostic {
                address: request.address,
                syndrome: 0,
                check: 0,
                corrected: false,
                write: false,
                read_modify_write: false,
            }),
        )),
    });
    assert!(matches!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteCmiDevice(CrimeCmiCompletion {
            id,
            result: Ok(CrimeCompletionPayload::ReadData(data)),
            memory_fault: Some(CrimeMemoryFault::Address),
        }) if id == CrimeTransactionId::new(21) && data == vec![0; 8]
    ));

    assert_eq!(
        BusDeviceRole::accept(
            &mut crime,
            CrimeCgiTransaction {
                id: CrimeTransactionId::new(22),
                controller: GBE,
                target: CRIME,
                operation: CrimeLinkOperation::Dma(request.clone()),
            }
        ),
        CrimeLinkDeviceResponse::Deferred
    );
    let CrimeAction::StartMemory(cgi_memory) = next_non_trace(&mut crime) else {
        panic!("expected CGI memory transaction");
    };
    crime.complete(CrimeMemoryCompletion {
        id: cgi_memory.id,
        result: Ok(CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(vec![0; 8].into()),
            Some(CrimeMemoryFault::UncorrectableEcc),
            Some(CrimeMemoryDiagnostic {
                address: request.address,
                syndrome: 1,
                check: 2,
                corrected: false,
                write: false,
                read_modify_write: false,
            }),
        )),
    });
    assert!(matches!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteCgiDevice(CrimeCgiCompletion {
            id,
            result: Ok(CrimeCompletionPayload::ReadData(data)),
            memory_fault: Some(CrimeMemoryFault::UncorrectableEcc),
        }) if id == CrimeTransactionId::new(22) && data == vec![0; 8]
    ));
}

#[test]
fn memory_error_latching_preserves_first_hard_error_and_priority() {
    let mut crime = crime();
    let origin = PendingMemoryOrigin::CmiDma {
        link_id: CrimeTransactionId::new(1),
    };
    let outcome = |address, fault, corrected, syndrome| {
        CrimeMemoryOutcome::new(
            CrimeCompletionPayload::ReadData(vec![0; 8].into()),
            fault,
            Some(CrimeMemoryDiagnostic {
                address,
                syndrome,
                check: syndrome,
                corrected,
                write: false,
                read_modify_write: false,
            }),
        )
    };

    crime.record_memory_diagnostic(
        &origin,
        &outcome(0x100, Some(CrimeMemoryFault::Address), false, 0),
    );
    assert_eq!(crime.memory_error_address, 0x100);
    crime.record_memory_diagnostic(&origin, &outcome(0x200, None, true, 1));
    assert_eq!(crime.memory_error_address, 0x200);
    crime.record_memory_diagnostic(
        &origin,
        &outcome(0x300, Some(CrimeMemoryFault::UncorrectableEcc), false, 2),
    );
    assert_eq!(crime.memory_error_address, 0x300);
    crime.record_memory_diagnostic(
        &origin,
        &outcome(0x400, Some(CrimeMemoryFault::UncorrectableEcc), false, 3),
    );
    assert_eq!(crime.memory_error_address, 0x300);
    assert_eq!(crime.memory_ecc_syndrome, 2);
    assert_ne!(
        crime.memory_error_status & registers::MEMORY_ERROR_MULTIPLE,
        0
    );
}
