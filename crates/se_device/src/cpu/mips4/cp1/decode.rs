//! Generic MIPS IV CP1 instruction encoding classification.
//!
//! This module classifies FPU instruction encodings without executing floating
//! point operations, reading FPR contents, or choosing implementation-specific
//! reserved-operation behavior.

use crate::cpu::mips4::instruction::Mips4Instruction;

use super::{
    MIPS4_COP1_OPCODE, MIPS4_COP1X_OPCODE, MIPS4_LDC1_OPCODE, MIPS4_LWC1_OPCODE, MIPS4_SDC1_OPCODE,
    MIPS4_SWC1_OPCODE, Mips4Cp1Format,
};

const MIPS4_SPECIAL_OPCODE: u8 = 0x00;
const MIPS4_SPECIAL_MOVCI_FUNCTION: u8 = 0x01;

const CP1_FMT_MFC1: u8 = 0x00;
const CP1_FMT_DMFC1: u8 = 0x01;
const CP1_FMT_CFC1: u8 = 0x02;
const CP1_FMT_MTC1: u8 = 0x04;
const CP1_FMT_DMTC1: u8 = 0x05;
const CP1_FMT_CTC1: u8 = 0x06;
const CP1_FMT_BRANCH: u8 = 0x08;

/// Decode result for a CP1-related MIPS IV instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1Decode {
    /// The encoding is a defined CP1 instruction or instruction class.
    Instruction(Mips4Cp1InstructionClass),

    /// The encoding is reserved or a floating-point unimplemented operation.
    ReservedOrUnimplementedOperation,
}

/// CP1 instruction class after generic decode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1InstructionClass {
    /// CP1 base+offset load or store.
    OffsetMemory(Mips4Cp1OffsetMemoryOperation),

    /// CPU/FPU register transfer through the COP1 opcode.
    RegisterTransfer(Mips4Cp1RegisterTransferOperation),

    /// CP1 condition-code branch.
    Branch(Mips4Cp1BranchOperation),

    /// Formatted CP1 arithmetic, conversion, compare, or FPR move operation.
    Formatted {
        /// Formatted operation selected by the function field.
        operation: Mips4Cp1Operation,

        /// Operand format selected by the fmt field or COP1X function.
        format: Mips4Cp1Format,
    },

    /// COP1X indexed load or store.
    IndexedMemory(Mips4Cp1IndexedMemoryOperation),

    /// COP1X indexed prefetch.
    IndexedPrefetch,

    /// SPECIAL/MOVCI CPU conditional move on an FPU condition code.
    Movci(Mips4Cp1MovciOperation),
}

/// CP1 base+offset load/store operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1OffsetMemoryOperation {
    /// `LWC1`.
    LoadWord,
    /// `LDC1`.
    LoadDoubleword,
    /// `SWC1`.
    StoreWord,
    /// `SDC1`.
    StoreDoubleword,
}

/// CP1 CPU/FPU register transfer operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1RegisterTransferOperation {
    /// `MFC1`.
    MoveWordFrom,
    /// `DMFC1`.
    MoveDoublewordFrom,
    /// `CFC1`.
    MoveControlFrom,
    /// `MTC1`.
    MoveWordTo,
    /// `DMTC1`.
    MoveDoublewordTo,
    /// `CTC1`.
    MoveControlTo,
}

/// CP1 condition-code branch operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1BranchOperation {
    /// `BC1F`.
    BranchFalse,
    /// `BC1T`.
    BranchTrue,
    /// `BC1FL`.
    BranchFalseLikely,
    /// `BC1TL`.
    BranchTrueLikely,
}

/// COP1X indexed load/store operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1IndexedMemoryOperation {
    /// `LWXC1`.
    LoadWordIndexed,
    /// `LDXC1`.
    LoadDoublewordIndexed,
    /// `SWXC1`.
    StoreWordIndexed,
    /// `SDXC1`.
    StoreDoublewordIndexed,
}

/// SPECIAL/MOVCI CPU conditional move operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1MovciOperation {
    /// `MOVF`.
    MoveFalse,
    /// `MOVT`.
    MoveTrue,
}

/// Validity of a formatted operand type for one CP1 operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1OperandFormatStatus {
    /// The operation and operand format combination is architecturally valid.
    Valid,

    /// The operation and operand format combination is unimplemented or reserved.
    UnimplementedOrReserved,

    /// The operation and operand format combination is invalid.
    Invalid,
}

