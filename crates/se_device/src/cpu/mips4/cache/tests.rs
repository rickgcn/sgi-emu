use super::*;

#[test]
fn cache_coherence_algorithm_accepts_only_three_bit_values() {
    for bits in 0..=7 {
        let cca = Mips4CacheCoherenceAlgorithm::from_bits(bits).unwrap();
        assert_eq!(cca.bits(), bits);
    }

    assert_eq!(Mips4CacheCoherenceAlgorithm::from_bits(8), None);
    assert_eq!(Mips4CacheCoherenceAlgorithm::from_bits(u8::MAX), None);
}

#[test]
fn memory_access_type_helpers_classify_generic_access_types() {
    assert!(Mips4MemoryAccessType::Uncached.is_uncached());
    assert!(!Mips4MemoryAccessType::Uncached.is_cached());
    assert!(!Mips4MemoryAccessType::Uncached.is_coherent());
    assert!(!Mips4MemoryAccessType::Uncached.is_ll_sc_eligible());

    assert!(Mips4MemoryAccessType::CachedNoncoherent.is_cached());
    assert!(!Mips4MemoryAccessType::CachedNoncoherent.is_uncached());
    assert!(!Mips4MemoryAccessType::CachedNoncoherent.is_coherent());
    assert!(Mips4MemoryAccessType::CachedNoncoherent.is_ll_sc_eligible());

    assert!(Mips4MemoryAccessType::CachedCoherent.is_cached());
    assert!(!Mips4MemoryAccessType::CachedCoherent.is_uncached());
    assert!(Mips4MemoryAccessType::CachedCoherent.is_coherent());
    assert!(Mips4MemoryAccessType::CachedCoherent.is_ll_sc_eligible());

    assert!(!Mips4MemoryAccessType::ImplementationSpecific.is_cached());
    assert!(!Mips4MemoryAccessType::ImplementationSpecific.is_uncached());
    assert!(!Mips4MemoryAccessType::ImplementationSpecific.is_coherent());
    assert!(!Mips4MemoryAccessType::ImplementationSpecific.is_ll_sc_eligible());
}

#[test]
fn memory_access_type_synchronizability_matches_sync_restriction() {
    assert!(Mips4MemoryAccessType::Uncached.is_synchronizable());
    assert!(Mips4MemoryAccessType::CachedCoherent.is_synchronizable());
    assert!(!Mips4MemoryAccessType::CachedNoncoherent.is_synchronizable());
    assert!(!Mips4MemoryAccessType::ImplementationSpecific.is_synchronizable());
}

#[test]
fn cache_instruction_decodes_only_cache_opcode() {
    let bits = ((MIPS4_CACHE_OPCODE as u32) << 26) | (4 << 21) | (0x15 << 16) | 0xfffc;
    let instruction = Mips4CacheInstruction::from_bits(bits).unwrap();

    assert_eq!(instruction.bits(), bits);
    assert_eq!(instruction.instruction(), Mips4Instruction::from_bits(bits));
    assert_eq!(instruction.base(), 4);
    assert_eq!(instruction.op(), 0x15);
    assert_eq!(instruction.raw_offset(), 0xfffc);
    assert_eq!(instruction.offset(), -4);

    assert_eq!(
        Mips4CacheInstruction::from_bits((0x23 << 26) | bits & 0x03ff_ffff),
        None
    );
}

#[test]
fn cache_instruction_splits_raw_op_bits_without_assigning_meaning() {
    let bits = ((MIPS4_CACHE_OPCODE as u32) << 26) | (0x1d << 16);
    let instruction = Mips4CacheInstruction::from_bits(bits).unwrap();

    assert_eq!(instruction.op(), 0b1_1101);
    assert_eq!(instruction.cache_selector_bits(), 0b01);
    assert_eq!(instruction.operation_bits(), 0b111);
}

#[test]
fn cache_line_geometry_requires_power_of_two_line_size() {
    assert_eq!(Mips4CacheLineGeometry::new(0), None);
    assert_eq!(Mips4CacheLineGeometry::new(24), None);
    assert_eq!(
        Mips4CacheLineGeometry::new(32),
        Some(Mips4CacheLineGeometry {
            line_size_bytes: 32,
        })
    );
}

#[test]
fn cache_line_geometry_computes_line_base_and_offset() {
    let geometry = Mips4CacheLineGeometry::new(32).unwrap();

    assert_eq!(geometry.line_offset(0x1000), 0);
    assert_eq!(geometry.line_offset(0x101f), 31);
    assert_eq!(geometry.line_base(0x1000), 0x1000);
    assert_eq!(geometry.line_base(0x101f), 0x1000);
    assert_eq!(
        geometry.line_base(0xffff_ffff_ffff_ffff),
        0xffff_ffff_ffff_ffe0
    );
}

#[test]
fn cache_line_geometry_computes_line_counts_and_indices() {
    let geometry = Mips4CacheLineGeometry::new(32).unwrap();

    assert_eq!(geometry.line_count(0), None);
    assert_eq!(geometry.line_count(48), None);
    assert_eq!(geometry.line_count(128), Some(4));

    assert_eq!(geometry.line_index(0x0000, 128), Some(0));
    assert_eq!(geometry.line_index(0x001f, 128), Some(0));
    assert_eq!(geometry.line_index(0x0020, 128), Some(1));
    assert_eq!(geometry.line_index(0x007f, 128), Some(3));
    assert_eq!(geometry.line_index(0x0080, 128), Some(0));
    assert_eq!(geometry.line_index(0x1000, 0), None);
}
