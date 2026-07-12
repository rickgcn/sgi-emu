use super::*;

#[test]
fn default_memory_is_two_32_mib_banks() {
    let config = CrimeMemoryConfig::default();

    assert_eq!(config.total_size_bytes(), 64 * 1024 * 1024);
    assert_eq!(
        config.banks[0],
        Some(CrimeSdramBankConfig {
            size: CrimeSdramBankSize::MiB32
        })
    );
    assert_eq!(config.banks[0], config.banks[1]);
    assert!(config.banks[2..].iter().all(Option::is_none));
    assert_eq!(config.validate(), Ok(()));
}

#[test]
fn total_size_supports_all_eight_large_banks() {
    let config = CrimeMemoryConfig {
        banks: [Some(CrimeSdramBankConfig {
            size: CrimeSdramBankSize::MiB128,
        }); 8],
    };

    assert_eq!(config.total_size_bytes(), 1024 * 1024 * 1024);
}

#[test]
fn missing_bank_zero_is_rejected() {
    assert_eq!(
        CrimeMemoryConfig::empty().validate(),
        Err(CrimeConfigError::MissingBankZero)
    );
}
