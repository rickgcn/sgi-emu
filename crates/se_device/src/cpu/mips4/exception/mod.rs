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

/// Checks whether a coprocessor operation may proceed.
///
/// This helper only classifies the architectural exception. It does not read or
/// update CP0 state.
pub const fn check_coprocessor_access(
    coprocessor: Mips4CoprocessorNumber,
    usable: bool,
) -> Result<(), Mips4Exception> {
    if usable {
        Ok(())
    } else {
        Err(Mips4Exception::CoprocessorUnusable { coprocessor })
    }
}

/// Restart metadata for an excepting instruction.
///
/// Records the branch delay-slot flag (`Cause.BD`) and the program counter at
/// which execution resumes after the exception (`EPC`). When the excepting
/// instruction is in a branch delay slot, the exception resumes at the branch
/// instruction so it is re-executed; otherwise it resumes at the excepting
/// instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4ExceptionRestart {
    /// Whether the excepting instruction executes in a branch delay slot.
    pub in_branch_delay_slot: bool,

    /// Program counter at which execution resumes after the exception.
    pub restart_pc: u64,
}

impl Mips4ExceptionRestart {
    /// Creates restart metadata for an excepting instruction.
    ///
    /// `branch_pc` is `Some(branch_instruction_pc)` when the excepting
    /// instruction is in a branch delay slot; the exception resumes at the
    /// branch with the delay-slot flag set. When `branch_pc` is `None`, the
    /// exception resumes at `instruction_pc`.
    pub const fn new(instruction_pc: u64, branch_pc: Option<u64>) -> Self {
        match branch_pc {
            Some(branch_pc) => Self {
                in_branch_delay_slot: true,
                restart_pc: branch_pc,
            },
            None => Self {
                in_branch_delay_slot: false,
                restart_pc: instruction_pc,
            },
        }
    }
}

/// Immutable image of a signalled exception.
///
/// This is the manual `SignalException` shape expressed as a pure result: it
/// records the exception reason, restart metadata, and an optional bad virtual
/// address. It does not write CP0 registers (`EPC`, `Cause`, `BadVAddr`),
/// select an exception vector, or mutate architectural state. A processor model
/// or execution layer consumes the image to update CP0 and vector to the
/// handler.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4ExceptionImage {
    /// Reason for the exception.
    pub reason: Mips4Exception,

    /// Restart metadata recording the delay-slot flag and resume program counter.
    pub restart: Mips4ExceptionRestart,

    /// Bad virtual address for address-related exceptions, when applicable.
    pub bad_virtual_address: Option<u64>,
}

impl Mips4ExceptionImage {
    /// Creates an exception image from its reason, restart metadata, and optional bad address.
    pub const fn new(
        reason: Mips4Exception,
        restart: Mips4ExceptionRestart,
        bad_virtual_address: Option<u64>,
    ) -> Self {
        Self {
            reason,
            restart,
            bad_virtual_address,
        }
    }
}

#[cfg(test)]
mod tests;
