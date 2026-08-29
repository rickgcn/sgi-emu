mod ffi;
mod mapping;

use crate::format::{Float32, Float64};
use crate::operation::{ComparisonMode, ExceptionFlags, Outcome, Relation, RoundingMode};

const DEFAULT_NAN_F32: u32 = 0x7fbf_ffff;
const DEFAULT_NAN_F64: u64 = 0x7ff7_ffff_ffff_ffff;
const SIGN_F32: u32 = 0x8000_0000;
const SIGN_F64: u64 = 0x8000_0000_0000_0000;

pub(super) fn add_f32(lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_add_f32(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f32(bits, raw_flags)
}

pub(super) fn add_f64(lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_add_f64(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f64(bits, raw_flags)
}

pub(super) fn sub_f32(lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_sub_f32(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f32(bits, raw_flags)
}

pub(super) fn sub_f64(lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_sub_f64(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f64(bits, raw_flags)
}

pub(super) fn mul_f32(lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_mul_f32(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f32(bits, raw_flags)
}

pub(super) fn mul_f64(lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_mul_f64(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f64(bits, raw_flags)
}

pub(super) fn div_f32(lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_div_f32(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f32(bits, raw_flags)
}

pub(super) fn div_f64(lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_div_f64(
            lhs.to_bits(),
            rhs.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f64(bits, raw_flags)
}

pub(super) fn abs_f32(value: Float32) -> Outcome<Float32> {
    let bits = value.to_bits();
    if is_signaling_nan_f32(bits) {
        invalid_f32()
    } else {
        exact_f32(bits & !SIGN_F32, ExceptionFlags::empty())
    }
}

pub(super) fn abs_f64(value: Float64) -> Outcome<Float64> {
    let bits = value.to_bits();
    if is_signaling_nan_f64(bits) {
        invalid_f64()
    } else {
        exact_f64(bits & !SIGN_F64, ExceptionFlags::empty())
    }
}

pub(super) fn neg_f32(value: Float32) -> Outcome<Float32> {
    let bits = value.to_bits();
    if is_signaling_nan_f32(bits) {
        invalid_f32()
    } else {
        exact_f32(bits ^ SIGN_F32, ExceptionFlags::empty())
    }
}

pub(super) fn neg_f64(value: Float64) -> Outcome<Float64> {
    let bits = value.to_bits();
    if is_signaling_nan_f64(bits) {
        invalid_f64()
    } else {
        exact_f64(bits ^ SIGN_F64, ExceptionFlags::empty())
    }
}

pub(super) fn convert_float32_to_float64(value: Float32) -> Outcome<Float64> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits =
        unsafe { ffi::se_float_softfloat_convert_f32_to_f64(value.to_bits(), &mut raw_flags) };
    exact_f64(bits, mapping::exception_flags(raw_flags))
}

pub(super) fn convert_float64_to_float32(
    value: Float64,
    rounding: RoundingMode,
) -> Outcome<Float32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_convert_f64_to_f32(
            value.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f32(bits, raw_flags)
}

pub(super) fn convert_i32_to_float32(value: i32, rounding: RoundingMode) -> Outcome<Float32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts a fixed-width integer and a valid pointer to one writable byte.
    let bits = unsafe {
        ffi::se_float_softfloat_convert_i32_to_f32(
            value,
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    rounded_f32(bits, raw_flags)
}

pub(super) fn convert_i32_to_float64(value: i32) -> Outcome<Float64> {
    // SAFETY: Every i32 is a valid input and the bridge returns raw binary64 bits by value.
    let bits = unsafe { ffi::se_float_softfloat_convert_i32_to_f64(value) };
    exact_f64(bits, ExceptionFlags::empty())
}

pub(super) fn convert_float32_to_i32(value: Float32, rounding: RoundingMode) -> Outcome<i32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let value = unsafe {
        ffi::se_float_softfloat_convert_f32_to_i32(
            value.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    integer(value, raw_flags)
}

pub(super) fn convert_float64_to_i32(value: Float64, rounding: RoundingMode) -> Outcome<i32> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let value = unsafe {
        ffi::se_float_softfloat_convert_f64_to_i32(
            value.to_bits(),
            mapping::rounding_mode(rounding),
            &mut raw_flags,
        )
    };
    integer(value, raw_flags)
}

pub(super) fn compare_f32(lhs: Float32, rhs: Float32, mode: ComparisonMode) -> Outcome<Relation> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let relation = unsafe {
        ffi::se_float_softfloat_compare_f32(
            lhs.to_bits(),
            rhs.to_bits(),
            u8::from(matches!(mode, ComparisonMode::Signaling)),
            &mut raw_flags,
        )
    };
    comparison(relation, raw_flags)
}

pub(super) fn compare_f64(lhs: Float64, rhs: Float64, mode: ComparisonMode) -> Outcome<Relation> {
    let mut raw_flags = 0;
    // SAFETY: The bridge accepts raw value bits and a valid pointer to one writable byte.
    let relation = unsafe {
        ffi::se_float_softfloat_compare_f64(
            lhs.to_bits(),
            rhs.to_bits(),
            u8::from(matches!(mode, ComparisonMode::Signaling)),
            &mut raw_flags,
        )
    };
    comparison(relation, raw_flags)
}

fn rounded_f32(bits: u32, raw_flags: u8) -> Outcome<Float32> {
    let flags = mapping::exception_flags(raw_flags);
    Outcome {
        value: Float32::from_bits(bits),
        flags,
        tiny: flags.contains(ExceptionFlags::UNDERFLOW) || mapping::is_subnormal_f32(bits),
    }
}

fn rounded_f64(bits: u64, raw_flags: u8) -> Outcome<Float64> {
    let flags = mapping::exception_flags(raw_flags);
    Outcome {
        value: Float64::from_bits(bits),
        flags,
        tiny: flags.contains(ExceptionFlags::UNDERFLOW) || mapping::is_subnormal_f64(bits),
    }
}

const fn exact_f32(bits: u32, flags: ExceptionFlags) -> Outcome<Float32> {
    Outcome {
        value: Float32::from_bits(bits),
        flags,
        tiny: false,
    }
}

const fn exact_f64(bits: u64, flags: ExceptionFlags) -> Outcome<Float64> {
    Outcome {
        value: Float64::from_bits(bits),
        flags,
        tiny: false,
    }
}

fn invalid_f32() -> Outcome<Float32> {
    exact_f32(DEFAULT_NAN_F32, ExceptionFlags::INVALID)
}

fn invalid_f64() -> Outcome<Float64> {
    exact_f64(DEFAULT_NAN_F64, ExceptionFlags::INVALID)
}

fn integer(value: i32, raw_flags: u8) -> Outcome<i32> {
    Outcome {
        value,
        flags: mapping::exception_flags(raw_flags),
        tiny: false,
    }
}

fn comparison(value: u8, raw_flags: u8) -> Outcome<Relation> {
    Outcome {
        value: mapping::relation(value),
        flags: mapping::exception_flags(raw_flags),
        tiny: false,
    }
}

const fn is_signaling_nan_f32(bits: u32) -> bool {
    bits & 0x7fc0_0000 == 0x7fc0_0000
}

const fn is_signaling_nan_f64(bits: u64) -> bool {
    bits & 0x7ff8_0000_0000_0000 == 0x7ff8_0000_0000_0000
}
