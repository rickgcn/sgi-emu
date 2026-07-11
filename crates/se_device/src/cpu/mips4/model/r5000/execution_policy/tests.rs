use crate::cpu::mips4::config::Mips4CacheConfig;
use crate::cpu::mips4::exception::{Mips4Exception, Mips4ExceptionImage, Mips4ExceptionRestart};
use crate::cpu::mips4::model::r5000::revision::R5000Revision;

use super::*;

fn profile(endianness: Mips4Endianness, secondary: Mips4CacheConfig) -> R5000Profile {
    R5000Profile::new(
        endianness,
        R5000Revision::from_bits(0x21),
        200_000_000,
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(32 * 1024, 32),
        secondary,
    )
}

fn boot_mode() -> R5000BootMode {
    R5000BootMode::from_low_bits(0).unwrap()
}

fn image() -> Mips4ExceptionImage {
    Mips4ExceptionImage::new(
        Mips4Exception::Interrupt,
        Mips4ExceptionRestart::new(0, None),
        None,
    )
}

#[test]
fn reset_config_records_fixed_geometry_and_boot_properties() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
        boot_mode(),
    );
    let config = policy.cp0_config();

    assert_ne!(config & CONFIG_SC, 0);
    assert_ne!(config & CONFIG_BE, 0);
    assert_ne!(config & CONFIG_EM, 0);
    assert_ne!(config & CONFIG_EB, 0);
    assert_eq!((config >> CONFIG_IC_SHIFT) & 0x07, 3);
    assert_eq!((config >> CONFIG_DC_SHIFT) & 0x07, 3);
    assert_eq!(config & 0x07, 2);
}

#[test]
fn config_writes_preserve_hardware_selected_fields() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Little, Mips4CacheConfig::disabled()),
        boot_mode(),
    );
    let current = u64::from(policy.cp0_config());
    let written = policy.cp0_write_value(Mips4Cp0Register::Config, current, u64::MAX);

    assert_eq!(
        written & !CONFIG_WRITABLE_MASK,
        current & !CONFIG_WRITABLE_MASK
    );
    assert_eq!(written & CONFIG_WRITABLE_MASK, CONFIG_WRITABLE_MASK);
}

#[test]
fn exception_vectors_distinguish_refill_width_and_general_entry() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
        boot_mode(),
    );
    let status = Mips4Cp0Status::from_bits(1 << 22);

    assert_eq!(
        policy.exception_vector(status, image(), Some(Mips4TlbAddressMode::Bits32)),
        BOOT_VECTOR_BASE
    );
    assert_eq!(
        policy.exception_vector(status, image(), Some(Mips4TlbAddressMode::Bits64)),
        BOOT_VECTOR_BASE + 0x80
    );
    assert_eq!(
        policy.exception_vector(status, image(), None),
        BOOT_VECTOR_BASE + 0x180
    );
}

#[test]
fn error_exception_vectors_match_r5000_reset_and_cache_entries() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
        boot_mode(),
    );

    assert_eq!(
        policy
            .error_exception_vector(Mips4Cp0Status::from_bits(0), Mips4ErrorException::SoftReset,),
        RESET_PC
    );
    assert_eq!(
        policy.error_exception_vector(
            Mips4Cp0Status::from_bits(0),
            Mips4ErrorException::NonMaskableInterrupt,
        ),
        RESET_PC
    );
    assert_eq!(
        policy.error_exception_vector(
            Mips4Cp0Status::from_bits(0),
            Mips4ErrorException::CacheError,
        ),
        NORMAL_CACHE_ERROR_VECTOR
    );
    assert_eq!(
        policy.error_exception_vector(
            Mips4Cp0Status::from_bits(1 << 22),
            Mips4ErrorException::CacheError,
        ),
        BOOT_VECTOR_BASE + 0x100
    );
}

#[test]
fn r5000_cca_mapping_has_no_coherent_cache_mode() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
        boot_mode(),
    );
    let attribute = |bits| {
        Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(
            Mips4CacheCoherenceAlgorithm::from_bits(bits).unwrap(),
        )
    };

    assert_eq!(
        policy.resolve_access_type(attribute(3)),
        Mips4MemoryAccessType::CachedNoncoherent
    );
    assert_eq!(
        policy.resolve_access_type(attribute(2)),
        Mips4MemoryAccessType::Uncached
    );
    assert_eq!(
        policy.resolve_access_type(attribute(7)),
        Mips4MemoryAccessType::Uncached
    );
    assert_eq!(
        policy.resolve_cache_policy(attribute(0)),
        Mips4CacheAccessPolicy::WriteThroughNoWriteAllocate
    );
    assert_eq!(
        policy.resolve_cache_policy(attribute(1)),
        Mips4CacheAccessPolicy::WriteThroughWriteAllocate
    );
    assert_eq!(
        policy.resolve_cache_policy(attribute(2)),
        Mips4CacheAccessPolicy::Uncached
    );
    assert_eq!(
        policy.resolve_cache_policy(attribute(3)),
        Mips4CacheAccessPolicy::WriteBackWriteAllocate
    );
}