/// CP1 formatted operation selected by COP1 or COP1X decode.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1Operation {
    /// `ABS.fmt`.
    Absolute,
    /// `ADD.fmt`.
    Add,
    /// `C.cond.fmt`.
    Compare(Mips4Cp1CompareCondition),
    /// `CEIL.L.fmt`.
    CeilLong,
    /// `CEIL.W.fmt`.
    CeilWord,
    /// `CVT.D.fmt`.
    ConvertDouble,
    /// `CVT.L.fmt`.
    ConvertLong,
    /// `CVT.S.fmt`.
    ConvertSingle,
    /// `CVT.W.fmt`.
    ConvertWord,
    /// `DIV.fmt`.
    Divide,
    /// `FLOOR.L.fmt`.
    FloorLong,
    /// `FLOOR.W.fmt`.
    FloorWord,
    /// `MADD.fmt`.
    MultiplyAdd,
    /// `MOV.fmt`.
    Move,
    /// `MOVF.fmt`.
    MoveConditionalFalse,
    /// `MOVT.fmt`.
    MoveConditionalTrue,
    /// `MOVN.fmt`.
    MoveConditionalNonzero,
    /// `MOVZ.fmt`.
    MoveConditionalZero,
    /// `MSUB.fmt`.
    MultiplySubtract,
    /// `MUL.fmt`.
    Multiply,
    /// `NEG.fmt`.
    Negate,
    /// `NMADD.fmt`.
    NegativeMultiplyAdd,
    /// `NMSUB.fmt`.
    NegativeMultiplySubtract,
    /// `RECIP.fmt`.
    Reciprocal,
    /// `ROUND.L.fmt`.
    RoundLong,
    /// `ROUND.W.fmt`.
    RoundWord,
    /// `RSQRT.fmt`.
    ReciprocalSquareRoot,
    /// `SQRT.fmt`.
    SquareRoot,
    /// `SUB.fmt`.
    Subtract,
    /// `TRUNC.L.fmt`.
    TruncLong,
    /// `TRUNC.W.fmt`.
    TruncWord,
}

impl Mips4Cp1Operation {
    /// Returns whether the selected operand format is valid for this operation.
    pub const fn operand_format_status(
        self,
        format: Mips4Cp1Format,
    ) -> Mips4Cp1OperandFormatStatus {
        match self {
            Self::Absolute
            | Self::Add
            | Self::Compare(_)
            | Self::Divide
            | Self::MultiplyAdd
            | Self::MultiplySubtract
            | Self::Multiply
            | Self::Negate
            | Self::NegativeMultiplyAdd
            | Self::NegativeMultiplySubtract
            | Self::Reciprocal
            | Self::ReciprocalSquareRoot
            | Self::SquareRoot
            | Self::Subtract => float_valid_fixed_unimplemented(format),
            Self::Move
            | Self::MoveConditionalFalse
            | Self::MoveConditionalTrue
            | Self::MoveConditionalNonzero
            | Self::MoveConditionalZero
            | Self::CeilLong
            | Self::CeilWord
            | Self::ConvertLong
            | Self::ConvertWord
            | Self::FloorLong
            | Self::FloorWord
            | Self::RoundLong
            | Self::RoundWord
            | Self::TruncLong
            | Self::TruncWord => float_valid_fixed_invalid(format),
            Self::ConvertDouble => match format {
                Mips4Cp1Format::Single | Mips4Cp1Format::Word | Mips4Cp1Format::Long => {
                    Mips4Cp1OperandFormatStatus::Valid
                }
                Mips4Cp1Format::Double => Mips4Cp1OperandFormatStatus::Invalid,
            },
            Self::ConvertSingle => match format {
                Mips4Cp1Format::Double | Mips4Cp1Format::Word | Mips4Cp1Format::Long => {
                    Mips4Cp1OperandFormatStatus::Valid
                }
                Mips4Cp1Format::Single => Mips4Cp1OperandFormatStatus::Invalid,
            },
        }
    }
}

/// CP1 compare condition encoded by the COP1 function field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp1CompareCondition {
    /// `C.F`.
    False,
    /// `C.UN`.
    Unordered,
    /// `C.EQ`.
    Equal,
    /// `C.UEQ`.
    UnorderedOrEqual,
    /// `C.OLT`.
    OrderedLessThan,
    /// `C.ULT`.
    UnorderedLessThan,
    /// `C.OLE`.
    OrderedLessOrEqual,
    /// `C.ULE`.
    UnorderedLessOrEqual,
    /// `C.SF`.
    SignalingFalse,
    /// `C.NGLE`.
    NotGreaterLessOrEqual,
    /// `C.SEQ`.
    SignalingEqual,
    /// `C.NGL`.
    NotGreaterLess,
    /// `C.LT`.
    LessThan,
    /// `C.NGE`.
    NotGreaterOrEqual,
    /// `C.LE`.
    LessOrEqual,
    /// `C.NGT`.
    NotGreaterThan,
}

