//! Generic MIPS IV instruction encoding classification.
//!
//! This module classifies MIPS IV instruction words using the ISA encoding
//! tables. It does not execute instructions, read architectural state, or apply
//! processor-specific COP0 behavior.

use crate::cpu::mips4::exception::Mips4CoprocessorNumber;
use crate::cpu::mips4::instruction::Mips4Instruction;

const MIPS4_SPECIAL_OPCODE: u8 = 0x00;
const MIPS4_REGIMM_OPCODE: u8 = 0x01;

/// Top-level decode result for a MIPS IV instruction word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4InstructionDecode {
    /// The instruction is a defined MIPS IV instruction or instruction class.
    Instruction(Mips4InstructionClass),

    /// The encoding is reserved and causes a Reserved Instruction exception.
    ReservedInstruction,

    /// The encoding is reserved and produces an undefined result.
    UndefinedResult,

    /// The encoding is reserved for processor-specific COP0 base+offset uses.
    ProcessorSpecificCp0Offset,
}

impl Mips4InstructionDecode {
    /// Returns the coprocessor required by this instruction, if one is implied.
    pub const fn required_coprocessor(self) -> Option<Mips4CoprocessorNumber> {
        match self {
            Self::Instruction(instruction) => instruction.required_coprocessor(),
            Self::ProcessorSpecificCp0Offset => Some(Mips4CoprocessorNumber::Cp0),
            Self::ReservedInstruction | Self::UndefinedResult => None,
        }
    }
}

/// MIPS IV instruction class after top-level decode.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4InstructionClass {
    /// A user-level CPU instruction.
    Cpu(Mips4CpuInstruction),

    /// A CP1 instruction or CP1 instruction class.
    Fpu(Mips4FpuInstructionClass),

    /// A non-CP1 coprocessor instruction class or load/store opcode.
    Coprocessor(Mips4CoprocessorNumber),
}

impl Mips4InstructionClass {
    /// Returns the coprocessor required by this class, if one is implied.
    pub const fn required_coprocessor(self) -> Option<Mips4CoprocessorNumber> {
        match self {
            Self::Cpu(_) => None,
            Self::Fpu(_) => Some(Mips4CoprocessorNumber::Cp1),
            Self::Coprocessor(coprocessor) => Some(coprocessor),
        }
    }
}

