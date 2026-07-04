use super::*;

const CONTROL: FloatControl = FloatControl::new(
    FloatRoundingMode::NearestEven,
    crate::control::FloatTininessMode::AfterRounding,
);

#[test]
fn native_backend_adds_simple_finite_values() {
    let backend = NativeFloatBackend::new();
    let result = backend.add_f32(
        CONTROL,
        Float32Bits::new(1.5_f32.to_bits()),
        Float32Bits::new(2.25_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), 3.75_f32.to_bits());
    assert!(result.flags.is_empty());
}

#[test]
fn native_backend_compares_simple_finite_values() {
    let backend = NativeFloatBackend::new();

    assert_eq!(
        backend
            .compare_f64(
                CONTROL,
                FloatCompareMode::Quiet,
                Float64Bits::new(1.0_f64.to_bits()),
                Float64Bits::new(2.0_f64.to_bits()),
            )
            .value,
        FloatRelation::Less
    );
}

#[test]
fn native_backend_reports_f32_divide_by_zero_without_overflow() {
    let backend = NativeFloatBackend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(1.0_f32.to_bits()),
        Float32Bits::new(0.0_f32.to_bits()),
    );

    assert_eq!(result.value.bits(), f32::INFINITY.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::DIVIDE_BY_ZERO));
    assert!(!result.flags.contains(FloatExceptionFlags::OVERFLOW));
}

#[test]
fn native_backend_reports_f64_divide_by_zero_without_overflow() {
    let backend = NativeFloatBackend::new();
    let result = backend.div_f64(
        CONTROL,
        Float64Bits::new(1.0_f64.to_bits()),
        Float64Bits::new(0.0_f64.to_bits()),
    );

    assert_eq!(result.value.bits(), f64::INFINITY.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::DIVIDE_BY_ZERO));
    assert!(!result.flags.contains(FloatExceptionFlags::OVERFLOW));
}

#[test]
fn native_backend_reports_f32_zero_divided_by_zero_as_invalid() {
    let backend = NativeFloatBackend::new();
    let result = backend.div_f32(
        CONTROL,
        Float32Bits::new(0.0_f32.to_bits()),
        Float32Bits::new(0.0_f32.to_bits()),
    );

    assert!(f32::from_bits(result.value.bits()).is_nan());
    assert!(result.flags.contains(FloatExceptionFlags::INVALID));
}

#[test]
fn native_backend_reports_f64_zero_divided_by_zero_as_invalid() {
    let backend = NativeFloatBackend::new();
    let result = backend.div_f64(
        CONTROL,
        Float64Bits::new(0.0_f64.to_bits()),
        Float64Bits::new(0.0_f64.to_bits()),
    );

    assert!(f64::from_bits(result.value.bits()).is_nan());
    assert!(result.flags.contains(FloatExceptionFlags::INVALID));
}

#[test]
fn native_backend_reports_exact_i32_to_f32_without_flags() {
    let backend = NativeFloatBackend::new();
    let result = backend.i32_to_f32(CONTROL, 16_777_216);

    assert_eq!(result.value.bits(), 16_777_216_f32.to_bits());
    assert!(result.flags.is_empty());
}

#[test]
fn native_backend_reports_i32_max_to_f32_as_inexact() {
    let backend = NativeFloatBackend::new();
    let result = backend.i32_to_f32(CONTROL, i32::MAX);

    assert_eq!(result.value.bits(), 2_147_483_648_f32.to_bits());
    assert!(result.flags.contains(FloatExceptionFlags::INEXACT));
}

#[test]
fn native_backend_reports_f32_two_to_31_to_i32_as_invalid() {
    let backend = NativeFloatBackend::new();
    let result = backend.f32_to_i32(CONTROL, Float32Bits::new(0x4f00_0000));

    assert!(result.flags.contains(FloatExceptionFlags::INVALID));
}

#[test]
fn native_backend_reports_f64_two_to_31_to_i32_as_invalid() {
    let backend = NativeFloatBackend::new();
    let result = backend.f64_to_i32(CONTROL, Float64Bits::new(2_147_483_648.0_f64.to_bits()));

    assert!(result.flags.contains(FloatExceptionFlags::INVALID));
}
