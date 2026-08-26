#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Instruction {
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

pub(super) fn decode(word: u32) -> Option<Instruction> {
    match opcode(word) {
        0x00 => decode_special(word),
        0x09 => Some(Instruction::Addiu {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        }),
        0x0a => Some(Instruction::Slti {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        }),
        0x0b => Some(Instruction::Sltiu {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        }),
        0x0c => Some(Instruction::Andi {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        }),
        0x0d => Some(Instruction::Ori {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        }),
        0x0e => Some(Instruction::Xori {
            rt: rt(word),
            rs: rs(word),
            immediate: immediate(word),
        }),
        0x0f => Some(Instruction::Lui {
            rt: rt(word),
            immediate: immediate(word),
        }),
        _ => None,
    }
}

fn decode_special(word: u32) -> Option<Instruction> {
    match function(word) {
        0x00 => Some(Instruction::Sll {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        }),
        0x02 => Some(Instruction::Srl {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        }),
        0x03 => Some(Instruction::Sra {
            rd: rd(word),
            rt: rt(word),
            shift_amount: shift_amount(word),
        }),
        0x04 => Some(Instruction::Sllv {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        }),
        0x06 => Some(Instruction::Srlv {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        }),
        0x07 => Some(Instruction::Srav {
            rd: rd(word),
            rt: rt(word),
            rs: rs(word),
        }),
        0x21 => Some(Instruction::Addu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x23 => Some(Instruction::Subu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x24 => Some(Instruction::And {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x25 => Some(Instruction::Or {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x26 => Some(Instruction::Xor {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x27 => Some(Instruction::Nor {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x2a => Some(Instruction::Slt {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
        }),
        0x2b => Some(Instruction::Sltu {
            rd: rd(word),
            rs: rs(word),
            rt: rt(word),
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

#[cfg(test)]
mod tests {
    use super::{Instruction, decode};

    fn encode_register(rs: u32, rt: u32, rd: u32, shift_amount: u32, function: u32) -> u32 {
        (rs << 21) | (rt << 16) | (rd << 11) | (shift_amount << 6) | function
    }

    fn encode_immediate(opcode: u32, rs: u32, rt: u32, immediate: u16) -> u32 {
        (opcode << 26) | (rs << 21) | (rt << 16) | u32::from(immediate)
    }

    #[test]
    fn decodes_every_supported_instruction() {
        let cases = [
            (
                encode_register(0, 2, 3, 4, 0x00),
                Instruction::Sll {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                },
            ),
            (
                encode_register(0, 2, 3, 4, 0x02),
                Instruction::Srl {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                },
            ),
            (
                encode_register(0, 2, 3, 4, 0x03),
                Instruction::Sra {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x04),
                Instruction::Sllv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x06),
                Instruction::Srlv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x07),
                Instruction::Srav {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x21),
                Instruction::Addu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x23),
                Instruction::Subu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x24),
                Instruction::And {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x25),
                Instruction::Or {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x26),
                Instruction::Xor {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x27),
                Instruction::Nor {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x2a),
                Instruction::Slt {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_register(1, 2, 3, 0, 0x2b),
                Instruction::Sltu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_immediate(0x09, 1, 2, 0x8001),
                Instruction::Addiu {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
            ),
            (
                encode_immediate(0x0a, 1, 2, 0x8001),
                Instruction::Slti {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
            ),
            (
                encode_immediate(0x0b, 1, 2, 0x8001),
                Instruction::Sltiu {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
            ),
            (
                encode_immediate(0x0c, 1, 2, 0x8001),
                Instruction::Andi {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
            ),
            (
                encode_immediate(0x0d, 1, 2, 0x8001),
                Instruction::Ori {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
            ),
            (
                encode_immediate(0x0e, 1, 2, 0x8001),
                Instruction::Xori {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
            ),
            (
                encode_immediate(0x0f, 0, 2, 0x8001),
                Instruction::Lui {
                    rt: 2,
                    immediate: 0x8001,
                },
            ),
        ];

        for (word, expected) in cases {
            assert_eq!(decode(word), Some(expected));
        }
    }

    #[test]
    fn ignores_fields_unused_by_an_instruction_format() {
        let cases = [
            (
                encode_register(31, 2, 3, 4, 0x00),
                Instruction::Sll {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                },
            ),
            (
                encode_register(31, 2, 3, 4, 0x02),
                Instruction::Srl {
                    rd: 3,
                    rt: 2,
                    shift_amount: 4,
                },
            ),
            (
                encode_register(1, 2, 3, 31, 0x04),
                Instruction::Sllv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                },
            ),
            (
                encode_register(1, 2, 3, 31, 0x06),
                Instruction::Srlv {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                },
            ),
            (
                encode_register(1, 2, 3, 31, 0x21),
                Instruction::Addu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
            ),
            (
                encode_immediate(0x0f, 31, 2, 0x8001),
                Instruction::Lui {
                    rt: 2,
                    immediate: 0x8001,
                },
            ),
        ];

        for (word, expected) in cases {
            assert_eq!(decode(word), Some(expected));
        }
    }

    #[test]
    fn rejects_unknown_and_unimplemented_encodings() {
        assert_eq!(decode(encode_register(1, 2, 3, 0, 0x01)), None);
        assert_eq!(decode(encode_immediate(0x08, 1, 2, 1)), None);
        assert_eq!(decode(encode_immediate(0x3f, 1, 2, 1)), None);
    }
}
