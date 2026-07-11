use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::exception::Mips4Exception;
use crate::cpu::mips4::instruction::Mips4Instruction;
use crate::cpu::mips4::instruction::decode::{
    Mips4CpuInstruction, Mips4InstructionClass, Mips4InstructionDecode, decode_instruction,
};

use super::{ConformanceMachine, assert_retired, i_type, r_type, regimm};

const ALL_CPU_INSTRUCTIONS: [Mips4CpuInstruction; 112] = [
    Mips4CpuInstruction::Add,
    Mips4CpuInstruction::Addi,
    Mips4CpuInstruction::Addiu,
    Mips4CpuInstruction::Addu,
    Mips4CpuInstruction::And,
    Mips4CpuInstruction::Andi,
    Mips4CpuInstruction::Beq,
    Mips4CpuInstruction::Beql,
    Mips4CpuInstruction::Bgez,
    Mips4CpuInstruction::Bgezal,
    Mips4CpuInstruction::Bgezall,
    Mips4CpuInstruction::Bgezl,
    Mips4CpuInstruction::Bgtz,
    Mips4CpuInstruction::Bgtzl,
    Mips4CpuInstruction::Blez,
    Mips4CpuInstruction::Blezl,
    Mips4CpuInstruction::Bltz,
    Mips4CpuInstruction::Bltzal,
    Mips4CpuInstruction::Bltzall,
    Mips4CpuInstruction::Bltzl,
    Mips4CpuInstruction::Bne,
    Mips4CpuInstruction::Bnel,
    Mips4CpuInstruction::Break,
    Mips4CpuInstruction::Dadd,
    Mips4CpuInstruction::Daddi,
    Mips4CpuInstruction::Daddiu,
    Mips4CpuInstruction::Daddu,
    Mips4CpuInstruction::Ddiv,
    Mips4CpuInstruction::Ddivu,
    Mips4CpuInstruction::Div,
    Mips4CpuInstruction::Divu,
    Mips4CpuInstruction::Dmult,
    Mips4CpuInstruction::Dmultu,
    Mips4CpuInstruction::Dsll,
    Mips4CpuInstruction::Dsll32,
    Mips4CpuInstruction::Dsllv,
    Mips4CpuInstruction::Dsra,
    Mips4CpuInstruction::Dsra32,
    Mips4CpuInstruction::Dsrav,
    Mips4CpuInstruction::Dsrl,
    Mips4CpuInstruction::Dsrl32,
    Mips4CpuInstruction::Dsrlv,
    Mips4CpuInstruction::Dsub,
    Mips4CpuInstruction::Dsubu,
    Mips4CpuInstruction::J,
    Mips4CpuInstruction::Jal,
    Mips4CpuInstruction::Jalr,
    Mips4CpuInstruction::Jr,
    Mips4CpuInstruction::Lb,
    Mips4CpuInstruction::Lbu,
    Mips4CpuInstruction::Ld,
    Mips4CpuInstruction::Ldl,
    Mips4CpuInstruction::Ldr,
    Mips4CpuInstruction::Lh,
    Mips4CpuInstruction::Lhu,
    Mips4CpuInstruction::Ll,
    Mips4CpuInstruction::Lld,
    Mips4CpuInstruction::Lui,
    Mips4CpuInstruction::Lw,
    Mips4CpuInstruction::Lwl,
    Mips4CpuInstruction::Lwr,
    Mips4CpuInstruction::Lwu,
    Mips4CpuInstruction::Mfhi,
    Mips4CpuInstruction::Mflo,
    Mips4CpuInstruction::Movn,
    Mips4CpuInstruction::Movz,
    Mips4CpuInstruction::Mthi,
    Mips4CpuInstruction::Mtlo,
    Mips4CpuInstruction::Mult,
    Mips4CpuInstruction::Multu,
    Mips4CpuInstruction::Nor,
    Mips4CpuInstruction::Or,
    Mips4CpuInstruction::Ori,
    Mips4CpuInstruction::Pref,
    Mips4CpuInstruction::Sb,
    Mips4CpuInstruction::Sc,
    Mips4CpuInstruction::Scd,
    Mips4CpuInstruction::Sd,
    Mips4CpuInstruction::Sdl,
    Mips4CpuInstruction::Sdr,
    Mips4CpuInstruction::Sh,
    Mips4CpuInstruction::Sll,
    Mips4CpuInstruction::Sllv,
    Mips4CpuInstruction::Slt,
    Mips4CpuInstruction::Slti,
    Mips4CpuInstruction::Sltiu,
    Mips4CpuInstruction::Sltu,
    Mips4CpuInstruction::Sra,
    Mips4CpuInstruction::Srav,
    Mips4CpuInstruction::Srl,
    Mips4CpuInstruction::Srlv,
    Mips4CpuInstruction::Sub,
    Mips4CpuInstruction::Subu,
    Mips4CpuInstruction::Sw,
    Mips4CpuInstruction::Swl,
    Mips4CpuInstruction::Swr,
    Mips4CpuInstruction::Sync,
    Mips4CpuInstruction::Syscall,
    Mips4CpuInstruction::Teq,
    Mips4CpuInstruction::Teqi,
    Mips4CpuInstruction::Tge,
    Mips4CpuInstruction::Tgei,
    Mips4CpuInstruction::Tgeiu,
    Mips4CpuInstruction::Tgeu,
    Mips4CpuInstruction::Tlt,
    Mips4CpuInstruction::Tlti,
    Mips4CpuInstruction::Tltiu,
    Mips4CpuInstruction::Tltu,
    Mips4CpuInstruction::Tne,
    Mips4CpuInstruction::Tnei,
    Mips4CpuInstruction::Xor,
    Mips4CpuInstruction::Xori,
];

