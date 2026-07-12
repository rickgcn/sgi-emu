use se_core::role::{BusDeviceRole, BusRole};
use se_device::chipset::crime::protocol::{
    CrimeLinkOperation, CrimePioRequest, CrimeTransactionId,
};
use se_device::cpu::execution::protocol::{ExecutionTransaction, ExecutionTransactionId};
use se_device::cpu::mips4::cache::Mips4MemoryAccessType;
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};

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
fn mace_endpoint_decodes_prom_and_preserves_link_identity() {
    let mut mace = Ip32MaceEndpoint::new(component_ids::MACE, "MACE", CrimeAccessPolicy::Strict);
    let transaction = CrimeCmiTransaction {
        id: CrimeTransactionId::new(9),
        controller: component_ids::CRIME,
        target: component_ids::MACE,
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: PROM_START + 0x20,
            transfer: CrimeTransfer::Read { length: 8 },
        }),
    };

    assert_eq!(
        mace.accept(transaction),
        Ip32MaceDeviceResponse::Prom {
            id: CrimeTransactionId::new(9),
            offset: 0x20,
            transfer: CrimeTransfer::Read { length: 8 },
        }
    );
}

#[test]
fn peer_policy_never_bypasses_the_link_protocol() {
    let transaction = CrimeCgiTransaction {
        id: CrimeTransactionId::new(1),
        controller: component_ids::CRIME,
        target: component_ids::GBE,
        operation: CrimeLinkOperation::Pio(CrimePioRequest {
            address: 0x1600_0000,
            transfer: CrimeTransfer::Read { length: 4 },
        }),
    };
    let mut strict = Ip32GbeEndpoint::new(component_ids::GBE, "GBE", CrimeAccessPolicy::Strict);
    let mut permissive =
        Ip32GbeEndpoint::new(component_ids::GBE, "GBE", CrimeAccessPolicy::Permissive);

    assert!(matches!(
        strict.accept(transaction.clone()),
        CrimeLinkDeviceResponse::Complete(CrimeCgiCompletion {
            result: Err(CrimeBusError::Unsupported),
            ..
        })
    ));
    assert!(matches!(
        permissive.accept(transaction),
        CrimeLinkDeviceResponse::Complete(CrimeCgiCompletion {
            result: Ok(CrimeCompletionPayload::ReadData(data)),
            ..
        }) if data == vec![0; 4]
    ));
}
