use core::fmt::Debug;

use se_float::env::{Relation, RoundingMode};
use se_float::{NativeBackend, SoftFloatBackend};

const DETERMINISTIC_SEED: u64 = 0x5e_f10a_7540_4d34;
const RANDOM_SAMPLES: usize = 512;

const F32_BOUNDARY_CORPUS: [u32; 30] = [
    0x0000_0000,
    0x8000_0000,
    0x0000_0001,
    0x8000_0001,
    0x007f_ffff,
    0x807f_ffff,
    0x0080_0000,
    0x8080_0000,
    0x3f7f_ffff,
    0xbf7f_ffff,
    0x3f80_0000,
    0xbf80_0000,
    0x3f80_0001,
    0xbf80_0001,
    0x337f_ffff,
    0xb37f_ffff,
    0x3380_0000,
    0xb380_0000,
    0x3380_0001,
    0xb380_0001,
    0x7f7f_ffff,
    0xff7f_ffff,
    0x7f80_0000,
    0xff80_0000,
    0x7fc0_0000,
    0x7f80_0001,
    0xffc0_0000,
    0xff80_0001,
    0x7fe1_2345,
    0xff81_2345,
];

const F64_BOUNDARY_CORPUS: [u64; 30] = [
    0x0000_0000_0000_0000,
    0x8000_0000_0000_0000,
    0x0000_0000_0000_0001,
    0x8000_0000_0000_0001,
    0x000f_ffff_ffff_ffff,
    0x800f_ffff_ffff_ffff,
    0x0010_0000_0000_0000,
    0x8010_0000_0000_0000,
    0x3fef_ffff_ffff_ffff,
    0xbfef_ffff_ffff_ffff,
    0x3ff0_0000_0000_0000,
    0xbff0_0000_0000_0000,
    0x3ff0_0000_0000_0001,
    0xbff0_0000_0000_0001,
    0x3c9f_ffff_ffff_ffff,
    0xbc9f_ffff_ffff_ffff,
    0x3ca0_0000_0000_0000,
    0xbca0_0000_0000_0000,
    0x3ca0_0000_0000_0001,
    0xbca0_0000_0000_0001,
    0x7fef_ffff_ffff_ffff,
    0xffef_ffff_ffff_ffff,
    0x7ff0_0000_0000_0000,
    0xfff0_0000_0000_0000,
    0x7ff8_0000_0000_0000,
    0x7ff0_0000_0000_0001,
    0xfff8_0000_0000_0000,
    0xfff0_0000_0000_0001,
    0x7ffa_1234_5678_9abc,
    0xfff0_1234_5678_9abc,
];

const F32_INTEGER_CORPUS: [u32; 20] = [
    0x0000_0000,
    0x8000_0000,
    0x3fa0_0000,
    0xbfa0_0000,
    0x4060_0000,
    0xc060_0000,
    0xcf00_0001,
    0xcf00_0000,
    0x4eff_ffff,
    0x4f00_0000,
    0xdf00_0001,
    0xdf00_0000,
    0x5eff_ffff,
    0x5f00_0000,
    0x7f7f_ffff,
    0xff7f_ffff,
    0x7f80_0000,
    0xff80_0000,
    0x7fc0_0000,
    0x7f80_0001,
];

const F64_INTEGER_CORPUS: [u64; 20] = [
    0x0000_0000_0000_0000,
    0x8000_0000_0000_0000,
    0x3ff4_0000_0000_0000,
    0xbff4_0000_0000_0000,
    0x400c_0000_0000_0000,
    0xc00c_0000_0000_0000,
    0xc1e0_0000_0010_0001,
    0xc1e0_0000_0010_0000,
    0x41df_ffff_ffdf_ffff,
    0x41df_ffff_ffe0_0000,
    0xc3e0_0000_0000_0001,
    0xc3e0_0000_0000_0000,
    0x43df_ffff_ffff_ffff,
    0x43e0_0000_0000_0000,
    0x7fef_ffff_ffff_ffff,
    0xffef_ffff_ffff_ffff,
    0x7ff0_0000_0000_0000,
    0xfff0_0000_0000_0000,
    0x7ff8_0000_0000_0000,
    0x7ff0_0000_0000_0001,
];

