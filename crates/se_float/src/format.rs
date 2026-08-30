//! Guest floating-point bit-pattern types.

/// A binary32 value stored without applying host floating-point semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Float32(u32);

impl Float32 {
    /// Creates a binary32 value from its raw bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw bits of this binary32 value.
    pub const fn to_bits(self) -> u32 {
        self.0
    }

    /// Reports whether this value is a NaN under the binary32 encoding.
    pub const fn is_nan(self) -> bool {
        self.0 & 0x7f80_0000 == 0x7f80_0000 && self.0 & 0x007f_ffff != 0
    }

    /// Reports whether this value is a signaling NaN under the legacy MIPS convention.
    pub const fn is_signaling_nan(self) -> bool {
        self.is_nan() && self.0 & 0x0040_0000 != 0
    }

    /// Reports whether this value is a nonzero subnormal binary32 value.
    pub const fn is_subnormal(self) -> bool {
        self.0 & 0x7f80_0000 == 0 && self.0 & 0x007f_ffff != 0
    }
}

/// A binary64 value stored without applying host floating-point semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Float64(u64);

impl Float64 {
    /// Creates a binary64 value from its raw bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the raw bits of this binary64 value.
    pub const fn to_bits(self) -> u64 {
        self.0
    }

    /// Reports whether this value is a NaN under the binary64 encoding.
    pub const fn is_nan(self) -> bool {
        self.0 & 0x7ff0_0000_0000_0000 == 0x7ff0_0000_0000_0000
            && self.0 & 0x000f_ffff_ffff_ffff != 0
    }

    /// Reports whether this value is a signaling NaN under the legacy MIPS convention.
    pub const fn is_signaling_nan(self) -> bool {
        self.is_nan() && self.0 & 0x0008_0000_0000_0000 != 0
    }

    /// Reports whether this value is a nonzero subnormal binary64 value.
    pub const fn is_subnormal(self) -> bool {
        self.0 & 0x7ff0_0000_0000_0000 == 0 && self.0 & 0x000f_ffff_ffff_ffff != 0
    }
}
