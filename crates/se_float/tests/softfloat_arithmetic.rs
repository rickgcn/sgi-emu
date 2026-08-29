use se_float::backend::Backend;
use se_float::format::{Float32, Float64};
use se_float::operation::{ExceptionFlags, Outcome, RoundingMode};

const BACKEND: Backend = Backend::SoftFloat;

#[test]
fn finite_arithmetic_produces_exact_results() {
    assert_f32(
        BACKEND.add_f32(f32_value(1.5), f32_value(2.25), RoundingMode::NearestEven),
        3.75,
    );
    assert_f32(
        BACKEND.sub_f32(f32_value(5.5), f32_value(2.0), RoundingMode::NearestEven),
        3.5,
    );
    assert_f32(
        BACKEND.mul_f32(f32_value(1.5), f32_value(-2.0), RoundingMode::NearestEven),
        -3.0,
    );
    assert_f32(
        BACKEND.div_f32(f32_value(7.5), f32_value(2.5), RoundingMode::NearestEven),
        3.0,
    );

    assert_f64(
        BACKEND.add_f64(f64_value(1.5), f64_value(2.25), RoundingMode::NearestEven),
        3.75,
    );
    assert_f64(
        BACKEND.sub_f64(f64_value(5.5), f64_value(2.0), RoundingMode::NearestEven),
        3.5,
    );
    assert_f64(
        BACKEND.mul_f64(f64_value(1.5), f64_value(-2.0), RoundingMode::NearestEven),
        -3.0,
    );
    assert_f64(
        BACKEND.div_f64(f64_value(7.5), f64_value(2.5), RoundingMode::NearestEven),
        3.0,
    );
}

#[test]
fn arithmetic_preserves_signed_zero_results() {
    let negative_zero_f32 =
        BACKEND.add_f32(f32_value(-0.0), f32_value(-0.0), RoundingMode::NearestEven);
    assert_eq!(negative_zero_f32.value.to_bits(), (-0.0_f32).to_bits());
    assert_eq!(negative_zero_f32.flags, ExceptionFlags::empty());
    assert!(!negative_zero_f32.tiny);

    let negative_zero_f64 =
        BACKEND.mul_f64(f64_value(0.0), f64_value(-1.0), RoundingMode::NearestEven);
    assert_eq!(negative_zero_f64.value.to_bits(), (-0.0_f64).to_bits());
    assert_eq!(negative_zero_f64.flags, ExceptionFlags::empty());
    assert!(!negative_zero_f64.tiny);
}

#[test]
fn directed_rounding_changes_halfway_additions() {
    let half_ulp_f32 = Float32::from_bits(0x3380_0000);
    let one_f32 = f32_value(1.0);
    let negative_half_ulp_f32 = Float32::from_bits(0xb380_0000);
    let negative_one_f32 = f32_value(-1.0);

    for (mode, expected) in [
        (RoundingMode::NearestEven, 0x3f80_0000),
        (RoundingMode::TowardZero, 0x3f80_0000),
        (RoundingMode::TowardPositive, 0x3f80_0001),
        (RoundingMode::TowardNegative, 0x3f80_0000),
    ] {
        let result = BACKEND.add_f32(one_f32, half_ulp_f32, mode);
        assert_eq!(result.value.to_bits(), expected);
        assert_eq!(result.flags, ExceptionFlags::INEXACT);
    }

    for (mode, expected) in [
        (RoundingMode::NearestEven, 0xbf80_0000),
        (RoundingMode::TowardZero, 0xbf80_0000),
        (RoundingMode::TowardPositive, 0xbf80_0000),
        (RoundingMode::TowardNegative, 0xbf80_0001),
    ] {
        let result = BACKEND.add_f32(negative_one_f32, negative_half_ulp_f32, mode);
        assert_eq!(result.value.to_bits(), expected);
        assert_eq!(result.flags, ExceptionFlags::INEXACT);
    }

    let half_ulp_f64 = Float64::from_bits(0x3ca0_0000_0000_0000);
    let nearest = BACKEND.add_f64(f64_value(1.0), half_ulp_f64, RoundingMode::NearestEven);
    let positive = BACKEND.add_f64(f64_value(1.0), half_ulp_f64, RoundingMode::TowardPositive);
    assert_eq!(nearest.value.to_bits(), 0x3ff0_0000_0000_0000);
    assert_eq!(positive.value.to_bits(), 0x3ff0_0000_0000_0001);
    assert_eq!(nearest.flags, ExceptionFlags::INEXACT);
    assert_eq!(positive.flags, ExceptionFlags::INEXACT);
}

#[test]
fn arithmetic_reports_each_exception_class() {
    let divide_by_zero = BACKEND.div_f32(f32_value(1.0), f32_value(0.0), RoundingMode::NearestEven);
    assert_eq!(divide_by_zero.value.to_bits(), f32::INFINITY.to_bits());
    assert_eq!(divide_by_zero.flags, ExceptionFlags::DIVIDE_BY_ZERO);

    let invalid = BACKEND.div_f64(f64_value(0.0), f64_value(0.0), RoundingMode::NearestEven);
    assert_eq!(invalid.value.to_bits(), 0x7ff7_ffff_ffff_ffff);
    assert_eq!(invalid.flags, ExceptionFlags::INVALID);

    let overflow = BACKEND.mul_f32(
        Float32::from_bits(f32::MAX.to_bits()),
        f32_value(2.0),
        RoundingMode::NearestEven,
    );
    assert_eq!(overflow.value.to_bits(), f32::INFINITY.to_bits());
    assert_eq!(
        overflow.flags,
        ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT
    );

    let inexact = BACKEND.div_f64(f64_value(1.0), f64_value(3.0), RoundingMode::NearestEven);
    assert_eq!(inexact.flags, ExceptionFlags::INEXACT);
}

