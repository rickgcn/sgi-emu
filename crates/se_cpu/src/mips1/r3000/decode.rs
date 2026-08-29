const COPROCESSOR_TRANSFER_RESERVED_MASK: u32 = 0x7ff;

const CP0_TLBR: u32 = 0x4200_0001;
const CP0_TLBWI: u32 = 0x4200_0002;
const CP0_TLBWR: u32 = 0x4200_0006;
const CP0_TLBP: u32 = 0x4200_0008;
const CP0_RFE: u32 = 0x4200_0010;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DecodeResult {
    Implemented(Instruction),
    UnsupportedCoprocessor { unit: usize },
    Reserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Instruction {
    Alu(AluInstruction),
    Control(ControlInstruction),
    Cp0(Cp0Instruction),
    Cp1(Cp1Instruction),
    Memory(MemoryInstruction),
    Syscall,
    Breakpoint,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum AluInstruction {
    Sll {
        rd: usize,
        rt: usize,
        shift_amount: u32,
    },
    Srl {
        rd: usize,
        rt: usize,
        shift_amount: u32,
    },
    Sra {
        rd: usize,
        rt: usize,
        shift_amount: u32,
    },
    Sllv {
        rd: usize,
        rt: usize,
        rs: usize,
    },
    Srlv {
        rd: usize,
        rt: usize,
        rs: usize,
    },
    Srav {
        rd: usize,
        rt: usize,
        rs: usize,
    },
    Mfhi {
        rd: usize,
    },
    Mthi {
        rs: usize,
    },
    Mflo {
        rd: usize,
    },
    Mtlo {
        rs: usize,
    },
    Mult {
        rs: usize,
        rt: usize,
    },
    Multu {
        rs: usize,
        rt: usize,
    },
    Div {
        rs: usize,
        rt: usize,
    },
    Divu {
        rs: usize,
        rt: usize,
    },
    Add {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Addu {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Sub {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Subu {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    And {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Or {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Xor {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Nor {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Slt {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Sltu {
        rd: usize,
        rs: usize,
        rt: usize,
    },
    Addiu {
        rt: usize,
        rs: usize,
        immediate: u16,
    },
    Addi {
        rt: usize,
        rs: usize,
        immediate: u16,
    },
    Slti {
        rt: usize,
        rs: usize,
        immediate: u16,
    },
    Sltiu {
        rt: usize,
        rs: usize,
        immediate: u16,
    },
    Andi {
        rt: usize,
        rs: usize,
        immediate: u16,
    },
    Ori {
        rt: usize,
        rs: usize,
        immediate: u16,
    },
    Xori {
        rt: usize,
        rs: usize,
        immediate: u16,
    },
    Lui {
        rt: usize,
        immediate: u16,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ControlInstruction {
    J { target: u32 },
    Jal { target: u32 },
    Jr { rs: usize },
    Jalr { rd: usize, rs: usize },
    Beq { rs: usize, rt: usize, offset: u16 },
    Bne { rs: usize, rt: usize, offset: u16 },
    Blez { rs: usize, offset: u16 },
    Bgtz { rs: usize, offset: u16 },
    Bltz { rs: usize, offset: u16 },
    Bgez { rs: usize, offset: u16 },
    Bltzal { rs: usize, offset: u16 },
    Bgezal { rs: usize, offset: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Cp0Instruction {
    Mfc0 { rt: usize, rd: usize },
    Cfc0 { rt: usize, rd: usize },
    Mtc0 { rt: usize, rd: usize },
    Ctc0 { rt: usize, rd: usize },
    Bc0f { offset: u16 },
    Bc0t { offset: u16 },
    Tlbr,
    Tlbwi,
    Tlbwr,
    Tlbp,
    Rfe,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Cp1Instruction {
    Mfc1 { rt: usize, rd: usize },
    Cfc1 { rt: usize, rd: usize },
    Mtc1 { rt: usize, rd: usize },
    Ctc1 { rt: usize, rd: usize },
    Bc1f { offset: u16 },
    Bc1t { offset: u16 },
    Lwc1 { base: usize, ft: usize, offset: u16 },
    Swc1 { base: usize, ft: usize, offset: u16 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum MemoryInstruction {
    Lb { base: usize, rt: usize, offset: u16 },
    Lbu { base: usize, rt: usize, offset: u16 },
    Lh { base: usize, rt: usize, offset: u16 },
    Lhu { base: usize, rt: usize, offset: u16 },
    Lwl { base: usize, rt: usize, offset: u16 },
    Lw { base: usize, rt: usize, offset: u16 },
    Lwr { base: usize, rt: usize, offset: u16 },
    Sb { base: usize, rt: usize, offset: u16 },
    Sh { base: usize, rt: usize, offset: u16 },
    Swl { base: usize, rt: usize, offset: u16 },
    Sw { base: usize, rt: usize, offset: u16 },
    Swr { base: usize, rt: usize, offset: u16 },
}

pub(super) fn decode(word: u32) -> DecodeResult {
    match opcode(word) {
        0x00 => decode_special(word),
        0x01 => decode_regimm(word),
        0x02 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::J {
            target: target(word),
        })),
        0x03 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Jal {
            target: target(word),
        })),
        0x04 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Beq {
            rs: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x05 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Bne {
            rs: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x06 if rt(word) == 0 => {
            DecodeResult::Implemented(Instruction::Control(ControlInstruction::Blez {
                rs: rs(word),
                offset: immediate(word),
            }))
        }
        0x06 => DecodeResult::Reserved,
        0x07 if rt(word) == 0 => {
            DecodeResult::Implemented(Instruction::Control(ControlInstruction::Bgtz {
                rs: rs(word),
                offset: immediate(word),
            }))
        }
        0x07 => DecodeResult::Reserved,
        0x08 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Addi {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x09 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Addiu {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0a => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Slti {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0b => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Sltiu {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0c => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Andi {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0d => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Ori {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0e => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Xori {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0f => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Lui {
            rt: rt(word),
            immediate: immediate(word),
        })),
        0x10 => decode_cp0(word),
        0x11 => decode_cp1(word),
        0x12..=0x13 => decode_unsupported_coprocessor(word),
        0x20 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Lb {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x21 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Lh {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x22 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Lwl {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x23 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Lw {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x24 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Lbu {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x25 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Lhu {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x26 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Lwr {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x31 => DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Lwc1 {
            base: rs(word),
            ft: rt(word),
            offset: immediate(word),
        })),
        0x32..=0x33 => DecodeResult::UnsupportedCoprocessor {
            unit: opcode(word) as usize - 0x30,
        },
        0x28 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Sb {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x29 => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Sh {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x2a => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Swl {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x2b => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Sw {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x2e => DecodeResult::Implemented(Instruction::Memory(MemoryInstruction::Swr {
            base: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x39 => DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Swc1 {
            base: rs(word),
            ft: rt(word),
            offset: immediate(word),
        })),
        0x3a..=0x3b => DecodeResult::UnsupportedCoprocessor {
            unit: opcode(word) as usize - 0x38,
        },
        _ => DecodeResult::Reserved,
    }
}

fn decode_special(word: u32) -> DecodeResult {
    match function(word) {
        0x00 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Sll {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        })),
        0x02 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Srl {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        })),
        0x03 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Sra {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        })),
        0x04 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Sllv {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        })),
        0x06 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Srlv {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        })),
        0x07 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Srav {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        })),
        0x08 if rd(word) == 0 => {
            DecodeResult::Implemented(Instruction::Control(ControlInstruction::Jr {
                rs: rs(word),
            }))
        }
        0x08 => DecodeResult::Reserved,
        0x09 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Jalr {
            rd: rd(word),
            rs: rs(word),
        })),
        0x0c => DecodeResult::Implemented(Instruction::Syscall),
        0x0d => DecodeResult::Implemented(Instruction::Breakpoint),
        0x10 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Mfhi { rd: rd(word) })),
        0x11 if rd(word) == 0 => {
            DecodeResult::Implemented(Instruction::Alu(AluInstruction::Mthi { rs: rs(word) }))
        }
        0x11 => DecodeResult::Reserved,
        0x12 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Mflo { rd: rd(word) })),
        0x13 if rd(word) == 0 => {
            DecodeResult::Implemented(Instruction::Alu(AluInstruction::Mtlo { rs: rs(word) }))
        }
        0x13 => DecodeResult::Reserved,
        0x18 if rd(word) == 0 => {
            DecodeResult::Implemented(Instruction::Alu(AluInstruction::Mult {
                rs: rs(word),
                rt: rt(word),
            }))
        }
        0x18 => DecodeResult::Reserved,
        0x19 if rd(word) == 0 => {
            DecodeResult::Implemented(Instruction::Alu(AluInstruction::Multu {
                rs: rs(word),
                rt: rt(word),
            }))
        }
        0x19 => DecodeResult::Reserved,
        0x1a if rd(word) == 0 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Div {
            rs: rs(word),
            rt: rt(word),
        })),
        0x1a => DecodeResult::Reserved,
        0x1b if rd(word) == 0 => {
            DecodeResult::Implemented(Instruction::Alu(AluInstruction::Divu {
                rs: rs(word),
                rt: rt(word),
            }))
        }
        0x1b => DecodeResult::Reserved,
        0x20 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Add {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x21 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Addu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x22 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Sub {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x23 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Subu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x24 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::And {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x25 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Or {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x26 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Xor {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x27 => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Nor {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x2a => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Slt {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        0x2b => DecodeResult::Implemented(Instruction::Alu(AluInstruction::Sltu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        })),
        _ => DecodeResult::Reserved,
    }
}

fn decode_regimm(word: u32) -> DecodeResult {
    match rt(word) {
        0x00 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Bltz {
            rs: rs(word),
            offset: immediate(word),
        })),
        0x01 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Bgez {
            rs: rs(word),
            offset: immediate(word),
        })),
        0x10 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Bltzal {
            rs: rs(word),
            offset: immediate(word),
        })),
        0x11 => DecodeResult::Implemented(Instruction::Control(ControlInstruction::Bgezal {
            rs: rs(word),
            offset: immediate(word),
        })),
        _ => DecodeResult::Reserved,
    }
}

fn decode_cp0(word: u32) -> DecodeResult {
    match rs(word) {
        0x00 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Mfc0 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x02 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Cfc0 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x04 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Mtc0 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x06 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Ctc0 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x00 | 0x02 | 0x04 | 0x06 => DecodeResult::Reserved,
        0x08 if rt(word) == 0 => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Bc0f {
                offset: immediate(word),
            }))
        }
        0x08 if rt(word) == 1 => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Bc0t {
                offset: immediate(word),
            }))
        }
        0x08 => DecodeResult::Reserved,
        0x10..=0x1f if word == CP0_TLBR => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Tlbr))
        }
        0x10..=0x1f if word == CP0_TLBWI => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Tlbwi))
        }
        0x10..=0x1f if word == CP0_TLBWR => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Tlbwr))
        }
        0x10..=0x1f if word == CP0_TLBP => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Tlbp))
        }
        0x10..=0x1f if word == CP0_RFE => {
            DecodeResult::Implemented(Instruction::Cp0(Cp0Instruction::Rfe))
        }
        _ => DecodeResult::Reserved,
    }
}

