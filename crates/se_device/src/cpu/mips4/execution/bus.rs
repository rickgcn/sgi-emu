//! Functional MIPS IV CPU bus protocol payloads.

use crate::cpu::mips4::cache::Mips4MemoryAccessType;

/// Transfer width of a functional CPU bus transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4ExecutionTransferSize {
    /// One byte.
    Byte,

    /// Two bytes.
    Halfword,

    /// Four bytes.
    Word,

    /// Eight bytes.
    Doubleword,
}

impl Mips4ExecutionTransferSize {
    /// Returns the transfer width in bytes.
    pub const fn bytes(self) -> u8 {
        match self {
            Self::Byte => 1,
            Self::Halfword => 2,
            Self::Word => 4,
            Self::Doubleword => 8,
        }
    }
}

/// Architectural purpose of a functional CPU bus transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4ExecutionAccessKind {
    /// Instruction fetch.
    InstructionFetch,

    /// Data load.
    DataLoad,

    /// Data store.
    DataStore,
}

/// Functional CPU bus transaction emitted by MIPS IV execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4ExecutionTransaction {
    /// Read bytes beginning at an aligned physical address.
    Read {
        /// Physical start address.
        physical_address: u64,

        /// Transfer width.
        size: Mips4ExecutionTransferSize,

        /// Architectural access purpose.
        kind: Mips4ExecutionAccessKind,

        /// Processor-resolved memory access type.
        access_type: Mips4MemoryAccessType,
    },

    /// Write enabled physical byte lanes.
    Write {
        /// Physical start address.
        physical_address: u64,

        /// Transfer container width.
        size: Mips4ExecutionTransferSize,

        /// Physical byte-lane data. The least significant byte corresponds to
        /// `physical_address`.
        data: u64,

        /// Enabled physical byte lanes relative to `physical_address`.
        byte_enable: u8,

        /// Processor-resolved memory access type.
        access_type: Mips4MemoryAccessType,
    },
}

/// Completion delivered for a functional MIPS IV CPU bus transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4ExecutionCompletion {
    /// Read data in physical byte-lane order.
    ReadData(u64),

    /// A write completed successfully.
    WriteComplete,

    /// The external bus reported an access failure.
    BusError,
}
