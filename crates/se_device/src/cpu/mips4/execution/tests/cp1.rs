use se_float::control::FloatExceptionFlags;

use crate::cpu::execution::protocol::ExecutionAction;
use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::cp0::Mips4Cp0Register;
use crate::cpu::mips4::cp1::Mips4Cp1ConditionCode;
use crate::cpu::mips4::cp1::decode::{
    Mips4Cp1CompareCondition, Mips4Cp1Decode, Mips4Cp1InstructionClass, Mips4Cp1Operation,
    decode_instruction,
};
use crate::cpu::mips4::cp1::{Mips4Cp1FgrIndex, Mips4Cp1RegisterMode, Mips4Cp1RoundingMode};
use crate::cpu::mips4::exception::Mips4Exception;
use crate::cpu::mips4::instruction::Mips4Instruction;

use super::super::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};
use super::super::target::Mips4ExecutionBoundary;
use super::{ConformanceMachine, assert_retired, i_type, r_type};

const STATUS_CP1_FR_ERL_BEV: u64 = (1 << 29) | (1 << 26) | (1 << 22) | (1 << 2);
const MIPS4_DEFAULT_QNAN_F32: u32 = 0x7fbf_ffff;
const MIPS4_DEFAULT_QNAN_F64: u64 = 0x7ff7_ffff_ffff_ffff;
const COP1_STANDARD_FUNCTIONS: [u8; 24] = [
    0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
    0x11, 0x12, 0x13, 0x15, 0x16, 0x24, 0x25, 0x30,
];
const COP1X_FUSED_FUNCTIONS: [u8; 8] = [0x20, 0x21, 0x28, 0x29, 0x30, 0x31, 0x38, 0x39];

#[test]
fn user_mips4_cp1_denials_are_precise_and_side_effect_free() {
    const STATUS_CU1: u32 = 1 << 29;
    let recip_s = cop1_formatted(0x10, 0, 2, 6, 0x15);
    let marker = 7.0_f32.to_bits();
    let mut unimplemented = ConformanceMachine::new(Mips4Endianness::Big);
    write_f32(&mut unimplemented, 2, 2.0);
    unimplemented
        .state_mut()
        .cp1
        .fgr_mut()
        .write_word(fgr(6), marker);
    unimplemented.enter_user_mode(STATUS_CU1);
    assert!(matches!(
        unimplemented.execute_user(recip_s),
        Mips4ExecutionBoundary::Exception {
            image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                reason: Mips4Exception::FloatingPoint,
                ..
            },
            ..
        }
    ));
    assert!(
        unimplemented
            .state()
            .cp1()
            .fcsr()
            .unimplemented_operation_cause()
    );
    assert_eq!(unimplemented.state().cp1().fgr().read_word(fgr(6)), marker);

    let movf = r_type(1, 0, 3, 0, 0x01);
    let mut reserved = ConformanceMachine::new(Mips4Endianness::Big);
    reserved.write_gpr(1, 0x1234);
    reserved.write_gpr(3, 0xfeed);
    reserved.enter_user_mode(STATUS_CU1);
    assert!(matches!(
        reserved.execute_user(movf),
        Mips4ExecutionBoundary::Exception {
            image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                reason: Mips4Exception::ReservedInstruction,
                ..
            },
            ..
        }
    ));
    assert_eq!(reserved.read_gpr(3), 0xfeed);
    assert!(
        !reserved
            .state()
            .cp1()
            .fcsr()
            .unimplemented_operation_cause()
    );
}

#[test]
fn every_cp1_instruction_class_reaches_an_architectural_boundary() {
    let transfer_formats = [0x00, 0x01, 0x02, 0x04, 0x05, 0x06];
    for format in transfer_formats {
        let mut machine = cp1_machine();
        let register = if matches!(format, 0x02 | 0x06) { 31 } else { 2 };
        let bits = cop1_transfer(format, 1, register);
        assert!(matches!(
            machine.execute(bits),
            Mips4ExecutionBoundary::Retired { .. }
        ));
    }

    for branch_bits in 0_u8..4 {
        let mut machine = cp1_machine();
        let bits = ((0x11_u32) << 26) | (0x08 << 21) | ((branch_bits as u32) << 16) | 1;
        assert!(matches!(
            machine.execute(bits),
            Mips4ExecutionBoundary::Retired { .. }
        ));
    }

    for opcode in [0x31, 0x35, 0x39, 0x3d] {
        let mut machine = cp1_machine();
        machine.write_gpr(1, 0xffff_ffff_a000_1000);
        let boundary = machine.execute_with_zero_bus(i_type(opcode, 1, 2, 0));
        assert!(matches!(boundary, Mips4ExecutionBoundary::Retired { .. }));
    }

    for function in [0x00, 0x01, 0x08, 0x09] {
        let mut machine = cp1_machine();
        machine.write_gpr(1, 0xffff_ffff_a000_1000);
        let bits = cop1x(1, 0, 0, 2, function);
        assert!(matches!(
            machine.execute_with_zero_bus(bits),
            Mips4ExecutionBoundary::Retired { .. }
        ));
    }

    for move_true in [false, true] {
        let mut machine = cp1_machine();
        let bits = r_type(1, u8::from(move_true), 3, 0, 0x01);
        assert_retired(machine.execute(bits), bits);
    }

    let mut prefetch = cp1_machine();
    let bits = cop1x(1, 2, 0, 0, 0x0f);
    assert_retired(prefetch.execute(bits), bits);
}

