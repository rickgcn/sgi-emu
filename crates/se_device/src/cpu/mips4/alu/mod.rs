//! Pure MIPS IV integer ALU operations.
//!
//! This module implements register-width arithmetic, logical operations,
//! shifts, comparisons, conditional move decisions, and multiply/divide result
//! calculation for MIPS IV. It does not read or write the general-purpose
//! register file, update `HI` or `LO`, decode instructions, or model pipeline
//! timing.

use crate::cpu::mips4::exception::Mips4Exception;
use crate::cpu::mips4::gpr::sign_extend_word;

/// Result destined for the MIPS IV `HI` and `LO` registers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4HiLoResult {
    /// Value written to `HI`.
    pub hi: u64,

    /// Value written to `LO`.
    pub lo: u64,
}

impl Mips4HiLoResult {
    fn from_word_product(value: u64) -> Self {
        Self {
            hi: sign_extend_word((value >> 32) as u32),
            lo: sign_extend_word(value as u32),
        }
    }

    fn from_doubleword_product(value: u128) -> Self {
        Self {
            hi: (value >> 64) as u64,
            lo: value as u64,
        }
    }
}

/// Stateless MIPS IV integer ALU helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mips4Alu;

impl Mips4Alu {
    /// Adds two signed words and reports arithmetic overflow.
    pub fn add(lhs: u64, rhs: u64) -> Result<u64, Mips4Exception> {
        signed_word(lhs)
            .checked_add(signed_word(rhs))
            .map(signed_word_result)
            .ok_or(Mips4Exception::ArithmeticOverflow)
    }

    /// Adds a sign-extended immediate to a signed word and reports overflow.
    pub fn add_immediate(lhs: u64, immediate: i16) -> Result<u64, Mips4Exception> {
        signed_word(lhs)
            .checked_add(immediate as i32)
            .map(signed_word_result)
            .ok_or(Mips4Exception::ArithmeticOverflow)
    }

    /// Subtracts two signed words and reports arithmetic overflow.
    pub fn sub(lhs: u64, rhs: u64) -> Result<u64, Mips4Exception> {
        signed_word(lhs)
            .checked_sub(signed_word(rhs))
            .map(signed_word_result)
            .ok_or(Mips4Exception::ArithmeticOverflow)
    }

    /// Adds two words without trapping on overflow.
    pub const fn addu(lhs: u64, rhs: u64) -> u64 {
        sign_extend_word((lhs as u32).wrapping_add(rhs as u32))
    }

    /// Adds a sign-extended immediate to a word without trapping on overflow.
    pub const fn addiu(lhs: u64, immediate: i16) -> u64 {
        sign_extend_word((lhs as u32).wrapping_add(immediate as i32 as u32))
    }

    /// Subtracts two words without trapping on overflow.
    pub const fn subu(lhs: u64, rhs: u64) -> u64 {
        sign_extend_word((lhs as u32).wrapping_sub(rhs as u32))
    }

    /// Adds two signed doublewords and reports arithmetic overflow.
    pub fn dadd(lhs: u64, rhs: u64) -> Result<u64, Mips4Exception> {
        signed_doubleword(lhs)
            .checked_add(signed_doubleword(rhs))
            .map(unsigned_doubleword)
            .ok_or(Mips4Exception::ArithmeticOverflow)
    }

    /// Adds a sign-extended immediate to a signed doubleword and reports overflow.
    pub fn daddi(lhs: u64, immediate: i16) -> Result<u64, Mips4Exception> {
        signed_doubleword(lhs)
            .checked_add(immediate as i64)
            .map(unsigned_doubleword)
            .ok_or(Mips4Exception::ArithmeticOverflow)
    }

    /// Subtracts two signed doublewords and reports arithmetic overflow.
    pub fn dsub(lhs: u64, rhs: u64) -> Result<u64, Mips4Exception> {
        signed_doubleword(lhs)
            .checked_sub(signed_doubleword(rhs))
            .map(unsigned_doubleword)
            .ok_or(Mips4Exception::ArithmeticOverflow)
    }

    /// Adds two doublewords without trapping on overflow.
    pub const fn daddu(lhs: u64, rhs: u64) -> u64 {
        lhs.wrapping_add(rhs)
    }

    /// Adds a sign-extended immediate to a doubleword without trapping on overflow.
    pub const fn daddiu(lhs: u64, immediate: i16) -> u64 {
        lhs.wrapping_add(immediate as i64 as u64)
    }

    /// Subtracts two doublewords without trapping on overflow.
    pub const fn dsubu(lhs: u64, rhs: u64) -> u64 {
        lhs.wrapping_sub(rhs)
    }

    /// Returns the bitwise AND of two doublewords.
    pub const fn and(lhs: u64, rhs: u64) -> u64 {
        lhs & rhs
    }

    /// Returns the bitwise AND with a zero-extended immediate.
    pub const fn andi(lhs: u64, immediate: u16) -> u64 {
        lhs & immediate as u64
    }

