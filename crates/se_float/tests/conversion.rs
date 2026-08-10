use core::fmt::Debug;

use se_float::env::{ExceptionFlags, Outcome, RoundingFacts, RoundingMode};
use se_float::{NativeBackend, SoftFloatBackend};

#[derive(Clone, Copy)]
struct ValidBoundary<Bits, Integer> {
    bits: Bits,
    value: Integer,
    flags: ExceptionFlags,
}

#[derive(Clone, Copy)]
struct RnBoundaryEvidence<Bits, Integer> {
    last_invalid_below_l: Bits,
    first_valid_at_or_above_l: ValidBoundary<Bits, Integer>,
    last_valid_below_u: ValidBoundary<Bits, Integer>,
    first_invalid_at_or_above_u: Bits,
}

#[derive(Clone, Copy)]
struct DirectedBoundaryEvidence<Bits, Integer> {
    last_invalid_on_negative_side: Bits,
    first_valid_on_negative_side: ValidBoundary<Bits, Integer>,
    last_valid_on_positive_side: ValidBoundary<Bits, Integer>,
    first_invalid_on_positive_side: Bits,
}

fn assert_float_outcome<Bits: Debug + Eq>(
    outcome: Outcome<Bits>,
    value: Bits,
    flags: ExceptionFlags,
    rounding: RoundingFacts,
) {
    assert_eq!(outcome.value, value);
    assert_eq!(outcome.flags, flags);
    assert_eq!(outcome.rounding, rounding);
}

fn assert_integer_outcome<Integer: Debug + Eq>(
    outcome: Outcome<Option<Integer>>,
    value: Option<Integer>,
    flags: ExceptionFlags,
) {
    assert_eq!(outcome.value, value);
    assert_eq!(outcome.flags, flags);
    assert_eq!(
        outcome.rounding,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact: flags.contains(ExceptionFlags::INEXACT),
        }
    );
}

fn assert_invalid<Integer: Debug + Eq>(outcome: Outcome<Option<Integer>>) {
    assert_integer_outcome(outcome, None, ExceptionFlags::INVALID);
}

fn assert_soft_boundary_evidence<Bits: Copy, Integer: Copy + Debug + Eq>(
    evidence: DirectedBoundaryEvidence<Bits, Integer>,
    convert: impl Fn(Bits) -> Outcome<Option<Integer>>,
) {
    assert_invalid(convert(evidence.last_invalid_on_negative_side));
    assert_integer_outcome(
        convert(evidence.first_valid_on_negative_side.bits),
        Some(evidence.first_valid_on_negative_side.value),
        evidence.first_valid_on_negative_side.flags,
    );
    assert_integer_outcome(
        convert(evidence.last_valid_on_positive_side.bits),
        Some(evidence.last_valid_on_positive_side.value),
        evidence.last_valid_on_positive_side.flags,
    );
    assert_invalid(convert(evidence.first_invalid_on_positive_side));
}

fn assert_rn_boundary_evidence<Bits: Copy, Integer: Copy + Debug + Eq>(
    evidence: RnBoundaryEvidence<Bits, Integer>,
    softfloat: impl Fn(Bits) -> Outcome<Option<Integer>>,
    native: impl Fn(Bits) -> Option<Integer>,
) {
    assert_invalid(softfloat(evidence.last_invalid_below_l));
    assert_eq!(native(evidence.last_invalid_below_l), None);

    assert_integer_outcome(
        softfloat(evidence.first_valid_at_or_above_l.bits),
        Some(evidence.first_valid_at_or_above_l.value),
        evidence.first_valid_at_or_above_l.flags,
    );
    assert_eq!(
        native(evidence.first_valid_at_or_above_l.bits),
        Some(evidence.first_valid_at_or_above_l.value)
    );

    assert_integer_outcome(
        softfloat(evidence.last_valid_below_u.bits),
        Some(evidence.last_valid_below_u.value),
        evidence.last_valid_below_u.flags,
    );
    assert_eq!(
        native(evidence.last_valid_below_u.bits),
        Some(evidence.last_valid_below_u.value)
    );

    assert_invalid(softfloat(evidence.first_invalid_at_or_above_u));
    assert_eq!(native(evidence.first_invalid_at_or_above_u), None);
}

