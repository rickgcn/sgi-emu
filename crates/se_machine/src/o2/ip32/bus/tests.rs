use se_core::role::{BusDeviceRole, BusRole};
use se_device::cpu::execution::protocol::{ExecutionTransaction, ExecutionTransactionId};
use se_device::cpu::mips4::cache::Mips4MemoryAccessType;
use se_device::cpu::mips4::execution::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};

use super::*;
use crate::o2::ip32::component_ids;

fn read(id: u128, address: u64) -> ExecutionTransaction<Mips4ExecutionTransaction> {
    ExecutionTransaction {
        id: ExecutionTransactionId::new(id),
        payload: Mips4ExecutionTransaction::Read {
            physical_address: address,
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::DataLoad,
            access_type: Mips4MemoryAccessType::Uncached,
        },
    }
}

#[test]
fn bus_preserves_transaction_identity_and_original_address() {
    let mut bus = Ip32CpuAddressBus::new(component_ids::CPU_SYSAD_BUS, "bus", 64 * 1024 * 1024);
    let transaction = read(0x1234, 0x4000_0020);

    assert_eq!(
        bus.route(transaction.clone()),
        Ip32BusRoute::Memory {
            region: Ip32PhysicalRegion::LinearMemory,
            target: component_ids::RAM,
            offset: 0x20,
            no_ecc: false,
            transaction,
        }
    );
}

#[test]
fn strict_and_permissive_stub_policies_are_distinct() {
    let transaction = read(1, 0x1400_0000).payload;
    let mut strict = Ip32MmioStub::new(
        component_ids::CRIME,
        "CRIME",
        Ip32UnimplementedAccessPolicy::Strict,
    );
    let mut permissive = Ip32MmioStub::new(
        component_ids::CRIME,
        "CRIME",
        Ip32UnimplementedAccessPolicy::Permissive,
    );

    assert_eq!(
        strict.accept(transaction),
        Mips4ExecutionCompletion::BusError
    );
    assert_eq!(
        permissive.accept(transaction),
        Mips4ExecutionCompletion::ReadData(0)
    );
    assert_eq!(
        permissive.accept(Mips4ExecutionTransaction::Write {
            physical_address: 0x1400_0000,
            size: Mips4ExecutionTransferSize::Word,
            data: 0x4433_2211,
            byte_enable: 0x0f,
            access_type: Mips4MemoryAccessType::Uncached,
        }),
        Mips4ExecutionCompletion::WriteComplete
    );
}

#[test]
fn unmapped_addresses_never_reach_a_stub() {
    let mut bus = Ip32CpuAddressBus::new(component_ids::CPU_SYSAD_BUS, "bus", 64 * 1024 * 1024);
    let transaction = read(9, 0x2000_0000);

    assert_eq!(
        bus.route(transaction.clone()),
        Ip32BusRoute::Unmapped {
            region: Some(Ip32PhysicalRegion::HighMemoryUnconfirmed),
            transaction,
        }
    );
}