#[test]
fn every_cpu_instruction_decodes_and_reaches_an_architectural_boundary() {
    let mut seen = [false; 112];
    for instruction in ALL_CPU_INSTRUCTIONS {
        let index = instruction as usize;
        assert!(!seen[index], "duplicate CPU instruction {instruction:?}");
        seen[index] = true;

        let bits = encoding(instruction);
        assert_eq!(
            decode_instruction(Mips4Instruction::from_bits(bits)),
            Mips4InstructionDecode::Instruction(Mips4InstructionClass::Cpu(instruction)),
            "decode for {instruction:?}"
        );
        let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
        machine.write_gpr(1, 0);
        machine.write_gpr(2, 0);
        let boundary = machine.execute_with_zero_bus(bits);
        match instruction {
            Mips4CpuInstruction::Break => assert!(matches!(
                boundary,
                super::super::target::Mips4ExecutionBoundary::Exception {
                    image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                        reason: Mips4Exception::Breakpoint,
                        ..
                    },
                    ..
                }
            )),
            Mips4CpuInstruction::Syscall => assert!(matches!(
                boundary,
                super::super::target::Mips4ExecutionBoundary::Exception {
                    image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                        reason: Mips4Exception::Syscall,
                        ..
                    },
                    ..
                }
            )),
            _ => {}
        }
    }
    assert!(seen.into_iter().all(|covered| covered));
}