    /// Returns the bitwise OR of two doublewords.
    pub const fn or(lhs: u64, rhs: u64) -> u64 {
        lhs | rhs
    }

    /// Returns the bitwise OR with a zero-extended immediate.
    pub const fn ori(lhs: u64, immediate: u16) -> u64 {
        lhs | immediate as u64
    }

    /// Returns the bitwise XOR of two doublewords.
    pub const fn xor(lhs: u64, rhs: u64) -> u64 {
        lhs ^ rhs
    }

    /// Returns the bitwise XOR with a zero-extended immediate.
    pub const fn xori(lhs: u64, immediate: u16) -> u64 {
        lhs ^ immediate as u64
    }

    /// Returns the bitwise NOR of two doublewords.
    pub const fn nor(lhs: u64, rhs: u64) -> u64 {
        !(lhs | rhs)
    }

    /// Loads an immediate into the high halfword of a sign-extended word.
    pub const fn lui(immediate: u16) -> u64 {
        sign_extend_word((immediate as u32) << 16)
    }

    /// Shifts a word left by a fixed shift amount.
    pub const fn sll(value: u64, shift: u8) -> u64 {
        sign_extend_word((value as u32).wrapping_shl(word_shift_amount(shift)))
    }

    /// Shifts a word left by the low five bits of a register value.
    pub const fn sllv(value: u64, shift_source: u64) -> u64 {
        sign_extend_word((value as u32).wrapping_shl(variable_word_shift_amount(shift_source)))
    }

    /// Shifts a word right logically by a fixed shift amount.
    pub const fn srl(value: u64, shift: u8) -> u64 {
        sign_extend_word((value as u32).wrapping_shr(word_shift_amount(shift)))
    }

    /// Shifts a word right logically by the low five bits of a register value.
    pub const fn srlv(value: u64, shift_source: u64) -> u64 {
        sign_extend_word((value as u32).wrapping_shr(variable_word_shift_amount(shift_source)))
    }

    /// Shifts a word right arithmetically by a fixed shift amount.
    pub const fn sra(value: u64, shift: u8) -> u64 {
        sign_extend_word(((value as u32 as i32) >> word_shift_amount(shift)) as u32)
    }

    /// Shifts a word right arithmetically by the low five bits of a register value.
    pub const fn srav(value: u64, shift_source: u64) -> u64 {
        sign_extend_word(((value as u32 as i32) >> variable_word_shift_amount(shift_source)) as u32)
    }

    /// Shifts a doubleword left by a fixed shift amount.
    pub const fn dsll(value: u64, shift: u8) -> u64 {
        value.wrapping_shl(doubleword_shift_amount(shift))
    }

    /// Shifts a doubleword left by a fixed shift amount plus 32.
    pub const fn dsll32(value: u64, shift: u8) -> u64 {
        value.wrapping_shl(word_shift_amount(shift) + 32)
    }

    /// Shifts a doubleword left by the low six bits of a register value.
    pub const fn dsllv(value: u64, shift_source: u64) -> u64 {
        value.wrapping_shl(variable_doubleword_shift_amount(shift_source))
    }

    /// Shifts a doubleword right logically by a fixed shift amount.
    pub const fn dsrl(value: u64, shift: u8) -> u64 {
        value.wrapping_shr(doubleword_shift_amount(shift))
    }

    /// Shifts a doubleword right logically by a fixed shift amount plus 32.
    pub const fn dsrl32(value: u64, shift: u8) -> u64 {
        value.wrapping_shr(word_shift_amount(shift) + 32)
    }

    /// Shifts a doubleword right logically by the low six bits of a register value.
    pub const fn dsrlv(value: u64, shift_source: u64) -> u64 {
        value.wrapping_shr(variable_doubleword_shift_amount(shift_source))
    }

    /// Shifts a doubleword right arithmetically by a fixed shift amount.
    pub const fn dsra(value: u64, shift: u8) -> u64 {
        ((value as i64) >> doubleword_shift_amount(shift)) as u64
    }

    /// Shifts a doubleword right arithmetically by a fixed shift amount plus 32.
    pub const fn dsra32(value: u64, shift: u8) -> u64 {
        ((value as i64) >> (word_shift_amount(shift) + 32)) as u64
    }

    /// Shifts a doubleword right arithmetically by the low six bits of a register value.
    pub const fn dsrav(value: u64, shift_source: u64) -> u64 {
        ((value as i64) >> variable_doubleword_shift_amount(shift_source)) as u64
    }

    /// Returns `1` when the signed left operand is less than the signed right operand.
    pub const fn slt(lhs: u64, rhs: u64) -> u64 {
        ((lhs as i64) < (rhs as i64)) as u64
    }

    /// Returns `1` when the signed left operand is less than the sign-extended immediate.
    pub const fn slti(lhs: u64, immediate: i16) -> u64 {
        ((lhs as i64) < immediate as i64) as u64
    }

    /// Returns `1` when the unsigned left operand is less than the unsigned right operand.
    pub const fn sltu(lhs: u64, rhs: u64) -> u64 {
        (lhs < rhs) as u64
    }

