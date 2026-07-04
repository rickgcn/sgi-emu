use super::*;

#[test]
fn register_numbers_accept_modeled_r3000a_registers() {
    let registers = [
        (0, Mips1Cp0Register::Index),
        (1, Mips1Cp0Register::Random),
        (2, Mips1Cp0Register::EntryLo),
        (4, Mips1Cp0Register::Context),
        (8, Mips1Cp0Register::BadVaddr),
        (10, Mips1Cp0Register::EntryHi),
        (12, Mips1Cp0Register::Status),
        (13, Mips1Cp0Register::Cause),
        (14, Mips1Cp0Register::Epc),
        (15, Mips1Cp0Register::ProcessorId),
    ];

    for (number, register) in registers {
        assert_eq!(Mips1Cp0Register::from_u8(number), Some(register));
        assert_eq!(register.number(), number);
    }
}

#[test]
fn register_numbers_reject_unmodeled_registers() {
    assert_eq!(Mips1Cp0Register::from_u8(3), None);
    assert_eq!(Mips1Cp0Register::from_u8(5), None);
    assert_eq!(Mips1Cp0Register::from_u8(9), None);
    assert_eq!(Mips1Cp0Register::from_u8(11), None);
}

#[test]
fn status_exposes_control_fields() {
    let status = Mips1Cp0Status::from_bits(
        STATUS_CU0
            | (STATUS_CU0 << 2)
            | STATUS_RE
            | STATUS_BEV
            | STATUS_TS
            | STATUS_PE
            | STATUS_CM
            | STATUS_PZ
            | STATUS_SWC
            | STATUS_ISC
            | (0xa5 << STATUS_IM_SHIFT)
            | 0x2d,
    );

    assert_eq!(status.cu(), 0b0101);
    assert!(status.coprocessor_usable(Mips1CoprocessorNumber::Cp0));
    assert!(!status.coprocessor_usable(Mips1CoprocessorNumber::Cp1));
    assert!(status.coprocessor_usable(Mips1CoprocessorNumber::Cp2));
    assert!(!status.coprocessor_usable(Mips1CoprocessorNumber::Cp3));
    assert!(status.reverse_endianness());
    assert!(status.boot_exception_vectors());
    assert!(status.tlb_shutdown());
    assert!(status.parity_error());
    assert!(status.cache_miss());
    assert!(status.parity_zero());
    assert!(status.swap_caches());
    assert!(status.isolate_cache());
    assert_eq!(status.interrupt_mask(), 0xa5);
    assert!(!status.kernel_user_current());
    assert!(status.interrupt_enable_current());
    assert!(status.kernel_user_previous());
    assert!(status.interrupt_enable_previous());
    assert!(status.kernel_user_old());
    assert!(!status.interrupt_enable_old());
}

#[test]
fn status_masks_reserved_bits() {
    assert_eq!(
        Mips1Cp0Status::from_bits(u32::MAX).bits(),
        STATUS_READABLE_MASK
    );
}

#[test]
fn status_restore_from_exception_pops_status_stack() {
    let status = Mips1Cp0Status::from_bits(STATUS_CU0 | STATUS_BEV | 0x27);
    let restored = status.restore_from_exception();

    assert_eq!(restored.bits() & STATUS_KU_IE_MASK, 0x29);
    assert_eq!(
        restored.bits() & !STATUS_KU_IE_MASK,
        status.bits() & !STATUS_KU_IE_MASK
    );
}

#[test]
fn cause_exposes_exception_fields() {
    let cause = Mips1Cp0Cause::from_bits(
        CAUSE_BD | (2 << CAUSE_CE_SHIFT) | (0xa5 << CAUSE_IP_SHIFT) | (11 << CAUSE_EXC_CODE_SHIFT),
    );

    assert!(cause.branch_delay());
    assert_eq!(cause.coprocessor_error(), Mips1CoprocessorNumber::Cp2);
    assert_eq!(cause.interrupt_pending(), 0xa5);
    assert_eq!(cause.exception_code(), 11);
}

#[test]
fn cause_and_processor_id_preserve_expected_raw_bits() {
    assert_eq!(
        Mips1Cp0Cause::from_bits(u32::MAX).bits(),
        CAUSE_READABLE_MASK
    );

    let processor_id = Mips1Cp0ProcessorId::from_bits(0xabcd_1234);
    assert_eq!(processor_id.bits(), 0xabcd_1234);
    assert_eq!(processor_id.implementation(), 0x12);
    assert_eq!(processor_id.revision(), 0x34);
}

#[test]
fn mmu_related_wrappers_extract_fields_and_mask_zero_bits() {
    let index = Mips1Cp0Index::from_bits(u32::MAX);
    assert_eq!(index.bits(), INDEX_READABLE_MASK);
    assert!(index.probe_failure());
    assert_eq!(index.index(), 63);

    let random = Mips1Cp0Random::from_bits(u32::MAX);
    assert_eq!(random.bits(), RANDOM_READABLE_MASK);
    assert_eq!(random.random(), 63);

    let entry_hi = Mips1Cp0EntryHi::from_bits(u32::MAX);
    assert_eq!(entry_hi.bits(), ENTRY_HI_READABLE_MASK);
    assert_eq!(entry_hi.virtual_page_number(), 0x000f_ffff);
    assert_eq!(entry_hi.address_space_identifier(), 63);

    let entry_lo = Mips1Cp0EntryLo::from_bits(u32::MAX);
    assert_eq!(entry_lo.bits(), ENTRY_LO_READABLE_MASK);
    assert_eq!(entry_lo.physical_frame_number(), 0x000f_ffff);
    assert!(entry_lo.noncacheable());
    assert!(entry_lo.dirty());
    assert!(entry_lo.valid());
    assert!(entry_lo.global());

    let context = Mips1Cp0Context::from_bits(u32::MAX);
    assert_eq!(context.bits(), CONTEXT_READABLE_MASK);
    assert_eq!(context.page_table_entry_base(), 0x07ff);
    assert_eq!(context.bad_virtual_page_number(), 0x0007_ffff);
}

