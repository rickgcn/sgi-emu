use super::*;

const ASID_1: Mips4TlbAsid = Mips4TlbAsid::new(0x12);
const ASID_2: Mips4TlbAsid = Mips4TlbAsid::new(0x34);

fn cca(bits: u8) -> Mips4CacheCoherenceAlgorithm {
    Mips4CacheCoherenceAlgorithm::from_bits(bits).unwrap()
}

fn entry_lo(
    pfn: u64,
    cache_coherence_algorithm: u8,
    dirty: bool,
    valid: bool,
    global: bool,
) -> Mips4TlbEntryLo {
    Mips4TlbEntryLo::from_parts(pfn, cca(cache_coherence_algorithm), dirty, valid, global).unwrap()
}

fn entry_for(
    address: u64,
    page_size: Mips4TlbPageSize,
    asid: Mips4TlbAsid,
    even_page: Mips4TlbEntryLo,
    odd_page: Mips4TlbEntryLo,
) -> Mips4TlbEntry {
    let page_mask = Mips4TlbPageMask::from_page_size(page_size);
    let entry_hi = Mips4TlbEntryHi::from_virtual_address(
        address,
        asid,
        page_mask,
        Mips4TlbAddressMode::Bits64,
    );

    Mips4TlbEntry::new(page_mask, entry_hi, even_page, odd_page)
}

#[test]
fn asid_preserves_raw_bits() {
    assert_eq!(Mips4TlbAsid::new(0xab).bits(), 0xab);
}

#[test]
fn page_sizes_round_trip_from_byte_counts() {
    assert_eq!(
        Mips4TlbPageSize::from_bytes(0x0000_1000),
        Some(Mips4TlbPageSize::Size4KiB)
    );
    assert_eq!(
        Mips4TlbPageSize::from_bytes(0x0000_4000),
        Some(Mips4TlbPageSize::Size16KiB)
    );
    assert_eq!(
        Mips4TlbPageSize::from_bytes(0x0001_0000),
        Some(Mips4TlbPageSize::Size64KiB)
    );
    assert_eq!(
        Mips4TlbPageSize::from_bytes(0x0004_0000),
        Some(Mips4TlbPageSize::Size256KiB)
    );
    assert_eq!(
        Mips4TlbPageSize::from_bytes(0x0010_0000),
        Some(Mips4TlbPageSize::Size1MiB)
    );
    assert_eq!(
        Mips4TlbPageSize::from_bytes(0x0040_0000),
        Some(Mips4TlbPageSize::Size4MiB)
    );
    assert_eq!(
        Mips4TlbPageSize::from_bytes(0x0100_0000),
        Some(Mips4TlbPageSize::Size16MiB)
    );
    assert_eq!(Mips4TlbPageSize::from_bytes(0x2000), None);
}

#[test]
fn page_mask_accepts_only_defined_encodings() {
    let expected = [
        (Mips4TlbPageSize::Size4KiB, 0x0000_0000),
        (Mips4TlbPageSize::Size16KiB, 0x0000_6000),
        (Mips4TlbPageSize::Size64KiB, 0x0001_e000),
        (Mips4TlbPageSize::Size256KiB, 0x0007_e000),
        (Mips4TlbPageSize::Size1MiB, 0x001f_e000),
        (Mips4TlbPageSize::Size4MiB, 0x007f_e000),
        (Mips4TlbPageSize::Size16MiB, 0x01ff_e000),
    ];

    for (page_size, bits) in expected {
        let mask = Mips4TlbPageMask::from_page_size(page_size);
        assert_eq!(mask.bits(), bits);
        assert_eq!(mask.page_size(), page_size);
        assert_eq!(Mips4TlbPageMask::from_bits(bits), Some(mask));
    }

    assert_eq!(Mips4TlbPageMask::from_bits(0x0000_2000), None);
    assert_eq!(Mips4TlbPageMask::from_bits(0x0200_0000), None);
    assert_eq!(Mips4TlbPageMask::from_bits(1), None);
}

#[test]
fn page_mask_reports_offset_masks() {
    let mask_4k = Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size4KiB);
    assert_eq!(mask_4k.page_offset_mask(), 0x0fff);
    assert_eq!(mask_4k.page_pair_offset_mask(), 0x1fff);

    let mask_16m = Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size16MiB);
    assert_eq!(mask_16m.page_offset_mask(), 0x00ff_ffff);
    assert_eq!(mask_16m.page_pair_offset_mask(), 0x01ff_ffff);
}

#[test]
fn entry_lo_round_trips_raw_bits() {
    let entry = entry_lo(0x12_3456, 3, true, true, true);
    let bits = (0x12_3456 << 6) | (3 << 3) | 0b111;

    assert_eq!(entry.bits(), bits);
    assert_eq!(Mips4TlbEntryLo::from_bits(bits), Some(entry));
    assert_eq!(entry.pfn(), 0x12_3456);
    assert_eq!(entry.cache_coherence_algorithm(), cca(3));
    assert!(entry.dirty());
    assert!(entry.valid());
    assert!(entry.global());
}

