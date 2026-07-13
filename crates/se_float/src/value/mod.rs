//! Raw IEEE-754 value wrappers and classification helpers.
//!
//! These wrappers preserve exact bit patterns. They do not canonicalize NaNs,
//! alter payload bits, or interpret values through host floating-point state.

/// NaN quiet/signaling interpretation.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FloatNanMode {
    /// The most significant fraction bit marks a quiet NaN when set.
    #[default]
    QuietBitSet,

    /// The most significant fraction bit marks a quiet NaN when clear.
    QuietBitClear,
}

/// IEEE-754 floating-point class.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FloatClass {
    /// Positive zero.
    PositiveZero,

    /// Negative zero.
    NegativeZero,

    /// Positive subnormal finite value.
    PositiveSubnormal,

    /// Negative subnormal finite value.
    NegativeSubnormal,

    /// Positive normal finite value.
    PositiveNormal,

    /// Negative normal finite value.
    NegativeNormal,

    /// Positive infinity.
    PositiveInfinity,

    /// Negative infinity.
    NegativeInfinity,

    /// Quiet NaN.
    QuietNan,

    /// Signaling NaN.
    SignalingNan,
}

impl FloatClass {
    /// Returns whether this class is any NaN.
    pub const fn is_nan(self) -> bool {
        matches!(self, Self::QuietNan | Self::SignalingNan)
    }

    /// Returns whether this class is a signaling NaN.
    pub const fn is_signaling_nan(self) -> bool {
        matches!(self, Self::SignalingNan)
    }
}

/// Ordered or unordered relation between two floating-point values.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FloatRelation {
    /// Left operand is less than the right operand.
    Less,

    /// Operands compare equal.
    Equal,

    /// Left operand is greater than the right operand.
    Greater,

    /// At least one operand is NaN.
    Unordered,
}

/// Floating-point compare exception behavior.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FloatCompareMode {
    /// Raise invalid only for signaling NaN operands.
    Quiet,

    /// Raise invalid for unordered operands.
    Signaling,
}

/// Raw single-precision IEEE-754 bits.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Float32Bits(u32);

impl Float32Bits {
    const SIGN_MASK: u32 = 0x8000_0000;
    const EXPONENT_MASK: u32 = 0x7f80_0000;
    const FRACTION_MASK: u32 = 0x007f_ffff;
    const QUIET_BIT: u32 = 0x0040_0000;

    /// Creates a raw single-precision value.
    pub const fn new(bits: u32) -> Self {
        Self(bits)
    }

    /// Creates a raw single-precision value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the sign bit.
    pub const fn sign_bit(self) -> bool {
        (self.0 & Self::SIGN_MASK) != 0
    }

    /// Returns the biased exponent field.
    pub const fn exponent_bits(self) -> u16 {
        ((self.0 & Self::EXPONENT_MASK) >> 23) as u16
    }

    /// Returns the fraction field.
    pub const fn fraction_bits(self) -> u32 {
        self.0 & Self::FRACTION_MASK
    }

    /// Returns the value with the sign bit cleared.
    pub const fn abs(self) -> Self {
        Self(self.0 & !Self::SIGN_MASK)
    }

    /// Returns the value with the sign bit toggled.
    pub const fn neg(self) -> Self {
        Self(self.0 ^ Self::SIGN_MASK)
    }

    /// Classifies this value under the requested NaN interpretation.
    pub const fn classify(self, nan_mode: FloatNanMode) -> FloatClass {
        let sign = self.sign_bit();
        let exponent = self.exponent_bits();
        let fraction = self.fraction_bits();

        if exponent == 0 {
            if fraction == 0 {
                if sign {
                    FloatClass::NegativeZero
                } else {
                    FloatClass::PositiveZero
                }
            } else if sign {
                FloatClass::NegativeSubnormal
            } else {
                FloatClass::PositiveSubnormal
            }
        } else if exponent == 0xff {
            if fraction == 0 {
                if sign {
                    FloatClass::NegativeInfinity
                } else {
                    FloatClass::PositiveInfinity
                }
            } else {
                classify_nan(fraction & Self::QUIET_BIT != 0, nan_mode)
            }
        } else if sign {
            FloatClass::NegativeNormal
        } else {
            FloatClass::PositiveNormal
        }
    }

    /// Returns whether this value is any NaN under the default NaN mode.
    pub const fn is_nan(self) -> bool {
        self.classify(FloatNanMode::QuietBitSet).is_nan()
    }
}

/// Raw double-precision IEEE-754 bits.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Float64Bits(u64);

impl Float64Bits {
    const SIGN_MASK: u64 = 0x8000_0000_0000_0000;
    const EXPONENT_MASK: u64 = 0x7ff0_0000_0000_0000;
    const FRACTION_MASK: u64 = 0x000f_ffff_ffff_ffff;
    const QUIET_BIT: u64 = 0x0008_0000_0000_0000;

    /// Creates a raw double-precision value.
    pub const fn new(bits: u64) -> Self {
        Self(bits)
    }

    /// Creates a raw double-precision value.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the raw bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns the sign bit.
    pub const fn sign_bit(self) -> bool {
        (self.0 & Self::SIGN_MASK) != 0
    }

    /// Returns the biased exponent field.
    pub const fn exponent_bits(self) -> u16 {
        ((self.0 & Self::EXPONENT_MASK) >> 52) as u16
    }

    /// Returns the fraction field.
    pub const fn fraction_bits(self) -> u64 {
        self.0 & Self::FRACTION_MASK
    }

    /// Returns the value with the sign bit cleared.
    pub const fn abs(self) -> Self {
        Self(self.0 & !Self::SIGN_MASK)
    }

    /// Returns the value with the sign bit toggled.
    pub const fn neg(self) -> Self {
        Self(self.0 ^ Self::SIGN_MASK)
    }

    /// Classifies this value under the requested NaN interpretation.
    pub const fn classify(self, nan_mode: FloatNanMode) -> FloatClass {
        let sign = self.sign_bit();
        let exponent = self.exponent_bits();
        let fraction = self.fraction_bits();

        if exponent == 0 {
            if fraction == 0 {
                if sign {
                    FloatClass::NegativeZero
                } else {
                    FloatClass::PositiveZero
                }
            } else if sign {
                FloatClass::NegativeSubnormal
            } else {
                FloatClass::PositiveSubnormal
            }
        } else if exponent == 0x7ff {
            if fraction == 0 {
                if sign {
                    FloatClass::NegativeInfinity
                } else {
                    FloatClass::PositiveInfinity
                }
            } else {
                classify_nan(fraction & Self::QUIET_BIT != 0, nan_mode)
            }
        } else if sign {
            FloatClass::NegativeNormal
        } else {
            FloatClass::PositiveNormal
        }
    }

    /// Returns whether this value is any NaN under the default NaN mode.
    pub const fn is_nan(self) -> bool {
        self.classify(FloatNanMode::QuietBitSet).is_nan()
    }
}

const fn classify_nan(quiet_bit_set: bool, nan_mode: FloatNanMode) -> FloatClass {
    let quiet = match nan_mode {
        FloatNanMode::QuietBitSet => quiet_bit_set,
        FloatNanMode::QuietBitClear => !quiet_bit_set,
    };

    if quiet {
        FloatClass::QuietNan
    } else {
        FloatClass::SignalingNan
    }
}

#[cfg(test)]
mod tests;
