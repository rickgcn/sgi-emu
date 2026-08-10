//! Implements host-native value operations with Rust floating-point primitives.
//!
//! Every operation uses the fixed toolchain's default floating-point control
//! state and returns only a value, relation, or conversion-validity result.
//! Guest exception state and result policy remain outside this module.

use core::cmp::Ordering;

use crate::NativeBackend;
use crate::env::Relation;

const I32_LOWER_F32: f32 = -2_147_483_648.0;
const I32_UPPER_F32: f32 = 2_147_483_648.0;
const I64_LOWER_F32: f32 = -9_223_372_036_854_775_808.0;
const I64_UPPER_F32: f32 = 9_223_372_036_854_775_808.0;
const I32_LOWER_F64: f64 = -2_147_483_648.0;
const I32_UPPER_F64: f64 = 2_147_483_648.0;
const I64_LOWER_F64: f64 = -9_223_372_036_854_775_808.0;
const I64_UPPER_F64: f64 = 9_223_372_036_854_775_808.0;

fn relation(ordering: Option<Ordering>) -> Relation {
    match ordering {
        Some(Ordering::Less) => Relation::Less,
        Some(Ordering::Equal) => Relation::Equal,
        Some(Ordering::Greater) => Relation::Greater,
        None => Relation::Unordered,
    }
}

fn round_f32_to_i32(value: f32) -> Option<i32> {
    let rounded = value.round_ties_even();
    if rounded.is_finite() && (I32_LOWER_F32..I32_UPPER_F32).contains(&rounded) {
        Some(rounded as i32)
    } else {
        None
    }
}

fn round_f32_to_i64(value: f32) -> Option<i64> {
    let rounded = value.round_ties_even();
    if rounded.is_finite() && (I64_LOWER_F32..I64_UPPER_F32).contains(&rounded) {
        Some(rounded as i64)
    } else {
        None
    }
}

fn round_f64_to_i32(value: f64) -> Option<i32> {
    let rounded = value.round_ties_even();
    if rounded.is_finite() && (I32_LOWER_F64..I32_UPPER_F64).contains(&rounded) {
        Some(rounded as i32)
    } else {
        None
    }
}

fn round_f64_to_i64(value: f64) -> Option<i64> {
    let rounded = value.round_ties_even();
    if rounded.is_finite() && (I64_LOWER_F64..I64_UPPER_F64).contains(&rounded) {
        Some(rounded as i64)
    } else {
        None
    }
}

