use se_float::backend::Backend;
use se_float::format::{Float32, Float64};
use se_float::operation::{ComparisonMode, ExceptionFlags, Relation};

const BACKEND: Backend = Backend::SoftFloat;

#[test]
fn finite_values_produce_ordered_relations() {
    assert_relation_f32(1.0, 2.0, Relation::Less);
    assert_relation_f32(2.0, 2.0, Relation::Equal);
    assert_relation_f32(3.0, 2.0, Relation::Greater);

    assert_relation_f64(-3.0, -2.0, Relation::Less);
    assert_relation_f64(-2.0, -2.0, Relation::Equal);
    assert_relation_f64(-1.0, -2.0, Relation::Greater);
}

#[test]
fn signed_zeros_compare_equal() {
    let f32_result = BACKEND.compare_f32(f32_value(0.0), f32_value(-0.0), ComparisonMode::Quiet);
    assert_eq!(f32_result.value, Relation::Equal);
    assert_eq!(f32_result.flags, ExceptionFlags::empty());

    let f64_result =
        BACKEND.compare_f64(f64_value(-0.0), f64_value(0.0), ComparisonMode::Signaling);
    assert_eq!(f64_result.value, Relation::Equal);
    assert_eq!(f64_result.flags, ExceptionFlags::empty());
}

#[test]
fn quiet_nan_signaling_depends_on_comparison_mode() {
    let quiet_nan_f32 = Float32::from_bits(0x7fa0_0001);
    let quiet = BACKEND.compare_f32(quiet_nan_f32, f32_value(1.0), ComparisonMode::Quiet);
    assert_eq!(quiet.value, Relation::Unordered);
    assert_eq!(quiet.flags, ExceptionFlags::empty());
    assert!(!quiet.tiny);

    let signaling = BACKEND.compare_f32(quiet_nan_f32, f32_value(1.0), ComparisonMode::Signaling);
    assert_eq!(signaling.value, Relation::Unordered);
    assert_eq!(signaling.flags, ExceptionFlags::INVALID);
    assert!(!signaling.tiny);

    let quiet_nan_f64 = Float64::from_bits(0xfff1_0000_0000_0001);
    let f64_signaling =
        BACKEND.compare_f64(f64_value(1.0), quiet_nan_f64, ComparisonMode::Signaling);
    assert_eq!(f64_signaling.value, Relation::Unordered);
    assert_eq!(f64_signaling.flags, ExceptionFlags::INVALID);
}

#[test]
fn signaling_nan_always_reports_invalid() {
    let signaling_nan_f32 = Float32::from_bits(0xffc0_0001);
    for mode in [ComparisonMode::Quiet, ComparisonMode::Signaling] {
        let result = BACKEND.compare_f32(f32_value(1.0), signaling_nan_f32, mode);
        assert_eq!(result.value, Relation::Unordered);
        assert_eq!(result.flags, ExceptionFlags::INVALID);
    }

    let signaling_nan_f64 = Float64::from_bits(0x7ff8_0000_0000_0001);
    for mode in [ComparisonMode::Quiet, ComparisonMode::Signaling] {
        let result = BACKEND.compare_f64(signaling_nan_f64, f64_value(1.0), mode);
        assert_eq!(result.value, Relation::Unordered);
        assert_eq!(result.flags, ExceptionFlags::INVALID);
    }
}

fn assert_relation_f32(lhs: f32, rhs: f32, expected: Relation) {
    let result = BACKEND.compare_f32(f32_value(lhs), f32_value(rhs), ComparisonMode::Quiet);
    assert_eq!(result.value, expected);
    assert_eq!(result.flags, ExceptionFlags::empty());
    assert!(!result.tiny);
}

fn assert_relation_f64(lhs: f64, rhs: f64, expected: Relation) {
    let result = BACKEND.compare_f64(f64_value(lhs), f64_value(rhs), ComparisonMode::Quiet);
    assert_eq!(result.value, expected);
    assert_eq!(result.flags, ExceptionFlags::empty());
    assert!(!result.tiny);
}

fn f32_value(value: f32) -> Float32 {
    Float32::from_bits(value.to_bits())
}

fn f64_value(value: f64) -> Float64 {
    Float64::from_bits(value.to_bits())
}
