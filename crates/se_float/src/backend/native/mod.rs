//! Best-effort host-native floating-point backend.
//!
//! This backend uses Rust `f32` and `f64` operations. It is useful for simple
//! fast execution paths but is not a reference implementation for exact
//! rounding control, NaN payload propagation, signaling NaNs, or exception flag
//! reporting.

use crate::backend::FloatBackend;
use crate::control::{FloatControl, FloatExceptionFlags, FloatRoundingMode};
use crate::result::FloatResult;
use crate::value::{
    Float32Bits, Float64Bits, FloatClass, FloatCompareMode, FloatNanMode, FloatRelation,
};

/// Host-native floating-point backend.
#[derive(Clone, Copy, Debug, Default)]
pub struct NativeFloatBackend;

impl NativeFloatBackend {
    /// Creates a host-native backend.
    pub const fn new() -> Self {
        Self
    }
}

impl FloatBackend for NativeFloatBackend {
    fn add_f32(
        &self,
        _control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        native_f32_binary(lhs, rhs, |lhs, rhs| lhs + rhs)
    }

    fn sub_f32(
        &self,
        _control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        native_f32_binary(lhs, rhs, |lhs, rhs| lhs - rhs)
    }

    fn mul_f32(
        &self,
        _control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        native_f32_binary(lhs, rhs, |lhs, rhs| lhs * rhs)
    }

    fn div_f32(
        &self,
        _control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        native_f32_div(lhs, rhs)
    }

    fn sqrt_f32(&self, _control: FloatControl, value: Float32Bits) -> FloatResult<Float32Bits> {
        let input = f32::from_bits(value.bits());
        let result = input.sqrt();
        let flags = if input < 0.0 {
            FloatExceptionFlags::INVALID
        } else {
            FloatExceptionFlags::empty()
        };
        FloatResult::new(Float32Bits::new(result.to_bits()), flags)
    }

    fn mul_add_f32(
        &self,
        _control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
        addend: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        let lhs = f32::from_bits(lhs.bits());
        let rhs = f32::from_bits(rhs.bits());
        let addend = f32::from_bits(addend.bits());
        let value = lhs.mul_add(rhs, addend);
        FloatResult::new(
            Float32Bits::new(value.to_bits()),
            native_ternary_flags_f32(lhs, rhs, addend, value),
        )
    }

    fn add_f64(
        &self,
        _control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        native_f64_binary(lhs, rhs, |lhs, rhs| lhs + rhs)
    }

    fn sub_f64(
        &self,
        _control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        native_f64_binary(lhs, rhs, |lhs, rhs| lhs - rhs)
    }

    fn mul_f64(
        &self,
        _control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        native_f64_binary(lhs, rhs, |lhs, rhs| lhs * rhs)
    }

    fn div_f64(
        &self,
        _control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        native_f64_div(lhs, rhs)
    }

    fn sqrt_f64(&self, _control: FloatControl, value: Float64Bits) -> FloatResult<Float64Bits> {
        let input = f64::from_bits(value.bits());
        let result = input.sqrt();
        let flags = if input < 0.0 {
            FloatExceptionFlags::INVALID
        } else {
            FloatExceptionFlags::empty()
        };
        FloatResult::new(Float64Bits::new(result.to_bits()), flags)
    }

    fn mul_add_f64(
        &self,
        _control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
        addend: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        let lhs = f64::from_bits(lhs.bits());
        let rhs = f64::from_bits(rhs.bits());
        let addend = f64::from_bits(addend.bits());
        let value = lhs.mul_add(rhs, addend);
        FloatResult::new(
            Float64Bits::new(value.to_bits()),
            native_ternary_flags_f64(lhs, rhs, addend, value),
        )
    }

    fn f32_to_f64(&self, _control: FloatControl, value: Float32Bits) -> FloatResult<Float64Bits> {
        let converted = f32::from_bits(value.bits()) as f64;
        FloatResult::new(
            Float64Bits::new(converted.to_bits()),
            FloatExceptionFlags::empty(),
        )
    }

    fn f64_to_f32(&self, _control: FloatControl, value: Float64Bits) -> FloatResult<Float32Bits> {
        let converted = f64::from_bits(value.bits()) as f32;
        FloatResult::new(
            Float32Bits::new(converted.to_bits()),
            FloatExceptionFlags::empty(),
        )
    }