#[test]
fn integer_alu_instructions_commit_manual_results_through_raw_encodings() {
    struct Case {
        bits: u32,
        rs: u64,
        rt: u64,
        expected: u64,
    }

    let cases = [
        Case {
            bits: r_type(1, 2, 3, 0, 0x20),
            rs: 7,
            rt: 5,
            expected: 12,
        },
        Case {
            bits: r_type(1, 2, 3, 0, 0x23),
            rs: 3,
            rt: 5,
            expected: 0xffff_ffff_ffff_fffe,
        },
        Case {
            bits: r_type(1, 2, 3, 0, 0x24),
            rs: 0xf0,
            rt: 0x5a,
            expected: 0x50,
        },
        Case {
            bits: r_type(1, 2, 3, 0, 0x25),
            rs: 0xf0,
            rt: 0x5a,
            expected: 0xfa,
        },
        Case {
            bits: r_type(1, 2, 3, 0, 0x26),
            rs: 0xf0,
            rt: 0x5a,
            expected: 0xaa,
        },
        Case {
            bits: r_type(1, 2, 3, 0, 0x27),
            rs: 0xf0,
            rt: 0x5a,
            expected: !0xfa,
        },
        Case {
            bits: r_type(1, 2, 3, 0, 0x2a),
            rs: (-2_i64) as u64,
            rt: 1,
            expected: 1,
        },
        Case {
            bits: r_type(1, 2, 3, 0, 0x2b),
            rs: u64::MAX,
            rt: 1,
            expected: 0,
        },
        Case {
            bits: r_type(0, 2, 3, 4, 0x00),
            rs: 0,
            rt: 3,
            expected: 48,
        },
        Case {
            bits: r_type(0, 2, 3, 4, 0x02),
            rs: 0,
            rt: 0xffff_ffff_8000_0000,
            expected: 0x0800_0000,
        },
        Case {
            bits: r_type(0, 2, 3, 4, 0x03),
            rs: 0,
            rt: 0xffff_ffff_8000_0000,
            expected: 0xffff_ffff_f800_0000,
        },
        Case {
            bits: r_type(0, 2, 3, 4, 0x38),
            rs: 0,
            rt: 3,
            expected: 48,
        },
        Case {
            bits: r_type(0, 2, 3, 1, 0x3c),
            rs: 0,
            rt: 3,
            expected: 0x0000_0006_0000_0000,
        },
    ];

    for case in cases {
        let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
        machine.write_gpr(1, case.rs);
        machine.write_gpr(2, case.rt);
        assert_retired(machine.execute(case.bits), case.bits);
        assert_eq!(
            machine.read_gpr(3),
            case.expected,
            "instruction {:#010x}",
            case.bits
        );
    }
}

#[test]
fn immediate_alu_instructions_apply_signed_or_zero_extended_operands() {
    struct Case {
        bits: u32,
        source: u64,
        expected: u64,
    }
    let cases = [
        Case {
            bits: i_type(0x08, 1, 2, 0xfffe),
            source: 10,
            expected: 8,
        },
        Case {
            bits: i_type(0x09, 1, 2, 1),
            source: u64::MAX,
            expected: 0,
        },
        Case {
            bits: i_type(0x0a, 1, 2, 0xffff),
            source: (-2_i64) as u64,
            expected: 1,
        },
        Case {
            bits: i_type(0x0b, 1, 2, 0xffff),
            source: 0,
            expected: 1,
        },
        Case {
            bits: i_type(0x0c, 1, 2, 0x00ff),
            source: 0x1234,
            expected: 0x34,
        },
        Case {
            bits: i_type(0x0d, 1, 2, 0x00f0),
            source: 0x1204,
            expected: 0x12f4,
        },
        Case {
            bits: i_type(0x0e, 1, 2, 0x00ff),
            source: 0x1234,
            expected: 0x12cb,
        },
        Case {
            bits: i_type(0x0f, 0, 2, 0x8001),
            source: 0,
            expected: 0xffff_ffff_8001_0000,
        },
        Case {
            bits: i_type(0x18, 1, 2, 0xffff),
            source: 0x1_0000_0000,
            expected: 0xffff_ffff,
        },
        Case {
            bits: i_type(0x19, 1, 2, 1),
            source: u64::MAX,
            expected: 0,
        },
    ];
    for case in cases {
        let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
        machine.write_gpr(1, case.source);
        assert_retired(machine.execute(case.bits), case.bits);
        assert_eq!(
            machine.read_gpr(2),
            case.expected,
            "instruction {:#010x}",
            case.bits
        );
    }
}

