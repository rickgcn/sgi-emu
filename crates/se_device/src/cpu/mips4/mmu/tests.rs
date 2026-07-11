use super::*;

const STATUS_KX: u32 = 1 << 7;
const STATUS_SX: u32 = 1 << 6;
const STATUS_UX: u32 = 1 << 5;
const STATUS_KSU_SHIFT: u8 = 3;
const STATUS_ERL: u32 = 1 << 2;
const STATUS_EXL: u32 = 1 << 1;

const ASID: Mips4TlbAsid = Mips4TlbAsid::new(0x22);

fn cca(bits: u8) -> Mips4CacheCoherenceAlgorithm {
    Mips4CacheCoherenceAlgorithm::from_bits(bits).unwrap()
}

fn config() -> Mips4MmuConfig {
    Mips4MmuConfig::new(
        crate::cpu::mips4::config::Mips4AddressConfig::new(36, 40),
        cca(3),
    )
}

fn width_config(physical_address_bits: u8, virtual_address_bits: u8) -> Mips4MmuConfig {
    Mips4MmuConfig::new(
        crate::cpu::mips4::config::Mips4AddressConfig::new(
            physical_address_bits,
            virtual_address_bits,
        ),
        cca(3),
    )
}

fn status(bits: u32) -> Mips4Cp0Status {
    Mips4Cp0Status::from_bits(bits)
}

fn kernel_status(extra_bits: u32) -> Mips4Cp0Status {
    status(extra_bits)
}

fn supervisor_status(extra_bits: u32) -> Mips4Cp0Status {
    status((1 << STATUS_KSU_SHIFT) | extra_bits)
}

fn user_status(extra_bits: u32) -> Mips4Cp0Status {
    status((2 << STATUS_KSU_SHIFT) | extra_bits)
}

fn reserved_status(extra_bits: u32) -> Mips4Cp0Status {
    status((3 << STATUS_KSU_SHIFT) | extra_bits)
}

fn mapped_classification(
    segment: Mips4MmuSegment,
    address_mode: Mips4TlbAddressMode,
) -> Mips4MmuAddressClassification {
    Mips4MmuAddressClassification::Mapped {
        segment,
        address_mode,
    }
}

fn unmapped_classification(
    segment: Mips4MmuSegment,
    physical_address: u64,
    cache_attribute: Mips4MmuCacheAttribute,
) -> Mips4MmuAddressClassification {
    Mips4MmuAddressClassification::Unmapped {
        segment,
        physical_address,
        cache_attribute,
    }
}

fn entry_lo(
    pfn: u64,
    cache_coherence_algorithm: u8,
    dirty: bool,
    valid: bool,
) -> crate::cpu::mips4::tlb::Mips4TlbEntryLo {
    crate::cpu::mips4::tlb::Mips4TlbEntryLo::from_parts(
        pfn,
        cca(cache_coherence_algorithm),
        dirty,
        valid,
        false,
    )
    .unwrap()
}

fn entry_for(
    address: u64,
    address_mode: Mips4TlbAddressMode,
    even_page: crate::cpu::mips4::tlb::Mips4TlbEntryLo,
    odd_page: crate::cpu::mips4::tlb::Mips4TlbEntryLo,
) -> Mips4TlbEntry {
    let page_mask =
        crate::cpu::mips4::tlb::Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size4KiB);
    let entry_hi = crate::cpu::mips4::tlb::Mips4TlbEntryHi::from_virtual_address(
        address,
        ASID,
        page_mask,
        address_mode,
    );

    Mips4TlbEntry::new(page_mask, entry_hi, even_page, odd_page)
}

fn fault_result(
    exception: Mips4Exception,
    bad_virtual_address: u64,
    segment: Option<Mips4MmuSegment>,
    address_mode: Option<Mips4TlbAddressMode>,
) -> Mips4MmuTranslationResult {
    Mips4MmuTranslationResult::Fault(Mips4MmuFault {
        exception,
        bad_virtual_address,
        segment,
        address_mode,
    })
}