impl Mips4Cp1CompareCondition {
    /// Creates a compare condition from a COP1 function field.
    pub const fn from_function(function: u8) -> Option<Self> {
        match function {
            0x30 => Some(Self::False),
            0x31 => Some(Self::Unordered),
            0x32 => Some(Self::Equal),
            0x33 => Some(Self::UnorderedOrEqual),
            0x34 => Some(Self::OrderedLessThan),
            0x35 => Some(Self::UnorderedLessThan),
            0x36 => Some(Self::OrderedLessOrEqual),
            0x37 => Some(Self::UnorderedLessOrEqual),
            0x38 => Some(Self::SignalingFalse),
            0x39 => Some(Self::NotGreaterLessOrEqual),
            0x3a => Some(Self::SignalingEqual),
            0x3b => Some(Self::NotGreaterLess),
            0x3c => Some(Self::LessThan),
            0x3d => Some(Self::NotGreaterOrEqual),
            0x3e => Some(Self::LessOrEqual),
            0x3f => Some(Self::NotGreaterThan),
            _ => None,
        }
    }

    /// Returns the COP1 function field value for this compare condition.
    pub const fn function(self) -> u8 {
        match self {
            Self::False => 0x30,
            Self::Unordered => 0x31,
            Self::Equal => 0x32,
            Self::UnorderedOrEqual => 0x33,
            Self::OrderedLessThan => 0x34,
            Self::UnorderedLessThan => 0x35,
            Self::OrderedLessOrEqual => 0x36,
            Self::UnorderedLessOrEqual => 0x37,
            Self::SignalingFalse => 0x38,
            Self::NotGreaterLessOrEqual => 0x39,
            Self::SignalingEqual => 0x3a,
            Self::NotGreaterLess => 0x3b,
            Self::LessThan => 0x3c,
            Self::NotGreaterOrEqual => 0x3d,
            Self::LessOrEqual => 0x3e,
            Self::NotGreaterThan => 0x3f,
        }
    }
}

/// Classifies a raw instruction as a CP1-related instruction.
pub const fn decode_instruction(instruction: Mips4Instruction) -> Option<Mips4Cp1Decode> {
    match instruction.opcode() {
        MIPS4_COP1_OPCODE => Some(decode_cop1(instruction)),
        MIPS4_COP1X_OPCODE => Some(decode_cop1x(instruction)),
        MIPS4_LWC1_OPCODE => Some(instruction_class(Mips4Cp1InstructionClass::OffsetMemory(
            Mips4Cp1OffsetMemoryOperation::LoadWord,
        ))),
        MIPS4_LDC1_OPCODE => Some(instruction_class(Mips4Cp1InstructionClass::OffsetMemory(
            Mips4Cp1OffsetMemoryOperation::LoadDoubleword,
        ))),
        MIPS4_SWC1_OPCODE => Some(instruction_class(Mips4Cp1InstructionClass::OffsetMemory(
            Mips4Cp1OffsetMemoryOperation::StoreWord,
        ))),
        MIPS4_SDC1_OPCODE => Some(instruction_class(Mips4Cp1InstructionClass::OffsetMemory(
            Mips4Cp1OffsetMemoryOperation::StoreDoubleword,
        ))),
        MIPS4_SPECIAL_OPCODE => {
            if instruction.funct() == MIPS4_SPECIAL_MOVCI_FUNCTION {
                Some(instruction_class(Mips4Cp1InstructionClass::Movci(
                    decode_movci(instruction),
                )))
            } else {
                None
            }
        }
        _ => None,
    }
}

const fn decode_cop1(instruction: Mips4Instruction) -> Mips4Cp1Decode {
    match instruction.fmt() {
        CP1_FMT_MFC1 => instruction_class(Mips4Cp1InstructionClass::RegisterTransfer(
            Mips4Cp1RegisterTransferOperation::MoveWordFrom,
        )),
        CP1_FMT_DMFC1 => instruction_class(Mips4Cp1InstructionClass::RegisterTransfer(
            Mips4Cp1RegisterTransferOperation::MoveDoublewordFrom,
        )),
        CP1_FMT_CFC1 => instruction_class(Mips4Cp1InstructionClass::RegisterTransfer(
            Mips4Cp1RegisterTransferOperation::MoveControlFrom,
        )),
        CP1_FMT_MTC1 => instruction_class(Mips4Cp1InstructionClass::RegisterTransfer(
            Mips4Cp1RegisterTransferOperation::MoveWordTo,
        )),
        CP1_FMT_DMTC1 => instruction_class(Mips4Cp1InstructionClass::RegisterTransfer(
            Mips4Cp1RegisterTransferOperation::MoveDoublewordTo,
        )),
        CP1_FMT_CTC1 => instruction_class(Mips4Cp1InstructionClass::RegisterTransfer(
            Mips4Cp1RegisterTransferOperation::MoveControlTo,
        )),
        CP1_FMT_BRANCH => {
            instruction_class(Mips4Cp1InstructionClass::Branch(decode_branch(instruction)))
        }
        _ => match Mips4Cp1Format::from_fmt_field(instruction.fmt()) {
            Some(format) => decode_cop1_formatted(format, instruction),
            None => Mips4Cp1Decode::ReservedOrUnimplementedOperation,
        },
    }
}

