//! R5000 processor and FPU revision identifiers.
//!
//! R5000-compatible processors report implementation ID `0x23` in both CP0
//! `PRId` and CP1 `FCR0`. Revision bits are preserved as raw `y.x` data and do
//! not imply behavior changes in this profile layer.

/// R5000 CP0 `PRId` and CP1 `FCR0` implementation field.
pub const R5000_IMPLEMENTATION_ID: u8 = 0x23;

/// R5000 processor or FPU revision encoded as `y.x`.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct R5000Revision(u8);

impl R5000Revision {
    /// Creates a revision wrapper from raw revision bits.
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    /// Returns the raw revision bits.
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Returns the major revision nibble.
    pub const fn major(self) -> u8 {
        self.0 >> 4
    }

    /// Returns the minor revision nibble.
    pub const fn minor(self) -> u8 {
        self.0 & 0x0f
    }
}

/// Creates a raw R5000 CP0 `PRId` value for the supplied revision.
pub const fn r5000_processor_id(revision: R5000Revision) -> u32 {
    ((R5000_IMPLEMENTATION_ID as u32) << 8) | revision.bits() as u32
}

/// Creates a raw R5000 CP1 `FCR0` value for the supplied revision.
pub const fn r5000_fcr0(revision: R5000Revision) -> u32 {
    ((R5000_IMPLEMENTATION_ID as u32) << 8) | revision.bits() as u32
}

#[cfg(test)]
mod tests;
