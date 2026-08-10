use se_float::env::{Outcome, Relation, RoundingMode};
use se_float::{NativeBackend, SoftFloatBackend};

#[test]
fn softfloat_backend_exposes_exactly_typed_instance_methods() {
    let _: fn(&SoftFloatBackend, u32, u32, RoundingMode) -> Outcome<u32> =
        SoftFloatBackend::add_f32;
    let _: fn(&SoftFloatBackend, u32, u32, RoundingMode) -> Outcome<u32> =
        SoftFloatBackend::sub_f32;
    let _: fn(&SoftFloatBackend, u32, u32, RoundingMode) -> Outcome<u32> =
        SoftFloatBackend::mul_f32;
    let _: fn(&SoftFloatBackend, u32, u32, RoundingMode) -> Outcome<u32> =
        SoftFloatBackend::div_f32;
    let _: fn(&SoftFloatBackend, u32, RoundingMode) -> Outcome<u32> = SoftFloatBackend::sqrt_f32;

    let _: fn(&SoftFloatBackend, u64, u64, RoundingMode) -> Outcome<u64> =
        SoftFloatBackend::add_f64;
    let _: fn(&SoftFloatBackend, u64, u64, RoundingMode) -> Outcome<u64> =
        SoftFloatBackend::sub_f64;
    let _: fn(&SoftFloatBackend, u64, u64, RoundingMode) -> Outcome<u64> =
        SoftFloatBackend::mul_f64;
    let _: fn(&SoftFloatBackend, u64, u64, RoundingMode) -> Outcome<u64> =
        SoftFloatBackend::div_f64;
    let _: fn(&SoftFloatBackend, u64, RoundingMode) -> Outcome<u64> = SoftFloatBackend::sqrt_f64;

    let _: fn(&SoftFloatBackend, u32, u32) -> Outcome<Relation> = SoftFloatBackend::compare_f32;
    let _: fn(&SoftFloatBackend, u64, u64) -> Outcome<Relation> = SoftFloatBackend::compare_f64;

    let _: fn(&SoftFloatBackend, u32) -> Outcome<u64> = SoftFloatBackend::f32_to_f64;
    let _: fn(&SoftFloatBackend, u64, RoundingMode) -> Outcome<u32> = SoftFloatBackend::f64_to_f32;

    let _: fn(&SoftFloatBackend, i32, RoundingMode) -> Outcome<u32> = SoftFloatBackend::i32_to_f32;
    let _: fn(&SoftFloatBackend, i64, RoundingMode) -> Outcome<u32> = SoftFloatBackend::i64_to_f32;
    let _: fn(&SoftFloatBackend, i32) -> Outcome<u64> = SoftFloatBackend::i32_to_f64;
    let _: fn(&SoftFloatBackend, i64, RoundingMode) -> Outcome<u64> = SoftFloatBackend::i64_to_f64;

    let _: fn(&SoftFloatBackend, u32, RoundingMode) -> Outcome<Option<i32>> =
        SoftFloatBackend::f32_to_i32;
    let _: fn(&SoftFloatBackend, u32, RoundingMode) -> Outcome<Option<i64>> =
        SoftFloatBackend::f32_to_i64;
    let _: fn(&SoftFloatBackend, u64, RoundingMode) -> Outcome<Option<i32>> =
        SoftFloatBackend::f64_to_i32;
    let _: fn(&SoftFloatBackend, u64, RoundingMode) -> Outcome<Option<i64>> =
        SoftFloatBackend::f64_to_i64;
}

#[test]
fn native_backend_exposes_exactly_typed_instance_methods() {
    let _: fn(&NativeBackend, u32, u32) -> u32 = NativeBackend::add_f32;
    let _: fn(&NativeBackend, u32, u32) -> u32 = NativeBackend::sub_f32;
    let _: fn(&NativeBackend, u32, u32) -> u32 = NativeBackend::mul_f32;
    let _: fn(&NativeBackend, u32, u32) -> u32 = NativeBackend::div_f32;
    let _: fn(&NativeBackend, u32) -> u32 = NativeBackend::sqrt_f32;

    let _: fn(&NativeBackend, u64, u64) -> u64 = NativeBackend::add_f64;
    let _: fn(&NativeBackend, u64, u64) -> u64 = NativeBackend::sub_f64;
    let _: fn(&NativeBackend, u64, u64) -> u64 = NativeBackend::mul_f64;
    let _: fn(&NativeBackend, u64, u64) -> u64 = NativeBackend::div_f64;
    let _: fn(&NativeBackend, u64) -> u64 = NativeBackend::sqrt_f64;

    let _: fn(&NativeBackend, u32, u32) -> Relation = NativeBackend::compare_f32;
    let _: fn(&NativeBackend, u64, u64) -> Relation = NativeBackend::compare_f64;

    let _: fn(&NativeBackend, u32) -> u64 = NativeBackend::f32_to_f64;
    let _: fn(&NativeBackend, u64) -> u32 = NativeBackend::f64_to_f32;

    let _: fn(&NativeBackend, i32) -> u32 = NativeBackend::i32_to_f32;
    let _: fn(&NativeBackend, i64) -> u32 = NativeBackend::i64_to_f32;
    let _: fn(&NativeBackend, i32) -> u64 = NativeBackend::i32_to_f64;
    let _: fn(&NativeBackend, i64) -> u64 = NativeBackend::i64_to_f64;

    let _: fn(&NativeBackend, u32) -> Option<i32> = NativeBackend::f32_to_i32;
    let _: fn(&NativeBackend, u32) -> Option<i64> = NativeBackend::f32_to_i64;
    let _: fn(&NativeBackend, u64) -> Option<i32> = NativeBackend::f64_to_i32;
    let _: fn(&NativeBackend, u64) -> Option<i64> = NativeBackend::f64_to_i64;
}
