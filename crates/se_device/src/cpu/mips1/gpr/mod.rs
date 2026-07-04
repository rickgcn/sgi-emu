//! MIPS I general-purpose registers.
//!
//! MIPS I has 32 general-purpose registers. Register `$0` always reads as
//! zero, and writes to it do not change architectural state. This module does
//! not model `HI`, `LO`, the program counter, or pipeline delay state.

/// Number of MIPS I general-purpose registers.
pub const MIPS1_GPR_COUNT: usize = 32;

/// MIPS I general-purpose register index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1GprIndex(u8);

impl Mips1GprIndex {
    /// Zero register index.
    pub const ZERO: Self = Self(0);

    /// Creates a register index from a raw register number.
    pub const fn from_u8(value: u8) -> Option<Self> {
        if value < MIPS1_GPR_COUNT as u8 {
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

/// MIPS I general-purpose register file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips1GprFile {
    registers: [u32; MIPS1_GPR_COUNT],
}

impl Mips1GprFile {
    /// Creates a zeroed register file.
    pub const fn new() -> Self {
        Self {
            registers: [0; MIPS1_GPR_COUNT],
        }
    }

    /// Reads a general-purpose register.
    pub const fn read(&self, index: Mips1GprIndex) -> u32 {
        if index.number() == 0 {
            0
        } else {
            self.registers[index.usize_index()]
        }
    }

    /// Writes a general-purpose register.
    pub fn write(&mut self, index: Mips1GprIndex, value: u32) {
        if index.number() != 0 {
            self.registers[index.usize_index()] = value;
        }
    }

    /// Resets all general-purpose registers to zero.
    pub fn reset(&mut self) {
        self.registers = [0; MIPS1_GPR_COUNT];
    }
}

impl Default for Mips1GprFile {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
