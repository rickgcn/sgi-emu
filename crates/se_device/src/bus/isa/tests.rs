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
        transfer: IsaTransfer::read(1),
    };
    assert_eq!(
        bus.route(transaction.clone()),
        IsaBusDisposition::QueuedAndNeedsService {
            delay: SimDuration::new(4),
            event: IsaBusEvent::Service { epoch: 0 },
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
        result: Ok(IsaCompletionPayload::ReadData([0xaa].into())),
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
fn queue_preserves_order_and_mismatched_completion_state() {
    let mut bus = IsaBus::new(BUS, "ISA", SimDuration::new(4));
    let request = |id| IsaTransaction {
        id: IsaTransactionId::new(id),
        time: SimTime::ZERO,
        controller: CONTROLLER,
        target: TARGET,
        address: 0x20,
        transfer: IsaTransfer::read(1),
    };
    assert!(matches!(
        bus.route(request(1)),
        IsaBusDisposition::QueuedAndNeedsService { .. }
    ));
    assert_eq!(bus.route(request(2)), IsaBusDisposition::Queued);
    assert_eq!(bus.queue.len(), 2);

    bus.handle_event(IsaBusEvent::Service { epoch: 0 });
    assert!(matches!(
        bus.poll(),
        IsaBusAction::Deliver { transaction, .. }
            if transaction.id == IsaTransactionId::new(1)
    ));
    assert!(!bus.accept_device_completion(IsaCompletion {
        id: IsaTransactionId::new(99),
        result: Ok(IsaCompletionPayload::WriteComplete),
    }));
    assert_eq!(bus.poll(), IsaBusAction::Idle);
    assert_eq!(bus.in_flight.as_ref().unwrap().id, IsaTransactionId::new(1));

    assert!(bus.accept_device_completion(IsaCompletion {
        id: IsaTransactionId::new(1),
        result: Ok(IsaCompletionPayload::WriteComplete),
    }));
    assert!(matches!(bus.poll(), IsaBusAction::Schedule { .. }));
    bus.handle_event(IsaBusEvent::Complete { epoch: 0 });
    assert!(matches!(
        bus.poll(),
        IsaBusAction::Complete { completion, .. }
            if completion.id == IsaTransactionId::new(1)
    ));
    assert!(matches!(bus.poll(), IsaBusAction::Schedule { .. }));
    assert_eq!(bus.queue.front().unwrap().id, IsaTransactionId::new(2));
}

#[test]
fn payloads_up_to_eight_bytes_remain_inline() {
    for length in [0, 1, 2, 4, 8] {
        let data: IsaData = (0..length).map(|value| value as u8).collect();
        let byte_enable: IsaByteEnable = (0..length).map(|_| true).collect();
        assert!(!data.spilled(), "{length}-byte data spilled");
        assert!(!byte_enable.spilled(), "{length}-lane enables spilled");
    }

    for length in [9, 32, 33] {
        let data: IsaData = (0..length).map(|value| value as u8).collect();
        let byte_enable: IsaByteEnable = (0..length).map(|_| true).collect();
        assert!(data.spilled(), "{length}-byte data stayed inline");
        assert!(byte_enable.spilled(), "{length}-lane enables stayed inline");
    }
}

#[test]
fn transfer_views_preserve_invalid_shapes_for_target_validation() {
    for length in [0, 1, 4, 8, 9, 32, 33, 256, 512] {
        let data: IsaData = (0..length).map(|index| index as u8).collect();
        let enables: IsaByteEnable = (0..length + 1).map(|index| index % 2 == 0).collect();
        let transfer = IsaTransfer::write(data, enables);
        let IsaTransferView::Write { data, byte_enable } = transfer.view() else {
            panic!("write transfer changed variant");
        };
        assert_eq!(data.len(), length);
        assert_eq!(byte_enable.len(), length + 1);
        assert_eq!(
            byte_enable.iter().collect::<Vec<_>>(),
            (0..length + 1)
                .map(|index| index % 2 == 0)
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn read_lengths_cover_complete_crime_blocks_without_truncation() {
    for length in [0, 1, 4, 8, 9, 32, 33, 256, 512] {
        let transfer = IsaTransfer::read(length);
        assert_eq!(transfer.length(), usize::from(length));
        assert_eq!(transfer.view(), IsaTransferView::Read { length });
    }
}

#[test]
fn vec_payloads_move_into_the_expected_storage_class() {
    for length in [0, 1, 4, 8, 9, 32, 33, 256, 512] {
        let data = IsaData::from(vec![0; length]);
        assert_eq!(data.spilled(), length > 8);
    }
}

#[test]
fn compact_isa_protocol_meets_hot_path_size_limits() {
    assert!(core::mem::size_of::<IsaTransaction>() <= 64);
    assert!(core::mem::size_of::<IsaBusAction>() <= 96);
}

#[test]
fn reset_invalidates_old_events() {
    let mut bus = IsaBus::new(BUS, "ISA", SimDuration::new(1));
    bus.hard_reset();
    bus.handle_event(IsaBusEvent::Service { epoch: 0 });
    assert_eq!(bus.poll(), IsaBusAction::Idle);
}