const I32_CORPUS: [i32; 12] = [
    i32::MIN,
    i32::MIN + 1,
    -16_777_217,
    -16_777_216,
    -1,
    0,
    1,
    16_777_215,
    16_777_216,
    16_777_217,
    i32::MAX - 1,
    i32::MAX,
];

const I64_CORPUS: [i64; 13] = [
    i64::MIN,
    i64::MIN + 1,
    -(1_i64 << 53) - 1,
    -(1_i64 << 53),
    -(1_i64 << 24) - 1,
    -1,
    0,
    1,
    (1_i64 << 24) + 1,
    1_i64 << 53,
    (1_i64 << 53) + 1,
    i64::MAX - 1,
    i64::MAX,
];

// SplitMix64 is used only to produce a reproducible test sample. It is not a
// cryptographic random-number generator.
struct DeterministicRng {
    state: u64,
}

impl DeterministicRng {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut value = self.state;
        value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        value ^ (value >> 31)
    }

    fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }
}

fn target_name() -> &'static str {
    if cfg!(all(
        target_arch = "x86_64",
        target_os = "linux",
        target_env = "gnu"
    )) {
        "x86_64-unknown-linux-gnu"
    } else if cfg!(all(target_arch = "x86_64", target_os = "windows")) {
        "x86_64-pc-windows-msvc"
    } else if cfg!(all(target_arch = "aarch64", target_os = "macos")) {
        "aarch64-apple-darwin"
    } else {
        "unsupported-target"
    }
}

fn profile_name() -> &'static str {
    if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    }
}

#[track_caller]
fn assert_f32_value_matches<Bits: Debug>(
    operation: &str,
    inputs: &[Bits],
    native: u32,
    softfloat: u32,
) {
    let native_is_nan = f32::from_bits(native).is_nan();
    let softfloat_is_nan = f32::from_bits(softfloat).is_nan();
    let context = format_args!(
        "target={} operation={operation} input_bits={inputs:#x?} SoftFloat_value_bits={softfloat:#010x} Native_value_bits={native:#010x} profile={}",
        target_name(),
        profile_name()
    );

    if native_is_nan || softfloat_is_nan {
        assert!(native_is_nan && softfloat_is_nan, "{context}");
    } else {
        assert_eq!(native, softfloat, "{context}");
    }
}

#[track_caller]
fn assert_f64_value_matches(operation: &str, inputs: &[u64], native: u64, softfloat: u64) {
    let native_is_nan = f64::from_bits(native).is_nan();
    let softfloat_is_nan = f64::from_bits(softfloat).is_nan();
    let context = format_args!(
        "target={} operation={operation} input_bits={inputs:#x?} SoftFloat_value_bits={softfloat:#018x} Native_value_bits={native:#018x} profile={}",
        target_name(),
        profile_name()
    );

    if native_is_nan || softfloat_is_nan {
        assert!(native_is_nan && softfloat_is_nan, "{context}");
    } else {
        assert_eq!(native, softfloat, "{context}");
    }
}

#[track_caller]
fn assert_relation_matches<Bits: Debug>(
    operation: &str,
    inputs: &[Bits],
    native: Relation,
    softfloat: Relation,
) {
    assert_eq!(
        native,
        softfloat,
        "target={} operation={operation} input_bits={inputs:#x?} SoftFloat_relation={softfloat:?} Native_relation={native:?} profile={}",
        target_name(),
        profile_name()
    );
}

#[track_caller]
fn assert_option_matches<Bits: Debug, Integer: Debug + Eq>(
    operation: &str,
    input: Bits,
    native: Option<Integer>,
    softfloat: Option<Integer>,
) {
    assert_eq!(
        native,
        softfloat,
        "target={} operation={operation} input_bits={input:#x?} SoftFloat_value={softfloat:?} Native_value={native:?} profile={}",
        target_name(),
        profile_name()
    );
}

