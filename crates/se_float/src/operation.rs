//! Rounding, comparison, exception, and result types.

use bitflags::bitflags;

/// A rounding mode supported by the MIPS I floating-point architecture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoundingMode {
    /// Round to the nearest representable value, choosing an even significand on ties.
    NearestEven,
    /// Round toward zero.
    TowardZero,
    /// Round toward positive infinity.
    TowardPositive,
    /// Round toward negative infinity.
    TowardNegative,
}

/// Controls whether a comparison signals invalid for quiet NaNs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComparisonMode {
    /// Signal invalid only when an operand is a signaling NaN.
    Quiet,
    /// Signal invalid when either operand is any NaN.
    Signaling,
}

/// The relation between two floating-point values.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Relation {
    /// The left operand is less than the right operand.
    Less,
    /// The operands compare equal.
    Equal,
    /// The left operand is greater than the right operand.
    Greater,
    /// At least one operand is a NaN.
    Unordered,
}

bitflags! {
    /// Floating-point exception conditions produced by one operation.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct ExceptionFlags: u8 {
        /// The rounded result is not exact.
        const INEXACT = 1 << 0;
        /// The result is tiny and inexact.
        const UNDERFLOW = 1 << 1;
        /// The rounded result exceeds the destination format.
        const OVERFLOW = 1 << 2;
        /// A finite nonzero value was divided by zero.
        const DIVIDE_BY_ZERO = 1 << 3;
        /// The operation or an operand is invalid.
        const INVALID = 1 << 4;
    }
}

/// The value and conditions produced by one floating-point operation.
#[must_use = "floating-point outcomes contain exception and tininess information"]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome<T> {
    /// The value produced by the operation.
    pub value: T,
    /// Exception conditions produced by this operation only.
    pub flags: ExceptionFlags,
    /// Whether the operation produced a result that is tiny after rounding.
    pub tiny: bool,
}
