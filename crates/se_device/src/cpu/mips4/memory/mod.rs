//! Pure MIPS IV memory access helpers.
//!
//! This module computes effective addresses, checks architectural alignment,
//! performs load extension, and applies partial word and doubleword merge
//! rules. It does not perform address translation, cache lookup, bus access, or
//! load-delay state updates.

use crate::cpu::mips4::config::Mips4Endianness;
use crate::cpu::mips4::exception::Mips4Exception;
use crate::cpu::mips4::gpr::sign_extend_word;

const REGION_BITS_MASK: u64 = 0xc000_0000_0000_0000;

/// MIPS IV memory access size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4MemoryAccessSize {
    /// One byte.
    Byte,

    /// Two bytes.
    Halfword,

    /// Four bytes.
    Word,

    /// Eight bytes.
    Doubleword,
}

/// MIPS IV memory access kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4MemoryAccessKind {
    /// Load from memory.
    Load {
        /// Load size.
        size: Mips4MemoryAccessSize,

        /// Whether the loaded value is sign-extended.
        signed: bool,
    },

    /// Store to memory.
    Store {
        /// Store size.
        size: Mips4MemoryAccessSize,
    },

    /// `LWL` partial-word load.
    LoadWordLeft,

    /// `LWR` partial-word load.
    LoadWordRight,

    /// `SWL` partial-word store.
    StoreWordLeft,

    /// `SWR` partial-word store.
    StoreWordRight,

    /// `LDL` partial-doubleword load.
    LoadDoublewordLeft,

    /// `LDR` partial-doubleword load.
    LoadDoublewordRight,

    /// `SDL` partial-doubleword store.
    StoreDoublewordLeft,

    /// `SDR` partial-doubleword store.
    StoreDoublewordRight,
}

/// Result of applying a partial-word store to an aligned memory word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4MaskedMemoryWord {
    /// Memory word after applying the partial-word store.
    pub value: u32,

    /// Bit mask identifying bytes written by the partial-word store.
    pub write_mask: u32,
}

/// Result of applying a partial-doubleword store to an aligned memory doubleword.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4MaskedMemoryDoubleword {
    /// Memory doubleword after applying the partial-doubleword store.
    pub value: u64,

    /// Bit mask identifying bytes written by the partial-doubleword store.
    pub write_mask: u64,
}

/// Stateless MIPS IV memory helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mips4Memory;

impl Mips4Memory {
    /// Computes a base-plus-sign-extended-offset effective address.
    pub const fn effective_address(base: u64, offset: i16) -> u64 {
        base.wrapping_add(offset as i64 as u64)
    }

    /// Computes a base-plus-index effective address for indexed memory operations.
    pub const fn indexed_effective_address(
        base: u64,
        index: u64,
        is_store: bool,
    ) -> Result<u64, Mips4Exception> {
        let address = base.wrapping_add(index);
        if address & REGION_BITS_MASK == base & REGION_BITS_MASK {
            Ok(address)
        } else if is_store {
            Err(Mips4Exception::AddressErrorStore)
        } else {
            Err(Mips4Exception::AddressErrorLoad)
        }
    }

    /// Checks architectural alignment for a memory access.
    pub const fn check_alignment(
        address: u64,
        kind: Mips4MemoryAccessKind,
    ) -> Result<(), Mips4Exception> {
        let aligned = match kind {
            Mips4MemoryAccessKind::Load { size, .. } | Mips4MemoryAccessKind::Store { size } => {
                size.is_aligned(address)
            }
            Mips4MemoryAccessKind::LoadWordLeft
            | Mips4MemoryAccessKind::LoadWordRight
            | Mips4MemoryAccessKind::StoreWordLeft
            | Mips4MemoryAccessKind::StoreWordRight
            | Mips4MemoryAccessKind::LoadDoublewordLeft
            | Mips4MemoryAccessKind::LoadDoublewordRight
            | Mips4MemoryAccessKind::StoreDoublewordLeft
            | Mips4MemoryAccessKind::StoreDoublewordRight => true,
        };

        if aligned {
            Ok(())
        } else if kind.is_store() {
            Err(Mips4Exception::AddressErrorStore)
        } else {
            Err(Mips4Exception::AddressErrorLoad)
        }
    }

    /// Sign-extends a loaded byte to a doubleword.
    pub const fn sign_extend_byte(value: u8) -> u64 {
        value as i8 as i64 as u64
    }

    /// Zero-extends a loaded byte to a doubleword.
    pub const fn zero_extend_byte(value: u8) -> u64 {
        value as u64
    }

    /// Sign-extends a loaded halfword to a doubleword.
    pub const fn sign_extend_halfword(value: u16) -> u64 {
        value as i16 as i64 as u64
    }