#[test]
fn multiply_divide_and_hilo_transfers_commit_architectural_pairs() {
    let mut multiply = ConformanceMachine::new(Mips4Endianness::Big);
    multiply.write_gpr(1, (-7_i64) as u64);
    multiply.write_gpr(2, 3);
    let mult = r_type(1, 2, 0, 0, 0x18);
    assert_retired(multiply.execute(mult), mult);
    assert_eq!(multiply.state().hi(), u64::MAX);
    assert_eq!(multiply.state().lo(), 0xffff_ffff_ffff_ffeb);

    let mut divide = ConformanceMachine::new(Mips4Endianness::Big);
    divide.write_gpr(1, (-7_i64) as u64);
    divide.write_gpr(2, 3);
    let div = r_type(1, 2, 0, 0, 0x1a);
    assert_retired(divide.execute(div), div);
    assert_eq!(divide.state().lo(), 0xffff_ffff_ffff_fffe);
    assert_eq!(divide.state().hi(), u64::MAX);

    let mut transfers = ConformanceMachine::new(Mips4Endianness::Big);
    transfers.write_gpr(1, 0x0123_4567_89ab_cdef);
    let mthi = r_type(1, 0, 0, 0, 0x11);
    assert_retired(transfers.execute(mthi), mthi);
    transfers.write_gpr(1, 0xfedc_ba98_7654_3210);
    let mtlo = r_type(1, 0, 0, 0, 0x13);
    assert!(matches!(
        transfers.execute(mtlo),
        super::super::target::Mips4ExecutionBoundary::Retired { .. }
    ));
    let mfhi = r_type(0, 0, 3, 0, 0x10);
    assert!(matches!(
        transfers.execute(mfhi),
        super::super::target::Mips4ExecutionBoundary::Retired { .. }
    ));
    let mflo = r_type(0, 0, 4, 0, 0x12);
    assert!(matches!(
        transfers.execute(mflo),
        super::super::target::Mips4ExecutionBoundary::Retired { .. }
    ));
    assert_eq!(transfers.read_gpr(3), 0x0123_4567_89ab_cdef);
    assert_eq!(transfers.read_gpr(4), 0xfedc_ba98_7654_3210);
}

#[test]
fn every_trap_encoding_enters_the_trap_exception_when_its_condition_is_true() {
    let cases = [
        (r_type(1, 2, 0, 0, 0x30), 2, 1),
        (r_type(1, 2, 0, 0, 0x31), 2, 1),
        (r_type(1, 2, 0, 0, 0x32), 1, 2),
        (r_type(1, 2, 0, 0, 0x33), 1, 2),
        (r_type(1, 2, 0, 0, 0x34), 1, 1),
        (r_type(1, 2, 0, 0, 0x36), 1, 2),
        (regimm(1, 0x08, 1), 2, 0),
        (regimm(1, 0x09, 1), 2, 0),
        (regimm(1, 0x0a, 2), 1, 0),
        (regimm(1, 0x0b, 2), 1, 0),
        (regimm(1, 0x0c, 1), 1, 0),
        (regimm(1, 0x0e, 2), 1, 0),
    ];
    for (bits, rs, rt) in cases {
        let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
        machine.write_gpr(1, rs);
        machine.write_gpr(2, rt);
        assert!(matches!(
            machine.execute(bits),
            super::super::target::Mips4ExecutionBoundary::Exception {
                image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                    reason: Mips4Exception::Trap,
                    ..
                },
                ..
            }
        ));
    }
}

#[test]
fn overflow_traps_without_writing_and_conditional_moves_preserve_false_destinations() {
    let mut overflow = ConformanceMachine::new(Mips4Endianness::Big);
    overflow.write_gpr(1, 0x0000_0000_7fff_ffff);
    overflow.write_gpr(2, 1);
    overflow.write_gpr(3, 0xfeed_face);
    let bits = r_type(1, 2, 3, 0, 0x20);
    assert!(matches!(
        overflow.execute(bits),
        super::super::target::Mips4ExecutionBoundary::Exception {
            image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                reason: Mips4Exception::ArithmeticOverflow,
                ..
            },
            ..
        }
    ));
    assert_eq!(overflow.read_gpr(3), 0xfeed_face);

    for (function, condition) in [(0x0a, 1_u64), (0x0b, 0_u64)] {
        let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
        machine.write_gpr(1, 0x1234);
        machine.write_gpr(2, condition);
        machine.write_gpr(3, 0xfeed_face);
        let bits = r_type(1, 2, 3, 0, function);
        assert_retired(machine.execute(bits), bits);
        assert_eq!(machine.read_gpr(3), 0xfeed_face);
    }
}

