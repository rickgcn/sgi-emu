use super::*;

#[test]
fn fgr_indices_accept_only_implemented_registers() {
    for index in 0..32 {
        let fgr = Mips4Cp1FgrIndex::from_u8(index).unwrap();
        assert_eq!(fgr.number(), index);
    }

    assert_eq!(Mips4Cp1FgrIndex::from_u8(32), None);
}

#[test]
fn condition_codes_accept_only_mips4_fcc_range() {
    for index in 0..8 {
        let condition_code = Mips4Cp1ConditionCode::from_u8(index).unwrap();
        assert_eq!(condition_code.number(), index);
    }

    assert_eq!(Mips4Cp1ConditionCode::from_u8(8), None);
}

#[test]
fn formats_round_trip_raw_fmt_fields() {
    let formats = [
        (CP1_FMT_SINGLE, Mips4Cp1Format::Single),
        (CP1_FMT_DOUBLE, Mips4Cp1Format::Double),
        (CP1_FMT_WORD, Mips4Cp1Format::Word),
        (CP1_FMT_LONG, Mips4Cp1Format::Long),
    ];

    for (bits, format) in formats {
        assert_eq!(Mips4Cp1Format::from_fmt_field(bits), Some(format));
        assert_eq!(format.fmt_field(), bits);
    }

    assert_eq!(Mips4Cp1Format::from_fmt_field(0x08), None);
}

#[test]
fn fgr_file_starts_zero_and_resets() {
    let mut fgr = Mips4Cp1FgrFile::new();
    let index = Mips4Cp1FgrIndex::from_u8(4).unwrap();

    assert_eq!(fgr.read_word(index), 0);
    assert_eq!(
        fgr.read_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, index),
        Ok(0)
    );

    fgr.write_word(index, 0x1234_5678);
    fgr.reset();

    assert_eq!(fgr.read_word(index), 0);
}

#[test]
fn word_access_uses_low_32_bit_payload() {
    let mut fgr = Mips4Cp1FgrFile::new();
    let index = Mips4Cp1FgrIndex::from_u8(5).unwrap();

    fgr.write_doubleword(
        Mips4Cp1RegisterMode::SixtyFourBit,
        index,
        0xfeed_face_1234_5678,
    )
    .unwrap();
    assert_eq!(fgr.read_word(index), 0x1234_5678);

    fgr.write_word(index, 0x89ab_cdef);
    assert_eq!(
        fgr.read_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, index),
        Ok(0x0000_0000_89ab_cdef)
    );
}

#[test]
fn doubleword_access_uses_register_pairs_in_32_bit_mode() {
    let mut fgr = Mips4Cp1FgrFile::new();
    let even = Mips4Cp1FgrIndex::from_u8(6).unwrap();
    let odd = Mips4Cp1FgrIndex::from_u8(7).unwrap();

    fgr.write_doubleword(
        Mips4Cp1RegisterMode::ThirtyTwoBit,
        even,
        0x1122_3344_5566_7788,
    )
    .unwrap();

    assert_eq!(fgr.read_word(even), 0x5566_7788);
    assert_eq!(fgr.read_word(odd), 0x1122_3344);
    assert_eq!(
        fgr.read_doubleword(Mips4Cp1RegisterMode::ThirtyTwoBit, even),
        Ok(0x1122_3344_5566_7788)
    );
}

#[test]
fn doubleword_access_rejects_odd_registers_in_32_bit_mode() {
    let mut fgr = Mips4Cp1FgrFile::new();
    let odd = Mips4Cp1FgrIndex::from_u8(31).unwrap();

    assert_eq!(
        fgr.read_doubleword(Mips4Cp1RegisterMode::ThirtyTwoBit, odd),
        Err(Mips4Cp1FgrAccessError::OddRegisterInThirtyTwoBitMode { index: odd })
    );
    assert_eq!(
        fgr.write_doubleword(Mips4Cp1RegisterMode::ThirtyTwoBit, odd, 1),
        Err(Mips4Cp1FgrAccessError::OddRegisterInThirtyTwoBitMode { index: odd })
    );
}

