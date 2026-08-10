use se_float::SoftFloatBackend;
use se_float::env::{ExceptionFlags, RoundingFacts, RoundingMode};

fn overflow_flags() -> ExceptionFlags {
    ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT
}

#[test]
fn formal_binary64_to_binary32_tininess_vectors_are_preserved() {
    let backend = SoftFloatBackend;

    // The precision-rounded value remains below 2^-126, but subnormal
    // quantization carries the final encoding into the minimum normal.
    let rounds_from_tiny = backend.f64_to_f32(0x380f_ffff_e100_0000, RoundingMode::NearestEven);
    assert_eq!(rounds_from_tiny.value, 0x0080_0000);
    assert_eq!(
        rounds_from_tiny.flags,
        ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT
    );
    assert_eq!(
        rounds_from_tiny.rounding,
        RoundingFacts {
            tiny_after_rounding: true,
            precision_inexact: true,
        }
    );

    // This adjacent source value first rounds to 2^-126 at binary32
    // precision, so it is inexact without being tiny.
    let rounds_from_normal = backend.f64_to_f32(0x380f_ffff_fc00_0000, RoundingMode::NearestEven);
    assert_eq!(rounds_from_normal.value, 0x0080_0000);
    assert_eq!(rounds_from_normal.flags, ExceptionFlags::INEXACT);
    assert_eq!(
        rounds_from_normal.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        }
    );

    // 2^-126 - 2^-149 is exactly the largest binary32 subnormal.
    let exact_max_subnormal = backend.f64_to_f32(0x380f_ffff_c000_0000, RoundingMode::NearestEven);
    assert_eq!(exact_max_subnormal.value, 0x007f_ffff);
    assert!(exact_max_subnormal.flags.is_empty());
    assert_eq!(
        exact_max_subnormal.rounding,
        RoundingFacts {
            tiny_after_rounding: true,
            precision_inexact: false,
        }
    );
}

#[test]
fn binary32_overflow_distinguishes_exponent_range_from_precision_loss() {
    let backend = SoftFloatBackend;

    let exact = backend.mul_f32(0x7f00_0000, 0x4000_0000, RoundingMode::NearestEven);
    let inexact = backend.mul_f32(0x7f7f_ffff, 0x3f80_0001, RoundingMode::NearestEven);

    assert_eq!(exact.value, 0x7f80_0000);
    assert_eq!(exact.flags, overflow_flags());
    assert_eq!(
        exact.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: false,
        }
    );
    assert_eq!(inexact.value, 0x7f80_0000);
    assert_eq!(inexact.flags, overflow_flags());
    assert_eq!(
        inexact.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        }
    );
}

#[test]
fn halfway_addition_obeys_all_rounding_modes_for_both_signs() {
    let backend = SoftFloatBackend;
    let modes = [
        RoundingMode::NearestEven,
        RoundingMode::TowardZero,
        RoundingMode::TowardPositive,
        RoundingMode::TowardNegative,
    ];
    let positive_f32 = [0x3f80_0000, 0x3f80_0000, 0x3f80_0001, 0x3f80_0000];
    let negative_f32 = [0xbf80_0000, 0xbf80_0000, 0xbf80_0000, 0xbf80_0001];
    let positive_f64 = [
        0x3ff0_0000_0000_0000,
        0x3ff0_0000_0000_0000,
        0x3ff0_0000_0000_0001,
        0x3ff0_0000_0000_0000,
    ];
    let negative_f64 = [
        0xbff0_0000_0000_0000,
        0xbff0_0000_0000_0000,
        0xbff0_0000_0000_0000,
        0xbff0_0000_0000_0001,
    ];

    // 2^-24 is halfway between 1.0f32 and its upward neighbor; 2^-53 is the
    // corresponding halfway addend for binary64.
    for (index, mode) in modes.into_iter().enumerate() {
        let positive = backend.add_f32(0x3f80_0000, 0x3380_0000, mode);
        let negative = backend.add_f32(0xbf80_0000, 0xb380_0000, mode);
        assert_eq!(positive.value, positive_f32[index]);
        assert_eq!(negative.value, negative_f32[index]);
        assert_eq!(positive.flags, ExceptionFlags::INEXACT);
        assert_eq!(negative.flags, ExceptionFlags::INEXACT);
        assert!(positive.rounding.precision_inexact);
        assert!(negative.rounding.precision_inexact);

        let positive = backend.add_f64(0x3ff0_0000_0000_0000, 0x3ca0_0000_0000_0000, mode);
        let negative = backend.add_f64(0xbff0_0000_0000_0000, 0xbca0_0000_0000_0000, mode);
        assert_eq!(positive.value, positive_f64[index]);
        assert_eq!(negative.value, negative_f64[index]);
        assert_eq!(positive.flags, ExceptionFlags::INEXACT);
        assert_eq!(negative.flags, ExceptionFlags::INEXACT);
        assert!(positive.rounding.precision_inexact);
        assert!(negative.rounding.precision_inexact);
    }
}