#[test]
fn branch_likely_and_link_paths_match_delay_slot_rules() {
    let mut likely = ConformanceMachine::new(Mips4Endianness::Big);
    likely.write_gpr(1, 1);
    likely.write_gpr(2, 2);
    let bits = i_type(0x14, 1, 2, 3);
    assert_retired(likely.execute(bits), bits);
    assert_eq!(likely.state().pc(), super::RESET_PC + 8);
    assert_eq!(likely.state().next_pc(), super::RESET_PC + 12);
    assert_eq!(likely.state().delay_slot_branch_pc(), None);

    let mut linked = ConformanceMachine::new(Mips4Endianness::Big);
    linked.write_gpr(1, 0);
    let bits = regimm(1, 0x11, 2);
    assert_retired(linked.execute(bits), bits);
    assert_eq!(linked.read_gpr(31), super::RESET_PC + 8);
    assert_eq!(linked.state().pc(), super::RESET_PC + 4);
    assert_eq!(linked.state().next_pc(), super::RESET_PC + 12);
    assert_eq!(linked.state().delay_slot_branch_pc(), Some(super::RESET_PC));
}

fn encoding(instruction: Mips4CpuInstruction) -> u32 {
    match instruction {
        Mips4CpuInstruction::J => i_type(0x02, 0, 0, 0),
        Mips4CpuInstruction::Jal => i_type(0x03, 0, 0, 0),
        Mips4CpuInstruction::Beq => i_type(0x04, 1, 2, 1),
        Mips4CpuInstruction::Bne => i_type(0x05, 1, 2, 1),
        Mips4CpuInstruction::Blez => i_type(0x06, 1, 0, 1),
        Mips4CpuInstruction::Bgtz => i_type(0x07, 1, 0, 1),
        Mips4CpuInstruction::Addi => i_type(0x08, 1, 2, 1),
        Mips4CpuInstruction::Addiu => i_type(0x09, 1, 2, 1),
        Mips4CpuInstruction::Slti => i_type(0x0a, 1, 2, 1),
        Mips4CpuInstruction::Sltiu => i_type(0x0b, 1, 2, 1),
        Mips4CpuInstruction::Andi => i_type(0x0c, 1, 2, 1),
        Mips4CpuInstruction::Ori => i_type(0x0d, 1, 2, 1),
        Mips4CpuInstruction::Xori => i_type(0x0e, 1, 2, 1),
        Mips4CpuInstruction::Lui => i_type(0x0f, 0, 2, 1),
        Mips4CpuInstruction::Beql => i_type(0x14, 1, 2, 1),
        Mips4CpuInstruction::Bnel => i_type(0x15, 1, 2, 1),
        Mips4CpuInstruction::Blezl => i_type(0x16, 1, 0, 1),
        Mips4CpuInstruction::Bgtzl => i_type(0x17, 1, 0, 1),
        Mips4CpuInstruction::Daddi => i_type(0x18, 1, 2, 1),
        Mips4CpuInstruction::Daddiu => i_type(0x19, 1, 2, 1),
        Mips4CpuInstruction::Ldl => i_type(0x1a, 1, 2, 0),
        Mips4CpuInstruction::Ldr => i_type(0x1b, 1, 2, 0),
        Mips4CpuInstruction::Lb => i_type(0x20, 1, 2, 0),
        Mips4CpuInstruction::Lh => i_type(0x21, 1, 2, 0),
        Mips4CpuInstruction::Lwl => i_type(0x22, 1, 2, 0),
        Mips4CpuInstruction::Lw => i_type(0x23, 1, 2, 0),
        Mips4CpuInstruction::Lbu => i_type(0x24, 1, 2, 0),
        Mips4CpuInstruction::Lhu => i_type(0x25, 1, 2, 0),
        Mips4CpuInstruction::Lwr => i_type(0x26, 1, 2, 0),
        Mips4CpuInstruction::Lwu => i_type(0x27, 1, 2, 0),
        Mips4CpuInstruction::Sb => i_type(0x28, 1, 2, 0),
        Mips4CpuInstruction::Sh => i_type(0x29, 1, 2, 0),
        Mips4CpuInstruction::Swl => i_type(0x2a, 1, 2, 0),
        Mips4CpuInstruction::Sw => i_type(0x2b, 1, 2, 0),
        Mips4CpuInstruction::Sdl => i_type(0x2c, 1, 2, 0),
        Mips4CpuInstruction::Sdr => i_type(0x2d, 1, 2, 0),
        Mips4CpuInstruction::Swr => i_type(0x2e, 1, 2, 0),
        Mips4CpuInstruction::Ll => i_type(0x30, 1, 2, 0),
        Mips4CpuInstruction::Pref => i_type(0x33, 1, 0, 0),
        Mips4CpuInstruction::Lld => i_type(0x34, 1, 2, 0),
        Mips4CpuInstruction::Ld => i_type(0x37, 1, 2, 0),
        Mips4CpuInstruction::Sc => i_type(0x38, 1, 2, 0),
        Mips4CpuInstruction::Scd => i_type(0x3c, 1, 2, 0),
        Mips4CpuInstruction::Sd => i_type(0x3f, 1, 2, 0),
        Mips4CpuInstruction::Bltz => regimm(1, 0x00, 1),
        Mips4CpuInstruction::Bgez => regimm(1, 0x01, 1),
        Mips4CpuInstruction::Bltzl => regimm(1, 0x02, 1),
        Mips4CpuInstruction::Bgezl => regimm(1, 0x03, 1),
        Mips4CpuInstruction::Tgei => regimm(1, 0x08, 1),
        Mips4CpuInstruction::Tgeiu => regimm(1, 0x09, 1),
        Mips4CpuInstruction::Tlti => regimm(1, 0x0a, 1),
        Mips4CpuInstruction::Tltiu => regimm(1, 0x0b, 1),
        Mips4CpuInstruction::Teqi => regimm(1, 0x0c, 1),
        Mips4CpuInstruction::Tnei => regimm(1, 0x0e, 1),
        Mips4CpuInstruction::Bltzal => regimm(1, 0x10, 1),
        Mips4CpuInstruction::Bgezal => regimm(1, 0x11, 1),
        Mips4CpuInstruction::Bltzall => regimm(1, 0x12, 1),
        Mips4CpuInstruction::Bgezall => regimm(1, 0x13, 1),
        Mips4CpuInstruction::Sll => r_type(0, 2, 3, 1, 0x00),
        Mips4CpuInstruction::Srl => r_type(0, 2, 3, 1, 0x02),
        Mips4CpuInstruction::Sra => r_type(0, 2, 3, 1, 0x03),
        Mips4CpuInstruction::Sllv => r_type(1, 2, 3, 0, 0x04),
        Mips4CpuInstruction::Srlv => r_type(1, 2, 3, 0, 0x06),
        Mips4CpuInstruction::Srav => r_type(1, 2, 3, 0, 0x07),
        Mips4CpuInstruction::Jr => r_type(1, 0, 0, 0, 0x08),
        Mips4CpuInstruction::Jalr => r_type(1, 0, 3, 0, 0x09),
        Mips4CpuInstruction::Movz => r_type(1, 2, 3, 0, 0x0a),
        Mips4CpuInstruction::Movn => r_type(1, 2, 3, 0, 0x0b),
        Mips4CpuInstruction::Syscall => 0x0c,
        Mips4CpuInstruction::Break => 0x0d,
        Mips4CpuInstruction::Sync => 0x0f,
        Mips4CpuInstruction::Mfhi => r_type(0, 0, 3, 0, 0x10),
        Mips4CpuInstruction::Mthi => r_type(1, 0, 0, 0, 0x11),
        Mips4CpuInstruction::Mflo => r_type(0, 0, 3, 0, 0x12),
        Mips4CpuInstruction::Mtlo => r_type(1, 0, 0, 0, 0x13),
        Mips4CpuInstruction::Dsllv => r_type(1, 2, 3, 0, 0x14),
        Mips4CpuInstruction::Dsrlv => r_type(1, 2, 3, 0, 0x16),
        Mips4CpuInstruction::Dsrav => r_type(1, 2, 3, 0, 0x17),
        Mips4CpuInstruction::Mult => r_type(1, 2, 0, 0, 0x18),
        Mips4CpuInstruction::Multu => r_type(1, 2, 0, 0, 0x19),
        Mips4CpuInstruction::Div => r_type(1, 2, 0, 0, 0x1a),
        Mips4CpuInstruction::Divu => r_type(1, 2, 0, 0, 0x1b),
        Mips4CpuInstruction::Dmult => r_type(1, 2, 0, 0, 0x1c),
        Mips4CpuInstruction::Dmultu => r_type(1, 2, 0, 0, 0x1d),
        Mips4CpuInstruction::Ddiv => r_type(1, 2, 0, 0, 0x1e),
        Mips4CpuInstruction::Ddivu => r_type(1, 2, 0, 0, 0x1f),
        Mips4CpuInstruction::Add => r_type(1, 2, 3, 0, 0x20),
        Mips4CpuInstruction::Addu => r_type(1, 2, 3, 0, 0x21),
        Mips4CpuInstruction::Sub => r_type(1, 2, 3, 0, 0x22),
        Mips4CpuInstruction::Subu => r_type(1, 2, 3, 0, 0x23),
        Mips4CpuInstruction::And => r_type(1, 2, 3, 0, 0x24),
        Mips4CpuInstruction::Or => r_type(1, 2, 3, 0, 0x25),
        Mips4CpuInstruction::Xor => r_type(1, 2, 3, 0, 0x26),
        Mips4CpuInstruction::Nor => r_type(1, 2, 3, 0, 0x27),
        Mips4CpuInstruction::Slt => r_type(1, 2, 3, 0, 0x2a),
        Mips4CpuInstruction::Sltu => r_type(1, 2, 3, 0, 0x2b),
        Mips4CpuInstruction::Dadd => r_type(1, 2, 3, 0, 0x2c),
        Mips4CpuInstruction::Daddu => r_type(1, 2, 3, 0, 0x2d),
        Mips4CpuInstruction::Dsub => r_type(1, 2, 3, 0, 0x2e),
        Mips4CpuInstruction::Dsubu => r_type(1, 2, 3, 0, 0x2f),
        Mips4CpuInstruction::Tge => r_type(1, 2, 0, 0, 0x30),
        Mips4CpuInstruction::Tgeu => r_type(1, 2, 0, 0, 0x31),
        Mips4CpuInstruction::Tlt => r_type(1, 2, 0, 0, 0x32),
        Mips4CpuInstruction::Tltu => r_type(1, 2, 0, 0, 0x33),
        Mips4CpuInstruction::Teq => r_type(1, 2, 0, 0, 0x34),
        Mips4CpuInstruction::Tne => r_type(1, 2, 0, 0, 0x36),
        Mips4CpuInstruction::Dsll => r_type(0, 2, 3, 1, 0x38),
        Mips4CpuInstruction::Dsrl => r_type(0, 2, 3, 1, 0x3a),
        Mips4CpuInstruction::Dsra => r_type(0, 2, 3, 1, 0x3b),
        Mips4CpuInstruction::Dsll32 => r_type(0, 2, 3, 1, 0x3c),
        Mips4CpuInstruction::Dsrl32 => r_type(0, 2, 3, 1, 0x3e),
        Mips4CpuInstruction::Dsra32 => r_type(0, 2, 3, 1, 0x3f),
    }
}