fn compare_f32_binary_case(native: &NativeBackend, softfloat: &SoftFloatBackend, a: u32, b: u32) {
    let rounding = RoundingMode::NearestEven;
    assert_f32_value_matches(
        "add_f32",
        &[a, b],
        native.add_f32(a, b),
        softfloat.add_f32(a, b, rounding).value,
    );
    assert_f32_value_matches(
        "sub_f32",
        &[a, b],
        native.sub_f32(a, b),
        softfloat.sub_f32(a, b, rounding).value,
    );
    assert_f32_value_matches(
        "mul_f32",
        &[a, b],
        native.mul_f32(a, b),
        softfloat.mul_f32(a, b, rounding).value,
    );
    assert_f32_value_matches(
        "div_f32",
        &[a, b],
        native.div_f32(a, b),
        softfloat.div_f32(a, b, rounding).value,
    );
    assert_relation_matches(
        "compare_f32",
        &[a, b],
        native.compare_f32(a, b),
        softfloat.compare_f32(a, b).value,
    );
}

fn compare_f64_binary_case(native: &NativeBackend, softfloat: &SoftFloatBackend, a: u64, b: u64) {
    let rounding = RoundingMode::NearestEven;
    assert_f64_value_matches(
        "add_f64",
        &[a, b],
        native.add_f64(a, b),
        softfloat.add_f64(a, b, rounding).value,
    );
    assert_f64_value_matches(
        "sub_f64",
        &[a, b],
        native.sub_f64(a, b),
        softfloat.sub_f64(a, b, rounding).value,
    );
    assert_f64_value_matches(
        "mul_f64",
        &[a, b],
        native.mul_f64(a, b),
        softfloat.mul_f64(a, b, rounding).value,
    );
    assert_f64_value_matches(
        "div_f64",
        &[a, b],
        native.div_f64(a, b),
        softfloat.div_f64(a, b, rounding).value,
    );
    assert_relation_matches(
        "compare_f64",
        &[a, b],
        native.compare_f64(a, b),
        softfloat.compare_f64(a, b).value,
    );
}

fn compare_f32_unary_case(native: &NativeBackend, softfloat: &SoftFloatBackend, value: u32) {
    let rounding = RoundingMode::NearestEven;
    assert_f32_value_matches(
        "sqrt_f32",
        &[value],
        native.sqrt_f32(value),
        softfloat.sqrt_f32(value, rounding).value,
    );
    assert_f64_value_matches(
        "f32_to_f64",
        &[u64::from(value)],
        native.f32_to_f64(value),
        softfloat.f32_to_f64(value).value,
    );
}

fn compare_f64_unary_case(native: &NativeBackend, softfloat: &SoftFloatBackend, value: u64) {
    let rounding = RoundingMode::NearestEven;
    assert_f64_value_matches(
        "sqrt_f64",
        &[value],
        native.sqrt_f64(value),
        softfloat.sqrt_f64(value, rounding).value,
    );
    assert_f32_value_matches(
        "f64_to_f32",
        &[value],
        native.f64_to_f32(value),
        softfloat.f64_to_f32(value, rounding).value,
    );
}

fn compare_f32_integer_case(native: &NativeBackend, softfloat: &SoftFloatBackend, value: u32) {
    let rounding = RoundingMode::NearestEven;
    assert_option_matches(
        "f32_to_i32",
        value,
        native.f32_to_i32(value),
        softfloat.f32_to_i32(value, rounding).value,
    );
    assert_option_matches(
        "f32_to_i64",
        value,
        native.f32_to_i64(value),
        softfloat.f32_to_i64(value, rounding).value,
    );
}