fn decode_cp1(word: u32) -> DecodeResult {
    match rs(word) {
        0x00 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Mfc1 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x02 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Cfc1 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x04 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Mtc1 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x06 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Ctc1 {
                rt: rt(word),
                rd: rd(word),
            }))
        }
        0x00 | 0x02 | 0x04 | 0x06 => DecodeResult::Reserved,
        0x08 if rt(word) == 0 => {
            DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Bc1f {
                offset: immediate(word),
            }))
        }
        0x08 if rt(word) == 1 => {
            DecodeResult::Implemented(Instruction::Cp1(Cp1Instruction::Bc1t {
                offset: immediate(word),
            }))
        }
        0x08 => DecodeResult::Reserved,
        0x10..=0x1f => DecodeResult::UnsupportedCoprocessor { unit: 1 },
        _ => DecodeResult::Reserved,
    }
}

fn decode_unsupported_coprocessor(word: u32) -> DecodeResult {
    let unit = opcode(word) as usize - 0x10;

    match rs(word) {
        0x00 | 0x02 | 0x04 | 0x06 if word & COPROCESSOR_TRANSFER_RESERVED_MASK == 0 => {
            DecodeResult::UnsupportedCoprocessor { unit }
        }
        0x00 | 0x02 | 0x04 | 0x06 => DecodeResult::Reserved,
        0x08 if matches!(rt(word), 0x00 | 0x01) => DecodeResult::UnsupportedCoprocessor { unit },
        0x08 => DecodeResult::Reserved,
        0x10..=0x1f => DecodeResult::UnsupportedCoprocessor { unit },
        _ => DecodeResult::Reserved,
    }
}

