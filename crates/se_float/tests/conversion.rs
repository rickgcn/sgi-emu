use core::fmt::Debug;

use se_float::SoftFloatBackend;
use se_float::env::{ExceptionFlags, Outcome, RoundingFacts, RoundingMode};

fn assert_invalid<T: Debug>(outcome: Outcome<Option<T>>) {
    assert!(outcome.value.is_none());
    assert_eq!(outcome.flags, ExceptionFlags::INVALID);
    assert_eq!(outcome.rounding, RoundingFacts::default());
    assert_eq!(
        outcome.value.is_none(),
        outcome.flags.contains(ExceptionFlags::INVALID)
    );
}

fn assert_inexact<T: Debug + Eq>(outcome: Outcome<Option<T>>, value: T) {
    assert_eq!(outcome.value, Some(value));
    assert_eq!(outcome.flags, ExceptionFlags::INEXACT);
    assert_eq!(
        outcome.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        }
    );
}

#[test]
fn format_conversions_preserve_values_and_standard_nan_contract() {
    let backend = SoftFloatBackend;

    let widened = backend.f32_to_f64(0x3fc0_0000);
    assert_eq!(widened.value, 0x3ff8_0000_0000_0000);
    assert!(widened.flags.is_empty());
    assert_eq!(widened.rounding, RoundingFacts::default());

    let narrowed = backend.f64_to_f32(0x3ff8_0000_0000_0000, RoundingMode::NearestEven);
    assert_eq!(narrowed.value, 0x3fc0_0000);
    assert!(narrowed.flags.is_empty());
    assert_eq!(narrowed.rounding, RoundingFacts::default());

    let quiet_widened = backend.f32_to_f64(0xffc0_0001);
    assert_eq!(quiet_widened.value, 0x7ff8_0000_0000_0000);
    assert!(quiet_widened.flags.is_empty());
    let signaling_widened = backend.f32_to_f64(0x7f80_0001);
    assert_eq!(signaling_widened.value, 0x7ff8_0000_0000_0000);
    assert_eq!(signaling_widened.flags, ExceptionFlags::INVALID);

    let quiet_narrowed = backend.f64_to_f32(0xfff8_0000_0000_0001, RoundingMode::TowardNegative);
    assert_eq!(quiet_narrowed.value, 0x7fc0_0000);
    assert!(quiet_narrowed.flags.is_empty());
    let signaling_narrowed =
        backend.f64_to_f32(0x7ff0_0000_0000_0001, RoundingMode::TowardPositive);
    assert_eq!(signaling_narrowed.value, 0x7fc0_0000);
    assert_eq!(signaling_narrowed.flags, ExceptionFlags::INVALID);
}

#[test]
fn signed_integer_to_float_conversions_report_precision_loss() {
    let backend = SoftFloatBackend;

    let exact_i32_f32 = backend.i32_to_f32(16_777_216, RoundingMode::NearestEven);
    assert_eq!(exact_i32_f32.value, 0x4b80_0000);
    assert!(exact_i32_f32.flags.is_empty());
    assert_eq!(exact_i32_f32.rounding, RoundingFacts::default());

    let inexact_i32_f32 = backend.i32_to_f32(16_777_217, RoundingMode::NearestEven);
    assert_eq!(inexact_i32_f32.value, 0x4b80_0000);
    assert_eq!(inexact_i32_f32.flags, ExceptionFlags::INEXACT);
    assert!(inexact_i32_f32.rounding.precision_inexact);
    let upward_i32_f32 = backend.i32_to_f32(16_777_217, RoundingMode::TowardPositive);
    assert_eq!(upward_i32_f32.value, 0x4b80_0001);

    let exact_i64_f32 = backend.i64_to_f32(i64::MIN, RoundingMode::NearestEven);
    assert_eq!(exact_i64_f32.value, 0xdf00_0000);
    assert!(exact_i64_f32.flags.is_empty());
    let inexact_i64_f32 = backend.i64_to_f32(i64::MAX, RoundingMode::NearestEven);
    assert_eq!(inexact_i64_f32.value, 0x5f00_0000);
    assert_eq!(inexact_i64_f32.flags, ExceptionFlags::INEXACT);
    assert!(inexact_i64_f32.rounding.precision_inexact);

    let exact_i32_f64 = backend.i32_to_f64(i32::MIN);
    assert_eq!(exact_i32_f64.value, 0xc1e0_0000_0000_0000);
    assert!(exact_i32_f64.flags.is_empty());
    assert_eq!(exact_i32_f64.rounding, RoundingFacts::default());

    let exact_i64_f64 = backend.i64_to_f64(1_i64 << 53, RoundingMode::NearestEven);
    assert_eq!(exact_i64_f64.value, 0x4340_0000_0000_0000);
    assert!(exact_i64_f64.flags.is_empty());
    let inexact_i64_f64 = backend.i64_to_f64((1_i64 << 53) + 1, RoundingMode::NearestEven);
    assert_eq!(inexact_i64_f64.value, 0x4340_0000_0000_0000);
    assert_eq!(inexact_i64_f64.flags, ExceptionFlags::INEXACT);
    assert!(inexact_i64_f64.rounding.precision_inexact);
}

