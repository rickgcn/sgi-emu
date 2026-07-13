//! Floating-point operation control and IEEE exception flags.
//!
//! Backends receive explicit control for every operation. Instruction-set
//! layers can pass the active control/status register rounding mode or select a
//! fixed mode for instructions whose rounding behavior is encoded directly in
//! the instruction.

use core::ops::{BitOr, BitOrAssign};

/// IEEE-754 rounding mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FloatRoundingMode {
    /// Round to nearest representable value, choosing an even low bit on ties.
    #[default]
    NearestEven,

    /// Round toward zero.
    TowardZero,

    /// Round toward positive infinity.
    TowardPositive,

    /// Round toward negative infinity.
    TowardNegative,
}

/// IEEE-754 tininess detection timing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FloatTininessMode {
    /// Detect underflow before rounding.
    BeforeRounding,

    /// Detect underflow after rounding.
    #[default]
    AfterRounding,
}

/// Per-operation floating-point control.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FloatControl {
    /// Rounding mode used by the operation.
    pub rounding_mode: FloatRoundingMode,

    /// Tininess detection timing used by the operation.
    pub tininess_mode: FloatTininessMode,
}

impl FloatControl {
    /// Creates per-operation floating-point control.
    pub const fn new(rounding_mode: FloatRoundingMode, tininess_mode: FloatTininessMode) -> Self {
        Self {
            rounding_mode,
            tininess_mode,
        }
    }

    /// Creates control with a specific rounding mode and default tininess.
    pub const fn with_rounding_mode(rounding_mode: FloatRoundingMode) -> Self {
        Self {
            rounding_mode,
            tininess_mode: FloatTininessMode::AfterRounding,
        }
    }
}

/// IEEE-754 exception flags.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FloatExceptionFlags(u8);

impl FloatExceptionFlags {
    /// Invalid operation.
    pub const INVALID: Self = Self(0x10);

    /// Divide by zero.
    pub const DIVIDE_BY_ZERO: Self = Self(0x08);

    /// Overflow.
    pub const OVERFLOW: Self = Self(0x04);

    /// Underflow.
    pub const UNDERFLOW: Self = Self(0x02);

    /// Inexact result.
    pub const INEXACT: Self = Self(0x01);

    /// Creates empty exception flags.
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Creates flags from known bits, discarding unknown bits.
    pub const fn from_bits_truncate(bits: u8) -> Self {
        Self(bits & 0x1f)
    }

    /// Returns the raw flag bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns whether no flags are set.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns whether all `flags` are present.
    pub const fn contains(self, flags: Self) -> bool {
        (self.0 & flags.0) == flags.0
    }

    /// Returns the union of two flag sets.
    pub const fn union(self, flags: Self) -> Self {
        Self(self.0 | flags.0)
    }
}

impl BitOr for FloatExceptionFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

impl BitOrAssign for FloatExceptionFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        *self = self.union(rhs);
    }
}

#[cfg(test)]
mod tests;
