use super::*;
use crate::cpu::mips4::tlb::{Mips4TlbPageMask, Mips4TlbPageSize};

#[test]
fn register_numbers_accept_modeled_mips4_registers() {
    let registers = [
        (0, Mips4Cp0Register::Index),
        (1, Mips4Cp0Register::Random),
        (2, Mips4Cp0Register::EntryLo0),
        (3, Mips4Cp0Register::EntryLo1),
        (4, Mips4Cp0Register::Context),
        (5, Mips4Cp0Register::PageMask),
        (6, Mips4Cp0Register::Wired),
        (8, Mips4Cp0Register::BadVaddr),
        (9, Mips4Cp0Register::Count),
        (10, Mips4Cp0Register::EntryHi),
        (11, Mips4Cp0Register::Compare),
        (12, Mips4Cp0Register::Status),
        (13, Mips4Cp0Register::Cause),
        (14, Mips4Cp0Register::Epc),
        (15, Mips4Cp0Register::ProcessorId),
        (16, Mips4Cp0Register::Config),
        (17, Mips4Cp0Register::LlAddr),
        (20, Mips4Cp0Register::XContext),
        (26, Mips4Cp0Register::Ecc),
        (27, Mips4Cp0Register::CacheErr),
        (28, Mips4Cp0Register::TagLo),
        (29, Mips4Cp0Register::TagHi),
        (30, Mips4Cp0Register::ErrorEpc),
    ];

    for (number, register) in registers {
        assert_eq!(Mips4Cp0Register::from_u8(number), Some(register));
        assert_eq!(register.number(), number);
    }
}

#[test]
fn register_numbers_reject_reserved_registers() {
    for register in [7, 18, 19, 21, 22, 23, 24, 25, 31] {
        assert_eq!(Mips4Cp0Register::from_u8(register), None);
    }
}

#[test]
fn mmu_register_wrappers_mask_reserved_bits() {
    let index = Mips4Cp0Index::from_bits(u64::MAX);
    assert_eq!(index.bits(), INDEX_READABLE_MASK as u32);
    assert!(index.probe_failure());
    assert_eq!(index.index(), 63);

    let random = Mips4Cp0Random::from_bits(u64::MAX);
    assert_eq!(random.bits(), RANDOM_INDEX_MASK as u32);
    assert_eq!(random.index(), 63);

    let wired = Mips4Cp0Wired::from_bits(u64::MAX);
    assert_eq!(wired.bits(), WIRED_INDEX_MASK as u32);
    assert_eq!(wired.boundary(), 63);

    let entry_lo = Mips4Cp0EntryLo::from_bits(u64::MAX);
    assert_eq!(entry_lo.bits(), ENTRY_LO_READABLE_MASK as u32);
    assert_eq!(entry_lo.page_frame_number(), 0x00ff_ffff);
    assert_eq!(entry_lo.cache_coherence_algorithm(), 7);
    assert!(entry_lo.dirty());
    assert!(entry_lo.valid());
    assert!(entry_lo.global());

    let page_mask = Mips4Cp0PageMask::from_bits(u64::MAX);
    assert_eq!(page_mask.bits(), PAGE_MASK_MASK as u32);

    let entry_hi = Mips4Cp0EntryHi::from_bits(u64::MAX);
    assert_eq!(entry_hi.bits(), ENTRY_HI_READABLE_MASK);
    assert_eq!(entry_hi.region_bits(), 3);
    assert_eq!(entry_hi.virtual_page_number2(), ENTRY_HI_VPN2_MASK);
    assert_eq!(entry_hi.address_space_identifier(), 0xff);
}

#[test]
fn context_wrappers_extract_fields() {
    let context_bits = (0x1_2345_6789_u64 << CONTEXT_PTE_BASE_SHIFT)
        | (0x5_4321_u64 << CONTEXT_BAD_VPN2_SHIFT)
        | 0x0f;
    let context = Mips4Cp0Context::from_bits(context_bits);

    assert_eq!(context.bits() & 0x0f, 0);
    assert_eq!(
        context.page_table_entry_base(),
        context_bits >> CONTEXT_PTE_BASE_SHIFT
    );
    assert_eq!(context.bad_virtual_page_number2(), 0x5_4321);

    let x_context_bits = (0x1234_5678_u64 << XCONTEXT_PTE_BASE_SHIFT)
        | (2 << XCONTEXT_REGION_SHIFT)
        | (0x654_3210_u64 << XCONTEXT_BAD_VPN2_SHIFT)
        | 0x0f;
    let x_context = Mips4Cp0XContext::from_bits(x_context_bits);

    assert_eq!(x_context.bits() & 0x0f, 0);
    assert_eq!(x_context.page_table_entry_base(), 0x1234_5678);
    assert_eq!(x_context.region_bits(), 2);
    assert_eq!(x_context.bad_virtual_page_number2(), 0x654_3210);
}

