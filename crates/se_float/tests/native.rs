use core::fmt::Debug;

use se_float::NativeBackend;
use se_float::env::Relation;

#[derive(Clone, Copy)]
struct BoundaryEvidence<Bits, Integer> {
    last_invalid_below_l: Bits,
    first_valid_at_or_above_l: (Bits, Integer),
    last_valid_below_u: (Bits, Integer),
    first_invalid_at_or_above_u: Bits,
}

fn assert_boundary_evidence<Bits: Copy, Integer: Debug + Eq>(
    evidence: BoundaryEvidence<Bits, Integer>,
    convert: impl Fn(Bits) -> Option<Integer>,
) {
    assert_eq!(convert(evidence.last_invalid_below_l), None);
    assert_eq!(
        convert(evidence.first_valid_at_or_above_l.0),
        Some(evidence.first_valid_at_or_above_l.1)
    );
    assert_eq!(
        convert(evidence.last_valid_below_u.0),
        Some(evidence.last_valid_below_u.1)
    );
    assert_eq!(convert(evidence.first_invalid_at_or_above_u), None);
}

#[test]
fn binary32_arithmetic_preserves_native_value_categories() {
    let backend = NativeBackend;

    assert_eq!(backend.add_f32(0x0000_0000, 0x8000_0000), 0x0000_0000);
    assert_eq!(backend.add_f32(0x8000_0000, 0x8000_0000), 0x8000_0000);
    assert_eq!(backend.sub_f32(0x4070_0000, 0x3fc0_0000), 0x4010_0000);
    assert_eq!(backend.mul_f32(0x0000_0001, 0x3f80_0000), 0x0000_0001);
    assert_eq!(backend.mul_f32(0x7f7f_ffff, 0x4000_0000), 0x7f80_0000);
    assert_eq!(backend.div_f32(0x0000_0001, 0x4000_0000), 0x0000_0000);
    assert_eq!(backend.div_f32(0x3f80_0000, 0x0000_0000), 0x7f80_0000);
    assert_eq!(backend.div_f32(0xbf80_0000, 0x0000_0000), 0xff80_0000);
    assert_eq!(backend.sqrt_f32(0x4080_0000), 0x4000_0000);
    assert_eq!(backend.sqrt_f32(0x8000_0000), 0x8000_0000);

    for result in [
        backend.div_f32(0x0000_0000, 0x0000_0000),
        backend.mul_f32(0x0000_0000, 0x7f80_0000),
        backend.sqrt_f32(0xbf80_0000),
        backend.add_f32(0x7fc1_2345, 0x3f80_0000),
    ] {
        assert!(f32::from_bits(result).is_nan());
    }
}

#[test]
fn binary64_arithmetic_preserves_native_value_categories() {
    let backend = NativeBackend;

    assert_eq!(
        backend.add_f64(0x0000_0000_0000_0000, 0x8000_0000_0000_0000),
        0x0000_0000_0000_0000
    );
    assert_eq!(
        backend.add_f64(0x8000_0000_0000_0000, 0x8000_0000_0000_0000),
        0x8000_0000_0000_0000
    );
    assert_eq!(
        backend.sub_f64(0x400e_0000_0000_0000, 0x3ff8_0000_0000_0000),
        0x4002_0000_0000_0000
    );
    assert_eq!(
        backend.mul_f64(0x0000_0000_0000_0001, 0x3ff0_0000_0000_0000),
        0x0000_0000_0000_0001
    );
    assert_eq!(
        backend.mul_f64(0x7fef_ffff_ffff_ffff, 0x4000_0000_0000_0000),
        0x7ff0_0000_0000_0000
    );
    assert_eq!(
        backend.div_f64(0x0000_0000_0000_0001, 0x4000_0000_0000_0000),
        0x0000_0000_0000_0000
    );
    assert_eq!(
        backend.div_f64(0x3ff0_0000_0000_0000, 0x0000_0000_0000_0000),
        0x7ff0_0000_0000_0000
    );
    assert_eq!(
        backend.div_f64(0xbff0_0000_0000_0000, 0x0000_0000_0000_0000),
        0xfff0_0000_0000_0000
    );
    assert_eq!(
        backend.sqrt_f64(0x4010_0000_0000_0000),
        0x4000_0000_0000_0000
    );
    assert_eq!(
        backend.sqrt_f64(0x8000_0000_0000_0000),
        0x8000_0000_0000_0000
    );

    for result in [
        backend.div_f64(0x0000_0000_0000_0000, 0x0000_0000_0000_0000),
        backend.mul_f64(0x0000_0000_0000_0000, 0x7ff0_0000_0000_0000),
        backend.sqrt_f64(0xbff0_0000_0000_0000),
        backend.add_f64(0x7ff8_1234_5678_9abc, 0x3ff0_0000_0000_0000),
    ] {
        assert!(f64::from_bits(result).is_nan());
    }
}