/// User-level CPU instructions defined by the MIPS IV CPU encoding tables.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4CpuInstruction {
    /// `ADD`.
    Add,
    /// `ADDI`.
    Addi,
    /// `ADDIU`.
    Addiu,
    /// `ADDU`.
    Addu,
    /// `AND`.
    And,
    /// `ANDI`.
    Andi,
    /// `BEQ`.
    Beq,
    /// `BEQL`.
    Beql,
    /// `BGEZ`.
    Bgez,
    /// `BGEZAL`.
    Bgezal,
    /// `BGEZALL`.
    Bgezall,
    /// `BGEZL`.
    Bgezl,
    /// `BGTZ`.
    Bgtz,
    /// `BGTZL`.
    Bgtzl,
    /// `BLEZ`.
    Blez,
    /// `BLEZL`.
    Blezl,
    /// `BLTZ`.
    Bltz,
    /// `BLTZAL`.
    Bltzal,
    /// `BLTZALL`.
    Bltzall,
    /// `BLTZL`.
    Bltzl,
    /// `BNE`.
    Bne,
    /// `BNEL`.
    Bnel,
    /// `BREAK`.
    Break,
    /// `DADD`.
    Dadd,
    /// `DADDI`.
    Daddi,
    /// `DADDIU`.
    Daddiu,
    /// `DADDU`.
    Daddu,
    /// `DDIV`.
    Ddiv,
    /// `DDIVU`.
    Ddivu,
    /// `DIV`.
    Div,
    /// `DIVU`.
    Divu,
    /// `DMULT`.
    Dmult,
    /// `DMULTU`.
    Dmultu,
    /// `DSLL`.
    Dsll,
    /// `DSLL32`.
    Dsll32,
    /// `DSLLV`.
    Dsllv,
    /// `DSRA`.
    Dsra,
    /// `DSRA32`.
    Dsra32,
    /// `DSRAV`.
    Dsrav,
    /// `DSRL`.
    Dsrl,
    /// `DSRL32`.
    Dsrl32,
    /// `DSRLV`.
    Dsrlv,
    /// `DSUB`.
    Dsub,
    /// `DSUBU`.
    Dsubu,
    /// `J`.
    J,
    /// `JAL`.
    Jal,
    /// `JALR`.
    Jalr,
    /// `JR`.
    Jr,
    /// `LB`.
    Lb,
    /// `LBU`.
    Lbu,
    /// `LD`.
    Ld,
    /// `LDL`.
    Ldl,
    /// `LDR`.
    Ldr,
    /// `LH`.
    Lh,
    /// `LHU`.
    Lhu,
    /// `LL`.
    Ll,
    /// `LLD`.
    Lld,
    /// `LUI`.
    Lui,
    /// `LW`.
    Lw,
    /// `LWL`.
    Lwl,
    /// `LWR`.
    Lwr,
    /// `LWU`.
    Lwu,
    /// `MFHI`.
    Mfhi,
    /// `MFLO`.
    Mflo,
    /// `MOVN`.
    Movn,
    /// `MOVZ`.
    Movz,
    /// `MTHI`.
    Mthi,
    /// `MTLO`.
    Mtlo,
    /// `MULT`.
    Mult,
    /// `MULTU`.
    Multu,
    /// `NOR`.
    Nor,
    /// `OR`.
    Or,
    /// `ORI`.
    Ori,
    /// `PREF`.
    Pref,
    /// `SB`.
    Sb,
    /// `SC`.
    Sc,
    /// `SCD`.
    Scd,
    /// `SD`.
    Sd,
    /// `SDL`.
    Sdl,
    /// `SDR`.
    Sdr,
    /// `SH`.
    Sh,
    /// `SLL`.
    Sll,
    /// `SLLV`.
    Sllv,
    /// `SLT`.
    Slt,
    /// `SLTI`.
    Slti,
    /// `SLTIU`.
    Sltiu,
    /// `SLTU`.
    Sltu,
    /// `SRA`.
    Sra,
    /// `SRAV`.
    Srav,
    /// `SRL`.
    Srl,
    /// `SRLV`.
    Srlv,
    /// `SUB`.
    Sub,
    /// `SUBU`.
    Subu,
    /// `SW`.
    Sw,
    /// `SWL`.
    Swl,
    /// `SWR`.
    Swr,
    /// `SYNC`.
    Sync,
    /// `SYSCALL`.
    Syscall,
    /// `TEQ`.
    Teq,
    /// `TEQI`.
    Teqi,
    /// `TGE`.
    Tge,
    /// `TGEI`.
    Tgei,
    /// `TGEIU`.
    Tgeiu,
    /// `TGEU`.
    Tgeu,
    /// `TLT`.
    Tlt,
    /// `TLTI`.
    Tlti,
    /// `TLTIU`.
    Tltiu,
    /// `TLTU`.
    Tltu,
    /// `TNE`.
    Tne,
    /// `TNEI`.
    Tnei,
    /// `XOR`.
    Xor,
    /// `XORI`.
    Xori,
}

/// CP1 classes that appear in the CPU encoding map.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4FpuInstructionClass {
    /// `COP1`.
    Cop1,
    /// `COP1X`.
    Cop1x,
    /// `SPECIAL` + `MOVCI`.
    Movci,
    /// `LWC1`.
    LoadWord,
    /// `LDC1`.
    LoadDoubleword,
    /// `SWC1`.
    StoreWord,
    /// `SDC1`.
    StoreDoubleword,
}