#[test]
fn cache_validation_rejects_primary_and_secondary_configuration_conflicts() {
    let invalid_primary = R5000Profile::new(
        Mips4Endianness::Big,
        R5000Revision::from_bits(0x21),
        200_000_000,
        Mips4CacheConfig::present(16 * 1024, 32),
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::disabled(),
    );
    assert_eq!(
        R5000ExecutionPolicy::new(invalid_primary, boot_mode()).validate_cache_config(),
        Err(Mips4CacheConfigError::InvalidR5000PrimaryGeometry)
    );

    let enabled_boot = R5000BootMode::from_low_bits(1 << 12).unwrap();
    assert_eq!(
        R5000ExecutionPolicy::new(
            profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
            enabled_boot,
        )
        .validate_cache_config(),
        Err(Mips4CacheConfigError::R5000SecondaryBootConflict)
    );
    for (size_bits, size_bytes) in [(0, 512 * 1024), (1, 1024 * 1024), (2, 2048 * 1024)] {
        let boot = R5000BootMode::from_low_bits((1 << 12) | (size_bits << 16)).unwrap();
        assert_eq!(
            R5000ExecutionPolicy::new(
                profile(
                    Mips4Endianness::Big,
                    Mips4CacheConfig::present(size_bytes, 32),
                ),
                boot,
            )
            .validate_cache_config(),
            Ok(())
        );
    }
}

#[test]
fn r5000_cp0_wait_enters_standby() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
        boot_mode(),
    );
    assert_eq!(policy.cp0_wait_policy(), Mips4Cp0WaitPolicy::Standby);
}

#[test]
fn r5000_cp0_doubleword_transfer_checks_effective_mode() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
        boot_mode(),
    );
    let decision = |status| {
        policy.cp0_doubleword_transfer_policy(
            Mips4Cp0DoublewordTransferDirection::FromCp0,
            Mips4Cp0Status::from_bits(status),
            Mips4Cp0Register::Epc,
        )
    };

    assert_eq!(decision(0), Mips4Cp0DoublewordTransferPolicy::Execute);
    assert_eq!(decision(1 << 7), Mips4Cp0DoublewordTransferPolicy::Execute);
    assert_eq!(
        decision((1 << 28) | (1 << 3)),
        Mips4Cp0DoublewordTransferPolicy::ReservedInstruction
    );
    assert_eq!(
        decision((1 << 28) | (1 << 6) | (1 << 3)),
        Mips4Cp0DoublewordTransferPolicy::Execute
    );
    assert_eq!(
        decision((1 << 28) | (2 << 3)),
        Mips4Cp0DoublewordTransferPolicy::ReservedInstruction
    );
    assert_eq!(
        decision((1 << 28) | (1 << 5) | (2 << 3)),
        Mips4Cp0DoublewordTransferPolicy::Execute
    );
    assert_eq!(
        decision((2 << 3) | (1 << 1)),
        Mips4Cp0DoublewordTransferPolicy::Execute
    );
    assert_eq!(
        decision((2 << 3) | (1 << 2)),
        Mips4Cp0DoublewordTransferPolicy::Execute
    );
    assert_eq!(
        decision((1 << 28) | (3 << 3)),
        Mips4Cp0DoublewordTransferPolicy::ReservedInstruction
    );
}

#[test]
fn r5000_cp0_doubleword_transfer_classifies_every_register_width() {
    let policy = R5000ExecutionPolicy::new(
        profile(Mips4Endianness::Big, Mips4CacheConfig::disabled()),
        boot_mode(),
    );
    let wide = [
        Mips4Cp0Register::EntryLo0,
        Mips4Cp0Register::EntryLo1,
        Mips4Cp0Register::Context,
        Mips4Cp0Register::BadVaddr,
        Mips4Cp0Register::EntryHi,
        Mips4Cp0Register::Epc,
        Mips4Cp0Register::XContext,
        Mips4Cp0Register::ErrorEpc,
    ];
    for raw in 0..32 {
        let Some(register) = Mips4Cp0Register::from_u8(raw) else {
            continue;
        };
        let expected = if wide.contains(&register) {
            Mips4Cp0DoublewordTransferPolicy::Execute
        } else {
            Mips4Cp0DoublewordTransferPolicy::NoOperation
        };
        for direction in [
            Mips4Cp0DoublewordTransferDirection::FromCp0,
            Mips4Cp0DoublewordTransferDirection::ToCp0,
        ] {
            assert_eq!(
                policy.cp0_doubleword_transfer_policy(
                    direction,
                    Mips4Cp0Status::from_bits(0),
                    register,
                ),
                expected,
                "register {raw} direction {direction:?}"
            );
        }
    }
}
