use se_core::role::BusDeviceRole;

use super::*;
use crate::chipset::crime::config::{CrimeSdramBankConfig, CrimeSdramBankSize};
use crate::chipset::crime::protocol::{
    CrimeMemoryBankSelect, CrimeMemoryClient, CrimeMemoryFault, CrimeMemoryInhibitReason,
    CrimeMemoryOutcome, CrimeTransactionId, CrimeTransfer,
};

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

#[test]
fn sparse_lane_accesses_share_one_page_lookup_and_preserve_zero_pages() {
    let config = CrimeSdramBankConfig {
        size: CrimeSdramBankSize::MiB32,
    };
    let mut bank = SparseBank::new(config);
    assert_eq!(bank.read_lane(0), (0, 0));
    assert_eq!(bank.read_lane((PAGE_SIZE - 8) as u64), (0, 0));

    let first = 0x0123_4567_89ab_cdef;
    bank.write_lane(0, first, 0x55);
    assert_eq!(bank.read_lane(0), (first, 0x55));

    let last = 0xfedc_ba98_7654_3210;
    bank.write_lane((PAGE_SIZE - 8) as u64, last, 0xaa);
    assert_eq!(bank.read_lane((PAGE_SIZE - 8) as u64), (last, 0xaa));
    assert_eq!(bank.pages.iter().filter(|page| page.is_some()).count(), 1);

    let mut zero_bank = SparseBank::new(config);
    zero_bank.write_lane(0, 0, 0);
    assert!(zero_bank.pages[0].is_none());
}

fn transaction(id: u128, address: u64, transfer: CrimeTransfer) -> CrimeMemoryTransaction {
    CrimeMemoryTransaction {
        id: CrimeTransactionId::new(id),
        time: se_core::scheduler::SimTime::ZERO,
        controller: CRIME,
        client: CrimeMemoryClient::Cpu,
        address,
        bank_select: CrimeMemoryBankSelect::Decode,
        no_ecc: false,
        transfer,
    }
}

fn successful(completion: CrimeMemoryCompletion) -> CrimeMemoryOutcome {
    completion.result.expect("memory transaction must complete")
}

#[test]
fn inhibited_bank_select_preserves_storage_and_returns_address_diagnostics() {
    let mut memory = small_memory();
    successful(memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0xaa; 8].into(), vec![true; 8].into()),
    )));

    let mut read = transaction(2, 0, CrimeTransfer::read(8));
    read.bank_select = CrimeMemoryBankSelect::Inhibited {
        reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
    };
    let read = successful(memory.accept(read));
    assert_eq!(
        read.payload,
        CrimeCompletionPayload::ReadData(vec![0; 8].into())
    );
    assert_eq!(read.fault, Some(CrimeMemoryFault::Address));
    let diagnostic = read.diagnostic().unwrap();
    assert_eq!(diagnostic.address, 0);
    assert!(!diagnostic.write);
    assert!(!diagnostic.read_modify_write);

    let mut write = transaction(
        3,
        0,
        CrimeTransfer::write(vec![0x55; 8].into(), vec![true; 8].into()),
    );
    write.bank_select = CrimeMemoryBankSelect::Inhibited {
        reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
    };
    let write = successful(memory.accept(write));
    assert_eq!(write.payload, CrimeCompletionPayload::WriteComplete);
    assert_eq!(write.fault, Some(CrimeMemoryFault::Address));
    let diagnostic = write.diagnostic().unwrap();
    assert!(diagnostic.write);
    assert!(!diagnostic.read_modify_write);

    let stored = successful(memory.accept(transaction(4, 0, CrimeTransfer::read(8))));
    assert_eq!(
        stored.payload,
        CrimeCompletionPayload::ReadData(vec![0xaa; 8].into())
    );

    let mut partial = transaction(
        5,
        2,
        CrimeTransfer::write(vec![0x11].into(), vec![true].into()),
    );
    partial.bank_select = CrimeMemoryBankSelect::Inhibited {
        reason: CrimeMemoryInhibitReason::InvalidRenderTlb,
    };
    assert!(
        successful(memory.accept(partial))
            .diagnostic()
            .unwrap()
            .read_modify_write
    );
}

