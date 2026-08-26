use super::{decode::AluInstruction, state::State};

pub(super) fn execute(state: &mut State, instruction: AluInstruction) {
    match instruction {
        AluInstruction::Sll {
            rd,
            rt,
            shift_amount,
        } => {
            let value = state.read_gpr(rt) << shift_amount;
            state.write_gpr(rd, value);
        }
        AluInstruction::Srl {
            rd,
            rt,
            shift_amount,
        } => {
            let value = state.read_gpr(rt) >> shift_amount;
            state.write_gpr(rd, value);
        }
        AluInstruction::Sra {
            rd,
            rt,
            shift_amount,
        } => {
            let value = ((state.read_gpr(rt) as i32) >> shift_amount) as u32;
            state.write_gpr(rd, value);
        }
        AluInstruction::Sllv { rd, rt, rs } => {
            let shift_amount = state.read_gpr(rs) & 0x1f;
            let value = state.read_gpr(rt) << shift_amount;
            state.write_gpr(rd, value);
        }
        AluInstruction::Srlv { rd, rt, rs } => {
            let shift_amount = state.read_gpr(rs) & 0x1f;
            let value = state.read_gpr(rt) >> shift_amount;
            state.write_gpr(rd, value);
        }
        AluInstruction::Srav { rd, rt, rs } => {
            let shift_amount = state.read_gpr(rs) & 0x1f;
            let value = ((state.read_gpr(rt) as i32) >> shift_amount) as u32;
            state.write_gpr(rd, value);
        }
        AluInstruction::Addu { rd, rs, rt } => {
            let value = state.read_gpr(rs).wrapping_add(state.read_gpr(rt));
            state.write_gpr(rd, value);
        }
        AluInstruction::Subu { rd, rs, rt } => {
            let value = state.read_gpr(rs).wrapping_sub(state.read_gpr(rt));
            state.write_gpr(rd, value);
        }
        AluInstruction::And { rd, rs, rt } => {
            let value = state.read_gpr(rs) & state.read_gpr(rt);
            state.write_gpr(rd, value);
        }
        AluInstruction::Or { rd, rs, rt } => {
            let value = state.read_gpr(rs) | state.read_gpr(rt);
            state.write_gpr(rd, value);
        }
        AluInstruction::Xor { rd, rs, rt } => {
            let value = state.read_gpr(rs) ^ state.read_gpr(rt);
            state.write_gpr(rd, value);
        }
        AluInstruction::Nor { rd, rs, rt } => {
            let value = !(state.read_gpr(rs) | state.read_gpr(rt));
            state.write_gpr(rd, value);
        }
        AluInstruction::Slt { rd, rs, rt } => {
            let value = u32::from((state.read_gpr(rs) as i32) < (state.read_gpr(rt) as i32));
            state.write_gpr(rd, value);
        }
        AluInstruction::Sltu { rd, rs, rt } => {
            let value = u32::from(state.read_gpr(rs) < state.read_gpr(rt));
            state.write_gpr(rd, value);
        }
        AluInstruction::Addiu { rt, rs, immediate } => {
            let value = state
                .read_gpr(rs)
                .wrapping_add(sign_extend_immediate(immediate));
            state.write_gpr(rt, value);
        }
        AluInstruction::Slti { rt, rs, immediate } => {
            let value =
                u32::from((state.read_gpr(rs) as i32) < (sign_extend_immediate(immediate) as i32));
            state.write_gpr(rt, value);
        }
        AluInstruction::Sltiu { rt, rs, immediate } => {
            let value = u32::from(state.read_gpr(rs) < sign_extend_immediate(immediate));
            state.write_gpr(rt, value);
        }
        AluInstruction::Andi { rt, rs, immediate } => {
            let value = state.read_gpr(rs) & u32::from(immediate);
            state.write_gpr(rt, value);
        }
        AluInstruction::Ori { rt, rs, immediate } => {
            let value = state.read_gpr(rs) | u32::from(immediate);
            state.write_gpr(rt, value);
        }
        AluInstruction::Xori { rt, rs, immediate } => {
            let value = state.read_gpr(rs) ^ u32::from(immediate);
            state.write_gpr(rt, value);
        }
        AluInstruction::Lui { rt, immediate } => {
            state.write_gpr(rt, u32::from(immediate) << 16);
        }
    }
}

