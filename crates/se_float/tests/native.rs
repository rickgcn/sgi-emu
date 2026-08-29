use se_float::backend::Backend;
use se_float::format::{Float32, Float64};
use se_float::operation::{ComparisonMode, ExceptionFlags, Relation, RoundingMode};

const BACKEND: Backend = Backend::Native;

#[test]
fn arithmetic_uses_host_results_without_exception_flags() {
    let add = BACKEND.add_f32(
        f32_value(1.25),
        f32_value(2.5),
        RoundingMode::TowardNegative,
    );
    assert_eq!(add.value.to_bits(), (1.25_f32 + 2.5_f32).to_bits());
    assert_eq!(add.flags, ExceptionFlags::empty());
    assert!(!add.tiny);

    let divide_by_zero = BACKEND.div_f64(f64_value(1.0), f64_value(0.0), RoundingMode::NearestEven);
    assert_eq!(divide_by_zero.value.to_bits(), f64::INFINITY.to_bits());
    assert_eq!(divide_by_zero.flags, ExceptionFlags::empty());

    let invalid = BACKEND.div_f32(f32_value(0.0), f32_value(0.0), RoundingMode::NearestEven);
    assert!(f32::from_bits(invalid.value.to_bits()).is_nan());
    assert_eq!(invalid.flags, ExceptionFlags::empty());
}

#[test]
fn requested_rounding_mode_is_ignored() {
    let lhs = f32_value(1.0);
    let rhs = Float32::from_bits(0x3380_0000);
    let host_bits = (1.0_f32 + f32::from_bits(0x3380_0000)).to_bits();

    for mode in [
        RoundingMode::NearestEven,
        RoundingMode::TowardZero,
        RoundingMode::TowardPositive,
        RoundingMode::TowardNegative,
    ] {
        let result = BACKEND.add_f32(lhs, rhs, mode);
        assert_eq!(result.value.to_bits(), host_bits);
        assert_eq!(result.flags, ExceptionFlags::empty());
    }
}

#[test]
fn native_float_to_integer_matches_rust_casts() {
    for value in [1.9_f32, -1.9, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let result = BACKEND.convert_float32_to_i32(f32_value(value), RoundingMode::TowardPositive);
        assert_eq!(result.value, value as i32);
        assert_eq!(result.flags, ExceptionFlags::empty());
        assert!(!result.tiny);
    }
}

#[test]
fn comparison_mode_is_ignored() {
    for mode in [ComparisonMode::Quiet, ComparisonMode::Signaling] {
        let result = BACKEND.compare_f64(f64_value(f64::NAN), f64_value(1.0), mode);
        assert_eq!(result.value, Relation::Unordered);
        assert_eq!(result.flags, ExceptionFlags::empty());
        assert!(!result.tiny);
    }
}

#[test]
fn tininess_follows_the_host_result_classification() {
    let exact_subnormal = BACKEND.mul_f32(
        Float32::from_bits(f32::MIN_POSITIVE.to_bits()),
        f32_value(0.5),
        RoundingMode::NearestEven,
    );
    assert_eq!(
        exact_subnormal.tiny,
        f32::from_bits(exact_subnormal.value.to_bits()).is_subnormal()
    );

    let narrowed = BACKEND.convert_float64_to_float32(
        Float64::from_bits(f64::from(f32::from_bits(1)).to_bits()),
        RoundingMode::TowardZero,
    );
    assert_eq!(narrowed.value.to_bits(), 1);
    assert!(narrowed.tiny);

    assert!(!BACKEND.abs_f32(Float32::from_bits(1)).tiny);
    assert!(!BACKEND.neg_f64(Float64::from_bits(1)).tiny);
}

fn f32_value(value: f32) -> Float32 {
    Float32::from_bits(value.to_bits())
}

fn f64_value(value: f64) -> Float64 {
    Float64::from_bits(value.to_bits())
}
