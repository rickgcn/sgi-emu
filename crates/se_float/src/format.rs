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
}
