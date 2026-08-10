use core::fmt::Debug;

use se_float::SoftFloatBackend;
use se_float::env::{ExceptionFlags, Outcome, Relation, RoundingFacts, RoundingMode};

fn assert_outcome<T: Debug + Eq>(
    outcome: Outcome<T>,
    value: T,
    flags: ExceptionFlags,
    rounding: RoundingFacts,
) {
    assert_eq!(outcome.value, value);
    assert_eq!(outcome.flags, flags);
    assert_eq!(outcome.rounding, rounding);
}

#[test]
fn binary32_finite_arithmetic_matches_independent_vectors() {
    let backend = SoftFloatBackend;
    let exact = RoundingFacts::default();
    let rounding = RoundingMode::NearestEven;

    assert_outcome(
        backend.add_f32(0x3fc0_0000, 0x4010_0000, rounding),
        0x4070_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.sub_f32(0x3f80_0000, 0x4000_0000, rounding),
        0xbf80_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.mul_f32(0xc000_0000, 0x3f00_0000, rounding),
        0xbf80_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.div_f32(0x40c0_0000, 0x4000_0000, rounding),
        0x4040_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.sqrt_f32(0x4080_0000, rounding),
        0x4000_0000,
        ExceptionFlags::empty(),
        exact,
    );
}

#[test]
fn binary64_finite_arithmetic_matches_independent_vectors() {
    let backend = SoftFloatBackend;
    let exact = RoundingFacts::default();
    let rounding = RoundingMode::NearestEven;

    assert_outcome(
        backend.add_f64(0x3ff8_0000_0000_0000, 0x4002_0000_0000_0000, rounding),
        0x400e_0000_0000_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.sub_f64(0x3ff0_0000_0000_0000, 0x4000_0000_0000_0000, rounding),
        0xbff0_0000_0000_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.mul_f64(0xc000_0000_0000_0000, 0x3fe0_0000_0000_0000, rounding),
        0xbff0_0000_0000_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.div_f64(0x4018_0000_0000_0000, 0x4000_0000_0000_0000, rounding),
        0x4008_0000_0000_0000,
        ExceptionFlags::empty(),
        exact,
    );
    assert_outcome(
        backend.sqrt_f64(0x4010_0000_0000_0000, rounding),
        0x4000_0000_0000_0000,
        ExceptionFlags::empty(),
        exact,
    );
}

#[test]
fn identity_operations_preserve_ieee_boundary_encodings() {
    let backend = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    for (value, tiny) in [
        (0x0000_0000, false),
        (0x8000_0000, false),
        (0x0000_0001, true),
        (0x007f_ffff, true),
        (0x0080_0000, false),
        (0x7f7f_ffff, false),
        (0x7f80_0000, false),
        (0xff80_0000, false),
    ] {
        assert_outcome(
            backend.mul_f32(value, 0x3f80_0000, rounding),
            value,
            ExceptionFlags::empty(),
            RoundingFacts {
                tiny_after_rounding: tiny,
                precision_inexact: false,
            },
        );
    }

    for (value, tiny) in [
        (0x0000_0000_0000_0000, false),
        (0x8000_0000_0000_0000, false),
        (0x0000_0000_0000_0001, true),
        (0x000f_ffff_ffff_ffff, true),
        (0x0010_0000_0000_0000, false),
        (0x7fef_ffff_ffff_ffff, false),
        (0x7ff0_0000_0000_0000, false),
        (0xfff0_0000_0000_0000, false),
    ] {
        assert_outcome(
            backend.mul_f64(value, 0x3ff0_0000_0000_0000, rounding),
            value,
            ExceptionFlags::empty(),
            RoundingFacts {
                tiny_after_rounding: tiny,
                precision_inexact: false,
            },
        );
    }
}

#[test]
fn exceptional_arithmetic_reports_non_trapping_flags() {
    let backend = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    assert_outcome(
        backend.div_f32(0x3f80_0000, 0, rounding),
        0x7f80_0000,
        ExceptionFlags::DIVIDE_BY_ZERO,
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.div_f64(0x3ff0_0000_0000_0000, 0, rounding),
        0x7ff0_0000_0000_0000,
        ExceptionFlags::DIVIDE_BY_ZERO,
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.div_f32(0, 0, rounding),
        0x7fc0_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.sqrt_f64(0xbff0_0000_0000_0000, rounding),
        0x7ff8_0000_0000_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );

    let inexact_f32 = backend.div_f32(0x3f80_0000, 0x4040_0000, rounding);
    assert_eq!(inexact_f32.value, 0x3eaa_aaab);
    assert_eq!(inexact_f32.flags, ExceptionFlags::INEXACT);
    assert_eq!(
        inexact_f32.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        }
    );

    let inexact_f64 = backend.div_f64(0x3ff0_0000_0000_0000, 0x4008_0000_0000_0000, rounding);
    assert_eq!(inexact_f64.value, 0x3fd5_5555_5555_5555);
    assert_eq!(inexact_f64.flags, ExceptionFlags::INEXACT);
    assert!(inexact_f64.rounding.precision_inexact);
}

