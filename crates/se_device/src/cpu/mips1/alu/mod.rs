//! Pure MIPS I integer ALU operations.
//!
//! This module implements register-width arithmetic, logical operations,
//! shifts, comparisons, and multiply/divide result calculation for MIPS I. It
//! does not read or write the general-purpose register file, update `HI` or
//! `LO`, decode instructions, or model pipeline timing.

use crate::cpu::mips1::exception::Mips1Exception;

/// Result destined for the MIPS I `HI` and `LO` registers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips1HiLoResult {
    /// Value written to `HI`.
    pub hi: u32,

    /// Value written to `LO`.
    pub lo: u32,
}

impl Mips1HiLoResult {
    fn from_u64(value: u64) -> Self {
        Self {
            hi: (value >> 32) as u32,
            lo: value as u32,
        }
    }
}

/// Stateless MIPS I integer ALU helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mips1Alu;

impl Mips1Alu {
    /// Adds two signed words and reports arithmetic overflow.
    pub fn add(lhs: u32, rhs: u32) -> Result<u32, Mips1Exception> {
        signed_word(lhs)
            .checked_add(signed_word(rhs))
            .map(unsigned_word)
            .ok_or(Mips1Exception::ArithmeticOverflow)
    }

    /// Adds a sign-extended immediate to a signed word and reports overflow.
    pub fn add_immediate(lhs: u32, immediate: i16) -> Result<u32, Mips1Exception> {
        signed_word(lhs)
            .checked_add(immediate as i32)
            .map(unsigned_word)
            .ok_or(Mips1Exception::ArithmeticOverflow)
    }

    /// Subtracts two signed words and reports arithmetic overflow.
    pub fn sub(lhs: u32, rhs: u32) -> Result<u32, Mips1Exception> {
        signed_word(lhs)
            .checked_sub(signed_word(rhs))
            .map(unsigned_word)
            .ok_or(Mips1Exception::ArithmeticOverflow)
    }

    /// Adds two words without trapping on overflow.
    pub const fn addu(lhs: u32, rhs: u32) -> u32 {
        lhs.wrapping_add(rhs)
    }

    /// Adds a sign-extended immediate without trapping on overflow.
    pub const fn addiu(lhs: u32, immediate: i16) -> u32 {
        lhs.wrapping_add(immediate as i32 as u32)
    }

    /// Subtracts two words without trapping on overflow.
    pub const fn subu(lhs: u32, rhs: u32) -> u32 {
        lhs.wrapping_sub(rhs)
    }

    /// Returns the bitwise AND of two words.
    pub const fn and(lhs: u32, rhs: u32) -> u32 {
        lhs & rhs
    }

    /// Returns the bitwise AND with a zero-extended immediate.
    pub const fn andi(lhs: u32, immediate: u16) -> u32 {
        lhs & immediate as u32
    }

    /// Returns the bitwise OR of two words.
    pub const fn or(lhs: u32, rhs: u32) -> u32 {
        lhs | rhs
    }

    /// Returns the bitwise OR with a zero-extended immediate.
    pub const fn ori(lhs: u32, immediate: u16) -> u32 {
        lhs | immediate as u32
    }

    /// Returns the bitwise XOR of two words.
    pub const fn xor(lhs: u32, rhs: u32) -> u32 {
        lhs ^ rhs
    }

    /// Returns the bitwise XOR with a zero-extended immediate.
    pub const fn xori(lhs: u32, immediate: u16) -> u32 {
        lhs ^ immediate as u32
    }

    /// Returns the bitwise NOR of two words.
    pub const fn nor(lhs: u32, rhs: u32) -> u32 {
        !(lhs | rhs)
    }

    /// Loads an immediate into the high halfword.
    pub const fn lui(immediate: u16) -> u32 {
        (immediate as u32) << 16
    }

    /// Shifts a word left by a fixed shift amount.
    pub const fn sll(value: u32, shift: u8) -> u32 {
        value << shift_amount(shift)
    }

    /// Shifts a word left by the low five bits of a register value.
    pub const fn sllv(value: u32, shift_source: u32) -> u32 {
        value << variable_shift_amount(shift_source)
    }

    /// Shifts a word right logically by a fixed shift amount.
    pub const fn srl(value: u32, shift: u8) -> u32 {
        value >> shift_amount(shift)
    }

    /// Shifts a word right logically by the low five bits of a register value.
    pub const fn srlv(value: u32, shift_source: u32) -> u32 {
        value >> variable_shift_amount(shift_source)
    }

    /// Shifts a word right arithmetically by a fixed shift amount.
    pub const fn sra(value: u32, shift: u8) -> u32 {
        ((value as i32) >> shift_amount(shift)) as u32
    }

    /// Shifts a word right arithmetically by the low five bits of a register value.
    pub const fn srav(value: u32, shift_source: u32) -> u32 {
        ((value as i32) >> variable_shift_amount(shift_source)) as u32
    }

    /// Returns `1` when the signed left operand is less than the signed right operand.
    pub const fn slt(lhs: u32, rhs: u32) -> u32 {
        (signed_word(lhs) < signed_word(rhs)) as u32
    }

    /// Returns `1` when the signed left operand is less than the sign-extended immediate.
    pub const fn slti(lhs: u32, immediate: i16) -> u32 {
        (signed_word(lhs) < immediate as i32) as u32
    }

    /// Returns `1` when the unsigned left operand is less than the unsigned right operand.
    pub const fn sltu(lhs: u32, rhs: u32) -> u32 {
        (lhs < rhs) as u32
    }

    /// Returns `1` when the unsigned left operand is less than the sign-extended immediate.
    pub const fn sltiu(lhs: u32, immediate: i16) -> u32 {
        (lhs < immediate as i32 as u32) as u32
    }

    /// Multiplies two signed words and returns the `HI` and `LO` result.
    pub fn mult(lhs: u32, rhs: u32) -> Mips1HiLoResult {
        Mips1HiLoResult::from_u64((signed_word(lhs) as i64 * signed_word(rhs) as i64) as u64)
    }

    /// Multiplies two unsigned words and returns the `HI` and `LO` result.
    pub fn multu(lhs: u32, rhs: u32) -> Mips1HiLoResult {
        Mips1HiLoResult::from_u64(lhs as u64 * rhs as u64)
    }

    /// Divides two signed words and returns quotient in `LO` and remainder in `HI`.
    pub fn div(lhs: u32, rhs: u32) -> Option<Mips1HiLoResult> {
        let dividend = signed_word(lhs);
        let divisor = signed_word(rhs);

        if divisor == 0 || dividend == i32::MIN && divisor == -1 {
            None
        } else {
            Some(Mips1HiLoResult {
                hi: unsigned_word(dividend % divisor),
                lo: unsigned_word(dividend / divisor),
            })
        }
    }

    /// Divides two unsigned words and returns quotient in `LO` and remainder in `HI`.
    pub fn divu(lhs: u32, rhs: u32) -> Option<Mips1HiLoResult> {
        if rhs == 0 {
            None
        } else {
            Some(Mips1HiLoResult {
                hi: lhs % rhs,
                lo: lhs / rhs,
            })
        }
    }
}

const fn signed_word(value: u32) -> i32 {
    value as i32
}

const fn unsigned_word(value: i32) -> u32 {
    value as u32
}

const fn shift_amount(value: u8) -> u32 {
    (value & 0x1f) as u32
}

const fn variable_shift_amount(value: u32) -> u32 {
    value & 0x1f
}

#[cfg(test)]
mod tests;