#[test]
fn cp1_instruction_class_registry_is_exhaustive() {
    let representatives = [
        i_type(0x31, 1, 2, 0),
        cop1_transfer(0x00, 1, 2),
        ((0x11_u32) << 26) | (0x08 << 21),
        cop1_formatted(0x10, 4, 2, 6, 0x00),
        cop1x(1, 2, 0, 4, 0x00),
        cop1x(1, 2, 0, 0, 0x0f),
        r_type(1, 0, 3, 0, 0x01),
    ];
    let mut covered = [false; 7];
    for bits in representatives {
        let Some(Mips4Cp1Decode::Instruction(class)) =
            decode_instruction(Mips4Instruction::from_bits(bits))
        else {
            panic!("expected CP1 instruction class for {bits:#010x}");
        };
        covered[class_key(class)] = true;
    }
    assert!(covered.into_iter().all(|present| present));
}

#[test]
fn every_valid_formatted_operation_decodes_and_executes() {
    let mut covered = [false; 46];
    for format in [0x10_u8, 0x11] {
        for function in COP1_STANDARD_FUNCTIONS {
            let functions: &[u8] = if function == 0x30 {
                &[
                    0x30, 0x31, 0x32, 0x33, 0x34, 0x35, 0x36, 0x37, 0x38, 0x39, 0x3a, 0x3b, 0x3c,
                    0x3d, 0x3e, 0x3f,
                ]
            } else {
                &[function]
            };
            for actual_function in functions {
                let bits = cop1_formatted(format, 2, 4, 6, *actual_function);
                assert!(matches!(
                    decode_instruction(Mips4Instruction::from_bits(bits)),
                    Some(Mips4Cp1Decode::Instruction(_))
                ));
                record_formatted_coverage(&mut covered, bits);
                let mut machine = cp1_machine();
                assert!(matches!(
                    machine.execute(bits),
                    Mips4ExecutionBoundary::Retired { .. }
                        | Mips4ExecutionBoundary::Exception { .. }
                ));
            }
        }
    }

    for format in [0x14_u8, 0x15] {
        for function in [0x20_u8, 0x21] {
            let bits = cop1_formatted(format, 0, 2, 4, function);
            record_formatted_coverage(&mut covered, bits);
            let mut machine = cp1_machine();
            assert_retired(machine.execute(bits), bits);
        }
    }

    for (format, function) in [(0x10_u8, 0x21_u8), (0x11, 0x20)] {
        let bits = cop1_formatted(format, 0, 2, 4, function);
        record_formatted_coverage(&mut covered, bits);
        let mut machine = cp1_machine();
        assert_retired(machine.execute(bits), bits);
    }

    for function in COP1X_FUSED_FUNCTIONS {
        let mut machine = cp1_machine();
        let bits = cop1x(2, 4, 6, 8, function);
        record_formatted_coverage(&mut covered, bits);
        assert!(matches!(
            machine.execute(bits),
            Mips4ExecutionBoundary::Retired { .. }
        ));
    }
    let movt_s = cop1_formatted(0x10, 1, 2, 4, 0x11);
    record_formatted_coverage(&mut covered, movt_s);
    let mut machine = cp1_machine();
    assert_retired(machine.execute(movt_s), movt_s);
    assert!(covered.into_iter().all(|present| present));
}

#[test]
fn arithmetic_and_fused_operations_produce_exact_manual_values() {
    let mut single = cp1_machine();
    write_f32(&mut single, 2, 1.5);
    write_f32(&mut single, 4, 2.25);
    let add_s = cop1_formatted(0x10, 4, 2, 6, 0x00);
    assert_retired(single.execute(add_s), add_s);
    assert_eq!(
        single.state().cp1().fgr().read_word(fgr(6)),
        3.75f32.to_bits()
    );

    let mut double = cp1_machine();
    write_f64(&mut double, 2, 1.5);
    write_f64(&mut double, 4, 2.25);
    let add_d = cop1_formatted(0x11, 4, 2, 6, 0x00);
    assert_retired(double.execute(add_d), add_d);
    assert_eq!(
        double
            .state()
            .cp1()
            .fgr()
            .read_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, fgr(6))
            .unwrap(),
        3.75f64.to_bits()
    );

    let mut fused = cp1_machine();
    write_f32(&mut fused, 2, 2.0);
    write_f32(&mut fused, 4, 3.0);
    write_f32(&mut fused, 6, 4.0);
    let madd_s = cop1x(6, 4, 2, 8, 0x20);
    assert_retired(fused.execute(madd_s), madd_s);
    assert_eq!(
        fused.state().cp1().fgr().read_word(fgr(8)),
        10.0f32.to_bits()
    );
}

