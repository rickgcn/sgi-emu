//! Defines ISA-independent floating-point controls and operation results.
//!
//! Exception flags describe one non-trapping IEEE operation and never
//! accumulate here. [`RoundingFacts`] preserves facts that cannot be recovered
//! from those flags alone; it does not represent guest status or trap policy.

use core::ops::{BitOr, BitOrAssign};

/// Selects the rounding direction for one deterministic operation.
///
/// Variant discriminants are not an ABI and do not match guest or SoftFloat
/// encodings. Every boundary maps variants explicitly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RoundingMode {
    /// Rounds to the nearest representable value, choosing an even low bit on a tie.
    NearestEven,
    /// Rounds toward zero.
    TowardZero,
    /// Rounds toward positive infinity.
    TowardPositive,
    /// Rounds toward negative infinity.
    TowardNegative,
}

/// Contains the non-trapping IEEE exception flags raised by one operation.
///
/// Only the five declared constants are valid. Values from foreign code are
/// checked before this type is constructed, so unknown bits are treated as an
/// internal contract violation instead of being discarded.
#[derive(Clone, Copy, Default, Eq, PartialEq)]
pub struct ExceptionFlags(u8);

impl ExceptionFlags {
    /// Indicates that target-precision rounding discarded nonzero information.
    pub const INEXACT: Self = Self(1 << 0);
    /// Indicates a tiny and inexact result under after-rounding tininess detection.
    pub const UNDERFLOW: Self = Self(1 << 1);
    /// Indicates that a finite result exceeded the destination exponent range.
    pub const OVERFLOW: Self = Self(1 << 2);
    /// Indicates finite nonzero division by zero.
    pub const DIVIDE_BY_ZERO: Self = Self(1 << 3);
    /// Indicates an invalid IEEE operation.
    pub const INVALID: Self = Self(1 << 4);

    const KNOWN_BITS: u8 = Self::INEXACT.0
        | Self::UNDERFLOW.0
        | Self::OVERFLOW.0
        | Self::DIVIDE_BY_ZERO.0
        | Self::INVALID.0;

    /// Returns a set containing no exception flags.
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    /// Constructs a flag set when every bit has a defined meaning.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits & !Self::KNOWN_BITS == 0 {
            Some(Self(bits))
        } else {
            None
        }
    }

    /// Returns the stable five-bit representation of this flag set.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns whether every flag in `other` is present.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// Returns whether the set contains no flags.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }
}

impl core::fmt::Debug for ExceptionFlags {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_tuple("ExceptionFlags")
            .field(&self.0)
            .finish()
    }
}

impl BitOr for ExceptionFlags {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for ExceptionFlags {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// Records ISA-independent facts about target-precision rounding.
///
/// `tiny_after_rounding` is computed after rounding to the target precision
/// with an unbounded exponent range. `precision_inexact` records discarded
/// nonzero precision bits; exponent overflow alone does not set it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RoundingFacts {
    /// Reports a nonzero magnitude below the minimum normal value after precision rounding.
    pub tiny_after_rounding: bool,
    /// Reports whether target-precision rounding discarded nonzero information.
    pub precision_inexact: bool,
}

/// Contains the value, flags, and rounding facts from one deterministic operation.
///
/// Results satisfy `UNDERFLOW == tiny_after_rounding && precision_inexact`
/// and `INEXACT == precision_inexact || OVERFLOW`. Floating-point conversions
/// to signed integers use `Outcome<Option<T>>`; those results additionally
/// satisfy `value.is_none() == flags.contains(INVALID)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Outcome<T> {
    /// Contains the operation value or conversion-validity result.
    pub value: T,
    /// Contains exception flags raised by this operation only.
    pub flags: ExceptionFlags,
    /// Contains ISA-independent rounding facts for this operation.
    pub rounding: RoundingFacts,
}

impl<T> Outcome<T> {
    /// Constructs an outcome after validating the relationship between flags and facts.
    ///
    /// # Panics
    ///
    /// Panics if the underflow or inexact flags disagree with `rounding`.
    #[must_use]
    pub fn new(value: T, flags: ExceptionFlags, rounding: RoundingFacts) -> Self {
        assert_rounding_invariants(flags, rounding);
        Self {
            value,
            flags,
            rounding,
        }
    }
}

