//! MIPS I exception classifications.
//!
//! This module describes exception reasons and their R30xx Cause register
//! `ExcCode` values. It does not update CP0 registers, compute exception
//! vectors, or manage exception restart state.

/// MIPS I coprocessor number.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips1CoprocessorNumber {
    /// System control coprocessor.
    Cp0,

    /// Floating-point coprocessor.
    Cp1,

    /// Coprocessor 2.
    Cp2,

    /// Coprocessor 3.
    Cp3,
}

impl Mips1CoprocessorNumber {
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

/// MIPS I exception reason.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips1Exception {
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
        coprocessor: Mips1CoprocessorNumber,
    },

    /// Arithmetic overflow exception.
    ArithmeticOverflow,
}

impl Mips1Exception {
    /// Returns the R30xx Cause register `ExcCode` value for this exception.
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
        }
    }
}

#[cfg(test)]
mod tests;
