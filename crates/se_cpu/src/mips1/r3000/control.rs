use super::{decode::ControlInstruction, state::State};

pub(super) fn execute(state: &mut State, instruction: ControlInstruction) -> u32 {
    let pc = state.pc();

    match instruction {
        ControlInstruction::J { target } => jump_target(pc, target),
        ControlInstruction::Jal { target } => {
            state.write_gpr(31, pc.wrapping_add(8));
            jump_target(pc, target)
        }
        ControlInstruction::Beq { rs, rt, offset } => {
            branch_resume_pc(pc, offset, state.read_gpr(rs) == state.read_gpr(rt))
        }
        ControlInstruction::Bne { rs, rt, offset } => {
            branch_resume_pc(pc, offset, state.read_gpr(rs) != state.read_gpr(rt))
        }
        ControlInstruction::Blez { rs, offset } => {
            branch_resume_pc(pc, offset, (state.read_gpr(rs) as i32) <= 0)
        }
        ControlInstruction::Bgtz { rs, offset } => {
            branch_resume_pc(pc, offset, (state.read_gpr(rs) as i32) > 0)
        }
        ControlInstruction::Bltz { rs, offset } => {
            branch_resume_pc(pc, offset, (state.read_gpr(rs) as i32) < 0)
        }
        ControlInstruction::Bgez { rs, offset } => {
            branch_resume_pc(pc, offset, (state.read_gpr(rs) as i32) >= 0)
        }
        ControlInstruction::Bltzal { rs, offset } => {
            let condition = (state.read_gpr(rs) as i32) < 0;
            state.write_gpr(31, pc.wrapping_add(8));
            branch_resume_pc(pc, offset, condition)
        }
        ControlInstruction::Bgezal { rs, offset } => {
            let condition = (state.read_gpr(rs) as i32) >= 0;
            state.write_gpr(31, pc.wrapping_add(8));
            branch_resume_pc(pc, offset, condition)
        }
    }
}

fn branch_resume_pc(pc: u32, offset: u16, condition: bool) -> u32 {
    if condition {
        let displacement = (i32::from(offset as i16) as u32).wrapping_shl(2);
        pc.wrapping_add(4).wrapping_add(displacement)
    } else {
        pc.wrapping_add(8)
    }
}

fn jump_target(pc: u32, target: u32) -> u32 {
    (pc.wrapping_add(4) & 0xf000_0000) | (target << 2)
}

#[cfg(test)]
mod tests {
    use super::{ControlInstruction, State, branch_resume_pc, execute, jump_target};

    fn run(instruction: ControlInstruction, registers: &[(usize, u32)]) -> (State, u32) {
        let mut state = State::new();
        for &(index, value) in registers {
            state.write_gpr(index, value);
        }

        let resume_pc = execute(&mut state, instruction);

        (state, resume_pc)
    }

    #[test]
    fn jump_targets_use_delay_slot_address_high_bits() {
        assert_eq!(jump_target(0xbfc0_0000, 0), 0xb000_0000);
        assert_eq!(jump_target(0xbfc0_0000, 0x03ff_ffff), 0xbfff_fffc);
        assert_eq!(jump_target(0x0fff_fffc, 0), 0x1000_0000);
    }

    #[test]
    fn branch_offsets_sign_extend_and_wrap() {
        let pc = 0x1000_0000;
        let cases = [
            (0x0000, 0x1000_0004),
            (0x0001, 0x1000_0008),
            (0x7fff, 0x1002_0000),
            (0x8000, 0x0ffe_0004),
            (0xffff, 0x1000_0000),
        ];

        for (offset, expected) in cases {
            assert_eq!(branch_resume_pc(pc, offset, true), expected);
        }

        assert_eq!(branch_resume_pc(pc, 0x8000, false), 0x1000_0008);
        assert_eq!(branch_resume_pc(0xffff_fffc, 0, true), 0);
        assert_eq!(branch_resume_pc(0xffff_fffc, 0, false), 4);
    }

    #[test]
    fn jump_and_jump_and_link_produce_expected_results() {
        let instruction_pc = State::new().pc();
        let (state, resume_pc) = run(ControlInstruction::J { target: 3 }, &[(31, 0x1234_5678)]);

        assert_eq!(resume_pc, 0xb000_000c);
        assert_eq!(state.read_gpr(31), 0x1234_5678);
        assert_eq!(state.pc(), instruction_pc);

        let (state, resume_pc) = run(ControlInstruction::Jal { target: 3 }, &[(31, 0x1234_5678)]);

        assert_eq!(resume_pc, 0xb000_000c);
        assert_eq!(state.read_gpr(31), instruction_pc.wrapping_add(8));
        assert_eq!(state.pc(), instruction_pc);
    }