fn compare_f64_integer_case(native: &NativeBackend, softfloat: &SoftFloatBackend, value: u64) {
    let rounding = RoundingMode::NearestEven;
    assert_option_matches(
        "f64_to_i32",
        value,
        native.f64_to_i32(value),
        softfloat.f64_to_i32(value, rounding).value,
    );
    assert_option_matches(
        "f64_to_i64",
        value,
        native.f64_to_i64(value),
        softfloat.f64_to_i64(value, rounding).value,
    );
}

fn compare_i32_case(native: &NativeBackend, softfloat: &SoftFloatBackend, value: i32) {
    let rounding = RoundingMode::NearestEven;
    assert_f32_value_matches(
        "i32_to_f32",
        &[value as u32],
        native.i32_to_f32(value),
        softfloat.i32_to_f32(value, rounding).value,
    );
    assert_f64_value_matches(
        "i32_to_f64",
        &[value as i64 as u64],
        native.i32_to_f64(value),
        softfloat.i32_to_f64(value).value,
    );
}

fn compare_i64_case(native: &NativeBackend, softfloat: &SoftFloatBackend, value: i64) {
    let rounding = RoundingMode::NearestEven;
    assert_f32_value_matches(
        "i64_to_f32",
        &[value as u64],
        native.i64_to_f32(value),
        softfloat.i64_to_f32(value, rounding).value,
    );
    assert_f64_value_matches(
        "i64_to_f64",
        &[value as u64],
        native.i64_to_f64(value),
        softfloat.i64_to_f64(value, rounding).value,
    );
}

#[test]
fn boundary_corpora_cartesian_products_match_nearest_even_values() {
    let native = NativeBackend;
    let softfloat = SoftFloatBackend;

    for &a in &F32_BOUNDARY_CORPUS {
        for &b in &F32_BOUNDARY_CORPUS {
            compare_f32_binary_case(&native, &softfloat, a, b);
        }
    }
    for &a in &F64_BOUNDARY_CORPUS {
        for &b in &F64_BOUNDARY_CORPUS {
            compare_f64_binary_case(&native, &softfloat, a, b);
        }
    }
}

#[test]
fn boundary_corpora_cover_unary_format_and_integer_conversions() {
    let native = NativeBackend;
    let softfloat = SoftFloatBackend;

    for &value in &F32_BOUNDARY_CORPUS {
        compare_f32_unary_case(&native, &softfloat, value);
    }
    for &value in &F64_BOUNDARY_CORPUS {
        compare_f64_unary_case(&native, &softfloat, value);
    }
    for &value in &F32_INTEGER_CORPUS {
        compare_f32_integer_case(&native, &softfloat, value);
    }
    for &value in &F64_INTEGER_CORPUS {
        compare_f64_integer_case(&native, &softfloat, value);
    }
    for &value in &I32_CORPUS {
        compare_i32_case(&native, &softfloat, value);
    }
    for &value in &I64_CORPUS {
        compare_i64_case(&native, &softfloat, value);
    }
}

#[test]
fn fixed_seed_raw_bit_samples_cover_every_primitive() {
    let native = NativeBackend;
    let softfloat = SoftFloatBackend;
    let mut rng = DeterministicRng::new(DETERMINISTIC_SEED);

    for _ in 0..RANDOM_SAMPLES {
        let a_f32 = rng.next_u32();
        let b_f32 = rng.next_u32();
        let a_f64 = rng.next_u64();
        let b_f64 = rng.next_u64();
        let i32_value = rng.next_u32() as i32;
        let i64_value = rng.next_u64() as i64;

        compare_f32_binary_case(&native, &softfloat, a_f32, b_f32);
        compare_f64_binary_case(&native, &softfloat, a_f64, b_f64);
        compare_f32_unary_case(&native, &softfloat, a_f32);
        compare_f64_unary_case(&native, &softfloat, a_f64);
        compare_f32_integer_case(&native, &softfloat, a_f32);
        compare_f64_integer_case(&native, &softfloat, a_f64);
        compare_i32_case(&native, &softfloat, i32_value);
        compare_i64_case(&native, &softfloat, i64_value);
    }
}