fn sign_extend_immediate(immediate: u16) -> u32 {
    i32::from(immediate as i16) as u32
}

#[cfg(test)]
mod tests {
    use super::{AluInstruction as Instruction, State, execute};

    fn run(instruction: Instruction, registers: &[(usize, u32)], destination: usize) -> u32 {
        let mut state = State::new();
        for &(index, value) in registers {
            state.write_gpr(index, value);
        }

        execute(&mut state, instruction);

        state.read_gpr(destination)
    }

    #[test]
    fn immediate_shifts_cover_zero_and_thirty_one() {
        assert_eq!(
            run(
                Instruction::Sll {
                    rd: 2,
                    rt: 1,
                    shift_amount: 0,
                },
                &[(1, 1)],
                2,
            ),
            1
        );
        assert_eq!(
            run(
                Instruction::Sll {
                    rd: 2,
                    rt: 1,
                    shift_amount: 31,
                },
                &[(1, 1)],
                2,
            ),
            0x8000_0000
        );
        assert_eq!(
            run(
                Instruction::Srl {
                    rd: 2,
                    rt: 1,
                    shift_amount: 0,
                },
                &[(1, 0x8000_0000)],
                2,
            ),
            0x8000_0000
        );
        assert_eq!(
            run(
                Instruction::Srl {
                    rd: 2,
                    rt: 1,
                    shift_amount: 31,
                },
                &[(1, 0x8000_0000)],
                2,
            ),
            1
        );
        assert_eq!(
            run(
                Instruction::Sra {
                    rd: 2,
                    rt: 1,
                    shift_amount: 0,
                },
                &[(1, 0x8000_0000)],
                2,
            ),
            0x8000_0000
        );
        assert_eq!(
            run(
                Instruction::Sra {
                    rd: 2,
                    rt: 1,
                    shift_amount: 31,
                },
                &[(1, 0x8000_0000)],
                2,
            ),
            u32::MAX
        );
        assert_eq!(
            run(
                Instruction::Sra {
                    rd: 2,
                    rt: 1,
                    shift_amount: 31,
                },
                &[(1, 0x7fff_ffff)],
                2,
            ),
            0
        );
    }

    #[test]
    fn variable_shifts_mask_the_amount_to_five_bits() {
        let cases = [
            (0, 1, 0x8000_0000, 0x8000_0000),
            (31, 0x8000_0000, 1, u32::MAX),
            (32, 1, 0x8000_0000, 0x8000_0000),
            (63, 0x8000_0000, 1, u32::MAX),
        ];

        for (shift_amount, sll, srl, sra) in cases {
            assert_eq!(
                run(
                    Instruction::Sllv {
                        rd: 3,
                        rt: 2,
                        rs: 1,
                    },
                    &[(1, shift_amount), (2, 1)],
                    3,
                ),
                sll
            );
            assert_eq!(
                run(
                    Instruction::Srlv {
                        rd: 3,
                        rt: 2,
                        rs: 1,
                    },
                    &[(1, shift_amount), (2, 0x8000_0000)],
                    3,
                ),
                srl
            );
            assert_eq!(
                run(
                    Instruction::Srav {
                        rd: 3,
                        rt: 2,
                        rs: 1,
                    },
                    &[(1, shift_amount), (2, 0x8000_0000)],
                    3,
                ),
                sra
            );
        }

        assert_eq!(
            run(
                Instruction::Srav {
                    rd: 3,
                    rt: 2,
                    rs: 1,
                },
                &[(1, 31), (2, 0x7fff_ffff)],
                3,
            ),
            0
        );
    }

