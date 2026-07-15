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

#[test]
fn cgi_allows_multiple_requests_and_scopes_ids_by_target() {
    let gbe = ComponentId::new(4);
    let mut bus = CrimeCgiBus::new(BUS, "CGI", 1_000_000_000);
    let transaction = |controller, target| CrimeCgiTransaction {
        id: CrimeTransactionId::new(5),
        controller,
        target,
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0,
            transfer: CrimeTransfer::read(4),
        }),
    };
    assert!(matches!(
        bus.route(transaction(CRIME, gbe)),
        CrimeBusDisposition::QueuedAndNeedsService { .. }
    ));
    assert_eq!(
        bus.route(transaction(gbe, CRIME)),
        CrimeBusDisposition::Queued
    );

    let epoch = bus.epoch();
    bus.handle_event(CrimeCgiBusEvent::Service { epoch });
    let CrimeBusAction::Deliver {
        target: first_target,
        ..
    } = bus.poll()
    else {
        panic!("first CGI request must be delivered");
    };
    assert_eq!(first_target, gbe);
    let CrimeBusAction::ScheduleService { .. } = bus.poll() else {
        panic!("second CGI request must be scheduled without waiting for completion");
    };
    let next = bus.next_scheduled_event().unwrap();
    bus.handle_event(next);
    assert!(matches!(
        bus.poll(),
        CrimeBusAction::Deliver {
            target,
            transaction: CrimeCgiTransaction { id, .. }
        } if target == CRIME && id == CrimeTransactionId::new(5)
    ));
    assert_eq!(bus.in_flight.len(), 2);

    let completion = || CrimeCgiCompletion {
        id: CrimeTransactionId::new(5),
        result: Ok(CrimeCompletionPayload::ReadData(vec![0; 4].into())),
        memory_fault: None,
    };
    bus.accept_device_completion(CRIME, completion());
    bus.accept_device_completion(gbe, completion());
    assert_eq!(bus.in_flight.len(), 0);
    assert_eq!(bus.pending_completions.len(), 2);
}

#[test]
fn cgi_dma_cycle_count_includes_one_cycle_per_sixteen_data_bytes() {
    use crate::chipset::crime::protocol::CrimeDmaRequest;

    let mut bus = CrimeCgiBus::new(BUS, "CGI", 1_000_000_000);
    let disposition = bus.route(CrimeCgiTransaction {
        id: CrimeTransactionId::new(1),
        controller: CRIME,
        target: MACE,
        operation: CrimeLinkOperation::Dma(CrimeDmaRequest {
            address: 0,
            transfer: CrimeTransfer::write(
                vec![0; 512].into(),
                crate::chipset::crime::protocol::CrimeByteEnable::enabled(512),
            ),
        }),
    });
    let CrimeBusDisposition::QueuedAndNeedsService { delay, .. } = disposition else {
        panic!("an idle CGI bus must schedule service");
    };
    assert_eq!(delay.get(), 495);
}
