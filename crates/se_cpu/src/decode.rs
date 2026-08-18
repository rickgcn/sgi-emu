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
const OPCODE_COP0: u8 = 0x10;
const OPCODE_LW: u8 = 0x23;
const OPCODE_SW: u8 = 0x2b;
const FIRST_RESERVED_PRIMARY_OPCODE: u8 = 0x1c;
const LAST_RESERVED_PRIMARY_OPCODE: u8 = 0x1f;

const FUNCTION_SLL: u8 = 0x00;
const FUNCTION_SYSCALL: u8 = 0x0c;
const FUNCTION_BREAK: u8 = 0x0d;
const FUNCTION_ADD: u8 = 0x20;
const FUNCTION_ADDU: u8 = 0x21;
const FUNCTION_OR: u8 = 0x25;
const ERET_ENCODING: u32 = 0x4200_0018;
const TLBWI_ENCODING: u32 = 0x4200_0002;
const COP0_MFC0: u8 = 0;
const COP0_MTC0: u8 = 4;

/// Identifies one five-bit CP0 register number encoded by a move instruction.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Cp0Register(u8);

impl Cp0Register {
    /// The indexed TLB-write target register.
    pub(crate) const INDEX: Self = Self(0);
    /// The even-page TLB staging register.
    pub(crate) const ENTRY_LO0: Self = Self(2);
    /// The odd-page TLB staging register.
    pub(crate) const ENTRY_LO1: Self = Self(3);
    /// The 32-bit page-table pointer register.
    pub(crate) const CONTEXT: Self = Self(4);
    /// The processor status register.
    pub(crate) const STATUS: Self = Self(12);

    const fn from_field(value: u8) -> Self {
        Self(value & 0x1f)
    }
}

/// Represents an architectural instruction with encoding details normalized into typed operands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Instruction {
    Sll { rd: Reg, rt: Reg, shift: u8 },
    Add { rd: Reg, rs: Reg, rt: Reg },
    Addiu { rt: Reg, rs: Reg, immediate: i16 },
    Or { rd: Reg, rs: Reg, rt: Reg },
    Ori { rt: Reg, rs: Reg, immediate: u16 },
    Lui { rt: Reg, immediate: u16 },
    Beq { rs: Reg, rt: Reg, offset: i16 },
    Bne { rs: Reg, rt: Reg, offset: i16 },
    J { index: u32 },
    Lw { rt: Reg, base: Reg, immediate: i16 },
    Sw { rt: Reg, base: Reg, immediate: i16 },
    Syscall { code: u32 },
    Break { code: u32 },
    Mfc0 { rt: Reg, register: Cp0Register },
    Mtc0 { rt: Reg, register: Cp0Register },
    Tlbwi,
    Eret,
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
        OPCODE_COP0 => decode_cop0(raw),
        OPCODE_LW => DecodeOutcome::Instruction(Instruction::Lw {
            rt: register(raw, 16),
            base: register(raw, 21),
            immediate: signed_immediate(raw),
        }),
        OPCODE_SW => DecodeOutcome::Instruction(Instruction::Sw {
            rt: register(raw, 16),
            base: register(raw, 21),
            immediate: signed_immediate(raw),
        }),
        FIRST_RESERVED_PRIMARY_OPCODE..=LAST_RESERVED_PRIMARY_OPCODE => {
            // Table A-40 marks every primary opcode in this range as reserved in MIPS IV.
            DecodeOutcome::ReservedEncoding { raw }
        }
        _ => unclassified(raw),
    }
}