#[test]
fn status_mode_derives_effective_privilege() {
    assert_eq!(
        Mips4MmuPrivilegeMode::from_status(user_status(0)),
        Some(Mips4MmuPrivilegeMode::User)
    );
    assert_eq!(
        Mips4MmuPrivilegeMode::from_status(supervisor_status(0)),
        Some(Mips4MmuPrivilegeMode::Supervisor)
    );
    assert_eq!(
        Mips4MmuPrivilegeMode::from_status(kernel_status(0)),
        Some(Mips4MmuPrivilegeMode::Kernel)
    );
    assert_eq!(Mips4MmuPrivilegeMode::from_status(reserved_status(0)), None);
    assert_eq!(
        Mips4MmuPrivilegeMode::from_status(user_status(STATUS_EXL)),
        Some(Mips4MmuPrivilegeMode::Kernel)
    );
    assert_eq!(
        Mips4MmuPrivilegeMode::from_status(supervisor_status(STATUS_ERL)),
        Some(Mips4MmuPrivilegeMode::Kernel)
    );
}

#[test]
fn user_mode_classifies_32_and_64_bit_user_segments() {
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), user_status(0), 0x0000_0000_7fff_ffff),
        mapped_classification(Mips4MmuSegment::Useg, Mips4TlbAddressMode::Bits32)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), user_status(0), 0x0000_0000_8000_0000),
        Mips4MmuAddressClassification::AddressError { segment: None }
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), user_status(STATUS_UX), 0x0000_00ff_ffff_ffff),
        mapped_classification(Mips4MmuSegment::Xuseg, Mips4TlbAddressMode::Bits64)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), user_status(STATUS_UX), 0x0000_0100_0000_0000),
        Mips4MmuAddressClassification::AddressError { segment: None }
    );
}

#[test]
fn configured_virtual_width_rejects_the_first_unimplemented_address() {
    let config = width_config(36, 39);
    assert!(matches!(
        Mips4Mmu::classify_virtual_address(config, user_status(STATUS_UX), (1_u64 << 39) - 1,),
        Mips4MmuAddressClassification::Mapped { .. }
    ));
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config, user_status(STATUS_UX), 1_u64 << 39),
        Mips4MmuAddressClassification::AddressError { segment: None }
    );
}

#[test]
fn configured_physical_width_limits_xkphys_and_tlb_results() {
    let config = width_config(35, 40);
    let xkphys_base = 0x9000_0000_0000_0000;
    assert!(matches!(
        Mips4Mmu::classify_virtual_address(
            config,
            kernel_status(STATUS_KX),
            xkphys_base | ((1_u64 << 35) - 1),
        ),
        Mips4MmuAddressClassification::Unmapped { .. }
    ));
    assert_eq!(
        Mips4Mmu::classify_virtual_address(
            config,
            kernel_status(STATUS_KX),
            xkphys_base | (1_u64 << 35),
        ),
        Mips4MmuAddressClassification::AddressError {
            segment: Some(Mips4MmuSegment::Xkphys),
        }
    );

    let address = 0x1000;
    let out_of_range = entry_lo(1_u64 << 23, 3, true, true);
    let entries = [entry_for(
        address,
        Mips4TlbAddressMode::Bits64,
        out_of_range,
        out_of_range,
    )];
    assert_eq!(
        Mips4Mmu::translate(
            config,
            user_status(STATUS_UX),
            ASID,
            &entries,
            address,
            Mips4TlbAccessKind::Load,
        ),
        fault_result(
            Mips4Exception::AddressErrorLoad,
            address,
            Some(Mips4MmuSegment::Xuseg),
            Some(Mips4TlbAddressMode::Bits64),
        )
    );
}

#[test]
fn supervisor_mode_classifies_user_and_supervisor_spaces() {
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), supervisor_status(0), 0x0000_0000_1234_5678),
        mapped_classification(Mips4MmuSegment::Suseg, Mips4TlbAddressMode::Bits32)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(
            config(),
            supervisor_status(STATUS_UX),
            0x0000_00ff_ffff_ffff
        ),
        mapped_classification(Mips4MmuSegment::Xsuseg, Mips4TlbAddressMode::Bits64)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(
            config(),
            supervisor_status(STATUS_SX),
            0x4000_0000_1000_0000
        ),
        mapped_classification(Mips4MmuSegment::Xsseg, Mips4TlbAddressMode::Bits64)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), supervisor_status(0), 0xffff_ffff_c000_1000),
        mapped_classification(Mips4MmuSegment::Sseg, Mips4TlbAddressMode::Bits32)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(
            config(),
            supervisor_status(STATUS_SX),
            0xffff_ffff_c000_1000
        ),
        mapped_classification(Mips4MmuSegment::Csseg, Mips4TlbAddressMode::Bits32)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(
            config(),
            supervisor_status(STATUS_SX),
            0xffff_ffff_8000_0000
        ),
        Mips4MmuAddressClassification::AddressError { segment: None }
    );
}

