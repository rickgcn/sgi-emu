use super::*;

#[test]
fn word_arithmetic_sign_extends_results_and_reports_overflow() {
    assert_eq!(Mips4Alu::add(1, 2), Ok(3));
    assert_eq!(Mips4Alu::add(1, 0xffff_ffff), Ok(0));
    assert_eq!(
        Mips4Alu::add(0x7fff_ffff, 1),
        Err(Mips4Exception::ArithmeticOverflow)
    );

    assert_eq!(Mips4Alu::add_immediate(1, -2), Ok(0xffff_ffff_ffff_ffff));
    assert_eq!(
        Mips4Alu::add_immediate(0x7fff_ffff, 1),
        Err(Mips4Exception::ArithmeticOverflow)
    );

    assert_eq!(Mips4Alu::sub(5, 3), Ok(2));
    assert_eq!(
        Mips4Alu::sub(0x8000_0000, 1),
        Err(Mips4Exception::ArithmeticOverflow)
    );
}

#[test]
fn word_unsigned_arithmetic_wraps_without_errors() {
    assert_eq!(Mips4Alu::addu(0xffff_ffff, 1), 0);
    assert_eq!(Mips4Alu::addiu(1, -2), 0xffff_ffff_ffff_ffff);
    assert_eq!(Mips4Alu::subu(0, 1), 0xffff_ffff_ffff_ffff);
}

#[test]
fn doubleword_arithmetic_traps_or_wraps_by_instruction_variant() {
    assert_eq!(Mips4Alu::dadd(1, 2), Ok(3));
    assert_eq!(
        Mips4Alu::dadd(i64::MAX as u64, 1),
        Err(Mips4Exception::ArithmeticOverflow)
    );
    assert_eq!(Mips4Alu::daddi(1, -2), Ok(0xffff_ffff_ffff_ffff));
    assert_eq!(Mips4Alu::dsub(5, 3), Ok(2));
    assert_eq!(
        Mips4Alu::dsub(i64::MIN as u64, 1),
        Err(Mips4Exception::ArithmeticOverflow)
    );

    assert_eq!(Mips4Alu::daddu(u64::MAX, 1), 0);
    assert_eq!(Mips4Alu::daddiu(0, -1), u64::MAX);
    assert_eq!(Mips4Alu::dsubu(0, 1), u64::MAX);
}

#[test]
fn logical_operations_use_full_doubleword_values() {
    assert_eq!(
        Mips4Alu::and(0xf0f0_ffff_0000_ffff, 0x0ff0_00ff_ffff_00ff),
        0x00f0_00ff_0000_00ff
    );
    assert_eq!(
        Mips4Alu::or(0xf000_0000_0000_000f, 0x000f_f000_000f_f000),
        0xf00f_f000_000f_f00f
    );
    assert_eq!(
        Mips4Alu::xor(0xffff_0000_ffff_0000, 0x00ff_00ff_00ff_00ff),
        0xff00_00ff_ff00_00ff
    );
    assert_eq!(
        Mips4Alu::nor(0xffff_0000_ffff_0000, 0x00ff_00ff_00ff_00ff),
        0x0000_ff00_0000_ff00
    );
}

#[test]
fn logical_immediates_are_zero_extended() {
    assert_eq!(
        Mips4Alu::andi(0xffff_ffff_ffff_ffff, 0x8001),
        0x0000_0000_0000_8001
    );
    assert_eq!(
        Mips4Alu::ori(0x1234_5678_0000_0000, 0x8001),
        0x1234_5678_0000_8001
    );
    assert_eq!(
        Mips4Alu::xori(0xffff_ffff_ffff_0000, 0x8001),
        0xffff_ffff_ffff_8001
    );
}

#[test]
fn lui_sign_extends_the_word_result() {
    assert_eq!(Mips4Alu::lui(0x7fff), 0x0000_0000_7fff_0000);
    assert_eq!(Mips4Alu::lui(0x8001), 0xffff_ffff_8001_0000);
}

#[test]
fn word_shift_operations_use_low_five_bits_and_sign_extend() {
    assert_eq!(Mips4Alu::sll(1, 33), 2);
    assert_eq!(Mips4Alu::sll(0x8000_0000, 0), 0xffff_ffff_8000_0000);
    assert_eq!(Mips4Alu::srl(0x8000_0000, 1), 0x0000_0000_4000_0000);
    assert_eq!(Mips4Alu::srlv(0x8000_0000, 33), 0x0000_0000_4000_0000);
    assert_eq!(Mips4Alu::sra(0x8000_0000, 1), 0xffff_ffff_c000_0000);
    assert_eq!(Mips4Alu::srav(0x8000_0000, 33), 0xffff_ffff_c000_0000);
}

