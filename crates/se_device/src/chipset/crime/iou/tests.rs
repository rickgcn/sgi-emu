use se_core::role::BusRole;

use super::*;
use crate::chipset::crime::protocol::{
    CrimeCompletionPayload, CrimeLinkOperation, CrimePioRequest, CrimeTransactionId, CrimeTransfer,
};

const BUS: ComponentId = ComponentId::new(1);
const CRIME: ComponentId = ComponentId::new(2);
const MACE: ComponentId = ComponentId::new(3);

fn request(id: u128) -> CrimeCmiTransaction {
    CrimeCmiTransaction {
        id: CrimeTransactionId::new(id),
        controller: CRIME,
        target: MACE,
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0x1fc0_0000,
            transfer: CrimeTransfer::read(8),
        }),
    }
}

#[test]
fn cmi_preserves_controller_and_transaction_identity() {
    let mut bus = CrimeCmiBus::new(BUS, "CMI", 1_000_000_000);
    assert!(matches!(
        bus.route(request(7)),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    let epoch = bus.epoch();
    bus.handle_event(CrimeCmiBusEvent::Service { epoch });
    assert!(matches!(
        bus.poll(),
        CrimeBusAction::Deliver {
            target: MACE,
            transaction: CrimeCmiTransaction { id, .. }
        } if id == CrimeTransactionId::new(7)
    ));

    bus.accept_device_completion(CrimeCmiCompletion {
        id: CrimeTransactionId::new(7),
        result: Ok(CrimeCompletionPayload::ReadData(vec![0; 8].into())),
        memory_fault: None,
    });
    assert!(matches!(bus.poll(), CrimeBusAction::ScheduleService { .. }));
    bus.handle_event(CrimeCmiBusEvent::Complete { epoch });
    assert!(matches!(
        bus.poll(),
        CrimeBusAction::Complete {
            controller: CRIME,
            completion: CrimeCmiCompletion { id, .. }
        } if id == CrimeTransactionId::new(7)
    ));
}

#[test]
fn cmi_queue_preserves_order_and_mismatched_completion_state() {
    let mut bus = CrimeCmiBus::new(BUS, "CMI", 1_000_000_000);
    assert!(matches!(
        bus.route(request(1)),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    assert_eq!(bus.route(request(2)), CrimeBusDisposition::Queued);
    assert_eq!(bus.inner.queue.len(), 2);

    let epoch = bus.epoch();
    bus.handle_event(CrimeCmiBusEvent::Service { epoch });
    assert!(matches!(
        bus.poll(),
        CrimeBusAction::Deliver { transaction, .. }
            if transaction.id == CrimeTransactionId::new(1)
    ));
    let completion = |id| CrimeCmiCompletion {
        id: CrimeTransactionId::new(id),
        result: Ok(CrimeCompletionPayload::WriteComplete),
        memory_fault: None,
    };
    bus.accept_device_completion(completion(99));
    assert_eq!(bus.poll(), CrimeBusAction::Idle);
    assert_eq!(
        bus.inner.in_flight.as_ref().unwrap().id,
        CrimeTransactionId::new(1)
    );

    bus.accept_device_completion(completion(1));
    assert!(matches!(bus.poll(), CrimeBusAction::ScheduleService { .. }));
    bus.handle_event(CrimeCmiBusEvent::Complete { epoch });
    assert!(matches!(
        bus.poll(),
        CrimeBusAction::Complete {
            completion: CrimeCmiCompletion { id, .. },
            ..
        } if id == CrimeTransactionId::new(1)
    ));
    assert!(matches!(bus.poll(), CrimeBusAction::ScheduleService { .. }));
    assert_eq!(
        bus.inner.queue.front().unwrap().id,
        CrimeTransactionId::new(2)
    );
}

#[test]
fn reset_invalidates_old_link_events() {
    let mut bus = CrimeCgiBus::new(BUS, "CGI", 1_000_000_000);
    let old = bus.epoch();
    bus.hard_reset();
    bus.handle_event(CrimeCgiBusEvent::Service { epoch: old });
    assert_eq!(bus.poll(), CrimeBusAction::Idle);
}