fn opcode(word: u32) -> u32 {
    word >> 26
}

fn function(word: u32) -> u32 {
    word & 0x3f
}

fn rs(word: u32) -> usize {
    ((word >> 21) & 0x1f) as usize
}

fn rt(word: u32) -> usize {
    ((word >> 16) & 0x1f) as usize
}

fn rd(word: u32) -> usize {
    ((word >> 11) & 0x1f) as usize
}

fn shift_amount(word: u32) -> u32 {
    (word >> 6) & 0x1f
}

fn immediate(word: u32) -> u16 {
    word as u16
}

fn target(word: u32) -> u32 {
    word & 0x03ff_ffff
}

#[cfg(test)]
mod tests {
    use super::{
        AluInstruction, CP0_RFE, CP0_TLBP, CP0_TLBR, CP0_TLBWI, CP0_TLBWR, ControlInstruction,
        Cp0Instruction, Cp1Instruction, DecodeResult, Instruction, MemoryInstruction, decode,
    };

    fn encode_register(rs: u32, rt: u32, rd: u32, shift_amount: u32, function: u32) -> u32 {
        (rs << 21) | (rt << 16) | (rd << 11) | (shift_amount << 6) | function
    }

    fn encode_immediate(opcode: u32, rs: u32, rt: u32, immediate: u16) -> u32 {
        (opcode << 26) | (rs << 21) | (rt << 16) | u32::from(immediate)
    }