#[test]
fn native_comparisons_cover_all_relations() {
    let backend = NativeBackend;

    for (a, b, expected) in [
        (0xbf80_0000, 0x3f80_0000, Relation::Less),
        (0x0000_0000, 0x8000_0000, Relation::Equal),
        (0x4000_0000, 0x3f80_0000, Relation::Greater),
        (0x7fc0_0000, 0x3f80_0000, Relation::Unordered),
        (0x3f80_0000, 0x7f80_0001, Relation::Unordered),
    ] {
        assert_eq!(backend.compare_f32(a, b), expected);
    }

    for (a, b, expected) in [
        (0xbff0_0000_0000_0000, 0x3ff0_0000_0000_0000, Relation::Less),
        (
            0x0000_0000_0000_0000,
            0x8000_0000_0000_0000,
            Relation::Equal,
        ),
        (
            0x4000_0000_0000_0000,
            0x3ff0_0000_0000_0000,
            Relation::Greater,
        ),
        (
            0x7ff8_0000_0000_0000,
            0x3ff0_0000_0000_0000,
            Relation::Unordered,
        ),
        (
            0x3ff0_0000_0000_0000,
            0x7ff0_0000_0000_0001,
            Relation::Unordered,
        ),
    ] {
        assert_eq!(backend.compare_f64(a, b), expected);
    }
}

#[test]
fn native_format_conversions_return_rust_value_bits() {
    let backend = NativeBackend;

    assert_eq!(backend.f32_to_f64(0x3fc0_0000), 0x3ff8_0000_0000_0000);
    assert_eq!(backend.f32_to_f64(0x8000_0000), 0x8000_0000_0000_0000);
    assert_eq!(backend.f32_to_f64(0x0000_0001), 0x36a0_0000_0000_0000);
    assert_eq!(backend.f64_to_f32(0x3ff8_0000_0000_0000), 0x3fc0_0000);
    assert_eq!(backend.f64_to_f32(0x8000_0000_0000_0000), 0x8000_0000);
    assert_eq!(backend.f64_to_f32(0x0000_0000_0000_0001), 0x0000_0000);

    assert!(f64::from_bits(backend.f32_to_f64(0x7fc1_2345)).is_nan());
    assert!(f32::from_bits(backend.f64_to_f32(0x7ff8_1234_5678_9abc)).is_nan());
}

#[test]
fn signed_integer_conversions_cover_precision_and_extremes() {
    let backend = NativeBackend;

    assert_eq!(backend.i32_to_f32(i32::MIN), 0xcf00_0000);
    assert_eq!(backend.i32_to_f32(i32::MAX), 0x4f00_0000);
    assert_eq!(backend.i64_to_f32(i64::MIN), 0xdf00_0000);
    assert_eq!(backend.i64_to_f32(i64::MAX), 0x5f00_0000);
    assert_eq!(backend.i32_to_f64(i32::MIN), 0xc1e0_0000_0000_0000);
    assert_eq!(backend.i32_to_f64(i32::MAX), 0x41df_ffff_ffc0_0000);
    assert_eq!(backend.i64_to_f64(i64::MIN), 0xc3e0_0000_0000_0000);
    assert_eq!(backend.i64_to_f64(i64::MAX), 0x43e0_0000_0000_0000);
}

