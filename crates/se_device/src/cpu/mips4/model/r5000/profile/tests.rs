use super::*;

#[test]
fn profile_preserves_configurable_fields() {
    let profile = R5000Profile::new(
        Mips4Endianness::Big,
        R5000Revision::from_bits(0x21),
        200_000_000,
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(1024 * 1024, 32),
    );

    assert_eq!(profile.endianness, Mips4Endianness::Big);
    assert_eq!(profile.revision.bits(), 0x21);
    assert_eq!(profile.processor_frequency_hz, 200_000_000);
    assert_eq!(
        profile.instruction_cache,
        Mips4CacheConfig::Present {
            size_bytes: 32 * 1024,
            line_size_bytes: 32,
        }
    );
    assert_eq!(
        profile.data_cache,
        Mips4CacheConfig::Present {
            size_bytes: 32 * 1024,
            line_size_bytes: 32,
        }
    );
    assert_eq!(
        profile.secondary_cache,
        Mips4CacheConfig::Present {
            size_bytes: 1024 * 1024,
            line_size_bytes: 32,
        }
    );
}

#[test]
fn profile_reports_identity_and_tlb_geometry() {
    let profile = R5000Profile::new(
        Mips4Endianness::Little,
        R5000Revision::from_bits(0x12),
        180_000_000,
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::disabled(),
    );

    assert_eq!(profile.processor_id(), 0x0000_2312);
    assert_eq!(profile.fcr0(), 0x0000_2312);
    assert_eq!(profile.tlb_entry_count(), 48);
    assert_eq!(profile.tlb_random_upper_bound(), 47);
}

#[test]
fn profile_builds_generic_mips4_config() {
    let profile = R5000Profile::new(
        Mips4Endianness::Little,
        R5000Revision::from_bits(0x10),
        150_000_000,
        Mips4CacheConfig::present(
            R5000_PRIMARY_INSTRUCTION_CACHE_SIZE_BYTES,
            R5000_PRIMARY_CACHE_LINE_SIZE_BYTES,
        ),
        Mips4CacheConfig::present(
            R5000_PRIMARY_DATA_CACHE_SIZE_BYTES,
            R5000_PRIMARY_CACHE_LINE_SIZE_BYTES,
        ),
        Mips4CacheConfig::disabled(),
    );
    let config = profile.to_mips4_config();

    assert_eq!(config.endianness, Mips4Endianness::Little);
    assert_eq!(config.processor_id, 0x0000_2310);
    assert_eq!(config.address.physical_address_bits, 36);
    assert_eq!(config.address.virtual_address_bits, 40);
    assert_eq!(
        config.instruction_cache,
        Mips4CacheConfig::Present {
            size_bytes: 32 * 1024,
            line_size_bytes: 32,
        }
    );
    assert_eq!(
        config.data_cache,
        Mips4CacheConfig::Present {
            size_bytes: 32 * 1024,
            line_size_bytes: 32,
        }
    );
    assert_eq!(config.secondary_cache, Mips4CacheConfig::Disabled);
    assert!(config.coprocessors.cp0());
    assert!(config.coprocessors.cp1);
    assert!(!config.coprocessors.cp2);
    assert!(config.coprocessors.cp3);
}

#[test]
fn secondary_cache_remains_caller_configurable() {
    let no_secondary = R5000Profile::new(
        Mips4Endianness::Big,
        R5000Revision::from_bits(0),
        1,
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::disabled(),
    );
    let custom_secondary = R5000Profile::new(
        Mips4Endianness::Big,
        R5000Revision::from_bits(0),
        1,
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::present(2 * 1024 * 1024, 32),
    );

    assert_eq!(
        no_secondary.to_mips4_config().secondary_cache,
        Mips4CacheConfig::Disabled
    );
    assert_eq!(
        custom_secondary.to_mips4_config().secondary_cache,
        Mips4CacheConfig::Present {
            size_bytes: 2 * 1024 * 1024,
            line_size_bytes: 32,
        }
    );
}