#[test]
fn cp1_compare_branch_and_conditional_moves_use_selected_condition_codes() {
    let mut compare = cp1_machine();
    write_f32(&mut compare, 2, 1.0);
    write_f32(&mut compare, 4, 2.0);
    let compare_olt_cc3 = cop1_formatted(0x10, 4, 2, 12, 0x34);
    assert_retired(compare.execute(compare_olt_cc3), compare_olt_cc3);
    assert!(
        compare
            .state()
            .cp1()
            .fcsr()
            .condition_code(Mips4Cp1ConditionCode::from_u8(3).unwrap())
    );

    let branch_true_cc3 = ((0x11_u32) << 26) | (0x08 << 21) | (13 << 16) | 2;
    assert!(matches!(
        compare.execute(branch_true_cc3),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert_eq!(
        compare.state().delay_slot_branch_pc(),
        Some(super::RESET_PC + 4)
    );

    let mut integer_move = cp1_machine();
    integer_move
        .state_mut()
        .cp1
        .fcsr_mut()
        .set_condition_code(Mips4Cp1ConditionCode::from_u8(0).unwrap(), true);
    integer_move.write_gpr(1, 0x1234_5678);
    let movt = r_type(1, 1, 3, 0, 0x01);
    assert_retired(integer_move.execute(movt), movt);
    assert_eq!(integer_move.read_gpr(3), 0x1234_5678);

    let mut floating_move = cp1_machine();
    floating_move
        .state_mut()
        .cp1
        .fcsr_mut()
        .set_condition_code(Mips4Cp1ConditionCode::from_u8(0).unwrap(), true);
    write_f32(&mut floating_move, 2, 7.5);
    let movt_s = cop1_formatted(0x10, 1, 2, 4, 0x11);
    assert_retired(floating_move.execute(movt_s), movt_s);
    assert_eq!(
        floating_move.state().cp1().fgr().read_word(fgr(4)),
        7.5f32.to_bits()
    );
}

#[test]
fn cp1_transfers_and_ctc1_write_before_trapping() {
    let mut transfer = cp1_machine();
    transfer.write_gpr(1, 0xffff_ffff_89ab_cdef);
    let mtc1 = cop1_transfer(0x04, 1, 2);
    assert_retired(transfer.execute(mtc1), mtc1);
    let mfc1 = cop1_transfer(0x00, 3, 2);
    assert!(matches!(
        transfer.execute(mfc1),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    assert_eq!(transfer.read_gpr(3), 0xffff_ffff_89ab_cdef);

    let mut ctc1 = cp1_machine();
    let written_fcsr = (1_u32 << 16) | (1_u32 << 11);
    ctc1.write_gpr(1, u64::from(written_fcsr));
    let bits = cop1_transfer(0x06, 1, 31);
    assert!(matches!(
        ctc1.execute(bits),
        Mips4ExecutionBoundary::Exception {
            image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                reason: Mips4Exception::FloatingPoint,
                ..
            },
            ..
        }
    ));
    assert_eq!(ctc1.state().cp1().fcsr().bits(), written_fcsr);

    let mut unimplemented = cp1_machine();
    unimplemented.write_gpr(1, 1 << 17);
    assert!(matches!(
        unimplemented.execute(bits),
        Mips4ExecutionBoundary::Exception { .. }
    ));
    assert!(
        unimplemented
            .state()
            .cp1()
            .fcsr()
            .unimplemented_operation_cause()
    );
}

#[test]
fn r5000_reserved_cp1_control_transfers_use_deterministic_undefined_values() {
    let mut read = cp1_machine();
    read.write_gpr(2, u64::MAX);

    let cfc1_fcr30 = cop1_transfer(0x02, 2, 30);
    assert_retired(read.execute(cfc1_fcr30), cfc1_fcr30);
    assert_eq!(read.read_gpr(2), 0);
    assert_eq!(read.state().cp1().fcsr().bits(), 0);

    let mut write = cp1_machine();
    write.write_gpr(2, 0xfeed_face);
    let ctc1_fcr30 = cop1_transfer(0x06, 2, 30);
    assert_retired(write.execute(ctc1_fcr30), ctc1_fcr30);
    assert_eq!(write.state().cp1().fcsr().bits(), 0);
}

// Oracle: VR5000 User's Manual, Table 9-2 and section 9.4.6.
#[test]
fn r5000_quiet_nan_arithmetic_is_unimplemented_and_preserves_state() {
    let add_s = cop1_formatted(0x10, 4, 2, 6, 0x00);
    for (lhs, rhs) in [
        (0x7f80_0001, 1.0_f32.to_bits()),
        (1.0_f32.to_bits(), 0xffbf_fffe),
        (0x7f81_2345, 0xffa5_4321),
    ] {
        let mut machine = cp1_machine();
        machine.state_mut().cp1.fgr_mut().write_word(fgr(2), lhs);
        machine.state_mut().cp1.fgr_mut().write_word(fgr(4), rhs);
        machine
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(6), 0xfeed_face);
        machine
            .state_mut()
            .cp1
            .fcsr_mut()
            .set_flag_flags(FloatExceptionFlags::DIVIDE_BY_ZERO);

        assert!(matches!(
            machine.execute(add_s),
            Mips4ExecutionBoundary::Exception { .. }
        ));
        let fcsr = machine.state().cp1().fcsr();
        assert!(fcsr.unimplemented_operation_cause());
        assert!(fcsr.cause_flags().is_empty());
        assert_eq!(fcsr.flag_flags(), FloatExceptionFlags::DIVIDE_BY_ZERO);
        assert_eq!(machine.state().cp1().fgr().read_word(fgr(6)), 0xfeed_face);
    }

    let add_d = cop1_formatted(0x11, 4, 2, 6, 0x00);
    for (lhs, rhs) in [
        (0x7ff0_0000_0000_0001, 1.0_f64.to_bits()),
        (1.0_f64.to_bits(), 0xfff7_ffff_ffff_fffe),
        (0x7ff0_0123_4567_89ab, 0xfff4_3210_9876_5432),
    ] {
        let mut machine = cp1_machine();
        write_f64_bits(&mut machine, 2, lhs);
        write_f64_bits(&mut machine, 4, rhs);
        write_f64_bits(&mut machine, 6, 0xfeed_face_dead_beef);

        assert!(matches!(
            machine.execute(add_d),
            Mips4ExecutionBoundary::Exception { .. }
        ));
        let fcsr = machine.state().cp1().fcsr();
        assert!(fcsr.unimplemented_operation_cause());
        assert!(fcsr.cause_flags().is_empty());
        assert_eq!(read_f64_bits(&machine, 6), 0xfeed_face_dead_beef);
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Table B-3; VR5000 User's Manual,
// sections 9.4.2 and 9.4.6.
#[test]
fn r5000_invalid_arithmetic_uses_exact_default_quiet_nan_bits() {
    let f32_cases = [
        (0_u32, 0_u32, 0x03_u8),
        (f32::INFINITY.to_bits(), f32::INFINITY.to_bits(), 0x01),
        ((-1.0_f32).to_bits(), 0, 0x04),
        (0xffc0_0123, 1.0_f32.to_bits(), 0x00),
    ];
    for (fs, ft, function) in f32_cases {
        let mut machine = cp1_machine();
        machine.state_mut().cp1.fgr_mut().write_word(fgr(2), fs);
        machine.state_mut().cp1.fgr_mut().write_word(fgr(4), ft);
        let bits = cop1_formatted(0x10, 4, 2, 6, function);

        assert_retired(machine.execute(bits), bits);
        assert_eq!(
            machine.state().cp1().fgr().read_word(fgr(6)),
            MIPS4_DEFAULT_QNAN_F32
        );
        assert_eq!(
            machine.state().cp1().fcsr().cause_flags(),
            FloatExceptionFlags::INVALID
        );
        assert_eq!(
            machine.state().cp1().fcsr().flag_flags(),
            FloatExceptionFlags::INVALID
        );
    }

    let f64_cases = [
        (0_u64, 0_u64, 0x03_u8),
        (f64::INFINITY.to_bits(), f64::INFINITY.to_bits(), 0x01),
        ((-1.0_f64).to_bits(), 0, 0x04),
        (0xffff_0000_0000_0123, 1.0_f64.to_bits(), 0x00),
    ];
    for (fs, ft, function) in f64_cases {
        let mut machine = cp1_machine();
        write_f64_bits(&mut machine, 2, fs);
        write_f64_bits(&mut machine, 4, ft);
        let bits = cop1_formatted(0x11, 4, 2, 6, function);

        assert_retired(machine.execute(bits), bits);
        assert_eq!(read_f64_bits(&machine, 6), MIPS4_DEFAULT_QNAN_F64);
        assert_eq!(
            machine.state().cp1().fcsr().cause_flags(),
            FloatExceptionFlags::INVALID
        );
        assert_eq!(
            machine.state().cp1().fcsr().flag_flags(),
            FloatExceptionFlags::INVALID
        );
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, ABS.fmt, NEG.fmt, MOV.fmt,
// MOVF.fmt, MOVT.fmt, MOVN.fmt, and MOVZ.fmt descriptions.
#[test]
fn r5000_unary_and_formatted_moves_handle_nan_bits_exactly() {
    for function in [0x05_u8, 0x07] {
        let mut signaling = cp1_machine();
        signaling
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(2), 0xffc0_0123);
        let bits = cop1_formatted(0x10, 0, 2, 6, function);
        assert_retired(signaling.execute(bits), bits);
        assert_eq!(
            signaling.state().cp1().fgr().read_word(fgr(6)),
            MIPS4_DEFAULT_QNAN_F32
        );
        assert_eq!(
            signaling.state().cp1().fcsr().cause_flags(),
            FloatExceptionFlags::INVALID
        );

        let mut quiet = cp1_machine();
        quiet
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(2), 0xff81_2345);
        quiet
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(6), 0xfeed_face);
        assert!(matches!(
            quiet.execute(bits),
            Mips4ExecutionBoundary::Exception { .. }
        ));
        assert!(quiet.state().cp1().fcsr().unimplemented_operation_cause());
        assert_eq!(quiet.state().cp1().fgr().read_word(fgr(6)), 0xfeed_face);
    }

    for (function, ft, condition, source) in [
        (0x06_u8, 0_u8, false, 0x7f80_1234_u32),
        (0x11, 0, false, 0xffc0_5678),
        (0x11, 1, true, 0xff81_9abc),
        (0x12, 1, false, 0x7fc0_def0),
        (0x13, 1, true, 0xff80_1357),
    ] {
        let mut machine = cp1_machine();
        machine
            .state_mut()
            .cp1
            .fcsr_mut()
            .set_condition_code(Mips4Cp1ConditionCode::from_u8(0).unwrap(), condition);
        machine.write_gpr(1, u64::from(condition));
        machine.state_mut().cp1.fgr_mut().write_word(fgr(2), source);
        let bits = cop1_formatted(0x10, ft, 2, 6, function);

        assert_retired(machine.execute(bits), bits);
        assert_eq!(machine.state().cp1().fgr().read_word(fgr(6)), source);
        assert!(machine.state().cp1().fcsr().cause_flags().is_empty());
        assert!(machine.state().cp1().fcsr().flag_flags().is_empty());
    }

    for (function, ft, condition, source) in [
        (0x06_u8, 0_u8, false, 0x7ff0_0123_4567_89ab_u64),
        (0x11, 0, false, 0xffff_0123_4567_89ab),
        (0x11, 1, true, 0xfff4_3210_9876_5432),
        (0x12, 1, false, 0x7ff8_0000_0000_0001),
        (0x13, 1, true, 0xfff0_0000_0000_1357),
    ] {
        let mut machine = cp1_machine();
        machine
            .state_mut()
            .cp1
            .fcsr_mut()
            .set_condition_code(Mips4Cp1ConditionCode::from_u8(0).unwrap(), condition);
        machine.write_gpr(1, u64::from(condition));
        write_f64_bits(&mut machine, 2, source);
        let bits = cop1_formatted(0x11, ft, 2, 6, function);

        assert_retired(machine.execute(bits), bits);
        assert_eq!(read_f64_bits(&machine, 6), source);
        assert!(machine.state().cp1().fcsr().cause_flags().is_empty());
        assert!(machine.state().cp1().fcsr().flag_flags().is_empty());
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, C.cond.fmt and Appendix B.2.1.2.
#[test]
fn r5000_compare_distinguishes_legacy_quiet_and_signaling_nan() {
    for (nan, function, expected_flags) in [
        (0x7f80_0001_u32, 0x31_u8, FloatExceptionFlags::empty()),
        (0x7fc0_0001, 0x31, FloatExceptionFlags::INVALID),
        (0x7f80_0001, 0x39, FloatExceptionFlags::INVALID),
    ] {
        let mut machine = cp1_machine();
        machine.state_mut().cp1.fgr_mut().write_word(fgr(2), nan);
        write_f32(&mut machine, 4, 1.0);
        let bits = cop1_formatted(0x10, 4, 2, 0, function);

        assert_retired(machine.execute(bits), bits);
        assert!(
            machine
                .state()
                .cp1()
                .fcsr()
                .condition_code(Mips4Cp1ConditionCode::from_u8(0).unwrap())
        );
        assert_eq!(machine.state().cp1().fcsr().cause_flags(), expected_flags);
        assert_eq!(machine.state().cp1().fcsr().flag_flags(), expected_flags);
    }
}

#[test]
fn floating_to_fixed_limits_and_signaling_nan_use_distinct_exception_causes() {
    let mut out_of_range = cp1_machine();
    write_f64(&mut out_of_range, 2, 2_f64.powi(53));
    out_of_range
        .state_mut()
        .cp1
        .fgr_mut()
        .write_doubleword(
            Mips4Cp1RegisterMode::SixtyFourBit,
            fgr(4),
            0xfeed_face_dead_beef,
        )
        .unwrap();
    let cvt_l_d = cop1_formatted(0x11, 0, 2, 4, 0x25);
    assert!(matches!(
        out_of_range.execute(cvt_l_d),
        Mips4ExecutionBoundary::Exception {
            image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                reason: Mips4Exception::FloatingPoint,
                ..
            },
            ..
        }
    ));
    assert!(
        out_of_range
            .state()
            .cp1()
            .fcsr()
            .unimplemented_operation_cause()
    );
    assert_eq!(
        out_of_range
            .state()
            .cp1()
            .fgr()
            .read_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, fgr(4),)
            .unwrap(),
        0xfeed_face_dead_beef
    );

    let mut signaling_nan = cp1_machine();
    signaling_nan
        .state_mut()
        .cp1
        .fgr_mut()
        .write_word(fgr(2), 0x7fc0_0001);
    signaling_nan.write_gpr(1, 1 << 11);
    let ctc1 = cop1_transfer(0x06, 1, 31);
    assert!(matches!(
        signaling_nan.execute(ctc1),
        Mips4ExecutionBoundary::Retired { .. }
    ));
    let cvt_w_s = cop1_formatted(0x10, 0, 2, 4, 0x24);
    assert!(matches!(
        signaling_nan.execute(cvt_w_s),
        Mips4ExecutionBoundary::Exception { .. }
    ));
    assert!(
        !signaling_nan
            .state()
            .cp1()
            .fcsr()
            .unimplemented_operation_cause()
    );
    assert!(
        signaling_nan
            .state()
            .cp1()
            .fcsr()
            .cause_flags()
            .contains(FloatExceptionFlags::INVALID)
    );
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Table B-3; VR5000 User's Manual,
// sections 9.4.2 and 9.4.6.
#[test]
fn r5000_fixed_conversion_nan_and_range_results_are_bit_exact() {
    let mut signaling = cp1_machine();
    signaling
        .state_mut()
        .cp1
        .fgr_mut()
        .write_word(fgr(2), 0xffc0_0001);
    let cvt_w_s = cop1_formatted(0x10, 0, 2, 4, 0x24);
    assert_retired(signaling.execute(cvt_w_s), cvt_w_s);
    assert_eq!(signaling.state().cp1().fgr().read_word(fgr(4)), 0x7fff_ffff);
    assert_eq!(
        signaling.state().cp1().fcsr().cause_flags(),
        FloatExceptionFlags::INVALID
    );
    assert_eq!(
        signaling.state().cp1().fcsr().flag_flags(),
        FloatExceptionFlags::INVALID
    );

    let modes = [
        Mips4Cp1RoundingMode::RoundToNearest,
        Mips4Cp1RoundingMode::RoundTowardZero,
        Mips4Cp1RoundingMode::RoundTowardPositive,
        Mips4Cp1RoundingMode::RoundTowardNegative,
    ];
    for rounding_mode in modes {
        let mut word = cp1_machine();
        word.state_mut()
            .cp1
            .fcsr_mut()
            .set_rounding_mode(rounding_mode);
        write_f64(&mut word, 2, 2_147_483_648.0);
        word.state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(4), 0xfeed_face);
        let cvt_w_d = cop1_formatted(0x11, 0, 2, 4, 0x24);
        assert!(matches!(
            word.execute(cvt_w_d),
            Mips4ExecutionBoundary::Exception { .. }
        ));
        assert!(word.state().cp1().fcsr().unimplemented_operation_cause());
        assert_eq!(word.state().cp1().fgr().read_word(fgr(4)), 0xfeed_face);

        let mut long = cp1_machine();
        long.state_mut()
            .cp1
            .fcsr_mut()
            .set_rounding_mode(rounding_mode);
        write_f64(&mut long, 2, 9_007_199_254_740_992.0);
        write_f64_bits(&mut long, 4, 0xfeed_face_dead_beef);
        let cvt_l_d = cop1_formatted(0x11, 0, 2, 4, 0x25);
        assert!(matches!(
            long.execute(cvt_l_d),
            Mips4ExecutionBoundary::Exception { .. }
        ));
        assert!(long.state().cp1().fcsr().unimplemented_operation_cause());
        assert_eq!(read_f64_bits(&long, 4), 0xfeed_face_dead_beef);
    }
}

#[test]
fn floating_to_fixed_boundaries_apply_the_instruction_rounding_mode() {
    for (value, function, expected) in [
        (2_147_483_647.0_f64, 0x24_u8, Some(0x7fff_ffff_u64)),
        (-2_147_483_648.0_f64, 0x24, Some(0x8000_0000)),
        (2_147_483_648.0_f64, 0x24, None),
        (-2_147_483_649.0_f64, 0x24, None),
        (2_147_483_647.5_f64, 0x0d, Some(0x7fff_ffff)),
        (2_147_483_647.5_f64, 0x0e, None),
    ] {
        let mut machine = cp1_machine();
        write_f64(&mut machine, 2, value);
        machine
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(4), 0xfeed_face);
        let bits = cop1_formatted(0x11, 0, 2, 4, function);
        let boundary = machine.execute(bits);
        match expected {
            Some(expected) => {
                assert!(matches!(boundary, Mips4ExecutionBoundary::Retired { .. }));
                assert_eq!(
                    u64::from(machine.state().cp1().fgr().read_word(fgr(4))),
                    expected
                );
            }
            None => {
                assert!(matches!(boundary, Mips4ExecutionBoundary::Exception { .. }));
                assert!(machine.state().cp1().fcsr().unimplemented_operation_cause());
                assert_eq!(machine.state().cp1().fgr().read_word(fgr(4)), 0xfeed_face);
            }
        }
    }

    for (value, expected) in [
        (9_007_199_254_740_991.0_f64, Some(0x001f_ffff_ffff_ffff_u64)),
        (-9_007_199_254_740_992.0_f64, Some(0xffe0_0000_0000_0000)),
        (9_007_199_254_740_992.0_f64, None),
        (-9_007_199_254_740_994.0_f64, None),
    ] {
        let mut machine = cp1_machine();
        write_f64(&mut machine, 2, value);
        machine
            .state_mut()
            .cp1
            .fgr_mut()
            .write_doubleword(
                Mips4Cp1RegisterMode::SixtyFourBit,
                fgr(4),
                0xfeed_face_dead_beef,
            )
            .unwrap();
        let bits = cop1_formatted(0x11, 0, 2, 4, 0x25);
        let boundary = machine.execute(bits);
        match expected {
            Some(expected) => {
                assert!(matches!(boundary, Mips4ExecutionBoundary::Retired { .. }));
                assert_eq!(
                    machine
                        .state()
                        .cp1()
                        .fgr()
                        .read_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, fgr(4))
                        .unwrap(),
                    expected
                );
            }
            None => {
                assert!(matches!(boundary, Mips4ExecutionBoundary::Exception { .. }));
                assert!(machine.state().cp1().fcsr().unimplemented_operation_cause());
            }
        }
    }
}

#[test]
fn floating_exception_vectors_preserve_destinations_and_record_precise_causes() {
    for (lhs, rhs, function, enabled, expected) in [
        (
            1.0f32.to_bits(),
            0.0f32.to_bits(),
            0x03,
            FloatExceptionFlags::DIVIDE_BY_ZERO,
            FloatExceptionFlags::DIVIDE_BY_ZERO,
        ),
        (
            0.0f32.to_bits(),
            0.0f32.to_bits(),
            0x03,
            FloatExceptionFlags::INVALID,
            FloatExceptionFlags::INVALID,
        ),
        (
            0x7f7f_ffff,
            2.0f32.to_bits(),
            0x02,
            FloatExceptionFlags::OVERFLOW,
            FloatExceptionFlags::OVERFLOW,
        ),
        (
            1.0f32.to_bits(),
            3.0f32.to_bits(),
            0x03,
            FloatExceptionFlags::INEXACT,
            FloatExceptionFlags::INEXACT,
        ),
    ] {
        let mut machine = cp1_machine();
        machine.state_mut().cp1.fgr_mut().write_word(fgr(2), lhs);
        machine.state_mut().cp1.fgr_mut().write_word(fgr(4), rhs);
        machine
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(6), 0xfeed_face);
        machine.state_mut().cp1.fcsr_mut().set_enable_flags(enabled);
        let bits = cop1_formatted(0x10, 4, 2, 6, function);
        assert!(matches!(
            machine.execute(bits),
            Mips4ExecutionBoundary::Exception {
                image: crate::cpu::mips4::exception::Mips4ExceptionImage {
                    reason: Mips4Exception::FloatingPoint,
                    ..
                },
                ..
            }
        ));
        assert_eq!(machine.state().cp1().fgr().read_word(fgr(6)), 0xfeed_face);
        assert!(
            machine
                .state()
                .cp1()
                .fcsr()
                .cause_flags()
                .contains(expected)
        );
    }

    for unusual in [0x7f80_0001_u32, 0x0000_0001] {
        let mut machine = cp1_machine();
        machine
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(2), unusual);
        let bits = cop1_formatted(0x10, 4, 2, 6, 0x00);
        assert!(matches!(
            machine.execute(bits),
            Mips4ExecutionBoundary::Exception { .. }
        ));
        assert!(machine.state().cp1().fcsr().unimplemented_operation_cause());
    }

    let mut underflow = cp1_machine();
    underflow
        .state_mut()
        .cp1
        .fgr_mut()
        .write_word(fgr(2), 0x0080_0000);
    write_f32(&mut underflow, 4, 0.5);
    let multiply = cop1_formatted(0x10, 4, 2, 6, 0x02);
    assert!(matches!(
        underflow.execute(multiply),
        Mips4ExecutionBoundary::Exception { .. }
    ));
    assert!(
        underflow
            .state()
            .cp1()
            .fcsr()
            .unimplemented_operation_cause()
    );

    let mut flushed = cp1_machine();
    flushed.state_mut().cp1.fcsr_mut().set_flush_to_zero(true);
    flushed
        .state_mut()
        .cp1
        .fgr_mut()
        .write_word(fgr(2), 0x0080_0000);
    write_f32(&mut flushed, 4, 0.5);
    assert_retired(flushed.execute(multiply), multiply);
    assert_eq!(flushed.state().cp1().fgr().read_word(fgr(6)), 0);
    let underflow_inexact = FloatExceptionFlags::UNDERFLOW | FloatExceptionFlags::INEXACT;
    assert!(
        flushed
            .state()
            .cp1()
            .fcsr()
            .cause_flags()
            .contains(underflow_inexact)
    );
    assert!(
        flushed
            .state()
            .cp1()
            .fcsr()
            .flag_flags()
            .contains(underflow_inexact)
    );
}

