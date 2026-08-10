use se_float::env::{Relation, RoundingMode};
use se_float::{NativeBackend, SoftFloatBackend};

fn assert_f32_value_matches(native: u32, accurate: u32) {
    let native_is_nan = f32::from_bits(native).is_nan();
    let accurate_is_nan = f32::from_bits(accurate).is_nan();
    if native_is_nan || accurate_is_nan {
        assert!(native_is_nan && accurate_is_nan);
    } else {
        assert_eq!(native, accurate);
    }
}

fn assert_f64_value_matches(native: u64, accurate: u64) {
    let native_is_nan = f64::from_bits(native).is_nan();
    let accurate_is_nan = f64::from_bits(accurate).is_nan();
    if native_is_nan || accurate_is_nan {
        assert!(native_is_nan && accurate_is_nan);
    } else {
        assert_eq!(native, accurate);
    }
}

#[test]
fn binary32_arithmetic_matches_softfloat_nearest_even_values() {
    let native = NativeBackend;
    let accurate = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    for (a, b) in [
        (0x0000_0000, 0x8000_0000),
        (0x3fc0_0000, 0x4010_0000),
        (0x0000_0001, 0x0000_0001),
        (0x7f7f_ffff, 0x7f7f_ffff),
        (0x7f80_0000, 0xff80_0000),
        (0x7fc1_2345, 0x3f80_0000),
    ] {
        assert_f32_value_matches(native.add_f32(a, b), accurate.add_f32(a, b, rounding).value);
    }

    for (a, b) in [
        (0x0000_0000, 0x8000_0000),
        (0x3f80_0000, 0x4000_0000),
        (0x0080_0000, 0x0000_0001),
        (0x7f80_0000, 0x7f80_0000),
        (0x7f80_0001, 0x3f80_0000),
    ] {
        assert_f32_value_matches(native.sub_f32(a, b), accurate.sub_f32(a, b, rounding).value);
    }

    for (a, b) in [
        (0x8000_0000, 0x4000_0000),
        (0xc000_0000, 0x3f00_0000),
        (0x0000_0001, 0x3f80_0000),
        (0x0000_0001, 0x3f00_0000),
        (0x7f7f_ffff, 0x4000_0000),
        (0x0000_0000, 0x7f80_0000),
        (0xffc0_0001, 0x3f80_0000),
    ] {
        assert_f32_value_matches(native.mul_f32(a, b), accurate.mul_f32(a, b, rounding).value);
    }

    for (a, b) in [
        (0x8000_0000, 0x4000_0000),
        (0x40c0_0000, 0x4000_0000),
        (0x0080_0000, 0x4000_0000),
        (0x3f80_0000, 0x0000_0000),
        (0x0000_0000, 0x0000_0000),
        (0x7fc0_0000, 0x3f80_0000),
    ] {
        assert_f32_value_matches(native.div_f32(a, b), accurate.div_f32(a, b, rounding).value);
    }

    for value in [
        0x0000_0000,
        0x8000_0000,
        0x0000_0001,
        0x4080_0000,
        0x7f80_0000,
        0xbf80_0000,
        0x7f80_0001,
    ] {
        assert_f32_value_matches(
            native.sqrt_f32(value),
            accurate.sqrt_f32(value, rounding).value,
        );
    }
}

#[test]
fn binary64_arithmetic_matches_softfloat_nearest_even_values() {
    let native = NativeBackend;
    let accurate = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    for (a, b) in [
        (0x0000_0000_0000_0000, 0x8000_0000_0000_0000),
        (0x3ff8_0000_0000_0000, 0x4002_0000_0000_0000),
        (0x0000_0000_0000_0001, 0x0000_0000_0000_0001),
        (0x7fef_ffff_ffff_ffff, 0x7fef_ffff_ffff_ffff),
        (0x7ff0_0000_0000_0000, 0xfff0_0000_0000_0000),
        (0x7ff8_1234_5678_9abc, 0x3ff0_0000_0000_0000),
    ] {
        assert_f64_value_matches(native.add_f64(a, b), accurate.add_f64(a, b, rounding).value);
    }

    for (a, b) in [
        (0x0000_0000_0000_0000, 0x8000_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x4000_0000_0000_0000),
        (0x0010_0000_0000_0000, 0x0000_0000_0000_0001),
        (0x7ff0_0000_0000_0000, 0x7ff0_0000_0000_0000),
        (0x7ff0_0000_0000_0001, 0x3ff0_0000_0000_0000),
    ] {
        assert_f64_value_matches(native.sub_f64(a, b), accurate.sub_f64(a, b, rounding).value);
    }

    for (a, b) in [
        (0x8000_0000_0000_0000, 0x4000_0000_0000_0000),
        (0xc000_0000_0000_0000, 0x3fe0_0000_0000_0000),
        (0x0000_0000_0000_0001, 0x3ff0_0000_0000_0000),
        (0x0000_0000_0000_0001, 0x3fe0_0000_0000_0000),
        (0x7fef_ffff_ffff_ffff, 0x4000_0000_0000_0000),
        (0x0000_0000_0000_0000, 0x7ff0_0000_0000_0000),
        (0xfff8_0000_0000_0001, 0x3ff0_0000_0000_0000),
    ] {
        assert_f64_value_matches(native.mul_f64(a, b), accurate.mul_f64(a, b, rounding).value);
    }

    for (a, b) in [
        (0x8000_0000_0000_0000, 0x4000_0000_0000_0000),
        (0x4018_0000_0000_0000, 0x4000_0000_0000_0000),
        (0x0010_0000_0000_0000, 0x4000_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x0000_0000_0000_0000),
        (0x0000_0000_0000_0000, 0x0000_0000_0000_0000),
        (0x7ff8_0000_0000_0000, 0x3ff0_0000_0000_0000),
    ] {
        assert_f64_value_matches(native.div_f64(a, b), accurate.div_f64(a, b, rounding).value);
    }

    for value in [
        0x0000_0000_0000_0000,
        0x8000_0000_0000_0000,
        0x0000_0000_0000_0001,
        0x4010_0000_0000_0000,
        0x7ff0_0000_0000_0000,
        0xbff0_0000_0000_0000,
        0x7ff0_0000_0000_0001,
    ] {
        assert_f64_value_matches(
            native.sqrt_f64(value),
            accurate.sqrt_f64(value, rounding).value,
        );
    }
}