#[test]
fn address_wrappers_preserve_raw_addresses() {
    let bad_vaddr = Mips1Cp0BadVaddr::from_bits(0xdead_beef);
    assert_eq!(bad_vaddr.bits(), 0xdead_beef);
    assert_eq!(bad_vaddr.address(), 0xdead_beef);

    let epc = Mips1Cp0Epc::from_bits(0x8000_0080);
    assert_eq!(epc.bits(), 0x8000_0080);
    assert_eq!(epc.address(), 0x8000_0080);
}

#[test]
fn cp0_register_file_reads_and_writes_modeled_registers() {
    let mut cp0 = Mips1Cp0::new(0xabcd_1234);

    assert_eq!(cp0.processor_id().bits(), 0xabcd_1234);
    assert_eq!(
        cp0.read(Mips1Cp0Register::ProcessorId),
        cp0.processor_id().bits()
    );

    cp0.write(Mips1Cp0Register::Status, u32::MAX).unwrap();
    cp0.write(Mips1Cp0Register::EntryHi, u32::MAX).unwrap();
    cp0.write(Mips1Cp0Register::EntryLo, u32::MAX).unwrap();
    cp0.write(Mips1Cp0Register::Context, u32::MAX).unwrap();
    cp0.write(Mips1Cp0Register::Index, u32::MAX).unwrap();
    cp0.write(Mips1Cp0Register::Random, u32::MAX).unwrap();
    cp0.write(Mips1Cp0Register::Epc, 0x8000_0080).unwrap();

    assert_eq!(cp0.status().bits(), STATUS_SOFTWARE_WRITABLE_MASK);
    assert!(!cp0.status().tlb_shutdown());
    assert!(!cp0.status().parity_error());
    assert!(!cp0.status().cache_miss());
    assert_eq!(cp0.entry_hi().bits(), ENTRY_HI_READABLE_MASK);
    assert_eq!(cp0.entry_lo().bits(), ENTRY_LO_READABLE_MASK);
    assert_eq!(cp0.context().bits(), CONTEXT_READABLE_MASK);
    assert_eq!(cp0.index().bits(), INDEX_READABLE_MASK);
    assert_eq!(cp0.random().bits(), RANDOM_READABLE_MASK);
    assert_eq!(cp0.epc().bits(), 0x8000_0080);
}

#[test]
fn cp0_register_file_rejects_public_writes_to_read_only_registers() {
    let mut cp0 = Mips1Cp0::new(0x0000_0300);

    assert_eq!(
        cp0.write(Mips1Cp0Register::ProcessorId, 0),
        Err(Mips1Cp0WriteError::ReadOnlyRegister {
            register: Mips1Cp0Register::ProcessorId
        })
    );
    assert_eq!(
        cp0.write(Mips1Cp0Register::BadVaddr, 0x1234_5678),
        Err(Mips1Cp0WriteError::ReadOnlyRegister {
            register: Mips1Cp0Register::BadVaddr
        })
    );
    assert_eq!(
        Mips1Cp0WriteError::ReadOnlyRegister {
            register: Mips1Cp0Register::BadVaddr
        }
        .register(),
        Mips1Cp0Register::BadVaddr
    );
    assert_eq!(cp0.processor_id().bits(), 0x0000_0300);
    assert_eq!(cp0.bad_vaddr().bits(), 0);
}

#[test]
fn cp0_status_write_preserves_hardware_bits_and_clears_parity_error() {
    let mut cp0 = Mips1Cp0 {
        status: Mips1Cp0Status::from_bits(STATUS_TS | STATUS_PE | STATUS_CM),
        ..Mips1Cp0::new(0)
    };

    cp0.write(Mips1Cp0Register::Status, 0).unwrap();

    assert!(cp0.status().tlb_shutdown());
    assert!(cp0.status().parity_error());
    assert!(cp0.status().cache_miss());

    cp0.write(Mips1Cp0Register::Status, STATUS_PE).unwrap();

    assert!(cp0.status().tlb_shutdown());
    assert!(!cp0.status().parity_error());
    assert!(cp0.status().cache_miss());
}

#[test]
fn cp0_register_file_only_writes_cause_software_interrupt_bits() {
    let mut cp0 = Mips1Cp0::new(0);

    cp0.write(Mips1Cp0Register::Cause, u32::MAX).unwrap();
    assert_eq!(cp0.cause().bits(), CAUSE_SOFTWARE_IP_MASK);
    assert_eq!(cp0.read(Mips1Cp0Register::Cause), CAUSE_SOFTWARE_IP_MASK);

    cp0.write(Mips1Cp0Register::Cause, 0).unwrap();
    assert_eq!(cp0.cause().bits(), 0);
}