// Oracle: VR5000 User's Manual, Table 8-4 and sections 9.4.5-9.4.6.
#[test]
fn r5000_flush_to_zero_matrix_matches_rounding_and_enable_rules() {
    let modes = [
        Mips4Cp1RoundingMode::RoundToNearest,
        Mips4Cp1RoundingMode::RoundTowardZero,
        Mips4Cp1RoundingMode::RoundTowardPositive,
        Mips4Cp1RoundingMode::RoundTowardNegative,
    ];
    let underflow_inexact = FloatExceptionFlags::UNDERFLOW | FloatExceptionFlags::INEXACT;
    let multiply = cop1_formatted(0x10, 4, 2, 6, 0x02);

    for rounding_mode in modes {
        for (negative, expected) in [
            (
                false,
                if matches!(rounding_mode, Mips4Cp1RoundingMode::RoundTowardPositive) {
                    0x0080_0000
                } else {
                    0
                },
            ),
            (
                true,
                if matches!(rounding_mode, Mips4Cp1RoundingMode::RoundTowardNegative) {
                    0x8080_0000
                } else {
                    0x8000_0000
                },
            ),
        ] {
            let mut machine = cp1_machine();
            machine.state_mut().cp1.fcsr_mut().set_flush_to_zero(true);
            machine
                .state_mut()
                .cp1
                .fcsr_mut()
                .set_rounding_mode(rounding_mode);
            machine
                .state_mut()
                .cp1
                .fgr_mut()
                .write_word(fgr(2), if negative { 0x8080_0000 } else { 0x0080_0000 });
            write_f32(&mut machine, 4, 0.5);

            assert_retired(machine.execute(multiply), multiply);
            assert_eq!(machine.state().cp1().fgr().read_word(fgr(6)), expected);
            assert_eq!(
                machine.state().cp1().fcsr().cause_flags(),
                underflow_inexact
            );
            assert_eq!(machine.state().cp1().fcsr().flag_flags(), underflow_inexact);
        }
    }

    for enable_flags in [
        FloatExceptionFlags::empty(),
        FloatExceptionFlags::UNDERFLOW,
        FloatExceptionFlags::INEXACT,
    ] {
        let mut machine = cp1_machine();
        machine
            .state_mut()
            .cp1
            .fcsr_mut()
            .set_flush_to_zero(!enable_flags.is_empty());
        machine
            .state_mut()
            .cp1
            .fcsr_mut()
            .set_enable_flags(enable_flags);
        machine
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(2), 0x0080_0000);
        write_f32(&mut machine, 4, 0.5);
        machine
            .state_mut()
            .cp1
            .fgr_mut()
            .write_word(fgr(6), 0xfeed_face);

        assert!(matches!(
            machine.execute(multiply),
            Mips4ExecutionBoundary::Exception { .. }
        ));
        assert!(machine.state().cp1().fcsr().unimplemented_operation_cause());
        assert!(machine.state().cp1().fcsr().cause_flags().is_empty());
        assert!(machine.state().cp1().fcsr().flag_flags().is_empty());
        assert_eq!(machine.state().cp1().fgr().read_word(fgr(6)), 0xfeed_face);
    }
}