#[test]
fn doubleword_access_uses_single_register_in_64_bit_mode() {
    let mut fgr = Mips4Cp1FgrFile::new();
    let odd = Mips4Cp1FgrIndex::from_u8(31).unwrap();

    fgr.write_doubleword(
        Mips4Cp1RegisterMode::SixtyFourBit,
        odd,
        0x0123_4567_89ab_cdef,
    )
    .unwrap();

    assert_eq!(
        fgr.read_doubleword(Mips4Cp1RegisterMode::SixtyFourBit, odd),
        Ok(0x0123_4567_89ab_cdef)
    );
}

#[test]
fn control_register_numbers_accept_only_fcr0_and_fcr31() {
    assert_eq!(
        Mips4Cp1ControlRegister::from_u8(0),
        Some(Mips4Cp1ControlRegister::ImplementationRevision)
    );
    assert_eq!(
        Mips4Cp1ControlRegister::from_u8(31),
        Some(Mips4Cp1ControlRegister::ControlStatus)
    );
    assert_eq!(Mips4Cp1ControlRegister::ImplementationRevision.number(), 0);
    assert_eq!(Mips4Cp1ControlRegister::ControlStatus.number(), 31);

    for register in 1..31 {
        assert_eq!(Mips4Cp1ControlRegister::from_u8(register), None);
    }
}

#[test]
fn fcr0_masks_reserved_bits_and_exposes_identity_fields() {
    let fcr0 = Mips4Cp1Fcr0::from_bits(0xdead_2310);

    assert_eq!(fcr0.bits(), 0x0000_2310);
    assert_eq!(fcr0.implementation(), 0x23);
    assert_eq!(fcr0.revision(), 0x10);
}

#[test]
fn fcsr_masks_reserved_bits() {
    assert_eq!(Mips4Cp1Fcsr::from_bits(u32::MAX).bits(), FCSR_READABLE_MASK);
    assert_eq!(Mips4Cp1Fcsr::from_bits(u32::MAX).bits() & 0x007c_0000, 0);
}

#[test]
fn fcsr_exposes_condition_codes_and_flush_bit() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);
    let cc0 = Mips4Cp1ConditionCode::from_u8(0).unwrap();
    let cc1 = Mips4Cp1ConditionCode::from_u8(1).unwrap();
    let cc7 = Mips4Cp1ConditionCode::from_u8(7).unwrap();

    fcsr.set_condition_code(cc0, true);
    fcsr.set_condition_code(cc1, true);
    fcsr.set_condition_code(cc7, true);
    fcsr.set_flush_to_zero(true);

    assert!(fcsr.condition_code(cc0));
    assert!(fcsr.condition_code(cc1));
    assert!(fcsr.condition_code(cc7));
    assert!(fcsr.flush_to_zero());
    assert_eq!(fcsr.bits() & FCSR_FCC0, FCSR_FCC0);
    assert_eq!(fcsr.bits() & (1 << 25), 1 << 25);
    assert_eq!(fcsr.bits() & (1 << 31), 1 << 31);

    fcsr.set_condition_code(cc1, false);
    fcsr.set_flush_to_zero(false);

    assert!(!fcsr.condition_code(cc1));
    assert!(!fcsr.flush_to_zero());
}

