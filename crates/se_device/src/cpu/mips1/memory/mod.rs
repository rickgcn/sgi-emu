//! Pure MIPS I memory access helpers.
//!
//! This module computes effective addresses, checks architectural alignment,
//! performs load extension, and applies partial-word merge rules. It does not
//! perform address translation, cache lookup, bus access, or load-delay state
//! updates.

use crate::cpu::mips1::config::Mips1Endianness;
use crate::cpu::mips1::exception::Mips1Exception;

/// MIPS I memory access size.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips1MemoryAccessSize {
    /// One byte.
    Byte,

    /// Two bytes.
    Halfword,

    /// Four bytes.
    Word,
}

/// MIPS I memory access kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips1MemoryAccessKind {
    /// Load from memory.
    Load {
        /// Load size.
        size: Mips1MemoryAccessSize,

        /// Whether the loaded value is sign-extended.
        signed: bool,
    },

    /// Store to memory.
    Store {
        /// Store size.
        size: Mips1MemoryAccessSize,
    },

    /// `LWL` partial-word load.
    LoadWordLeft,

    /// `LWR` partial-word load.
    LoadWordRight,

    /// `SWL` partial-word store.
    StoreWordLeft,

    /// `SWR` partial-word store.
    StoreWordRight,
}

/// Result of applying a partial-word store to an aligned memory word.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips1MaskedMemoryWord {
    /// Memory word after applying the partial-word store.
    pub value: u32,

    /// Bit mask identifying bytes written by the partial-word store.
    pub write_mask: u32,
}

/// Stateless MIPS I memory helper.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Mips1Memory;

impl Mips1Memory {
    /// Computes a base-plus-sign-extended-offset effective address.
    pub const fn effective_address(base: u32, offset: i16) -> u32 {
        base.wrapping_add(offset as i32 as u32)
    }

    /// Checks architectural alignment for a memory access.
    pub const fn check_alignment(
        address: u32,
        kind: Mips1MemoryAccessKind,
    ) -> Result<(), Mips1Exception> {
        let aligned = match kind {
            Mips1MemoryAccessKind::Load { size, .. } | Mips1MemoryAccessKind::Store { size } => {
                size.is_aligned(address)
            }
            Mips1MemoryAccessKind::LoadWordLeft
            | Mips1MemoryAccessKind::LoadWordRight
            | Mips1MemoryAccessKind::StoreWordLeft
            | Mips1MemoryAccessKind::StoreWordRight => true,
        };

        if aligned {
            Ok(())
        } else if kind.is_store() {
            Err(Mips1Exception::AddressErrorStore)
        } else {
            Err(Mips1Exception::AddressErrorLoad)
        }
    }

    /// Sign-extends a loaded byte to a word.
    pub const fn sign_extend_byte(value: u8) -> u32 {
        value as i8 as i32 as u32
    }

    /// Zero-extends a loaded byte to a word.
    pub const fn zero_extend_byte(value: u8) -> u32 {
        value as u32
    }

    /// Sign-extends a loaded halfword to a word.
    pub const fn sign_extend_halfword(value: u16) -> u32 {
        value as i16 as i32 as u32
    }

    /// Zero-extends a loaded halfword to a word.
    pub const fn zero_extend_halfword(value: u16) -> u32 {
        value as u32
    }

