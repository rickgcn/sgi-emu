use super::*;

#[test]
fn effective_address_uses_wrapping_signed_offset() {
    assert_eq!(Mips4Memory::effective_address(0x1000, 4), 0x1004);
    assert_eq!(Mips4Memory::effective_address(0x1000, -4), 0x0ffc);
    assert_eq!(Mips4Memory::effective_address(0, -1), 0xffff_ffff_ffff_ffff);
}

#[test]
fn indexed_effective_address_requires_base_region_bits() {
    assert_eq!(
        Mips4Memory::indexed_effective_address(0, 0x3fff_ffff_ffff_ffff, false),
        Ok(0x3fff_ffff_ffff_ffff)
    );
    assert_eq!(
        Mips4Memory::indexed_effective_address(0x3fff_ffff_ffff_ffff, 1, false),
        Err(Mips4Exception::AddressErrorLoad)
    );
    assert_eq!(
        Mips4Memory::indexed_effective_address(0xbfff_ffff_ffff_ffff, 1, true),
        Err(Mips4Exception::AddressErrorStore)
    );
}

#[test]
fn load_extension_helpers_extend_to_doubleword() {
    assert_eq!(Mips4Memory::sign_extend_byte(0x80), 0xffff_ffff_ffff_ff80);
    assert_eq!(Mips4Memory::zero_extend_byte(0x80), 0x0000_0000_0000_0080);
    assert_eq!(
        Mips4Memory::sign_extend_halfword(0x8001),
        0xffff_ffff_ffff_8001
    );
    assert_eq!(
        Mips4Memory::zero_extend_halfword(0x8001),
        0x0000_0000_0000_8001
    );
    assert_eq!(
        Mips4Memory::sign_extend_loaded_word(0x8000_0001),
        0xffff_ffff_8000_0001
    );
    assert_eq!(
        Mips4Memory::zero_extend_word(0x8000_0001),
        0x0000_0000_8000_0001
    );
}

#[test]
fn load_alignment_errors_use_load_address_exception() {
    assert_eq!(
        Mips4Memory::check_alignment(
            0x1001,
            Mips4MemoryAccessKind::Load {
                size: Mips4MemoryAccessSize::Halfword,
                signed: true,
            },
        ),
        Err(Mips4Exception::AddressErrorLoad)
    );
    assert_eq!(
        Mips4Memory::check_alignment(
            0x1002,
            Mips4MemoryAccessKind::Load {
                size: Mips4MemoryAccessSize::Word,
                signed: true,
            },
        ),
        Err(Mips4Exception::AddressErrorLoad)
    );
    assert_eq!(
        Mips4Memory::check_alignment(
            0x1004,
            Mips4MemoryAccessKind::Load {
                size: Mips4MemoryAccessSize::Doubleword,
                signed: true,
            },
        ),
        Err(Mips4Exception::AddressErrorLoad)
    );
}

#[test]
fn store_alignment_errors_use_store_address_exception() {
    assert_eq!(
        Mips4Memory::check_alignment(
            0x1001,
            Mips4MemoryAccessKind::Store {
                size: Mips4MemoryAccessSize::Halfword,
            },
        ),
        Err(Mips4Exception::AddressErrorStore)
    );
    assert_eq!(
        Mips4Memory::check_alignment(
            0x1002,
            Mips4MemoryAccessKind::Store {
                size: Mips4MemoryAccessSize::Word,
            },
        ),
        Err(Mips4Exception::AddressErrorStore)
    );
    assert_eq!(
        Mips4Memory::check_alignment(
            0x1004,
            Mips4MemoryAccessKind::Store {
                size: Mips4MemoryAccessSize::Doubleword,
            },
        ),
        Err(Mips4Exception::AddressErrorStore)
    );
}

#[test]
fn byte_and_partial_accesses_do_not_require_alignment() {
    assert_eq!(
        Mips4Memory::check_alignment(
            0x1001,
            Mips4MemoryAccessKind::Load {
                size: Mips4MemoryAccessSize::Byte,
                signed: false,
            },
        ),
        Ok(())
    );
    assert_eq!(
        Mips4Memory::check_alignment(0x1001, Mips4MemoryAccessKind::LoadWordLeft),
        Ok(())
    );
    assert_eq!(
        Mips4Memory::check_alignment(0x1001, Mips4MemoryAccessKind::StoreWordRight),
        Ok(())
    );
    assert_eq!(
        Mips4Memory::check_alignment(0x1001, Mips4MemoryAccessKind::LoadDoublewordLeft),
        Ok(())
    );
    assert_eq!(
        Mips4Memory::check_alignment(0x1001, Mips4MemoryAccessKind::StoreDoublewordRight),
        Ok(())
    );
}