#[test]
fn fcsr_exposes_cause_enable_and_flag_fields() {
    let bits = FCSR_CAUSE_UNIMPLEMENTED
        | (((FloatExceptionFlags::INVALID | FloatExceptionFlags::OVERFLOW).bits() as u32)
            << FCSR_CAUSE_SHIFT)
        | (((FloatExceptionFlags::DIVIDE_BY_ZERO | FloatExceptionFlags::UNDERFLOW).bits() as u32)
            << FCSR_ENABLE_SHIFT)
        | (((FloatExceptionFlags::INEXACT | FloatExceptionFlags::UNDERFLOW).bits() as u32)
            << FCSR_FLAG_SHIFT);
    let mut fcsr = Mips4Cp1Fcsr::from_bits(bits);

    assert!(fcsr.unimplemented_operation_cause());
    assert!(fcsr.cause_flags().contains(FloatExceptionFlags::INVALID));
    assert!(fcsr.cause_flags().contains(FloatExceptionFlags::OVERFLOW));
    assert!(
        fcsr.enable_flags()
            .contains(FloatExceptionFlags::DIVIDE_BY_ZERO)
    );
    assert!(fcsr.enable_flags().contains(FloatExceptionFlags::UNDERFLOW));
    assert!(fcsr.flag_flags().contains(FloatExceptionFlags::INEXACT));
    assert!(fcsr.flag_flags().contains(FloatExceptionFlags::UNDERFLOW));

    fcsr.set_enable_flags(FloatExceptionFlags::INVALID | FloatExceptionFlags::INEXACT);
    fcsr.set_flag_flags(FloatExceptionFlags::OVERFLOW);

    assert_eq!(
        fcsr.enable_flags(),
        FloatExceptionFlags::INVALID | FloatExceptionFlags::INEXACT
    );
    assert_eq!(fcsr.flag_flags(), FloatExceptionFlags::OVERFLOW);
}

#[test]
fn rounding_modes_map_mips4_bits_to_backend_modes() {
    let modes = [
        (
            0,
            Mips4Cp1RoundingMode::RoundToNearest,
            FloatRoundingMode::NearestEven,
        ),
        (
            1,
            Mips4Cp1RoundingMode::RoundTowardZero,
            FloatRoundingMode::TowardZero,
        ),
        (
            2,
            Mips4Cp1RoundingMode::RoundTowardPositive,
            FloatRoundingMode::TowardPositive,
        ),
        (
            3,
            Mips4Cp1RoundingMode::RoundTowardNegative,
            FloatRoundingMode::TowardNegative,
        ),
    ];

    for (bits, mode, backend_mode) in modes {
        assert_eq!(Mips4Cp1RoundingMode::from_bits(bits), Some(mode));
        assert_eq!(mode.bits(), bits);
        assert_eq!(mode.to_float_rounding_mode(), backend_mode);
        assert_eq!(
            mode.to_float_control(),
            FloatControl::new(backend_mode, FloatTininessMode::AfterRounding)
        );
    }

    assert_eq!(Mips4Cp1RoundingMode::from_bits(4), None);
}

#[test]
fn fcsr_rounding_mode_drives_normal_float_control() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);

    fcsr.set_rounding_mode(Mips4Cp1RoundingMode::RoundTowardPositive);
    assert_eq!(
        fcsr.rounding_mode(),
        Mips4Cp1RoundingMode::RoundTowardPositive
    );
    assert_eq!(
        fcsr.float_control(),
        FloatControl::new(
            FloatRoundingMode::TowardPositive,
            FloatTininessMode::AfterRounding
        )
    );

    fcsr.set_rounding_mode(Mips4Cp1RoundingMode::RoundTowardNegative);
    assert_eq!(
        fcsr.rounding_mode(),
        Mips4Cp1RoundingMode::RoundTowardNegative
    );
    assert_eq!(
        fcsr.float_control(),
        FloatControl::new(
            FloatRoundingMode::TowardNegative,
            FloatTininessMode::AfterRounding
        )
    );
}

