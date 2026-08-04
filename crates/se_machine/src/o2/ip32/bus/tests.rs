use se_core::component::{Component, ComponentId};
use se_core::scheduler::SimTime;
use se_device::chipset::crime::protocol::{
    CrimeCompletionPayload, CrimeSysAdCompletion, CrimeTransactionId,
};
use se_device::cpu::execution::protocol::{ExecutionTransaction, ExecutionTransactionId};
use se_device::cpu::mips4::cache::Mips4MemoryAccessType;
use se_device::cpu::mips4::execution::bus::{
    Mips4ExecutionAccessKind, Mips4ExecutionCompletion, Mips4ExecutionTransaction,
    Mips4ExecutionTransferSize,
};

use super::Ip32SysAdBus;

fn bus() -> Ip32SysAdBus {
    Ip32SysAdBus::new(
        ComponentId::new(1),
        "SysAD",
        ComponentId::new(2),
        ComponentId::new(3),
    )
}

#[test]
fn maps_cpu_reads_and_correlated_completions() {
    let bus = bus();
    let transaction = ExecutionTransaction {
        id: ExecutionTransactionId::new(7),
        payload: Mips4ExecutionTransaction::Read {
            physical_address: 0x1000,
            size: Mips4ExecutionTransferSize::Word,
            kind: Mips4ExecutionAccessKind::DataLoad,
            access_type: Mips4MemoryAccessType::Uncached,
        },
    };

    let request = bus.translate_request(&transaction, SimTime::new(11));
    assert_eq!(request.id, CrimeTransactionId::new(7));
    assert_eq!(request.time, SimTime::new(11));
    assert_eq!(request.address, 0x1000);

    let completion = bus
        .translate_completion(
            &transaction,
            CrimeSysAdCompletion {
                id: CrimeTransactionId::new(7),
                result: Ok(CrimeCompletionPayload::ReadData(
                    [0x12, 0x34, 0x56, 0x78].into_iter().collect(),
                )),
            },
        )
        .unwrap();
    assert_eq!(completion.id, transaction.id);
    assert_eq!(
        completion.payload,
        Mips4ExecutionCompletion::ReadData(0x7856_3412)
    );
}

#[test]
fn state_contains_only_fixed_wiring() {
    let mut bus = bus();
    let state = bus.save_state();
    bus.reset();
    bus.restore_state(state).unwrap();
    assert_eq!(bus.controller(), ComponentId::new(2));
    assert_eq!(bus.target(), ComponentId::new(3));
}
