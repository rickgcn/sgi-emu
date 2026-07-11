//! MIPS architecture-level requirements for decoded instructions.

use crate::cpu::mips4::cp1::Mips4Cp1Format;
use crate::cpu::mips4::cp1::decode::{
    Mips4Cp1BranchOperation, Mips4Cp1InstructionClass, Mips4Cp1OffsetMemoryOperation,
    Mips4Cp1Operation, Mips4Cp1RegisterTransferOperation,
};

use super::Mips4Instruction;
use super::decode::Mips4CpuInstruction;

/// Architecture level in which an instruction was introduced.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4ArchitectureLevel {
    /// MIPS I.
    Mips1,
    /// MIPS II.
    Mips2,
    /// MIPS III.
    Mips3,
    /// MIPS IV.
    Mips4,
}

/// Architectural result when an instruction's ISA level is disabled.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4DisabledInstructionAction {
    /// Raise a Reserved Instruction exception.
    ReservedInstruction,
    /// Record and raise a CP1 unimplemented-operation exception.
    FloatingPointUnimplemented,
}

/// Static architecture requirements of one decoded instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4InstructionRequirements {
    /// Architecture level in which the instruction was introduced.
    pub architecture_level: Mips4ArchitectureLevel,

    /// Result when the current mode disables the architecture level.
    pub disabled_action: Mips4DisabledInstructionAction,
}

impl Mips4InstructionRequirements {
    /// Creates requirements for an architecture level.
    pub const fn new(architecture_level: Mips4ArchitectureLevel) -> Self {
        Self {
            architecture_level,
            disabled_action: Mips4DisabledInstructionAction::ReservedInstruction,
        }
    }

    /// Creates requirements with an explicit disabled-instruction result.
    pub const fn with_disabled_action(
        architecture_level: Mips4ArchitectureLevel,
        disabled_action: Mips4DisabledInstructionAction,
    ) -> Self {
        Self {
            architecture_level,
            disabled_action,
        }
    }
}

/// Returns the architecture requirements for a CPU instruction.
pub const fn cpu_requirements(instruction: Mips4CpuInstruction) -> Mips4InstructionRequirements {
    use Mips4ArchitectureLevel::{Mips1, Mips2, Mips3, Mips4};
    use Mips4CpuInstruction::*;

    let level = match instruction {
        Beql | Bnel | Bgezl | Bgezall | Bgtzl | Blezl | Bltzl | Bltzall | Ll | Sc | Sync | Teq
        | Teqi | Tge | Tgei | Tgeiu | Tgeu | Tlt | Tlti | Tltiu | Tltu | Tne | Tnei => Mips2,
        Dadd | Daddi | Daddiu | Daddu | Ddiv | Ddivu | Dmult | Dmultu | Dsll | Dsll32 | Dsllv
        | Dsra | Dsra32 | Dsrav | Dsrl | Dsrl32 | Dsrlv | Dsub | Dsubu | Ld | Ldl | Ldr | Lld
        | Lwu | Scd | Sd | Sdl | Sdr => Mips3,
        Movn | Movz | Pref => Mips4,
        Add | Addi | Addiu | Addu | And | Andi | Beq | Bgez | Bgezal | Bgtz | Blez | Bltz
        | Bltzal | Bne | Break | Div | Divu | J | Jal | Jalr | Jr | Lb | Lbu | Lh | Lhu | Lui
        | Lw | Lwl | Lwr | Mfhi | Mflo | Mthi | Mtlo | Mult | Multu | Nor | Or | Ori | Sb | Sh
        | Sll | Sllv | Slt | Slti | Sltiu | Sltu | Sra | Srav | Srl | Srlv | Sub | Subu | Sw
        | Swl | Swr | Syscall | Xor | Xori => Mips1,
    };
    Mips4InstructionRequirements::new(level)
}