#[test]
fn cp1_memory_instructions_emit_offset_and_indexed_transactions() {
    for (bits, expected_address) in [
        (i_type(0x31, 1, 2, 4), 0x1004_u64),
        (cop1x(1, 2, 0, 4, 0x00), 0x1008_u64),
    ] {
        let mut machine = cp1_machine();
        machine.write_gpr(1, 0xffff_ffff_a000_1000);
        machine.write_gpr(2, 8);
        let ExecutionAction::Transaction(transaction) = machine.begin(bits) else {
            panic!("expected CP1 memory transaction");
        };
        assert!(matches!(
            transaction.payload,
            Mips4ExecutionTransaction::Read {
                physical_address,
                ..
            } if physical_address == expected_address
        ));
        let ExecutionAction::Boundary(boundary) = machine.complete(
            transaction,
            Mips4ExecutionCompletion::ReadData(0x0000_0000_3f80_0000),
        ) else {
            panic!("expected CP1 memory boundary");
        };
        assert!(matches!(boundary, Mips4ExecutionBoundary::Retired { .. }));
    }
}

fn cp1_machine() -> ConformanceMachine {
    let mut machine = ConformanceMachine::new(Mips4Endianness::Big);
    machine
        .state_mut()
        .cp0
        .write(Mips4Cp0Register::Status, STATUS_CP1_FR_ERL_BEV)
        .unwrap();
    machine
}

