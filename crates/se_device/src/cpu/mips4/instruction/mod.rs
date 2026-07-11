//! MIPS IV instruction bit fields and decode helpers.
//!
//! This module provides exact bit extraction for the 32-bit MIPS IV instruction
//! word. Decode helpers classify instruction encodings without executing them.

pub mod decode;
pub mod requirements;

/// Raw 32-bit MIPS IV instruction word.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4Instruction(u32);

impl Mips4Instruction {
    /// Creates a raw MIPS IV instruction.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw instruction bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the primary opcode field.
    pub const fn opcode(self) -> u8 {
        ((self.0 >> 26) & 0x3f) as u8
    }

    /// Returns the `rs` register field.
    pub const fn rs(self) -> u8 {
        ((self.0 >> 21) & 0x1f) as u8
    }

    /// Returns the `rt` register field.
    pub const fn rt(self) -> u8 {
        ((self.0 >> 16) & 0x1f) as u8
    }

    /// Returns the `rd` register field.
    pub const fn rd(self) -> u8 {
        ((self.0 >> 11) & 0x1f) as u8
    }

    /// Returns the shift amount field.
    pub const fn shamt(self) -> u8 {
        ((self.0 >> 6) & 0x1f) as u8
    }

    /// Returns the function field.
    pub const fn funct(self) -> u8 {
        (self.0 & 0x3f) as u8
    }

    /// Returns the unsigned immediate field.
    pub const fn immediate(self) -> u16 {
        (self.0 & 0xffff) as u16
    }

    /// Returns the signed immediate field.
    pub const fn signed_immediate(self) -> i16 {
        self.immediate() as i16
    }

    /// Returns the branch displacement in bytes.
    pub const fn branch_offset(self) -> i32 {
        (self.signed_immediate() as i32) << 2
    }

    /// Returns the raw jump target field.
    pub const fn target(self) -> u32 {
        self.0 & 0x03ff_ffff
    }

    /// Returns the shifted jump index.
    pub const fn jump_index(self) -> u32 {
        self.target() << 2
    }

    /// Returns the coprocessor or floating-point format field.
    pub const fn fmt(self) -> u8 {
        self.rs()
    }

    /// Returns the floating-point `ft` register field.
    pub const fn ft(self) -> u8 {
        self.rt()
    }

    /// Returns the floating-point `fs` register field.
    pub const fn fs(self) -> u8 {
        self.rd()
    }

    /// Returns the floating-point `fd` register field.
    pub const fn fd(self) -> u8 {
        self.shamt()
    }
}

#[cfg(test)]
mod tests;