#[test]
fn all_float_to_integer_shims_request_exact_reporting() {
    let backend = SoftFloatBackend;

    assert_inexact(
        backend.f32_to_i32(0x3fc0_0000, RoundingMode::NearestEven),
        2,
    );
    assert_inexact(backend.f32_to_i64(0x3fc0_0000, RoundingMode::TowardZero), 1);
    assert_inexact(
        backend.f64_to_i32(0xbff8_0000_0000_0000, RoundingMode::NearestEven),
        -2,
    );
    assert_inexact(
        backend.f64_to_i64(0xbff8_0000_0000_0000, RoundingMode::TowardPositive),
        -1,
    );
}

#[test]
fn float_to_integer_boundaries_return_none_exactly_on_invalid() {
    let backend = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    let min_i32_from_f32 = backend.f32_to_i32(0xcf00_0000, rounding);
    assert_eq!(min_i32_from_f32.value, Some(i32::MIN));
    assert!(min_i32_from_f32.flags.is_empty());
    assert_invalid(backend.f32_to_i32(0x4f00_0000, rounding));

    let min_i64_from_f32 = backend.f32_to_i64(0xdf00_0000, rounding);
    assert_eq!(min_i64_from_f32.value, Some(i64::MIN));
    assert!(min_i64_from_f32.flags.is_empty());
    assert_invalid(backend.f32_to_i64(0x5f00_0000, rounding));

    let min_i32_from_f64 = backend.f64_to_i32(0xc1e0_0000_0000_0000, rounding);
    assert_eq!(min_i32_from_f64.value, Some(i32::MIN));
    assert!(min_i32_from_f64.flags.is_empty());
    assert_invalid(backend.f64_to_i32(0x41e0_0000_0000_0000, rounding));

    let min_i64_from_f64 = backend.f64_to_i64(0xc3e0_0000_0000_0000, rounding);
    assert_eq!(min_i64_from_f64.value, Some(i64::MIN));
    assert!(min_i64_from_f64.flags.is_empty());
    assert_invalid(backend.f64_to_i64(0x43e0_0000_0000_0000, rounding));
}

#[test]
fn every_nan_integer_conversion_is_invalid_without_inexact() {
    let backend = SoftFloatBackend;

    for bits in [0x7fc0_0000, 0x7f80_0001] {
        assert_invalid(backend.f32_to_i32(bits, RoundingMode::NearestEven));
        assert_invalid(backend.f32_to_i64(bits, RoundingMode::TowardNegative));
    }
    for bits in [0x7ff8_0000_0000_0000, 0x7ff0_0000_0000_0001] {
        assert_invalid(backend.f64_to_i32(bits, RoundingMode::TowardZero));
        assert_invalid(backend.f64_to_i64(bits, RoundingMode::TowardPositive));
    }
}
