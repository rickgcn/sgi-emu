//! Floating-point backend selection and operations.

use crate::format::{Float32, Float64};
use crate::operation::{ComparisonMode, Outcome, Relation, RoundingMode};
use crate::{native, softfloat};

/// Selects the implementation used for floating-point operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Backend {
    /// Exact crate-level legacy MIPS semantics implemented with Berkeley SoftFloat.
    ///
    /// This backend implements the operations, exception conditions, NaN convention,
    /// rounding modes, and tininess contract exposed by this crate. It does not model a
    /// complete R3010, FCR31 state, instruction exceptions, or register writeback.
    SoftFloat,
    /// Fast host arithmetic with platform-dependent and intentionally approximate semantics.
    ///
    /// This backend uses host `f32` and `f64` arithmetic and Rust numeric casts. It ignores
    /// requested rounding and comparison modes, never collects exception flags, and accepts
    /// host NaN conventions, payload behavior, subnormal handling, and default rounding.
    /// Empty exception flags therefore mean that exceptions were not collected, not that no
    /// exception occurred. Tininess is approximated only when an arithmetic or binary64-to-
    /// binary32 result remains subnormal; underflow to zero cannot be detected.
    Native,
}

impl Backend {
    /// Adds two binary32 values.
    pub fn add_f32(&self, lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::add_f32(lhs, rhs, rounding),
            Self::Native => native::add_f32(lhs, rhs, rounding),
        }
    }

    /// Adds two binary64 values.
    pub fn add_f64(&self, lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::add_f64(lhs, rhs, rounding),
            Self::Native => native::add_f64(lhs, rhs, rounding),
        }
    }

    /// Subtracts one binary32 value from another.
    pub fn sub_f32(&self, lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::sub_f32(lhs, rhs, rounding),
            Self::Native => native::sub_f32(lhs, rhs, rounding),
        }
    }

    /// Subtracts one binary64 value from another.
    pub fn sub_f64(&self, lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::sub_f64(lhs, rhs, rounding),
            Self::Native => native::sub_f64(lhs, rhs, rounding),
        }
    }

    /// Multiplies two binary32 values.
    pub fn mul_f32(&self, lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::mul_f32(lhs, rhs, rounding),
            Self::Native => native::mul_f32(lhs, rhs, rounding),
        }
    }

    /// Multiplies two binary64 values.
    pub fn mul_f64(&self, lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::mul_f64(lhs, rhs, rounding),
            Self::Native => native::mul_f64(lhs, rhs, rounding),
        }
    }

    /// Divides one binary32 value by another.
    pub fn div_f32(&self, lhs: Float32, rhs: Float32, rounding: RoundingMode) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::div_f32(lhs, rhs, rounding),
            Self::Native => native::div_f32(lhs, rhs, rounding),
        }
    }

    /// Divides one binary64 value by another.
    pub fn div_f64(&self, lhs: Float64, rhs: Float64, rounding: RoundingMode) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::div_f64(lhs, rhs, rounding),
            Self::Native => native::div_f64(lhs, rhs, rounding),
        }
    }

    /// Returns the arithmetic absolute value of a binary32 value.
    pub fn abs_f32(&self, value: Float32) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::abs_f32(value),
            Self::Native => native::abs_f32(value),
        }
    }

    /// Returns the arithmetic absolute value of a binary64 value.
    pub fn abs_f64(&self, value: Float64) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::abs_f64(value),
            Self::Native => native::abs_f64(value),
        }
    }

    /// Negates a binary32 value.
    pub fn neg_f32(&self, value: Float32) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::neg_f32(value),
            Self::Native => native::neg_f32(value),
        }
    }

    /// Negates a binary64 value.
    pub fn neg_f64(&self, value: Float64) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::neg_f64(value),
            Self::Native => native::neg_f64(value),
        }
    }

    /// Converts a binary32 value to binary64.
    pub fn convert_float32_to_float64(&self, value: Float32) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::convert_float32_to_float64(value),
            Self::Native => native::convert_float32_to_float64(value),
        }
    }

    /// Converts a binary64 value to binary32.
    pub fn convert_float64_to_float32(
        &self,
        value: Float64,
        rounding: RoundingMode,
    ) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::convert_float64_to_float32(value, rounding),
            Self::Native => native::convert_float64_to_float32(value, rounding),
        }
    }

    /// Converts a signed 32-bit integer to binary32.
    pub fn convert_i32_to_float32(&self, value: i32, rounding: RoundingMode) -> Outcome<Float32> {
        match self {
            Self::SoftFloat => softfloat::convert_i32_to_float32(value, rounding),
            Self::Native => native::convert_i32_to_float32(value, rounding),
        }
    }

    /// Converts a signed 32-bit integer to binary64.
    pub fn convert_i32_to_float64(&self, value: i32) -> Outcome<Float64> {
        match self {
            Self::SoftFloat => softfloat::convert_i32_to_float64(value),
            Self::Native => native::convert_i32_to_float64(value),
        }
    }

    /// Converts a binary32 value to a signed 32-bit integer.
    pub fn convert_float32_to_i32(&self, value: Float32, rounding: RoundingMode) -> Outcome<i32> {
        match self {
            Self::SoftFloat => softfloat::convert_float32_to_i32(value, rounding),
            Self::Native => native::convert_float32_to_i32(value, rounding),
        }
    }

    /// Converts a binary64 value to a signed 32-bit integer.
    pub fn convert_float64_to_i32(&self, value: Float64, rounding: RoundingMode) -> Outcome<i32> {
        match self {
            Self::SoftFloat => softfloat::convert_float64_to_i32(value, rounding),
            Self::Native => native::convert_float64_to_i32(value, rounding),
        }
    }

    /// Compares two binary32 values.
    pub fn compare_f32(
        &self,
        lhs: Float32,
        rhs: Float32,
        mode: ComparisonMode,
    ) -> Outcome<Relation> {
        match self {
            Self::SoftFloat => softfloat::compare_f32(lhs, rhs, mode),
            Self::Native => native::compare_f32(lhs, rhs, mode),
        }
    }

    /// Compares two binary64 values.
    pub fn compare_f64(
        &self,
        lhs: Float64,
        rhs: Float64,
        mode: ComparisonMode,
    ) -> Outcome<Relation> {
        match self {
            Self::SoftFloat => softfloat::compare_f64(lhs, rhs, mode),
            Self::Native => native::compare_f64(lhs, rhs, mode),
        }
    }
}
