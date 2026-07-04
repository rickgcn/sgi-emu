use super::*;

#[test]
fn new_register_file_reads_as_zero() {
    let registers = Mips1GprFile::new();

    for raw in 0..MIPS1_GPR_COUNT as u8 {
        let index = Mips1GprIndex::from_u8(raw).unwrap();
        assert_eq!(registers.read(index), 0);
    }
}

#[test]
fn ordinary_register_write_can_be_read_back() {
    let mut registers = Mips1GprFile::new();
    let index = Mips1GprIndex::from_u8(31).unwrap();

    registers.write(index, 0x1234_5678);

    assert_eq!(registers.read(index), 0x1234_5678);
}

#[test]
fn zero_register_ignores_writes() {
    let mut registers = Mips1GprFile::new();

    registers.write(Mips1GprIndex::ZERO, 0xffff_ffff);

    assert_eq!(registers.read(Mips1GprIndex::ZERO), 0);
}

#[test]
fn reset_clears_ordinary_registers() {
    let mut registers = Mips1GprFile::new();
    let index = Mips1GprIndex::from_u8(5).unwrap();

    registers.write(index, 0xa5a5_5a5a);
    registers.reset();

    assert_eq!(registers.read(index), 0);
}

#[test]
fn register_index_accepts_only_architectural_range() {
    assert_eq!(Mips1GprIndex::from_u8(31).unwrap().number(), 31);
    assert_eq!(Mips1GprIndex::from_u8(32), None);
}