    fn i32_to_f32(&self, _control: FloatControl, value: i32) -> FloatResult<Float32Bits> {
        let converted = value as f32;
        let flags = if converted as f64 == value as f64 {
            FloatExceptionFlags::empty()
        } else {
            FloatExceptionFlags::INEXACT
        };

        FloatResult::new(Float32Bits::new(converted.to_bits()), flags)
    }

    fn i32_to_f64(&self, _control: FloatControl, value: i32) -> FloatResult<Float64Bits> {
        FloatResult::new(
            Float64Bits::new((value as f64).to_bits()),
            FloatExceptionFlags::empty(),
        )
    }

    fn i64_to_f32(&self, _control: FloatControl, value: i64) -> FloatResult<Float32Bits> {
        let converted = value as f32;
        let flags = if converted as i64 == value {
            FloatExceptionFlags::empty()
        } else {
            FloatExceptionFlags::INEXACT
        };
        FloatResult::new(Float32Bits::new(converted.to_bits()), flags)
    }

    fn i64_to_f64(&self, _control: FloatControl, value: i64) -> FloatResult<Float64Bits> {
        let converted = value as f64;
        let flags = if converted as i64 == value {
            FloatExceptionFlags::empty()
        } else {
            FloatExceptionFlags::INEXACT
        };
        FloatResult::new(Float64Bits::new(converted.to_bits()), flags)
    }

    fn f32_to_i32(&self, control: FloatControl, value: Float32Bits) -> FloatResult<i32> {
        native_f32_to_i32(control, value)
    }

    fn f64_to_i32(&self, control: FloatControl, value: Float64Bits) -> FloatResult<i32> {
        native_f64_to_i32(control, value)
    }

    fn f32_to_i64(&self, control: FloatControl, value: Float32Bits) -> FloatResult<i64> {
        native_f32_to_i64(control, value)
    }

    fn f64_to_i64(&self, control: FloatControl, value: Float64Bits) -> FloatResult<i64> {
        native_f64_to_i64(control, value)
    }