#[test]
fn entry_lo_rejects_out_of_range_fields() {
    assert_eq!(
        Mips4TlbEntryLo::from_parts(0x1_000000, cca(0), false, false, false),
        None
    );
    assert_eq!(Mips4TlbEntryLo::from_bits(1 << 30), None);
}

#[test]
fn entry_hi_validates_raw_fields() {
    assert_eq!(
        Mips4TlbEntryHi::from_parts(ENTRY_HI_VPN2_64_MASK, ASID_1, 3)
            .unwrap()
            .vpn2(),
        ENTRY_HI_VPN2_64_MASK
    );
    assert_eq!(
        Mips4TlbEntryHi::from_parts(ENTRY_HI_VPN2_64_MASK + 1, ASID_1, 0),
        None
    );
    assert_eq!(Mips4TlbEntryHi::from_parts(0, ASID_1, 4), None);
}

#[test]
fn entry_hi_from_virtual_address_uses_64_bit_region_and_clears_masked_vpn2_bits() {
    let page_mask = Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size64KiB);
    let address = 0x4000_0001_2345_6000;
    let entry_hi = Mips4TlbEntryHi::from_virtual_address(
        address,
        ASID_1,
        page_mask,
        Mips4TlbAddressMode::Bits64,
    );

    assert_eq!(entry_hi.asid(), ASID_1);
    assert_eq!(entry_hi.region_bits(), 1);
    assert_eq!(entry_hi.vpn2() & page_mask.vpn2_mask_bits(), 0);
    assert_eq!(
        entry_hi.vpn2(),
        ((address >> ENTRY_HI_VPN2_SHIFT) & ENTRY_HI_VPN2_64_MASK) & !page_mask.vpn2_mask_bits()
    );
}

#[test]
fn entry_hi_from_virtual_address_uses_32_bit_vpn_without_region() {
    let page_mask = Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size4KiB);
    let address = 0xffff_ffff_8123_4000;
    let entry_hi = Mips4TlbEntryHi::from_virtual_address(
        address,
        ASID_1,
        page_mask,
        Mips4TlbAddressMode::Bits32,
    );

    assert_eq!(entry_hi.region_bits(), 0);
    assert_eq!(
        entry_hi.vpn2(),
        (address >> ENTRY_HI_VPN2_SHIFT) & ENTRY_HI_VPN2_32_MASK
    );
}

#[test]
fn entry_match_checks_asid_unless_both_halves_are_global() {
    let address = 0x0000_0000_1234_5000;
    let even = entry_lo(0x2000, 3, true, true, false);
    let odd = entry_lo(0x3000, 3, true, true, false);
    let entry = entry_for(address, Mips4TlbPageSize::Size4KiB, ASID_1, even, odd);

    assert!(entry.matches_virtual_address(address, ASID_1, Mips4TlbAddressMode::Bits64));
    assert!(!entry.matches_virtual_address(address, ASID_2, Mips4TlbAddressMode::Bits64));

    let half_global = entry_for(
        address,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, true, true),
        entry_lo(0x3000, 3, true, true, false),
    );
    assert!(!half_global.global());
    assert!(!half_global.matches_virtual_address(address, ASID_2, Mips4TlbAddressMode::Bits64));

    let global = entry_for(
        address,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, true, true),
        entry_lo(0x3000, 3, true, true, true),
    );
    assert!(global.global());
    assert!(global.matches_virtual_address(address, ASID_2, Mips4TlbAddressMode::Bits64));
}

#[test]
fn entry_match_ignores_page_masked_vpn2_bits() {
    let entry = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size64KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, true, false),
        entry_lo(0x3000, 3, true, true, false),
    );

    assert!(entry.matches_virtual_address(
        0x0000_0000_1000_f000,
        ASID_1,
        Mips4TlbAddressMode::Bits64
    ));
    assert!(entry.matches_virtual_address(
        0x0000_0000_1001_0000,
        ASID_1,
        Mips4TlbAddressMode::Bits64
    ));
    assert!(!entry.matches_virtual_address(
        0x0000_0000_1002_0000,
        ASID_1,
        Mips4TlbAddressMode::Bits64
    ));
}

#[test]
fn page_half_uses_page_size_bit() {
    let entry_4k = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, true, false),
        entry_lo(0x3000, 3, true, true, false),
    );
    assert_eq!(entry_4k.page_half(0x1000_0fff), Mips4TlbPageHalf::Even);
    assert_eq!(entry_4k.page_half(0x1000_1000), Mips4TlbPageHalf::Odd);

    let entry_16k = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size16KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, true, false),
        entry_lo(0x3000, 3, true, true, false),
    );
    assert_eq!(entry_16k.page_half(0x1000_3fff), Mips4TlbPageHalf::Even);
    assert_eq!(entry_16k.page_half(0x1000_4000), Mips4TlbPageHalf::Odd);
}

