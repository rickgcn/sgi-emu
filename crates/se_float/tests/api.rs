use se_float::SoftFloatBackend;
use se_float::env::{Outcome, Relation, RoundingMode};

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