    #[test]
    fn arithmetic_operations_wrap_without_overflow_exceptions() {
        assert_eq!(
            run(
                Instruction::Addu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &[(1, u32::MAX), (2, 1)],
                3,
            ),
            0
        );
        assert_eq!(
            run(
                Instruction::Subu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &[(1, 0), (2, 1)],
                3,
            ),
            u32::MAX
        );
        assert_eq!(
            run(
                Instruction::Addiu {
                    rt: 2,
                    rs: 1,
                    immediate: 1,
                },
                &[(1, 0x7fff_ffff)],
                2,
            ),
            0x8000_0000
        );
    }

    #[test]
    fn register_logic_operations_produce_expected_bits() {
        let registers = [(1, 0x0000_f0f0), (2, 0x0000_0ff0)];

        assert_eq!(
            run(
                Instruction::And {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &registers,
                3,
            ),
            0x0000_00f0
        );
        assert_eq!(
            run(
                Instruction::Or {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &registers,
                3,
            ),
            0x0000_fff0
        );
        assert_eq!(
            run(
                Instruction::Xor {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &registers,
                3,
            ),
            0x0000_ff00
        );
        assert_eq!(
            run(
                Instruction::Nor {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &registers,
                3,
            ),
            0xffff_000f
        );
    }

    #[test]
    fn signed_and_unsigned_comparisons_are_distinct() {
        let registers = [(1, u32::MAX), (2, 1)];

        assert_eq!(
            run(
                Instruction::Slt {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &registers,
                3,
            ),
            1
        );
        assert_eq!(
            run(
                Instruction::Sltu {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &registers,
                3,
            ),
            0
        );
    }

    #[test]
    fn signed_immediates_cover_sign_extension_boundaries() {
        for (immediate, expected) in [
            (0x7fff, 0x0000_7fff),
            (0x8000, 0xffff_8000),
            (0xffff, u32::MAX),
        ] {
            assert_eq!(
                run(
                    Instruction::Addiu {
                        rt: 1,
                        rs: 0,
                        immediate,
                    },
                    &[],
                    1,
                ),
                expected
            );
        }

        for (immediate, expected) in [(0x7fff, 1), (0x8000, 0), (0xffff, 0)] {
            assert_eq!(
                run(
                    Instruction::Slti {
                        rt: 2,
                        rs: 1,
                        immediate,
                    },
                    &[(1, 0)],
                    2,
                ),
                expected
            );
        }

        for (immediate, expected) in [(0x7fff, 0), (0x8000, 1), (0xffff, 1)] {
            assert_eq!(
                run(
                    Instruction::Sltiu {
                        rt: 2,
                        rs: 1,
                        immediate,
                    },
                    &[(1, 0x0001_0000)],
                    2,
                ),
                expected
            );
        }
    }

    #[test]
    fn logical_immediates_are_zero_extended() {
        assert_eq!(
            run(
                Instruction::Andi {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
                &[(1, u32::MAX)],
                2,
            ),
            0x0000_8001
        );
        assert_eq!(
            run(
                Instruction::Ori {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
                &[(1, 0xffff_0000)],
                2,
            ),
            0xffff_8001
        );
        assert_eq!(
            run(
                Instruction::Xori {
                    rt: 2,
                    rs: 1,
                    immediate: 0x8001,
                },
                &[(1, 0xffff_0000)],
                2,
            ),
            0xffff_8001
        );
    }

    #[test]
    fn load_upper_immediate_places_bits_in_the_upper_half() {
        assert_eq!(
            run(
                Instruction::Lui {
                    rt: 1,
                    immediate: 0x8001,
                },
                &[],
                1,
            ),
            0x8001_0000
        );
    }

    #[test]
    fn execution_supports_aliasing_and_preserves_pc_and_register_zero() {
        let mut state = State::new();
        state.write_gpr(1, 5);
        state.write_gpr(2, 7);
        let pc = state.pc();

        execute(
            &mut state,
            Instruction::Addu {
                rd: 1,
                rs: 1,
                rt: 2,
            },
        );
        execute(
            &mut state,
            Instruction::Or {
                rd: 0,
                rs: 1,
                rt: 2,
            },
        );

        assert_eq!(state.read_gpr(1), 12);
        assert_eq!(state.read_gpr(0), 0);
        assert_eq!(state.pc(), pc);
    }
}
