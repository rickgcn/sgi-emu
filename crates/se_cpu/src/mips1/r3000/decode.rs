#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Instruction {
    Alu(AluInstruction),
    Control(ControlInstruction),
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
    Addu {
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
    Beq { rs: usize, rt: usize, offset: u16 },
    Bne { rs: usize, rt: usize, offset: u16 },
    Blez { rs: usize, offset: u16 },
    Bgtz { rs: usize, offset: u16 },
    Bltz { rs: usize, offset: u16 },
    Bgez { rs: usize, offset: u16 },
    Bltzal { rs: usize, offset: u16 },
    Bgezal { rs: usize, offset: u16 },
}

pub(super) fn decode(word: u32) -> Option<Instruction> {
    match opcode(word) {
        0x00 => decode_special(word).map(Instruction::Alu),
        0x01 => decode_regimm(word).map(Instruction::Control),
        0x02 => Some(Instruction::Control(ControlInstruction::J {
            target: target(word),
        })),
        0x03 => Some(Instruction::Control(ControlInstruction::Jal {
            target: target(word),
        })),
        0x04 => Some(Instruction::Control(ControlInstruction::Beq {
            rs: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x05 => Some(Instruction::Control(ControlInstruction::Bne {
            rs: rs(word),
            rt: rt(word),
            offset: immediate(word),
        })),
        0x06 if rt(word) == 0 => Some(Instruction::Control(ControlInstruction::Blez {
            rs: rs(word),
            offset: immediate(word),
        })),
        0x07 if rt(word) == 0 => Some(Instruction::Control(ControlInstruction::Bgtz {
            rs: rs(word),
            offset: immediate(word),
        })),
        0x09 => Some(Instruction::Alu(AluInstruction::Addiu {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0a => Some(Instruction::Alu(AluInstruction::Slti {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0b => Some(Instruction::Alu(AluInstruction::Sltiu {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0c => Some(Instruction::Alu(AluInstruction::Andi {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0d => Some(Instruction::Alu(AluInstruction::Ori {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0e => Some(Instruction::Alu(AluInstruction::Xori {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        })),
        0x0f => Some(Instruction::Alu(AluInstruction::Lui {
            rt: rt(word),
            immediate: immediate(word),
        })),
        _ => None,
    }
}

fn decode_special(word: u32) -> Option<AluInstruction> {
    match function(word) {
        0x00 => Some(AluInstruction::Sll {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        }),
        0x02 => Some(AluInstruction::Srl {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        }),
        0x03 => Some(AluInstruction::Sra {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        }),
        0x04 => Some(AluInstruction::Sllv {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        }),
        0x06 => Some(AluInstruction::Srlv {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        }),
        0x07 => Some(AluInstruction::Srav {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        }),
        0x21 => Some(AluInstruction::Addu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x23 => Some(AluInstruction::Subu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x24 => Some(AluInstruction::And {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x25 => Some(AluInstruction::Or {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x26 => Some(AluInstruction::Xor {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x27 => Some(AluInstruction::Nor {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x2a => Some(AluInstruction::Slt {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x2b => Some(AluInstruction::Sltu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        _ => None,
    }
}

fn decode_regimm(word: u32) -> Option<ControlInstruction> {
    match rt(word) {
        0x00 => Some(ControlInstruction::Bltz {
            rs: rs(word),
            offset: immediate(word),
        }),
        0x01 => Some(ControlInstruction::Bgez {
            rs: rs(word),
            offset: immediate(word),
        }),
        0x10 => Some(ControlInstruction::Bltzal {
            rs: rs(word),
            offset: immediate(word),
        }),
        0x11 => Some(ControlInstruction::Bgezal {
            rs: rs(word),
            offset: immediate(word),
        }),
        _ => None,
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
    use super::{AluInstruction, ControlInstruction, Instruction, decode};

    fn encode_register(rs: u32, rt: u32, rd: u32, shift_amount: u32, function: u32) -> u32 {
        (rs << 21) | (rt << 16) | (rd << 11) | (shift_amount << 6) | function
    }

    fn encode_immediate(opcode: u32, rs: u32, rt: u32, immediate: u16) -> u32 {
        (opcode << 26) | (rs << 21) | (rt << 16) | u32::from(immediate)
    }

    fn encode_jump(opcode: u32, target: u32) -> u32 {
        (opcode << 26) | target
    }

    fn alu(instruction: AluInstruction) -> Instruction {
        Instruction::Alu(instruction)
    }

    fn control(instruction: ControlInstruction) -> Instruction {
        Instruction::Control(instruction)
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
                encode_register(1, 2, 3, 0, 0x21),
                alu(AluInstruction::Addu {
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
            assert_eq!(decode(word), Some(expected));
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
            assert_eq!(decode(word), Some(expected));
        }
    }

    #[test]
    fn extracts_all_twenty_six_jump_target_bits() {
        assert_eq!(
            decode(encode_jump(0x02, 0x03ff_ffff)),
            Some(control(ControlInstruction::J {
                target: 0x03ff_ffff,
            }))
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
            assert_eq!(decode(word), Some(expected));
        }
    }

    #[test]
    fn rejects_invalid_control_fields_and_later_branch_encodings() {
        assert_eq!(decode(encode_immediate(0x06, 1, 1, 0)), None);
        assert_eq!(decode(encode_immediate(0x07, 1, 31, 0)), None);

        for selector in [0x02, 0x03, 0x12, 0x13, 0x1f] {
            assert_eq!(decode(encode_immediate(0x01, 1, selector, 0)), None);
        }

        for opcode in [0x14, 0x15, 0x16, 0x17] {
            assert_eq!(decode(encode_immediate(opcode, 1, 0, 0)), None);
        }
    }

    #[test]
    fn rejects_unknown_and_unimplemented_encodings() {
        assert_eq!(decode(encode_register(1, 2, 3, 0, 0x01)), None);
        assert_eq!(decode(encode_immediate(0x08, 1, 2, 1)), None);
        assert_eq!(decode(encode_immediate(0x3f, 1, 2, 1)), None);
    }
}