#[test]
fn conversion_rounding_can_use_fcsr_or_directed_modes() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);
    fcsr.set_rounding_mode(Mips4Cp1RoundingMode::RoundTowardPositive);

    assert_eq!(
        Mips4Cp1ConversionRoundingMode::Fcsr.rounding_mode(fcsr),
        Mips4Cp1RoundingMode::RoundTowardPositive
    );
    assert_eq!(
        Mips4Cp1ConversionRoundingMode::Round.rounding_mode(fcsr),
        Mips4Cp1RoundingMode::RoundToNearest
    );
    assert_eq!(
        Mips4Cp1ConversionRoundingMode::Trunc.rounding_mode(fcsr),
        Mips4Cp1RoundingMode::RoundTowardZero
    );
    assert_eq!(
        Mips4Cp1ConversionRoundingMode::Ceil.rounding_mode(fcsr),
        Mips4Cp1RoundingMode::RoundTowardPositive
    );
    assert_eq!(
        Mips4Cp1ConversionRoundingMode::Floor.rounding_mode(fcsr),
        Mips4Cp1RoundingMode::RoundTowardNegative
    );
}

#[test]
fn recording_float_flags_replaces_cause_and_sets_sticky_flags_when_not_trapping() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(FCSR_CAUSE_UNIMPLEMENTED);
    fcsr.set_enable_flags(FloatExceptionFlags::DIVIDE_BY_ZERO);
    fcsr.set_flag_flags(FloatExceptionFlags::INEXACT);

    assert_eq!(
        fcsr.record_float_flags(FloatExceptionFlags::INVALID | FloatExceptionFlags::UNDERFLOW),
        Ok(())
    );

    assert!(!fcsr.unimplemented_operation_cause());
    assert_eq!(
        fcsr.cause_flags(),
        FloatExceptionFlags::INVALID | FloatExceptionFlags::UNDERFLOW
    );
    assert_eq!(
        fcsr.flag_flags(),
        FloatExceptionFlags::INEXACT
            | FloatExceptionFlags::INVALID
            | FloatExceptionFlags::UNDERFLOW
    );
}

#[test]
fn recording_enabled_float_flags_traps_without_updating_sticky_flags() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);
    fcsr.set_enable_flags(FloatExceptionFlags::OVERFLOW);
    fcsr.set_flag_flags(FloatExceptionFlags::INEXACT);

    assert_eq!(
        fcsr.record_float_flags(FloatExceptionFlags::OVERFLOW | FloatExceptionFlags::INEXACT),
        Err(Mips4Exception::FloatingPoint)
    );

    assert_eq!(
        fcsr.cause_flags(),
        FloatExceptionFlags::OVERFLOW | FloatExceptionFlags::INEXACT
    );
    assert_eq!(fcsr.flag_flags(), FloatExceptionFlags::INEXACT);
}

#[test]
fn recording_empty_float_flags_clears_cause_without_setting_sticky_flags() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(FCSR_CAUSE_MASK | FCSR_FLAG_MASK);

    assert_eq!(
        fcsr.record_float_flags(FloatExceptionFlags::empty()),
        Ok(())
    );

    assert!(!fcsr.unimplemented_operation_cause());
    assert!(fcsr.cause_flags().is_empty());
    assert_eq!(
        fcsr.flag_flags(),
        FloatExceptionFlags::from_bits_truncate(0x1f)
    );
}

#[test]
fn recording_unimplemented_operation_sets_e_cause_and_traps() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(FCSR_CAUSE_IEEE_MASK | FCSR_FLAG_MASK);

    assert_eq!(
        fcsr.record_unimplemented_operation(),
        Mips4Exception::FloatingPoint
    );

    assert!(fcsr.unimplemented_operation_cause());
    assert!(fcsr.cause_flags().is_empty());
    assert_eq!(
        fcsr.flag_flags(),
        FloatExceptionFlags::from_bits_truncate(0x1f)
    );
}

#[test]
fn decide_float_exception_traps_when_cause_and_enable_overlap() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);
    fcsr.set_enable_flags(FloatExceptionFlags::OVERFLOW);
    fcsr.set_flag_flags(FloatExceptionFlags::INEXACT);

    let decision = decide_float_exception(
        fcsr,
        FloatExceptionFlags::OVERFLOW | FloatExceptionFlags::UNDERFLOW,
    );

    assert!(decision.traps);
    assert_eq!(decision.flag_flags, FloatExceptionFlags::INEXACT);
}