/// Classifies one raw MIPS IV instruction word.
pub const fn decode_instruction(instruction: Mips4Instruction) -> Mips4InstructionDecode {
    match instruction.opcode() {
        MIPS4_SPECIAL_OPCODE => decode_special(instruction.funct()),
        MIPS4_REGIMM_OPCODE => decode_regimm(instruction.rt()),
        0x02 => cpu(Mips4CpuInstruction::J),
        0x03 => cpu(Mips4CpuInstruction::Jal),
        0x04 => cpu(Mips4CpuInstruction::Beq),
        0x05 => cpu(Mips4CpuInstruction::Bne),
        0x06 => cpu(Mips4CpuInstruction::Blez),
        0x07 => cpu(Mips4CpuInstruction::Bgtz),
        0x08 => cpu(Mips4CpuInstruction::Addi),
        0x09 => cpu(Mips4CpuInstruction::Addiu),
        0x0a => cpu(Mips4CpuInstruction::Slti),
        0x0b => cpu(Mips4CpuInstruction::Sltiu),
        0x0c => cpu(Mips4CpuInstruction::Andi),
        0x0d => cpu(Mips4CpuInstruction::Ori),
        0x0e => cpu(Mips4CpuInstruction::Xori),
        0x0f => cpu(Mips4CpuInstruction::Lui),
        0x10 => coprocessor(Mips4CoprocessorNumber::Cp0),
        0x11 => fpu(Mips4FpuInstructionClass::Cop1),
        0x12 => coprocessor(Mips4CoprocessorNumber::Cp2),
        0x13 => fpu(Mips4FpuInstructionClass::Cop1x),
        0x14 => cpu(Mips4CpuInstruction::Beql),
        0x15 => cpu(Mips4CpuInstruction::Bnel),
        0x16 => cpu(Mips4CpuInstruction::Blezl),
        0x17 => cpu(Mips4CpuInstruction::Bgtzl),
        0x18 => cpu(Mips4CpuInstruction::Daddi),
        0x19 => cpu(Mips4CpuInstruction::Daddiu),
        0x1a => cpu(Mips4CpuInstruction::Ldl),
        0x1b => cpu(Mips4CpuInstruction::Ldr),
        0x20 => cpu(Mips4CpuInstruction::Lb),
        0x21 => cpu(Mips4CpuInstruction::Lh),
        0x22 => cpu(Mips4CpuInstruction::Lwl),
        0x23 => cpu(Mips4CpuInstruction::Lw),
        0x24 => cpu(Mips4CpuInstruction::Lbu),
        0x25 => cpu(Mips4CpuInstruction::Lhu),
        0x26 => cpu(Mips4CpuInstruction::Lwr),
        0x27 => cpu(Mips4CpuInstruction::Lwu),
        0x28 => cpu(Mips4CpuInstruction::Sb),
        0x29 => cpu(Mips4CpuInstruction::Sh),
        0x2a => cpu(Mips4CpuInstruction::Swl),
        0x2b => cpu(Mips4CpuInstruction::Sw),
        0x2c => cpu(Mips4CpuInstruction::Sdl),
        0x2d => cpu(Mips4CpuInstruction::Sdr),
        0x2e => cpu(Mips4CpuInstruction::Swr),
        0x2f => Mips4InstructionDecode::ProcessorSpecificCp0Offset,
        0x30 => cpu(Mips4CpuInstruction::Ll),
        0x31 => fpu(Mips4FpuInstructionClass::LoadWord),
        0x32 => coprocessor(Mips4CoprocessorNumber::Cp2),
        0x33 => cpu(Mips4CpuInstruction::Pref),
        0x34 => cpu(Mips4CpuInstruction::Lld),
        0x35 => fpu(Mips4FpuInstructionClass::LoadDoubleword),
        0x36 => coprocessor(Mips4CoprocessorNumber::Cp2),
        0x37 => cpu(Mips4CpuInstruction::Ld),
        0x38 => cpu(Mips4CpuInstruction::Sc),
        0x39 => fpu(Mips4FpuInstructionClass::StoreWord),
        0x3a => coprocessor(Mips4CoprocessorNumber::Cp2),
        0x3b => Mips4InstructionDecode::UndefinedResult,
        0x3c => cpu(Mips4CpuInstruction::Scd),
        0x3d => fpu(Mips4FpuInstructionClass::StoreDoubleword),
        0x3e => coprocessor(Mips4CoprocessorNumber::Cp2),
        0x3f => cpu(Mips4CpuInstruction::Sd),
        _ => Mips4InstructionDecode::ReservedInstruction,
    }
}