#[test]
fn format_conversion_boundaries_have_independent_expected_encodings() {
    let backend = SoftFloatBackend;

    for (source, expected) in [
        (0x0000_0000, 0x0000_0000_0000_0000),
        (0x8000_0000, 0x8000_0000_0000_0000),
        (0x0000_0001, 0x36a0_0000_0000_0000),
        (0x007f_ffff, 0x380f_ffff_c000_0000),
        (0x0080_0000, 0x3810_0000_0000_0000),
        (0x7f7f_ffff, 0x47ef_ffff_e000_0000),
        (0x7f80_0000, 0x7ff0_0000_0000_0000),
        (0xff80_0000, 0xfff0_0000_0000_0000),
    ] {
        assert_float_outcome(
            backend.f32_to_f64(source),
            expected,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
    }

    for (source, expected, tiny) in [
        (0x0000_0000_0000_0000, 0x0000_0000, false),
        (0x8000_0000_0000_0000, 0x8000_0000, false),
        (0x36a0_0000_0000_0000, 0x0000_0001, true),
        (0x380f_ffff_c000_0000, 0x007f_ffff, true),
        (0x3810_0000_0000_0000, 0x0080_0000, false),
        (0x47ef_ffff_e000_0000, 0x7f7f_ffff, false),
        (0x7ff0_0000_0000_0000, 0x7f80_0000, false),
        (0xfff0_0000_0000_0000, 0xff80_0000, false),
    ] {
        assert_float_outcome(
            backend.f64_to_f32(source, RoundingMode::NearestEven),
            expected,
            ExceptionFlags::empty(),
            RoundingFacts {
                tiny_after_rounding: tiny,
                precision_inexact: false,
            },
        );
    }

    let quiet_widened = backend.f32_to_f64(0xffc0_0001);
    assert_float_outcome(
        quiet_widened,
        0x7ff8_0000_0000_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_float_outcome(
        backend.f32_to_f64(0x7f80_0001),
        0x7ff8_0000_0000_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
    assert_float_outcome(
        backend.f64_to_f32(0xfff8_0000_0000_0001, RoundingMode::TowardNegative),
        0x7fc0_0000,
        ExceptionFlags::empty(),
        RoundingFacts::default(),
    );
    assert_float_outcome(
        backend.f64_to_f32(0x7ff0_0000_0000_0001, RoundingMode::TowardPositive),
        0x7fc0_0000,
        ExceptionFlags::INVALID,
        RoundingFacts::default(),
    );
}

#[test]
fn exactly_representable_integer_property_has_empty_flags_and_facts() {
    let softfloat = SoftFloatBackend;
    let native = NativeBackend;
    let rounding = RoundingMode::NearestEven;

    for (value, expected) in [
        (i32::MIN, 0xcf00_0000),
        (-16_777_216, 0xcb80_0000),
        (-16_777_215, 0xcb7f_ffff),
        (-1, 0xbf80_0000),
        (0, 0x0000_0000),
        (1, 0x3f80_0000),
        (16_777_215, 0x4b7f_ffff),
        (16_777_216, 0x4b80_0000),
    ] {
        assert_float_outcome(
            softfloat.i32_to_f32(value, rounding),
            expected,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        assert_eq!(native.i32_to_f32(value), expected);
    }

    for (value, expected) in [
        (i64::MIN, 0xdf00_0000),
        (-(1_i64 << 40), 0xd380_0000),
        (-(1_i64 << 24), 0xcb80_0000),
        (-1, 0xbf80_0000),
        (0, 0x0000_0000),
        (1, 0x3f80_0000),
        (1_i64 << 24, 0x4b80_0000),
        (1_i64 << 40, 0x5380_0000),
    ] {
        assert_float_outcome(
            softfloat.i64_to_f32(value, rounding),
            expected,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        assert_eq!(native.i64_to_f32(value), expected);
    }

    for (value, expected) in [
        (i32::MIN, 0xc1e0_0000_0000_0000),
        (-1, 0xbff0_0000_0000_0000),
        (0, 0x0000_0000_0000_0000),
        (1, 0x3ff0_0000_0000_0000),
        (i32::MAX, 0x41df_ffff_ffc0_0000),
    ] {
        assert_float_outcome(
            softfloat.i32_to_f64(value),
            expected,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        assert_eq!(native.i32_to_f64(value), expected);
    }

    for (value, expected) in [
        (i64::MIN, 0xc3e0_0000_0000_0000),
        (-(1_i64 << 53), 0xc340_0000_0000_0000),
        (-1, 0xbff0_0000_0000_0000),
        (0, 0x0000_0000_0000_0000),
        (1, 0x3ff0_0000_0000_0000),
        (1_i64 << 53, 0x4340_0000_0000_0000),
    ] {
        assert_float_outcome(
            softfloat.i64_to_f64(value, rounding),
            expected,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        assert_eq!(native.i64_to_f64(value), expected);
    }
}

#[test]
fn integer_to_float_first_precision_loss_obeys_all_rounding_modes() {
    let softfloat = SoftFloatBackend;
    let native = NativeBackend;
    let modes = [
        RoundingMode::NearestEven,
        RoundingMode::TowardZero,
        RoundingMode::TowardPositive,
        RoundingMode::TowardNegative,
    ];
    let positive_f32 = [0x4b80_0000, 0x4b80_0000, 0x4b80_0001, 0x4b80_0000];
    let negative_f32 = [0xcb80_0000, 0xcb80_0000, 0xcb80_0000, 0xcb80_0001];
    let positive_f64 = [
        0x4340_0000_0000_0000,
        0x4340_0000_0000_0000,
        0x4340_0000_0000_0001,
        0x4340_0000_0000_0000,
    ];
    let negative_f64 = [
        0xc340_0000_0000_0000,
        0xc340_0000_0000_0000,
        0xc340_0000_0000_0000,
        0xc340_0000_0000_0001,
    ];
    let inexact = RoundingFacts {
        tiny_after_rounding: false,
        precision_inexact: true,
    };

    // 2^24 + 1 and -(2^24 + 1) are the first positive and negative
    // integers that binary32 cannot represent exactly.
    for (index, mode) in modes.into_iter().enumerate() {
        assert_float_outcome(
            softfloat.i32_to_f32(16_777_217, mode),
            positive_f32[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
        assert_float_outcome(
            softfloat.i32_to_f32(-16_777_217, mode),
            negative_f32[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
        assert_float_outcome(
            softfloat.i64_to_f32(16_777_217, mode),
            positive_f32[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
        assert_float_outcome(
            softfloat.i64_to_f32(-16_777_217, mode),
            negative_f32[index],
            ExceptionFlags::INEXACT,
            inexact,
        );

        // Binary64 first loses integer precision at 2^53 + 1.
        assert_float_outcome(
            softfloat.i64_to_f64((1_i64 << 53) + 1, mode),
            positive_f64[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
        assert_float_outcome(
            softfloat.i64_to_f64(-((1_i64 << 53) + 1), mode),
            negative_f64[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
    }

    assert_eq!(native.i32_to_f32(16_777_217), positive_f32[0]);
    assert_eq!(native.i32_to_f32(-16_777_217), negative_f32[0]);
    assert_eq!(native.i64_to_f32(16_777_217), positive_f32[0]);
    assert_eq!(native.i64_to_f32(-16_777_217), negative_f32[0]);
    assert_eq!(native.i64_to_f64((1_i64 << 53) + 1), positive_f64[0]);
    assert_eq!(native.i64_to_f64(-((1_i64 << 53) + 1)), negative_f64[0]);
}

#[test]
fn integer_to_float_extremes_follow_direction() {
    let softfloat = SoftFloatBackend;
    let native = NativeBackend;
    let modes = [
        RoundingMode::NearestEven,
        RoundingMode::TowardZero,
        RoundingMode::TowardPositive,
        RoundingMode::TowardNegative,
    ];
    let i32_max_f32 = [0x4f00_0000, 0x4eff_ffff, 0x4f00_0000, 0x4eff_ffff];
    let i64_max_f32 = [0x5f00_0000, 0x5eff_ffff, 0x5f00_0000, 0x5eff_ffff];
    let i64_max_f64 = [
        0x43e0_0000_0000_0000,
        0x43df_ffff_ffff_ffff,
        0x43e0_0000_0000_0000,
        0x43df_ffff_ffff_ffff,
    ];
    let inexact = RoundingFacts {
        tiny_after_rounding: false,
        precision_inexact: true,
    };

    for (index, mode) in modes.into_iter().enumerate() {
        assert_float_outcome(
            softfloat.i32_to_f32(i32::MIN, mode),
            0xcf00_0000,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        assert_float_outcome(
            softfloat.i32_to_f32(i32::MAX, mode),
            i32_max_f32[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
        assert_float_outcome(
            softfloat.i64_to_f32(i64::MIN, mode),
            0xdf00_0000,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        assert_float_outcome(
            softfloat.i64_to_f32(i64::MAX, mode),
            i64_max_f32[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
        assert_float_outcome(
            softfloat.i64_to_f64(i64::MIN, mode),
            0xc3e0_0000_0000_0000,
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        assert_float_outcome(
            softfloat.i64_to_f64(i64::MAX, mode),
            i64_max_f64[index],
            ExceptionFlags::INEXACT,
            inexact,
        );
    }

    assert_eq!(native.i32_to_f32(i32::MIN), 0xcf00_0000);
    assert_eq!(native.i32_to_f32(i32::MAX), i32_max_f32[0]);
    assert_eq!(native.i64_to_f32(i64::MIN), 0xdf00_0000);
    assert_eq!(native.i64_to_f32(i64::MAX), i64_max_f32[0]);
    assert_eq!(native.i64_to_f64(i64::MIN), 0xc3e0_0000_0000_0000);
    assert_eq!(native.i64_to_f64(i64::MAX), i64_max_f64[0]);
}

#[test]
fn round_nearest_float_to_integer_boundaries_follow_half_integer_intervals() {
    let softfloat = SoftFloatBackend;
    let native = NativeBackend;
    let rounding = RoundingMode::NearestEven;

    // Binary32 cannot encode either i32 half-integer bound. Its adjacent
    // values bracket L = -2^31 - 0.5 and U = 2^31 - 0.5.
    assert_rn_boundary_evidence(
        RnBoundaryEvidence {
            last_invalid_below_l: 0xcf00_0001,
            first_valid_at_or_above_l: ValidBoundary {
                bits: 0xcf00_0000,
                value: i32::MIN,
                flags: ExceptionFlags::empty(),
            },
            last_valid_below_u: ValidBoundary {
                bits: 0x4eff_ffff,
                value: 2_147_483_520,
                flags: ExceptionFlags::empty(),
            },
            first_invalid_at_or_above_u: 0x4f00_0000,
        },
        |bits| softfloat.f32_to_i32(bits, rounding),
        |bits| native.f32_to_i32(bits),
    );

    // Binary32 spacing around 2^63 also brackets both mathematical bounds.
    assert_rn_boundary_evidence(
        RnBoundaryEvidence {
            last_invalid_below_l: 0xdf00_0001,
            first_valid_at_or_above_l: ValidBoundary {
                bits: 0xdf00_0000,
                value: i64::MIN,
                flags: ExceptionFlags::empty(),
            },
            last_valid_below_u: ValidBoundary {
                bits: 0x5eff_ffff,
                value: 9_223_371_487_098_961_920_i64,
                flags: ExceptionFlags::empty(),
            },
            first_invalid_at_or_above_u: 0x5f00_0000,
        },
        |bits| softfloat.f32_to_i64(bits, rounding),
        |bits| native.f32_to_i64(bits),
    );

    // Binary64 represents both i32 half-integer boundaries exactly. L is
    // included because its tie rounds to the even lower endpoint; U is
    // excluded because its tie rounds to 2^31.
    assert_rn_boundary_evidence(
        RnBoundaryEvidence {
            last_invalid_below_l: 0xc1e0_0000_0010_0001,
            first_valid_at_or_above_l: ValidBoundary {
                bits: 0xc1e0_0000_0010_0000,
                value: i32::MIN,
                flags: ExceptionFlags::INEXACT,
            },
            last_valid_below_u: ValidBoundary {
                bits: 0x41df_ffff_ffdf_ffff,
                value: i32::MAX,
                flags: ExceptionFlags::INEXACT,
            },
            first_invalid_at_or_above_u: 0x41df_ffff_ffe0_0000,
        },
        |bits| softfloat.f64_to_i32(bits, rounding),
        |bits| native.f64_to_i32(bits),
    );

    // Binary64 cannot encode unit-scale half-integers near 2^63, so adjacent
    // representable values bracket L and U.
    assert_rn_boundary_evidence(
        RnBoundaryEvidence {
            last_invalid_below_l: 0xc3e0_0000_0000_0001,
            first_valid_at_or_above_l: ValidBoundary {
                bits: 0xc3e0_0000_0000_0000,
                value: i64::MIN,
                flags: ExceptionFlags::empty(),
            },
            last_valid_below_u: ValidBoundary {
                bits: 0x43df_ffff_ffff_ffff,
                value: 9_223_372_036_854_774_784_i64,
                flags: ExceptionFlags::empty(),
            },
            first_invalid_at_or_above_u: 0x43e0_0000_0000_0000,
        },
        |bits| softfloat.f64_to_i64(bits, rounding),
        |bits| native.f64_to_i64(bits),
    );
}

#[test]
fn directed_binary32_to_i32_boundaries_are_mode_specific() {
    let backend = SoftFloatBackend;
    let evidence = DirectedBoundaryEvidence {
        last_invalid_on_negative_side: 0xcf00_0001,
        first_valid_on_negative_side: ValidBoundary {
            bits: 0xcf00_0000,
            value: i32::MIN,
            flags: ExceptionFlags::empty(),
        },
        last_valid_on_positive_side: ValidBoundary {
            bits: 0x4eff_ffff,
            value: 2_147_483_520,
            flags: ExceptionFlags::empty(),
        },
        first_invalid_on_positive_side: 0x4f00_0000,
    };

    // RZ accepts (i32::MIN - 1, 2^31).
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f32_to_i32(bits, RoundingMode::TowardZero)
    });
    // RP accepts (i32::MIN - 1, i32::MAX].
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f32_to_i32(bits, RoundingMode::TowardPositive)
    });
    // RM accepts [i32::MIN, 2^31). Binary32 spacing at the bounds brackets
    // all three distinct intervals with the same four encodings.
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f32_to_i32(bits, RoundingMode::TowardNegative)
    });
}

#[test]
fn directed_binary32_to_i64_boundaries_are_mode_specific() {
    let backend = SoftFloatBackend;
    let evidence = DirectedBoundaryEvidence {
        last_invalid_on_negative_side: 0xdf00_0001,
        first_valid_on_negative_side: ValidBoundary {
            bits: 0xdf00_0000,
            value: i64::MIN,
            flags: ExceptionFlags::empty(),
        },
        last_valid_on_positive_side: ValidBoundary {
            bits: 0x5eff_ffff,
            value: 9_223_371_487_098_961_920_i64,
            flags: ExceptionFlags::empty(),
        },
        first_invalid_on_positive_side: 0x5f00_0000,
    };

    // RZ accepts (i64::MIN - 1, 2^63).
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f32_to_i64(bits, RoundingMode::TowardZero)
    });
    // RP accepts (i64::MIN - 1, i64::MAX].
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f32_to_i64(bits, RoundingMode::TowardPositive)
    });
    // RM accepts [i64::MIN, 2^63). Binary32 spacing near 2^63 brackets all
    // three distinct mathematical intervals with the same four encodings.
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f32_to_i64(bits, RoundingMode::TowardNegative)
    });
}

#[test]
fn directed_binary64_to_i32_boundaries_are_mode_specific() {
    let backend = SoftFloatBackend;

    // RZ accepts (i32::MIN - 1, 2^31). Both bounds are exactly representable,
    // so the first valid and last valid values are their inward neighbors.
    assert_soft_boundary_evidence(
        DirectedBoundaryEvidence {
            last_invalid_on_negative_side: 0xc1e0_0000_0020_0000,
            first_valid_on_negative_side: ValidBoundary {
                bits: 0xc1e0_0000_001f_ffff,
                value: i32::MIN,
                flags: ExceptionFlags::INEXACT,
            },
            last_valid_on_positive_side: ValidBoundary {
                bits: 0x41df_ffff_ffff_ffff,
                value: i32::MAX,
                flags: ExceptionFlags::INEXACT,
            },
            first_invalid_on_positive_side: 0x41e0_0000_0000_0000,
        },
        |bits| backend.f64_to_i32(bits, RoundingMode::TowardZero),
    );

    // RP accepts (i32::MIN - 1, i32::MAX]. The upper endpoint is exact and
    // its next binary64 neighbor already rounds outside the integer range.
    assert_soft_boundary_evidence(
        DirectedBoundaryEvidence {
            last_invalid_on_negative_side: 0xc1e0_0000_0020_0000,
            first_valid_on_negative_side: ValidBoundary {
                bits: 0xc1e0_0000_001f_ffff,
                value: i32::MIN,
                flags: ExceptionFlags::INEXACT,
            },
            last_valid_on_positive_side: ValidBoundary {
                bits: 0x41df_ffff_ffc0_0000,
                value: i32::MAX,
                flags: ExceptionFlags::empty(),
            },
            first_invalid_on_positive_side: 0x41df_ffff_ffc0_0001,
        },
        |bits| backend.f64_to_i32(bits, RoundingMode::TowardPositive),
    );

    // RM accepts [i32::MIN, 2^31). The exact lower endpoint is valid and the
    // exact positive power-of-two endpoint is not.
    assert_soft_boundary_evidence(
        DirectedBoundaryEvidence {
            last_invalid_on_negative_side: 0xc1e0_0000_0000_0001,
            first_valid_on_negative_side: ValidBoundary {
                bits: 0xc1e0_0000_0000_0000,
                value: i32::MIN,
                flags: ExceptionFlags::empty(),
            },
            last_valid_on_positive_side: ValidBoundary {
                bits: 0x41df_ffff_ffff_ffff,
                value: i32::MAX,
                flags: ExceptionFlags::INEXACT,
            },
            first_invalid_on_positive_side: 0x41e0_0000_0000_0000,
        },
        |bits| backend.f64_to_i32(bits, RoundingMode::TowardNegative),
    );
}

#[test]
fn directed_binary64_to_i64_boundaries_are_mode_specific() {
    let backend = SoftFloatBackend;
    let evidence = DirectedBoundaryEvidence {
        last_invalid_on_negative_side: 0xc3e0_0000_0000_0001,
        first_valid_on_negative_side: ValidBoundary {
            bits: 0xc3e0_0000_0000_0000,
            value: i64::MIN,
            flags: ExceptionFlags::empty(),
        },
        last_valid_on_positive_side: ValidBoundary {
            bits: 0x43df_ffff_ffff_ffff,
            value: 9_223_372_036_854_774_784_i64,
            flags: ExceptionFlags::empty(),
        },
        first_invalid_on_positive_side: 0x43e0_0000_0000_0000,
    };

    // RZ accepts (i64::MIN - 1, 2^63).
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f64_to_i64(bits, RoundingMode::TowardZero)
    });
    // RP accepts (i64::MIN - 1, i64::MAX].
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f64_to_i64(bits, RoundingMode::TowardPositive)
    });
    // RM accepts [i64::MIN, 2^63). Binary64 spacing is 2^10 below and 2^11
    // above 2^63, so the same adjacent encodings prove all three intervals.
    assert_soft_boundary_evidence(evidence, |bits| {
        backend.f64_to_i64(bits, RoundingMode::TowardNegative)
    });
}

#[test]
fn directed_float_to_integer_fractional_halfway_and_nonvalue_cases_are_fixed() {
    let backend = SoftFloatBackend;
    let modes = [
        (RoundingMode::TowardZero, 1, -1, 3, -3),
        (RoundingMode::TowardPositive, 2, -1, 4, -3),
        (RoundingMode::TowardNegative, 1, -2, 3, -4),
    ];

    for (mode, positive_fraction, negative_fraction, positive_half, negative_half) in modes {
        for (bits, expected) in [
            (0x3fa0_0000, positive_fraction),
            (0xbfa0_0000, negative_fraction),
            (0x4060_0000, positive_half),
            (0xc060_0000, negative_half),
        ] {
            assert_integer_outcome(
                backend.f32_to_i32(bits, mode),
                Some(expected),
                ExceptionFlags::INEXACT,
            );
            assert_integer_outcome(
                backend.f32_to_i64(bits, mode),
                Some(i64::from(expected)),
                ExceptionFlags::INEXACT,
            );
        }

        for (bits, expected) in [
            (0x3ff4_0000_0000_0000, positive_fraction),
            (0xbff4_0000_0000_0000, negative_fraction),
            (0x400c_0000_0000_0000, positive_half),
            (0xc00c_0000_0000_0000, negative_half),
        ] {
            assert_integer_outcome(
                backend.f64_to_i32(bits, mode),
                Some(expected),
                ExceptionFlags::INEXACT,
            );
            assert_integer_outcome(
                backend.f64_to_i64(bits, mode),
                Some(i64::from(expected)),
                ExceptionFlags::INEXACT,
            );
        }

        for bits in [0x7fc0_0000, 0x7f80_0001, 0x7f80_0000, 0xff80_0000] {
            assert_invalid(backend.f32_to_i32(bits, mode));
            assert_invalid(backend.f32_to_i64(bits, mode));
        }
        for bits in [
            0x7ff8_0000_0000_0000,
            0x7ff0_0000_0000_0001,
            0x7ff0_0000_0000_0000,
            0xfff0_0000_0000_0000,
        ] {
            assert_invalid(backend.f64_to_i32(bits, mode));
            assert_invalid(backend.f64_to_i64(bits, mode));
        }
    }
}