/// Returns the architecture requirements for a detailed CP1 instruction.
pub const fn cp1_requirements(
    raw: Mips4Instruction,
    class: Mips4Cp1InstructionClass,
) -> Mips4InstructionRequirements {
    use Mips4ArchitectureLevel::{Mips1, Mips2, Mips3, Mips4};

    let level = match class {
        Mips4Cp1InstructionClass::RegisterTransfer(operation) => match operation {
            Mips4Cp1RegisterTransferOperation::MoveDoublewordFrom
            | Mips4Cp1RegisterTransferOperation::MoveDoublewordTo => Mips3,
            _ => Mips1,
        },
        Mips4Cp1InstructionClass::Branch(operation) => {
            if raw.rt() >> 2 != 0 {
                Mips4
            } else if matches!(
                operation,
                Mips4Cp1BranchOperation::BranchFalseLikely
                    | Mips4Cp1BranchOperation::BranchTrueLikely
            ) {
                Mips2
            } else {
                Mips1
            }
        }
        Mips4Cp1InstructionClass::Formatted { operation, format } => {
            formatted_level(raw, operation, format)
        }
        Mips4Cp1InstructionClass::IndexedMemory(_)
        | Mips4Cp1InstructionClass::IndexedPrefetch
        | Mips4Cp1InstructionClass::Movci(_) => Mips4,
        Mips4Cp1InstructionClass::OffsetMemory(
            Mips4Cp1OffsetMemoryOperation::LoadDoubleword
            | Mips4Cp1OffsetMemoryOperation::StoreDoubleword,
        ) => Mips2,
        Mips4Cp1InstructionClass::OffsetMemory(
            Mips4Cp1OffsetMemoryOperation::LoadWord | Mips4Cp1OffsetMemoryOperation::StoreWord,
        ) => Mips1,
    };
    let disabled_action = if matches!(
        (class, level),
        (
            Mips4Cp1InstructionClass::Formatted { .. },
            Mips4ArchitectureLevel::Mips4
        )
    ) {
        Mips4DisabledInstructionAction::FloatingPointUnimplemented
    } else {
        Mips4DisabledInstructionAction::ReservedInstruction
    };
    Mips4InstructionRequirements::with_disabled_action(level, disabled_action)
}

const fn formatted_level(
    raw: Mips4Instruction,
    operation: Mips4Cp1Operation,
    format: Mips4Cp1Format,
) -> Mips4ArchitectureLevel {
    use Mips4ArchitectureLevel::{Mips1, Mips3, Mips4};
    use Mips4Cp1Operation::*;

    match operation {
        MultiplyAdd
        | MultiplySubtract
        | NegativeMultiplyAdd
        | NegativeMultiplySubtract
        | MoveConditionalFalse
        | MoveConditionalTrue
        | MoveConditionalNonzero
        | MoveConditionalZero
        | Reciprocal
        | ReciprocalSquareRoot => Mips4,
        Compare(_) if raw.fd() >> 2 != 0 => Mips4,
        CeilLong | ConvertLong | FloorLong | RoundLong | TruncLong => Mips3,
        ConvertSingle | ConvertDouble if matches!(format, Mips4Cp1Format::Long) => Mips3,
        _ => Mips1,
    }
}

/// Returns the architecture requirements for a decoded CP0 instruction.
pub const fn cp0_requirements(raw: Mips4Instruction) -> Mips4InstructionRequirements {
    let level = match (raw.rs(), raw.funct()) {
        (0x01 | 0x05, _) | (0x10, 0x18 | 0x20) => Mips4ArchitectureLevel::Mips3,
        _ => Mips4ArchitectureLevel::Mips1,
    };
    Mips4InstructionRequirements::new(level)
}

/// Returns the requirements for the processor-specific CP0 base+offset opcode.
pub const fn cp0_offset_requirements() -> Mips4InstructionRequirements {
    Mips4InstructionRequirements::new(Mips4ArchitectureLevel::Mips3)
}

/// Returns the architecture requirements for a non-CP1 coprocessor encoding.
pub const fn coprocessor_requirements(raw: Mips4Instruction) -> Mips4InstructionRequirements {
    let level = match raw.opcode() {
        0x36 | 0x3e => Mips4ArchitectureLevel::Mips2,
        _ => Mips4ArchitectureLevel::Mips1,
    };
    Mips4InstructionRequirements::new(level)
}

#[cfg(test)]
mod tests {
    use crate::cpu::mips4::cp1::decode::{
        Mips4Cp1Decode, Mips4Cp1InstructionClass, decode_instruction as decode_cp1,
    };
    use crate::cpu::mips4::instruction::decode::{
        Mips4InstructionClass, Mips4InstructionDecode, decode_instruction as decode_mips4,
    };

    use super::*;

    fn cp1_requirements_for(bits: u32) -> Mips4InstructionRequirements {
        let raw = Mips4Instruction::from_bits(bits);
        let Some(Mips4Cp1Decode::Instruction(class)) = decode_cp1(raw) else {
            panic!("expected CP1 instruction for {bits:#010x}");
        };
        cp1_requirements(raw, class)
    }

