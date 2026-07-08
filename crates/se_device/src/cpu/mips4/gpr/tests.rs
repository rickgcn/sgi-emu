use super::*;

#[test]
fn new_register_file_reads_as_zero() {
    let registers = Mips4GprFile::new();

    for raw in 0..MIPS4_GPR_COUNT as u8 {
        let index = Mips4GprIndex::from_u8(raw).unwrap();
        assert_eq!(registers.read(index), 0);
    }
}

#[test]
fn ordinary_register_write_can_be_read_back() {
    let mut registers = Mips4GprFile::new();
    let index = Mips4GprIndex::from_u8(31).unwrap();

    registers.write(index, 0x1234_5678_9abc_def0);

    assert_eq!(registers.read(index), 0x1234_5678_9abc_def0);
}

#[test]
fn zero_register_ignores_writes() {
    let mut registers = Mips4GprFile::new();

    registers.write(Mips4GprIndex::ZERO, 0xffff_ffff_ffff_ffff);

    assert_eq!(registers.read(Mips4GprIndex::ZERO), 0);
}

#[test]
fn reset_clears_ordinary_registers() {
    let mut registers = Mips4GprFile::new();
    let index = Mips4GprIndex::from_u8(5).unwrap();

    registers.write(index, 0xa5a5_5a5a_c3c3_3c3c);
    registers.reset();

    assert_eq!(registers.read(index), 0);
}

#[test]
fn register_index_accepts_only_architectural_range() {
    assert_eq!(Mips4GprIndex::from_u8(31).unwrap().number(), 31);
    assert_eq!(Mips4GprIndex::from_u8(32), None);
}

#[test]
fn sign_extend_word_extends_positive_and_negative_values() {
    assert_eq!(sign_extend_word(0x0000_0000), 0x0000_0000_0000_0000);
    assert_eq!(sign_extend_word(0x7fff_ffff), 0x0000_0000_7fff_ffff);
    assert_eq!(sign_extend_word(0x8000_0000), 0xffff_ffff_8000_0000);
    assert_eq!(sign_extend_word(0xffff_ffff), 0xffff_ffff_ffff_ffff);
}

#[test]
fn detects_sign_extended_word_values() {
    assert!(is_sign_extended_word(0x0000_0000_0000_0000));
    assert!(is_sign_extended_word(0x0000_0000_7fff_ffff));
    assert!(is_sign_extended_word(0xffff_ffff_8000_0000));
    assert!(is_sign_extended_word(0xffff_ffff_ffff_ffff));
    assert!(!is_sign_extended_word(0x0000_0000_8000_0000));
    assert!(!is_sign_extended_word(0xffff_ffff_7fff_ffff));
}

#[test]
fn write_sign_extended_word_stores_extended_value() {
    let mut registers = Mips4GprFile::new();
    let index = Mips4GprIndex::from_u8(2).unwrap();

    registers.write_sign_extended_word(index, 0x8000_0000);

    assert_eq!(registers.read(index), 0xffff_ffff_8000_0000);
}