impl<T> Outcome<Option<T>> {
    /// Constructs an integer-conversion outcome after validating all result invariants.
    ///
    /// # Panics
    ///
    /// Panics if the flags and rounding facts disagree, or if conversion
    /// validity does not match the invalid-operation flag.
    #[must_use]
    pub fn new_optional(value: Option<T>, flags: ExceptionFlags, rounding: RoundingFacts) -> Self {
        assert_rounding_invariants(flags, rounding);
        assert_eq!(value.is_none(), flags.contains(ExceptionFlags::INVALID));
        Self {
            value,
            flags,
            rounding,
        }
    }
}

fn assert_rounding_invariants(flags: ExceptionFlags, rounding: RoundingFacts) {
    assert_eq!(
        flags.contains(ExceptionFlags::UNDERFLOW),
        rounding.tiny_after_rounding && rounding.precision_inexact
    );
    assert_eq!(
        flags.contains(ExceptionFlags::INEXACT),
        rounding.precision_inexact || flags.contains(ExceptionFlags::OVERFLOW)
    );
}

/// Describes the result of one quiet IEEE comparison.
///
/// This type has no guest condition-code encoding. Any NaN operand produces
/// [`Self::Unordered`]; signaling NaN handling is reported separately through
/// [`ExceptionFlags::INVALID`] by the deterministic backend.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Relation {
    /// The left operand is less than the right operand.
    Less,
    /// The operands compare equal, including positive and negative zero.
    Equal,
    /// The left operand is greater than the right operand.
    Greater,
    /// At least one operand is a NaN.
    Unordered,
}

#[cfg(test)]
mod tests {
    use super::{ExceptionFlags, Outcome, RoundingFacts};

    #[test]
    fn exception_flags_combine_without_unknown_bits() {
        let mut flags = ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT;
        flags |= ExceptionFlags::INVALID;

        assert!(flags.contains(ExceptionFlags::UNDERFLOW));
        assert!(flags.contains(ExceptionFlags::INEXACT));
        assert!(flags.contains(ExceptionFlags::INVALID));
        assert!(!flags.contains(ExceptionFlags::OVERFLOW));
        assert_eq!(ExceptionFlags::from_bits(flags.bits()), Some(flags));
        assert_eq!(ExceptionFlags::from_bits(0x80), None);
    }

    #[test]
    fn outcome_accepts_exact_tiny_and_exact_overflow() {
        let tiny = Outcome::new(
            0x007f_ffff_u32,
            ExceptionFlags::empty(),
            RoundingFacts {
                tiny_after_rounding: true,
                precision_inexact: false,
            },
        );
        let overflow = Outcome::new(
            0x7f80_0000_u32,
            ExceptionFlags::OVERFLOW | ExceptionFlags::INEXACT,
            RoundingFacts {
                tiny_after_rounding: false,
                precision_inexact: false,
            },
        );

        assert!(tiny.rounding.tiny_after_rounding);
        assert!(!overflow.rounding.precision_inexact);
    }

    #[test]
    #[should_panic(expected = "assertion `left == right` failed")]
    fn outcome_rejects_underflow_without_tiny_inexact_rounding() {
        let _ = Outcome::new(
            0_u32,
            ExceptionFlags::UNDERFLOW | ExceptionFlags::INEXACT,
            RoundingFacts::default(),
        );
    }

    #[test]
    fn optional_outcome_accepts_valid_and_invalid_conversions() {
        let valid = Outcome::new_optional(
            Some(7_i32),
            ExceptionFlags::empty(),
            RoundingFacts::default(),
        );
        let invalid = Outcome::<Option<i32>>::new_optional(
            None,
            ExceptionFlags::INVALID,
            RoundingFacts::default(),
        );

        assert_eq!(valid.value, Some(7));
        assert_eq!(invalid.value, None);
    }

    #[test]
    #[should_panic(expected = "assertion `left == right` failed")]
    fn optional_outcome_rejects_invalid_flag_with_value() {
        let _ = Outcome::new_optional(
            Some(7_i32),
            ExceptionFlags::INVALID,
            RoundingFacts::default(),
        );
    }
}