const fn decode_branch(instruction: Mips4Instruction) -> Mips4Cp1BranchOperation {
    match instruction.rt() & 0x03 {
        0x00 => Mips4Cp1BranchOperation::BranchFalse,
        0x01 => Mips4Cp1BranchOperation::BranchTrue,
        0x02 => Mips4Cp1BranchOperation::BranchFalseLikely,
        _ => Mips4Cp1BranchOperation::BranchTrueLikely,
    }
}

const fn decode_cop1_formatted(
    format: Mips4Cp1Format,
    instruction: Mips4Instruction,
) -> Mips4Cp1Decode {
    match format {
        Mips4Cp1Format::Single | Mips4Cp1Format::Double => {
            decode_cop1_float_format(format, instruction)
        }
        Mips4Cp1Format::Word | Mips4Cp1Format::Long => {
            decode_cop1_fixed_format(format, instruction.funct())
        }
    }
}

const fn decode_cop1_float_format(
    format: Mips4Cp1Format,
    instruction: Mips4Instruction,
) -> Mips4Cp1Decode {
    let operation = match instruction.funct() {
        0x00 => Some(Mips4Cp1Operation::Add),
        0x01 => Some(Mips4Cp1Operation::Subtract),
        0x02 => Some(Mips4Cp1Operation::Multiply),
        0x03 => Some(Mips4Cp1Operation::Divide),
        0x04 => Some(Mips4Cp1Operation::SquareRoot),
        0x05 => Some(Mips4Cp1Operation::Absolute),
        0x06 => Some(Mips4Cp1Operation::Move),
        0x07 => Some(Mips4Cp1Operation::Negate),
        0x08 => Some(Mips4Cp1Operation::RoundLong),
        0x09 => Some(Mips4Cp1Operation::TruncLong),
        0x0a => Some(Mips4Cp1Operation::CeilLong),
        0x0b => Some(Mips4Cp1Operation::FloorLong),
        0x0c => Some(Mips4Cp1Operation::RoundWord),
        0x0d => Some(Mips4Cp1Operation::TruncWord),
        0x0e => Some(Mips4Cp1Operation::CeilWord),
        0x0f => Some(Mips4Cp1Operation::FloorWord),
        0x11 => Some(decode_movcf(instruction)),
        0x12 => Some(Mips4Cp1Operation::MoveConditionalZero),
        0x13 => Some(Mips4Cp1Operation::MoveConditionalNonzero),
        0x15 => Some(Mips4Cp1Operation::Reciprocal),
        0x16 => Some(Mips4Cp1Operation::ReciprocalSquareRoot),
        0x20 => match format {
            Mips4Cp1Format::Double => Some(Mips4Cp1Operation::ConvertSingle),
            _ => None,
        },
        0x21 => match format {
            Mips4Cp1Format::Single => Some(Mips4Cp1Operation::ConvertDouble),
            _ => None,
        },
        0x24 => Some(Mips4Cp1Operation::ConvertWord),
        0x25 => Some(Mips4Cp1Operation::ConvertLong),
        0x30..=0x3f => match Mips4Cp1CompareCondition::from_function(instruction.funct()) {
            Some(condition) => Some(Mips4Cp1Operation::Compare(condition)),
            None => None,
        },
        _ => None,
    };

    match operation {
        Some(operation) => formatted(operation, format),
        None => Mips4Cp1Decode::ReservedOrUnimplementedOperation,
    }
}

const fn decode_cop1_fixed_format(format: Mips4Cp1Format, function: u8) -> Mips4Cp1Decode {
    match function {
        0x20 => formatted(Mips4Cp1Operation::ConvertSingle, format),
        0x21 => formatted(Mips4Cp1Operation::ConvertDouble, format),
        _ => Mips4Cp1Decode::ReservedOrUnimplementedOperation,
    }
}

const fn decode_movcf(instruction: Mips4Instruction) -> Mips4Cp1Operation {
    if condition_true_bit(instruction) {
        Mips4Cp1Operation::MoveConditionalTrue
    } else {
        Mips4Cp1Operation::MoveConditionalFalse
    }
}

