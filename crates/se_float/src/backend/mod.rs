//! Floating-point backend abstraction.
//!
//! Backend operations receive raw bits and explicit operation control. They
//! return raw bits plus IEEE exception flags. CPU and ISA layers are
//! responsible for mapping those results into architectural state.

use crate::control::FloatControl;
use crate::result::FloatResult;
use crate::value::{Float32Bits, Float64Bits, FloatCompareMode, FloatRelation};

pub mod native;
pub mod softfloat3;

/// Floating-point arithmetic backend.
pub trait FloatBackend {
    /// Adds two single-precision values.
    fn add_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits>;

    /// Subtracts two single-precision values.
    fn sub_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits>;

    /// Multiplies two single-precision values.
    fn mul_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits>;

    /// Divides two single-precision values.
    fn div_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits>;

    /// Computes the square root of a single-precision value.
    fn sqrt_f32(&self, control: FloatControl, value: Float32Bits) -> FloatResult<Float32Bits>;

    /// Computes one rounded single-precision fused multiply-add operation.
    fn mul_add_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
        addend: Float32Bits,
    ) -> FloatResult<Float32Bits>;

    /// Adds two double-precision values.
    fn add_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits>;

    /// Subtracts two double-precision values.
    fn sub_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits>;

    /// Multiplies two double-precision values.
    fn mul_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits>;

    /// Divides two double-precision values.
    fn div_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits>;

    /// Computes the square root of a double-precision value.
    fn sqrt_f64(&self, control: FloatControl, value: Float64Bits) -> FloatResult<Float64Bits>;

    /// Computes one rounded double-precision fused multiply-add operation.
    fn mul_add_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
        addend: Float64Bits,
    ) -> FloatResult<Float64Bits>;

    /// Converts single precision to double precision.
    fn f32_to_f64(&self, control: FloatControl, value: Float32Bits) -> FloatResult<Float64Bits>;

    /// Converts double precision to single precision.
    fn f64_to_f32(&self, control: FloatControl, value: Float64Bits) -> FloatResult<Float32Bits>;

    /// Converts a signed 32-bit integer to single precision.
    fn i32_to_f32(&self, control: FloatControl, value: i32) -> FloatResult<Float32Bits>;

    /// Converts a signed 32-bit integer to double precision.
    fn i32_to_f64(&self, control: FloatControl, value: i32) -> FloatResult<Float64Bits>;

    /// Converts a signed 64-bit integer to single precision.
    fn i64_to_f32(&self, control: FloatControl, value: i64) -> FloatResult<Float32Bits>;

    /// Converts a signed 64-bit integer to double precision.
    fn i64_to_f64(&self, control: FloatControl, value: i64) -> FloatResult<Float64Bits>;

    /// Converts single precision to a signed 32-bit integer.
    fn f32_to_i32(&self, control: FloatControl, value: Float32Bits) -> FloatResult<i32>;

    /// Converts double precision to a signed 32-bit integer.
    fn f64_to_i32(&self, control: FloatControl, value: Float64Bits) -> FloatResult<i32>;

    /// Converts single precision to a signed 64-bit integer.
    fn f32_to_i64(&self, control: FloatControl, value: Float32Bits) -> FloatResult<i64>;

    /// Converts double precision to a signed 64-bit integer.
    fn f64_to_i64(&self, control: FloatControl, value: Float64Bits) -> FloatResult<i64>;

    /// Compares two single-precision values.
    fn compare_f32(
        &self,
        control: FloatControl,
        mode: FloatCompareMode,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<FloatRelation>;

    /// Compares two double-precision values.
    fn compare_f64(
        &self,
        control: FloatControl,
        mode: FloatCompareMode,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<FloatRelation>;
}