fn write_f32(machine: &mut ConformanceMachine, register: u8, value: f32) {
    machine
        .state_mut()
        .cp1
        .fgr_mut()
        .write_word(fgr(register), value.to_bits());
}

fn write_f64(machine: &mut ConformanceMachine, register: u8, value: f64) {
    write_f64_bits(machine, register, value.to_bits());
}

fn write_f64_bits(machine: &mut ConformanceMachine, register: u8, value: u64) {
    machine
        .state_mut()
        .cp1
        .fgr_mut()
        .write_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, fgr(register), value)
        .unwrap();
}

fn read_f64_bits(machine: &ConformanceMachine, register: u8) -> u64 {
    machine
        .state()
        .cp1()
        .fgr()
        .read_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, fgr(register))
        .unwrap()
}

fn fgr(register: u8) -> Mips4Cp1FgrIndex {
    Mips4Cp1FgrIndex::from_u8(register).unwrap()
}

const fn cop1_transfer(format: u8, rt: u8, fs: u8) -> u32 {
    ((0x11_u32) << 26) | ((format as u32) << 21) | ((rt as u32) << 16) | ((fs as u32) << 11)
}

const fn cop1_formatted(format: u8, ft: u8, fs: u8, fd: u8, function: u8) -> u32 {
    ((0x11_u32) << 26)
        | ((format as u32) << 21)
        | ((ft as u32) << 16)
        | ((fs as u32) << 11)
        | ((fd as u32) << 6)
        | function as u32
}

