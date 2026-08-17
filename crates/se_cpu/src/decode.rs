//! Parses raw 32-bit MIPS encodings without consulting execution state.
//!
//! Successful parsing produces [`Instruction`] with register operands and
//! immediate signedness normalized by type. Other words remain separated into
//! positively classified reserved encodings, known valid instructions without
//! semantic handlers, and unclassified encodings. The fallback classification is
//! always [`DecodeGap::UnclassifiedEncoding`].

use crate::gpr::Reg;

const OPCODE_SPECIAL: u8 = 0x00;
const OPCODE_J: u8 = 0x02;
const OPCODE_BEQ: u8 = 0x04;
const OPCODE_BNE: u8 = 0x05;
const OPCODE_ADDIU: u8 = 0x09;
const OPCODE_ORI: u8 = 0x0d;
const OPCODE_LUI: u8 = 0x0f;
const FIRST_RESERVED_PRIMARY_OPCODE: u8 = 0x1c;
const LAST_RESERVED_PRIMARY_OPCODE: u8 = 0x1f;

const FUNCTION_SLL: u8 = 0x00;
const FUNCTION_ADDU: u8 = 0x21;
const FUNCTION_OR: u8 = 0x25;

/// Represents an architectural instruction with encoding details normalized into typed operands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Instruction {
    Sll { rd: Reg, rt: Reg, shift: u8 },
    Addiu { rt: Reg, rs: Reg, immediate: i16 },
    Or { rd: Reg, rs: Reg, rt: Reg },
    Ori { rt: Reg, rs: Reg, immediate: u16 },
    Lui { rt: Reg, immediate: u16 },
    Beq { rs: Reg, rt: Reg, offset: i16 },
    Bne { rs: Reg, rt: Reg, offset: i16 },
    J { index: u32 },
}

/// Classifies a raw word that has no typed semantic handler and is not proven reserved.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodeGap {
    /// The word encodes a known valid instruction without a typed semantic handler.
    ValidButUnimplemented { raw: u32 },
    /// No audited architectural classification exists for the word.
    UnclassifiedEncoding { raw: u32 },
}

/// Separates typed instructions, positively proven reserved words, and implementation gaps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DecodeOutcome {
    /// A decoded typed instruction.
    Instruction(Instruction),
    /// An audited table match proves that `raw` is architecturally reserved.
    ReservedEncoding { raw: u32 },
    /// A non-guest stop that must not be converted into a reserved-instruction exception.
    ImplementationGap(DecodeGap),
}

/// Classifies a raw instruction solely from its encoded bits.
///
/// The fallback is [`DecodeGap::UnclassifiedEncoding`].
/// [`DecodeOutcome::ReservedEncoding`] is returned only for explicit, audited
/// matches.
pub(crate) fn decode(raw: u32) -> DecodeOutcome {
    match opcode(raw) {
        OPCODE_SPECIAL => decode_special(raw),
        OPCODE_J => DecodeOutcome::Instruction(Instruction::J {
            index: jump_index(raw),
        }),
        OPCODE_BEQ => DecodeOutcome::Instruction(Instruction::Beq {
            rs: register(raw, 21),
            rt: register(raw, 16),
            offset: signed_immediate(raw),
        }),
        OPCODE_BNE => DecodeOutcome::Instruction(Instruction::Bne {
            rs: register(raw, 21),
            rt: register(raw, 16),
            offset: signed_immediate(raw),
        }),
        OPCODE_ADDIU => DecodeOutcome::Instruction(Instruction::Addiu {
            rt: register(raw, 16),
            rs: register(raw, 21),
            immediate: signed_immediate(raw),
        }),
        OPCODE_ORI => DecodeOutcome::Instruction(Instruction::Ori {
            rt: register(raw, 16),
            rs: register(raw, 21),
            immediate: unsigned_immediate(raw),
        }),
        OPCODE_LUI => decode_lui(raw),
        FIRST_RESERVED_PRIMARY_OPCODE..=LAST_RESERVED_PRIMARY_OPCODE => {
            // Table A-40 marks every primary opcode in this range as reserved in MIPS IV.
            DecodeOutcome::ReservedEncoding { raw }
        }
        _ => unclassified(raw),
    }
}