#[test]
fn comparisons_match_softfloat_relations() {
    let native = NativeBackend;
    let accurate = SoftFloatBackend;

    for (a, b) in [
        (0xbf80_0000, 0x3f80_0000),
        (0x0000_0000, 0x8000_0000),
        (0x4000_0000, 0x3f80_0000),
        (0x7fc0_0000, 0x3f80_0000),
        (0x3f80_0000, 0x7f80_0001),
    ] {
        assert_eq!(native.compare_f32(a, b), accurate.compare_f32(a, b).value);
    }

    for (a, b) in [
        (0xbff0_0000_0000_0000, 0x3ff0_0000_0000_0000),
        (0x0000_0000_0000_0000, 0x8000_0000_0000_0000),
        (0x4000_0000_0000_0000, 0x3ff0_0000_0000_0000),
        (0x7ff8_0000_0000_0000, 0x3ff0_0000_0000_0000),
        (0x3ff0_0000_0000_0000, 0x7ff0_0000_0000_0001),
    ] {
        assert_eq!(native.compare_f64(a, b), accurate.compare_f64(a, b).value);
    }

    assert_eq!(native.compare_f32(0x3f80_0000, 0x4000_0000), Relation::Less);
}

#[test]
fn format_and_integer_to_float_conversions_match_softfloat_values() {
    let native = NativeBackend;
    let accurate = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    for value in [
        0x0000_0000,
        0x8000_0000,
        0x0000_0001,
        0x3fc0_0000,
        0x7f80_0000,
        0x7fc1_2345,
        0x7f80_0001,
    ] {
        assert_f64_value_matches(native.f32_to_f64(value), accurate.f32_to_f64(value).value);
    }

    for value in [
        0x0000_0000_0000_0000,
        0x8000_0000_0000_0000,
        0x0000_0000_0000_0001,
        0x3800_0000_0000_0000,
        0x3ff8_0000_0000_0000,
        0x7fef_ffff_ffff_ffff,
        0x7ff0_0000_0000_0000,
        0x7ff8_1234_5678_9abc,
        0x7ff0_0000_0000_0001,
    ] {
        assert_f32_value_matches(
            native.f64_to_f32(value),
            accurate.f64_to_f32(value, rounding).value,
        );
    }

    for value in [i32::MIN, -16_777_217, -1, 0, 16_777_217, i32::MAX] {
        assert_f32_value_matches(
            native.i32_to_f32(value),
            accurate.i32_to_f32(value, rounding).value,
        );
        assert_f64_value_matches(native.i32_to_f64(value), accurate.i32_to_f64(value).value);
    }

    for value in [
        i64::MIN,
        -(1_i64 << 53) - 1,
        -1,
        0,
        (1_i64 << 53) + 1,
        i64::MAX,
    ] {
        assert_f32_value_matches(
            native.i64_to_f32(value),
            accurate.i64_to_f32(value, rounding).value,
        );
        assert_f64_value_matches(
            native.i64_to_f64(value),
            accurate.i64_to_f64(value, rounding).value,
        );
    }
}

#[test]
fn float_to_integer_options_match_softfloat_nearest_even_values() {
    let native = NativeBackend;
    let accurate = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    for value in [
        0x0000_0000,
        0x8000_0000,
        0x4020_0000,
        0xc020_0000,
        0xcf00_0001,
        0xcf00_0000,
        0x4eff_ffff,
        0x4f00_0000,
        0x7f80_0000,
        0x7fc0_0000,
    ] {
        assert_eq!(
            native.f32_to_i32(value),
            accurate.f32_to_i32(value, rounding).value
        );
    }

    for value in [
        0x0000_0000,
        0x8000_0000,
        0x5eff_ffff,
        0x5f00_0000,
        0xdf00_0000,
        0xdf00_0001,
        0xff80_0000,
        0x7f80_0001,
    ] {
        assert_eq!(
            native.f32_to_i64(value),
            accurate.f32_to_i64(value, rounding).value
        );
    }

    for value in [
        0x0000_0000_0000_0000,
        0x8000_0000_0000_0000,
        0x4004_0000_0000_0000,
        0xc004_0000_0000_0000,
        0xc1e0_0000_0010_0001,
        0xc1e0_0000_0010_0000,
        0x41df_ffff_ffdf_ffff,
        0x41df_ffff_ffe0_0000,
        0x7ff0_0000_0000_0000,
        0x7ff8_0000_0000_0000,
    ] {
        assert_eq!(
            native.f64_to_i32(value),
            accurate.f64_to_i32(value, rounding).value
        );
    }

    for value in [
        0x0000_0000_0000_0000,
        0x8000_0000_0000_0000,
        0x43df_ffff_ffff_ffff,
        0x43e0_0000_0000_0000,
        0xc3e0_0000_0000_0000,
        0xc3e0_0000_0000_0001,
        0xfff0_0000_0000_0000,
        0x7ff0_0000_0000_0001,
    ] {
        assert_eq!(
            native.f64_to_i64(value),
            accurate.f64_to_i64(value, rounding).value
        );
    }
}
