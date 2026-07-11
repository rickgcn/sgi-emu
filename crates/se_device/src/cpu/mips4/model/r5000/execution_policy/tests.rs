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
}
