use super::*;

#[test]
fn effective_address_uses_wrapping_signed_offset() {
    assert_eq!(Mips1Memory::effective_address(0x1000, 4), 0x1004);
    assert_eq!(Mips1Memory::effective_address(0x1000, -4), 0x0ffc);
    assert_eq!(Mips1Memory::effective_address(0, -1), 0xffff_ffff);
}

#[test]
fn load_extension_helpers_extend_to_word() {
    assert_eq!(Mips1Memory::sign_extend_byte(0x80), 0xffff_ff80);
    assert_eq!(Mips1Memory::zero_extend_byte(0x80), 0x0000_0080);
    assert_eq!(Mips1Memory::sign_extend_halfword(0x8001), 0xffff_8001);
    assert_eq!(Mips1Memory::zero_extend_halfword(0x8001), 0x0000_8001);
}

#[test]
fn load_alignment_errors_use_load_address_exception() {
    assert_eq!(
        Mips1Memory::check_alignment(
            0x1001,
            Mips1MemoryAccessKind::Load {
                size: Mips1MemoryAccessSize::Halfword,
                signed: true,
            },
        ),
        Err(Mips1Exception::AddressErrorLoad)
    );
    assert_eq!(
        Mips1Memory::check_alignment(
            0x1002,
            Mips1MemoryAccessKind::Load {
                size: Mips1MemoryAccessSize::Word,
                signed: true,
            },
        ),
        Err(Mips1Exception::AddressErrorLoad)
    );
}

#[test]
fn store_alignment_errors_use_store_address_exception() {
    assert_eq!(
        Mips1Memory::check_alignment(
            0x1001,
            Mips1MemoryAccessKind::Store {
                size: Mips1MemoryAccessSize::Halfword,
            },
        ),
        Err(Mips1Exception::AddressErrorStore)
    );
    assert_eq!(
        Mips1Memory::check_alignment(
            0x1002,
            Mips1MemoryAccessKind::Store {
                size: Mips1MemoryAccessSize::Word,
            },
        ),
        Err(Mips1Exception::AddressErrorStore)
    );
}

#[test]
fn byte_and_partial_word_accesses_do_not_require_alignment() {
    assert_eq!(
        Mips1Memory::check_alignment(
            0x1001,
            Mips1MemoryAccessKind::Load {
                size: Mips1MemoryAccessSize::Byte,
                signed: false,
            },
        ),
        Ok(())
    );
    assert_eq!(
        Mips1Memory::check_alignment(0x1001, Mips1MemoryAccessKind::LoadWordLeft),
        Ok(())
    );
    assert_eq!(
        Mips1Memory::check_alignment(0x1001, Mips1MemoryAccessKind::StoreWordRight),
        Ok(())
    );
}

#[test]
fn lwl_and_lwr_merge_big_endian_words() {
    assert_eq!(
        Mips1Memory::lwl_merge(Mips1Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0x2233_44dd
    );
    assert_eq!(
        Mips1Memory::lwr_merge(Mips1Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0xaabb_1122
    );
}

#[test]
fn lwl_and_lwr_merge_little_endian_words() {
    assert_eq!(
        Mips1Memory::lwl_merge(Mips1Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0x3344_ccdd
    );
    assert_eq!(
        Mips1Memory::lwr_merge(Mips1Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0xaa11_2233
    );
}

#[test]
fn swl_and_swr_return_big_endian_masks_and_merged_words() {
    assert_eq!(
        Mips1Memory::swl_masked_word(Mips1Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips1MaskedMemoryWord {
            value: 0x11aa_bbcc,
            write_mask: 0x00ff_ffff,
        }
    );
    assert_eq!(
        Mips1Memory::swr_masked_word(Mips1Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips1MaskedMemoryWord {
            value: 0xccdd_3344,
            write_mask: 0xffff_0000,
        }
    );
}

#[test]
fn swl_and_swr_return_little_endian_masks_and_merged_words() {
    assert_eq!(
        Mips1Memory::swl_masked_word(Mips1Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips1MaskedMemoryWord {
            value: 0x1122_aabb,
            write_mask: 0x0000_ffff,
        }
    );
    assert_eq!(
        Mips1Memory::swr_masked_word(Mips1Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips1MaskedMemoryWord {
            value: 0xbbcc_dd44,
            write_mask: 0xffff_ff00,
        }
    );
}
