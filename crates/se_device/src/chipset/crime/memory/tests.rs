use se_core::role::BusDeviceRole;

use super::*;
use crate::chipset::crime::config::{CrimeSdramBankConfig, CrimeSdramBankSize};
use crate::chipset::crime::protocol::{CrimeMemoryClient, CrimeTransactionId};

const RAM: ComponentId = ComponentId::new(1);
const CRIME: ComponentId = ComponentId::new(2);

fn small_memory() -> CrimeSdram {
    let mut memory = CrimeSdram::new(
        RAM,
        "memory",
        CrimeMemoryConfig {
            banks: [
                Some(CrimeSdramBankConfig {
                    size: CrimeSdramBankSize::MiB32,
                }),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        },
    );
    memory.accept(CrimeSdramSignal::SetEccControl {
        enabled: true,
        use_replacement: false,
        replacement: 0,
    });
    memory
}

fn transaction(id: u128, address: u64, transfer: CrimeTransfer) -> CrimeMemoryTransaction {
    CrimeMemoryTransaction {
        id: CrimeTransactionId::new(id),
        time: se_core::scheduler::SimTime::ZERO,
        controller: CRIME,
        client: CrimeMemoryClient::Cpu,
        address,
        no_ecc: false,
        transfer,
    }
}

#[test]
fn partial_write_uses_read_modify_write_and_regenerates_ecc() {
    let mut memory = small_memory();
    let write = transaction(
        1,
        2,
        CrimeTransfer::Write {
            data: vec![0x12, 0x34],
            byte_enable: vec![true, true],
        },
    );
    assert_eq!(
        memory.accept(write).result,
        Ok(CrimeCompletionPayload::WriteComplete)
    );
    let read = transaction(2, 0, CrimeTransfer::Read { length: 8 });
    assert_eq!(
        memory.accept(read).result,
        Ok(CrimeCompletionPayload::ReadData(vec![
            0, 0, 0x12, 0x34, 0, 0, 0, 0
        ]))
    );
}

#[test]
fn overlapping_bank_controls_select_the_lowest_bank() {
    let mut config = CrimeMemoryConfig::default();
    config.banks[1] = Some(CrimeSdramBankConfig {
        size: CrimeSdramBankSize::MiB32,
    });
    let mut memory = CrimeSdram::new(RAM, "memory", config);
    memory.accept(CrimeSdramSignal::SetBankControl { bank: 0, value: 0 });
    memory.accept(CrimeSdramSignal::SetBankControl { bank: 1, value: 0 });
    let write = transaction(
        1,
        0,
        CrimeTransfer::Write {
            data: vec![0xaa],
            byte_enable: vec![true],
        },
    );
    memory.accept(write);

    assert_eq!(memory.banks[0].as_ref().unwrap().read_byte(0), 0xaa);
    assert_eq!(memory.banks[1].as_ref().unwrap().read_byte(0), 0);
}

#[test]
fn reset_mapping_uses_the_installed_bank_capacity_bit() {
    let memory = CrimeSdram::new(
        RAM,
        "memory",
        CrimeMemoryConfig {
            banks: [
                Some(CrimeSdramBankConfig {
                    size: CrimeSdramBankSize::MiB128,
                }),
                None,
                None,
                None,
                None,
                None,
                None,
                None,
            ],
        },
    );

    assert_eq!(
        memory.bank_control(0),
        Some(registers::MEMORY_BANK_SIZE_128_MIB)
    );
}

#[test]
fn single_data_bit_is_corrected_and_double_error_is_reported() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::Write {
            data: vec![0x5a; 8],
            byte_enable: vec![true; 8],
        },
    ));
    memory.inject_data_bit(0, 3).unwrap();
    let corrected = memory.accept(transaction(2, 0, CrimeTransfer::Read { length: 8 }));
    assert_eq!(
        corrected.result,
        Ok(CrimeCompletionPayload::ReadData(vec![0x5a; 8]))
    );
    assert!(corrected.diagnostic.unwrap().corrected);

    memory.inject_data_bit(0, 4).unwrap();
    let failed = memory.accept(transaction(3, 0, CrimeTransfer::Read { length: 8 }));
    assert_eq!(failed.result, Err(CrimeBusError::UncorrectableEcc));
}

#[test]
fn no_ecc_alias_bypasses_read_correction() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::Write {
            data: vec![0; 8],
            byte_enable: vec![true; 8],
        },
    ));
    memory.inject_data_bit(0, 0).unwrap();
    let mut read = transaction(2, 0, CrimeTransfer::Read { length: 8 });
    read.no_ecc = true;

    assert_eq!(
        memory.accept(read).result,
        Ok(CrimeCompletionPayload::ReadData(vec![
            1, 0, 0, 0, 0, 0, 0, 0
        ]))
    );
}

#[test]
fn hard_reset_preserves_data_but_power_on_clears_it() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::Write {
            data: vec![0xaa],
            byte_enable: vec![true],
        },
    ));
    memory.accept(CrimeSdramSignal::HardReset);
    assert_eq!(
        memory
            .accept(transaction(2, 0, CrimeTransfer::Read { length: 1 }))
            .result,
        Ok(CrimeCompletionPayload::ReadData(vec![0xaa]))
    );
    memory.accept(CrimeSdramSignal::PowerOn);
    assert_eq!(
        memory
            .accept(transaction(3, 0, CrimeTransfer::Read { length: 1 }))
            .result,
        Ok(CrimeCompletionPayload::ReadData(vec![0]))
    );
}
