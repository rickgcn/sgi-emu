use se_core::role::BusRole;

use super::*;
use crate::chipset::crime::protocol::{CrimeTransactionId, CrimeTransfer};

const BUS: ComponentId = ComponentId::new(1);
const CRIME: ComponentId = ComponentId::new(2);
const RAM: ComponentId = ComponentId::new(3);

fn request(id: u128, client: CrimeMemoryClient) -> CrimeMemoryTransaction {
    CrimeMemoryTransaction {
        id: CrimeTransactionId::new(id),
        time: SimTime::ZERO,
        controller: CRIME,
        client,
        address: 0,
        no_ecc: false,
        transfer: CrimeTransfer::Read { length: 8 },
    }
}

#[test]
fn first_request_schedules_service_and_later_requests_merge() {
    let mut bus = CrimeMemoryBus::new(BUS, "memory", RAM, 1_000_000_000, SimDuration::new(27_000));

    assert_eq!(
        bus.route(request(1, CrimeMemoryClient::Cpu)),
        CrimeBusDisposition::QueuedAndNeedsService {
            delay: SimDuration::new(15)
        }
    );
    assert_eq!(
        bus.route(request(2, CrimeMemoryClient::Gbe)),
        CrimeBusDisposition::Queued
    );
}

#[test]
fn weighted_slot_order_prioritizes_gbe_before_cpu() {
    let mut bus = CrimeMemoryBus::new(BUS, "memory", RAM, 1_000_000_000, SimDuration::new(27_000));
    bus.route(request(1, CrimeMemoryClient::Cpu));
    bus.route(request(2, CrimeMemoryClient::Gbe));
    let epoch = bus.epoch();
    bus.handle_event(CrimeMemoryBusEvent::Service { epoch });

    assert!(matches!(
        bus.poll(),
        CrimeBusAction::Deliver {
            transaction: CrimeMemoryTransaction { id, .. },
            ..
        } if id == CrimeTransactionId::new(2)
    ));
}

#[test]
fn stale_service_event_has_no_effect() {
    let mut bus = CrimeMemoryBus::new(BUS, "memory", RAM, 1_000_000_000, SimDuration::new(27_000));
    let old = bus.epoch();
    bus.hard_reset(SimTime::ZERO);
    bus.handle_event(CrimeMemoryBusEvent::Service { epoch: old });

    assert_eq!(bus.poll(), CrimeBusAction::Idle);
}

#[test]
fn refresh_periods_are_accounted_lazily_when_work_arrives() {
    let mut bus = CrimeMemoryBus::new(BUS, "memory", RAM, 1_000_000_000, SimDuration::new(27_000));
    bus.power_on(SimTime::ZERO);
    let mut transaction = request(1, CrimeMemoryClient::Cpu);
    transaction.time = SimTime::new(81_000);

    bus.route(transaction);

    assert_eq!(bus.refresh_debt, 3);
    assert_eq!(bus.next_refresh_time, SimTime::new(108_000));
}
