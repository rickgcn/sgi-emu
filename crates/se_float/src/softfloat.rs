//! Owns the private Rust boundary to the SoftFloat C transaction layer.
//!
//! C-backed calls use fixed-width integer values and tagged `u8` control and
//! result fields. Each call resets rounding mode, tininess detection, flags,
//! and round-pack observations before executing one primitive. This module
//! validates every tagged result before constructing public Rust values.

use crate::SoftFloatBackend;
use crate::bits::{is_nonzero_subnormal_f32, is_nonzero_subnormal_f64};
use crate::env::{ExceptionFlags, Outcome, Relation, RoundingFacts, RoundingMode};

#[derive(Clone, Copy, Debug)]
struct RawOutcome<T> {
    value: T,
    flags: u8,
    precision_inexact: u8,
}

mod ffi {
    //! Declares and contains the raw fixed-width C ABI.
    //!
    //! Binary32 and binary64 values cross as `u32` and `u64`. Rounding tags
    //! use `0/1/2/3` for nearest-even, toward-zero, toward-positive, and
    //! toward-negative; relation tags use `0/1/2/3` for less, equal, greater,
    //! and unordered. Every output pointer is non-null, addresses one distinct
    //! writable byte for the duration of a call, and is not retained. The C
    //! transaction writes flags in bits `0..=4` and a precision-inexact tag of
    //! `0` or `1`; contract failures use encodings that the parent module
    //! rejects.

    use super::RawOutcome;

