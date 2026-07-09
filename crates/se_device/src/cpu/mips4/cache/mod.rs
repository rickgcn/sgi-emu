//! Generic MIPS IV cache and memory access helpers.
//!
//! This module contains architecture-level cache access classifications, raw
//! `CACHE` instruction field extraction, and cache-line address arithmetic. It
//! does not define implementation-specific cache operations, cache storage,
//! cache tags, CP0 cache registers, address translation, or bus transactions.

use crate::cpu::mips4::instruction::Mips4Instruction;

/// Primary opcode for the privileged `CACHE` instruction.
pub const MIPS4_CACHE_OPCODE: u8 = 0x2f;

/// Architecture-level memory access type.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4MemoryAccessType {
    /// Physical memory resolves each reference without cache lookup or update.
    Uncached,

    /// The local processor cache hierarchy may resolve the reference.
    CachedNoncoherent,

    /// A coherent cache hierarchy may resolve the reference.
    CachedCoherent,

    /// Implementation-defined access type.
    ImplementationSpecific,
}

impl Mips4MemoryAccessType {
    /// Returns whether this access type is architecturally cached.
    pub const fn is_cached(self) -> bool {
        matches!(self, Self::CachedNoncoherent | Self::CachedCoherent)
    }

    /// Returns whether this access type is architecturally uncached.
    pub const fn is_uncached(self) -> bool {
        matches!(self, Self::Uncached)
    }

    /// Returns whether this access type is architecturally coherent.
    pub const fn is_coherent(self) -> bool {
        matches!(self, Self::CachedCoherent)
    }

    /// Returns whether this access type permits LL/SC atomicity.
    ///
    /// The manual restricts linked load and store conditional to cached memory:
    /// cached noncoherent or cached coherent (MIPS IV manual sections A.3 and
    /// the `LL`/`SC` restrictions). This must be evaluated on a processor-model
    /// resolved concrete access type; the base layer resolves raw cached CCAs to
    /// [`Self::ImplementationSpecific`], which is not eligible until a model
    /// refines it.
    pub const fn is_ll_sc_eligible(self) -> bool {
        self.is_cached()
    }

    /// Returns whether `SYNC` orders loads and stores of this access type.
    ///
    /// The manual restricts `SYNC` ordering to synchronizable accesses: loads and
    /// stores to shared memory using an uncached or cached coherent access type
    /// (MIPS IV manual section A.2, Table A-19). This must be evaluated on a
    /// processor-model resolved concrete access type; the base layer resolves raw
    /// cached CCAs to [`Self::ImplementationSpecific`], which is not synchronizable
    /// until a model refines it.
    pub const fn is_synchronizable(self) -> bool {
        matches!(self, Self::Uncached | Self::CachedCoherent)
    }
}

/// Raw 3-bit cache-coherence algorithm value.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4CacheCoherenceAlgorithm(u8);

impl Mips4CacheCoherenceAlgorithm {
    /// Creates a raw cache-coherence algorithm value from its 3-bit encoding.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        if bits <= 0x07 { Some(Self(bits)) } else { None }
    }

    /// Returns the raw 3-bit cache-coherence algorithm value.
    pub const fn bits(self) -> u8 {
        self.0
    }
}

/// Raw fields of a MIPS IV `CACHE` instruction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4CacheInstruction {
    instruction: Mips4Instruction,
}

impl Mips4CacheInstruction {
    /// Creates a `CACHE` instruction wrapper from a raw instruction word.
    pub const fn from_bits(bits: u32) -> Option<Self> {
        Self::from_instruction(Mips4Instruction::from_bits(bits))
    }

    /// Creates a `CACHE` instruction wrapper when the primary opcode matches.
    pub const fn from_instruction(instruction: Mips4Instruction) -> Option<Self> {
        if instruction.opcode() == MIPS4_CACHE_OPCODE {
            Some(Self { instruction })
        } else {
            None
        }
    }

    /// Returns the wrapped instruction.
    pub const fn instruction(self) -> Mips4Instruction {
        self.instruction
    }

    /// Returns the raw instruction word.
    pub const fn bits(self) -> u32 {
        self.instruction.bits()
    }

    /// Returns the base register field.
    pub const fn base(self) -> u8 {
        self.instruction.rs()
    }

    /// Returns the raw 5-bit cache operation field.
    pub const fn op(self) -> u8 {
        self.instruction.rt()
    }

    /// Returns the raw unsigned offset field.
    pub const fn raw_offset(self) -> u16 {
        self.instruction.immediate()
    }

    /// Returns the signed offset field.
    pub const fn offset(self) -> i16 {
        self.instruction.signed_immediate()
    }

    /// Returns the low two bits of the raw cache operation field.
    pub const fn cache_selector_bits(self) -> u8 {
        self.op() & 0x03
    }

    /// Returns the high three bits of the raw cache operation field.
    pub const fn operation_bits(self) -> u8 {
        self.op() >> 2
    }
}

/// Cache-line address geometry.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4CacheLineGeometry {
    /// Cache line size in bytes.
    pub line_size_bytes: u64,
}

impl Mips4CacheLineGeometry {
    /// Creates cache-line geometry when the line size is a non-zero power of two.
    pub const fn new(line_size_bytes: u64) -> Option<Self> {
        if line_size_bytes != 0 && (line_size_bytes & (line_size_bytes - 1)) == 0 {
            Some(Self { line_size_bytes })
        } else {
            None
        }
    }

    /// Returns the byte offset within the cache line containing `address`.
    pub const fn line_offset(self, address: u64) -> u64 {
        address & (self.line_size_bytes - 1)
    }

    /// Returns the base address of the cache line containing `address`.
    pub const fn line_base(self, address: u64) -> u64 {
        address & !(self.line_size_bytes - 1)
    }

    /// Returns the number of cache lines for a cache size.
    pub const fn line_count(self, cache_size_bytes: u64) -> Option<u64> {
        if cache_size_bytes != 0 && cache_size_bytes.is_multiple_of(self.line_size_bytes) {
            Some(cache_size_bytes / self.line_size_bytes)
        } else {
            None
        }
    }

    /// Returns the cache-line index for `address` within a cache of the given size.
    pub const fn line_index(self, address: u64, cache_size_bytes: u64) -> Option<u64> {
        match self.line_count(cache_size_bytes) {
            Some(line_count) => Some((self.line_base(address) / self.line_size_bytes) % line_count),
            None => None,
        }
    }
}

#[cfg(test)]
mod tests;
