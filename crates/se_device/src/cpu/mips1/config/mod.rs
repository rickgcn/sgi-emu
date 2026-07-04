//! Configurable MIPS I processor parameters.
//!
//! MIPS I device models use this module to describe architectural parameters
//! that vary between concrete processor integrations. The configuration stores
//! values such as processor identity, byte order, cache geometry, and
//! coprocessor availability without assigning board-specific meanings to them.

/// Byte order used by memory-facing MIPS I operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips1Endianness {
    /// Most significant byte is stored at the lowest address.
    Big,

    /// Least significant byte is stored at the lowest address.
    Little,
}

/// Cache configuration visible to a MIPS I processor model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips1CacheConfig {
    /// The cache is not present.
    Disabled,

    /// The cache is present with fixed geometry.
    Present {
        /// Total cache size in bytes.
        size_bytes: u32,

        /// Cache line size in bytes.
        line_size_bytes: u32,
    },
}

impl Mips1CacheConfig {
    /// Creates a cache configuration for a present cache.
    pub const fn present(size_bytes: u32, line_size_bytes: u32) -> Self {
        Self::Present {
            size_bytes,
            line_size_bytes,
        }
    }

    /// Creates a cache configuration for an absent cache.
    pub const fn disabled() -> Self {
        Self::Disabled
    }

    /// Returns whether the cache is present.
    pub const fn is_present(self) -> bool {
        matches!(self, Self::Present { .. })
    }

    /// Returns the cache size in bytes.
    pub const fn size_bytes(self) -> Option<u32> {
        match self {
            Self::Disabled => None,
            Self::Present { size_bytes, .. } => Some(size_bytes),
        }
    }

    /// Returns the cache line size in bytes.
    pub const fn line_size_bytes(self) -> Option<u32> {
        match self {
            Self::Disabled => None,
            Self::Present {
                line_size_bytes, ..
            } => Some(line_size_bytes),
        }
    }
}

/// Coprocessor availability for a MIPS I processor model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips1CoprocessorConfig {
    /// Whether coprocessor 1 is available.
    pub cp1: bool,

    /// Whether coprocessor 2 is available.
    pub cp2: bool,

    /// Whether coprocessor 3 is available.
    pub cp3: bool,
}

impl Mips1CoprocessorConfig {
    /// Creates coprocessor availability configuration.
    pub const fn new(cp1: bool, cp2: bool, cp3: bool) -> Self {
        Self { cp1, cp2, cp3 }
    }

    /// Returns whether coprocessor 0 is available.
    pub const fn cp0(self) -> bool {
        true
    }
}

/// Configurable MIPS I processor parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips1Config {
    /// Processor memory byte order.
    pub endianness: Mips1Endianness,

    /// Raw processor identity value.
    pub processor_id: u32,

    /// Instruction cache configuration.
    pub instruction_cache: Mips1CacheConfig,

    /// Data cache configuration.
    pub data_cache: Mips1CacheConfig,

    /// Coprocessor availability configuration.
    pub coprocessors: Mips1CoprocessorConfig,
}

impl Mips1Config {
    /// Creates MIPS I processor configuration.
    pub const fn new(
        endianness: Mips1Endianness,
        processor_id: u32,
        instruction_cache: Mips1CacheConfig,
        data_cache: Mips1CacheConfig,
        coprocessors: Mips1CoprocessorConfig,
    ) -> Self {
        Self {
            endianness,
            processor_id,
            instruction_cache,
            data_cache,
            coprocessors,
        }
    }
}

#[cfg(test)]
mod tests;
