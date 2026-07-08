//! MIPS IV exception classifications.
//!
//! This module describes exception reasons and their Cause register `ExcCode`
//! values. It does not update CP0 registers, compute exception vectors, or
//! manage exception restart state.

/// MIPS IV coprocessor number.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4CoprocessorNumber {
    /// System control coprocessor.
    Cp0,

    /// Floating-point coprocessor.
    Cp1,

    /// Coprocessor 2.
    Cp2,

    /// Coprocessor 3.
    Cp3,
}

impl Mips4CoprocessorNumber {
    /// Creates a coprocessor number from its raw instruction field value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Cp0),
            1 => Some(Self::Cp1),
            2 => Some(Self::Cp2),
            3 => Some(Self::Cp3),
            _ => None,
        }
    }

    /// Returns the raw coprocessor number.
    pub const fn number(self) -> u8 {
        match self {
            Self::Cp0 => 0,
            Self::Cp1 => 1,
            Self::Cp2 => 2,
            Self::Cp3 => 3,
        }
    }
}

/// MIPS IV exception reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Exception {
    /// External, software, or coprocessor interrupt.
    Interrupt,

    /// TLB modification exception.
    TlbModification,

    /// TLB exception on load or instruction fetch.
    TlbLoad,

    /// TLB exception on store.
    TlbStore,

    /// Address error on load or instruction fetch.
    AddressErrorLoad,

    /// Address error on store.
    AddressErrorStore,

    /// Bus error on instruction fetch.
    InstructionBusError,

    /// Bus error on data load.
    DataBusError,

    /// `syscall` instruction exception.
    Syscall,

    /// `break` instruction exception.
    Breakpoint,

    /// Reserved instruction exception.
    ReservedInstruction,

    /// Coprocessor unusable exception.
    CoprocessorUnusable {
        /// Coprocessor selected by the trapping instruction.
        coprocessor: Mips4CoprocessorNumber,
    },

    /// Arithmetic overflow exception.
    ArithmeticOverflow,

    /// Trap instruction exception.
    Trap,

    /// Floating-point exception.
    FloatingPoint,
}

impl Mips4Exception {
    /// Returns the Cause register `ExcCode` value for this exception.
    pub const fn cause_code(self) -> u8 {
        match self {
            Self::Interrupt => 0,
            Self::TlbModification => 1,
            Self::TlbLoad => 2,
            Self::TlbStore => 3,
            Self::AddressErrorLoad => 4,
            Self::AddressErrorStore => 5,
            Self::InstructionBusError => 6,
            Self::DataBusError => 7,
            Self::Syscall => 8,
            Self::Breakpoint => 9,
            Self::ReservedInstruction => 10,
            Self::CoprocessorUnusable { .. } => 11,
            Self::ArithmeticOverflow => 12,
            Self::Trap => 13,
            Self::FloatingPoint => 15,
        }
    }
}

#[cfg(test)]
mod tests;