#[test]
fn subnormal_boundary_tininess_obeys_all_rounding_modes() {
    let backend = SoftFloatBackend;
    let modes = [
        RoundingMode::NearestEven,
        RoundingMode::TowardZero,
        RoundingMode::TowardPositive,
        RoundingMode::TowardNegative,
    ];
    let positive_f32 = [0x0080_0000, 0x007f_ffff, 0x0080_0000, 0x007f_ffff];
    let negative_f32 = [0x8080_0000, 0x807f_ffff, 0x807f_ffff, 0x8080_0000];
    let positive_f64 = [
        0x0010_0000_0000_0000,
        0x000f_ffff_ffff_ffff,
        0x0010_0000_0000_0000,
        0x000f_ffff_ffff_ffff,
    ];
    let negative_f64 = [
        0x8010_0000_0000_0000,
        0x800f_ffff_ffff_ffff,
        0x800f_ffff_ffff_ffff,
        0x8010_0000_0000_0000,
    ];
    let expected_flags = ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT;
    let expected_facts = RoundingFacts {
        tiny_after_rounding: true,
        precision_inexact: true,
    };

    // The exact products are 2^-126 - 2^-150 and 2^-1022 - 2^-1075.
    // Each is halfway between the maximum subnormal and minimum normal after
    // the destination format loses precision in the subnormal range.
    for (index, mode) in modes.into_iter().enumerate() {
        let positive = backend.mul_f32(0x0080_0000, 0x3f7f_ffff, mode);
        let negative = backend.mul_f32(0x8080_0000, 0x3f7f_ffff, mode);
        assert_eq!(positive.value, positive_f32[index]);
        assert_eq!(negative.value, negative_f32[index]);
        assert_eq!(positive.flags, expected_flags);
        assert_eq!(negative.flags, expected_flags);
        assert_eq!(positive.rounding, expected_facts);
        assert_eq!(negative.rounding, expected_facts);

        let positive = backend.mul_f64(0x0010_0000_0000_0000, 0x3fef_ffff_ffff_ffff, mode);
        let negative = backend.mul_f64(0x8010_0000_0000_0000, 0x3fef_ffff_ffff_ffff, mode);
        assert_eq!(positive.value, positive_f64[index]);
        assert_eq!(negative.value, negative_f64[index]);
        assert_eq!(positive.flags, expected_flags);
        assert_eq!(negative.flags, expected_flags);
        assert_eq!(positive.rounding, expected_facts);
        assert_eq!(negative.rounding, expected_facts);
    }
}

#[test]
fn exact_overflow_results_follow_direction_for_both_formats_and_signs() {
    let backend = SoftFloatBackend;
    let modes = [
        RoundingMode::NearestEven,
        RoundingMode::TowardZero,
        RoundingMode::TowardPositive,
        RoundingMode::TowardNegative,
    ];
    let positive_f32 = [0x7f80_0000, 0x7f7f_ffff, 0x7f80_0000, 0x7f7f_ffff];
    let negative_f32 = [0xff80_0000, 0xff7f_ffff, 0xff7f_ffff, 0xff80_0000];
    let positive_f64 = [
        0x7ff0_0000_0000_0000,
        0x7fef_ffff_ffff_ffff,
        0x7ff0_0000_0000_0000,
        0x7fef_ffff_ffff_ffff,
    ];
    let negative_f64 = [
        0xfff0_0000_0000_0000,
        0xffef_ffff_ffff_ffff,
        0xffef_ffff_ffff_ffff,
        0xfff0_0000_0000_0000,
    ];

    // Multiplying 2^127 or 2^1023 by two exceeds only the destination
    // exponent range and discards no significand information.
    for (index, mode) in modes.into_iter().enumerate() {
        let positive = backend.mul_f32(0x7f00_0000, 0x4000_0000, mode);
        let negative = backend.mul_f32(0xff00_0000, 0x4000_0000, mode);
        assert_eq!(positive.value, positive_f32[index]);
        assert_eq!(negative.value, negative_f32[index]);
        assert_eq!(positive.flags, overflow_flags());
        assert_eq!(negative.flags, overflow_flags());
        assert!(!positive.rounding.precision_inexact);
        assert!(!negative.rounding.precision_inexact);

        let positive = backend.mul_f64(0x7fe0_0000_0000_0000, 0x4000_0000_0000_0000, mode);
        let negative = backend.mul_f64(0xffe0_0000_0000_0000, 0x4000_0000_0000_0000, mode);
        assert_eq!(positive.value, positive_f64[index]);
        assert_eq!(negative.value, negative_f64[index]);
        assert_eq!(positive.flags, overflow_flags());
        assert_eq!(negative.flags, overflow_flags());
        assert!(!positive.rounding.precision_inexact);
        assert!(!negative.rounding.precision_inexact);
    }
}

#[test]
fn inexact_overflow_results_follow_direction_for_both_formats() {
    let backend = SoftFloatBackend;
    let modes = [
        RoundingMode::NearestEven,
        RoundingMode::TowardZero,
        RoundingMode::TowardPositive,
        RoundingMode::TowardNegative,
    ];
    let positive_f32 = [0x7f80_0000, 0x7f7f_ffff, 0x7f80_0000, 0x7f7f_ffff];
    let positive_f64 = [
        0x7ff0_0000_0000_0000,
        0x7fef_ffff_ffff_ffff,
        0x7ff0_0000_0000_0000,
        0x7fef_ffff_ffff_ffff,
    ];
    let expected_facts = RoundingFacts {
        tiny_after_rounding: false,
        precision_inexact: true,
    };

    // Multiplying each maximum finite value by one plus one significand ULP
    // both exceeds the exponent range and discards nonzero precision bits.
    for (index, mode) in modes.into_iter().enumerate() {
        let f32_outcome = backend.mul_f32(0x7f7f_ffff, 0x3f80_0001, mode);
        assert_eq!(f32_outcome.value, positive_f32[index]);
        assert_eq!(f32_outcome.flags, overflow_flags());
        assert_eq!(f32_outcome.rounding, expected_facts);

        let f64_outcome = backend.mul_f64(0x7fef_ffff_ffff_ffff, 0x3ff0_0000_0000_0001, mode);
        assert_eq!(f64_outcome.value, positive_f64[index]);
        assert_eq!(f64_outcome.flags, overflow_flags());
        assert_eq!(f64_outcome.rounding, expected_facts);
    }
}