fn decode_special(raw: u32) -> DecodeOutcome {
    match function(raw) {
        FUNCTION_SLL if register_field(raw, 21) == 0 => {
            DecodeOutcome::Instruction(Instruction::Sll {
                rd: register(raw, 11),
                rt: register(raw, 16),
                shift: shift_amount(raw),
            })
        }
        FUNCTION_ADDU if shift_amount(raw) == 0 => {
            DecodeOutcome::ImplementationGap(DecodeGap::ValidButUnimplemented { raw })
        }
        FUNCTION_OR if shift_amount(raw) == 0 => DecodeOutcome::Instruction(Instruction::Or {
            rd: register(raw, 11),
            rs: register(raw, 21),
            rt: register(raw, 16),
        }),
        _ => unclassified(raw),
    }
}

fn decode_lui(raw: u32) -> DecodeOutcome {
    if register_field(raw, 21) == 0 {
        DecodeOutcome::Instruction(Instruction::Lui {
            rt: register(raw, 16),
            immediate: unsigned_immediate(raw),
        })
    } else {
        unclassified(raw)
    }
}

fn unclassified(raw: u32) -> DecodeOutcome {
    DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding { raw })
}

fn opcode(raw: u32) -> u8 {
    (raw >> 26) as u8
}

fn function(raw: u32) -> u8 {
    (raw & 0x3f) as u8
}

fn register(raw: u32, shift: u32) -> Reg {
    let index = register_field(raw, shift);
    Reg::new(index).expect("a masked five-bit field must be a valid register")
}

fn register_field(raw: u32, shift: u32) -> u8 {
    ((raw >> shift) & 0x1f) as u8
}

fn shift_amount(raw: u32) -> u8 {
    register_field(raw, 6)
}

fn signed_immediate(raw: u32) -> i16 {
    raw as u16 as i16
}

fn unsigned_immediate(raw: u32) -> u16 {
    raw as u16
}

fn jump_index(raw: u32) -> u32 {
    raw & 0x03ff_ffff
}

#[cfg(test)]
mod tests {
    use super::{DecodeGap, DecodeOutcome, Instruction, decode};
    use crate::gpr::Reg;

    fn reg(index: u8) -> Reg {
        Reg::new(index).expect("test register index must be architectural")
    }

    fn encode_r(rs: u8, rt: u8, rd: u8, shift: u8, function: u8) -> u32 {
        (u32::from(rs) << 21)
            | (u32::from(rt) << 16)
            | (u32::from(rd) << 11)
            | (u32::from(shift) << 6)
            | u32::from(function)
    }

    fn encode_i(opcode: u8, rs: u8, rt: u8, immediate: u16) -> u32 {
        (u32::from(opcode) << 26)
            | (u32::from(rs) << 21)
            | (u32::from(rt) << 16)
            | u32::from(immediate)
    }

    #[test]
    fn canonical_nop_decodes_as_sll() {
        assert_eq!(
            decode(0),
            DecodeOutcome::Instruction(Instruction::Sll {
                rd: Reg::ZERO,
                rt: Reg::ZERO,
                shift: 0,
            })
        );
    }

    #[test]
    fn decodes_sll_operands() {
        let raw = encode_r(0, 4, 5, 17, 0x00);

        assert_eq!(
            decode(raw),
            DecodeOutcome::Instruction(Instruction::Sll {
                rd: reg(5),
                rt: reg(4),
                shift: 17,
            })
        );
    }