    fn expected_cp1_level(
        raw: Mips4Instruction,
        class: Mips4Cp1InstructionClass,
    ) -> Mips4ArchitectureLevel {
        use Mips4ArchitectureLevel::{Mips1, Mips2, Mips3, Mips4};
        use Mips4Cp1Operation::*;

        if raw.opcode() == 0x13 {
            return Mips4;
        }
        match class {
            Mips4Cp1InstructionClass::OffsetMemory(
                Mips4Cp1OffsetMemoryOperation::LoadDoubleword
                | Mips4Cp1OffsetMemoryOperation::StoreDoubleword,
            ) => Mips2,
            Mips4Cp1InstructionClass::OffsetMemory(_) => Mips1,
            Mips4Cp1InstructionClass::RegisterTransfer(
                Mips4Cp1RegisterTransferOperation::MoveDoublewordFrom
                | Mips4Cp1RegisterTransferOperation::MoveDoublewordTo,
            ) => Mips3,
            Mips4Cp1InstructionClass::RegisterTransfer(_) => Mips1,
            Mips4Cp1InstructionClass::Branch(operation) => {
                if raw.rt() >> 2 != 0 {
                    Mips4
                } else if matches!(
                    operation,
                    Mips4Cp1BranchOperation::BranchFalseLikely
                        | Mips4Cp1BranchOperation::BranchTrueLikely
                ) {
                    Mips2
                } else {
                    Mips1
                }
            }
            Mips4Cp1InstructionClass::Formatted { operation, format } => match operation {
                MoveConditionalFalse
                | MoveConditionalTrue
                | MoveConditionalNonzero
                | MoveConditionalZero
                | Reciprocal
                | ReciprocalSquareRoot => Mips4,
                Compare(_) if raw.fd() >> 2 != 0 => Mips4,
                CeilLong | ConvertLong | FloorLong | RoundLong | TruncLong => Mips3,
                ConvertSingle | ConvertDouble if matches!(format, Mips4Cp1Format::Long) => Mips3,
                _ => Mips1,
            },
            Mips4Cp1InstructionClass::IndexedMemory(_)
            | Mips4Cp1InstructionClass::IndexedPrefetch
            | Mips4Cp1InstructionClass::Movci(_) => Mips4,
        }
    }

    fn assert_cp1_classification(raw: Mips4Instruction) -> bool {
        let Some(Mips4Cp1Decode::Instruction(class)) = decode_cp1(raw) else {
            return false;
        };
        let requirements = cp1_requirements(raw, class);
        assert_eq!(
            requirements.architecture_level,
            expected_cp1_level(raw, class),
            "unexpected CP1 architecture level for {:#010x}",
            raw.bits()
        );
        let expected_action = if matches!(
            (class, requirements.architecture_level),
            (
                Mips4Cp1InstructionClass::Formatted { .. },
                Mips4ArchitectureLevel::Mips4
            )
        ) {
            Mips4DisabledInstructionAction::FloatingPointUnimplemented
        } else {
            Mips4DisabledInstructionAction::ReservedInstruction
        };
        assert_eq!(requirements.disabled_action, expected_action);
        true
    }

    #[test]
    fn cpu_requirements_cover_each_architecture_generation() {
        assert_eq!(
            cpu_requirements(Mips4CpuInstruction::Add).architecture_level,
            Mips4ArchitectureLevel::Mips1
        );
        assert_eq!(
            cpu_requirements(Mips4CpuInstruction::Beql).architecture_level,
            Mips4ArchitectureLevel::Mips2
        );
        assert_eq!(
            cpu_requirements(Mips4CpuInstruction::Dadd).architecture_level,
            Mips4ArchitectureLevel::Mips3
        );
        assert_eq!(
            cpu_requirements(Mips4CpuInstruction::Movn).architecture_level,
            Mips4ArchitectureLevel::Mips4
        );
    }

    #[test]
    fn cpu_requirements_exhaustively_classify_all_112_instructions() {
        let mut instructions = Vec::new();
        for opcode in 0_u32..64 {
            for rt in 0_u32..32 {
                for function in 0_u32..64 {
                    let raw = Mips4Instruction::from_bits((opcode << 26) | (rt << 16) | function);
                    if let Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(
                        instruction,
                    )) = decode_mips4(raw)
                        && !instructions.contains(&instruction)
                    {
                        instructions.push(instruction);
                    }
                }
            }
        }