    fn compare_f32(
        &self,
        _control: FloatControl,
        mode: FloatCompareMode,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<FloatRelation> {
        native_compare(
            mode,
            lhs.classify(FloatNanMode::QuietBitSet),
            rhs.classify(FloatNanMode::QuietBitSet),
            || compare_f32_values(f32::from_bits(lhs.bits()), f32::from_bits(rhs.bits())),
        )
    }

    fn compare_f64(
        &self,
        _control: FloatControl,
        mode: FloatCompareMode,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<FloatRelation> {
        native_compare(
            mode,
            lhs.classify(FloatNanMode::QuietBitSet),
            rhs.classify(FloatNanMode::QuietBitSet),
            || compare_f64_values(f64::from_bits(lhs.bits()), f64::from_bits(rhs.bits())),
        )
    }
}

fn native_f32_binary(
    lhs: Float32Bits,
    rhs: Float32Bits,
    operation: impl FnOnce(f32, f32) -> f32,
) -> FloatResult<Float32Bits> {
    let lhs_value = f32::from_bits(lhs.bits());
    let rhs_value = f32::from_bits(rhs.bits());
    let value = operation(lhs_value, rhs_value);

    FloatResult::new(
        Float32Bits::new(value.to_bits()),
        native_binary_flags_f32(lhs_value, rhs_value, value),
    )
}

fn native_f64_binary(
    lhs: Float64Bits,
    rhs: Float64Bits,
    operation: impl FnOnce(f64, f64) -> f64,
) -> FloatResult<Float64Bits> {
    let lhs_value = f64::from_bits(lhs.bits());
    let rhs_value = f64::from_bits(rhs.bits());
    let value = operation(lhs_value, rhs_value);

    FloatResult::new(
        Float64Bits::new(value.to_bits()),
        native_binary_flags_f64(lhs_value, rhs_value, value),
    )
}

fn native_f32_div(lhs: Float32Bits, rhs: Float32Bits) -> FloatResult<Float32Bits> {
    let lhs_value = f32::from_bits(lhs.bits());
    let rhs_value = f32::from_bits(rhs.bits());
    let value = lhs_value / rhs_value;

    FloatResult::new(
        Float32Bits::new(value.to_bits()),
        native_div_flags_f32(lhs_value, rhs_value, value),
    )
}

fn native_f64_div(lhs: Float64Bits, rhs: Float64Bits) -> FloatResult<Float64Bits> {
    let lhs_value = f64::from_bits(lhs.bits());
    let rhs_value = f64::from_bits(rhs.bits());
    let value = lhs_value / rhs_value;

    FloatResult::new(
        Float64Bits::new(value.to_bits()),
        native_div_flags_f64(lhs_value, rhs_value, value),
    )
}

fn native_binary_flags_f32(lhs: f32, rhs: f32, value: f32) -> FloatExceptionFlags {
    let mut flags = FloatExceptionFlags::empty();

    if value.is_nan() && !lhs.is_nan() && !rhs.is_nan() {
        flags |= FloatExceptionFlags::INVALID;
    }
    if value.is_infinite() && lhs.is_finite() && rhs.is_finite() {
        flags |= FloatExceptionFlags::OVERFLOW;
    }

    flags
}

fn native_binary_flags_f64(lhs: f64, rhs: f64, value: f64) -> FloatExceptionFlags {
    let mut flags = FloatExceptionFlags::empty();

    if value.is_nan() && !lhs.is_nan() && !rhs.is_nan() {
        flags |= FloatExceptionFlags::INVALID;
    }
    if value.is_infinite() && lhs.is_finite() && rhs.is_finite() {
        flags |= FloatExceptionFlags::OVERFLOW;
    }

    flags
}

fn native_ternary_flags_f32(lhs: f32, rhs: f32, addend: f32, value: f32) -> FloatExceptionFlags {
    let mut flags = FloatExceptionFlags::empty();
    if value.is_nan() && !lhs.is_nan() && !rhs.is_nan() && !addend.is_nan() {
        flags |= FloatExceptionFlags::INVALID;
    }
    if value.is_infinite() && lhs.is_finite() && rhs.is_finite() && addend.is_finite() {
        flags |= FloatExceptionFlags::OVERFLOW;
    }
    flags
}

fn native_ternary_flags_f64(lhs: f64, rhs: f64, addend: f64, value: f64) -> FloatExceptionFlags {
    let mut flags = FloatExceptionFlags::empty();
    if value.is_nan() && !lhs.is_nan() && !rhs.is_nan() && !addend.is_nan() {
        flags |= FloatExceptionFlags::INVALID;
    }
    if value.is_infinite() && lhs.is_finite() && rhs.is_finite() && addend.is_finite() {
        flags |= FloatExceptionFlags::OVERFLOW;
    }
    flags
}

fn native_div_flags_f32(lhs: f32, rhs: f32, value: f32) -> FloatExceptionFlags {
    let mut flags = FloatExceptionFlags::empty();

    if lhs.is_finite() && lhs != 0.0 && rhs == 0.0 {
        flags |= FloatExceptionFlags::DIVIDE_BY_ZERO;
    } else if value.is_nan() && !lhs.is_nan() && !rhs.is_nan() {
        flags |= FloatExceptionFlags::INVALID;
    } else if value.is_infinite() && lhs.is_finite() && rhs.is_finite() {
        flags |= FloatExceptionFlags::OVERFLOW;
    }

    flags
}

fn native_div_flags_f64(lhs: f64, rhs: f64, value: f64) -> FloatExceptionFlags {
    let mut flags = FloatExceptionFlags::empty();

    if lhs.is_finite() && lhs != 0.0 && rhs == 0.0 {
        flags |= FloatExceptionFlags::DIVIDE_BY_ZERO;
    } else if value.is_nan() && !lhs.is_nan() && !rhs.is_nan() {
        flags |= FloatExceptionFlags::INVALID;
    } else if value.is_infinite() && lhs.is_finite() && rhs.is_finite() {
        flags |= FloatExceptionFlags::OVERFLOW;
    }

    flags
}

fn native_f32_to_i32(control: FloatControl, value: Float32Bits) -> FloatResult<i32> {
    let value = f32::from_bits(value.bits());
    let rounded = round_f32(control.rounding_mode, value);
    let mut flags = FloatExceptionFlags::empty();

    if !value.is_finite() || !i32_range_contains(rounded as f64) {
        flags |= FloatExceptionFlags::INVALID;
    } else if rounded != value {
        flags |= FloatExceptionFlags::INEXACT;
    }

    FloatResult::new(rounded as i32, flags)
}

fn native_f64_to_i32(control: FloatControl, value: Float64Bits) -> FloatResult<i32> {
    let value = f64::from_bits(value.bits());
    let rounded = round_f64(control.rounding_mode, value);
    let mut flags = FloatExceptionFlags::empty();

    if !value.is_finite() || !i32_range_contains(rounded) {
        flags |= FloatExceptionFlags::INVALID;
    } else if rounded != value {
        flags |= FloatExceptionFlags::INEXACT;
    }

    FloatResult::new(rounded as i32, flags)
}

fn native_f32_to_i64(control: FloatControl, value: Float32Bits) -> FloatResult<i64> {
    let value = f32::from_bits(value.bits());
    let rounded = round_f32(control.rounding_mode, value);
    let mut flags = FloatExceptionFlags::empty();
    if !value.is_finite() || !i64_range_contains(rounded as f64) {
        flags |= FloatExceptionFlags::INVALID;
    } else if rounded != value {
        flags |= FloatExceptionFlags::INEXACT;
    }
    FloatResult::new(rounded as i64, flags)
}

fn native_f64_to_i64(control: FloatControl, value: Float64Bits) -> FloatResult<i64> {
    let value = f64::from_bits(value.bits());
    let rounded = round_f64(control.rounding_mode, value);
    let mut flags = FloatExceptionFlags::empty();
    if !value.is_finite() || !i64_range_contains(rounded) {
        flags |= FloatExceptionFlags::INVALID;
    } else if rounded != value {
        flags |= FloatExceptionFlags::INEXACT;
    }
    FloatResult::new(rounded as i64, flags)
}

fn i32_range_contains(value: f64) -> bool {
    value >= i32::MIN as f64 && value <= i32::MAX as f64
}

fn i64_range_contains(value: f64) -> bool {
    value >= i64::MIN as f64 && value < -(i64::MIN as f64)
}

fn round_f32(rounding_mode: FloatRoundingMode, value: f32) -> f32 {
    match rounding_mode {
        FloatRoundingMode::NearestEven => value.round_ties_even(),
        FloatRoundingMode::TowardZero => value.trunc(),
        FloatRoundingMode::TowardPositive => value.ceil(),
        FloatRoundingMode::TowardNegative => value.floor(),
    }
}

fn round_f64(rounding_mode: FloatRoundingMode, value: f64) -> f64 {
    match rounding_mode {
        FloatRoundingMode::NearestEven => value.round_ties_even(),
        FloatRoundingMode::TowardZero => value.trunc(),
        FloatRoundingMode::TowardPositive => value.ceil(),
        FloatRoundingMode::TowardNegative => value.floor(),
    }
}

fn native_compare(
    mode: FloatCompareMode,
    lhs_class: FloatClass,
    rhs_class: FloatClass,
    ordered: impl FnOnce() -> FloatRelation,
) -> FloatResult<FloatRelation> {
    if lhs_class.is_nan() || rhs_class.is_nan() {
        let invalid = match mode {
            FloatCompareMode::Quiet => lhs_class.is_signaling_nan() || rhs_class.is_signaling_nan(),
            FloatCompareMode::Signaling => true,
        };

        let flags = if invalid {
            FloatExceptionFlags::INVALID
        } else {
            FloatExceptionFlags::empty()
        };

        return FloatResult::new(FloatRelation::Unordered, flags);
    }

    FloatResult::new(ordered(), FloatExceptionFlags::empty())
}

fn compare_f32_values(lhs: f32, rhs: f32) -> FloatRelation {
    if lhs < rhs {
        FloatRelation::Less
    } else if lhs == rhs {
        FloatRelation::Equal
    } else {
        FloatRelation::Greater
    }
}

fn compare_f64_values(lhs: f64, rhs: f64) -> FloatRelation {
    if lhs < rhs {
        FloatRelation::Less
    } else if lhs == rhs {
        FloatRelation::Equal
    } else {
        FloatRelation::Greater
    }
}

#[cfg(test)]
mod tests;