#[test]
fn kernel_mode_classifies_32_bit_compatibility_spaces() {
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), kernel_status(0), 0x0000_0000_7fff_ffff),
        mapped_classification(Mips4MmuSegment::Kuseg, Mips4TlbAddressMode::Bits32)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), kernel_status(0), 0xffff_ffff_8000_0010),
        unmapped_classification(
            Mips4MmuSegment::Kseg0,
            0x10,
            Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(3))
        )
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), kernel_status(0), 0xffff_ffff_a000_0020),
        unmapped_classification(
            Mips4MmuSegment::Kseg1,
            0x20,
            Mips4MmuCacheAttribute::Uncached
        )
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), kernel_status(0), 0xffff_ffff_c000_1000),
        mapped_classification(Mips4MmuSegment::Ksseg, Mips4TlbAddressMode::Bits32)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), kernel_status(0), 0xffff_ffff_e000_1000),
        mapped_classification(Mips4MmuSegment::Kseg3, Mips4TlbAddressMode::Bits32)
    );
}

#[test]
fn kernel_mode_classifies_64_bit_kernel_spaces() {
    let status = kernel_status(STATUS_KX | STATUS_SX | STATUS_UX);

    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0x0000_00ff_ffff_ffff),
        mapped_classification(Mips4MmuSegment::Xkuseg, Mips4TlbAddressMode::Bits64)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0x4000_0000_0000_1000),
        mapped_classification(Mips4MmuSegment::Xksseg, Mips4TlbAddressMode::Bits64)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0xc000_0000_0000_1000),
        mapped_classification(Mips4MmuSegment::Xkseg, Mips4TlbAddressMode::Bits64)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0xffff_ffff_8000_0010),
        unmapped_classification(
            Mips4MmuSegment::Ckseg0,
            0x10,
            Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(3))
        )
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0xffff_ffff_a000_0020),
        unmapped_classification(
            Mips4MmuSegment::Ckseg1,
            0x20,
            Mips4MmuCacheAttribute::Uncached
        )
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0xffff_ffff_c000_1000),
        mapped_classification(Mips4MmuSegment::Cksseg, Mips4TlbAddressMode::Bits32)
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0xffff_ffff_e000_1000),
        mapped_classification(Mips4MmuSegment::Ckseg3, Mips4TlbAddressMode::Bits32)
    );
}

#[test]
fn xkphys_classification_extracts_physical_address_and_raw_cca() {
    let status = kernel_status(STATUS_KX);

    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0x9000_0001_2345_6789),
        unmapped_classification(
            Mips4MmuSegment::Xkphys,
            0x0000_0001_2345_6789,
            Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(2))
        )
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0xb800_0000_0000_1234),
        unmapped_classification(
            Mips4MmuSegment::Xkphys,
            0x1234,
            Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(7))
        )
    );
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0x8000_0010_0000_0000),
        Mips4MmuAddressClassification::AddressError {
            segment: Some(Mips4MmuSegment::Xkphys)
        }
    );
}

#[test]
fn erl_low_user_region_is_unmapped_uncached() {
    let status = user_status(STATUS_ERL | STATUS_UX | STATUS_KX);

    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), status, 0x0000_0000_1234_5678),
        unmapped_classification(
            Mips4MmuSegment::Xkuseg,
            0x1234_5678,
            Mips4MmuCacheAttribute::Uncached
        )
    );
}

#[test]
fn invalid_status_mode_classifies_invalid_mode() {
    assert_eq!(
        Mips4Mmu::classify_virtual_address(config(), reserved_status(0), 0),
        Mips4MmuAddressClassification::InvalidMode
    );
}

#[test]
fn unmapped_translation_returns_direct_physical_address() {
    assert_eq!(
        Mips4Mmu::translate(
            config(),
            kernel_status(0),
            ASID,
            &[],
            0xffff_ffff_8000_1234,
            Mips4TlbAccessKind::Load
        ),
        Mips4MmuTranslationResult::Hit(Mips4MmuTranslation {
            physical_address: 0x1234,
            segment: Mips4MmuSegment::Kseg0,
            source: Mips4MmuTranslationSource::Unmapped,
            cache_attribute: Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(3)),
        })
    );
    assert_eq!(
        Mips4Mmu::translate(
            config(),
            kernel_status(STATUS_KX),
            ASID,
            &[],
            0x9000_0000_0000_1234,
            Mips4TlbAccessKind::Store
        ),
        Mips4MmuTranslationResult::Hit(Mips4MmuTranslation {
            physical_address: 0x1234,
            segment: Mips4MmuSegment::Xkphys,
            source: Mips4MmuTranslationSource::Unmapped,
            cache_attribute: Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(2)),
        })
    );
}