#[test]
fn partial_write_uses_read_modify_write_and_regenerates_ecc() {
    let mut memory = small_memory();
    let write = transaction(
        1,
        2,
        CrimeTransfer::write(vec![0x12, 0x34].into(), vec![true, true].into()),
    );
    assert_eq!(
        successful(memory.accept(write)).payload,
        CrimeCompletionPayload::WriteComplete
    );
    let read = transaction(2, 0, CrimeTransfer::read(8));
    assert_eq!(
        successful(memory.accept(read)).payload,
        CrimeCompletionPayload::ReadData(vec![0, 0, 0x12, 0x34, 0, 0, 0, 0].into())
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
        CrimeTransfer::write(vec![0xaa].into(), vec![true].into()),
    );
    memory.accept(write);

    assert_eq!(memory.banks[0].as_ref().unwrap().read_byte(0), 0xaa);
    assert_eq!(memory.banks[1].as_ref().unwrap().read_byte(0), 0);
}

#[test]
fn unpopulated_lower_bank_shadows_populated_higher_bank() {
    let mut config = CrimeMemoryConfig::default();
    config.banks[1] = None;
    config.banks[2] = Some(CrimeSdramBankConfig {
        size: CrimeSdramBankSize::MiB32,
    });
    let mut memory = CrimeSdram::new(RAM, "memory", config);
    memory.accept(CrimeSdramSignal::SetBankControl {
        bank: 0,
        value: 0x1f,
    });
    memory.accept(CrimeSdramSignal::SetBankControl { bank: 1, value: 0 });
    memory.accept(CrimeSdramSignal::SetBankControl { bank: 2, value: 0 });

    let write = successful(memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0xaa].into(), vec![true].into()),
    )));
    assert_eq!(write.payload, CrimeCompletionPayload::WriteComplete);
    assert_eq!(write.fault, None);
    assert_eq!(write.diagnostic(), None);
    assert_eq!(memory.banks[2].as_ref().unwrap().read_byte(0), 0);

    let read = successful(memory.accept(transaction(2, 0, CrimeTransfer::read(8))));
    assert_eq!(
        read.payload,
        CrimeCompletionPayload::ReadData(vec![0; 8].into())
    );
    assert_eq!(read.fault, None);
    assert_eq!(read.diagnostic(), None);
}

#[test]
fn programmed_128_mib_decode_ignores_low_control_bits_and_wraps_small_dimms() {
    let mut memory = small_memory();
    memory.accept(CrimeSdramSignal::SetBankControl {
        bank: 0,
        value: registers::MEMORY_BANK_SIZE_128_MIB | 3,
    });
    for bank in 1..8 {
        memory.accept(CrimeSdramSignal::SetBankControl { bank, value: 0x1f });
    }

    let write = successful(memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0x5a].into(), vec![true].into()),
    )));
    assert_eq!(write.fault, None);

    let alias = successful(memory.accept(transaction(2, 32 * 1024 * 1024, CrimeTransfer::read(1))));
    assert_eq!(
        alias.payload,
        CrimeCompletionPayload::ReadData(vec![0x5a].into())
    );
    assert_eq!(alias.fault, None);
}