impl NativeBackend {
    /// Adds two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f32` addition uses fixed roundTiesToEven and the
    /// returned bits come directly from `to_bits`. A NaN result promises only
    /// the NaN category. This operation provides no flags or rounding facts
    /// and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn add_f32(&self, a: u32, b: u32) -> u32 {
        (f32::from_bits(a) + f32::from_bits(b)).to_bits()
    }

    /// Subtracts two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f32` subtraction uses fixed roundTiesToEven and
    /// the returned bits come directly from `to_bits`. A NaN result promises
    /// only the NaN category. This operation provides no flags or rounding
    /// facts and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn sub_f32(&self, a: u32, b: u32) -> u32 {
        (f32::from_bits(a) - f32::from_bits(b)).to_bits()
    }

    /// Multiplies two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f32` multiplication uses fixed roundTiesToEven
    /// and the returned bits come directly from `to_bits`. A NaN result
    /// promises only the NaN category. This operation provides no flags or
    /// rounding facts and does not inspect or manage the host floating-point
    /// environment.
    #[must_use]
    pub fn mul_f32(&self, a: u32, b: u32) -> u32 {
        (f32::from_bits(a) * f32::from_bits(b)).to_bits()
    }

    /// Divides two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f32` division uses fixed roundTiesToEven and the
    /// returned bits come directly from `to_bits`. A NaN result promises only
    /// the NaN category. This operation provides no flags or rounding facts
    /// and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn div_f32(&self, a: u32, b: u32) -> u32 {
        (f32::from_bits(a) / f32::from_bits(b)).to_bits()
    }

    /// Computes the square root of an IEEE binary32 raw `u32` bit pattern.
    ///
    /// Rust 1.95.0 `f32::sqrt` uses fixed roundTiesToEven and the returned bits
    /// come directly from `to_bits`. A NaN result promises only the NaN
    /// category. This operation provides no flags or rounding facts and does
    /// not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn sqrt_f32(&self, value: u32) -> u32 {
        f32::from_bits(value).sqrt().to_bits()
    }

    /// Adds two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f64` addition uses fixed roundTiesToEven and the
    /// returned bits come directly from `to_bits`. A NaN result promises only
    /// the NaN category. This operation provides no flags or rounding facts
    /// and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn add_f64(&self, a: u64, b: u64) -> u64 {
        (f64::from_bits(a) + f64::from_bits(b)).to_bits()
    }

    /// Subtracts two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f64` subtraction uses fixed roundTiesToEven and
    /// the returned bits come directly from `to_bits`. A NaN result promises
    /// only the NaN category. This operation provides no flags or rounding
    /// facts and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn sub_f64(&self, a: u64, b: u64) -> u64 {
        (f64::from_bits(a) - f64::from_bits(b)).to_bits()
    }

    /// Multiplies two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f64` multiplication uses fixed roundTiesToEven
    /// and the returned bits come directly from `to_bits`. A NaN result
    /// promises only the NaN category. This operation provides no flags or
    /// rounding facts and does not inspect or manage the host floating-point
    /// environment.
    #[must_use]
    pub fn mul_f64(&self, a: u64, b: u64) -> u64 {
        (f64::from_bits(a) * f64::from_bits(b)).to_bits()
    }

    /// Divides two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// Ordinary Rust 1.95.0 `f64` division uses fixed roundTiesToEven and the
    /// returned bits come directly from `to_bits`. A NaN result promises only
    /// the NaN category. This operation provides no flags or rounding facts
    /// and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn div_f64(&self, a: u64, b: u64) -> u64 {
        (f64::from_bits(a) / f64::from_bits(b)).to_bits()
    }

    /// Computes the square root of an IEEE binary64 raw `u64` bit pattern.
    ///
    /// Rust 1.95.0 `f64::sqrt` uses fixed roundTiesToEven and the returned bits
    /// come directly from `to_bits`. A NaN result promises only the NaN
    /// category. This operation provides no flags or rounding facts and does
    /// not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn sqrt_f64(&self, value: u64) -> u64 {
        f64::from_bits(value).sqrt().to_bits()
    }

    /// Quietly compares two IEEE binary32 raw `u32` bit patterns.
    ///
    /// Rust `f32` comparison treats both zero signs as equal and maps any NaN
    /// operand to [`Relation::Unordered`]. This operation accepts no rounding
    /// mode, provides no flags or rounding facts, and does not inspect or
    /// manage the host floating-point environment.
    #[must_use]
    pub fn compare_f32(&self, a: u32, b: u32) -> Relation {
        relation(f32::from_bits(a).partial_cmp(&f32::from_bits(b)))
    }

    /// Quietly compares two IEEE binary64 raw `u64` bit patterns.
    ///
    /// Rust `f64` comparison treats both zero signs as equal and maps any NaN
    /// operand to [`Relation::Unordered`]. This operation accepts no rounding
    /// mode, provides no flags or rounding facts, and does not inspect or
    /// manage the host floating-point environment.
    #[must_use]
    pub fn compare_f64(&self, a: u64, b: u64) -> Relation {
        relation(f64::from_bits(a).partial_cmp(&f64::from_bits(b)))
    }

    /// Widens an IEEE binary32 raw `u32` bit pattern to binary64 raw bits.
    ///
    /// Rust 1.95.0 performs the finite conversion exactly and returns the
    /// `f64::to_bits` result unchanged. A NaN result promises only the NaN
    /// category. This operation provides no flags or rounding facts and does
    /// not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn f32_to_f64(&self, value: u32) -> u64 {
        (f32::from_bits(value) as f64).to_bits()
    }

    /// Narrows an IEEE binary64 raw `u64` bit pattern to binary32 raw bits.
    ///
    /// Rust 1.95.0 performs the conversion with fixed roundTiesToEven and
    /// returns the `f32::to_bits` result unchanged. A NaN result promises only
    /// the NaN category. This operation provides no flags or rounding facts
    /// and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn f64_to_f32(&self, value: u64) -> u32 {
        (f64::from_bits(value) as f32).to_bits()
    }

    /// Converts an `i32` value to an IEEE binary32 raw `u32` bit pattern.
    ///
    /// Rust 1.95.0 performs the conversion with fixed roundTiesToEven and
    /// returns `f32::to_bits` unchanged. This operation provides no flags or
    /// rounding facts and does not inspect or manage the host floating-point
    /// environment.
    #[must_use]
    pub fn i32_to_f32(&self, value: i32) -> u32 {
        (value as f32).to_bits()
    }

    /// Converts an `i64` value to an IEEE binary32 raw `u32` bit pattern.
    ///
    /// Rust 1.95.0 performs the conversion with fixed roundTiesToEven and
    /// returns `f32::to_bits` unchanged. This operation provides no flags or
    /// rounding facts and does not inspect or manage the host floating-point
    /// environment.
    #[must_use]
    pub fn i64_to_f32(&self, value: i64) -> u32 {
        (value as f32).to_bits()
    }

    /// Converts an `i32` value to an IEEE binary64 raw `u64` bit pattern.
    ///
    /// Every `i32` is represented exactly by Rust 1.95.0 `f64`, and the method
    /// returns `f64::to_bits` unchanged. This operation provides no flags or
    /// rounding facts and does not inspect or manage the host floating-point
    /// environment.
    #[must_use]
    pub fn i32_to_f64(&self, value: i32) -> u64 {
        (value as f64).to_bits()
    }

    /// Converts an `i64` value to an IEEE binary64 raw `u64` bit pattern.
    ///
    /// Rust 1.95.0 performs the conversion with fixed roundTiesToEven and
    /// returns `f64::to_bits` unchanged. This operation provides no flags or
    /// rounding facts and does not inspect or manage the host floating-point
    /// environment.
    #[must_use]
    pub fn i64_to_f64(&self, value: i64) -> u64 {
        (value as f64).to_bits()
    }

    /// Converts an IEEE binary32 raw `u32` bit pattern to an `i32` value.
    ///
    /// The value is rounded with `round_ties_even` and accepted only when the
    /// rounded result is in `-2^31..2^31`. NaN, infinity, and out-of-range
    /// results return `None`. This operation provides no flags or rounding
    /// facts and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn f32_to_i32(&self, value: u32) -> Option<i32> {
        round_f32_to_i32(f32::from_bits(value))
    }

    /// Converts an IEEE binary32 raw `u32` bit pattern to an `i64` value.
    ///
    /// The value is rounded with `round_ties_even` and accepted only when the
    /// rounded result is in `-2^63..2^63`. NaN, infinity, and out-of-range
    /// results return `None`. This operation provides no flags or rounding
    /// facts and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn f32_to_i64(&self, value: u32) -> Option<i64> {
        round_f32_to_i64(f32::from_bits(value))
    }

    /// Converts an IEEE binary64 raw `u64` bit pattern to an `i32` value.
    ///
    /// The value is rounded with `round_ties_even` and accepted only when the
    /// rounded result is in `-2^31..2^31`. NaN, infinity, and out-of-range
    /// results return `None`. This operation provides no flags or rounding
    /// facts and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn f64_to_i32(&self, value: u64) -> Option<i32> {
        round_f64_to_i32(f64::from_bits(value))
    }

    /// Converts an IEEE binary64 raw `u64` bit pattern to an `i64` value.
    ///
    /// The value is rounded with `round_ties_even` and accepted only when the
    /// rounded result is in `-2^63..2^63`. NaN, infinity, and out-of-range
    /// results return `None`. This operation provides no flags or rounding
    /// facts and does not inspect or manage the host floating-point environment.
    #[must_use]
    pub fn f64_to_i64(&self, value: u64) -> Option<i64> {
        round_f64_to_i64(f64::from_bits(value))
    }
}