    fn encode_jump(opcode: u32, target: u32) -> u32 {
        (opcode << 26) | target
    }

    fn encode_coprocessor(opcode: u32, selector: u32, rt: u32, rd: u32, low_bits: u32) -> u32 {
        (opcode << 26) | (selector << 21) | (rt << 16) | (rd << 11) | (low_bits & 0x7ff)
    }

    fn alu(instruction: AluInstruction) -> DecodeResult {
        DecodeResult::Implemented(Instruction::Alu(instruction))
    }

    fn control(instruction: ControlInstruction) -> DecodeResult {
        DecodeResult::Implemented(Instruction::Control(instruction))
    }

    fn cp0(instruction: Cp0Instruction) -> DecodeResult {
        DecodeResult::Implemented(Instruction::Cp0(instruction))
    }

    fn cp1(instruction: Cp1Instruction) -> DecodeResult {
        DecodeResult::Implemented(Instruction::Cp1(instruction))
    }

    fn memory(instruction: MemoryInstruction) -> DecodeResult {
        DecodeResult::Implemented(Instruction::Memory(instruction))
    }

    #[test]
    fn decodes_every_supported_alu_instruction() {
        let cases = [
            (
                encode_register(0, 2, 3, 4, 0x00),
                alu(AluInstruction::Sll {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                }),
            ),
            (
                encode_register(0, 2, 3, 4, 0x02),
                alu(AluInstruction::Srl {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                }),
            ),
            (
                encode_register(0, 2, 3, 4, 0x03),
                alu(AluInstruction::Sra {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x04),
                alu(AluInstruction::Sllv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x06),
                alu(AluInstruction::Srlv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x07),
                alu(AluInstruction::Srav {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                }),
            ),
            (
                encode_register(0, 0, 31, 0, 0x10),
                alu(AluInstruction::Mfhi { rd: 31 }),
            ),
            (
                encode_register(31, 0, 0, 0, 0x11),
                alu(AluInstruction::Mthi { rs: 31 }),
            ),
            (
                encode_register(0, 0, 0, 0, 0x12),
                alu(AluInstruction::Mflo { rd: 0 }),
            ),
            (
                encode_register(0, 0, 0, 0, 0x13),
                alu(AluInstruction::Mtlo { rs: 0 }),
            ),
            (
                encode_register(31, 0, 0, 0, 0x18),
                alu(AluInstruction::Mult { rs: 31, rt: 0 }),
            ),
            (
                encode_register(0, 31, 0, 0, 0x19),
                alu(AluInstruction::Multu { rs: 0, rt: 31 }),
            ),
            (
                encode_register(31, 1, 0, 0, 0x1a),
                alu(AluInstruction::Div { rs: 31, rt: 1 }),
            ),
            (
                encode_register(1, 31, 0, 0, 0x1b),
                alu(AluInstruction::Divu { rs: 1, rt: 31 }),
            ),
            (
                encode_register(1, 2, 3, 31, 0x20),
                alu(AluInstruction::Add {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x21),
                alu(AluInstruction::Addu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 31, 0x22),
                alu(AluInstruction::Sub {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x23),
                alu(AluInstruction::Subu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x24),
                alu(AluInstruction::And {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x25),
                alu(AluInstruction::Or {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x26),
                alu(AluInstruction::Xor {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x27),
                alu(AluInstruction::Nor {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x2a),
                alu(AluInstruction::Slt {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_register(1, 2, 3, 0, 0x2b),
                alu(AluInstruction::Sltu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_immediate(0x08, 1, 2, 0x8001),
                alu(AluInstruction::Addi {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                }),
            ),
            (
                encode_immediate(0x09, 1, 2, 0x8001),
                alu(AluInstruction::Addiu {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                }),
            ),
            (
                encode_immediate(0x0a, 1, 2, 0x8001),
                alu(AluInstruction::Slti {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                }),
            ),
            (
                encode_immediate(0x0b, 1, 2, 0x8001),
                alu(AluInstruction::Sltiu {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                }),
            ),
            (
                encode_immediate(0x0c, 1, 2, 0x8001),
                alu(AluInstruction::Andi {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                }),
            ),
            (
                encode_immediate(0x0d, 1, 2, 0x8001),
                alu(AluInstruction::Ori {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                }),
            ),
            (
                encode_immediate(0x0e, 1, 2, 0x8001),
                alu(AluInstruction::Xori {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                }),
            ),
            (
                encode_immediate(0x0f, 0, 2, 0x8001),
                alu(AluInstruction::Lui {
                    rt: 2,
                    immediate: 0x8001,
                }),
            ),
        ];

        for (word, expected) in cases {
            assert_eq!(decode(word), expected);
        }
    }

    #[test]
    fn decodes_every_supported_control_instruction() {
        let cases = [
            (
                encode_jump(0x02, 0x0123_4567),
                control(ControlInstruction::J {
                    target: 0x0123_4567,
                }),
            ),
            (
                encode_jump(0x03, 0x02ab_cdef),
                control(ControlInstruction::Jal {
                    target: 0x02ab_cdef,
                }),
            ),
            (
                encode_register(1, 31, 0, 31, 0x08),
                control(ControlInstruction::Jr { rs: 1 }),
            ),
            (
                encode_register(1, 31, 3, 31, 0x09),
                control(ControlInstruction::Jalr { rd: 3, rs: 1 }),
            ),
            (
                encode_immediate(0x04, 1, 2, 0x8001),
                control(ControlInstruction::Beq {
                    rs: 1,
                    rt: 2,
                    offset: 0x8001,
                }),
            ),
            (
                encode_immediate(0x05, 1, 2, 0x8001),
                control(ControlInstruction::Bne {
                    rs: 1,
                    rt: 2,
                    offset: 0x8001,
                }),
            ),
            (
                encode_immediate(0x06, 1, 0, 0x8001),
                control(ControlInstruction::Blez {
                    rs: 1,
                    offset: 0x8001,
                }),
            ),
            (
                encode_immediate(0x07, 1, 0, 0x8001),
                control(ControlInstruction::Bgtz {
                    rs: 1,
                    offset: 0x8001,
                }),
            ),
            (
                encode_immediate(0x01, 1, 0x00, 0x8001),
                control(ControlInstruction::Bltz {
                    rs: 1,
                    offset: 0x8001,
                }),
            ),
            (
                encode_immediate(0x01, 1, 0x01, 0x8001),
                control(ControlInstruction::Bgez {
                    rs: 1,
                    offset: 0x8001,
                }),
            ),
            (
                encode_immediate(0x01, 1, 0x10, 0x8001),
                control(ControlInstruction::Bltzal {
                    rs: 1,
                    offset: 0x8001,
                }),
            ),
            (
                encode_immediate(0x01, 1, 0x11, 0x8001),
                control(ControlInstruction::Bgezal {
                    rs: 1,
                    offset: 0x8001,
                }),
            ),
        ];

        for (word, expected) in cases {
            assert_eq!(decode(word), expected);
        }
    }

    #[test]
    fn decodes_every_supported_cp0_instruction() {
        let cases = [
            (
                encode_coprocessor(0x10, 0x00, 0, 31, 0),
                cp0(Cp0Instruction::Mfc0 { rt: 0, rd: 31 }),
            ),
            (
                encode_coprocessor(0x10, 0x02, 31, 0, 0),
                cp0(Cp0Instruction::Cfc0 { rt: 31, rd: 0 }),
            ),
            (
                encode_coprocessor(0x10, 0x04, 1, 12, 0),
                cp0(Cp0Instruction::Mtc0 { rt: 1, rd: 12 }),
            ),
            (
                encode_coprocessor(0x10, 0x06, 2, 13, 0),
                cp0(Cp0Instruction::Ctc0 { rt: 2, rd: 13 }),
            ),
            (
                encode_immediate(0x10, 0x08, 0, 0x8001),
                cp0(Cp0Instruction::Bc0f { offset: 0x8001 }),
            ),
            (
                encode_immediate(0x10, 0x08, 1, 0x7fff),
                cp0(Cp0Instruction::Bc0t { offset: 0x7fff }),
            ),
            (CP0_TLBR, cp0(Cp0Instruction::Tlbr)),
            (CP0_TLBWI, cp0(Cp0Instruction::Tlbwi)),
            (CP0_TLBWR, cp0(Cp0Instruction::Tlbwr)),
            (CP0_TLBP, cp0(Cp0Instruction::Tlbp)),
            (CP0_RFE, cp0(Cp0Instruction::Rfe)),
        ];

        for (word, expected) in cases {
            assert_eq!(decode(word), expected);
        }
    }

    #[test]
    fn decodes_every_supported_cp1_instruction() {
        let cases = [
            (
                encode_coprocessor(0x11, 0x00, 0, 31, 0),
                cp1(Cp1Instruction::Mfc1 { rt: 0, rd: 31 }),
            ),
            (
                encode_coprocessor(0x11, 0x02, 31, 0, 0),
                cp1(Cp1Instruction::Cfc1 { rt: 31, rd: 0 }),
            ),
            (
                encode_coprocessor(0x11, 0x04, 1, 12, 0),
                cp1(Cp1Instruction::Mtc1 { rt: 1, rd: 12 }),
            ),
            (
                encode_coprocessor(0x11, 0x06, 2, 13, 0),
                cp1(Cp1Instruction::Ctc1 { rt: 2, rd: 13 }),
            ),
            (
                encode_immediate(0x11, 0x08, 0, 0x8001),
                cp1(Cp1Instruction::Bc1f { offset: 0x8001 }),
            ),
            (
                encode_immediate(0x11, 0x08, 1, 0x7fff),
                cp1(Cp1Instruction::Bc1t { offset: 0x7fff }),
            ),
        ];

        for (word, expected) in cases {
            assert_eq!(decode(word), expected);
        }
    }

    #[test]
    fn decodes_every_supported_memory_instruction() {
        let cases = [
            (
                0x20,
                MemoryInstruction::Lb {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x24,
                MemoryInstruction::Lbu {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x21,
                MemoryInstruction::Lh {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x25,
                MemoryInstruction::Lhu {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x22,
                MemoryInstruction::Lwl {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x23,
                MemoryInstruction::Lw {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x26,
                MemoryInstruction::Lwr {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x28,
                MemoryInstruction::Sb {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x29,
                MemoryInstruction::Sh {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x2a,
                MemoryInstruction::Swl {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x2b,
                MemoryInstruction::Sw {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x2e,
                MemoryInstruction::Swr {
                    base: 1,
                    rt: 31,
                    offset: 0x8001,
                },
            ),
        ];

        for (opcode, instruction) in cases {
            assert_eq!(
                decode(encode_immediate(opcode, 1, 31, 0x8001)),
                memory(instruction)
            );
        }
    }

    #[test]
    fn decodes_cp1_memory_as_cp1_instructions() {
        let cases = [
            (
                0x31,
                Cp1Instruction::Lwc1 {
                    base: 1,
                    ft: 31,
                    offset: 0x8001,
                },
            ),
            (
                0x39,
                Cp1Instruction::Swc1 {
                    base: 1,
                    ft: 31,
                    offset: 0x8001,
                },
            ),
        ];

        for (opcode, instruction) in cases {
            assert_eq!(
                decode(encode_immediate(opcode, 1, 31, 0x8001)),
                cp1(instruction)
            );
        }
    }

    #[test]
    fn decodes_explicit_exception_instructions_and_ignores_code() {
        let code = 0x0a_bcde;

        assert_eq!(
            decode((code << 6) | 0x0c),
            DecodeResult::Implemented(Instruction::Syscall)
        );
        assert_eq!(
            decode((code << 6) | 0x0d),
            DecodeResult::Implemented(Instruction::Breakpoint)
        );
    }

    #[test]
    fn extracts_all_twenty_six_jump_target_bits() {
        assert_eq!(
            decode(encode_jump(0x02, 0x03ff_ffff)),
            control(ControlInstruction::J {
                target: 0x03ff_ffff,
            })
        );
    }

    #[test]
    fn ignores_fields_unused_by_an_alu_instruction_format() {
        let cases = [
            (
                encode_register(31, 2, 3, 4, 0x00),
                alu(AluInstruction::Sll {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                }),
            ),
            (
                encode_register(31, 2, 3, 4, 0x02),
                alu(AluInstruction::Srl {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                }),
            ),
            (
                encode_register(1, 2, 3, 31, 0x04),
                alu(AluInstruction::Sllv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                }),
            ),
            (
                encode_register(1, 2, 3, 31, 0x06),
                alu(AluInstruction::Srlv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                }),
            ),
            (
                encode_register(31, 30, 3, 29, 0x10),
                alu(AluInstruction::Mfhi { rd: 3 }),
            ),
            (
                encode_register(31, 30, 3, 29, 0x12),
                alu(AluInstruction::Mflo { rd: 3 }),
            ),
            (
                encode_register(1, 31, 0, 30, 0x11),
                alu(AluInstruction::Mthi { rs: 1 }),
            ),
            (
                encode_register(1, 31, 0, 30, 0x13),
                alu(AluInstruction::Mtlo { rs: 1 }),
            ),
            (
                encode_register(1, 2, 0, 31, 0x18),
                alu(AluInstruction::Mult { rs: 1, rt: 2 }),
            ),
            (
                encode_register(1, 2, 0, 31, 0x19),
                alu(AluInstruction::Multu { rs: 1, rt: 2 }),
            ),
            (
                encode_register(1, 2, 0, 31, 0x1a),
                alu(AluInstruction::Div { rs: 1, rt: 2 }),
            ),
            (
                encode_register(1, 2, 0, 31, 0x1b),
                alu(AluInstruction::Divu { rs: 1, rt: 2 }),
            ),
            (
                encode_register(1, 2, 3, 31, 0x21),
                alu(AluInstruction::Addu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                }),
            ),
            (
                encode_immediate(0x0f, 31, 2, 0x8001),
                alu(AluInstruction::Lui {
                    rt: 2,
                    immediate: 0x8001,
                }),
            ),
        ];

        for (word, expected) in cases {
            assert_eq!(decode(word), expected);
        }
    }

    #[test]
    fn classifies_unsupported_coprocessor_encodings() {
        assert_eq!(
            decode(encode_coprocessor(0x11, 0x10, 2, 3, 0x155)),
            DecodeResult::UnsupportedCoprocessor { unit: 1 }
        );

        for opcode in 0x12..=0x13 {
            let unit = opcode as usize - 0x10;
            for selector in [0x00, 0x02, 0x04, 0x06] {
                assert_eq!(
                    decode(encode_coprocessor(opcode, selector, 2, 3, 0)),
                    DecodeResult::UnsupportedCoprocessor { unit }
                );
            }

            for condition in [0x00, 0x01] {
                assert_eq!(
                    decode(encode_immediate(opcode, 0x08, condition, 0x8001)),
                    DecodeResult::UnsupportedCoprocessor { unit }
                );
            }

            assert_eq!(
                decode(encode_coprocessor(opcode, 0x1f, 2, 3, 0x155)),
                DecodeResult::UnsupportedCoprocessor { unit }
            );
        }

        for (opcode, unit) in [(0x32, 2), (0x33, 3), (0x3a, 2), (0x3b, 3)] {
            assert_eq!(
                decode(encode_immediate(opcode, 1, 2, 0x8001)),
                DecodeResult::UnsupportedCoprocessor { unit }
            );
        }
    }

    #[test]
    fn classifies_reserved_encodings() {
        assert_eq!(
            decode(encode_register(1, 2, 3, 0, 0x01)),
            DecodeResult::Reserved
        );
        assert_eq!(
            decode(encode_register(1, 2, 3, 0, 0x08)),
            DecodeResult::Reserved
        );

        for function in [0x11, 0x13, 0x18, 0x19, 0x1a, 0x1b] {
            assert_eq!(
                decode(encode_register(1, 2, 3, 31, function)),
                DecodeResult::Reserved
            );
        }

        assert_eq!(
            decode(encode_immediate(0x06, 1, 1, 0)),
            DecodeResult::Reserved
        );
        assert_eq!(
            decode(encode_immediate(0x07, 1, 31, 0)),
            DecodeResult::Reserved
        );

        for selector in [0x02, 0x03, 0x12, 0x13, 0x1f] {
            assert_eq!(
                decode(encode_immediate(0x01, 1, selector, 0)),
                DecodeResult::Reserved
            );
        }

        for opcode in [0x14, 0x15, 0x16, 0x17] {
            assert_eq!(
                decode(encode_immediate(opcode, 1, 0, 0)),
                DecodeResult::Reserved
            );
        }

        for opcode in [
            0x18, 0x1f, 0x27, 0x2c, 0x2d, 0x2f, 0x30, 0x34, 0x35, 0x36, 0x37, 0x38, 0x3c, 0x3d,
            0x3e, 0x3f,
        ] {
            assert_eq!(
                decode(encode_immediate(opcode, 1, 2, 1)),
                DecodeResult::Reserved
            );
        }

        for selector in [0x00, 0x02, 0x04, 0x06] {
            assert_eq!(
                decode(encode_coprocessor(0x10, selector, 2, 3, 1)),
                DecodeResult::Reserved
            );
        }

        for opcode in 0x10..=0x13 {
            assert_eq!(
                decode(encode_coprocessor(opcode, 0x00, 2, 3, 1)),
                DecodeResult::Reserved
            );
            assert_eq!(
                decode(encode_immediate(opcode, 0x08, 0x02, 0)),
                DecodeResult::Reserved
            );
            assert_eq!(
                decode(encode_coprocessor(opcode, 0x01, 2, 3, 0)),
                DecodeResult::Reserved
            );
        }

        assert_eq!(decode(CP0_TLBR | (1 << 6)), DecodeResult::Reserved);
        assert_eq!(decode(CP0_RFE | (1 << 6)), DecodeResult::Reserved);
        assert_eq!(decode(0x4200_0000), DecodeResult::Reserved);
    }
}