#[test]
fn lwl_and_lwr_merge_big_endian_words_and_sign_extend() {
    assert_eq!(
        Mips4Memory::lwl_merge(Mips4Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0x0000_0000_2233_44dd
    );
    assert_eq!(
        Mips4Memory::lwr_merge(Mips4Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0xffff_ffff_aabb_1122
    );
    assert_eq!(
        Mips4Memory::lwl_merge(Mips4Endianness::Big, 0x1000, 0, 0x8122_3344),
        0xffff_ffff_8122_3344
    );
}

#[test]
fn lwl_and_lwr_merge_little_endian_words_and_sign_extend() {
    assert_eq!(
        Mips4Memory::lwl_merge(Mips4Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0x0000_0000_3344_ccdd
    );
    assert_eq!(
        Mips4Memory::lwr_merge(Mips4Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        0xffff_ffff_aa11_2233
    );
}

#[test]
fn swl_and_swr_return_big_endian_masks_and_merged_words() {
    assert_eq!(
        Mips4Memory::swl_masked_word(Mips4Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips4MaskedMemoryWord {
            value: 0x11aa_bbcc,
            write_mask: 0x00ff_ffff,
        }
    );
    assert_eq!(
        Mips4Memory::swr_masked_word(Mips4Endianness::Big, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips4MaskedMemoryWord {
            value: 0xccdd_3344,
            write_mask: 0xffff_0000,
        }
    );
}

#[test]
fn swl_and_swr_return_little_endian_masks_and_merged_words() {
    assert_eq!(
        Mips4Memory::swl_masked_word(Mips4Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips4MaskedMemoryWord {
            value: 0x1122_aabb,
            write_mask: 0x0000_ffff,
        }
    );
    assert_eq!(
        Mips4Memory::swr_masked_word(Mips4Endianness::Little, 0x1001, 0xaabb_ccdd, 0x1122_3344),
        Mips4MaskedMemoryWord {
            value: 0xbbcc_dd44,
            write_mask: 0xffff_ff00,
        }
    );
}

#[test]
fn ldl_and_ldr_merge_big_endian_doublewords() {
    assert_eq!(
        Mips4Memory::ldl_merge(
            Mips4Endianness::Big,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        0x2233_4455_6677_8811
    );
    assert_eq!(
        Mips4Memory::ldr_merge(
            Mips4Endianness::Big,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        0xaabb_ccdd_eeff_1122
    );
}

#[test]
fn ldl_and_ldr_merge_little_endian_doublewords() {
    assert_eq!(
        Mips4Memory::ldl_merge(
            Mips4Endianness::Little,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        0x7788_ccdd_eeff_0011
    );
    assert_eq!(
        Mips4Memory::ldr_merge(
            Mips4Endianness::Little,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        0xaa11_2233_4455_6677
    );
}

#[test]
fn sdl_and_sdr_return_big_endian_masks_and_merged_doublewords() {
    assert_eq!(
        Mips4Memory::sdl_masked_doubleword(
            Mips4Endianness::Big,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        Mips4MaskedMemoryDoubleword {
            value: 0x11aa_bbcc_ddee_ff00,
            write_mask: 0x00ff_ffff_ffff_ffff,
        }
    );
    assert_eq!(
        Mips4Memory::sdr_masked_doubleword(
            Mips4Endianness::Big,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        Mips4MaskedMemoryDoubleword {
            value: 0x0011_3344_5566_7788,
            write_mask: 0xffff_0000_0000_0000,
        }
    );
}

#[test]
fn sdl_and_sdr_return_little_endian_masks_and_merged_doublewords() {
    assert_eq!(
        Mips4Memory::sdl_masked_doubleword(
            Mips4Endianness::Little,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        Mips4MaskedMemoryDoubleword {
            value: 0x1122_3344_5566_aabb,
            write_mask: 0x0000_0000_0000_ffff,
        }
    );
    assert_eq!(
        Mips4Memory::sdr_masked_doubleword(
            Mips4Endianness::Little,
            0x1001,
            0xaabb_ccdd_eeff_0011,
            0x1122_3344_5566_7788,
        ),
        Mips4MaskedMemoryDoubleword {
            value: 0xbbcc_ddee_ff00_1188,
            write_mask: 0xffff_ffff_ffff_ff00,
        }
    );
}