    #[test]
    fn decodes_representative_r_instruction() {
        let raw = encode_r(3, 4, 5, 0, 0x25);

        assert_eq!(
            decode(raw),
            DecodeOutcome::Instruction(Instruction::Or {
                rd: reg(5),
                rs: reg(3),
                rt: reg(4),
            })
        );
    }

    #[test]
    fn decodes_typed_signed_and_unsigned_immediates() {
        assert_eq!(
            decode(encode_i(0x09, 1, 2, 0xffff)),
            DecodeOutcome::Instruction(Instruction::Addiu {
                rt: reg(2),
                rs: reg(1),
                immediate: -1,
            })
        );
        assert_eq!(
            decode(encode_i(0x0d, 1, 2, 0xffff)),
            DecodeOutcome::Instruction(Instruction::Ori {
                rt: reg(2),
                rs: reg(1),
                immediate: 0xffff,
            })
        );
    }

    #[test]
    fn decodes_lui_and_both_conditional_branches() {
        assert_eq!(
            decode(encode_i(0x0f, 0, 3, 0x8001)),
            DecodeOutcome::Instruction(Instruction::Lui {
                rt: reg(3),
                immediate: 0x8001,
            })
        );
        assert_eq!(
            decode(encode_i(0x04, 1, 2, 0xfffc)),
            DecodeOutcome::Instruction(Instruction::Beq {
                rs: reg(1),
                rt: reg(2),
                offset: -4,
            })
        );
        assert_eq!(
            decode(encode_i(0x05, 4, 5, 7)),
            DecodeOutcome::Instruction(Instruction::Bne {
                rs: reg(4),
                rt: reg(5),
                offset: 7,
            })
        );
    }

    #[test]
    fn decodes_representative_j_instruction() {
        let index = 0x0123_4567;
        let raw = (0x02_u32 << 26) | index;

        assert_eq!(
            decode(raw),
            DecodeOutcome::Instruction(Instruction::J { index })
        );
    }

    #[test]
    fn audited_reserved_classification_preserves_raw() {
        let raw = 0x1c_u32 << 26;

        assert_eq!(decode(raw), DecodeOutcome::ReservedEncoding { raw });
    }

    #[test]
    fn audited_valid_instruction_is_distinct_from_reserved() {
        let raw = encode_r(1, 2, 3, 0, 0x21);

        assert_eq!(
            decode(raw),
            DecodeOutcome::ImplementationGap(DecodeGap::ValidButUnimplemented { raw })
        );
    }

    #[test]
    fn default_unknown_is_unclassified() {
        let raw = 0x10_u32 << 26;

        assert_eq!(
            decode(raw),
            DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding { raw })
        );
    }

    #[test]
    fn unmatched_fixed_fields_are_not_guessed_to_be_reserved() {
        let raw = encode_r(1, 2, 3, 0, 0x00);

        assert_eq!(
            decode(raw),
            DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding { raw })
        );
    }

    #[test]
    fn reserved_primary_range_has_exact_boundaries() {
        let cases = [
            (
                0x1b_u32 << 26,
                DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding {
                    raw: 0x1b_u32 << 26,
                }),
            ),
            (
                0x1c_u32 << 26,
                DecodeOutcome::ReservedEncoding {
                    raw: 0x1c_u32 << 26,
                },
            ),
            (
                0x1d_u32 << 26,
                DecodeOutcome::ReservedEncoding {
                    raw: 0x1d_u32 << 26,
                },
            ),
            (
                0x1e_u32 << 26,
                DecodeOutcome::ReservedEncoding {
                    raw: 0x1e_u32 << 26,
                },
            ),
            (
                0x1f_u32 << 26,
                DecodeOutcome::ReservedEncoding {
                    raw: 0x1f_u32 << 26,
                },
            ),
            (
                0x20_u32 << 26,
                DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding {
                    raw: 0x20_u32 << 26,
                }),
            ),
        ];

        for (raw, expected) in cases {
            assert_eq!(decode(raw), expected);
        }
    }
}