    /// Merges an aligned memory word into a register using `LWL` rules.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn lwl_merge(
        endianness: Mips1Endianness,
        address: u32,
        register_value: u32,
        memory_word: u32,
    ) -> u32 {
        let offset = word_offset(address);
        match endianness {
            Mips1Endianness::Big => match offset {
                0 => memory_word,
                1 => (register_value & 0x0000_00ff) | (memory_word << 8),
                2 => (register_value & 0x0000_ffff) | (memory_word << 16),
                _ => (register_value & 0x00ff_ffff) | (memory_word << 24),
            },
            Mips1Endianness::Little => match offset {
                0 => (register_value & 0x00ff_ffff) | (memory_word << 24),
                1 => (register_value & 0x0000_ffff) | (memory_word << 16),
                2 => (register_value & 0x0000_00ff) | (memory_word << 8),
                _ => memory_word,
            },
        }
    }

    /// Merges an aligned memory word into a register using `LWR` rules.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn lwr_merge(
        endianness: Mips1Endianness,
        address: u32,
        register_value: u32,
        memory_word: u32,
    ) -> u32 {
        let offset = word_offset(address);
        match endianness {
            Mips1Endianness::Big => match offset {
                0 => (register_value & 0xffff_ff00) | (memory_word >> 24),
                1 => (register_value & 0xffff_0000) | (memory_word >> 16),
                2 => (register_value & 0xff00_0000) | (memory_word >> 8),
                _ => memory_word,
            },
            Mips1Endianness::Little => match offset {
                0 => memory_word,
                1 => (register_value & 0xff00_0000) | (memory_word >> 8),
                2 => (register_value & 0xffff_0000) | (memory_word >> 16),
                _ => (register_value & 0xffff_ff00) | (memory_word >> 24),
            },
        }
    }

    /// Applies `SWL` rules to an aligned memory word.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn swl_masked_word(
        endianness: Mips1Endianness,
        address: u32,
        register_value: u32,
        memory_word: u32,
    ) -> Mips1MaskedMemoryWord {
        let offset = word_offset(address);
        let write_mask = match endianness {
            Mips1Endianness::Big => match offset {
                0 => 0xffff_ffff,
                1 => 0x00ff_ffff,
                2 => 0x0000_ffff,
                _ => 0x0000_00ff,
            },
            Mips1Endianness::Little => match offset {
                0 => 0x0000_00ff,
                1 => 0x0000_ffff,
                2 => 0x00ff_ffff,
                _ => 0xffff_ffff,
            },
        };
        let write_value = match endianness {
            Mips1Endianness::Big => match offset {
                0 => register_value,
                1 => register_value >> 8,
                2 => register_value >> 16,
                _ => register_value >> 24,
            },
            Mips1Endianness::Little => match offset {
                0 => register_value >> 24,
                1 => register_value >> 16,
                2 => register_value >> 8,
                _ => register_value,
            },
        };

        masked_word(memory_word, write_value, write_mask)
    }

    /// Applies `SWR` rules to an aligned memory word.
    ///
    /// `memory_word` is the value an aligned `LW` would produce for the memory
    /// word containing `address`.
    pub const fn swr_masked_word(
        endianness: Mips1Endianness,
        address: u32,
        register_value: u32,
        memory_word: u32,
    ) -> Mips1MaskedMemoryWord {
        let offset = word_offset(address);
        let write_mask = match endianness {
            Mips1Endianness::Big => match offset {
                0 => 0xff00_0000,
                1 => 0xffff_0000,
                2 => 0xffff_ff00,
                _ => 0xffff_ffff,
            },
            Mips1Endianness::Little => match offset {
                0 => 0xffff_ffff,
                1 => 0xffff_ff00,
                2 => 0xffff_0000,
                _ => 0xff00_0000,
            },
        };
        let write_value = match endianness {
            Mips1Endianness::Big => match offset {
                0 => register_value << 24,
                1 => register_value << 16,
                2 => register_value << 8,
                _ => register_value,
            },
            Mips1Endianness::Little => match offset {
                0 => register_value,
                1 => register_value << 8,
                2 => register_value << 16,
                _ => register_value << 24,
            },
        };

        masked_word(memory_word, write_value, write_mask)
    }
}

impl Mips1MemoryAccessSize {
    const fn is_aligned(self, address: u32) -> bool {
        match self {
            Self::Byte => true,
            Self::Halfword => address & 0x1 == 0,
            Self::Word => address & 0x3 == 0,
        }
    }
}

impl Mips1MemoryAccessKind {
    const fn is_store(self) -> bool {
        matches!(
            self,
            Self::Store { .. } | Self::StoreWordLeft | Self::StoreWordRight
        )
    }
}

const fn word_offset(address: u32) -> u32 {
    address & 0x3
}

const fn masked_word(memory_word: u32, write_value: u32, write_mask: u32) -> Mips1MaskedMemoryWord {
    Mips1MaskedMemoryWord {
        value: (memory_word & !write_mask) | (write_value & write_mask),
        write_mask,
    }
}

#[cfg(test)]
mod tests;