#[test]
fn mapped_translation_returns_tlb_hit_metadata() {
    let address = 0x0000_0000_1234_4004;
    let entry = entry_for(
        address,
        Mips4TlbAddressMode::Bits64,
        entry_lo(0x12345, 3, true, true),
        entry_lo(0x54321, 3, true, true),
    );

    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(STATUS_UX),
            ASID,
            &[entry],
            address,
            Mips4TlbAccessKind::Load
        ),
        Mips4MmuTranslationResult::Hit(Mips4MmuTranslation {
            physical_address: 0x1234_5004,
            segment: Mips4MmuSegment::Xuseg,
            source: Mips4MmuTranslationSource::Mapped {
                address_mode: Mips4TlbAddressMode::Bits64,
                page_size: Mips4TlbPageSize::Size4KiB,
                page_half: Mips4TlbPageHalf::Even,
            },
            cache_attribute: Mips4MmuCacheAttribute::CacheCoherenceAlgorithm(cca(3)),
        })
    );
}

#[test]
fn mapped_translation_reports_tlb_faults() {
    let address = 0x0000_0000_1234_4004;

    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(STATUS_UX),
            ASID,
            &[],
            address,
            Mips4TlbAccessKind::Load
        ),
        fault_result(
            Mips4Exception::TlbLoad,
            address,
            Some(Mips4MmuSegment::Xuseg),
            Some(Mips4TlbAddressMode::Bits64)
        )
    );

    let invalid_entry = entry_for(
        address,
        Mips4TlbAddressMode::Bits64,
        entry_lo(0x12345, 3, true, false),
        entry_lo(0x54321, 3, true, false),
    );
    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(STATUS_UX),
            ASID,
            &[invalid_entry],
            address,
            Mips4TlbAccessKind::Load
        ),
        fault_result(
            Mips4Exception::TlbLoad,
            address,
            Some(Mips4MmuSegment::Xuseg),
            Some(Mips4TlbAddressMode::Bits64)
        )
    );

    let clean_entry = entry_for(
        address,
        Mips4TlbAddressMode::Bits64,
        entry_lo(0x12345, 3, false, true),
        entry_lo(0x54321, 3, false, true),
    );
    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(STATUS_UX),
            ASID,
            &[clean_entry],
            address,
            Mips4TlbAccessKind::Store
        ),
        fault_result(
            Mips4Exception::TlbModification,
            address,
            Some(Mips4MmuSegment::Xuseg),
            Some(Mips4TlbAddressMode::Bits64)
        )
    );
}

#[test]
fn mapped_translation_preserves_multiple_tlb_match_as_undefined() {
    let address = 0x0000_0000_1234_4004;
    let entry = entry_for(
        address,
        Mips4TlbAddressMode::Bits64,
        entry_lo(0x12345, 3, true, true),
        entry_lo(0x54321, 3, true, true),
    );

    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(STATUS_UX),
            ASID,
            &[entry, entry],
            address,
            Mips4TlbAccessKind::Load
        ),
        Mips4MmuTranslationResult::UndefinedMultipleTlbMatch {
            segment: Mips4MmuSegment::Xuseg,
            address_mode: Mips4TlbAddressMode::Bits64,
        }
    );
}

#[test]
fn address_errors_use_load_or_store_exception_code() {
    let address = 0xffff_ffff_8000_0000;

    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(0),
            ASID,
            &[],
            address,
            Mips4TlbAccessKind::InstructionFetch
        ),
        fault_result(Mips4Exception::AddressErrorLoad, address, None, None)
    );
    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(0),
            ASID,
            &[],
            address,
            Mips4TlbAccessKind::Load
        ),
        fault_result(Mips4Exception::AddressErrorLoad, address, None, None)
    );
    assert_eq!(
        Mips4Mmu::translate(
            config(),
            user_status(0),
            ASID,
            &[],
            address,
            Mips4TlbAccessKind::Store
        ),
        fault_result(Mips4Exception::AddressErrorStore, address, None, None)
    );
}