#[test]
fn unmatched_addresses_complete_with_typed_memory_faults() {
    let mut memory = small_memory();
    for bank in 0..8 {
        memory.accept(CrimeSdramSignal::SetBankControl { bank, value: 0 });
    }
    let address = 0x2000_0000;

    let read = successful(memory.accept(transaction(1, address, CrimeTransfer::read(8))));
    assert_eq!(
        read.payload,
        CrimeCompletionPayload::ReadData(vec![0; 8].into())
    );
    assert_eq!(read.fault, Some(CrimeMemoryFault::Address));
    let diagnostic = read.diagnostic().unwrap();
    assert!(!diagnostic.write);
    assert!(!diagnostic.read_modify_write);

    let write = successful(memory.accept(transaction(
        2,
        address,
        CrimeTransfer::write(vec![0xaa; 8].into(), vec![true; 8].into()),
    )));
    assert_eq!(write.payload, CrimeCompletionPayload::WriteComplete);
    assert_eq!(write.fault, Some(CrimeMemoryFault::Address));
    let diagnostic = write.diagnostic().unwrap();
    assert!(diagnostic.write);
    assert!(!diagnostic.read_modify_write);

    let read_modify_write = successful(memory.accept(transaction(
        3,
        address,
        CrimeTransfer::write(vec![0xaa].into(), vec![true].into()),
    )));
    assert_eq!(read_modify_write.fault, Some(CrimeMemoryFault::Address));
    let diagnostic = read_modify_write.diagnostic().unwrap();
    assert!(diagnostic.write);
    assert!(diagnostic.read_modify_write);
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
        CrimeTransfer::write(vec![0x5a; 8].into(), vec![true; 8].into()),
    ));
    memory.inject_data_bit(0, 3).unwrap();
    let corrected = successful(memory.accept(transaction(2, 0, CrimeTransfer::read(8))));
    assert_eq!(
        corrected.payload,
        CrimeCompletionPayload::ReadData(vec![0x5a; 8].into())
    );
    assert_eq!(corrected.fault, None);
    assert!(corrected.diagnostic().unwrap().corrected);

    memory.inject_data_bit(0, 4).unwrap();
    let failed = successful(memory.accept(transaction(3, 0, CrimeTransfer::read(8))));
    assert_eq!(
        failed.payload,
        CrimeCompletionPayload::ReadData(
            vec![0x42, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a].into()
        )
    );
    assert_eq!(failed.fault, Some(CrimeMemoryFault::UncorrectableEcc));
    assert!(!failed.diagnostic().unwrap().corrected);
}

#[test]
fn uncorrectable_rmw_merges_incorrect_data_and_reports_a_memory_fault() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0x5a; 8].into(), vec![true; 8].into()),
    ));
    memory.inject_data_bit(0, 0).unwrap();
    memory.inject_data_bit(0, 1).unwrap();

    let write = successful(memory.accept(transaction(
        2,
        1,
        CrimeTransfer::write(vec![0xaa].into(), vec![true].into()),
    )));
    assert_eq!(write.payload, CrimeCompletionPayload::WriteComplete);
    assert_eq!(write.fault, Some(CrimeMemoryFault::UncorrectableEcc));
    let diagnostic = write.diagnostic().unwrap();
    assert!(diagnostic.write);
    assert!(diagnostic.read_modify_write);
    assert!(!diagnostic.corrected);

    let read = successful(memory.accept(transaction(3, 0, CrimeTransfer::read(8))));
    assert_eq!(
        read.payload,
        CrimeCompletionPayload::ReadData(
            vec![0x59, 0xaa, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a, 0x5a].into()
        )
    );
    assert_eq!(read.fault, None);
}

#[test]
fn no_ecc_alias_bypasses_read_correction() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0; 8].into(), vec![true; 8].into()),
    ));
    memory.inject_data_bit(0, 0).unwrap();
    let mut read = transaction(2, 0, CrimeTransfer::read(8));
    read.no_ecc = true;

    assert_eq!(
        successful(memory.accept(read)).payload,
        CrimeCompletionPayload::ReadData(vec![1, 0, 0, 0, 0, 0, 0, 0].into())
    );
}

#[test]
fn hard_reset_preserves_data_but_power_on_clears_it() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0xaa].into(), vec![true].into()),
    ));
    memory.accept(CrimeSdramSignal::HardReset);
    assert_eq!(
        successful(memory.accept(transaction(2, 0, CrimeTransfer::read(1)))).payload,
        CrimeCompletionPayload::ReadData(vec![0xaa].into())
    );
    memory.accept(CrimeSdramSignal::PowerOn);
    assert_eq!(
        successful(memory.accept(transaction(3, 0, CrimeTransfer::read(1)))).payload,
        CrimeCompletionPayload::ReadData(vec![0].into())
    );
}