#[test]
fn tininess_distinguishes_exact_and_inexact_subnormal_results() {
    let exact = BACKEND.mul_f32(
        Float32::from_bits(f32::MIN_POSITIVE.to_bits()),
        f32_value(0.5),
        RoundingMode::NearestEven,
    );
    assert_eq!(exact.value.to_bits(), 0x0040_0000);
    assert_eq!(exact.flags, ExceptionFlags::empty());
    assert!(exact.tiny);

    let inexact = BACKEND.div_f32(
        Float32::from_bits(f32::MIN_POSITIVE.to_bits()),
        f32_value(3.0),
        RoundingMode::NearestEven,
    );
    assert!(inexact.flags.contains(ExceptionFlags::UNDERFLOW));
    assert!(inexact.flags.contains(ExceptionFlags::INEXACT));
    assert!(inexact.tiny);

    let rounded_to_zero = BACKEND.div_f32(
        Float32::from_bits(1),
        f32_value(2.0),
        RoundingMode::NearestEven,
    );
    assert_eq!(rounded_to_zero.value.to_bits(), 0);
    assert_eq!(
        rounded_to_zero.flags,
        ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT
    );
    assert!(rounded_to_zero.tiny);

    let exact_f64 = BACKEND.mul_f64(
        Float64::from_bits(f64::MIN_POSITIVE.to_bits()),
        f64_value(0.5),
        RoundingMode::NearestEven,
    );
    assert_eq!(exact_f64.value.to_bits(), 0x0008_0000_0000_0000);
    assert_eq!(exact_f64.flags, ExceptionFlags::empty());
    assert!(exact_f64.tiny);
}

#[test]
fn legacy_nan_propagation_prefers_quiet_lhs_and_canonicalizes_signaling_inputs() {
    let lhs_qnan_f32 = Float32::from_bits(0xffa1_2345);
    let rhs_qnan_f32 = Float32::from_bits(0x7f81_2345);
    let signaling_f32 = Float32::from_bits(0x7fc1_2345);

    let lhs = BACKEND.add_f32(lhs_qnan_f32, rhs_qnan_f32, RoundingMode::NearestEven);
    assert_eq!(lhs.value, lhs_qnan_f32);
    assert_eq!(lhs.flags, ExceptionFlags::empty());

    let rhs = BACKEND.add_f32(f32_value(1.0), rhs_qnan_f32, RoundingMode::NearestEven);
    assert_eq!(rhs.value, rhs_qnan_f32);
    assert_eq!(rhs.flags, ExceptionFlags::empty());

    let signaling = BACKEND.mul_f32(signaling_f32, rhs_qnan_f32, RoundingMode::NearestEven);
    assert_eq!(signaling.value.to_bits(), 0x7fbf_ffff);
    assert_eq!(signaling.flags, ExceptionFlags::INVALID);

    let invalid_non_nan = BACKEND.mul_f64(
        f64_value(0.0),
        Float64::from_bits(f64::INFINITY.to_bits()),
        RoundingMode::NearestEven,
    );
    assert_eq!(invalid_non_nan.value.to_bits(), 0x7ff7_ffff_ffff_ffff);
    assert_eq!(invalid_non_nan.flags, ExceptionFlags::INVALID);
}

#[test]
fn sign_operations_preserve_quiet_payloads_and_reject_signaling_nans() {
    assert_eq!(BACKEND.abs_f32(f32_value(-0.0)).value.to_bits(), 0);
    assert_eq!(
        BACKEND.neg_f64(f64_value(0.0)).value.to_bits(),
        0x8000_0000_0000_0000
    );

    let negative_qnan = Float32::from_bits(0xffa1_2345);
    let positive_qnan = Float64::from_bits(0x7ff1_2345_6789_abcd);
    assert_eq!(BACKEND.abs_f32(negative_qnan).value.to_bits(), 0x7fa1_2345);
    assert_eq!(
        BACKEND.neg_f64(positive_qnan).value.to_bits(),
        0xfff1_2345_6789_abcd
    );

    let abs_signaling = BACKEND.abs_f32(Float32::from_bits(0xffc1_2345));
    assert_eq!(abs_signaling.value.to_bits(), 0x7fbf_ffff);
    assert_eq!(abs_signaling.flags, ExceptionFlags::INVALID);
    assert!(!abs_signaling.tiny);

    let neg_signaling = BACKEND.neg_f64(Float64::from_bits(0xfff8_0000_0000_0001));
    assert_eq!(neg_signaling.value.to_bits(), 0x7ff7_ffff_ffff_ffff);
    assert_eq!(neg_signaling.flags, ExceptionFlags::INVALID);
    assert!(!neg_signaling.tiny);
}

fn f32_value(value: f32) -> Float32 {
    Float32::from_bits(value.to_bits())
}

fn f64_value(value: f64) -> Float64 {
    Float64::from_bits(value.to_bits())
}

fn assert_f32(outcome: Outcome<Float32>, expected: f32) {
    assert_eq!(outcome.value.to_bits(), expected.to_bits());
    assert_eq!(outcome.flags, ExceptionFlags::empty());
    assert!(!outcome.tiny);
}

fn assert_f64(outcome: Outcome<Float64>, expected: f64) {
    assert_eq!(outcome.value.to_bits(), expected.to_bits());
    assert_eq!(outcome.flags, ExceptionFlags::empty());
    assert!(!outcome.tiny);
}