fn assert_f32_round_trip(native: &NativeBackend, softfloat: &SoftFloatBackend, bits: u32) {
    let widened_softfloat = softfloat.f32_to_f64(bits);
    assert!(widened_softfloat.flags.is_empty());
    assert!(!widened_softfloat.rounding.precision_inexact);
    let narrowed_softfloat =
        softfloat.f64_to_f32(widened_softfloat.value, RoundingMode::NearestEven);
    assert_eq!(narrowed_softfloat.value, bits);
    assert!(narrowed_softfloat.flags.is_empty());
    assert!(!narrowed_softfloat.rounding.precision_inexact);

    let widened_native = native.f32_to_f64(bits);
    assert_eq!(native.f64_to_f32(widened_native), bits);
}

#[test]
fn non_nan_f32_widen_narrow_round_trip_is_bit_exact() {
    let native = NativeBackend;
    let softfloat = SoftFloatBackend;

    for &bits in &F32_BOUNDARY_CORPUS {
        if !f32::from_bits(bits).is_nan() {
            assert_f32_round_trip(&native, &softfloat, bits);
        }
    }

    let mut rng = DeterministicRng::new(DETERMINISTIC_SEED);
    let mut checked = 0;
    while checked < RANDOM_SAMPLES {
        let bits = rng.next_u32();
        if !f32::from_bits(bits).is_nan() {
            assert_f32_round_trip(&native, &softfloat, bits);
            checked += 1;
        }
    }
}

#[test]
fn finite_addition_and_multiplication_are_commutative() {
    let native = NativeBackend;
    let softfloat = SoftFloatBackend;
    let rounding = RoundingMode::NearestEven;

    for &a in F32_BOUNDARY_CORPUS
        .iter()
        .filter(|&&bits| f32::from_bits(bits).is_finite())
    {
        for &b in F32_BOUNDARY_CORPUS
            .iter()
            .filter(|&&bits| f32::from_bits(bits).is_finite())
        {
            let softfloat_ab = softfloat.add_f32(a, b, rounding);
            let softfloat_ba = softfloat.add_f32(b, a, rounding);
            assert_eq!(softfloat_ab, softfloat_ba);
            assert_eq!(
                native.add_f32(a, b),
                native.add_f32(b, a),
                "target={} operation=commutative_add_f32 input_bits={:#x?} profile={}",
                target_name(),
                [a, b],
                profile_name()
            );

            let softfloat_ab = softfloat.mul_f32(a, b, rounding);
            let softfloat_ba = softfloat.mul_f32(b, a, rounding);
            assert_eq!(softfloat_ab, softfloat_ba);
            assert_eq!(
                native.mul_f32(a, b),
                native.mul_f32(b, a),
                "target={} operation=commutative_mul_f32 input_bits={:#x?} profile={}",
                target_name(),
                [a, b],
                profile_name()
            );
        }
    }

    for &a in F64_BOUNDARY_CORPUS
        .iter()
        .filter(|&&bits| f64::from_bits(bits).is_finite())
    {
        for &b in F64_BOUNDARY_CORPUS
            .iter()
            .filter(|&&bits| f64::from_bits(bits).is_finite())
        {
            let softfloat_ab = softfloat.add_f64(a, b, rounding);
            let softfloat_ba = softfloat.add_f64(b, a, rounding);
            assert_eq!(softfloat_ab, softfloat_ba);
            assert_eq!(
                native.add_f64(a, b),
                native.add_f64(b, a),
                "target={} operation=commutative_add_f64 input_bits={:#x?} profile={}",
                target_name(),
                [a, b],
                profile_name()
            );

            let softfloat_ab = softfloat.mul_f64(a, b, rounding);
            let softfloat_ba = softfloat.mul_f64(b, a, rounding);
            assert_eq!(softfloat_ab, softfloat_ba);
            assert_eq!(
                native.mul_f64(a, b),
                native.mul_f64(b, a),
                "target={} operation=commutative_mul_f64 input_bits={:#x?} profile={}",
                target_name(),
                [a, b],
                profile_name()
            );
        }
    }
}