    #[test]
    fn equality_branches_cover_taken_and_not_taken() {
        let instruction_pc = State::new().pc();
        let taken = branch_resume_pc(instruction_pc, 2, true);
        let not_taken = branch_resume_pc(instruction_pc, 2, false);

        assert_eq!(
            run(
                ControlInstruction::Beq {
                    rs: 1,
                    rt: 2,
                    offset: 2,
                },
                &[(1, 7), (2, 7)],
            )
            .1,
            taken
        );
        assert_eq!(
            run(
                ControlInstruction::Beq {
                    rs: 1,
                    rt: 2,
                    offset: 2,
                },
                &[(1, 7), (2, 8)],
            )
            .1,
            not_taken
        );
        assert_eq!(
            run(
                ControlInstruction::Bne {
                    rs: 1,
                    rt: 2,
                    offset: 2,
                },
                &[(1, 7), (2, 8)],
            )
            .1,
            taken
        );
        assert_eq!(
            run(
                ControlInstruction::Bne {
                    rs: 1,
                    rt: 2,
                    offset: 2,
                },
                &[(1, 7), (2, 7)],
            )
            .1,
            not_taken
        );
    }

    #[test]
    fn signed_zero_branches_cover_negative_zero_and_positive() {
        let instruction_pc = State::new().pc();

        for (value, blez_taken, bgtz_taken) in
            [(u32::MAX, true, false), (0, true, false), (1, false, true)]
        {
            assert_eq!(
                run(ControlInstruction::Blez { rs: 1, offset: 2 }, &[(1, value)]).1,
                branch_resume_pc(instruction_pc, 2, blez_taken)
            );
            assert_eq!(
                run(ControlInstruction::Bgtz { rs: 1, offset: 2 }, &[(1, value)]).1,
                branch_resume_pc(instruction_pc, 2, bgtz_taken)
            );
        }
    }

    #[test]
    fn sign_branches_cover_negative_and_nonnegative_values() {
        let instruction_pc = State::new().pc();

        for (value, bltz_taken, bgez_taken) in
            [(u32::MAX, true, false), (0, false, true), (1, false, true)]
        {
            assert_eq!(
                run(ControlInstruction::Bltz { rs: 1, offset: 2 }, &[(1, value)]).1,
                branch_resume_pc(instruction_pc, 2, bltz_taken)
            );
            assert_eq!(
                run(ControlInstruction::Bgez { rs: 1, offset: 2 }, &[(1, value)]).1,
                branch_resume_pc(instruction_pc, 2, bgez_taken)
            );
        }
    }

    #[test]
    fn link_branches_write_link_for_both_outcomes() {
        let instruction_pc = State::new().pc();
        let link = instruction_pc.wrapping_add(8);
        let cases = [
            (
                ControlInstruction::Bltzal { rs: 1, offset: 2 },
                u32::MAX,
                true,
            ),
            (ControlInstruction::Bltzal { rs: 1, offset: 2 }, 0, false),
            (ControlInstruction::Bgezal { rs: 1, offset: 2 }, 0, true),
            (
                ControlInstruction::Bgezal { rs: 1, offset: 2 },
                u32::MAX,
                false,
            ),
        ];

        for (instruction, value, condition) in cases {
            let (state, resume_pc) = run(instruction, &[(1, value), (31, 0x1234_5678)]);

            assert_eq!(resume_pc, branch_resume_pc(instruction_pc, 2, condition));
            assert_eq!(state.read_gpr(31), link);
            assert_eq!(state.pc(), instruction_pc);
        }
    }

    #[test]
    fn link_branches_read_register_thirty_one_before_writing_it() {
        let instruction_pc = State::new().pc();
        let link = instruction_pc.wrapping_add(8);

        let (state, resume_pc) = run(
            ControlInstruction::Bltzal { rs: 31, offset: 2 },
            &[(31, u32::MAX)],
        );
        assert_eq!(resume_pc, branch_resume_pc(instruction_pc, 2, true));
        assert_eq!(state.read_gpr(31), link);

        let (state, resume_pc) = run(
            ControlInstruction::Bgezal { rs: 31, offset: 2 },
            &[(31, u32::MAX)],
        );
        assert_eq!(resume_pc, branch_resume_pc(instruction_pc, 2, false));
        assert_eq!(state.read_gpr(31), link);
    }
}