const fn decode_cop1x(instruction: Mips4Instruction) -> Mips4Cp1Decode {
    match instruction.funct() {
        0x00 => instruction_class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::LoadWordIndexed,
        )),
        0x01 => instruction_class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::LoadDoublewordIndexed,
        )),
        0x08 => instruction_class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::StoreWordIndexed,
        )),
        0x09 => instruction_class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::StoreDoublewordIndexed,
        )),
        0x0f => instruction_class(Mips4Cp1InstructionClass::IndexedPrefetch),
        0x20 => formatted(Mips4Cp1Operation::MultiplyAdd, Mips4Cp1Format::Single),
        0x21 => formatted(Mips4Cp1Operation::MultiplyAdd, Mips4Cp1Format::Double),
        0x28 => formatted(Mips4Cp1Operation::MultiplySubtract, Mips4Cp1Format::Single),
        0x29 => formatted(Mips4Cp1Operation::MultiplySubtract, Mips4Cp1Format::Double),
        0x30 => formatted(
            Mips4Cp1Operation::NegativeMultiplyAdd,
            Mips4Cp1Format::Single,
        ),
        0x31 => formatted(
            Mips4Cp1Operation::NegativeMultiplyAdd,
            Mips4Cp1Format::Double,
        ),
        0x38 => formatted(
            Mips4Cp1Operation::NegativeMultiplySubtract,
            Mips4Cp1Format::Single,
        ),
        0x39 => formatted(
            Mips4Cp1Operation::NegativeMultiplySubtract,
            Mips4Cp1Format::Double,
        ),
        _ => Mips4Cp1Decode::ReservedOrUnimplementedOperation,
    }
}

const fn decode_movci(instruction: Mips4Instruction) -> Mips4Cp1MovciOperation {
    if condition_true_bit(instruction) {
        Mips4Cp1MovciOperation::MoveTrue
    } else {
        Mips4Cp1MovciOperation::MoveFalse
    }
}

const fn condition_true_bit(instruction: Mips4Instruction) -> bool {
    (instruction.rt() & 0x01) != 0
}

const fn formatted(operation: Mips4Cp1Operation, format: Mips4Cp1Format) -> Mips4Cp1Decode {
    instruction_class(Mips4Cp1InstructionClass::Formatted { operation, format })
}

const fn instruction_class(instruction: Mips4Cp1InstructionClass) -> Mips4Cp1Decode {
    Mips4Cp1Decode::Instruction(instruction)
}

const fn float_valid_fixed_unimplemented(format: Mips4Cp1Format) -> Mips4Cp1OperandFormatStatus {
    match format {
        Mips4Cp1Format::Single | Mips4Cp1Format::Double => Mips4Cp1OperandFormatStatus::Valid,
        Mips4Cp1Format::Word | Mips4Cp1Format::Long => {
            Mips4Cp1OperandFormatStatus::UnimplementedOrReserved
        }
    }
}