const fn cop1x(fr: u8, ft: u8, fs: u8, fd: u8, function: u8) -> u32 {
    ((0x13_u32) << 26)
        | ((fr as u32) << 21)
        | ((ft as u32) << 16)
        | ((fs as u32) << 11)
        | ((fd as u32) << 6)
        | function as u32
}

fn record_formatted_coverage(covered: &mut [bool; 46], bits: u32) {
    let Some(Mips4Cp1Decode::Instruction(Mips4Cp1InstructionClass::Formatted {
        operation, ..
    })) = decode_instruction(Mips4Instruction::from_bits(bits))
    else {
        panic!("expected formatted CP1 instruction for {bits:#010x}");
    };
    covered[operation_key(operation)] = true;
}

fn operation_key(operation: Mips4Cp1Operation) -> usize {
    match operation {
        Mips4Cp1Operation::Absolute => 0,
        Mips4Cp1Operation::Add => 1,
        Mips4Cp1Operation::CeilLong => 2,
        Mips4Cp1Operation::CeilWord => 3,
        Mips4Cp1Operation::ConvertDouble => 4,
        Mips4Cp1Operation::ConvertLong => 5,
        Mips4Cp1Operation::ConvertSingle => 6,
        Mips4Cp1Operation::ConvertWord => 7,
        Mips4Cp1Operation::Divide => 8,
        Mips4Cp1Operation::FloorLong => 9,
        Mips4Cp1Operation::FloorWord => 10,
        Mips4Cp1Operation::MultiplyAdd => 11,
        Mips4Cp1Operation::Move => 12,
        Mips4Cp1Operation::MoveConditionalFalse => 13,
        Mips4Cp1Operation::MoveConditionalTrue => 14,
        Mips4Cp1Operation::MoveConditionalNonzero => 15,
        Mips4Cp1Operation::MoveConditionalZero => 16,
        Mips4Cp1Operation::MultiplySubtract => 17,
        Mips4Cp1Operation::Multiply => 18,
        Mips4Cp1Operation::Negate => 19,
        Mips4Cp1Operation::NegativeMultiplyAdd => 20,
        Mips4Cp1Operation::NegativeMultiplySubtract => 21,
        Mips4Cp1Operation::Reciprocal => 22,
        Mips4Cp1Operation::RoundLong => 23,
        Mips4Cp1Operation::RoundWord => 24,
        Mips4Cp1Operation::ReciprocalSquareRoot => 25,
        Mips4Cp1Operation::SquareRoot => 26,
        Mips4Cp1Operation::Subtract => 27,
        Mips4Cp1Operation::TruncLong => 28,
        Mips4Cp1Operation::TruncWord => 29,
        Mips4Cp1Operation::Compare(condition) => 30 + compare_key(condition),
    }
}

