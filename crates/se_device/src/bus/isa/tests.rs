use se_core::role::BusRole;

use super::*;

const BUS: ComponentId = ComponentId::new(1);
const CONTROLLER: ComponentId = ComponentId::new(2);
const TARGET: ComponentId = ComponentId::new(3);

#[test]
fn routes_and_completes_in_order() {
    let mut bus = IsaBus::new(BUS, "ISA", SimDuration::new(4));
    let transaction = IsaTransaction {
        id: IsaTransactionId::new(7),
        time: SimTime::ZERO,
        controller: CONTROLLER,
        target: TARGET,
        address: 0x20,
        transfer: IsaTransfer::Read { length: 1 },
    };
    assert_eq!(
        bus.route(transaction.clone()),
        IsaBusDisposition::QueuedAndNeedsService {
            delay: SimDuration::new(4)
        }
    );
    bus.handle_event(IsaBusEvent::Service { epoch: 0 });
    assert_eq!(
        bus.poll(),
        IsaBusAction::Deliver {
            target: TARGET,
            transaction
        }
    );
    assert!(bus.accept_device_completion(IsaCompletion {
        id: IsaTransactionId::new(7),
        result: Ok(IsaCompletionPayload::ReadData(vec![0xaa])),
    }));
    assert!(matches!(bus.poll(), IsaBusAction::Schedule { .. }));
    bus.handle_event(IsaBusEvent::Complete { epoch: 0 });
    assert!(matches!(
        bus.poll(),
        IsaBusAction::Complete {
            controller: CONTROLLER,
            ..
        }
    ));
}

#[test]
fn reset_invalidates_old_events() {
    let mut bus = IsaBus::new(BUS, "ISA", SimDuration::new(1));
    bus.hard_reset();
    bus.handle_event(IsaBusEvent::Service { epoch: 0 });
    assert_eq!(bus.poll(), IsaBusAction::Idle);
}