const fn float_valid_fixed_invalid(format: Mips4Cp1Format) -> Mips4Cp1OperandFormatStatus {
    match format {
        Mips4Cp1Format::Single | Mips4Cp1Format::Double => Mips4Cp1OperandFormatStatus::Valid,
        Mips4Cp1Format::Word | Mips4Cp1Format::Long => Mips4Cp1OperandFormatStatus::Invalid,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cop1(fmt: u8, rt: u8, function: u8) -> Mips4Instruction {
        Mips4Instruction::from_bits(
            ((MIPS4_COP1_OPCODE as u32) << 26)
                | ((fmt as u32) << 21)
                | ((rt as u32) << 16)
                | (function as u32),
        )
    }

    fn cop1x(function: u8) -> Mips4Instruction {
        Mips4Instruction::from_bits(((MIPS4_COP1X_OPCODE as u32) << 26) | (function as u32))
    }

    fn special_movci(rt: u8) -> Mips4Instruction {
        Mips4Instruction::from_bits(((rt as u32) << 16) | (MIPS4_SPECIAL_MOVCI_FUNCTION as u32))
    }

    fn decode(instruction: Mips4Instruction) -> Mips4Cp1Decode {
        decode_instruction(instruction).unwrap()
    }

    fn class(instruction: Mips4Cp1InstructionClass) -> Mips4Cp1Decode {
        Mips4Cp1Decode::Instruction(instruction)
    }

    fn formatted_class(operation: Mips4Cp1Operation, format: Mips4Cp1Format) -> Mips4Cp1Decode {
        class(Mips4Cp1InstructionClass::Formatted { operation, format })
    }

    #[test]
    fn cp1_fmt_table_classifies_register_transfer_branch_and_format_classes() {
        let cases = [
            (
                CP1_FMT_MFC1,
                0,
                class(Mips4Cp1InstructionClass::RegisterTransfer(
                    Mips4Cp1RegisterTransferOperation::MoveWordFrom,
                )),
            ),
            (
                CP1_FMT_DMFC1,
                0,
                class(Mips4Cp1InstructionClass::RegisterTransfer(
                    Mips4Cp1RegisterTransferOperation::MoveDoublewordFrom,
                )),
            ),
            (
                CP1_FMT_CFC1,
                0,
                class(Mips4Cp1InstructionClass::RegisterTransfer(
                    Mips4Cp1RegisterTransferOperation::MoveControlFrom,
                )),
            ),
            (
                CP1_FMT_MTC1,
                0,
                class(Mips4Cp1InstructionClass::RegisterTransfer(
                    Mips4Cp1RegisterTransferOperation::MoveWordTo,
                )),
            ),
            (
                CP1_FMT_DMTC1,
                0,
                class(Mips4Cp1InstructionClass::RegisterTransfer(
                    Mips4Cp1RegisterTransferOperation::MoveDoublewordTo,
                )),
            ),
            (
                CP1_FMT_CTC1,
                0,
                class(Mips4Cp1InstructionClass::RegisterTransfer(
                    Mips4Cp1RegisterTransferOperation::MoveControlTo,
                )),
            ),
            (
                CP1_FMT_BRANCH,
                0,
                class(Mips4Cp1InstructionClass::Branch(
                    Mips4Cp1BranchOperation::BranchFalse,
                )),
            ),
            (
                Mips4Cp1Format::Single.fmt_field(),
                0,
                formatted_class(Mips4Cp1Operation::Add, Mips4Cp1Format::Single),
            ),
            (
                Mips4Cp1Format::Double.fmt_field(),
                0,
                formatted_class(Mips4Cp1Operation::Add, Mips4Cp1Format::Double),
            ),
            (
                Mips4Cp1Format::Word.fmt_field(),
                0x20,
                formatted_class(Mips4Cp1Operation::ConvertSingle, Mips4Cp1Format::Word),
            ),
            (
                Mips4Cp1Format::Long.fmt_field(),
                0x20,
                formatted_class(Mips4Cp1Operation::ConvertSingle, Mips4Cp1Format::Long),
            ),
        ];

        for (fmt, function, expected) in cases {
            assert_eq!(decode(cop1(fmt, 0, function)), expected, "fmt {fmt:#04x}");
        }

        for fmt in [0x03, 0x07, 0x09, 0x0f, 0x12, 0x13, 0x16, 0x17, 0x1f] {
            assert_eq!(
                decode(cop1(fmt, 0, 0)),
                Mips4Cp1Decode::ReservedOrUnimplementedOperation,
                "reserved fmt {fmt:#04x}"
            );
        }
    }

    #[test]
    fn cp1_offset_load_store_opcodes_decode_directly() {
        let cases = [
            (MIPS4_LWC1_OPCODE, Mips4Cp1OffsetMemoryOperation::LoadWord),
            (
                MIPS4_LDC1_OPCODE,
                Mips4Cp1OffsetMemoryOperation::LoadDoubleword,
            ),
            (MIPS4_SWC1_OPCODE, Mips4Cp1OffsetMemoryOperation::StoreWord),
            (
                MIPS4_SDC1_OPCODE,
                Mips4Cp1OffsetMemoryOperation::StoreDoubleword,
            ),
        ];

        for (opcode, operation) in cases {
            let instruction = Mips4Instruction::from_bits((opcode as u32) << 26);
            assert_eq!(
                decode(instruction),
                class(Mips4Cp1InstructionClass::OffsetMemory(operation))
            );
        }
    }

    #[test]
    fn cp1_branch_table_uses_nd_and_tf_bits() {
        let cases = [
            (0b00, Mips4Cp1BranchOperation::BranchFalse),
            (0b01, Mips4Cp1BranchOperation::BranchTrue),
            (0b10, Mips4Cp1BranchOperation::BranchFalseLikely),
            (0b11, Mips4Cp1BranchOperation::BranchTrueLikely),
        ];

        for (rt_bits, operation) in cases {
            let rt = 0b10100 | rt_bits;
            assert_eq!(
                decode(cop1(CP1_FMT_BRANCH, rt, 0)),
                class(Mips4Cp1InstructionClass::Branch(operation))
            );
        }
    }

    #[test]
    fn cp1_float_function_tables_decode_s_and_d_operations() {
        let mut single_expected = [Mips4Cp1Decode::ReservedOrUnimplementedOperation; 64];
        let mut double_expected = [Mips4Cp1Decode::ReservedOrUnimplementedOperation; 64];

        for (function, operation) in [
            (0x00, Mips4Cp1Operation::Add),
            (0x01, Mips4Cp1Operation::Subtract),
            (0x02, Mips4Cp1Operation::Multiply),
            (0x03, Mips4Cp1Operation::Divide),
            (0x04, Mips4Cp1Operation::SquareRoot),
            (0x05, Mips4Cp1Operation::Absolute),
            (0x06, Mips4Cp1Operation::Move),
            (0x07, Mips4Cp1Operation::Negate),
            (0x08, Mips4Cp1Operation::RoundLong),
            (0x09, Mips4Cp1Operation::TruncLong),
            (0x0a, Mips4Cp1Operation::CeilLong),
            (0x0b, Mips4Cp1Operation::FloorLong),
            (0x0c, Mips4Cp1Operation::RoundWord),
            (0x0d, Mips4Cp1Operation::TruncWord),
            (0x0e, Mips4Cp1Operation::CeilWord),
            (0x0f, Mips4Cp1Operation::FloorWord),
            (0x11, Mips4Cp1Operation::MoveConditionalFalse),
            (0x12, Mips4Cp1Operation::MoveConditionalZero),
            (0x13, Mips4Cp1Operation::MoveConditionalNonzero),
            (0x15, Mips4Cp1Operation::Reciprocal),
            (0x16, Mips4Cp1Operation::ReciprocalSquareRoot),
            (0x24, Mips4Cp1Operation::ConvertWord),
            (0x25, Mips4Cp1Operation::ConvertLong),
        ] {
            single_expected[function] = formatted_class(operation, Mips4Cp1Format::Single);
            double_expected[function] = formatted_class(operation, Mips4Cp1Format::Double);
        }

        single_expected[0x21] =
            formatted_class(Mips4Cp1Operation::ConvertDouble, Mips4Cp1Format::Single);
        double_expected[0x20] =
            formatted_class(Mips4Cp1Operation::ConvertSingle, Mips4Cp1Format::Double);

        for function in 0x30..=0x3f {
            let condition = Mips4Cp1CompareCondition::from_function(function as u8).unwrap();
            single_expected[function] = formatted_class(
                Mips4Cp1Operation::Compare(condition),
                Mips4Cp1Format::Single,
            );
            double_expected[function] = formatted_class(
                Mips4Cp1Operation::Compare(condition),
                Mips4Cp1Format::Double,
            );
        }

        for function in 0..64 {
            assert_eq!(
                decode(cop1(Mips4Cp1Format::Single.fmt_field(), 0, function as u8)),
                single_expected[function],
                "S function {function:#04x}"
            );
            assert_eq!(
                decode(cop1(Mips4Cp1Format::Double.fmt_field(), 0, function as u8)),
                double_expected[function],
                "D function {function:#04x}"
            );
        }

        assert_eq!(
            decode(cop1(Mips4Cp1Format::Single.fmt_field(), 1, 0x11)),
            formatted_class(
                Mips4Cp1Operation::MoveConditionalTrue,
                Mips4Cp1Format::Single
            )
        );
    }

    #[test]
    fn cp1_fixed_function_tables_decode_only_float_conversions() {
        let mut expected_word = [Mips4Cp1Decode::ReservedOrUnimplementedOperation; 64];
        let mut expected_long = [Mips4Cp1Decode::ReservedOrUnimplementedOperation; 64];
        expected_word[0x20] =
            formatted_class(Mips4Cp1Operation::ConvertSingle, Mips4Cp1Format::Word);
        expected_word[0x21] =
            formatted_class(Mips4Cp1Operation::ConvertDouble, Mips4Cp1Format::Word);
        expected_long[0x20] =
            formatted_class(Mips4Cp1Operation::ConvertSingle, Mips4Cp1Format::Long);
        expected_long[0x21] =
            formatted_class(Mips4Cp1Operation::ConvertDouble, Mips4Cp1Format::Long);

        for function in 0..64 {
            assert_eq!(
                decode(cop1(Mips4Cp1Format::Word.fmt_field(), 0, function as u8)),
                expected_word[function],
                "W function {function:#04x}"
            );
            assert_eq!(
                decode(cop1(Mips4Cp1Format::Long.fmt_field(), 0, function as u8)),
                expected_long[function],
                "L function {function:#04x}"
            );
        }
    }

    #[test]
    fn compare_conditions_round_trip_function_values() {
        for function in 0x30..=0x3f {
            let condition = Mips4Cp1CompareCondition::from_function(function).unwrap();
            assert_eq!(condition.function(), function);
        }

        assert_eq!(Mips4Cp1CompareCondition::from_function(0x2f), None);
    }

    #[test]
    fn cop1x_function_table_classifies_indexed_and_fused_operations() {
        let mut expected = [Mips4Cp1Decode::ReservedOrUnimplementedOperation; 64];
        expected[0x00] = class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::LoadWordIndexed,
        ));
        expected[0x01] = class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::LoadDoublewordIndexed,
        ));
        expected[0x08] = class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::StoreWordIndexed,
        ));
        expected[0x09] = class(Mips4Cp1InstructionClass::IndexedMemory(
            Mips4Cp1IndexedMemoryOperation::StoreDoublewordIndexed,
        ));
        expected[0x0f] = class(Mips4Cp1InstructionClass::IndexedPrefetch);
        expected[0x20] = formatted_class(Mips4Cp1Operation::MultiplyAdd, Mips4Cp1Format::Single);
        expected[0x21] = formatted_class(Mips4Cp1Operation::MultiplyAdd, Mips4Cp1Format::Double);
        expected[0x28] =
            formatted_class(Mips4Cp1Operation::MultiplySubtract, Mips4Cp1Format::Single);
        expected[0x29] =
            formatted_class(Mips4Cp1Operation::MultiplySubtract, Mips4Cp1Format::Double);
        expected[0x30] = formatted_class(
            Mips4Cp1Operation::NegativeMultiplyAdd,
            Mips4Cp1Format::Single,
        );
        expected[0x31] = formatted_class(
            Mips4Cp1Operation::NegativeMultiplyAdd,
            Mips4Cp1Format::Double,
        );
        expected[0x38] = formatted_class(
            Mips4Cp1Operation::NegativeMultiplySubtract,
            Mips4Cp1Format::Single,
        );
        expected[0x39] = formatted_class(
            Mips4Cp1Operation::NegativeMultiplySubtract,
            Mips4Cp1Format::Double,
        );

        for (function, expected_decode) in expected.iter().enumerate() {
            assert_eq!(
                decode(cop1x(function as u8)),
                *expected_decode,
                "COP1X function {function:#04x}"
            );
        }
    }

    #[test]
    fn special_movci_decodes_without_cp1_primary_wrapper() {
        assert_eq!(
            decode(special_movci(0)),
            class(Mips4Cp1InstructionClass::Movci(
                Mips4Cp1MovciOperation::MoveFalse
            ))
        );
        assert_eq!(
            decode(special_movci(1)),
            class(Mips4Cp1InstructionClass::Movci(
                Mips4Cp1MovciOperation::MoveTrue
            ))
        );

        assert_eq!(decode_instruction(Mips4Instruction::from_bits(0)), None);
    }

    #[test]
    fn operand_format_status_matches_b7_matrix() {
        let valid = Mips4Cp1OperandFormatStatus::Valid;
        let unimplemented = Mips4Cp1OperandFormatStatus::UnimplementedOrReserved;
        let invalid = Mips4Cp1OperandFormatStatus::Invalid;

        for operation in [
            Mips4Cp1Operation::Absolute,
            Mips4Cp1Operation::Add,
            Mips4Cp1Operation::Compare(Mips4Cp1CompareCondition::Equal),
            Mips4Cp1Operation::Divide,
            Mips4Cp1Operation::MultiplyAdd,
            Mips4Cp1Operation::MultiplySubtract,
            Mips4Cp1Operation::Multiply,
            Mips4Cp1Operation::Negate,
            Mips4Cp1Operation::NegativeMultiplyAdd,
            Mips4Cp1Operation::NegativeMultiplySubtract,
            Mips4Cp1Operation::Reciprocal,
            Mips4Cp1Operation::ReciprocalSquareRoot,
            Mips4Cp1Operation::SquareRoot,
            Mips4Cp1Operation::Subtract,
        ] {
            assert_format_statuses(operation, [valid, valid, unimplemented, unimplemented]);
        }

        for operation in [
            Mips4Cp1Operation::CeilLong,
            Mips4Cp1Operation::CeilWord,
            Mips4Cp1Operation::ConvertLong,
            Mips4Cp1Operation::ConvertWord,
            Mips4Cp1Operation::FloorLong,
            Mips4Cp1Operation::FloorWord,
            Mips4Cp1Operation::Move,
            Mips4Cp1Operation::MoveConditionalFalse,
            Mips4Cp1Operation::MoveConditionalTrue,
            Mips4Cp1Operation::MoveConditionalNonzero,
            Mips4Cp1Operation::MoveConditionalZero,
            Mips4Cp1Operation::RoundLong,
            Mips4Cp1Operation::RoundWord,
            Mips4Cp1Operation::TruncLong,
            Mips4Cp1Operation::TruncWord,
        ] {
            assert_format_statuses(operation, [valid, valid, invalid, invalid]);
        }

        assert_format_statuses(
            Mips4Cp1Operation::ConvertDouble,
            [valid, invalid, valid, valid],
        );
        assert_format_statuses(
            Mips4Cp1Operation::ConvertSingle,
            [invalid, valid, valid, valid],
        );
    }

    fn assert_format_statuses(
        operation: Mips4Cp1Operation,
        expected: [Mips4Cp1OperandFormatStatus; 4],
    ) {
        let formats = [
            Mips4Cp1Format::Single,
            Mips4Cp1Format::Double,
            Mips4Cp1Format::Word,
            Mips4Cp1Format::Long,
        ];

        for (format, status) in formats.into_iter().zip(expected) {
            assert_eq!(
                operation.operand_format_status(format),
                status,
                "{operation:?} {format:?}"
            );
        }
    }
}