fn decode_cop0(raw: u32) -> DecodeOutcome {
    match raw {
        ERET_ENCODING => DecodeOutcome::Instruction(Instruction::Eret),
        TLBWI_ENCODING => DecodeOutcome::Instruction(Instruction::Tlbwi),
        _ if raw & 0x7ff == 0 => match register_field(raw, 21) {
            COP0_MFC0 => DecodeOutcome::Instruction(Instruction::Mfc0 {
                rt: register(raw, 16),
                register: Cp0Register::from_field(register_field(raw, 11)),
            }),
            COP0_MTC0 => DecodeOutcome::Instruction(Instruction::Mtc0 {
                rt: register(raw, 16),
                register: Cp0Register::from_field(register_field(raw, 11)),
            }),
            _ => unclassified(raw),
        },
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
        FUNCTION_SYSCALL => DecodeOutcome::Instruction(Instruction::Syscall {
            code: special_code(raw),
        }),
        FUNCTION_BREAK => DecodeOutcome::Instruction(Instruction::Break {
            code: special_code(raw),
        }),
        FUNCTION_ADD if shift_amount(raw) == 0 => DecodeOutcome::Instruction(Instruction::Add {
            rd: register(raw, 11),
            rs: register(raw, 21),
            rt: register(raw, 16),
        }),
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

fn special_code(raw: u32) -> u32 {
    (raw >> 6) & 0x000f_ffff
}

#[cfg(test)]
mod tests {
    use super::{
        Cp0Register, DecodeGap, DecodeOutcome, ERET_ENCODING, Instruction, TLBWI_ENCODING, decode,
    };
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

    fn encode_special_code(code: u32, function: u8) -> u32 {
        ((code & 0x000f_ffff) << 6) | u32::from(function)
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
    fn decodes_add_with_its_fixed_shift_field() {
        let raw = encode_r(3, 4, 5, 0, 0x20);

        assert_eq!(
            decode(raw),
            DecodeOutcome::Instruction(Instruction::Add {
                rd: reg(5),
                rs: reg(3),
                rt: reg(4),
            })
        );
    }

    #[test]
    fn decodes_syscall_and_break_code_fields() {
        assert_eq!(
            decode(encode_special_code(0xabcde, 0x0c)),
            DecodeOutcome::Instruction(Instruction::Syscall { code: 0xabcde })
        );
        assert_eq!(
            decode(encode_special_code(0x54321, 0x0d)),
            DecodeOutcome::Instruction(Instruction::Break { code: 0x54321 })
        );
    }

    #[test]
    fn decodes_only_the_exact_eret_encoding() {
        assert_eq!(
            decode(ERET_ENCODING),
            DecodeOutcome::Instruction(Instruction::Eret)
        );

        for raw in [ERET_ENCODING ^ 1, ERET_ENCODING ^ (1 << 6)] {
            assert_eq!(
                decode(raw),
                DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding { raw })
            );
        }
    }

    #[test]
    fn decodes_exact_cp0_moves_and_tlbwi() {
        let mfc0 = (0x10_u32 << 26) | (2 << 16) | (4 << 11);
        let mtc0 = (0x10_u32 << 26) | (4 << 21) | (3 << 16) | (2 << 11);

        assert_eq!(
            decode(mfc0),
            DecodeOutcome::Instruction(Instruction::Mfc0 {
                rt: reg(2),
                register: Cp0Register::CONTEXT,
            })
        );
        assert_eq!(
            decode(mtc0),
            DecodeOutcome::Instruction(Instruction::Mtc0 {
                rt: reg(3),
                register: Cp0Register::ENTRY_LO0,
            })
        );
        assert_eq!(
            decode(TLBWI_ENCODING),
            DecodeOutcome::Instruction(Instruction::Tlbwi)
        );
    }

    #[test]
    fn cp0_encodings_with_nonzero_fixed_fields_remain_unclassified() {
        let cases = [
            ((0x10_u32 << 26) | (2 << 16) | (4 << 11)) | 1,
            ((0x10_u32 << 26) | (4 << 21) | (3 << 16) | (2 << 11)) | (1 << 6),
            TLBWI_ENCODING ^ (1 << 6),
        ];

        for raw in cases {
            assert_eq!(
                decode(raw),
                DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding { raw })
            );
        }
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
    fn decodes_lw_and_sw_with_signed_offsets() {
        assert_eq!(
            decode(encode_i(0x23, 3, 4, 0xfffc)),
            DecodeOutcome::Instruction(Instruction::Lw {
                rt: reg(4),
                base: reg(3),
                immediate: -4,
            })
        );
        assert_eq!(
            decode(encode_i(0x2b, 5, 6, 0x7ffc)),
            DecodeOutcome::Instruction(Instruction::Sw {
                rt: reg(6),
                base: reg(5),
                immediate: 0x7ffc,
            })
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
        let raw = (0x10_u32 << 26) | 1;

        assert_eq!(
            decode(raw),
            DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding { raw })
        );
    }

    #[test]
    fn unmatched_fixed_fields_are_not_guessed_to_be_reserved() {
        let cases = [encode_r(1, 2, 3, 0, 0x00), encode_r(1, 2, 3, 1, 0x20)];

        for raw in cases {
            assert_eq!(
                decode(raw),
                DecodeOutcome::ImplementationGap(DecodeGap::UnclassifiedEncoding { raw })
            );
        }
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
