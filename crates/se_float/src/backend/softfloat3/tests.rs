use super::*;

const CONTROL: FloatControl = FloatControl::new(
    FloatRoundingMode::NearestEven,
    FloatTininessMode::AfterRounding,
);
const DEFAULT_QNAN_F32: u32 = 0x7fbf_ffff;
const DEFAULT_QNAN_F64: u64 = 0x7ff7_ffff_ffff_ffff;

#[test]
fn mips4_softfloat_adds_exact_f32_values() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.add_f32(
        CONTROL,
        Float32Bits::new(1.5_f32.to_bits()),
        Float32Bits::new(2.25_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), 3.75_f32.to_bits());
    assert!(result.flags.is_empty());
}

#[test]
fn mips4_softfloat_adds_exact_f64_values() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.add_f64(
        CONTROL,
        Float64Bits::new(1.5_f64.to_bits()),
        Float64Bits::new(2.25_f64.to_bits()),
    );

    assert_eq!(result.value.bits(), 3.75_f64.to_bits());
    assert!(result.flags.is_empty());
}

#[test]
fn mips4_softfloat_reports_divide_by_zero() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(1.0_f32.to_bits()),
        Float32Bits::new(0.0_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), f32::INFINITY.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::DIVIDE_BY_ZERO));
}

#[test]
fn mips4_softfloat_reports_invalid_zero_divided_by_zero() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(0.0_f32.to_bits()),
        Float32Bits::new(0.0_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), DEFAULT_QNAN_F32);
    assert_eq!(result.flags, FloatExceptionFlags::INVALID);
}

#[test]
fn mips4_softfloat_reports_inexact_division() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(1.0_f32.to_bits()),
        Float32Bits::new(3.0_f32.to_bits()),
    );

    assert!(result.flags.contains(FloatExceptionFlags::INEXACT));
}

#[test]
fn mips4_softfloat_reports_overflow() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.mul_f32(
        CONTROL,
        Float32Bits::new(f32::MAX.to_bits()),
        Float32Bits::new(2.0_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), f32::INFINITY.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::OVERFLOW));
    assert!(result.flags.contains(FloatExceptionFlags::INEXACT));
}

#[test]
fn mips4_softfloat_reports_underflow() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.mul_f32(
        CONTROL,
        Float32Bits::new(0x0000_0001),
        Float32Bits::new(0.5_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), 0);
    assert!(result.flags.contains(FloatExceptionFlags::UNDERFLOW));
    assert!(result.flags.contains(FloatExceptionFlags::INEXACT));
}

#[test]
fn mips4_softfloat_conversions_honor_explicit_rounding() {
    let backend = Mips4SoftFloatBackend::new();
    let value = Float32Bits::new(1.9_f32.to_bits());

    assert_eq!(
        backend
            .f32_to_i32(
                FloatControl::with_rounding_mode(FloatRoundingMode::TowardZero),
                value,
            )
            .value,
        1
    );
    assert_eq!(
        backend
            .f32_to_i32(
                FloatControl::with_rounding_mode(FloatRoundingMode::TowardPositive),
                value,
            )
            .value,
        2
    );
    assert_eq!(
        backend
            .f32_to_i32(
                FloatControl::with_rounding_mode(FloatRoundingMode::TowardNegative),
                value,
            )
            .value,
        1
    );
}

#[test]
fn mips4_softfloat_integer_to_float_conversion_reports_inexact() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.i32_to_f32(CONTROL, 16_777_217);

    assert_eq!(result.value.bits(), 16_777_216_f32.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::INEXACT));
}

#[test]
fn mips4_softfloat_compare_reports_ordered_relations() {
    let backend = Mips4SoftFloatBackend::new();

    assert_eq!(
        backend
            .compare_f32(
                CONTROL,
                FloatCompareMode::Quiet,
                Float32Bits::new(1.0_f32.to_bits()),
                Float32Bits::new(2.0_f32.to_bits()),
            )
            .value,
        FloatRelation::Less
    );
    assert_eq!(
        backend
            .compare_f32(
                CONTROL,
                FloatCompareMode::Quiet,
                Float32Bits::new(2.0_f32.to_bits()),
                Float32Bits::new(2.0_f32.to_bits()),
            )
            .value,
        FloatRelation::Equal
    );
    assert_eq!(
        backend
            .compare_f64(
                CONTROL,
                FloatCompareMode::Quiet,
                Float64Bits::new(3.0_f64.to_bits()),
                Float64Bits::new(2.0_f64.to_bits()),
            )
            .value,
        FloatRelation::Greater
    );
}

