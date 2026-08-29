use se_float::backend::Backend;
use se_float::format::{Float32, Float64};
use se_float::operation::{ExceptionFlags, RoundingMode};

const BACKEND: Backend = Backend::SoftFloat;

#[test]
fn exact_conversions_preserve_values() {
    let widened = BACKEND.convert_float32_to_float64(f32_value(-1.5));
    assert_eq!(widened.value.to_bits(), (-1.5_f64).to_bits());
    assert_eq!(widened.flags, ExceptionFlags::empty());
    assert!(!widened.tiny);

    let integer = BACKEND.convert_i32_to_float64(i32::MIN);
    assert_eq!(integer.value.to_bits(), f64::from(i32::MIN).to_bits());
    assert_eq!(integer.flags, ExceptionFlags::empty());
    assert!(!integer.tiny);
}

#[test]
fn integer_to_float32_obeys_rounding_mode() {
    for (mode, expected) in [
        (RoundingMode::NearestEven, 0x4b80_0000),
        (RoundingMode::TowardZero, 0x4b80_0000),
        (RoundingMode::TowardPositive, 0x4b80_0001),
        (RoundingMode::TowardNegative, 0x4b80_0000),
    ] {
        let result = BACKEND.convert_i32_to_float32(16_777_217, mode);
        assert_eq!(result.value.to_bits(), expected);
        assert_eq!(result.flags, ExceptionFlags::INEXACT);
        assert!(!result.tiny);
    }
}

#[test]
fn float64_to_float32_obeys_rounding_mode() {
    let halfway = Float64::from_bits(0x3ff0_0000_1000_0000);

    for (mode, expected) in [
        (RoundingMode::NearestEven, 0x3f80_0000),
        (RoundingMode::TowardZero, 0x3f80_0000),
        (RoundingMode::TowardPositive, 0x3f80_0001),
        (RoundingMode::TowardNegative, 0x3f80_0000),
    ] {
        let result = BACKEND.convert_float64_to_float32(halfway, mode);
        assert_eq!(result.value.to_bits(), expected);
        assert_eq!(result.flags, ExceptionFlags::INEXACT);
        assert!(!result.tiny);
    }
}

#[test]
fn float64_to_float32_reports_tininess_after_rounding() {
    let exact_subnormal = Float64::from_bits(f64::from(f32::from_bits(1)).to_bits());
    let exact = BACKEND.convert_float64_to_float32(exact_subnormal, RoundingMode::NearestEven);
    assert_eq!(exact.value.to_bits(), 1);
    assert_eq!(exact.flags, ExceptionFlags::empty());
    assert!(exact.tiny);

    let half_minimum = Float64::from_bits((f64::from(f32::from_bits(1)) / 2.0).to_bits());
    let underflow = BACKEND.convert_float64_to_float32(half_minimum, RoundingMode::NearestEven);
    assert_eq!(underflow.value.to_bits(), 0);
    assert_eq!(
        underflow.flags,
        ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT
    );
    assert!(underflow.tiny);

    let rounds_to_minimum_normal = Float64::from_bits(
        (f64::from(f32::MIN_POSITIVE) - f64::from(f32::from_bits(1)) / 4.0).to_bits(),
    );
    let normal =
        BACKEND.convert_float64_to_float32(rounds_to_minimum_normal, RoundingMode::NearestEven);
    assert_eq!(normal.value.to_bits(), f32::MIN_POSITIVE.to_bits());
    assert_eq!(normal.flags, ExceptionFlags::INEXACT);
    assert!(!normal.tiny);
}

#[test]
fn float_to_integer_obeys_rounding_mode() {
    for (mode, positive, negative) in [
        (RoundingMode::NearestEven, 2, -2),
        (RoundingMode::TowardZero, 1, -1),
        (RoundingMode::TowardPositive, 2, -1),
        (RoundingMode::TowardNegative, 1, -2),
    ] {
        let positive_result = BACKEND.convert_float32_to_i32(f32_value(1.5), mode);
        assert_eq!(positive_result.value, positive);
        assert_eq!(positive_result.flags, ExceptionFlags::INEXACT);

        let negative_result = BACKEND.convert_float64_to_i32(f64_value(-1.5), mode);
        assert_eq!(negative_result.value, negative);
        assert_eq!(negative_result.flags, ExceptionFlags::INEXACT);
    }
}

#[test]
fn invalid_float_to_integer_returns_architectural_indefinite_value() {
    for value in [
        Float32::from_bits(f32::NAN.to_bits()),
        Float32::from_bits(f32::INFINITY.to_bits()),
        Float32::from_bits(f32::NEG_INFINITY.to_bits()),
    ] {
        let result = BACKEND.convert_float32_to_i32(value, RoundingMode::NearestEven);
        assert_eq!(result.value, i32::MAX);
        assert_eq!(result.flags, ExceptionFlags::INVALID);
        assert!(!result.tiny);
    }

    for value in [2_147_483_648.0_f64, -2_147_483_649.0] {
        let result = BACKEND.convert_float64_to_i32(f64_value(value), RoundingMode::TowardZero);
        assert_eq!(result.value, i32::MAX);
        assert_eq!(result.flags, ExceptionFlags::INVALID);
        assert!(!result.tiny);
    }
}

#[test]
fn nan_conversions_align_quiet_payloads_and_canonicalize_signaling_inputs() {
    let f32_quiet_bits = 0xffa1_2345;
    let widened = BACKEND.convert_float32_to_float64(Float32::from_bits(f32_quiet_bits));
    let expected_f64 = 0xfff0_0000_0000_0000 | (u64::from(f32_quiet_bits & 0x003f_ffff) << 29);
    assert_eq!(widened.value.to_bits(), expected_f64);
    assert_eq!(widened.flags, ExceptionFlags::empty());

    let f64_quiet_bits = 0x7ff1_2345_6789_abcd;
    let narrowed = BACKEND.convert_float64_to_float32(
        Float64::from_bits(f64_quiet_bits),
        RoundingMode::NearestEven,
    );
    let expected_f32 = 0x7f80_0000 | ((f64_quiet_bits & 0x0007_ffff_ffff_ffff) >> 29) as u32;
    assert_eq!(narrowed.value.to_bits(), expected_f32);
    assert_eq!(narrowed.flags, ExceptionFlags::empty());

    let collapsed = BACKEND.convert_float64_to_float32(
        Float64::from_bits(0x7ff0_0000_0000_0001),
        RoundingMode::NearestEven,
    );
    assert_eq!(collapsed.value.to_bits(), 0x7fbf_ffff);
    assert_eq!(collapsed.flags, ExceptionFlags::empty());

    let signaling_f32 = BACKEND.convert_float32_to_float64(Float32::from_bits(0x7fc0_0001));
    assert_eq!(signaling_f32.value.to_bits(), 0x7ff7_ffff_ffff_ffff);
    assert_eq!(signaling_f32.flags, ExceptionFlags::INVALID);

    let signaling_f64 = BACKEND.convert_float64_to_float32(
        Float64::from_bits(0xfff8_0000_0000_0001),
        RoundingMode::NearestEven,
    );
    assert_eq!(signaling_f64.value.to_bits(), 0x7fbf_ffff);
    assert_eq!(signaling_f64.flags, ExceptionFlags::INVALID);
}

fn f32_value(value: f32) -> Float32 {
    Float32::from_bits(value.to_bits())
}

fn f64_value(value: f64) -> Float64 {
    Float64::from_bits(value.to_bits())
}
