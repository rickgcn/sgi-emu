use super::*;

#[test]
fn trapping_add_reports_signed_overflow() {
    assert_eq!(Mips1Alu::add(1, 2), Ok(3));
    assert_eq!(
        Mips1Alu::add(0x7fff_ffff, 1),
        Err(Mips1Exception::ArithmeticOverflow)
    );
}

#[test]
fn trapping_add_immediate_sign_extends_and_reports_overflow() {
    assert_eq!(Mips1Alu::add_immediate(1, -2), Ok(0xffff_ffff));
    assert_eq!(
        Mips1Alu::add_immediate(0x7fff_ffff, 1),
        Err(Mips1Exception::ArithmeticOverflow)
    );
}

#[test]
fn trapping_sub_reports_signed_overflow() {
    assert_eq!(Mips1Alu::sub(5, 3), Ok(2));
    assert_eq!(
        Mips1Alu::sub(0x8000_0000, 1),
        Err(Mips1Exception::ArithmeticOverflow)
    );
}

#[test]
fn unsigned_arithmetic_wraps_without_errors() {
    assert_eq!(Mips1Alu::addu(0xffff_ffff, 1), 0);
    assert_eq!(Mips1Alu::addiu(1, -2), 0xffff_ffff);
    assert_eq!(Mips1Alu::subu(0, 1), 0xffff_ffff);
}

#[test]
fn logical_operations_return_expected_bits() {
    assert_eq!(Mips1Alu::and(0xf0f0_ffff, 0x0ff0_00ff), 0x00f0_00ff);
    assert_eq!(Mips1Alu::or(0xf000_000f, 0x000f_f000), 0xf00f_f00f);
    assert_eq!(Mips1Alu::xor(0xffff_0000, 0x00ff_00ff), 0xff00_00ff);
    assert_eq!(Mips1Alu::nor(0xffff_0000, 0x00ff_00ff), 0x0000_ff00);
}

#[test]
fn logical_immediates_are_zero_extended() {
    assert_eq!(Mips1Alu::andi(0xffff_ffff, 0x8001), 0x0000_8001);
    assert_eq!(Mips1Alu::ori(0x1234_0000, 0x8001), 0x1234_8001);
    assert_eq!(Mips1Alu::xori(0xffff_0000, 0x8001), 0xffff_8001);
}

#[test]
fn lui_places_immediate_in_high_halfword() {
    assert_eq!(Mips1Alu::lui(0x8001), 0x8001_0000);
}

#[test]
fn fixed_shift_operations_use_low_five_bits() {
    assert_eq!(Mips1Alu::sll(1, 33), 2);
    assert_eq!(Mips1Alu::srl(0x8000_0000, 33), 0x4000_0000);
    assert_eq!(Mips1Alu::sra(0x8000_0000, 33), 0xc000_0000);
}

#[test]
fn variable_shift_operations_use_low_five_bits() {
    assert_eq!(Mips1Alu::sllv(1, 33), 2);
    assert_eq!(Mips1Alu::srlv(0x8000_0000, 33), 0x4000_0000);
    assert_eq!(Mips1Alu::srav(0x8000_0000, 33), 0xc000_0000);
}

#[test]
fn comparisons_return_zero_or_one() {
    assert_eq!(Mips1Alu::slt(0xffff_ffff, 0), 1);
    assert_eq!(Mips1Alu::slti(0, -1), 0);
    assert_eq!(Mips1Alu::sltu(0xffff_ffff, 0), 0);
    assert_eq!(Mips1Alu::sltiu(0, -1), 1);
}

#[test]
fn signed_multiply_splits_hi_and_lo() {
    assert_eq!(
        Mips1Alu::mult(0xffff_ffff, 2),
        Mips1HiLoResult {
            hi: 0xffff_ffff,
            lo: 0xffff_fffe,
        }
    );
}

#[test]
fn unsigned_multiply_splits_hi_and_lo() {
    assert_eq!(
        Mips1Alu::multu(0xffff_ffff, 2),
        Mips1HiLoResult {
            hi: 1,
            lo: 0xffff_fffe,
        }
    );
}

#[test]
fn signed_divide_returns_quotient_in_lo_and_remainder_in_hi() {
    assert_eq!(
        Mips1Alu::div((-7_i32) as u32, 3),
        Some(Mips1HiLoResult {
            hi: (-1_i32) as u32,
            lo: (-2_i32) as u32,
        })
    );
}

#[test]
fn unsigned_divide_returns_quotient_in_lo_and_remainder_in_hi() {
    assert_eq!(Mips1Alu::divu(7, 3), Some(Mips1HiLoResult { hi: 1, lo: 2 }));
}

#[test]
fn undefined_divide_results_return_none() {
    assert_eq!(Mips1Alu::div(1, 0), None);
    assert_eq!(Mips1Alu::divu(1, 0), None);
    assert_eq!(Mips1Alu::div(i32::MIN as u32, (-1_i32) as u32), None);
}
