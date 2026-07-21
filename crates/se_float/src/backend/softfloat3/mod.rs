//! Berkeley SoftFloat 3e architecture-specific backends.
//!
//! The backend calls the local SoftFloat source tree through a small fixed-width
//! C wrapper. Every operation sets the requested control state, clears the
//! SoftFloat exception state, executes the operation, and returns the generated
//! flags with the value.

pub mod mips4;

use std::sync::Mutex;

use self::mips4::Mips4SoftFloatBackend;
use crate::backend::FloatBackend;
use crate::control::{FloatControl, FloatExceptionFlags, FloatRoundingMode, FloatTininessMode};
use crate::result::FloatResult;
use crate::value::{Float32Bits, Float64Bits, FloatCompareMode, FloatRelation};

static SOFTFLOAT_LOCK: Mutex<()> = Mutex::new(());

unsafe extern "C" {
    fn se_softfloat_set_rounding_mode(value: u8);
    fn se_softfloat_set_tininess_mode(value: u8);
    fn se_softfloat_clear_exception_flags();
    fn se_softfloat_exception_flags() -> u8;

    fn se_softfloat_f32_add(lhs: u32, rhs: u32) -> u32;
    fn se_softfloat_f32_sub(lhs: u32, rhs: u32) -> u32;
    fn se_softfloat_f32_mul(lhs: u32, rhs: u32) -> u32;
    fn se_softfloat_f32_div(lhs: u32, rhs: u32) -> u32;
    fn se_softfloat_f32_sqrt(value: u32) -> u32;
    fn se_softfloat_f32_mul_add(lhs: u32, rhs: u32, addend: u32) -> u32;

    fn se_softfloat_f64_add(lhs: u64, rhs: u64) -> u64;
    fn se_softfloat_f64_sub(lhs: u64, rhs: u64) -> u64;
    fn se_softfloat_f64_mul(lhs: u64, rhs: u64) -> u64;
    fn se_softfloat_f64_div(lhs: u64, rhs: u64) -> u64;
    fn se_softfloat_f64_sqrt(value: u64) -> u64;
    fn se_softfloat_f64_mul_add(lhs: u64, rhs: u64, addend: u64) -> u64;

    fn se_softfloat_f32_to_f64(value: u32) -> u64;
    fn se_softfloat_f64_to_f32(value: u64) -> u32;
    fn se_softfloat_i32_to_f32(value: i32) -> u32;
    fn se_softfloat_i32_to_f64(value: i32) -> u64;
    fn se_softfloat_i64_to_f32(value: i64) -> u32;
    fn se_softfloat_i64_to_f64(value: i64) -> u64;
    fn se_softfloat_f32_to_i32(value: u32, rounding_mode: u8, exact: bool) -> i32;
    fn se_softfloat_f64_to_i32(value: u64, rounding_mode: u8, exact: bool) -> i32;
    fn se_softfloat_f32_to_i64(value: u32, rounding_mode: u8, exact: bool) -> i64;
    fn se_softfloat_f64_to_i64(value: u64, rounding_mode: u8, exact: bool) -> i64;

    fn se_softfloat_f32_eq(lhs: u32, rhs: u32) -> bool;
    fn se_softfloat_f32_eq_signaling(lhs: u32, rhs: u32) -> bool;
    fn se_softfloat_f32_lt(lhs: u32, rhs: u32) -> bool;

    fn se_softfloat_f64_eq(lhs: u64, rhs: u64) -> bool;
    fn se_softfloat_f64_eq_signaling(lhs: u64, rhs: u64) -> bool;
    fn se_softfloat_f64_lt(lhs: u64, rhs: u64) -> bool;
}