#[test]
fn decide_float_exception_unions_flags_when_not_trapping() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);
    fcsr.set_enable_flags(FloatExceptionFlags::DIVIDE_BY_ZERO);
    fcsr.set_flag_flags(FloatExceptionFlags::INEXACT);

    let decision = decide_float_exception(
        fcsr,
        FloatExceptionFlags::INVALID | FloatExceptionFlags::UNDERFLOW,
    );

    assert!(!decision.traps);
    assert_eq!(
        decision.flag_flags,
        FloatExceptionFlags::INEXACT
            | FloatExceptionFlags::INVALID
            | FloatExceptionFlags::UNDERFLOW
    );
}

#[test]
fn decide_float_exception_empty_cause_never_traps() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);
    fcsr.set_enable_flags(FloatExceptionFlags::OVERFLOW);
    fcsr.set_flag_flags(FloatExceptionFlags::INEXACT);

    let decision = decide_float_exception(fcsr, FloatExceptionFlags::empty());

    assert!(!decision.traps);
    assert_eq!(decision.flag_flags, FloatExceptionFlags::INEXACT);
}

#[test]
fn decide_unimplemented_operation_always_traps_keeping_flags() {
    let mut fcsr = Mips4Cp1Fcsr::from_bits(0);
    fcsr.set_flag_flags(FloatExceptionFlags::INEXACT | FloatExceptionFlags::OVERFLOW);

    let decision = decide_unimplemented_operation(fcsr);

    assert!(decision.traps);
    assert_eq!(
        decision.flag_flags,
        FloatExceptionFlags::INEXACT | FloatExceptionFlags::OVERFLOW
    );
}

#[test]
fn cp1_register_file_reads_and_writes_control_registers() {
    let mut cp1 = Mips4Cp1::new(0xffff_2310);

    assert_eq!(cp1.fcr0().bits(), 0x2310);
    assert_eq!(
        cp1.read_control(Mips4Cp1ControlRegister::ImplementationRevision),
        0x2310
    );
    assert_eq!(cp1.read_control(Mips4Cp1ControlRegister::ControlStatus), 0);

    assert_eq!(
        cp1.write_control(Mips4Cp1ControlRegister::ImplementationRevision, 0),
        Err(Mips4Cp1WriteError::ReadOnlyControlRegister {
            register: Mips4Cp1ControlRegister::ImplementationRevision
        })
    );

    cp1.write_control(Mips4Cp1ControlRegister::ControlStatus, u32::MAX)
        .unwrap();
    assert_eq!(
        cp1.read_control(Mips4Cp1ControlRegister::ControlStatus),
        FCSR_READABLE_MASK
    );
}

#[test]
fn cp1_register_file_exposes_mutable_fgr_and_fcsr_state() {
    let mut cp1 = Mips4Cp1::new(0);
    let index = Mips4Cp1FgrIndex::from_u8(2).unwrap();

    cp1.fgr_mut().write_word(index, 0xfeed_face);
    cp1.fcsr_mut()
        .set_rounding_mode(Mips4Cp1RoundingMode::RoundTowardZero);

    assert_eq!(cp1.fgr().read_word(index), 0xfeed_face);
    assert_eq!(
        cp1.fcsr().rounding_mode(),
        Mips4Cp1RoundingMode::RoundTowardZero
    );
}

#[test]
fn cp1_instruction_decodes_only_cp1_related_opcodes() {
    let opcodes = [
        MIPS4_COP1_OPCODE,
        MIPS4_COP1X_OPCODE,
        MIPS4_LWC1_OPCODE,
        MIPS4_LDC1_OPCODE,
        MIPS4_SWC1_OPCODE,
        MIPS4_SDC1_OPCODE,
    ];

    for opcode in opcodes {
        let bits = (opcode as u32) << 26;
        let instruction = Mips4Cp1Instruction::from_bits(bits).unwrap();
        assert_eq!(instruction.bits(), bits);
        assert_eq!(instruction.opcode(), opcode);
        assert_eq!(instruction.instruction(), Mips4Instruction::from_bits(bits));
    }

    assert_eq!(Mips4Cp1Instruction::from_bits(0), None);
}