#[test]
fn status_exposes_control_fields() {
    let status = Mips4Cp0Status::from_bits(
        STATUS_XX
            | (0b0101 << STATUS_CU_SHIFT)
            | STATUS_FR
            | STATUS_RE
            | STATUS_BEV
            | STATUS_SR
            | STATUS_CE
            | STATUS_DE
            | (0xa5 << STATUS_IM_SHIFT)
            | STATUS_KX
            | STATUS_SX
            | STATUS_UX
            | (Mips4Cp0KernelUserMode::User.bits() as u32) << STATUS_KSU_SHIFT
            | STATUS_IE,
    );

    assert!(status.xx());
    assert_eq!(status.cu(), 0b0101);
    assert!(status.coprocessor_usable(Mips4CoprocessorNumber::Cp0));
    assert!(!status.coprocessor_usable(Mips4CoprocessorNumber::Cp1));
    assert!(status.coprocessor_usable(Mips4CoprocessorNumber::Cp2));
    assert!(!status.coprocessor_usable(Mips4CoprocessorNumber::Cp3));
    assert!(status.additional_float_registers());
    assert!(status.reverse_endianness());
    assert!(status.boot_exception_vectors());
    assert!(status.soft_reset_or_nmi());
    assert!(status.cache_check_bits());
    assert!(status.cache_error_disabled());
    assert_eq!(status.interrupt_mask(), 0xa5);
    assert!(status.kernel_64_bit_addressing());
    assert!(status.supervisor_64_bit_addressing());
    assert!(status.user_64_bit_addressing());
    assert_eq!(status.kernel_user_mode(), Mips4Cp0KernelUserMode::User);
    assert!(!status.error_level());
    assert!(!status.exception_level());
    assert!(status.interrupt_enable());
    assert!(status.interrupts_enabled());
}

#[test]
fn status_masks_reserved_bits() {
    assert_eq!(
        Mips4Cp0Status::from_bits(u32::MAX).bits(),
        STATUS_READABLE_MASK
    );
}

#[test]
fn cause_exposes_exception_fields() {
    let cause = Mips4Cp0Cause::from_bits(
        CAUSE_BD | (2 << CAUSE_CE_SHIFT) | (0xa5 << CAUSE_IP_SHIFT) | (11 << CAUSE_EXC_CODE_SHIFT),
    );

    assert!(cause.branch_delay());
    assert_eq!(cause.coprocessor_error(), Mips4CoprocessorNumber::Cp2);
    assert_eq!(cause.interrupt_pending(), 0xa5);
    assert_eq!(cause.exception_code(), 11);
    assert_eq!(
        Mips4Cp0Cause::from_bits(u32::MAX).bits(),
        CAUSE_READABLE_MASK
    );
}

#[test]
fn raw_wrappers_preserve_or_mask_expected_bits() {
    let bad_vaddr = Mips4Cp0BadVaddr::from_bits(0xffff_ffff_8000_0000);
    assert_eq!(bad_vaddr.address(), 0xffff_ffff_8000_0000);

    let count = Mips4Cp0Count::from_bits(u64::MAX);
    let compare = Mips4Cp0Compare::from_bits(u64::MAX);
    assert_eq!(count.bits(), u32::MAX);
    assert_eq!(compare.bits(), u32::MAX);

    let epc = Mips4Cp0Epc::from_bits(0xffff_ffff_8000_0180);
    let error_epc = Mips4Cp0ErrorEpc::from_bits(0xffff_ffff_bfc0_0000);
    assert_eq!(epc.address(), 0xffff_ffff_8000_0180);
    assert_eq!(error_epc.address(), 0xffff_ffff_bfc0_0000);

    let processor_id = Mips4Cp0ProcessorId::from_bits(0xabcd_2310);
    assert_eq!(processor_id.bits(), 0x0000_2310);
    assert_eq!(processor_id.implementation(), 0x23);
    assert_eq!(processor_id.revision(), 0x10);

    let config = Mips4Cp0Config::from_bits(0xdead_beef);
    let ll_addr = Mips4Cp0LlAddr::from_physical_address(0x123_4567_89ab);
    let ecc = Mips4Cp0Ecc::from_bits(u32::MAX);
    let cache_err = Mips4Cp0CacheErr::from_bits(0x1234_5678);
    let tag_lo = Mips4Cp0TagLo::from_bits(0x8765_4321);
    let tag_hi = Mips4Cp0TagHi::from_bits(0xfedc_ba98);

    assert_eq!(config.bits(), 0xdead_beef);
    assert_eq!(ll_addr.physical_address_bits_35_4(), 0x3456_789a);
    assert_eq!(ecc.bits(), 0xff);
    assert_eq!(cache_err.bits(), 0x1234_5678);
    assert_eq!(tag_lo.bits(), 0x8765_4321);
    assert_eq!(tag_hi.bits(), 0xfedc_ba98);
}