        let mut counts = [0_usize; 4];
        for instruction in instructions.iter().copied() {
            let index = match cpu_requirements(instruction).architecture_level {
                Mips4ArchitectureLevel::Mips1 => 0,
                Mips4ArchitectureLevel::Mips2 => 1,
                Mips4ArchitectureLevel::Mips3 => 2,
                Mips4ArchitectureLevel::Mips4 => 3,
            };
            counts[index] += 1;
        }
        assert_eq!(instructions.len(), 112);
        assert_eq!(counts, [58, 23, 28, 3]);
    }

    #[test]
    fn cp1_requirements_use_operation_format_and_condition_code() {
        let add_s = (0x11_u32 << 26) | (0x10 << 21);
        let dmfc1 = (0x11_u32 << 26) | (0x01 << 21);
        let recip_s = add_s | 0x15;
        let bc1_cc1 = (0x11_u32 << 26) | (0x08 << 21) | (4 << 16);
        let compare_cc1 = add_s | (4 << 6) | 0x32;
        let cop1x = (0x13_u32 << 26) | 0x20;
        let lwc1 = 0x31_u32 << 26;
        let ldc1 = 0x35_u32 << 26;

        assert_eq!(
            cp1_requirements_for(add_s).architecture_level,
            Mips4ArchitectureLevel::Mips1
        );
        assert_eq!(
            cp1_requirements_for(dmfc1).architecture_level,
            Mips4ArchitectureLevel::Mips3
        );
        assert_eq!(
            cp1_requirements_for(lwc1).architecture_level,
            Mips4ArchitectureLevel::Mips1
        );
        assert_eq!(
            cp1_requirements_for(ldc1).architecture_level,
            Mips4ArchitectureLevel::Mips2
        );
        for bits in [recip_s, bc1_cc1, compare_cc1, cop1x] {
            assert_eq!(
                cp1_requirements_for(bits).architecture_level,
                Mips4ArchitectureLevel::Mips4
            );
        }
    }

    #[test]
    fn cp1_requirements_exhaustively_classify_all_valid_forms() {
        let mut valid_forms = 0;

        for opcode in [0x31_u32, 0x35, 0x39, 0x3d] {
            valid_forms += usize::from(assert_cp1_classification(Mips4Instruction::from_bits(
                opcode << 26,
            )));
        }
        for rt in 0_u32..32 {
            valid_forms += usize::from(assert_cp1_classification(Mips4Instruction::from_bits(
                (rt << 16) | 0x01,
            )));
        }
        for format in [0_u32, 1, 2, 4, 5, 6] {
            valid_forms += usize::from(assert_cp1_classification(Mips4Instruction::from_bits(
                (0x11 << 26) | (format << 21),
            )));
        }
        for rt in 0_u32..32 {
            valid_forms += usize::from(assert_cp1_classification(Mips4Instruction::from_bits(
                (0x11 << 26) | (0x08 << 21) | (rt << 16),
            )));
        }
        for format in [0x10_u32, 0x11, 0x14, 0x15] {
            for condition_code_field in 0_u32..32 {
                for rt in 0_u32..2 {
                    for function in 0_u32..64 {
                        valid_forms +=
                            usize::from(assert_cp1_classification(Mips4Instruction::from_bits(
                                (0x11 << 26)
                                    | (format << 21)
                                    | (rt << 16)
                                    | (condition_code_field << 6)
                                    | function,
                            )));
                    }
                }
            }
        }
        for function in 0_u32..64 {
            let raw = Mips4Instruction::from_bits((0x13 << 26) | function);
            if assert_cp1_classification(raw) {
                assert_eq!(
                    cp1_requirements_for(raw.bits()).architecture_level,
                    Mips4ArchitectureLevel::Mips4
                );
                valid_forms += 1;
            }
        }

        assert!(valid_forms > 1_000);
    }

    #[test]
    fn cp2_memory_requirements_distinguish_word_and_doubleword_transfers() {
        assert_eq!(
            coprocessor_requirements(Mips4Instruction::from_bits(0x32_u32 << 26))
                .architecture_level,
            Mips4ArchitectureLevel::Mips1
        );
        assert_eq!(
            coprocessor_requirements(Mips4Instruction::from_bits(0x36_u32 << 26))
                .architecture_level,
            Mips4ArchitectureLevel::Mips2
        );
    }

    #[test]
    fn cp1_class_match_remains_exhaustive() {
        let representatives = [
            (0x31_u32 << 26),
            (0x11_u32 << 26),
            (0x11_u32 << 26) | (0x08 << 21),
            (0x11_u32 << 26) | (0x10 << 21),
            (0x13_u32 << 26),
            (0x13_u32 << 26) | 0x0f,
            0x01,
        ];
        for bits in representatives {
            let raw = Mips4Instruction::from_bits(bits);
            let Some(Mips4Cp1Decode::Instruction(class)) = decode_cp1(raw) else {
                panic!("expected CP1 class for {bits:#010x}");
            };
            match class {
                Mips4Cp1InstructionClass::OffsetMemory(_)
                | Mips4Cp1InstructionClass::RegisterTransfer(_)
                | Mips4Cp1InstructionClass::Branch(_)
                | Mips4Cp1InstructionClass::Formatted { .. }
                | Mips4Cp1InstructionClass::IndexedMemory(_)
                | Mips4Cp1InstructionClass::IndexedPrefetch
                | Mips4Cp1InstructionClass::Movci(_) => {
                    let _ = cp1_requirements(raw, class);
                }
            }
        }
    }
}