#[test]
fn mips4_softfloat_compare_reports_unordered_without_quiet_invalid() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.compare_f32(
        CONTROL,
        FloatCompareMode::Quiet,
        Float32Bits::new(0x7f80_0001),
        Float32Bits::new(1.0_f32.to_bits()),
    );

    assert_eq!(result.value, FloatRelation::Unordered);
    assert!(!result.flags.contains(FloatExceptionFlags::INVALID));
}

#[test]
fn mips4_softfloat_signaling_compare_reports_invalid_for_unordered() {
    let backend = Mips4SoftFloatBackend::new();
    let result = backend.compare_f32(
        CONTROL,
        FloatCompareMode::Signaling,
        Float32Bits::new(0x7fc0_0000),
        Float32Bits::new(1.0_f32.to_bits()),
    );

    assert_eq!(result.value, FloatRelation::Unordered);
    assert!(result.flags.contains(FloatExceptionFlags::INVALID));
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Appendix B.2.1.2 and Table B-3.
#[test]
fn mips4_softfloat_propagates_legacy_quiet_nan_bits() {
    let backend = Mips4SoftFloatBackend::new();
    let finite_f32 = Float32Bits::new(1.0_f32.to_bits());
    let finite_f64 = Float64Bits::new(1.0_f64.to_bits());
    let positive_f32 = Float32Bits::new(0x7f81_2345);
    let negative_f32 = Float32Bits::new(0xffa5_4321);
    let positive_f64 = Float64Bits::new(0x7ff0_0123_4567_89ab);
    let negative_f64 = Float64Bits::new(0xfff4_3210_9876_5432);

    for (lhs, rhs, expected) in [
        (positive_f32, finite_f32, positive_f32),
        (finite_f32, negative_f32, negative_f32),
        (positive_f32, negative_f32, positive_f32),
        (negative_f32, positive_f32, negative_f32),
    ] {
        let result = backend.add_f32(CONTROL, lhs, rhs);
        assert_eq!(result.value, expected);
        assert!(result.flags.is_empty());
    }

    for (lhs, rhs, expected) in [
        (positive_f64, finite_f64, positive_f64),
        (finite_f64, negative_f64, negative_f64),
        (positive_f64, negative_f64, positive_f64),
        (negative_f64, positive_f64, negative_f64),
    ] {
        let result = backend.add_f64(CONTROL, lhs, rhs);
        assert_eq!(result.value, expected);
        assert!(result.flags.is_empty());
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Appendix B.2.1.2 and Table B-3.
#[test]
fn mips4_softfloat_signaling_nan_uses_the_default_quiet_nan() {
    let backend = Mips4SoftFloatBackend::new();

    for signaling_nan in [0x7fc1_2345, 0xffe5_4321] {
        let result = backend.add_f32(
            CONTROL,
            Float32Bits::new(signaling_nan),
            Float32Bits::new(1.0_f32.to_bits()),
        );
        assert_eq!(result.value.bits(), DEFAULT_QNAN_F32);
        assert_eq!(result.flags, FloatExceptionFlags::INVALID);
    }

    for signaling_nan in [0x7ff8_0123_4567_89ab, 0xffff_4321_9876_5432] {
        let result = backend.add_f64(
            CONTROL,
            Float64Bits::new(signaling_nan),
            Float64Bits::new(1.0_f64.to_bits()),
        );
        assert_eq!(result.value.bits(), DEFAULT_QNAN_F64);
        assert_eq!(result.flags, FloatExceptionFlags::INVALID);
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Appendix B.2.1.2.
#[test]
fn mips4_softfloat_prefers_an_existing_quiet_nan_to_a_signaling_nan() {
    let backend = Mips4SoftFloatBackend::new();
    let quiet_f32 = Float32Bits::new(0xffa1_2345);
    let signaling_f32 = Float32Bits::new(0x7fc0_0001);
    let quiet_f64 = Float64Bits::new(0xfff4_3210_9876_5432);
    let signaling_f64 = Float64Bits::new(0x7ff8_0000_0000_0001);

    for (lhs, rhs) in [(quiet_f32, signaling_f32), (signaling_f32, quiet_f32)] {
        let result = backend.add_f32(CONTROL, lhs, rhs);
        assert_eq!(result.value, quiet_f32);
        assert_eq!(result.flags, FloatExceptionFlags::INVALID);
    }
    for (lhs, rhs) in [(quiet_f64, signaling_f64), (signaling_f64, quiet_f64)] {
        let result = backend.add_f64(CONTROL, lhs, rhs);
        assert_eq!(result.value, quiet_f64);
        assert_eq!(result.flags, FloatExceptionFlags::INVALID);
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Table B-3.
#[test]
fn mips4_softfloat_invalid_arithmetic_returns_exact_default_nan_bits() {
    let backend = Mips4SoftFloatBackend::new();
    let invalid_f32 = [
        backend.div_f32(CONTROL, Float32Bits::new(0), Float32Bits::new(0)),
        backend.sub_f32(
            CONTROL,
            Float32Bits::new(f32::INFINITY.to_bits()),
            Float32Bits::new(f32::INFINITY.to_bits()),
        ),
        backend.sqrt_f32(CONTROL, Float32Bits::new((-1.0_f32).to_bits())),
    ];
    let invalid_f64 = [
        backend.div_f64(CONTROL, Float64Bits::new(0), Float64Bits::new(0)),
        backend.sub_f64(
            CONTROL,
            Float64Bits::new(f64::INFINITY.to_bits()),
            Float64Bits::new(f64::INFINITY.to_bits()),
        ),
        backend.sqrt_f64(CONTROL, Float64Bits::new((-1.0_f64).to_bits())),
    ];

    for result in invalid_f32 {
        assert_eq!(result.value.bits(), DEFAULT_QNAN_F32);
        assert_eq!(result.flags, FloatExceptionFlags::INVALID);
    }
    for result in invalid_f64 {
        assert_eq!(result.value.bits(), DEFAULT_QNAN_F64);
        assert_eq!(result.flags, FloatExceptionFlags::INVALID);
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Appendix B.2.1.2 and Table B-3.
#[test]
fn mips4_softfloat_nan_format_conversions_preserve_or_default() {
    let backend = Mips4SoftFloatBackend::new();
    let quiet_f32 = 0xff92_3456;
    let quiet_f64 = 0xfff4_3210_0000_0000;

    let widened = backend.f32_to_f64(CONTROL, Float32Bits::new(quiet_f32));
    assert_eq!(
        widened.value.bits(),
        0xfff0_0000_0000_0000 | (u64::from(quiet_f32 & 0x007f_ffff) << 29)
    );
    assert!(widened.flags.is_empty());

    let narrowed = backend.f64_to_f32(CONTROL, Float64Bits::new(quiet_f64));
    assert_eq!(
        narrowed.value.bits(),
        0xff80_0000 | (((quiet_f64 & 0x000f_ffff_ffff_ffff) >> 29) as u32)
    );
    assert!(narrowed.flags.is_empty());

    let signaling_f32 = backend.f32_to_f64(CONTROL, Float32Bits::new(0x7fc0_0001));
    assert_eq!(signaling_f32.value.bits(), DEFAULT_QNAN_F64);
    assert_eq!(signaling_f32.flags, FloatExceptionFlags::INVALID);

    let signaling_f64 = backend.f64_to_f32(CONTROL, Float64Bits::new(0xfff8_0000_0000_0001));
    assert_eq!(signaling_f64.value.bits(), DEFAULT_QNAN_F32);
    assert_eq!(signaling_f64.flags, FloatExceptionFlags::INVALID);
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Table B-3.
#[test]
fn mips4_softfloat_invalid_integer_conversions_ignore_rounding_mode() {
    let backend = Mips4SoftFloatBackend::new();
    let modes = [
        FloatRoundingMode::NearestEven,
        FloatRoundingMode::TowardZero,
        FloatRoundingMode::TowardPositive,
        FloatRoundingMode::TowardNegative,
    ];

    for rounding_mode in modes {
        let control = FloatControl::new(rounding_mode, FloatTininessMode::AfterRounding);
        for (input, expected) in [
            (0x7f80_0001, i32::MAX),
            (0x7fc0_0001, i32::MAX),
            (0x4f00_0000, i32::MAX),
            (0xcf00_0001, i32::MIN),
        ] {
            let result = backend.f32_to_i32(control, Float32Bits::new(input));
            assert_eq!(result.value, expected);
            assert_eq!(result.flags, FloatExceptionFlags::INVALID);
        }
        for (input, expected) in [
            (0x7ff0_0000_0000_0001, i64::MAX),
            (0x7ff8_0000_0000_0001, i64::MAX),
            (0x43e0_0000_0000_0000, i64::MAX),
            (0xc3e0_0000_0000_0001, i64::MIN),
        ] {
            let result = backend.f64_to_i64(control, Float64Bits::new(input));
            assert_eq!(result.value, expected);
            assert_eq!(result.flags, FloatExceptionFlags::INVALID);
        }
    }
}

// Oracle: MIPS IV Instruction Set Rev. 3.2, Appendix B.5.5.
#[test]
fn mips4_softfloat_detects_tininess_after_rounding() {
    let backend = Mips4SoftFloatBackend::new();
    let f32_result = backend.mul_f32(
        CONTROL,
        Float32Bits::new(0x0080_0001),
        Float32Bits::new(0x3f7f_fffe),
    );
    let f64_result = backend.mul_f64(
        CONTROL,
        Float64Bits::new(0x0010_0000_0000_0001),
        Float64Bits::new(0x3fef_ffff_ffff_fffe),
    );

    assert_eq!(f32_result.value.bits(), 0x0080_0000);
    assert_eq!(f32_result.flags, FloatExceptionFlags::INEXACT);
    assert_eq!(f64_result.value.bits(), 0x0010_0000_0000_0000);
    assert_eq!(f64_result.flags, FloatExceptionFlags::INEXACT);
}

#[test]
fn mips4_softfloat_does_not_leak_flags_between_operations() {
    let backend = Mips4SoftFloatBackend::new();

    let divide = backend.div_f32(
        CONTROL,
        Float32Bits::new(1.0_f32.to_bits()),
        Float32Bits::new(0.0_f32.to_bits()),
    );
    let add = backend.add_f32(
        CONTROL,
        Float32Bits::new(1.0_f32.to_bits()),
        Float32Bits::new(2.0_f32.to_bits()),
    );

    assert!(divide.flags.contains(FloatExceptionFlags::DIVIDE_BY_ZERO));
    assert!(add.flags.is_empty());
}

#[test]
fn mips4_softfloat_supports_square_root_and_fused_multiply_add() {
    let backend = Mips4SoftFloatBackend::new();
    let root = backend.sqrt_f64(CONTROL, Float64Bits::new(9.0f64.to_bits()));
    let fused = backend.mul_add_f32(
        CONTROL,
        Float32Bits::new(2.0f32.to_bits()),
        Float32Bits::new(3.0f32.to_bits()),
        Float32Bits::new(4.0f32.to_bits()),
    );

    assert_eq!(root.value.bits(), 3.0f64.to_bits());
    assert!(root.flags.is_empty());
    assert_eq!(fused.value.bits(), 10.0f32.to_bits());
    assert!(fused.flags.is_empty());
}

#[test]
fn mips4_softfloat_round_trips_exact_i64_values() {
    let backend = Mips4SoftFloatBackend::new();
    let converted = backend.i64_to_f64(CONTROL, 1_i64 << 40);
    let restored = backend.f64_to_i64(CONTROL, converted.value);

    assert_eq!(converted.value.bits(), ((1_i64 << 40) as f64).to_bits());
    assert!(converted.flags.is_empty());
    assert_eq!(restored.value, 1_i64 << 40);
    assert!(restored.flags.is_empty());
}
