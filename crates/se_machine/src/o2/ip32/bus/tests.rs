use se_core::role::BusRole;
use se_device::cpu::execution::protocol::{ExecutionTransaction, ExecutionTransactionId};
use se_device::cpu::mips4::cache::Mips4MemoryAccessType;
use se_device::cpu::mips4::execution::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransferSize,
};

use super::*;
use crate::o2::ip32::component_ids;

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

#[test]
fn sysad_bus_only_delivers_cpu_traffic_to_crime() {
    let mut bus = Ip32SysAdBus::new(
        component_ids::CPU_SYSAD_BUS,
        "SysAD",
        component_ids::CPU0,
        component_ids::CRIME,
        1_000_000_000,
        66_666_500,
    );
    let transaction = cpu_read(7);
    assert!(matches!(
        bus.route(transaction.clone()),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    bus.handle_event(Ip32SysAdBusEvent::Service {
        generation: bus.generation(),
    });
    assert_eq!(
        bus.poll(),
        Ip32SysAdBusAction::Deliver {
            target: component_ids::CRIME,
            transaction,
        }
    );
}

#[test]
fn sysad_queue_preserves_order_and_rejects_mismatched_completion() {
    let mut bus = Ip32SysAdBus::new(
        component_ids::CPU_SYSAD_BUS,
        "SysAD",
        component_ids::CPU0,
        component_ids::CRIME,
        1_000_000_000,
        66_666_500,
    );
    assert!(matches!(
        bus.route(cpu_read(1)),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    assert_eq!(bus.route(cpu_read(2)), CrimeBusDisposition::Queued);
    assert_eq!(bus.queue.len(), 2);

    bus.handle_event(Ip32SysAdBusEvent::Service {
        generation: bus.generation(),
    });
    assert!(matches!(
        bus.poll(),
        Ip32SysAdBusAction::Deliver { transaction, .. }
            if transaction.id == ExecutionTransactionId::new(1)
    ));
    bus.accept_device_completion(ExecutionCompletion {
        id: ExecutionTransactionId::new(99),
        payload: Mips4ExecutionCompletion::ReadData(0),
    });
    assert_eq!(bus.poll(), Ip32SysAdBusAction::Idle);
    assert_eq!(
        bus.in_flight.as_ref().unwrap().id,
        ExecutionTransactionId::new(1)
    );

    bus.accept_device_completion(ExecutionCompletion {
        id: ExecutionTransactionId::new(1),
        payload: Mips4ExecutionCompletion::ReadData(0x1234),
    });
    assert!(matches!(bus.poll(), Ip32SysAdBusAction::Schedule { .. }));
    bus.handle_event(Ip32SysAdBusEvent::Complete {
        generation: bus.generation(),
    });
    assert!(matches!(
        bus.poll(),
        Ip32SysAdBusAction::Complete {
            transaction,
            ..
        } if transaction.id == ExecutionTransactionId::new(1)
    ));
    assert!(matches!(bus.poll(), Ip32SysAdBusAction::Schedule { .. }));
    assert_eq!(
        bus.queue.front().unwrap().id,
        ExecutionTransactionId::new(2)
    );
}
