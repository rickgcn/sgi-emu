use se_core::role::{BusControllerRole, BusDeviceRole};

use super::*;
use crate::chipset::crime::protocol::CrimeMemoryDiagnostic;
use crate::cpu::execution::protocol::{ExecutionTransaction, ExecutionTransactionId};
use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};

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

fn read(id: u128, address: u64, size: Mips4ExecutionTransferSize) -> CrimeSysAdRequest {
    CrimeSysAdRequest {
        time: SimTime::ZERO,
        transaction: ExecutionTransaction {
            id: ExecutionTransactionId::new(id),
            payload: Mips4ExecutionTransaction::Read {
                physical_address: address,
                size,
                kind: Mips4ExecutionAccessKind::DataLoad,
                access_type: Mips4MemoryAccessType::Uncached,
            },
        },
    }
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
    crime
        .accept(read(
            1,
            registers::ID,
            Mips4ExecutionTransferSize::Doubleword,
        ))
        .unwrap();

    assert_eq!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(ExecutionCompletion {
            id: ExecutionTransactionId::new(1),
            payload: Mips4ExecutionCompletion::ReadData(0xa100_0000_0000_0000),
        })
    );
}

#[test]
fn word_access_to_piu_is_a_precise_bus_error() {
    let mut crime = crime();
    crime
        .accept(read(1, registers::ID, Mips4ExecutionTransferSize::Word))
        .unwrap();

    assert!(matches!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(ExecutionCompletion {
            payload: Mips4ExecutionCompletion::BusError,
            ..
        })
    ));
}

#[test]
fn memory_request_is_deferred_until_the_matching_bus_completion() {
    let mut crime = crime();
    crime
        .accept(read(
            9,
            LINEAR_MEMORY_START,
            Mips4ExecutionTransferSize::Word,
        ))
        .unwrap();
    let CrimeAction::StartMemory(transaction) = next_non_trace(&mut crime) else {
        panic!("expected a memory transaction");
    };
    assert_eq!(crime.poll().unwrap(), CrimePoll::Idle);

    crime.complete(CrimeMemoryCompletion {
        id: transaction.id,
        result: Ok(CrimeMemoryOutcome {
            payload: CrimeCompletionPayload::ReadData(vec![1, 2, 3, 4]),
            fault: None,
            diagnostic: None,
        }),
    });
    assert_eq!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(ExecutionCompletion {
            id: ExecutionTransactionId::new(9),
            payload: Mips4ExecutionCompletion::ReadData(0x0403_0201),
        })
    );
}

#[test]
fn memory_address_fault_updates_miu_and_completes_sysad_without_cpu_error() {
    let mut crime = crime();
    enable_interrupt_bit(&mut crime, 21);
    crime
        .accept(read(
            11,
            LINEAR_MEMORY_START,
            Mips4ExecutionTransferSize::Doubleword,
        ))
        .unwrap();
    let CrimeAction::StartMemory(transaction) = next_non_trace(&mut crime) else {
        panic!("expected a memory transaction");
    };
    crime.complete(CrimeMemoryCompletion {
        id: transaction.id,
        result: Ok(CrimeMemoryOutcome {
            payload: CrimeCompletionPayload::ReadData(vec![0; 8]),
            fault: Some(CrimeMemoryFault::Address),
            diagnostic: Some(CrimeMemoryDiagnostic {
                address: 0x1000_0000,
                syndrome: 0,
                check: 0,
                corrected: false,
                write: false,
                read_modify_write: false,
            }),
        }),
    });

    assert!(matches!(next_non_trace(&mut crime), CrimeAction::SetIrq(_)));
    assert_eq!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(ExecutionCompletion {
            id: ExecutionTransactionId::new(11),
            payload: Mips4ExecutionCompletion::ReadData(0),
        })
    );
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
        transfer: CrimeTransfer::Read { length: 8 },
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
        result: Ok(CrimeMemoryOutcome {
            payload: CrimeCompletionPayload::ReadData(vec![0; 8]),
            fault: Some(CrimeMemoryFault::Address),
            diagnostic: Some(CrimeMemoryDiagnostic {
                address: request.address,
                syndrome: 0,
                check: 0,
                corrected: false,
                write: false,
                read_modify_write: false,
            }),
        }),
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
        result: Ok(CrimeMemoryOutcome {
            payload: CrimeCompletionPayload::ReadData(vec![0; 8]),
            fault: Some(CrimeMemoryFault::UncorrectableEcc),
            diagnostic: Some(CrimeMemoryDiagnostic {
                address: request.address,
                syndrome: 1,
                check: 2,
                corrected: false,
                write: false,
                read_modify_write: false,
            }),
        }),
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
    let outcome = |address, fault, corrected, syndrome| CrimeMemoryOutcome {
        payload: CrimeCompletionPayload::ReadData(vec![0; 8]),
        fault,
        diagnostic: Some(CrimeMemoryDiagnostic {
            address,
            syndrome,
            check: syndrome,
            corrected,
            write: false,
            read_modify_write: false,
        }),
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
