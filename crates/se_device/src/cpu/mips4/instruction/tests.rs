use super::*;

#[test]
fn extracts_r_format_fields() {
    let instruction =
        Mips4Instruction::from_bits((1 << 21) | (2 << 16) | (3 << 11) | (4 << 6) | 0x20);

    assert_eq!(instruction.bits(), 0x0022_1920);
    assert_eq!(instruction.opcode(), 0);
    assert_eq!(instruction.rs(), 1);
    assert_eq!(instruction.rt(), 2);
    assert_eq!(instruction.rd(), 3);
    assert_eq!(instruction.shamt(), 4);
    assert_eq!(instruction.funct(), 0x20);
}

#[test]
fn extracts_i_format_fields_and_sign_extends_immediate() {
    let instruction = Mips4Instruction::from_bits((0x08 << 26) | (2 << 21) | (3 << 16) | 0xfffc);

    assert_eq!(instruction.opcode(), 0x08);
    assert_eq!(instruction.rs(), 2);
    assert_eq!(instruction.rt(), 3);
    assert_eq!(instruction.immediate(), 0xfffc);
    assert_eq!(instruction.signed_immediate(), -4);
}

#[test]
fn extracts_j_format_target_and_shifted_index() {
    let instruction = Mips4Instruction::from_bits((0x02 << 26) | 0x0012_3456);

    assert_eq!(instruction.opcode(), 0x02);
    assert_eq!(instruction.target(), 0x0012_3456);
    assert_eq!(instruction.jump_index(), 0x0048_d158);
}

#[test]
fn branch_offset_uses_signed_immediate_shifted_by_two() {
    let instruction = Mips4Instruction::from_bits((0x04 << 26) | 0xfffe);

    assert_eq!(instruction.signed_immediate(), -2);
    assert_eq!(instruction.branch_offset(), -8);
}

#[test]
fn extracts_floating_point_alias_fields() {
    let instruction =
        Mips4Instruction::from_bits((0x11 << 26) | (0x10 << 21) | (2 << 16) | (3 << 11) | (4 << 6));

    assert_eq!(instruction.opcode(), 0x11);
    assert_eq!(instruction.fmt(), 0x10);
    assert_eq!(instruction.ft(), 2);
    assert_eq!(instruction.fs(), 3);
    assert_eq!(instruction.fd(), 4);
}