    unsafe extern "C" {
        fn se_float_shim_add_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_sub_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_mul_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_div_f32(
            a: u32,
            b: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_sqrt_f32(
            value: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_compare_f32(
            a: u32,
            b: u32,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u8;
        fn se_float_shim_add_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_sub_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_mul_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_div_f64(
            a: u64,
            b: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_sqrt_f64(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_compare_f64(
            a: u64,
            b: u64,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u8;
        fn se_float_shim_f32_to_f64(
            value: u32,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_f64_to_f32(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_i32_to_f32(
            value: i32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_i64_to_f32(
            value: i64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u32;
        fn se_float_shim_i32_to_f64(
            value: i32,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_i64_to_f64(
            value: i64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> u64;
        fn se_float_shim_f32_to_i32(
            value: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i32;
        fn se_float_shim_f32_to_i64(
            value: u32,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i64;
        fn se_float_shim_f64_to_i32(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i32;
        fn se_float_shim_f64_to_i64(
            value: u64,
            rounding: u8,
            out_flags: *mut u8,
            out_precision_inexact: *mut u8,
        ) -> i64;
    }

    fn capture<T>(invoke: impl FnOnce(*mut u8, *mut u8) -> T) -> RawOutcome<T> {
        let mut flags = 0;
        let mut precision_inexact = 0;
        let value = invoke(&mut flags, &mut precision_inexact);
        RawOutcome {
            value,
            flags,
            precision_inexact,
        }
    }

    pub(super) fn add_f32(a: u32, b: u32, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_add_f32(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn sub_f32(a: u32, b: u32, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_sub_f32(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn mul_f32(a: u32, b: u32, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_mul_f32(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn div_f32(a: u32, b: u32, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_div_f32(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn sqrt_f32(value: u32, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_sqrt_f32(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn compare_f32(a: u32, b: u32) -> RawOutcome<u8> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and both
            // operands are fixed-width binary32 bit patterns.
            unsafe { se_float_shim_compare_f32(a, b, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn add_f64(a: u64, b: u64, rounding: u8) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_add_f64(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn sub_f64(a: u64, b: u64, rounding: u8) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_sub_f64(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn mul_f64(a: u64, b: u64, rounding: u8) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_mul_f64(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn div_f64(a: u64, b: u64, rounding: u8) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_div_f64(a, b, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn sqrt_f64(value: u64, rounding: u8) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_sqrt_f64(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn compare_f64(a: u64, b: u64) -> RawOutcome<u8> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and both
            // operands are fixed-width binary64 bit patterns.
            unsafe { se_float_shim_compare_f64(a, b, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn f32_to_f64(value: u32) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // operand is a fixed-width binary32 bit pattern.
            unsafe { se_float_shim_f32_to_f64(value, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn f64_to_f32(value: u64, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_f64_to_f32(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn i32_to_f32(value: i32, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_i32_to_f32(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn i64_to_f32(value: i64, rounding: u8) -> RawOutcome<u32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_i64_to_f32(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn i32_to_f64(value: i32) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // operand exactly matches the fixed-width shim declaration.
            unsafe { se_float_shim_i32_to_f64(value, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn i64_to_f64(value: i64, rounding: u8) -> RawOutcome<u64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_i64_to_f64(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn f32_to_i32(value: u32, rounding: u8) -> RawOutcome<i32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_f32_to_i32(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn f32_to_i64(value: u32, rounding: u8) -> RawOutcome<i64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_f32_to_i64(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn f64_to_i32(value: u64, rounding: u8) -> RawOutcome<i32> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_f64_to_i32(value, rounding, out_flags, out_precision_inexact) }
        })
    }

    pub(super) fn f64_to_i64(value: u64, rounding: u8) -> RawOutcome<i64> {
        capture(|out_flags, out_precision_inexact| {
            // SAFETY: `capture` supplies distinct valid byte pointers, and the
            // scalar arguments exactly match the fixed-width shim declaration.
            unsafe { se_float_shim_f64_to_i64(value, rounding, out_flags, out_precision_inexact) }
        })
    }
}

fn encode_rounding(rounding: RoundingMode) -> u8 {
    match rounding {
        RoundingMode::NearestEven => 0,
        RoundingMode::TowardZero => 1,
        RoundingMode::TowardPositive => 2,
        RoundingMode::TowardNegative => 3,
    }
}

fn decode_flags(raw: u8) -> ExceptionFlags {
    ExceptionFlags::from_bits(raw)
        .unwrap_or_else(|| panic!("invalid SoftFloat flag byte {raw:#04x}"))
}

fn decode_fact(raw: u8) -> bool {
    match raw {
        0 => false,
        1 => true,
        _ => panic!("invalid SoftFloat rounding-fact byte {raw:#04x}"),
    }
}

fn decode_relation(raw: u8) -> Relation {
    match raw {
        0 => Relation::Less,
        1 => Relation::Equal,
        2 => Relation::Greater,
        3 => Relation::Unordered,
        _ => panic!("invalid SoftFloat relation byte {raw:#04x}"),
    }
}

fn outcome_f32(raw: RawOutcome<u32>) -> Outcome<u32> {
    let flags = decode_flags(raw.flags);
    let precision_inexact = decode_fact(raw.precision_inexact);
    let nonzero_subnormal = is_nonzero_subnormal_f32(raw.value);
    validate_float_flags(flags, nonzero_subnormal);
    Outcome::new(
        raw.value,
        flags,
        RoundingFacts {
            tiny_after_rounding: flags.contains(ExceptionFlags::UNDERFLOW)
                || (nonzero_subnormal && !flags.contains(ExceptionFlags::INEXACT)),
            precision_inexact,
        },
    )
}

fn outcome_f64(raw: RawOutcome<u64>) -> Outcome<u64> {
    let flags = decode_flags(raw.flags);
    let precision_inexact = decode_fact(raw.precision_inexact);
    let nonzero_subnormal = is_nonzero_subnormal_f64(raw.value);
    validate_float_flags(flags, nonzero_subnormal);
    Outcome::new(
        raw.value,
        flags,
        RoundingFacts {
            tiny_after_rounding: flags.contains(ExceptionFlags::UNDERFLOW)
                || (nonzero_subnormal && !flags.contains(ExceptionFlags::INEXACT)),
            precision_inexact,
        },
    )
}

fn outcome_relation(raw: RawOutcome<u8>) -> Outcome<Relation> {
    let flags = decode_flags(raw.flags);
    let precision_inexact = decode_fact(raw.precision_inexact);
    assert_eq!(
        flags.bits() & !ExceptionFlags::INVALID.bits(),
        0,
        "SoftFloat comparison returned an impossible exception flag"
    );
    assert!(
        !precision_inexact,
        "SoftFloat comparison returned a precision-inexact fact"
    );
    Outcome::new(decode_relation(raw.value), flags, RoundingFacts::default())
}

fn outcome_integer<T>(raw: RawOutcome<T>) -> Outcome<Option<T>> {
    let flags = decode_flags(raw.flags);
    let precision_inexact = decode_fact(raw.precision_inexact);
    let allowed = (ExceptionFlags::INEXACT | ExceptionFlags::INVALID).bits();
    assert_eq!(
        flags.bits() & !allowed,
        0,
        "SoftFloat integer conversion returned an impossible exception flag"
    );
    let invalid = flags.contains(ExceptionFlags::INVALID);
    assert!(
        !invalid || !flags.contains(ExceptionFlags::INEXACT),
        "SoftFloat invalid integer conversion also reported inexact"
    );
    assert!(
        !invalid || !precision_inexact,
        "SoftFloat invalid integer conversion reported discarded precision"
    );
    let value = if invalid { None } else { Some(raw.value) };
    Outcome::new_optional(
        value,
        flags,
        RoundingFacts {
            tiny_after_rounding: false,
            precision_inexact,
        },
    )
}

fn validate_float_flags(flags: ExceptionFlags, nonzero_subnormal: bool) {
    assert!(
        !flags.contains(ExceptionFlags::UNDERFLOW) || flags.contains(ExceptionFlags::INEXACT),
        "SoftFloat underflow did not include inexact"
    );
    assert!(
        !nonzero_subnormal
            || !flags.contains(ExceptionFlags::INEXACT)
            || flags.contains(ExceptionFlags::UNDERFLOW),
        "SoftFloat inexact subnormal did not include underflow"
    );
}

impl SoftFloatBackend {
    /// Adds two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary32 result and may report `INVALID`, `OVERFLOW`,
    /// or `INEXACT`. Its rounding facts record post-precision tininess and
    /// discarded precision. Standard quiet NaN operands do not themselves
    /// report [`ExceptionFlags::INVALID`];
    /// signaling NaNs do, and every NaN result is canonical `0x7FC00000`.
    /// Guest status, trap, payload, and denormal policies are not applied.
    #[must_use]
    pub fn add_f32(&self, a: u32, b: u32, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::add_f32(a, b, encode_rounding(rounding)))
    }

    /// Subtracts two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary32 result and may report `INVALID`, `OVERFLOW`,
    /// or `INEXACT`. Its rounding facts record post-precision tininess and
    /// discarded precision. Standard quiet NaN operands do not themselves
    /// report [`ExceptionFlags::INVALID`];
    /// signaling NaNs do, and every NaN result is canonical `0x7FC00000`.
    /// Guest status, trap, payload, and denormal policies are not applied.
    #[must_use]
    pub fn sub_f32(&self, a: u32, b: u32, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::sub_f32(a, b, encode_rounding(rounding)))
    }

    /// Multiplies two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary32 result and may report `INVALID`, `OVERFLOW`,
    /// `UNDERFLOW`, or `INEXACT`. Its rounding facts record post-precision
    /// tininess and discarded precision. Standard quiet NaN operands do not
    /// themselves report [`ExceptionFlags::INVALID`];
    /// signaling NaNs do, and every NaN result is canonical `0x7FC00000`.
    /// Guest status, trap, payload, and denormal policies are not applied.
    #[must_use]
    pub fn mul_f32(&self, a: u32, b: u32, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::mul_f32(a, b, encode_rounding(rounding)))
    }

    /// Divides two IEEE binary32 values supplied as raw `u32` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary32 result and may report `INVALID`,
    /// [`ExceptionFlags::DIVIDE_BY_ZERO`], `OVERFLOW`, `UNDERFLOW`, or
    /// `INEXACT`. Its rounding facts record post-precision tininess and
    /// discarded precision. Standard quiet NaN operands do not themselves
    /// report invalid; signaling NaNs do, and
    /// every NaN result is canonical `0x7FC00000`. Guest exception policy is
    /// not applied.
    #[must_use]
    pub fn div_f32(&self, a: u32, b: u32, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::div_f32(a, b, encode_rounding(rounding)))
    }

    /// Computes the square root of an IEEE binary32 raw `u32` bit pattern.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary32 result and may report `INVALID` or `INEXACT`;
    /// its rounding facts record tininess and discarded precision. A standard
    /// quiet NaN does not itself report invalid; a signaling NaN or a negative
    /// nonzero finite operand reports
    /// [`ExceptionFlags::INVALID`]. NaN results are canonical `0x7FC00000`.
    /// Guest exception and result policies are not applied.
    #[must_use]
    pub fn sqrt_f32(&self, value: u32, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::sqrt_f32(value, encode_rounding(rounding)))
    }

    /// Adds two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary64 result and may report `INVALID`, `OVERFLOW`,
    /// or `INEXACT`. Its rounding facts record post-precision tininess and
    /// discarded precision. Standard quiet NaN operands do not themselves
    /// report [`ExceptionFlags::INVALID`];
    /// signaling NaNs do, and every NaN result is canonical
    /// `0x7FF8000000000000`. Guest policies are not applied.
    #[must_use]
    pub fn add_f64(&self, a: u64, b: u64, rounding: RoundingMode) -> Outcome<u64> {
        outcome_f64(ffi::add_f64(a, b, encode_rounding(rounding)))
    }

    /// Subtracts two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary64 result and may report `INVALID`, `OVERFLOW`,
    /// or `INEXACT`. Its rounding facts record post-precision tininess and
    /// discarded precision. Standard quiet NaN operands do not themselves
    /// report [`ExceptionFlags::INVALID`];
    /// signaling NaNs do, and every NaN result is canonical
    /// `0x7FF8000000000000`. Guest policies are not applied.
    #[must_use]
    pub fn sub_f64(&self, a: u64, b: u64, rounding: RoundingMode) -> Outcome<u64> {
        outcome_f64(ffi::sub_f64(a, b, encode_rounding(rounding)))
    }

    /// Multiplies two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary64 result and may report `INVALID`, `OVERFLOW`,
    /// `UNDERFLOW`, or `INEXACT`. Its rounding facts record post-precision
    /// tininess and discarded precision. Standard quiet NaN operands do not
    /// themselves report [`ExceptionFlags::INVALID`];
    /// signaling NaNs do, and every NaN result is canonical
    /// `0x7FF8000000000000`. Guest policies are not applied.
    #[must_use]
    pub fn mul_f64(&self, a: u64, b: u64, rounding: RoundingMode) -> Outcome<u64> {
        outcome_f64(ffi::mul_f64(a, b, encode_rounding(rounding)))
    }

    /// Divides two IEEE binary64 values supplied as raw `u64` bit patterns.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary64 result and may report `INVALID`,
    /// [`ExceptionFlags::DIVIDE_BY_ZERO`], `OVERFLOW`, `UNDERFLOW`, or
    /// `INEXACT`. Its rounding facts record post-precision tininess and
    /// discarded precision. Standard quiet NaN operands do not themselves
    /// report invalid; signaling NaNs do, and
    /// every NaN result is canonical `0x7FF8000000000000`. Guest exception
    /// policy is not applied.
    #[must_use]
    pub fn div_f64(&self, a: u64, b: u64, rounding: RoundingMode) -> Outcome<u64> {
        outcome_f64(ffi::div_f64(a, b, encode_rounding(rounding)))
    }

    /// Computes the square root of an IEEE binary64 raw `u64` bit pattern.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains the raw binary64 result and may report `INVALID` or `INEXACT`;
    /// its rounding facts record tininess and discarded precision. A standard
    /// quiet NaN does not itself report invalid; a signaling NaN or a negative
    /// nonzero finite operand reports
    /// [`ExceptionFlags::INVALID`]. NaN results are canonical
    /// `0x7FF8000000000000`. Guest exception and result policies are not
    /// applied.
    #[must_use]
    pub fn sqrt_f64(&self, value: u64, rounding: RoundingMode) -> Outcome<u64> {
        outcome_f64(ffi::sqrt_f64(value, encode_rounding(rounding)))
    }

    /// Quietly compares two IEEE binary32 raw `u32` bit patterns.
    ///
    /// The returned [`Relation`] preserves numeric ordering and treats both
    /// zero signs as equal. Any NaN produces [`Relation::Unordered`]; a standard
    /// quiet NaN does not report invalid, while a signaling NaN reports
    /// [`ExceptionFlags::INVALID`]. Both rounding facts are always `false`.
    /// No guest condition encoding, status update, or trap policy is applied.
    #[must_use]
    pub fn compare_f32(&self, a: u32, b: u32) -> Outcome<Relation> {
        outcome_relation(ffi::compare_f32(a, b))
    }

    /// Quietly compares two IEEE binary64 raw `u64` bit patterns.
    ///
    /// The returned [`Relation`] preserves numeric ordering and treats both
    /// zero signs as equal. Any NaN produces [`Relation::Unordered`]; a standard
    /// quiet NaN does not report invalid, while a signaling NaN reports
    /// [`ExceptionFlags::INVALID`]. Both rounding facts are always `false`.
    /// No guest condition encoding, status update, or trap policy is applied.
    #[must_use]
    pub fn compare_f64(&self, a: u64, b: u64) -> Outcome<Relation> {
        outcome_relation(ffi::compare_f64(a, b))
    }

    /// Widens an IEEE binary32 raw `u32` bit pattern to binary64.
    ///
    /// Finite values widen exactly, so no rounding mode is accepted and both
    /// rounding facts are `false`. The returned [`Outcome`] contains a raw
    /// binary64 `u64` result. A standard quiet NaN does not report invalid; a
    /// signaling NaN reports [`ExceptionFlags::INVALID`], and either produces
    /// canonical `0x7FF8000000000000`. Guest NaN and exception policies are
    /// not applied.
    #[must_use]
    pub fn f32_to_f64(&self, value: u32) -> Outcome<u64> {
        outcome_f64(ffi::f32_to_f64(value))
    }

    /// Narrows an IEEE binary64 raw `u64` bit pattern to binary32.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// contains a raw binary32 `u32` result and may report `INVALID`,
    /// `OVERFLOW`, `UNDERFLOW`, or `INEXACT`. Its rounding facts record
    /// post-precision tininess and discarded precision. A standard quiet
    /// NaN does not report invalid; a signaling NaN does, and all NaN results
    /// are canonical `0x7FC00000`. Guest result and exception policies are not
    /// applied.
    #[must_use]
    pub fn f64_to_f32(&self, value: u64, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::f64_to_f32(value, encode_rounding(rounding)))
    }

    /// Converts an `i32` value to an IEEE binary32 raw `u32` bit pattern.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// reports [`ExceptionFlags::INEXACT`] and discarded precision when the
    /// integer is not exactly representable; post-rounding tininess is always
    /// `false`. No guest status or exception policy is applied.
    #[must_use]
    pub fn i32_to_f32(&self, value: i32, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::i32_to_f32(value, encode_rounding(rounding)))
    }

    /// Converts an `i64` value to an IEEE binary32 raw `u32` bit pattern.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// reports [`ExceptionFlags::INEXACT`] and discarded precision when the
    /// integer is not exactly representable; post-rounding tininess is always
    /// `false`. No guest status or exception policy is applied.
    #[must_use]
    pub fn i64_to_f32(&self, value: i64, rounding: RoundingMode) -> Outcome<u32> {
        outcome_f32(ffi::i64_to_f32(value, encode_rounding(rounding)))
    }

    /// Converts an `i32` value exactly to an IEEE binary64 raw `u64` bit pattern.
    ///
    /// No rounding mode is accepted because every `i32` is representable. The
    /// returned [`Outcome`] has empty flags and both rounding facts are
    /// `false`. No guest status or exception policy is applied.
    #[must_use]
    pub fn i32_to_f64(&self, value: i32) -> Outcome<u64> {
        outcome_f64(ffi::i32_to_f64(value))
    }

    /// Converts an `i64` value to an IEEE binary64 raw `u64` bit pattern.
    ///
    /// `rounding` selects the rounding direction. The returned [`Outcome`]
    /// reports [`ExceptionFlags::INEXACT`] and discarded precision when the
    /// integer is not exactly representable; post-rounding tininess is always
    /// `false`. No guest status or exception policy is applied.
    #[must_use]
    pub fn i64_to_f64(&self, value: i64, rounding: RoundingMode) -> Outcome<u64> {
        outcome_f64(ffi::i64_to_f64(value, encode_rounding(rounding)))
    }

    /// Converts an IEEE binary32 raw `u32` bit pattern to `i32`.
    ///
    /// `rounding` selects the integer rounding direction. A finite in-range
    /// conversion returns `Some` and reports [`ExceptionFlags::INEXACT`] when
    /// fractional information is discarded. Any NaN, infinity, or out-of-range
    /// result returns `None` with only [`ExceptionFlags::INVALID`]; the C
    /// sentinel is ignored. Tininess is always `false`, and no guest exception
    /// or integer-result policy is applied.
    #[must_use]
    pub fn f32_to_i32(&self, value: u32, rounding: RoundingMode) -> Outcome<Option<i32>> {
        outcome_integer(ffi::f32_to_i32(value, encode_rounding(rounding)))
    }

    /// Converts an IEEE binary32 raw `u32` bit pattern to `i64`.
    ///
    /// `rounding` selects the integer rounding direction. A finite in-range
    /// conversion returns `Some` and reports [`ExceptionFlags::INEXACT`] when
    /// fractional information is discarded. Any NaN, infinity, or out-of-range
    /// result returns `None` with only [`ExceptionFlags::INVALID`]; the C
    /// sentinel is ignored. Tininess is always `false`, and no guest exception
    /// or integer-result policy is applied.
    #[must_use]
    pub fn f32_to_i64(&self, value: u32, rounding: RoundingMode) -> Outcome<Option<i64>> {
        outcome_integer(ffi::f32_to_i64(value, encode_rounding(rounding)))
    }

    /// Converts an IEEE binary64 raw `u64` bit pattern to `i32`.
    ///
    /// `rounding` selects the integer rounding direction. A finite in-range
    /// conversion returns `Some` and reports [`ExceptionFlags::INEXACT`] when
    /// fractional information is discarded. Any NaN, infinity, or out-of-range
    /// result returns `None` with only [`ExceptionFlags::INVALID`]; the C
    /// sentinel is ignored. Tininess is always `false`, and no guest exception
    /// or integer-result policy is applied.
    #[must_use]
    pub fn f64_to_i32(&self, value: u64, rounding: RoundingMode) -> Outcome<Option<i32>> {
        outcome_integer(ffi::f64_to_i32(value, encode_rounding(rounding)))
    }

    /// Converts an IEEE binary64 raw `u64` bit pattern to `i64`.
    ///
    /// `rounding` selects the integer rounding direction. A finite in-range
    /// conversion returns `Some` and reports [`ExceptionFlags::INEXACT`] when
    /// fractional information is discarded. Any NaN, infinity, or out-of-range
    /// result returns `None` with only [`ExceptionFlags::INVALID`]; the C
    /// sentinel is ignored. Tininess is always `false`, and no guest exception
    /// or integer-result policy is applied.
    #[must_use]
    pub fn f64_to_i64(&self, value: u64, rounding: RoundingMode) -> Outcome<Option<i64>> {
        outcome_integer(ffi::f64_to_i64(value, encode_rounding(rounding)))
    }
}

#[cfg(test)]
mod tests {
    use std::panic;

    use super::{
        RawOutcome, decode_fact, decode_flags, decode_relation, encode_rounding, ffi, outcome_f32,
        outcome_f64, outcome_integer, outcome_relation,
    };
    use crate::env::{ExceptionFlags, Relation, RoundingMode};

    #[test]
    fn tagged_byte_codecs_are_exhaustive() {
        assert_eq!(encode_rounding(RoundingMode::NearestEven), 0);
        assert_eq!(encode_rounding(RoundingMode::TowardZero), 1);
        assert_eq!(encode_rounding(RoundingMode::TowardPositive), 2);
        assert_eq!(encode_rounding(RoundingMode::TowardNegative), 3);
        assert_eq!(decode_relation(0), Relation::Less);
        assert_eq!(decode_relation(1), Relation::Equal);
        assert_eq!(decode_relation(2), Relation::Greater);
        assert_eq!(decode_relation(3), Relation::Unordered);
        assert!(!decode_fact(0));
        assert!(decode_fact(1));
    }

    #[test]
    fn invalid_tagged_bytes_fail_immediately() {
        assert!(panic::catch_unwind(|| decode_flags(0x80)).is_err());
        assert!(panic::catch_unwind(|| decode_fact(2)).is_err());
        assert!(panic::catch_unwind(|| decode_relation(4)).is_err());

        let raw = ffi::add_f32(0x3f80_0000, 0x3f80_0000, 0xff);
        assert_eq!(raw.flags, 0x80);
        assert_eq!(raw.precision_inexact, 2);
        assert!(panic::catch_unwind(|| outcome_f32(raw)).is_err());
    }

    #[test]
    fn operation_specific_invalid_encodings_fail() {
        let bad_relation = RawOutcome {
            value: 4,
            flags: 0,
            precision_inexact: 0,
        };
        let bad_comparison_fact = RawOutcome {
            value: 0,
            flags: 0,
            precision_inexact: 1,
        };
        let bad_integer_flags = RawOutcome {
            value: 0_i32,
            flags: ExceptionFlags::INVALID.bits() | ExceptionFlags::INEXACT.bits(),
            precision_inexact: 1,
        };

        assert!(panic::catch_unwind(|| outcome_relation(bad_relation)).is_err());
        assert!(panic::catch_unwind(|| outcome_relation(bad_comparison_fact)).is_err());
        assert!(panic::catch_unwind(|| outcome_integer(bad_integer_flags)).is_err());
    }

    #[test]
    fn binary64_result_codec_recovers_exact_subnormal_tininess() {
        let outcome = outcome_f64(RawOutcome {
            value: 1,
            flags: 0,
            precision_inexact: 0,
        });

        assert!(outcome.rounding.tiny_after_rounding);
        assert!(!outcome.rounding.precision_inexact);
    }

    #[test]
    fn comparison_relation_uses_softfloat_transaction() {
        let outcome = outcome_relation(ffi::compare_f32(0x7f80_0001, 0x3f80_0000));

        assert_eq!(outcome.value, Relation::Unordered);
        assert_eq!(outcome.flags, ExceptionFlags::INVALID);
        assert!(!outcome.rounding.precision_inexact);
    }

    #[test]
    fn float_to_integer_shims_request_inexact_reporting() {
        let outcome = outcome_integer(ffi::f32_to_i32(0x3fc0_0000, 0));

        assert_eq!(outcome.value, Some(2));
        assert_eq!(outcome.flags, ExceptionFlags::INEXACT);
        assert!(outcome.rounding.precision_inexact);
    }
}

#[cfg(test)]
mod source_contract_tests {
    use std::fs;
    use std::path::Path;

    const PLATFORM_PRIMITIVES: &[&str] = &[
        "approxRecip32_1",
        "approxRecipSqrt32_1",
        "countLeadingZeros32",
        "countLeadingZeros64",
        "mul64To128",
        "shiftRightJam32",
        "shiftRightJam64",
        "shiftRightJam64Extra",
        "shortShiftRightJam64",
    ];

    fn manifest_entries(build_script: &str) -> Vec<&str> {
        let start = build_script
            .find("const UPSTREAM_TRANSLATION_UNITS")
            .expect("translation-unit manifest declaration");
        let body = &build_script[start..];
        let end = body.find("\n];").expect("translation-unit manifest end");
        body[..end]
            .lines()
            .filter_map(|line| {
                let line = line.trim();
                line.strip_prefix('"')
                    .and_then(|line| line.strip_suffix("\","))
            })
            .collect()
    }

    fn occurrences(haystack: &str, needle: &str) -> usize {
        haystack.match_indices(needle).count()
    }

    #[test]
    fn source_manifest_is_sorted_explicit_and_complete_on_disk() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let source_dir = crate_dir.join("../../3rdparty/softfloat3/source");
        let build_script = include_str!("../build.rs");
        let entries = manifest_entries(build_script);

        assert!(!entries.is_empty());
        assert!(entries.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(entries.iter().all(|path| source_dir.join(path).is_file()));
        assert!(!build_script.contains("read_dir"));
        assert!(!build_script.contains("walkdir"));
    }

    #[test]
    fn platform_and_round_pack_replacements_are_unique() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let build_script = include_str!("../build.rs");
        let entries = manifest_entries(build_script);
        let platform =
            fs::read_to_string(crate_dir.join("csrc/platform.h")).expect("read platform.h");
        let primitives =
            fs::read_to_string(crate_dir.join("csrc/primitives.c")).expect("read primitives.c");
        let rename = fs::read_to_string(crate_dir.join("csrc/rename.h")).expect("read rename.h");
        let round_pack =
            fs::read_to_string(crate_dir.join("csrc/round_pack.c")).expect("read round_pack.c");

        for primitive in PLATFORM_PRIMITIVES {
            let upstream_file = format!("s_{primitive}.c");
            assert!(!entries.contains(&upstream_file.as_str()));
            assert_eq!(
                occurrences(&primitives, &format!("se_float_sf_{primitive}(")),
                1
            );
            assert_eq!(
                occurrences(&primitives, &format!("source/{upstream_file}")),
                1
            );
            assert_eq!(
                occurrences(
                    &platform,
                    &format!("#define softfloat_{primitive} se_float_sf_{primitive}")
                ),
                1
            );
            assert!(!rename.contains(&format!("#define softfloat_{primitive} ")));
        }

        for round_pack_name in ["s_roundPackToF32.c", "s_roundPackToF64.c"] {
            assert!(!entries.contains(&round_pack_name));
            assert_eq!(occurrences(&round_pack, round_pack_name), 2);
            assert_eq!(
                occurrences(&round_pack, &format!("#include \"{round_pack_name}\"")),
                1
            );
        }
    }

    #[test]
    fn public_surface_keeps_implementations_private_and_concrete() {
        let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let lib = fs::read_to_string(crate_dir.join("src/lib.rs")).expect("read lib.rs");
        let softfloat =
            fs::read_to_string(crate_dir.join("src/softfloat.rs")).expect("read softfloat.rs");
        let public_use = ["pub ", "use"].concat();
        let public_trait = ["pub ", "trait"].concat();
        let public_extern = ["pub ", "extern"].concat();
        let public_unsafe = ["pub ", "unsafe"].concat();

        assert!(!lib.contains(&public_use));
        assert!(!lib.contains(&public_trait));
        assert!(!lib.contains("pub mod softfloat"));
        assert!(!softfloat.contains(&public_trait));
        assert!(!softfloat.contains(&public_extern));
        assert!(!softfloat.contains(&public_unsafe));
    }
}