    /// Zero-extends a loaded halfword to a doubleword.
    pub const fn zero_extend_halfword(value: u16) -> u64 {
        value as u64
    }

    /// Sign-extends a loaded word to a doubleword.
    pub const fn sign_extend_loaded_word(value: u32) -> u64 {
        sign_extend_word(value)
    }

    /// Zero-extends a loaded word to a doubleword.
    pub const fn zero_extend_word(value: u32) -> u64 {
        value as u64
    }

    /// Merges an aligned memory word into a register using `LWL` rules.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn lwl_merge(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_word: u32,
    ) -> u64 {
        sign_extend_word(lwl_merge_word(
            endianness,
            word_offset(address),
            register_value as u32,
            memory_word,
        ))
    }

    /// Merges an aligned memory word into a register using `LWR` rules.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn lwr_merge(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_word: u32,
    ) -> u64 {
        sign_extend_word(lwr_merge_word(
            endianness,
            word_offset(address),
            register_value as u32,
            memory_word,
        ))
    }

    /// Applies `SWL` rules to an aligned memory word.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn swl_masked_word(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_word: u32,
    ) -> Mips4MaskedMemoryWord {
        let offset = word_offset(address);
        let write_mask = match endianness {
            Mips4Endianness::Big => left_store_mask32(offset),
            Mips4Endianness::Little => left_store_mask32(3 - offset),
        };
        let write_value = match endianness {
            Mips4Endianness::Big => (register_value as u32) >> (offset * 8),
            Mips4Endianness::Little => (register_value as u32) >> ((3 - offset) * 8),
        };

        masked_word(memory_word, write_value, write_mask)
    }

    /// Applies `SWR` rules to an aligned memory word.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn swr_masked_word(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_word: u32,
    ) -> Mips4MaskedMemoryWord {
        let offset = word_offset(address);
        let write_mask = match endianness {
            Mips4Endianness::Big => right_store_mask32(offset),
            Mips4Endianness::Little => right_store_mask32(3 - offset),
        };
        let write_value = match endianness {
            Mips4Endianness::Big => (register_value as u32) << ((3 - offset) * 8),
            Mips4Endianness::Little => (register_value as u32) << (offset * 8),
        };

        masked_word(memory_word, write_value, write_mask)
    }

    /// Merges an aligned memory doubleword into a register using `LDL` rules.
    ///
    /// `memory_doubleword` is the value an aligned `LD` would produce for the
    /// memory doubleword containing `address`.
    pub const fn ldl_merge(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_doubleword: u64,
    ) -> u64 {
        let offset = doubleword_offset(address);
        match endianness {
            Mips4Endianness::Big => left_load64(offset, register_value, memory_doubleword),
            Mips4Endianness::Little => left_load64(7 - offset, register_value, memory_doubleword),
        }
    }

    /// Merges an aligned memory doubleword into a register using `LDR` rules.
    ///
    /// `memory_doubleword` is the value an aligned `LD` would produce for the
    /// memory doubleword containing `address`.
    pub const fn ldr_merge(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_doubleword: u64,
    ) -> u64 {
        let offset = doubleword_offset(address);
        match endianness {
            Mips4Endianness::Big => right_load64(offset, register_value, memory_doubleword),
            Mips4Endianness::Little => right_load64(7 - offset, register_value, memory_doubleword),
        }
    }

    /// Applies `SDL` rules to an aligned memory doubleword.
    ///
    /// `memory_doubleword` is the value an aligned `LD` would produce for the
    /// memory doubleword containing `address`.
    pub const fn sdl_masked_doubleword(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_doubleword: u64,
    ) -> Mips4MaskedMemoryDoubleword {
        let offset = doubleword_offset(address);
        let write_mask = match endianness {
            Mips4Endianness::Big => left_store_mask64(offset),
            Mips4Endianness::Little => left_store_mask64(7 - offset),
        };
        let write_value = match endianness {
            Mips4Endianness::Big => register_value >> (offset * 8),
            Mips4Endianness::Little => register_value >> ((7 - offset) * 8),
        };

        masked_doubleword(memory_doubleword, write_value, write_mask)
    }

    /// Applies `SDR` rules to an aligned memory doubleword.
    ///
    /// `memory_doubleword` is the value an aligned `LD` would produce for the
    /// memory doubleword containing `address`.
    pub const fn sdr_masked_doubleword(
        endianness: Mips4Endianness,
        address: u64,
        register_value: u64,
        memory_doubleword: u64,
    ) -> Mips4MaskedMemoryDoubleword {
        let offset = doubleword_offset(address);
        let write_mask = match endianness {
            Mips4Endianness::Big => right_store_mask64(offset),
            Mips4Endianness::Little => right_store_mask64(7 - offset),
        };
        let write_value = match endianness {
            Mips4Endianness::Big => register_value << ((7 - offset) * 8),
            Mips4Endianness::Little => register_value << (offset * 8),
        };

        masked_doubleword(memory_doubleword, write_value, write_mask)
    }
}

