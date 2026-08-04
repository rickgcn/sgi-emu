use super::*;
use crate::chipset::crime::protocol::{
    CrimeCompletionPayload, CrimeLinkOperation, CrimePioRequest, CrimeTransfer,
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
fn cmi_correlates_target_completion_with_controller() {
    let mut bus = CrimeCmiBus::new(BUS, "CMI");
    let request = request(7);
    assert!(bus.begin(&request));
    assert_eq!(bus.pending_transactions(), 1);

    let completion = CrimeCmiCompletion {
        id: request.id,
        result: Ok(CrimeCompletionPayload::ReadData(vec![0; 8].into())),
        memory_fault: None,
    };
    let (controller, completion) = bus.complete(MACE, completion).unwrap();
    assert_eq!(controller, CRIME);
    assert_eq!(completion.id, request.id);
    assert_eq!(bus.pending_transactions(), 0);
}

#[test]
fn cmi_rejects_duplicate_and_unrelated_transactions() {
    let mut bus = CrimeCmiBus::new(BUS, "CMI");
    let request = request(1);
    assert!(bus.begin(&request));
    assert!(!bus.begin(&request));
    assert!(
        bus.complete(
            CRIME,
            CrimeCmiCompletion {
                id: request.id,
                result: Ok(CrimeCompletionPayload::WriteComplete),
                memory_fault: None,
            }
        )
        .is_none()
    );
    assert_eq!(bus.pending_transactions(), 1);
}

#[test]
fn cgi_scopes_equal_ids_by_target() {
    let gbe = ComponentId::new(4);
    let mut bus = CrimeCgiBus::new(BUS, "CGI");
    let transaction = |controller, target| CrimeCgiTransaction {
        id: CrimeTransactionId::new(5),
        controller,
        target,
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0,
            transfer: CrimeTransfer::read(4),
        }),
    };
    assert!(bus.begin(&transaction(CRIME, gbe)));
    assert!(bus.begin(&transaction(gbe, CRIME)));
    assert_eq!(bus.pending_transactions(), 2);

    for target in [CRIME, gbe] {
        let result = bus.complete(
            target,
            CrimeCgiCompletion {
                id: CrimeTransactionId::new(5),
                result: Ok(CrimeCompletionPayload::WriteComplete),
                memory_fault: None,
            },
        );
        assert!(result.is_some());
    }
    assert_eq!(bus.pending_transactions(), 0);
}
