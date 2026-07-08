//! MIPS IV general-purpose registers.
//!
//! MIPS IV has 32 general-purpose registers. Register `$0` always reads as
//! zero, and writes to it do not change architectural state. This module does
//! not model `HI`, `LO`, the program counter, or pipeline delay state.

/// Number of MIPS IV general-purpose registers.
pub const MIPS4_GPR_COUNT: usize = 32;

/// MIPS IV general-purpose register index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4GprIndex(u8);

impl Mips4GprIndex {
    /// Zero register index.
    pub const ZERO: Self = Self(0);

    /// Creates a register index from a raw register number.
    pub const fn from_u8(value: u8) -> Option<Self> {
        if value < MIPS4_GPR_COUNT as u8 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the raw register number.
    pub const fn number(self) -> u8 {
        self.0
    }

    /// Returns the register number as an array index.
    pub const fn usize_index(self) -> usize {
        self.0 as usize
    }
}

/// MIPS IV general-purpose register file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4GprFile {
    registers: [u64; MIPS4_GPR_COUNT],
}

impl Mips4GprFile {
    /// Creates a zeroed register file.
    pub const fn new() -> Self {
        Self {
            registers: [0; MIPS4_GPR_COUNT],
        }
    }

    /// Reads a general-purpose register.
    pub const fn read(&self, index: Mips4GprIndex) -> u64 {
        if index.number() == 0 {
            0
        } else {
            self.registers[index.usize_index()]
        }
    }

    /// Writes a general-purpose register.
    pub fn write(&mut self, index: Mips4GprIndex, value: u64) {
        if index.number() != 0 {
            self.registers[index.usize_index()] = value;
        }
    }

    /// Writes a sign-extended 32-bit word to a general-purpose register.
    pub fn write_sign_extended_word(&mut self, index: Mips4GprIndex, value: u32) {
        self.write(index, sign_extend_word(value));
    }

    /// Resets all general-purpose registers to zero.
    pub fn reset(&mut self) {
        self.registers = [0; MIPS4_GPR_COUNT];
    }
}

impl Default for Mips4GprFile {
    fn default() -> Self {
        Self::new()
    }
}

/// Sign-extends a 32-bit word to a MIPS IV 64-bit general-purpose value.
pub const fn sign_extend_word(value: u32) -> u64 {
    value as i32 as i64 as u64
}

/// Returns whether a 64-bit value is a sign-extended 32-bit word.
pub const fn is_sign_extended_word(value: u64) -> bool {
    value == sign_extend_word(value as u32)
}

#[cfg(test)]
mod tests;
