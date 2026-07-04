use super::*;

#[test]
fn config_preserves_raw_processor_id() {
    let config = Mips1Config::new(
        Mips1Endianness::Big,
        0x1234_5678,
        Mips1CacheConfig::disabled(),
        Mips1CacheConfig::disabled(),
        Mips1CoprocessorConfig::new(false, false, false),
    );

    assert_eq!(config.processor_id, 0x1234_5678);
}

#[test]
fn instruction_and_data_caches_can_use_distinct_geometry() {
    let config = Mips1Config::new(
        Mips1Endianness::Little,
        0,
        Mips1CacheConfig::present(4 * 1024, 16),
        Mips1CacheConfig::present(8 * 1024, 32),
        Mips1CoprocessorConfig::new(false, false, false),
    );

    assert_eq!(
        config.instruction_cache,
        Mips1CacheConfig::Present {
            size_bytes: 4 * 1024,
            line_size_bytes: 16,
        }
    );
    assert_eq!(
        config.data_cache,
        Mips1CacheConfig::Present {
            size_bytes: 8 * 1024,
            line_size_bytes: 32,
        }
    );
    assert!(config.instruction_cache.is_present());
    assert_eq!(config.instruction_cache.size_bytes(), Some(4 * 1024));
    assert_eq!(config.instruction_cache.line_size_bytes(), Some(16));
}

#[test]
fn coprocessor_availability_is_independent() {
    let coprocessors = Mips1CoprocessorConfig::new(true, false, true);

    assert!(coprocessors.cp0());
    assert!(coprocessors.cp1);
    assert!(!coprocessors.cp2);
    assert!(coprocessors.cp3);
}

#[test]
fn disabled_cache_has_no_geometry() {
    let cache = Mips1CacheConfig::disabled();

    assert_eq!(cache, Mips1CacheConfig::Disabled);
    assert!(!cache.is_present());
    assert_eq!(cache.size_bytes(), None);
    assert_eq!(cache.line_size_bytes(), None);
}