    /// Returns `1` when the unsigned left operand is less than the sign-extended immediate.
    pub const fn sltiu(lhs: u64, immediate: i16) -> u64 {
        (lhs < immediate as i64 as u64) as u64
    }

    /// Multiplies two signed words and returns the `HI` and `LO` result.
    pub fn mult(lhs: u64, rhs: u64) -> Mips4HiLoResult {
        Mips4HiLoResult::from_word_product(
            (signed_word(lhs) as i64 * signed_word(rhs) as i64) as u64,
        )
    }

    /// Multiplies two unsigned words and returns the `HI` and `LO` result.
    pub fn multu(lhs: u64, rhs: u64) -> Mips4HiLoResult {
        Mips4HiLoResult::from_word_product(lhs as u32 as u64 * rhs as u32 as u64)
    }

    /// Divides two signed words and returns quotient in `LO` and remainder in `HI`.
    pub fn div(lhs: u64, rhs: u64) -> Option<Mips4HiLoResult> {
        let dividend = signed_word(lhs);
        let divisor = signed_word(rhs);

        if divisor == 0 || dividend == i32::MIN && divisor == -1 {
            None
        } else {
            Some(Mips4HiLoResult {
                hi: sign_extend_word((dividend % divisor) as u32),
                lo: sign_extend_word((dividend / divisor) as u32),
            })
        }
    }

    /// Divides two unsigned words and returns quotient in `LO` and remainder in `HI`.
    pub fn divu(lhs: u64, rhs: u64) -> Option<Mips4HiLoResult> {
        let dividend = lhs as u32;
        let divisor = rhs as u32;

        if divisor == 0 {
            None
        } else {
            Some(Mips4HiLoResult {
                hi: sign_extend_word(dividend % divisor),
                lo: sign_extend_word(dividend / divisor),
            })
        }
    }

    /// Multiplies two signed doublewords and returns the `HI` and `LO` result.
    pub fn dmult(lhs: u64, rhs: u64) -> Mips4HiLoResult {
        Mips4HiLoResult::from_doubleword_product(
            (signed_doubleword(lhs) as i128 * signed_doubleword(rhs) as i128) as u128,
        )
    }

    /// Multiplies two unsigned doublewords and returns the `HI` and `LO` result.
    pub fn dmultu(lhs: u64, rhs: u64) -> Mips4HiLoResult {
        Mips4HiLoResult::from_doubleword_product(lhs as u128 * rhs as u128)
    }

    /// Divides two signed doublewords and returns quotient in `LO` and remainder in `HI`.
    pub fn ddiv(lhs: u64, rhs: u64) -> Option<Mips4HiLoResult> {
        let dividend = signed_doubleword(lhs);
        let divisor = signed_doubleword(rhs);

        if divisor == 0 || dividend == i64::MIN && divisor == -1 {
            None
        } else {
            Some(Mips4HiLoResult {
                hi: unsigned_doubleword(dividend % divisor),
                lo: unsigned_doubleword(dividend / divisor),
            })
        }
    }

    /// Divides two unsigned doublewords and returns quotient in `LO` and remainder in `HI`.
    pub fn ddivu(lhs: u64, rhs: u64) -> Option<Mips4HiLoResult> {
        if rhs == 0 {
            None
        } else {
            Some(Mips4HiLoResult {
                hi: lhs % rhs,
                lo: lhs / rhs,
            })
        }
    }

    /// Returns the moved value for `MOVN` when the condition register is not zero.
    pub const fn movn(value: u64, condition: u64) -> Option<u64> {
        if condition != 0 { Some(value) } else { None }
    }

    /// Returns the moved value for `MOVZ` when the condition register is zero.
    pub const fn movz(value: u64, condition: u64) -> Option<u64> {
        if condition == 0 { Some(value) } else { None }
    }

    /// Returns the moved value for `MOVT` when the condition is true.
    pub const fn movt(value: u64, condition: bool) -> Option<u64> {
        if condition { Some(value) } else { None }
    }

    /// Returns the moved value for `MOVF` when the condition is false.
    pub const fn movf(value: u64, condition: bool) -> Option<u64> {
        if !condition { Some(value) } else { None }
    }
}

const fn signed_word(value: u64) -> i32 {
    value as u32 as i32
}

const fn signed_word_result(value: i32) -> u64 {
    sign_extend_word(value as u32)
}

const fn signed_doubleword(value: u64) -> i64 {
    value as i64
}

const fn unsigned_doubleword(value: i64) -> u64 {
    value as u64
}

const fn word_shift_amount(value: u8) -> u32 {
    (value & 0x1f) as u32
}

const fn doubleword_shift_amount(value: u8) -> u32 {
    (value & 0x3f) as u32
}

const fn variable_word_shift_amount(value: u64) -> u32 {
    (value & 0x1f) as u32
}

const fn variable_doubleword_shift_amount(value: u64) -> u32 {
    (value & 0x3f) as u32
}

#[cfg(test)]
mod tests;