#[test]
fn tlb_interop_round_trips_defined_fields() {
    let tlb_entry_lo = Mips4TlbEntryLo::from_bits(0x0123_4567).unwrap();
    let cp0_entry_lo = Mips4Cp0EntryLo::from_tlb_entry_lo(tlb_entry_lo);
    assert_eq!(cp0_entry_lo.to_tlb_entry_lo(), Some(tlb_entry_lo));

    let tlb_page_mask = Mips4TlbPageMask::from_page_size(Mips4TlbPageSize::Size64KiB);
    let cp0_page_mask = Mips4Cp0PageMask::from_tlb_page_mask(tlb_page_mask);
    assert_eq!(cp0_page_mask.to_tlb_page_mask(), Some(tlb_page_mask));

    let undefined_page_mask = Mips4Cp0PageMask::from_bits(0x0000_2000);
    assert_eq!(undefined_page_mask.bits(), 0x0000_2000);
    assert_eq!(undefined_page_mask.to_tlb_page_mask(), None);

    let tlb_entry_hi =
        Mips4TlbEntryHi::from_parts(0x0123_4567, Mips4TlbAsid::new(0xab), 3).unwrap();
    let cp0_entry_hi = Mips4Cp0EntryHi::from_tlb_entry_hi(tlb_entry_hi);
    assert_eq!(cp0_entry_hi.to_tlb_entry_hi(), Some(tlb_entry_hi));
}

#[test]
fn register_file_initializes_reset_visible_state() {
    let cp0 = Mips4Cp0::new(0xffff_2310, 0xdead_beef, 47);

    assert_eq!(cp0.processor_id().bits(), 0x0000_2310);
    assert_eq!(cp0.config().bits(), 0xdead_beef);
    assert_eq!(cp0.random_upper_bound(), 47);
    assert_eq!(cp0.random().index(), 47);
    assert_eq!(cp0.wired().boundary(), 0);
    assert!(cp0.status().error_level());
    assert!(cp0.status().boot_exception_vectors());
    assert_eq!(
        cp0.read(Mips4Cp0Register::Status),
        (STATUS_ERL | STATUS_BEV) as u64
    );
}