const fn decode_special(function: u8) -> Mips4InstructionDecode {
    match function {
        0x00 => cpu(Mips4CpuInstruction::Sll),
        0x01 => fpu(Mips4FpuInstructionClass::Movci),
        0x02 => cpu(Mips4CpuInstruction::Srl),
        0x03 => cpu(Mips4CpuInstruction::Sra),
        0x04 => cpu(Mips4CpuInstruction::Sllv),
        0x06 => cpu(Mips4CpuInstruction::Srlv),
        0x07 => cpu(Mips4CpuInstruction::Srav),
        0x08 => cpu(Mips4CpuInstruction::Jr),
        0x09 => cpu(Mips4CpuInstruction::Jalr),
        0x0a => cpu(Mips4CpuInstruction::Movz),
        0x0b => cpu(Mips4CpuInstruction::Movn),
        0x0c => cpu(Mips4CpuInstruction::Syscall),
        0x0d => cpu(Mips4CpuInstruction::Break),
        0x0f => cpu(Mips4CpuInstruction::Sync),
        0x10 => cpu(Mips4CpuInstruction::Mfhi),
        0x11 => cpu(Mips4CpuInstruction::Mthi),
        0x12 => cpu(Mips4CpuInstruction::Mflo),
        0x13 => cpu(Mips4CpuInstruction::Mtlo),
        0x14 => cpu(Mips4CpuInstruction::Dsllv),
        0x16 => cpu(Mips4CpuInstruction::Dsrlv),
        0x17 => cpu(Mips4CpuInstruction::Dsrav),
        0x18 => cpu(Mips4CpuInstruction::Mult),
        0x19 => cpu(Mips4CpuInstruction::Multu),
        0x1a => cpu(Mips4CpuInstruction::Div),
        0x1b => cpu(Mips4CpuInstruction::Divu),
        0x1c => cpu(Mips4CpuInstruction::Dmult),
        0x1d => cpu(Mips4CpuInstruction::Dmultu),
        0x1e => cpu(Mips4CpuInstruction::Ddiv),
        0x1f => cpu(Mips4CpuInstruction::Ddivu),
        0x20 => cpu(Mips4CpuInstruction::Add),
        0x21 => cpu(Mips4CpuInstruction::Addu),
        0x22 => cpu(Mips4CpuInstruction::Sub),
        0x23 => cpu(Mips4CpuInstruction::Subu),
        0x24 => cpu(Mips4CpuInstruction::And),
        0x25 => cpu(Mips4CpuInstruction::Or),
        0x26 => cpu(Mips4CpuInstruction::Xor),
        0x27 => cpu(Mips4CpuInstruction::Nor),
        0x2a => cpu(Mips4CpuInstruction::Slt),
        0x2b => cpu(Mips4CpuInstruction::Sltu),
        0x2c => cpu(Mips4CpuInstruction::Dadd),
        0x2d => cpu(Mips4CpuInstruction::Daddu),
        0x2e => cpu(Mips4CpuInstruction::Dsub),
        0x2f => cpu(Mips4CpuInstruction::Dsubu),
        0x30 => cpu(Mips4CpuInstruction::Tge),
        0x31 => cpu(Mips4CpuInstruction::Tgeu),
        0x32 => cpu(Mips4CpuInstruction::Tlt),
        0x33 => cpu(Mips4CpuInstruction::Tltu),
        0x34 => cpu(Mips4CpuInstruction::Teq),
        0x36 => cpu(Mips4CpuInstruction::Tne),
        0x38 => cpu(Mips4CpuInstruction::Dsll),
        0x3a => cpu(Mips4CpuInstruction::Dsrl),
        0x3b => cpu(Mips4CpuInstruction::Dsra),
        0x3c => cpu(Mips4CpuInstruction::Dsll32),
        0x3e => cpu(Mips4CpuInstruction::Dsrl32),
        0x3f => cpu(Mips4CpuInstruction::Dsra32),
        _ => Mips4InstructionDecode::ReservedInstruction,
    }
}

