use super::*;
use crate::value::{FloatClass, FloatNanMode};

const CONTROL: FloatControl = FloatControl::new(
    FloatRoundingMode::NearestEven,
    FloatTininessMode::AfterRounding,
);

#[test]
fn softfloat3_adds_exact_f32_values() {
    let backend = SoftFloat3Backend::new();
    let result = backend.add_f32(
        CONTROL,
        Float32Bits::new(1.5_f32.to_bits()),
        Float32Bits::new(2.25_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), 3.75_f32.to_bits());
    assert!(result.flags.is_empty());
}

#[test]
fn softfloat3_adds_exact_f64_values() {
    let backend = SoftFloat3Backend::new();
    let result = backend.add_f64(
        CONTROL,
        Float64Bits::new(1.5_f64.to_bits()),
        Float64Bits::new(2.25_f64.to_bits()),
    );

    assert_eq!(result.value.bits(), 3.75_f64.to_bits());
    assert!(result.flags.is_empty());
}

#[test]
fn softfloat3_reports_divide_by_zero() {
    let backend = SoftFloat3Backend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(1.0_f32.to_bits()),
        Float32Bits::new(0.0_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), f32::INFINITY.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::DIVIDE_BY_ZERO));
}

#[test]
fn softfloat3_reports_invalid_zero_divided_by_zero() {
    let backend = SoftFloat3Backend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(0.0_f32.to_bits()),
        Float32Bits::new(0.0_f32.to_bits()),
    );

    assert_eq!(
        result.value.classify(FloatNanMode::QuietBitSet),
        FloatClass::QuietNan
    );
    assert!(result.flags.contains(FloatExceptionFlags::INVALID));
}

#[test]
fn softfloat3_reports_inexact_division() {
    let backend = SoftFloat3Backend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(1.0_f32.to_bits()),
        Float32Bits::new(3.0_f32.to_bits()),
    );

    assert!(result.flags.contains(FloatExceptionFlags::INEXACT));
}

#[test]
fn softfloat3_reports_overflow() {
    let backend = SoftFloat3Backend::new();
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
fn softfloat3_reports_underflow() {
    let backend = SoftFloat3Backend::new();
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
fn softfloat3_conversions_honor_explicit_rounding() {
    let backend = SoftFloat3Backend::new();
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
fn softfloat3_integer_to_float_conversion_reports_inexact() {
    let backend = SoftFloat3Backend::new();
    let result = backend.i32_to_f32(CONTROL, 16_777_217);

    assert_eq!(result.value.bits(), 16_777_216_f32.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::INEXACT));
}

#[test]
fn softfloat3_compare_reports_ordered_relations() {
    let backend = SoftFloat3Backend::new();

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
fn softfloat3_compare_reports_unordered_without_quiet_invalid() {
    let backend = SoftFloat3Backend::new();
    let result = backend.compare_f32(
        CONTROL,
        FloatCompareMode::Quiet,
        Float32Bits::new(0x7fc0_0000),
        Float32Bits::new(1.0_f32.to_bits()),
    );

    assert_eq!(result.value, FloatRelation::Unordered);
    assert!(!result.flags.contains(FloatExceptionFlags::INVALID));
}

#[test]
fn softfloat3_signaling_compare_reports_invalid_for_unordered() {
    let backend = SoftFloat3Backend::new();
    let result = backend.compare_f32(
        CONTROL,
        FloatCompareMode::Signaling,
        Float32Bits::new(0x7fc0_0000),
        Float32Bits::new(1.0_f32.to_bits()),
    );

    assert_eq!(result.value, FloatRelation::Unordered);
    assert!(result.flags.contains(FloatExceptionFlags::INVALID));
}

#[test]
fn softfloat3_does_not_leak_flags_between_operations() {
    let backend = SoftFloat3Backend::new();

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
fn softfloat3_supports_square_root_and_fused_multiply_add() {
    let backend = SoftFloat3Backend::new();
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
fn softfloat3_round_trips_exact_i64_values() {
    let backend = SoftFloat3Backend::new();
    let converted = backend.i64_to_f64(CONTROL, 1_i64 << 40);
    let restored = backend.f64_to_i64(CONTROL, converted.value);

    assert_eq!(converted.value.bits(), ((1_i64 << 40) as f64).to_bits());
    assert!(converted.flags.is_empty());
    assert_eq!(restored.value, 1_i64 << 40);
    assert!(restored.flags.is_empty());
}