const fn class_key(class: Mips4Cp1InstructionClass) -> usize {
    match class {
        Mips4Cp1InstructionClass::OffsetMemory(_) => 0,
        Mips4Cp1InstructionClass::RegisterTransfer(_) => 1,
        Mips4Cp1InstructionClass::Branch(_) => 2,
        Mips4Cp1InstructionClass::Formatted { .. } => 3,
        Mips4Cp1InstructionClass::IndexedMemory(_) => 4,
        Mips4Cp1InstructionClass::IndexedPrefetch => 5,
        Mips4Cp1InstructionClass::Movci(_) => 6,
    }
}

const fn compare_key(condition: Mips4Cp1CompareCondition) -> usize {
    match condition {
        Mips4Cp1CompareCondition::False => 0,
        Mips4Cp1CompareCondition::Unordered => 1,
        Mips4Cp1CompareCondition::Equal => 2,
        Mips4Cp1CompareCondition::UnorderedOrEqual => 3,
        Mips4Cp1CompareCondition::OrderedLessThan => 4,
        Mips4Cp1CompareCondition::UnorderedLessThan => 5,
        Mips4Cp1CompareCondition::OrderedLessOrEqual => 6,
        Mips4Cp1CompareCondition::UnorderedLessOrEqual => 7,
        Mips4Cp1CompareCondition::SignalingFalse => 8,
        Mips4Cp1CompareCondition::NotGreaterLessOrEqual => 9,
        Mips4Cp1CompareCondition::SignalingEqual => 10,
        Mips4Cp1CompareCondition::NotGreaterLess => 11,
        Mips4Cp1CompareCondition::LessThan => 12,
        Mips4Cp1CompareCondition::NotGreaterOrEqual => 13,
        Mips4Cp1CompareCondition::LessOrEqual => 14,
        Mips4Cp1CompareCondition::NotGreaterThan => 15,
    }
}