#[test]
fn every_non_trapping_flag_class_and_hidden_rounding_fact_is_fixed() {
    let backend = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    assert_outcome(
        backend.add_f32(0x3f80_0000, 0x3f80_0000, rounding),
        0x4000_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.div_f32(0x3f80_0000, 0x4040_0000, rounding),
        0x3eaa_aaab,
        ExceptionFlags::INEXACT,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        },
    );

    // This value is precision-tiny before subnormal quantization, which then
    // carries into the minimum normal encoding.
    assert_outcome(
        backend.f64_to_f32(0x380f_ffff_e100_0000, rounding),
        0x0080_0000,
        ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT,
        RoundingFacts {
            tiny_after_rounding: true,
            precision_inexact: true,
        },
    );
    // The exact maximum subnormal is tiny without losing precision.
    assert_outcome(
        backend.f64_to_f32(0x380f_ffff_c000_0000, rounding),
        0x007f_ffff,
        ExceptionFlags::empty(),
        RoundingFacts {
            tiny_after_rounding: true,
            precision_inexact: false,
        },
    );

    // Both products exceed the exponent range. Only the second product also
    // discards nonzero significand information before overflow replacement.
    assert_outcome(
        backend.mul_f32(0x7f00_0000, 0x4000_0000, rounding),
        0x7f80_0000,
        ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: false,
        },
    );
    assert_outcome(
        backend.mul_f32(0x7f7f_ffff, 0x3f80_0001, rounding),
        0x7f80_0000,
        ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        },
    );
    assert_outcome(
        backend.div_f64(0x3ff0_0000_0000_0000, 0, rounding),
        0x7ff0_0000_0000_0000,
        ExceptionFlags::DIVIDE_BY_ZERO,
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.sqrt_f32(0xbf80_0000, rounding),
        0x7fc0_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
}

#[test]
fn conversion_boundaries_use_independent_golden_encodings() {
    let backend = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    assert_outcome(
        backend.i32_to_f32(i32::MIN, rounding),
        0xcf00_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.i64_to_f64(i64::MAX, rounding),
        0x43e0_0000_0000_0000,
        ExceptionFlags::INEXACT,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        },
    );

    // Binary64 represents the lower i32 RN boundary exactly. The tie rounds
    // to the even integer i32::MIN and remains a valid inexact conversion.
    assert_outcome(
        backend.f64_to_i32(0xc1e0_0000_0010_0000, rounding),
        Some(i32::MIN),
        ExceptionFlags::INEXACT,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: true,
        },
    );
    assert_outcome(
        backend.f64_to_i32(0x41df_ffff_ffe0_0000, rounding),
        None,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );

    assert_outcome(
        backend.f32_to_f64(0x007f_ffff),
        0x380f_ffff_c000_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.f64_to_f32(0x47ef_ffff_e000_0000, rounding),
        0x7f7f_ffff,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
}

#[test]
fn standard_nan_polarity_and_canonical_results_are_fixed() {
    let backend = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    assert_outcome(
        backend.add_f32(0xffc1_2345, 0x3f80_0000, rounding),
        0x7fc0_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.add_f32(0x7f80_0001, 0x3f80_0000, rounding),
        0x7fc0_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.mul_f64(0xfff8_0000_0000_1234, 0x3ff0_0000_0000_0000, rounding),
        0x7ff8_0000_0000_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.mul_f64(0x7ff0_0000_0000_0001, 0x3ff0_0000_0000_0000, rounding),
        0x7ff8_0000_0000_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );

    assert_outcome(
        backend.add_f32(0x3f80_0000, 0x7fe1_2345, rounding),
        0x7fc0_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.add_f32(0xff80_0123, 0x7fc0_0000, rounding),
        0x7fc0_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.add_f64(0x3ff0_0000_0000_0000, 0x7ffa_1234_5678_9abc, rounding),
        0x7ff8_0000_0000_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_outcome(
        backend.add_f64(0xfff0_1234_5678_9abc, 0x7ff8_0000_0000_0000, rounding),
        0x7ff8_0000_0000_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
}

#[test]
fn quiet_comparisons_cover_all_relations_and_nan_flags() {
    let backend = SoftFloatBackend;

    for (a, b, relation) in [
        (0x3f80_0000, 0x4000_0000, Relation::Less),
        (0x0000_0000, 0x8000_0000, Relation::Equal),
        (0x4040_0000, 0x4000_0000, Relation::Greater),
        (0x7fc0_0000, 0x3f80_0000, Relation::Unordered),
    ] {
        assert_outcome(
            backend.compare_f32(a, b),
            relation,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
    }
    assert_outcome(
        backend.compare_f32(0x7f80_0001, 0x3f80_0000),
        Relation::Unordered,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );

    for (a, b, relation) in [
        (0x3ff0_0000_0000_0000, 0x4000_0000_0000_0000, Relation::Less),
        (
            0x0000_0000_0000_0000,
            0x8000_0000_0000_0000,
            Relation::Equal,
        ),
        (
            0x4008_0000_0000_0000,
            0x4000_0000_0000_0000,
            Relation::Greater,
        ),
        (
            0x7ff8_0000_0000_0000,
            0x3ff0_0000_0000_0000,
            Relation::Unordered,
        ),
    ] {
        assert_outcome(
            backend.compare_f64(a, b),
            relation,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
    }
    assert_outcome(
        backend.compare_f64(0x7ff0_0000_0000_0001, 0x3ff0_0000_0000_0000),
        Relation::Unordered,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
}