const fn decode_regimm(rt: u8) -> Mips4InstructionDecode {
    match rt {
        0x00 => cpu(Mips4CpuInstruction::Bltz),
        0x01 => cpu(Mips4CpuInstruction::Bgez),
        0x02 => cpu(Mips4CpuInstruction::Bltzl),
        0x03 => cpu(Mips4CpuInstruction::Bgezl),
        0x08 => cpu(Mips4CpuInstruction::Tgei),
        0x09 => cpu(Mips4CpuInstruction::Tgeiu),
        0x0a => cpu(Mips4CpuInstruction::Tlti),
        0x0b => cpu(Mips4CpuInstruction::Tltiu),
        0x0c => cpu(Mips4CpuInstruction::Teqi),
        0x0e => cpu(Mips4CpuInstruction::Tnei),
        0x10 => cpu(Mips4CpuInstruction::Bltzal),
        0x11 => cpu(Mips4CpuInstruction::Bgezal),
        0x12 => cpu(Mips4CpuInstruction::Bltzall),
        0x13 => cpu(Mips4CpuInstruction::Bgezall),
        _ => Mips4InstructionDecode::ReservedInstruction,
    }
}

const fn cpu(instruction: Mips4CpuInstruction) -> Mips4InstructionDecode {
    Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(instruction))
}

const fn fpu(instruction: Mips4FpuInstructionClass) -> Mips4InstructionDecode {
    Mips4InstructionDecode::Instruction(Mips4InstructionClass::Fpu(instruction))
}