#[test]
fn state_restore_preserves_name_and_rejects_topology_and_page_shape_atomically() {
    let source = small_memory();
    let mut renamed = CrimeSdram::new(
        RAM,
        "replacement",
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
    renamed.restore_state(source.save_state()).unwrap();
    assert_eq!(renamed.name(), "replacement");

    let mut large_config = CrimeMemoryConfig::default();
    large_config.banks[0] = Some(CrimeSdramBankConfig {
        size: CrimeSdramBankSize::MiB128,
    });
    let mismatched = CrimeSdram::new(RAM, "source", large_config).save_state();
    let mut target = small_memory();
    let before = target.clone();
    assert!(matches!(
        target.restore_state(mismatched),
        Err(ComponentStateError::ConfigurationMismatch { .. })
    ));
    assert_eq!(target, before);

    let mut invalid = source.save_state();
    invalid.banks[0].as_mut().unwrap().pages.pop();
    assert!(matches!(
        target.restore_state(invalid),
        Err(ComponentStateError::InvalidState { .. })
    ));
    assert_eq!(target, before);
}

#[test]
fn stable_code_window_version_tracks_sparse_page_mutations() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0x11; 8].into(), vec![true; 8].into()),
    ));

    let initial_version = memory.stable_code_window_version(0, 32).unwrap();
    let initial_fingerprint = memory.stable_code_window(0, 32, false).unwrap().1;
    let initial_source_revision = memory.code_revision();

    memory.accept(transaction(
        2,
        0,
        CrimeTransfer::write(vec![0x11; 8].into(), vec![true; 8].into()),
    ));
    assert_eq!(
        memory.stable_code_window_version(0, 32).unwrap(),
        initial_version
    );
    assert_eq!(memory.code_revision(), initial_source_revision);

    memory.accept(transaction(
        3,
        64,
        CrimeTransfer::write(vec![0x22; 8].into(), vec![true; 8].into()),
    ));
    let unrelated_version = memory.stable_code_window_version(0, 32).unwrap();
    assert_ne!(unrelated_version, initial_version);
    assert_ne!(memory.code_revision(), initial_source_revision);
    assert_eq!(
        memory.stable_code_window(0, 32, false).unwrap().1,
        initial_fingerprint
    );

    memory.accept(transaction(
        4,
        4,
        CrimeTransfer::write(vec![0x33].into(), vec![true].into()),
    ));
    assert_ne!(
        memory.stable_code_window_version(0, 32).unwrap(),
        unrelated_version
    );
    assert_ne!(
        memory.stable_code_window(0, 32, false).unwrap().1,
        initial_fingerprint
    );
}

#[test]
fn stable_code_window_version_tracks_mapping_ecc_and_restoration() {
    let mut source = small_memory();
    source.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0xaa; 8].into(), vec![true; 8].into()),
    ));

    let initial_version = source.stable_code_window_version(0, 32).unwrap();
    source.accept(CrimeSdramSignal::SetBankControl {
        bank: 7,
        value: 0x1e,
    });
    let mapped_version = source.stable_code_window_version(0, 32).unwrap();
    assert_ne!(mapped_version, initial_version);

    source.accept(CrimeSdramSignal::SetEccControl {
        enabled: true,
        use_replacement: true,
        replacement: 0x5a,
    });
    let ecc_version = source.stable_code_window_version(0, 32).unwrap();
    assert_ne!(ecc_version, mapped_version);

    let mut restored = small_memory();
    restored.accept(transaction(
        2,
        0,
        CrimeTransfer::write(vec![0x55; 8].into(), vec![true; 8].into()),
    ));
    let before_restore = restored.stable_code_window_version(0, 32).unwrap();
    restored.restore_state(source.save_state()).unwrap();
    assert_ne!(
        restored.stable_code_window_version(0, 32).unwrap(),
        before_restore
    );
    assert_eq!(
        restored.stable_code_window(0, 8, false).unwrap().0,
        vec![0xaa; 8]
    );
}

#[test]
fn stable_code_window_version_tracks_ecc_fault_injection() {
    let mut memory = small_memory();
    memory.accept(transaction(
        1,
        0,
        CrimeTransfer::write(vec![0x5a; 8].into(), vec![true; 8].into()),
    ));
    let clean_version = memory.stable_code_window_version(0, 32).unwrap();

    memory.inject_data_bit(0, 0).unwrap();

    assert_ne!(
        memory.stable_code_window_version(0, 32).unwrap(),
        clean_version
    );
    assert!(memory.stable_code_window(0, 32, false).is_none());
    assert_eq!(memory.stable_code_window(0, 8, true).unwrap().0[0], 0x5b);
}
