use se_core::role::BusRole;
use se_device::chipset::crime::protocol::{CrimeBusError, CrimeTransferView};
use se_device::cpu::execution::protocol::{ExecutionTransaction, ExecutionTransactionId};
use se_device::cpu::mips4::cache::Mips4MemoryAccessType;
use se_device::cpu::mips4::execution::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransferSize,
};

use super::*;
use crate::o2::ip32::component_ids;

const DELIVERY_TIME: SimTime = SimTime::new(42);

fn cpu_read(id: u128) -> ExecutionTransaction<Mips4ExecutionTransaction> {
    ExecutionTransaction {
        id: ExecutionTransactionId::new(id),
        payload: Mips4ExecutionTransaction::Read {
            physical_address: 0x4000_0000,
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::DataLoad,
            access_type: Mips4MemoryAccessType::Uncached,
        },
    }
}

fn cpu_write(id: u128) -> ExecutionTransaction<Mips4ExecutionTransaction> {
    ExecutionTransaction {
        id: ExecutionTransactionId::new(id),
        payload: Mips4ExecutionTransaction::Write {
            physical_address: 0x4000_0008,
            size: Mips4ExecutionTransferSize::Doubleword,
            data: 0x8877_6655_4433_2211,
            byte_enable: 0xa5,
            access_type: Mips4MemoryAccessType::Uncached,
        },
    }
}

fn new_bus() -> Ip32SysAdBus {
    Ip32SysAdBus::new(
        component_ids::CPU_SYSAD_BUS,
        "SysAD",
        component_ids::CPU0,
        component_ids::CRIME,
        1_000_000_000,
        66_666_500,
    )
}

#[test]
fn sysad_bus_translates_cpu_reads_before_delivery_to_crime() {
    let mut bus = new_bus();
    let transaction = cpu_read(7);
    assert!(matches!(
        bus.route(transaction),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    bus.handle_event(
        DELIVERY_TIME,
        Ip32SysAdBusEvent::Service {
            generation: bus.generation(),
        },
    );
    assert_eq!(
        bus.poll(),
        Ip32SysAdBusAction::Deliver {
            target: component_ids::CRIME,
            request: CrimeSysAdRequest {
                id: CrimeTransactionId::new(7),
                time: DELIVERY_TIME,
                address: 0x4000_0000,
                transfer: CrimeTransfer::read(4),
            },
        }
    );
}

#[test]
fn sysad_bus_translates_cpu_write_lanes_and_byte_enables() {
    let mut bus = new_bus();
    bus.route(cpu_write(8));
    bus.handle_event(
        DELIVERY_TIME,
        Ip32SysAdBusEvent::Service {
            generation: bus.generation(),
        },
    );
    let Ip32SysAdBusAction::Deliver { request, .. } = bus.poll() else {
        panic!("expected a CRIME delivery");
    };
    assert_eq!(request.id, CrimeTransactionId::new(8));
    assert_eq!(request.address, 0x4000_0008);
    let CrimeTransferView::Write { data, byte_enable } = request.transfer.view() else {
        panic!("expected a write transfer");
    };
    assert_eq!(data, &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    assert_eq!(
        byte_enable.iter().collect::<Vec<_>>(),
        [true, false, true, false, false, true, false, true]
    );
}

#[test]
fn sysad_queue_preserves_order_and_rejects_mismatched_completion() {
    let mut bus = new_bus();
    assert!(matches!(
        bus.route(cpu_read(1)),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    assert_eq!(bus.route(cpu_read(2)), CrimeBusDisposition::Queued);
    assert_eq!(bus.queue.len(), 2);

    bus.handle_event(
        DELIVERY_TIME,
        Ip32SysAdBusEvent::Service {
            generation: bus.generation(),
        },
    );
    assert!(matches!(
        bus.poll(),
        Ip32SysAdBusAction::Deliver { request, .. }
            if request.id == CrimeTransactionId::new(1)
    ));
    bus.accept_device_completion(CrimeSysAdCompletion {
        id: CrimeTransactionId::new(99),
        result: Ok(CrimeCompletionPayload::ReadData([0; 4].into())),
    });
    assert_eq!(bus.poll(), Ip32SysAdBusAction::Idle);
    assert_eq!(
        bus.in_flight.as_ref().unwrap().id,
        ExecutionTransactionId::new(1)
    );

    bus.accept_device_completion(CrimeSysAdCompletion {
        id: CrimeTransactionId::new(1),
        result: Ok(CrimeCompletionPayload::ReadData([0x34, 0x12, 0, 0].into())),
    });
    assert!(matches!(bus.poll(), Ip32SysAdBusAction::Schedule { .. }));
    bus.handle_event(
        DELIVERY_TIME,
        Ip32SysAdBusEvent::Complete {
            generation: bus.generation(),
        },
    );
    assert!(matches!(
        bus.poll(),
        Ip32SysAdBusAction::Complete {
            transaction,
            completion: ExecutionCompletion {
                payload: Mips4ExecutionCompletion::ReadData(0x1234),
                ..
            },
            ..
        } if transaction.id == ExecutionTransactionId::new(1)
    ));
    assert!(matches!(bus.poll(), Ip32SysAdBusAction::Schedule { .. }));
    assert_eq!(
        bus.queue.front().unwrap().id,
        ExecutionTransactionId::new(2)
    );
}

#[test]
fn sysad_bus_maps_crime_errors_and_oversized_reads_to_cpu_bus_errors() {
    for result in [
        Err(CrimeBusError::Timeout),
        Ok(CrimeCompletionPayload::WriteComplete),
        Ok(CrimeCompletionPayload::ReadData(vec![0; 3].into())),
        Ok(CrimeCompletionPayload::ReadData(vec![0; 9].into())),
    ] {
        let mut bus = new_bus();
        bus.route(cpu_read(3));
        bus.handle_event(
            DELIVERY_TIME,
            Ip32SysAdBusEvent::Service {
                generation: bus.generation(),
            },
        );
        let _ = bus.poll();
        bus.accept_device_completion(CrimeSysAdCompletion {
            id: CrimeTransactionId::new(3),
            result,
        });
        let _ = bus.poll();
        bus.handle_event(
            DELIVERY_TIME,
            Ip32SysAdBusEvent::Complete {
                generation: bus.generation(),
            },
        );
        assert!(matches!(
            bus.poll(),
            Ip32SysAdBusAction::Complete {
                completion: ExecutionCompletion {
                    payload: Mips4ExecutionCompletion::BusError,
                    ..
                },
                ..
            }
        ));
    }
}

#[test]
fn direct_sysad_commit_matches_scheduled_clock_progress() {
    let mut scheduled = new_bus();
    let mut direct = new_bus();
    let transaction = cpu_read(7);
    let plan = direct.plan_direct_transaction().unwrap();

    assert!(matches!(
        scheduled.route(transaction),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    scheduled.handle_event(
        DELIVERY_TIME,
        Ip32SysAdBusEvent::Service {
            generation: scheduled.generation(),
        },
    );
    assert!(matches!(
        scheduled.poll(),
        Ip32SysAdBusAction::Deliver { .. }
    ));
    scheduled.accept_device_completion(CrimeSysAdCompletion {
        id: CrimeTransactionId::new(7),
        result: Ok(CrimeCompletionPayload::ReadData([0x34, 0x12, 0, 0].into())),
    });
    assert!(matches!(
        scheduled.poll(),
        Ip32SysAdBusAction::Schedule { .. }
    ));

    assert!(direct.commit_direct_transaction(plan));
    assert_eq!(direct.clock, scheduled.clock);
}