impl FloatBackend for Mips4SoftFloatBackend {
    fn add_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_f32_add(lhs.bits(), rhs.bits()))
        })
    }

    fn sub_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_f32_sub(lhs.bits(), rhs.bits()))
        })
    }

    fn mul_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_f32_mul(lhs.bits(), rhs.bits()))
        })
    }

    fn div_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_f32_div(lhs.bits(), rhs.bits()))
        })
    }

    fn sqrt_f32(&self, control: FloatControl, value: Float32Bits) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_f32_sqrt(value.bits()))
        })
    }

    fn mul_add_f32(
        &self,
        control: FloatControl,
        lhs: Float32Bits,
        rhs: Float32Bits,
        addend: Float32Bits,
    ) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_f32_mul_add(
                lhs.bits(),
                rhs.bits(),
                addend.bits(),
            ))
        })
    }

    fn add_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_f64_add(lhs.bits(), rhs.bits()))
        })
    }

    fn sub_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_f64_sub(lhs.bits(), rhs.bits()))
        })
    }

    fn mul_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_f64_mul(lhs.bits(), rhs.bits()))
        })
    }

    fn div_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_f64_div(lhs.bits(), rhs.bits()))
        })
    }

    fn sqrt_f64(&self, control: FloatControl, value: Float64Bits) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_f64_sqrt(value.bits()))
        })
    }

    fn mul_add_f64(
        &self,
        control: FloatControl,
        lhs: Float64Bits,
        rhs: Float64Bits,
        addend: Float64Bits,
    ) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_f64_mul_add(
                lhs.bits(),
                rhs.bits(),
                addend.bits(),
            ))
        })
    }

    fn f32_to_f64(&self, control: FloatControl, value: Float32Bits) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_f32_to_f64(value.bits()))
        })
    }

    fn f64_to_f32(&self, control: FloatControl, value: Float64Bits) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_f64_to_f32(value.bits()))
        })
    }

    fn i32_to_f32(&self, control: FloatControl, value: i32) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_i32_to_f32(value))
        })
    }

    fn i32_to_f64(&self, control: FloatControl, value: i32) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_i32_to_f64(value))
        })
    }

    fn i64_to_f32(&self, control: FloatControl, value: i64) -> FloatResult<Float32Bits> {
        run(control, || unsafe {
            Float32Bits::new(se_softfloat_i64_to_f32(value))
        })
    }

    fn i64_to_f64(&self, control: FloatControl, value: i64) -> FloatResult<Float64Bits> {
        run(control, || unsafe {
            Float64Bits::new(se_softfloat_i64_to_f64(value))
        })
    }

    fn f32_to_i32(&self, control: FloatControl, value: Float32Bits) -> FloatResult<i32> {
        let rounding_mode = softfloat_rounding_mode(control.rounding_mode);
        run(control, || unsafe {
            se_softfloat_f32_to_i32(value.bits(), rounding_mode, true)
        })
    }

    fn f64_to_i32(&self, control: FloatControl, value: Float64Bits) -> FloatResult<i32> {
        let rounding_mode = softfloat_rounding_mode(control.rounding_mode);
        run(control, || unsafe {
            se_softfloat_f64_to_i32(value.bits(), rounding_mode, true)
        })
    }

    fn f32_to_i64(&self, control: FloatControl, value: Float32Bits) -> FloatResult<i64> {
        let rounding_mode = softfloat_rounding_mode(control.rounding_mode);
        run(control, || unsafe {
            se_softfloat_f32_to_i64(value.bits(), rounding_mode, true)
        })
    }

    fn f64_to_i64(&self, control: FloatControl, value: Float64Bits) -> FloatResult<i64> {
        let rounding_mode = softfloat_rounding_mode(control.rounding_mode);
        run(control, || unsafe {
            se_softfloat_f64_to_i64(value.bits(), rounding_mode, true)
        })
    }

    fn compare_f32(
        &self,
        control: FloatControl,
        mode: FloatCompareMode,
        lhs: Float32Bits,
        rhs: Float32Bits,
    ) -> FloatResult<FloatRelation> {
        run(control, || compare_f32(mode, lhs, rhs))
    }

    fn compare_f64(
        &self,
        control: FloatControl,
        mode: FloatCompareMode,
        lhs: Float64Bits,
        rhs: Float64Bits,
    ) -> FloatResult<FloatRelation> {
        run(control, || compare_f64(mode, lhs, rhs))
    }
}

fn run<T>(control: FloatControl, operation: impl FnOnce() -> T) -> FloatResult<T> {
    let _guard = SOFTFLOAT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    unsafe {
        se_softfloat_set_rounding_mode(softfloat_rounding_mode(control.rounding_mode));
        se_softfloat_set_tininess_mode(softfloat_tininess_mode(control.tininess_mode));
        se_softfloat_clear_exception_flags();
    }

    let value = operation();
    let flags = unsafe { FloatExceptionFlags::from_bits_truncate(se_softfloat_exception_flags()) };

    FloatResult::new(value, flags)
}

fn compare_f32(mode: FloatCompareMode, lhs: Float32Bits, rhs: Float32Bits) -> FloatRelation {
    if lhs.is_nan() || rhs.is_nan() {
        unsafe {
            match mode {
                FloatCompareMode::Quiet => {
                    se_softfloat_f32_eq(lhs.bits(), rhs.bits());
                }
                FloatCompareMode::Signaling => {
                    se_softfloat_f32_eq_signaling(lhs.bits(), rhs.bits());
                }
            }
        }
        FloatRelation::Unordered
    } else if unsafe { se_softfloat_f32_lt(lhs.bits(), rhs.bits()) } {
        FloatRelation::Less
    } else if unsafe { se_softfloat_f32_eq(lhs.bits(), rhs.bits()) } {
        FloatRelation::Equal
    } else {
        FloatRelation::Greater
    }
}

fn compare_f64(mode: FloatCompareMode, lhs: Float64Bits, rhs: Float64Bits) -> FloatRelation {
    if lhs.is_nan() || rhs.is_nan() {
        unsafe {
            match mode {
                FloatCompareMode::Quiet => {
                    se_softfloat_f64_eq(lhs.bits(), rhs.bits());
                }
                FloatCompareMode::Signaling => {
                    se_softfloat_f64_eq_signaling(lhs.bits(), rhs.bits());
                }
            }
        }
        FloatRelation::Unordered
    } else if unsafe { se_softfloat_f64_lt(lhs.bits(), rhs.bits()) } {
        FloatRelation::Less
    } else if unsafe { se_softfloat_f64_eq(lhs.bits(), rhs.bits()) } {
        FloatRelation::Equal
    } else {
        FloatRelation::Greater
    }
}

fn softfloat_rounding_mode(rounding_mode: FloatRoundingMode) -> u8 {
    match rounding_mode {
        FloatRoundingMode::NearestEven => 0,
        FloatRoundingMode::TowardZero => 1,
        FloatRoundingMode::TowardNegative => 2,
        FloatRoundingMode::TowardPositive => 3,
    }
}

fn softfloat_tininess_mode(tininess_mode: FloatTininessMode) -> u8 {
    match tininess_mode {
        FloatTininessMode::BeforeRounding => 0,
        FloatTininessMode::AfterRounding => 1,
    }
}

#[cfg(test)]
mod tests;
