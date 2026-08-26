use super::{cp0::Exception, decode::AluInstruction, state::State};

pub(super) fn execute(state: &mut State, instruction: AluInstruction) -> Result<(), Exception> {
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
        AluInstruction::Mfhi { rd } => {
            state.write_gpr(rd, state.read_hi());
        }
        AluInstruction::Mthi { rs } => {
            state.write_hi(state.read_gpr(rs));
        }
        AluInstruction::Mflo { rd } => {
            state.write_gpr(rd, state.read_lo());
        }
        AluInstruction::Mtlo { rs } => {
            state.write_lo(state.read_gpr(rs));
        }
        AluInstruction::Mult { rs, rt } => {
            let lhs = i64::from(state.read_gpr(rs) as i32);
            let rhs = i64::from(state.read_gpr(rt) as i32);
            let product = (lhs * rhs) as u64;
            state.write_hi((product >> 32) as u32);
            state.write_lo(product as u32);
        }
        AluInstruction::Multu { rs, rt } => {
            let lhs = u64::from(state.read_gpr(rs));
            let rhs = u64::from(state.read_gpr(rt));
            let product = lhs * rhs;
            state.write_hi((product >> 32) as u32);
            state.write_lo(product as u32);
        }
        AluInstruction::Div { rs, rt } => {
            let dividend = state.read_gpr(rs) as i32;
            let divisor = state.read_gpr(rt) as i32;
            let (hi, lo) = if divisor == 0 {
                let quotient = if dividend >= 0 { u32::MAX } else { 1 };
                (dividend as u32, quotient)
            } else if dividend == i32::MIN && divisor == -1 {
                (0, i32::MIN as u32)
            } else {
                ((dividend % divisor) as u32, (dividend / divisor) as u32)
            };
            state.write_hi(hi);
            state.write_lo(lo);
        }
        AluInstruction::Divu { rs, rt } => {
            let dividend = state.read_gpr(rs);
            let divisor = state.read_gpr(rt);
            let (hi, lo) = if divisor == 0 {
                (dividend, u32::MAX)
            } else {
                (dividend % divisor, dividend / divisor)
            };
            state.write_hi(hi);
            state.write_lo(lo);
        }
        AluInstruction::Add { rd, rs, rt } => {
            let lhs = state.read_gpr(rs) as i32;
            let rhs = state.read_gpr(rt) as i32;
            let (value, overflowed) = lhs.overflowing_add(rhs);
            if overflowed {
                return Err(Exception::Overflow);
            }
            state.write_gpr(rd, value as u32);
        }
        AluInstruction::Addu { rd, rs, rt } => {
            let value = state.read_gpr(rs).wrapping_add(state.read_gpr(rt));
            state.write_gpr(rd, value);
        }
        AluInstruction::Sub { rd, rs, rt } => {
            let lhs = state.read_gpr(rs) as i32;
            let rhs = state.read_gpr(rt) as i32;
            let (value, overflowed) = lhs.overflowing_sub(rhs);
            if overflowed {
                return Err(Exception::Overflow);
            }
            state.write_gpr(rd, value as u32);
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
        AluInstruction::Addi { rt, rs, immediate } => {
            let lhs = state.read_gpr(rs) as i32;
            let rhs = i32::from(immediate as i16);
            let (value, overflowed) = lhs.overflowing_add(rhs);
            if overflowed {
                return Err(Exception::Overflow);
            }
            state.write_gpr(rt, value as u32);
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

    Ok(())
}

fn sign_extend_immediate(immediate: u16) -> u32 {
    i32::from(immediate as i16) as u32
}

#[cfg(test)]
mod tests {
    use super::{AluInstruction as Instruction, Exception, State, execute};

    fn run(instruction: Instruction, registers: &[(usize, u32)], destination: usize) -> u32 {
        let mut state = State::new();
        for &(index, value) in registers {
            state.write_gpr(index, value);
        }

        execute(&mut state, instruction).expect("instruction should not trap");

        state.read_gpr(destination)
    }

    fn run_hilo(instruction: Instruction, registers: &[(usize, u32)]) -> (u32, u32) {
        let mut state = State::new();
        for &(index, value) in registers {
            state.write_gpr(index, value);
        }

        execute(&mut state, instruction).expect("instruction should not trap");

        (state.read_hi(), state.read_lo())
    }

    fn assert_overflow(instruction: Instruction, registers: &[(usize, u32)], destination: usize) {
        let sentinel = 0xdead_beef;
        let mut state = State::new();
        for &(index, value) in registers {
            state.write_gpr(index, value);
        }
        state.write_gpr(destination, sentinel);

        let result = execute(&mut state, instruction);

        assert_eq!(result, Err(Exception::Overflow));
        assert_eq!(state.read_gpr(destination), sentinel);
    }

    #[test]
    fn hi_lo_transfers_preserve_the_companion_register() {
        let mut state = State::new();
        state.write_gpr(1, 0x1234_5678);
        state.write_gpr(2, 0x89ab_cdef);
        state.write_hi(0xaaaa_aaaa);
        state.write_lo(0xbbbb_bbbb);

        execute(&mut state, Instruction::Mthi { rs: 1 }).expect("MTHI should not trap");
        assert_eq!(state.read_hi(), 0x1234_5678);
        assert_eq!(state.read_lo(), 0xbbbb_bbbb);

        execute(&mut state, Instruction::Mtlo { rs: 2 }).expect("MTLO should not trap");
        assert_eq!(state.read_hi(), 0x1234_5678);
        assert_eq!(state.read_lo(), 0x89ab_cdef);

        execute(&mut state, Instruction::Mfhi { rd: 3 }).expect("MFHI should not trap");
        execute(&mut state, Instruction::Mflo { rd: 4 }).expect("MFLO should not trap");
        execute(&mut state, Instruction::Mfhi { rd: 0 }).expect("MFHI should not trap");
        execute(&mut state, Instruction::Mflo { rd: 0 }).expect("MFLO should not trap");

        assert_eq!(state.read_gpr(3), 0x1234_5678);
        assert_eq!(state.read_gpr(4), 0x89ab_cdef);
        assert_eq!(state.read_gpr(0), 0);
    }

    #[test]
    fn signed_multiplication_produces_the_full_product() {
        let cases = [
            (7_i32, 3_i32, 0, 21),
            (-7, 3, u32::MAX, 0xffff_ffeb),
            (7, -3, u32::MAX, 0xffff_ffeb),
            (-7, -3, 0, 21),
            (i32::MIN, i32::MIN, 0x4000_0000, 0),
        ];

        for (lhs, rhs, expected_hi, expected_lo) in cases {
            assert_eq!(
                run_hilo(
                    Instruction::Mult { rs: 1, rt: 2 },
                    &[(1, lhs as u32), (2, rhs as u32)],
                ),
                (expected_hi, expected_lo)
            );
        }
    }

    #[test]
    fn unsigned_multiplication_produces_the_full_product() {
        assert_eq!(
            run_hilo(
                Instruction::Multu { rs: 1, rt: 2 },
                &[(1, 0), (2, u32::MAX)],
            ),
            (0, 0)
        );
        assert_eq!(
            run_hilo(
                Instruction::Multu { rs: 1, rt: 2 },
                &[(1, u32::MAX), (2, u32::MAX)],
            ),
            (0xffff_fffe, 1)
        );
    }

    #[test]
    fn signed_division_truncates_toward_zero() {
        let cases = [
            (7_i32, 3_i32, 1_i32, 2_i32),
            (-7, 3, -1, -2),
            (7, -3, 1, -2),
            (-7, -3, -1, 2),
        ];

        for (dividend, divisor, remainder, quotient) in cases {
            assert_eq!(
                run_hilo(
                    Instruction::Div { rs: 1, rt: 2 },
                    &[(1, dividend as u32), (2, divisor as u32)],
                ),
                (remainder as u32, quotient as u32)
            );
        }
    }

    #[test]
    fn unsigned_division_produces_quotient_and_remainder() {
        assert_eq!(
            run_hilo(Instruction::Divu { rs: 1, rt: 2 }, &[(1, 12), (2, 3)],),
            (0, 4)
        );
        assert_eq!(
            run_hilo(Instruction::Divu { rs: 1, rt: 2 }, &[(1, u32::MAX), (2, 2)],),
            (1, 0x7fff_ffff)
        );
    }

    #[test]
    fn division_special_cases_have_deterministic_results() {
        assert_eq!(
            run_hilo(
                Instruction::Divu { rs: 1, rt: 2 },
                &[(1, 0x1234_5678), (2, 0)],
            ),
            (0x1234_5678, u32::MAX)
        );

        for (dividend, expected_hi, expected_lo) in [
            (7_i32, 7_u32, u32::MAX),
            (0, 0, u32::MAX),
            (-7, (-7_i32) as u32, 1),
        ] {
            assert_eq!(
                run_hilo(
                    Instruction::Div { rs: 1, rt: 2 },
                    &[(1, dividend as u32), (2, 0)],
                ),
                (expected_hi, expected_lo)
            );
        }

        assert_eq!(
            run_hilo(
                Instruction::Div { rs: 1, rt: 2 },
                &[(1, i32::MIN as u32), (2, (-1_i32) as u32)],
            ),
            (0, i32::MIN as u32)
        );
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
    fn signed_arithmetic_produces_expected_results() {
        assert_eq!(
            run(
                Instruction::Add {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &[(1, 5), (2, 7)],
                3,
            ),
            12
        );
        assert_eq!(
            run(
                Instruction::Add {
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
                Instruction::Sub {
                    rd: 3,
                    rs: 1,
                    rt: 2,
                },
                &[(1, 5), (2, 7)],
                3,
            ),
            0xffff_fffe
        );

        for (immediate, expected) in [
            (0x7fff, 0x0000_7fff),
            (0x8000, 0xffff_8000),
            (0xffff, u32::MAX),
        ] {
            assert_eq!(
                run(
                    Instruction::Addi {
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

        assert_eq!(
            run(
                Instruction::Add {
                    rd: 1,
                    rs: 1,
                    rt: 2,
                },
                &[(1, 5), (2, 7)],
                1,
            ),
            12
        );
    }

    #[test]
    fn signed_arithmetic_reports_overflow_without_writing_destination() {
        assert_overflow(
            Instruction::Add {
                rd: 3,
                rs: 1,
                rt: 2,
            },
            &[(1, i32::MAX as u32), (2, 1)],
            3,
        );
        assert_overflow(
            Instruction::Add {
                rd: 3,
                rs: 1,
                rt: 2,
            },
            &[(1, i32::MIN as u32), (2, u32::MAX)],
            3,
        );
        assert_overflow(
            Instruction::Sub {
                rd: 3,
                rs: 1,
                rt: 2,
            },
            &[(1, i32::MAX as u32), (2, u32::MAX)],
            3,
        );
        assert_overflow(
            Instruction::Sub {
                rd: 3,
                rs: 1,
                rt: 2,
            },
            &[(1, i32::MIN as u32), (2, 1)],
            3,
        );
        assert_overflow(
            Instruction::Addi {
                rt: 3,
                rs: 1,
                immediate: 1,
            },
            &[(1, i32::MAX as u32)],
            3,
        );
        assert_overflow(
            Instruction::Addi {
                rt: 3,
                rs: 1,
                immediate: 0xffff,
            },
            &[(1, i32::MIN as u32)],
            3,
        );

        let mut state = State::new();
        state.write_gpr(1, i32::MAX as u32);
        state.write_gpr(2, 1);

        assert_eq!(
            execute(
                &mut state,
                Instruction::Add {
                    rd: 0,
                    rs: 1,
                    rt: 2,
                },
            ),
            Err(Exception::Overflow)
        );
        assert_eq!(state.read_gpr(0), 0);
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
        )
        .expect("ADDU should not trap");
        execute(
            &mut state,
            Instruction::Or {
                rd: 0,
                rs: 1,
                rt: 2,
            },
        )
        .expect("OR should not trap");

        assert_eq!(state.read_gpr(1), 12);
        assert_eq!(state.read_gpr(0), 0);
        assert_eq!(state.pc(), pc);
    }
}
