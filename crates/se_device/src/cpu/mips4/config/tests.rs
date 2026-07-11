use super::*;

#[test]
fn config_preserves_raw_processor_id() {
    let config = Mips4Config::new(
        Mips4Endianness::Big,
        0x1234_5678,
        Mips4AddressConfig::new(36, 40),
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::disabled(),
        Mips4CacheConfig::disabled(),
        Mips4CoprocessorConfig::new(false, false),
    );

    assert_eq!(config.processor_id, 0x1234_5678);
}

#[test]
fn address_width_configuration_is_preserved() {
    let address = Mips4AddressConfig::new(36, 40);

    assert_eq!(address.physical_address_bits, 36);
    assert_eq!(address.virtual_address_bits, 40);
}

#[test]
fn caches_can_use_distinct_geometry() {
    let config = Mips4Config::new(
        Mips4Endianness::Little,
        0,
        Mips4AddressConfig::new(36, 40),
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(32 * 1024, 32),
        Mips4CacheConfig::present(512 * 1024, 32),
        Mips4CoprocessorConfig::new(false, false),
    );

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
    assert_eq!(
        config.secondary_cache,
        Mips4CacheConfig::Present {
            size_bytes: 512 * 1024,
            line_size_bytes: 32,
        }
    );
    assert!(config.secondary_cache.is_present());
    assert_eq!(config.secondary_cache.size_bytes(), Some(512 * 1024));
    assert_eq!(config.secondary_cache.line_size_bytes(), Some(32));
}

#[test]
fn coprocessor_availability_is_independent() {
    let coprocessors = Mips4CoprocessorConfig::new(true, false);

    assert!(coprocessors.cp0());
    assert!(coprocessors.cp1);
    assert!(!coprocessors.cp2);
}

#[test]
fn disabled_cache_has_no_geometry() {
    let cache = Mips4CacheConfig::disabled();

    assert_eq!(cache, Mips4CacheConfig::Disabled);
    assert!(!cache.is_present());
    assert_eq!(cache.size_bytes(), None);
    assert_eq!(cache.line_size_bytes(), None);
}

#[test]
fn effective_cpu_endianness_applies_reverse_endian_xor() {
    assert_eq!(
        Mips4Endianness::Big.effective_cpu_endianness(false),
        Mips4Endianness::Big
    );
    assert_eq!(
        Mips4Endianness::Big.effective_cpu_endianness(true),
        Mips4Endianness::Little
    );
    assert_eq!(
        Mips4Endianness::Little.effective_cpu_endianness(false),
        Mips4Endianness::Little
    );
    assert_eq!(
        Mips4Endianness::Little.effective_cpu_endianness(true),
        Mips4Endianness::Big
    );
}
