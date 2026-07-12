use se_core::role::{BusControllerRole, BusDeviceRole};

use super::*;
use crate::cpu::execution::protocol::{ExecutionTransaction, ExecutionTransactionId};
use crate::cpu::mips4::cache::Mips4MemoryAccessType;
use crate::cpu::mips4::execution::bus::{Mips4ExecutionAccessKind, Mips4ExecutionTransferSize};

const CRIME: ComponentId = ComponentId::new(1);
const RAM: ComponentId = ComponentId::new(2);
const MACE: ComponentId = ComponentId::new(3);
const GBE: ComponentId = ComponentId::new(4);
const TIMEBASE_HZ: u64 = 1_000_000_000;

fn crime() -> Crime {
    Crime::new(
        CRIME,
        "CRIME",
        CrimeConfig::default(),
        TIMEBASE_HZ,
        RAM,
        MACE,
        GBE,
    )
    .unwrap()
}

fn read(id: u128, address: u64, size: Mips4ExecutionTransferSize) -> CrimeSysAdRequest {
    CrimeSysAdRequest {
        time: SimTime::ZERO,
        transaction: ExecutionTransaction {
            id: ExecutionTransactionId::new(id),
            payload: Mips4ExecutionTransaction::Read {
                physical_address: address,
                size,
                kind: Mips4ExecutionAccessKind::DataLoad,
                access_type: Mips4MemoryAccessType::Uncached,
            },
        },
    }
}

fn next_non_trace(crime: &mut Crime) -> CrimeAction {
    loop {
        let CrimePoll::Action(action) = crime.poll().unwrap() else {
            panic!("CRIME became idle before producing an action");
        };
        if !matches!(action, CrimeAction::Trace(_)) {
            return action;
        }
    }
}

#[test]
fn crime_implements_all_required_topological_roles() {
    fn sysad_device<T: BusDeviceRole<CrimeSysAdRequest>>() {}
    fn memory_controller<T: BusControllerRole<CrimeMemoryCompletion>>() {}
    fn cmi_controller<T: BusControllerRole<CrimeCmiCompletion>>() {}
    fn cgi_controller<T: BusControllerRole<CrimeCgiCompletion>>() {}
    fn cmi_device<T: BusDeviceRole<CrimeCmiTransaction>>() {}
    fn cgi_device<T: BusDeviceRole<CrimeCgiTransaction>>() {}

    sysad_device::<Crime>();
    memory_controller::<Crime>();
    cmi_controller::<Crime>();
    cgi_controller::<Crime>();
    cmi_device::<Crime>();
    cgi_device::<Crime>();
}

#[test]
fn doubleword_id_read_returns_crime_11_identity_in_big_endian_lanes() {
    let mut crime = crime();
    crime
        .accept(read(
            1,
            registers::ID,
            Mips4ExecutionTransferSize::Doubleword,
        ))
        .unwrap();

    assert_eq!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(ExecutionCompletion {
            id: ExecutionTransactionId::new(1),
            payload: Mips4ExecutionCompletion::ReadData(0xa100_0000_0000_0000),
        })
    );
}

#[test]
fn word_access_to_piu_is_a_precise_bus_error() {
    let mut crime = crime();
    crime
        .accept(read(1, registers::ID, Mips4ExecutionTransferSize::Word))
        .unwrap();

    assert!(matches!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(ExecutionCompletion {
            payload: Mips4ExecutionCompletion::BusError,
            ..
        })
    ));
}

#[test]
fn memory_request_is_deferred_until_the_matching_bus_completion() {
    let mut crime = crime();
    crime
        .accept(read(
            9,
            LINEAR_MEMORY_START,
            Mips4ExecutionTransferSize::Word,
        ))
        .unwrap();
    let CrimeAction::StartMemory(transaction) = next_non_trace(&mut crime) else {
        panic!("expected a memory transaction");
    };
    assert_eq!(crime.poll().unwrap(), CrimePoll::Idle);

    crime.complete(CrimeMemoryCompletion {
        id: transaction.id,
        result: Ok(CrimeCompletionPayload::ReadData(vec![1, 2, 3, 4])),
        diagnostic: None,
    });
    assert_eq!(
        next_non_trace(&mut crime),
        CrimeAction::CompleteSysAd(ExecutionCompletion {
            id: ExecutionTransactionId::new(9),
            payload: Mips4ExecutionCompletion::ReadData(0x0403_0201),
        })
    );
}