#[test]
fn translation_reports_miss_for_non_matching_entry() {
    let entry = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, true, false),
        entry_lo(0x3000, 3, true, true, false),
    );

    assert_eq!(
        entry.translate(
            0x0000_0000_2000_0000,
            ASID_1,
            Mips4TlbAccessKind::Load,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbTranslationResult::Miss {
            exception: Mips4Exception::TlbLoad,
        }
    );
    assert_eq!(
        entry.translate(
            0x0000_0000_2000_0000,
            ASID_1,
            Mips4TlbAccessKind::Store,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbTranslationResult::Miss {
            exception: Mips4Exception::TlbStore,
        }
    );
}

#[test]
fn translation_reports_invalid_for_clear_valid_bit() {
    let entry = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, false, false),
        entry_lo(0x3000, 3, true, true, false),
    );

    assert_eq!(
        entry.translate(
            0x0000_0000_1000_0000,
            ASID_1,
            Mips4TlbAccessKind::InstructionFetch,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbTranslationResult::Invalid {
            exception: Mips4Exception::TlbLoad,
        }
    );
}

#[test]
fn translation_reports_modified_for_store_to_clean_page() {
    let entry = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 3, false, true, false),
        entry_lo(0x3000, 3, true, true, false),
    );

    assert_eq!(
        entry.translate(
            0x0000_0000_1000_0000,
            ASID_1,
            Mips4TlbAccessKind::Store,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbTranslationResult::Modified {
            exception: Mips4Exception::TlbModification,
        }
    );
}

#[test]
fn translation_returns_physical_address_and_cca_for_hit() {
    let entry = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 2, true, true, false),
        entry_lo(0x3000, 3, true, true, false),
    );

    assert_eq!(
        entry.translate(
            0x0000_0000_1000_0123,
            ASID_1,
            Mips4TlbAccessKind::Load,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbTranslationResult::Hit(Mips4TlbTranslation {
            physical_address: 0x0200_0123,
            cache_coherence_algorithm: cca(2),
            page_size: Mips4TlbPageSize::Size4KiB,
            page_half: Mips4TlbPageHalf::Even,
        })
    );
    assert_eq!(
        entry.translate(
            0x0000_0000_1000_1123,
            ASID_1,
            Mips4TlbAccessKind::Load,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbTranslationResult::Hit(Mips4TlbTranslation {
            physical_address: 0x0300_0123,
            cache_coherence_algorithm: cca(3),
            page_size: Mips4TlbPageSize::Size4KiB,
            page_half: Mips4TlbPageHalf::Odd,
        })
    );
}

#[test]
fn large_page_translation_masks_pfn_bits_covered_by_page_offset() {
    let entry = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size16KiB,
        ASID_1,
        entry_lo(0x12_3455, 3, true, true, false),
        entry_lo(0x20_0000, 3, true, true, false),
    );

    assert_eq!(
        entry.translate(
            0x0000_0000_1000_0123,
            ASID_1,
            Mips4TlbAccessKind::Load,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbTranslationResult::Hit(Mips4TlbTranslation {
            physical_address: 0x1_2345_4123,
            cache_coherence_algorithm: cca(3),
            page_size: Mips4TlbPageSize::Size16KiB,
            page_half: Mips4TlbPageHalf::Even,
        })
    );
}

#[test]
fn probe_reports_miss_hit_and_multiple_matches() {
    let first = entry_for(
        0x0000_0000_1000_0000,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x2000, 3, true, true, false),
        entry_lo(0x3000, 3, true, true, false),
    );
    let second = entry_for(
        0x0000_0000_2000_0000,
        Mips4TlbPageSize::Size4KiB,
        ASID_1,
        entry_lo(0x4000, 3, true, true, false),
        entry_lo(0x5000, 3, true, true, false),
    );
    let duplicate = first;

    assert_eq!(
        Mips4Tlb::probe(
            &[first, second],
            0x0000_0000_3000_0000,
            ASID_1,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbProbeResult::Miss
    );
    assert_eq!(
        Mips4Tlb::probe(
            &[first, second],
            0x0000_0000_2000_0000,
            ASID_1,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbProbeResult::Hit { index: 1 }
    );
    assert_eq!(
        Mips4Tlb::probe(
            &[first, second, duplicate],
            0x0000_0000_1000_0000,
            ASID_1,
            Mips4TlbAddressMode::Bits64,
        ),
        Mips4TlbProbeResult::Multiple
    );
}