#[test]
fn float_to_integer_uses_round_ties_even_and_rejects_non_values() {
    let backend = NativeBackend;

    assert_eq!(backend.f32_to_i32(0x4020_0000), Some(2));
    assert_eq!(backend.f32_to_i64(0x4020_0000), Some(2));
    assert_eq!(backend.f32_to_i32(0x4060_0000), Some(4));
    assert_eq!(backend.f32_to_i64(0x4060_0000), Some(4));
    assert_eq!(backend.f32_to_i32(0xc020_0000), Some(-2));
    assert_eq!(backend.f32_to_i64(0xc020_0000), Some(-2));
    assert_eq!(backend.f32_to_i32(0xc060_0000), Some(-4));
    assert_eq!(backend.f32_to_i64(0xc060_0000), Some(-4));
    assert_eq!(backend.f32_to_i32(0x8000_0000), Some(0));
    assert_eq!(backend.f32_to_i64(0x8000_0000), Some(0));
    assert_eq!(backend.f64_to_i32(2.5_f64.to_bits()), Some(2));
    assert_eq!(backend.f64_to_i64(2.5_f64.to_bits()), Some(2));
    assert_eq!(backend.f64_to_i32(3.5_f64.to_bits()), Some(4));
    assert_eq!(backend.f64_to_i64(3.5_f64.to_bits()), Some(4));
    assert_eq!(backend.f64_to_i32((-2.5_f64).to_bits()), Some(-2));
    assert_eq!(backend.f64_to_i64((-2.5_f64).to_bits()), Some(-2));
    assert_eq!(backend.f64_to_i32((-3.5_f64).to_bits()), Some(-4));
    assert_eq!(backend.f64_to_i64((-3.5_f64).to_bits()), Some(-4));
    assert_eq!(backend.f64_to_i32((-0.0_f64).to_bits()), Some(0));
    assert_eq!(backend.f64_to_i64((-0.0_f64).to_bits()), Some(0));

    for bits in [0x7fc0_0000, 0x7f80_0001, 0x7f80_0000, 0xff80_0000] {
        assert_eq!(backend.f32_to_i32(bits), None);
        assert_eq!(backend.f32_to_i64(bits), None);
    }
    for bits in [
        0x7ff8_0000_0000_0000,
        0x7ff0_0000_0000_0001,
        0x7ff0_0000_0000_0000,
        0xfff0_0000_0000_0000,
    ] {
        assert_eq!(backend.f64_to_i32(bits), None);
        assert_eq!(backend.f64_to_i64(bits), None);
    }
}

#[test]
fn float_to_integer_boundaries_use_source_format_evidence() {
    let backend = NativeBackend;

    // Around 2^31, binary32 spacing is 2^7 below the positive limit and
    // 2^8 beyond it; the half-integer mathematical bounds are not encodable.
    assert_boundary_evidence(
        BoundaryEvidence {
            last_invalid_below_l: 0xcf00_0001,
            first_valid_at_or_above_l: (0xcf00_0000, i32::MIN),
            last_valid_below_u: (0x4eff_ffff, 2_147_483_520),
            first_invalid_at_or_above_u: 0x4f00_0000,
        },
        |bits| backend.f32_to_i32(bits),
    );

    // Around 2^63, binary32 spacing is 2^39 below the positive limit and
    // 2^40 beyond it, so the mathematical half-integer bounds are bracketed.
    assert_boundary_evidence(
        BoundaryEvidence {
            last_invalid_below_l: 0xdf00_0001,
            first_valid_at_or_above_l: (0xdf00_0000, i64::MIN),
            last_valid_below_u: (0x5eff_ffff, 9_223_371_487_098_961_920_i64),
            first_invalid_at_or_above_u: 0x5f00_0000,
        },
        |bits| backend.f32_to_i64(bits),
    );

    // Binary64 represents both i32 half-integer bounds exactly, so the four
    // encodings distinguish the inclusive lower tie from the exclusive upper tie.
    assert_boundary_evidence(
        BoundaryEvidence {
            last_invalid_below_l: 0xc1e0_0000_0010_0001,
            first_valid_at_or_above_l: (0xc1e0_0000_0010_0000, i32::MIN),
            last_valid_below_u: (0x41df_ffff_ffdf_ffff, i32::MAX),
            first_invalid_at_or_above_u: 0x41df_ffff_ffe0_0000,
        },
        |bits| backend.f64_to_i32(bits),
    );

    // Around 2^63, binary64 spacing is 2^10 below the positive limit and
    // 2^11 beyond it; adjacent encodings bracket each half-integer bound.
    assert_boundary_evidence(
        BoundaryEvidence {
            last_invalid_below_l: 0xc3e0_0000_0000_0001,
            first_valid_at_or_above_l: (0xc3e0_0000_0000_0000, i64::MIN),
            last_valid_below_u: (0x43df_ffff_ffff_ffff, 9_223_372_036_854_774_784_i64),
            first_invalid_at_or_above_u: 0x43e0_0000_0000_0000,
        },
        |bits| backend.f64_to_i64(bits),
    );
}
