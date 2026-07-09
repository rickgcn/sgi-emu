//! Configurable MIPS IV processor parameters.
//!
//! MIPS IV device models use this module to describe architectural parameters
//! that vary between concrete processor integrations. The configuration stores
//! values such as processor identity, byte order, address width, cache geometry,
//! and coprocessor availability without assigning board-specific meanings to
//! them.

/// Byte order used by memory-facing MIPS IV operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Endianness {
    /// Most significant byte is stored at the lowest address.
    Big,

    /// Least significant byte is stored at the lowest address.
    Little,
}

impl Mips4Endianness {
    /// Resolves the effective CPU byte order from the memory endianness and the
    /// reverse-endian signal.
    ///
    /// This mirrors the manual relation
    /// `BigEndianCPU = BigEndianMem XOR ReverseEndian` (MIPS IV manual section
    /// A.5, Table A-25). `BigEndianMem` is the reset-configured memory byte order
    /// carried by this value; `reverse_endian` is the `ReverseEndian` signal,
    /// which the caller must gate on User mode because the `RE` Status bit only
    /// takes effect in User mode. When `reverse_endian` is set, the effective CPU
    /// byte order is the opposite of the memory byte order.
    pub const fn effective_cpu_endianness(self, reverse_endian: bool) -> Self {
        match (self, reverse_endian) {
            (Self::Big, false) | (Self::Little, true) => Self::Big,
            (Self::Big, true) | (Self::Little, false) => Self::Little,
        }
    }
}

/// Cache configuration visible to a MIPS IV processor model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4CacheConfig {
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

impl Mips4CacheConfig {
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

/// Virtual and physical address widths for a MIPS IV processor model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4AddressConfig {
    /// Implemented physical address width in bits.
    pub physical_address_bits: u8,

    /// Implemented virtual address width in bits.
    pub virtual_address_bits: u8,
}

impl Mips4AddressConfig {
    /// Creates address width configuration.
    pub const fn new(physical_address_bits: u8, virtual_address_bits: u8) -> Self {
        Self {
            physical_address_bits,
            virtual_address_bits,
        }
    }
}

/// Coprocessor availability for a MIPS IV processor model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4CoprocessorConfig {
    /// Whether coprocessor 1 is available.
    pub cp1: bool,

    /// Whether coprocessor 2 is available.
    pub cp2: bool,

    /// Whether coprocessor 3 is available.
    pub cp3: bool,
}

impl Mips4CoprocessorConfig {
    /// Creates coprocessor availability configuration.
    pub const fn new(cp1: bool, cp2: bool, cp3: bool) -> Self {
        Self { cp1, cp2, cp3 }
    }

    /// Returns whether coprocessor 0 is available.
    pub const fn cp0(self) -> bool {
        true
    }
}

/// Configurable MIPS IV processor parameters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4Config {
    /// Processor memory byte order.
    pub endianness: Mips4Endianness,

    /// Raw processor identity value.
    pub processor_id: u32,

    /// Implemented address widths.
    pub address: Mips4AddressConfig,

    /// Instruction cache configuration.
    pub instruction_cache: Mips4CacheConfig,

    /// Data cache configuration.
    pub data_cache: Mips4CacheConfig,

    /// Secondary cache configuration.
    pub secondary_cache: Mips4CacheConfig,

    /// Coprocessor availability configuration.
    pub coprocessors: Mips4CoprocessorConfig,
}

impl Mips4Config {
    /// Creates MIPS IV processor configuration.
    pub const fn new(
        endianness: Mips4Endianness,
        processor_id: u32,
        address: Mips4AddressConfig,
        instruction_cache: Mips4CacheConfig,
        data_cache: Mips4CacheConfig,
        secondary_cache: Mips4CacheConfig,
        coprocessors: Mips4CoprocessorConfig,
    ) -> Self {
        Self {
            endianness,
            processor_id,
            address,
            instruction_cache,
            data_cache,
            secondary_cache,
            coprocessors,
        }
    }
}

#[cfg(test)]
mod tests;