impl Mips4MemoryAccessSize {
    const fn is_aligned(self, address: u64) -> bool {
        match self {
            Self::Byte => true,
            Self::Halfword => address & 0x1 == 0,
            Self::Word => address & 0x3 == 0,
            Self::Doubleword => address & 0x7 == 0,
        }
    }
}

impl Mips4MemoryAccessKind {
    const fn is_store(self) -> bool {
        matches!(
            self,
            Self::Store { .. }
                | Self::StoreWordLeft
                | Self::StoreWordRight
                | Self::StoreDoublewordLeft
                | Self::StoreDoublewordRight
        )
    }
}

const fn word_offset(address: u64) -> u32 {
    (address & 0x3) as u32
}

const fn doubleword_offset(address: u64) -> u32 {
    (address & 0x7) as u32
}

const fn left_load32(offset: u32, register_value: u32, memory_word: u32) -> u32 {
    if offset == 0 {
        memory_word
    } else {
        (register_value & low_bytes_mask32(offset)) | (memory_word << (offset * 8))
    }
}

const fn right_load32(offset: u32, register_value: u32, memory_word: u32) -> u32 {
    if offset == 3 {
        memory_word
    } else {
        (register_value & high_bytes_mask32(3 - offset)) | (memory_word >> ((3 - offset) * 8))
    }
}

const fn lwl_merge_word(
    endianness: Mips4Endianness,
    offset: u32,
    register_value: u32,
    memory_word: u32,
) -> u32 {
    match endianness {
        Mips4Endianness::Big => left_load32(offset, register_value, memory_word),
        Mips4Endianness::Little => left_load32(3 - offset, register_value, memory_word),
    }
}

const fn lwr_merge_word(
    endianness: Mips4Endianness,
    offset: u32,
    register_value: u32,
    memory_word: u32,
) -> u32 {
    match endianness {
        Mips4Endianness::Big => right_load32(offset, register_value, memory_word),
        Mips4Endianness::Little => right_load32(3 - offset, register_value, memory_word),
    }
}

const fn left_load64(offset: u32, register_value: u64, memory_doubleword: u64) -> u64 {
    if offset == 0 {
        memory_doubleword
    } else {
        (register_value & low_bytes_mask64(offset)) | (memory_doubleword << (offset * 8))
    }
}

const fn right_load64(offset: u32, register_value: u64, memory_doubleword: u64) -> u64 {
    if offset == 7 {
        memory_doubleword
    } else {
        (register_value & high_bytes_mask64(7 - offset)) | (memory_doubleword >> ((7 - offset) * 8))
    }
}

const fn low_bytes_mask32(count: u32) -> u32 {
    if count == 0 {
        0
    } else {
        u32::MAX >> ((4 - count) * 8)
    }
}

const fn high_bytes_mask32(count: u32) -> u32 {
    if count == 0 {
        0
    } else {
        u32::MAX << ((4 - count) * 8)
    }
}

const fn low_bytes_mask64(count: u32) -> u64 {
    if count == 0 {
        0
    } else {
        u64::MAX >> ((8 - count) * 8)
    }
}

const fn high_bytes_mask64(count: u32) -> u64 {
    if count == 0 {
        0
    } else {
        u64::MAX << ((8 - count) * 8)
    }
}

const fn left_store_mask32(offset: u32) -> u32 {
    u32::MAX >> (offset * 8)
}

const fn right_store_mask32(offset: u32) -> u32 {
    u32::MAX << ((3 - offset) * 8)
}

const fn left_store_mask64(offset: u32) -> u64 {
    u64::MAX >> (offset * 8)
}

const fn right_store_mask64(offset: u32) -> u64 {
    u64::MAX << ((7 - offset) * 8)
}

const fn masked_word(memory_word: u32, write_value: u32, write_mask: u32) -> Mips4MaskedMemoryWord {
    Mips4MaskedMemoryWord {
        value: (memory_word & !write_mask) | (write_value & write_mask),
        write_mask,
    }
}

const fn masked_doubleword(
    memory_doubleword: u64,
    write_value: u64,
    write_mask: u64,
) -> Mips4MaskedMemoryDoubleword {
    Mips4MaskedMemoryDoubleword {
        value: (memory_doubleword & !write_mask) | (write_value & write_mask),
        write_mask,
    }
}

#[cfg(test)]
mod tests;