const fn coprocessor(coprocessor: Mips4CoprocessorNumber) -> Mips4InstructionDecode {
    Mips4InstructionDecode::Instruction(Mips4InstructionClass::Coprocessor(coprocessor))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits_with_opcode(opcode: u8) -> Mips4Instruction {
        Mips4Instruction::from_bits((opcode as u32) << 26)
    }

    fn bits_with_special_function(function: u8) -> Mips4Instruction {
        Mips4Instruction::from_bits(function as u32)
    }

    fn bits_with_regimm_rt(rt: u8) -> Mips4Instruction {
        Mips4Instruction::from_bits(((MIPS4_REGIMM_OPCODE as u32) << 26) | ((rt as u32) << 16))
    }

    fn cpu_instruction(instruction: Mips4CpuInstruction) -> Mips4InstructionDecode {
        Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(instruction))
    }

    fn fpu_instruction(instruction: Mips4FpuInstructionClass) -> Mips4InstructionDecode {
        Mips4InstructionDecode::Instruction(Mips4InstructionClass::Fpu(instruction))
    }

    fn coprocessor_instruction(coprocessor: Mips4CoprocessorNumber) -> Mips4InstructionDecode {
        Mips4InstructionDecode::Instruction(Mips4InstructionClass::Coprocessor(coprocessor))
    }

    #[test]
    fn primary_opcode_table_classifies_mips4_encodings() {
        let mut expected = [Mips4InstructionDecode::ReservedInstruction; 64];
        expected[0x00] = cpu_instruction(Mips4CpuInstruction::Sll);
        expected[0x01] = cpu_instruction(Mips4CpuInstruction::Bltz);
        expected[0x02] = cpu_instruction(Mips4CpuInstruction::J);
        expected[0x03] = cpu_instruction(Mips4CpuInstruction::Jal);
        expected[0x04] = cpu_instruction(Mips4CpuInstruction::Beq);
        expected[0x05] = cpu_instruction(Mips4CpuInstruction::Bne);
        expected[0x06] = cpu_instruction(Mips4CpuInstruction::Blez);
        expected[0x07] = cpu_instruction(Mips4CpuInstruction::Bgtz);
        expected[0x08] = cpu_instruction(Mips4CpuInstruction::Addi);
        expected[0x09] = cpu_instruction(Mips4CpuInstruction::Addiu);
        expected[0x0a] = cpu_instruction(Mips4CpuInstruction::Slti);
        expected[0x0b] = cpu_instruction(Mips4CpuInstruction::Sltiu);
        expected[0x0c] = cpu_instruction(Mips4CpuInstruction::Andi);
        expected[0x0d] = cpu_instruction(Mips4CpuInstruction::Ori);
        expected[0x0e] = cpu_instruction(Mips4CpuInstruction::Xori);
        expected[0x0f] = cpu_instruction(Mips4CpuInstruction::Lui);
        expected[0x10] = coprocessor_instruction(Mips4CoprocessorNumber::Cp0);
        expected[0x11] = fpu_instruction(Mips4FpuInstructionClass::Cop1);
        expected[0x12] = coprocessor_instruction(Mips4CoprocessorNumber::Cp2);
        expected[0x13] = fpu_instruction(Mips4FpuInstructionClass::Cop1x);
        expected[0x14] = cpu_instruction(Mips4CpuInstruction::Beql);
        expected[0x15] = cpu_instruction(Mips4CpuInstruction::Bnel);
        expected[0x16] = cpu_instruction(Mips4CpuInstruction::Blezl);
        expected[0x17] = cpu_instruction(Mips4CpuInstruction::Bgtzl);
        expected[0x18] = cpu_instruction(Mips4CpuInstruction::Daddi);
        expected[0x19] = cpu_instruction(Mips4CpuInstruction::Daddiu);
        expected[0x1a] = cpu_instruction(Mips4CpuInstruction::Ldl);
        expected[0x1b] = cpu_instruction(Mips4CpuInstruction::Ldr);
        expected[0x20] = cpu_instruction(Mips4CpuInstruction::Lb);
        expected[0x21] = cpu_instruction(Mips4CpuInstruction::Lh);
        expected[0x22] = cpu_instruction(Mips4CpuInstruction::Lwl);
        expected[0x23] = cpu_instruction(Mips4CpuInstruction::Lw);
        expected[0x24] = cpu_instruction(Mips4CpuInstruction::Lbu);
        expected[0x25] = cpu_instruction(Mips4CpuInstruction::Lhu);
        expected[0x26] = cpu_instruction(Mips4CpuInstruction::Lwr);
        expected[0x27] = cpu_instruction(Mips4CpuInstruction::Lwu);
        expected[0x28] = cpu_instruction(Mips4CpuInstruction::Sb);
        expected[0x29] = cpu_instruction(Mips4CpuInstruction::Sh);
        expected[0x2a] = cpu_instruction(Mips4CpuInstruction::Swl);
        expected[0x2b] = cpu_instruction(Mips4CpuInstruction::Sw);
        expected[0x2c] = cpu_instruction(Mips4CpuInstruction::Sdl);
        expected[0x2d] = cpu_instruction(Mips4CpuInstruction::Sdr);
        expected[0x2e] = cpu_instruction(Mips4CpuInstruction::Swr);
        expected[0x2f] = Mips4InstructionDecode::ProcessorSpecificCp0Offset;
        expected[0x30] = cpu_instruction(Mips4CpuInstruction::Ll);
        expected[0x31] = fpu_instruction(Mips4FpuInstructionClass::LoadWord);
        expected[0x32] = coprocessor_instruction(Mips4CoprocessorNumber::Cp2);
        expected[0x33] = cpu_instruction(Mips4CpuInstruction::Pref);
        expected[0x34] = cpu_instruction(Mips4CpuInstruction::Lld);
        expected[0x35] = fpu_instruction(Mips4FpuInstructionClass::LoadDoubleword);
        expected[0x36] = coprocessor_instruction(Mips4CoprocessorNumber::Cp2);
        expected[0x37] = cpu_instruction(Mips4CpuInstruction::Ld);
        expected[0x38] = cpu_instruction(Mips4CpuInstruction::Sc);
        expected[0x39] = fpu_instruction(Mips4FpuInstructionClass::StoreWord);
        expected[0x3a] = coprocessor_instruction(Mips4CoprocessorNumber::Cp2);
        expected[0x3b] = Mips4InstructionDecode::UndefinedResult;
        expected[0x3c] = cpu_instruction(Mips4CpuInstruction::Scd);
        expected[0x3d] = fpu_instruction(Mips4FpuInstructionClass::StoreDoubleword);
        expected[0x3e] = coprocessor_instruction(Mips4CoprocessorNumber::Cp2);
        expected[0x3f] = cpu_instruction(Mips4CpuInstruction::Sd);

        for opcode in 0..64 {
            assert_eq!(
                decode_instruction(bits_with_opcode(opcode)),
                expected[opcode as usize],
                "opcode {opcode:#04x}"
            );
        }
    }

    #[test]
    fn special_function_table_classifies_mips4_encodings() {
        let mut expected = [Mips4InstructionDecode::ReservedInstruction; 64];
        expected[0x00] = cpu_instruction(Mips4CpuInstruction::Sll);
        expected[0x01] = fpu_instruction(Mips4FpuInstructionClass::Movci);
        expected[0x02] = cpu_instruction(Mips4CpuInstruction::Srl);
        expected[0x03] = cpu_instruction(Mips4CpuInstruction::Sra);
        expected[0x04] = cpu_instruction(Mips4CpuInstruction::Sllv);
        expected[0x06] = cpu_instruction(Mips4CpuInstruction::Srlv);
        expected[0x07] = cpu_instruction(Mips4CpuInstruction::Srav);
        expected[0x08] = cpu_instruction(Mips4CpuInstruction::Jr);
        expected[0x09] = cpu_instruction(Mips4CpuInstruction::Jalr);
        expected[0x0a] = cpu_instruction(Mips4CpuInstruction::Movz);
        expected[0x0b] = cpu_instruction(Mips4CpuInstruction::Movn);
        expected[0x0c] = cpu_instruction(Mips4CpuInstruction::Syscall);
        expected[0x0d] = cpu_instruction(Mips4CpuInstruction::Break);
        expected[0x0f] = cpu_instruction(Mips4CpuInstruction::Sync);
        expected[0x10] = cpu_instruction(Mips4CpuInstruction::Mfhi);
        expected[0x11] = cpu_instruction(Mips4CpuInstruction::Mthi);
        expected[0x12] = cpu_instruction(Mips4CpuInstruction::Mflo);
        expected[0x13] = cpu_instruction(Mips4CpuInstruction::Mtlo);
        expected[0x14] = cpu_instruction(Mips4CpuInstruction::Dsllv);
        expected[0x16] = cpu_instruction(Mips4CpuInstruction::Dsrlv);
        expected[0x17] = cpu_instruction(Mips4CpuInstruction::Dsrav);
        expected[0x18] = cpu_instruction(Mips4CpuInstruction::Mult);
        expected[0x19] = cpu_instruction(Mips4CpuInstruction::Multu);
        expected[0x1a] = cpu_instruction(Mips4CpuInstruction::Div);
        expected[0x1b] = cpu_instruction(Mips4CpuInstruction::Divu);
        expected[0x1c] = cpu_instruction(Mips4CpuInstruction::Dmult);
        expected[0x1d] = cpu_instruction(Mips4CpuInstruction::Dmultu);
        expected[0x1e] = cpu_instruction(Mips4CpuInstruction::Ddiv);
        expected[0x1f] = cpu_instruction(Mips4CpuInstruction::Ddivu);
        expected[0x20] = cpu_instruction(Mips4CpuInstruction::Add);
        expected[0x21] = cpu_instruction(Mips4CpuInstruction::Addu);
        expected[0x22] = cpu_instruction(Mips4CpuInstruction::Sub);
        expected[0x23] = cpu_instruction(Mips4CpuInstruction::Subu);
        expected[0x24] = cpu_instruction(Mips4CpuInstruction::And);
        expected[0x25] = cpu_instruction(Mips4CpuInstruction::Or);
        expected[0x26] = cpu_instruction(Mips4CpuInstruction::Xor);
        expected[0x27] = cpu_instruction(Mips4CpuInstruction::Nor);
        expected[0x2a] = cpu_instruction(Mips4CpuInstruction::Slt);
        expected[0x2b] = cpu_instruction(Mips4CpuInstruction::Sltu);
        expected[0x2c] = cpu_instruction(Mips4CpuInstruction::Dadd);
        expected[0x2d] = cpu_instruction(Mips4CpuInstruction::Daddu);
        expected[0x2e] = cpu_instruction(Mips4CpuInstruction::Dsub);
        expected[0x2f] = cpu_instruction(Mips4CpuInstruction::Dsubu);
        expected[0x30] = cpu_instruction(Mips4CpuInstruction::Tge);
        expected[0x31] = cpu_instruction(Mips4CpuInstruction::Tgeu);
        expected[0x32] = cpu_instruction(Mips4CpuInstruction::Tlt);
        expected[0x33] = cpu_instruction(Mips4CpuInstruction::Tltu);
        expected[0x34] = cpu_instruction(Mips4CpuInstruction::Teq);
        expected[0x36] = cpu_instruction(Mips4CpuInstruction::Tne);
        expected[0x38] = cpu_instruction(Mips4CpuInstruction::Dsll);
        expected[0x3a] = cpu_instruction(Mips4CpuInstruction::Dsrl);
        expected[0x3b] = cpu_instruction(Mips4CpuInstruction::Dsra);
        expected[0x3c] = cpu_instruction(Mips4CpuInstruction::Dsll32);
        expected[0x3e] = cpu_instruction(Mips4CpuInstruction::Dsrl32);
        expected[0x3f] = cpu_instruction(Mips4CpuInstruction::Dsra32);

        for function in 0..64 {
            assert_eq!(
                decode_instruction(bits_with_special_function(function)),
                expected[function as usize],
                "SPECIAL function {function:#04x}"
            );
        }
    }

    #[test]
    fn regimm_rt_table_classifies_mips4_encodings() {
        let mut expected = [Mips4InstructionDecode::ReservedInstruction; 32];
        expected[0x00] = cpu_instruction(Mips4CpuInstruction::Bltz);
        expected[0x01] = cpu_instruction(Mips4CpuInstruction::Bgez);
        expected[0x02] = cpu_instruction(Mips4CpuInstruction::Bltzl);
        expected[0x03] = cpu_instruction(Mips4CpuInstruction::Bgezl);
        expected[0x08] = cpu_instruction(Mips4CpuInstruction::Tgei);
        expected[0x09] = cpu_instruction(Mips4CpuInstruction::Tgeiu);
        expected[0x0a] = cpu_instruction(Mips4CpuInstruction::Tlti);
        expected[0x0b] = cpu_instruction(Mips4CpuInstruction::Tltiu);
        expected[0x0c] = cpu_instruction(Mips4CpuInstruction::Teqi);
        expected[0x0e] = cpu_instruction(Mips4CpuInstruction::Tnei);
        expected[0x10] = cpu_instruction(Mips4CpuInstruction::Bltzal);
        expected[0x11] = cpu_instruction(Mips4CpuInstruction::Bgezal);
        expected[0x12] = cpu_instruction(Mips4CpuInstruction::Bltzall);
        expected[0x13] = cpu_instruction(Mips4CpuInstruction::Bgezall);

        for rt in 0..32 {
            assert_eq!(
                decode_instruction(bits_with_regimm_rt(rt)),
                expected[rt as usize],
                "REGIMM rt {rt:#04x}"
            );
        }
    }

    #[test]
    fn decode_reports_required_coprocessors() {
        assert_eq!(
            decode_instruction(bits_with_opcode(0x11)).required_coprocessor(),
            Some(Mips4CoprocessorNumber::Cp1)
        );
        assert_eq!(
            decode_instruction(bits_with_opcode(0x13)).required_coprocessor(),
            Some(Mips4CoprocessorNumber::Cp1)
        );
        assert_eq!(
            decode_instruction(bits_with_special_function(0x01)).required_coprocessor(),
            Some(Mips4CoprocessorNumber::Cp1)
        );
        assert_eq!(
            decode_instruction(bits_with_opcode(0x31)).required_coprocessor(),
            Some(Mips4CoprocessorNumber::Cp1)
        );
        assert_eq!(
            decode_instruction(bits_with_opcode(0x12)).required_coprocessor(),
            Some(Mips4CoprocessorNumber::Cp2)
        );
        assert_eq!(
            decode_instruction(bits_with_opcode(0x02)).required_coprocessor(),
            None
        );
        assert_eq!(
            decode_instruction(bits_with_opcode(0x2f)).required_coprocessor(),
            Some(Mips4CoprocessorNumber::Cp0)
        );
    }
}