#[test]
fn register_file_reads_and_writes_modeled_registers() {
    let mut cp0 = Mips4Cp0::new(0x0000_2310, 0, 47);

    cp0.write(Mips4Cp0Register::Index, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::EntryLo0, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::EntryLo1, 0x0123_4567).unwrap();
    cp0.write(Mips4Cp0Register::Context, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::PageMask, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::Count, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::EntryHi, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::Compare, 0x1234_5678).unwrap();
    cp0.write(Mips4Cp0Register::Status, u32::MAX as u64)
        .unwrap();
    cp0.write(Mips4Cp0Register::Epc, 0xffff_ffff_8000_0180)
        .unwrap();
    cp0.write(Mips4Cp0Register::Config, 0xfeed_face).unwrap();
    cp0.write(Mips4Cp0Register::LlAddr, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::XContext, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::Ecc, u64::MAX).unwrap();
    cp0.write(Mips4Cp0Register::TagLo, 0x1234_5678).unwrap();
    cp0.write(Mips4Cp0Register::TagHi, 0x8765_4321).unwrap();
    cp0.write(Mips4Cp0Register::ErrorEpc, 0xffff_ffff_bfc0_0000)
        .unwrap();

    assert_eq!(cp0.index().bits(), INDEX_READABLE_MASK as u32);
    assert_eq!(cp0.entry_lo0().bits(), ENTRY_LO_READABLE_MASK as u32);
    assert_eq!(cp0.entry_lo1().bits(), 0x0123_4567);
    assert_eq!(cp0.context().bits(), CONTEXT_READABLE_MASK);
    assert_eq!(cp0.page_mask().bits(), PAGE_MASK_MASK as u32);
    assert_eq!(cp0.count().bits(), u32::MAX);
    assert_eq!(cp0.entry_hi().bits(), ENTRY_HI_READABLE_MASK);
    assert_eq!(cp0.compare().bits(), 0x1234_5678);
    assert_eq!(cp0.status().bits(), STATUS_READABLE_MASK);
    assert_eq!(cp0.epc().bits(), 0xffff_ffff_8000_0180);
    assert_eq!(cp0.config().bits(), 0xfeed_face);
    assert_eq!(cp0.ll_addr().bits(), u32::MAX);
    assert_eq!(cp0.x_context().bits(), XCONTEXT_READABLE_MASK);
    assert_eq!(cp0.ecc().bits(), 0xff);
    assert_eq!(cp0.tag_lo().bits(), 0x1234_5678);
    assert_eq!(cp0.tag_hi().bits(), 0x8765_4321);
    assert_eq!(cp0.error_epc().bits(), 0xffff_ffff_bfc0_0000);
}

#[test]
fn register_file_rejects_public_writes_to_read_only_registers() {
    let mut cp0 = Mips4Cp0::new(0x0000_2310, 0, 47);

    for register in [
        Mips4Cp0Register::Random,
        Mips4Cp0Register::BadVaddr,
        Mips4Cp0Register::ProcessorId,
        Mips4Cp0Register::CacheErr,
    ] {
        assert_eq!(
            cp0.write(register, u64::MAX),
            Err(Mips4Cp0WriteError::ReadOnlyRegister { register })
        );
        assert_eq!(
            Mips4Cp0WriteError::ReadOnlyRegister { register }.register(),
            register
        );
    }

    assert_eq!(cp0.processor_id().bits(), 0x0000_2310);
    assert_eq!(cp0.random().index(), 47);
    assert_eq!(cp0.bad_vaddr().bits(), 0);
    assert_eq!(cp0.cache_err().bits(), 0);
}

#[test]
fn cause_write_only_updates_software_interrupt_bits() {
    let mut cp0 = Mips4Cp0::new(0, 0, 47);
    cp0.cause = Mips4Cp0Cause::from_bits(CAUSE_BD | CAUSE_TIMER_IP | (5 << CAUSE_EXC_CODE_SHIFT));

    cp0.write(Mips4Cp0Register::Cause, u64::MAX).unwrap();
    assert_eq!(
        cp0.cause().bits(),
        CAUSE_BD | CAUSE_TIMER_IP | (5 << CAUSE_EXC_CODE_SHIFT) | CAUSE_SOFTWARE_IP_MASK
    );

    cp0.write(Mips4Cp0Register::Cause, 0).unwrap();
    assert_eq!(
        cp0.cause().bits(),
        CAUSE_BD | CAUSE_TIMER_IP | (5 << CAUSE_EXC_CODE_SHIFT)
    );
}

#[test]
fn compare_write_clears_timer_interrupt_pending() {
    let mut cp0 = Mips4Cp0::new(0, 0, 47);
    cp0.cause = Mips4Cp0Cause::from_bits(CAUSE_TIMER_IP | CAUSE_SOFTWARE_IP_MASK);

    cp0.write(Mips4Cp0Register::Compare, 0x1234_5678).unwrap();

    assert_eq!(cp0.compare().bits(), 0x1234_5678);
    assert_eq!(cp0.cause().bits(), CAUSE_SOFTWARE_IP_MASK);
}

#[test]
fn wired_write_resets_random_to_upper_bound() {
    let mut cp0 = Mips4Cp0::new(0, 0, 47);
    cp0.random = Mips4Cp0Random::from_bits(12);

    cp0.write(Mips4Cp0Register::Wired, 4).unwrap();

    assert_eq!(cp0.wired().boundary(), 4);
    assert_eq!(cp0.random().index(), 47);
}