#[test]
fn cp1_instruction_extracts_raw_cop1_fields() {
    let bits = ((MIPS4_COP1_OPCODE as u32) << 26)
        | ((CP1_FMT_DOUBLE as u32) << 21)
        | (2 << 16)
        | (3 << 11)
        | (4 << 6)
        | 0x21;
    let instruction = Mips4Cp1Instruction::from_bits(bits).unwrap();

    assert!(instruction.is_cop1());
    assert!(!instruction.is_cop1x());
    assert_eq!(instruction.fmt(), CP1_FMT_DOUBLE);
    assert_eq!(instruction.ft(), 2);
    assert_eq!(instruction.fs(), 3);
    assert_eq!(instruction.fd(), 4);
    assert_eq!(instruction.funct(), 0x21);
}

#[test]
fn cp1_instruction_extracts_offset_load_store_fields() {
    let bits = ((MIPS4_LWC1_OPCODE as u32) << 26) | (7 << 21) | (8 << 16) | 0xfffc;
    let instruction = Mips4Cp1Instruction::from_bits(bits).unwrap();

    assert_eq!(instruction.base(), 7);
    assert_eq!(instruction.ft(), 8);
    assert_eq!(instruction.offset(), -4);
}

#[test]
fn cp1_instruction_extracts_branch_condition_bits() {
    let bits = ((MIPS4_COP1_OPCODE as u32) << 26)
        | ((CP1_FMT_BRANCH as u32) << 21)
        | (0b10110 << 16)
        | 0x0004;
    let instruction = Mips4Cp1Instruction::from_bits(bits).unwrap();

    assert!(instruction.is_branch_format());
    assert_eq!(instruction.branch_condition_code_bits(), 0b101);
    assert!(instruction.branch_nullify_delay_slot_bit());
    assert!(!instruction.branch_true_bit());
}

#[test]
fn cp1_instruction_extracts_compare_and_conditional_move_bits() {
    let bits = ((MIPS4_COP1_OPCODE as u32) << 26)
        | ((CP1_FMT_SINGLE as u32) << 21)
        | (0b1 << 16)
        | (0b11010 << 6)
        | 0x11;
    let instruction = Mips4Cp1Instruction::from_bits(bits).unwrap();

    assert_eq!(instruction.condition_code_bits(), 0b110);
    assert!(instruction.condition_true_bit());
}

#[test]
fn fpu_conditional_move_decision_truth_table() {
    assert!(Mips4Cp1MoveDecision::move_conditional_false(false).is_move());
    assert!(!Mips4Cp1MoveDecision::move_conditional_false(true).is_move());
    assert!(Mips4Cp1MoveDecision::move_conditional_true(true).is_move());
    assert!(!Mips4Cp1MoveDecision::move_conditional_true(false).is_move());
    assert!(Mips4Cp1MoveDecision::move_conditional_nonzero(1).is_move());
    assert!(!Mips4Cp1MoveDecision::move_conditional_nonzero(0).is_move());
    assert!(Mips4Cp1MoveDecision::move_conditional_zero(0).is_move());
    assert!(!Mips4Cp1MoveDecision::move_conditional_zero(1).is_move());

    assert_eq!(
        Mips4Cp1MoveDecision::move_conditional_false(false),
        Mips4Cp1MoveDecision::Move
    );
    assert_eq!(
        Mips4Cp1MoveDecision::move_conditional_false(true),
        Mips4Cp1MoveDecision::KeepDestination
    );
    assert_eq!(
        Mips4Cp1MoveDecision::move_conditional_zero(0),
        Mips4Cp1MoveDecision::Move
    );
    assert_eq!(
        Mips4Cp1MoveDecision::move_conditional_nonzero(2),
        Mips4Cp1MoveDecision::Move
    );
}