#[test]
fn doubleword_shift_operations_use_low_six_bits_and_32_variants() {
    assert_eq!(Mips4Alu::dsll(1, 65), 2);
    assert_eq!(Mips4Alu::dsll32(1, 1), 0x0000_0002_0000_0000);
    assert_eq!(Mips4Alu::dsllv(1, 65), 0x0000_0000_0000_0002);
    assert_eq!(
        Mips4Alu::dsrl(0x8000_0000_0000_0000, 1),
        0x4000_0000_0000_0000
    );
    assert_eq!(Mips4Alu::dsrl32(0x8000_0000_0000_0000, 0), 0x8000_0000);
    assert_eq!(
        Mips4Alu::dsrlv(0x8000_0000_0000_0000, 65),
        0x4000_0000_0000_0000
    );
    assert_eq!(
        Mips4Alu::dsra(0x8000_0000_0000_0000, 1),
        0xc000_0000_0000_0000
    );
    assert_eq!(
        Mips4Alu::dsra32(0x8000_0000_0000_0000, 0),
        0xffff_ffff_8000_0000
    );
    assert_eq!(
        Mips4Alu::dsrav(0x8000_0000_0000_0000, 65),
        0xc000_0000_0000_0000
    );
}

#[test]
fn comparisons_use_full_doubleword_values() {
    assert_eq!(Mips4Alu::slt(0xffff_ffff_ffff_ffff, 0), 1);
    assert_eq!(Mips4Alu::slti(0, -1), 0);
    assert_eq!(Mips4Alu::sltu(u64::MAX, 0), 0);
    assert_eq!(Mips4Alu::sltiu(0, -1), 1);
}

#[test]
fn word_multiply_and_divide_sign_extend_hi_lo_words() {
    assert_eq!(
        Mips4Alu::mult(0xffff_ffff, 2),
        Mips4HiLoResult {
            hi: 0xffff_ffff_ffff_ffff,
            lo: 0xffff_ffff_ffff_fffe,
        }
    );
    assert_eq!(
        Mips4Alu::multu(0xffff_ffff, 2),
        Mips4HiLoResult {
            hi: 1,
            lo: 0xffff_ffff_ffff_fffe,
        }
    );
    assert_eq!(
        Mips4Alu::div((-7_i32) as u32 as u64, 3),
        Some(Mips4HiLoResult {
            hi: 0xffff_ffff_ffff_ffff,
            lo: 0xffff_ffff_ffff_fffe,
        })
    );
    assert_eq!(Mips4Alu::divu(7, 3), Some(Mips4HiLoResult { hi: 1, lo: 2 }));
    assert_eq!(Mips4Alu::div(1, 0), None);
    assert_eq!(Mips4Alu::divu(1, 0), None);
    assert_eq!(
        Mips4Alu::div(i32::MIN as u32 as u64, (-1_i32) as u32 as u64),
        None
    );
}

#[test]
fn doubleword_multiply_and_divide_split_full_results() {
    assert_eq!(
        Mips4Alu::dmult(u64::MAX, 2),
        Mips4HiLoResult {
            hi: u64::MAX,
            lo: 0xffff_ffff_ffff_fffe,
        }
    );
    assert_eq!(
        Mips4Alu::dmultu(u64::MAX, 2),
        Mips4HiLoResult {
            hi: 1,
            lo: 0xffff_ffff_ffff_fffe,
        }
    );
    assert_eq!(
        Mips4Alu::ddiv((-7_i64) as u64, 3),
        Some(Mips4HiLoResult {
            hi: u64::MAX,
            lo: (-2_i64) as u64,
        })
    );
    assert_eq!(
        Mips4Alu::ddivu(7, 3),
        Some(Mips4HiLoResult { hi: 1, lo: 2 })
    );
    assert_eq!(Mips4Alu::ddiv(1, 0), None);
    assert_eq!(Mips4Alu::ddivu(1, 0), None);
    assert_eq!(Mips4Alu::ddiv(i64::MIN as u64, (-1_i64) as u64), None);
}

#[test]
fn conditional_moves_return_value_only_when_written() {
    assert_eq!(Mips4Alu::movn(0x1234, 1), Some(0x1234));
    assert_eq!(Mips4Alu::movn(0x1234, 0), None);
    assert_eq!(Mips4Alu::movz(0x1234, 0), Some(0x1234));
    assert_eq!(Mips4Alu::movz(0x1234, 1), None);
    assert_eq!(Mips4Alu::movt(0x1234, true), Some(0x1234));
    assert_eq!(Mips4Alu::movt(0x1234, false), None);
    assert_eq!(Mips4Alu::movf(0x1234, false), Some(0x1234));
    assert_eq!(Mips4Alu::movf(0x1234, true), None);
}
