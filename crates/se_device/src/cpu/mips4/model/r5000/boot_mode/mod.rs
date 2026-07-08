//! R5000 boot-mode serial stream parsing.
//!
//! Power-on and cold reset sample a 256-bit serial boot-mode stream through the
//! initialization interface before execution starts. This module parses the raw
//! stream and exposes documented mode fields without modeling reset timing,
//! board wiring, CP0 state updates, or secondary-cache presence policy.

use crate::cpu::mips4::config::Mips4Endianness;

/// Number of bits in the boot-mode serial stream.
pub const R5000_BOOT_MODE_BIT_COUNT: usize = 256;

/// Last boot-mode bit with a documented R5000 setting.
pub const R5000_BOOT_MODE_LAST_DEFINED_BIT: usize = 38;

const FIELD_TRANSMIT_DATA_PATTERN_SHIFT: usize = 1;
const FIELD_SYS_CLOCK_RATIO_SHIFT: usize = 5;
const FIELD_NON_BLOCK_WRITE_SHIFT: usize = 9;
const FIELD_DRIVER_SLEW_RATE_SHIFT: usize = 13;
const FIELD_SECONDARY_CACHE_SIZE_SHIFT: usize = 16;

const BIT_RESERVED_0: usize = 0;
const BIT_ENDIAN_MODE: usize = 8;
const BIT_TIMER_INTERRUPT_DISABLE: usize = 11;
const BIT_SECONDARY_CACHE_ENABLE: usize = 12;
const BIT_SECONDARY_CACHE_SRAM_PROTOCOL: usize = 15;
const BIT_COUNT_UPDATE_RATE: usize = 18;
const BIT_REV_2_41_WORKAROUND_20: usize = 20;
const BIT_REV_2_41_WORKAROUND_33: usize = 33;
const BIT_REV_2_X_WORKAROUND_37: usize = 37;
const BIT_R5000A_TWO_POINT_FIVE_MULTIPLIER: usize = 38;

const LOW_DEFINED_MASK: u64 = (1u64 << (R5000_BOOT_MODE_LAST_DEFINED_BIT + 1)) - 1;
const LOW_RESERVED_MASK: u64 =
    (1u64 << BIT_RESERVED_0) | (1u64 << 19) | (0x0fffu64 << 21) | (0x07u64 << 34);

/// Boot-mode field used in reserved-value diagnostics.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000BootModeField {
    /// System interface data rate for block writes.
    TransmitDataPattern,

    /// PClock to SysClock multiplier field.
    SysClockRatio,

    /// Non-block write handling field.
    NonBlockWrite,

    /// Secondary cache size field.
    SecondaryCacheSize,
}

/// Boot-mode stream validation error.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000BootModeError {
    /// A reserved stream bit was set.
    ReservedBitSet {
        /// Bit index in the 256-bit stream.
        bit: usize,
    },

    /// A decoded field used a reserved value.
    ReservedFieldValue {
        /// Field containing the reserved value.
        field: R5000BootModeField,

        /// Raw field value.
        value: u8,
    },

    /// The requested bit index is outside the 256-bit stream.
    BitIndexOutOfRange {
        /// Requested bit index.
        index: usize,
    },
}

/// System interface data rate for block writes.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000TransmitDataPattern {
    /// `DDDD`.
    Dddd,

    /// `DDxDDx`.
    DdxDdx,

    /// `DDxxDDxx`.
    DdxxDdxx,

    /// `DxDxDxDx`.
    Dxdxdxdx,

    /// `DDxxxDDxxx`.
    DdxxxDdxxx,

    /// `DDxxxxDDxxxx`.
    DdxxxxDdxxxx,

    /// `DxxDxxDxxDxx`.
    DxxDxxDxxDxx,

    /// `DDxxxxxxDDxxxxxx`.
    DdxxxxxxDdxxxxxx,

    /// `DxxxDxxxDxxxDxxx`.
    DxxxDxxxDxxxDxxx,
}

impl R5000TransmitDataPattern {
    /// Creates a transmit data pattern from its boot-mode field value.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Dddd),
            1 => Some(Self::DdxDdx),
            2 => Some(Self::DdxxDdxx),
            3 => Some(Self::Dxdxdxdx),
            4 => Some(Self::DdxxxDdxxx),
            5 => Some(Self::DdxxxxDdxxxx),
            6 => Some(Self::DxxDxxDxxDxx),
            7 => Some(Self::DdxxxxxxDdxxxxxx),
            8 => Some(Self::DxxxDxxxDxxxDxxx),
            _ => None,
        }
    }

    /// Returns the raw boot-mode field value.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Dddd => 0,
            Self::DdxDdx => 1,
            Self::DdxxDdxx => 2,
            Self::Dxdxdxdx => 3,
            Self::DdxxxDdxxx => 4,
            Self::DdxxxxDdxxxx => 5,
            Self::DxxDxxDxxDxx => 6,
            Self::DdxxxxxxDdxxxxxx => 7,
            Self::DxxxDxxxDxxxDxxx => 8,
        }
    }
}

/// PClock to SysClock multiplier selected by boot mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000ClockMultiplier {
    /// 2x SysClock.
    Times2,

    /// 2.5x SysClock.
    TimesTwoAndOneHalf,

    /// 3x SysClock.
    Times3,

    /// 4x SysClock.
    Times4,

    /// 5x SysClock.
    Times5,

    /// 6x SysClock.
    Times6,

    /// 7x SysClock.
    Times7,

    /// 8x SysClock.
    Times8,
}

impl R5000ClockMultiplier {
    /// Creates an integer clock multiplier from the SysCkRatio field value.
    pub const fn from_sys_clock_ratio_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Times2),
            1 => Some(Self::Times3),
            2 => Some(Self::Times4),
            3 => Some(Self::Times5),
            4 => Some(Self::Times6),
            5 => Some(Self::Times7),
            6 => Some(Self::Times8),
            _ => None,
        }
    }

    /// Returns the multiplier numerator.
    pub const fn numerator(self) -> u8 {
        match self {
            Self::Times2 => 2,
            Self::TimesTwoAndOneHalf => 5,
            Self::Times3 => 3,
            Self::Times4 => 4,
            Self::Times5 => 5,
            Self::Times6 => 6,
            Self::Times7 => 7,
            Self::Times8 => 8,
        }
    }

    /// Returns the multiplier denominator.
    pub const fn denominator(self) -> u8 {
        match self {
            Self::TimesTwoAndOneHalf => 2,
            _ => 1,
        }
    }
}

/// Non-block write handling selected by boot mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000NonBlockWriteMode {
    /// VR4x00-compatible handling.
    Vr4x00Compatible,

    /// Pipelined writes.
    PipelinedWrites,

    /// Write-reissue handling.
    WriteReissue,
}

impl R5000NonBlockWriteMode {
    /// Creates a non-block write mode from its boot-mode field value.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Vr4x00Compatible),
            2 => Some(Self::PipelinedWrites),
            3 => Some(Self::WriteReissue),
            _ => None,
        }
    }

    /// Returns the raw boot-mode field value.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Vr4x00Compatible => 0,
            Self::PipelinedWrites => 2,
            Self::WriteReissue => 3,
        }
    }
}

/// Output driver slew rate selected by boot mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000DriverSlewRate {
    /// 100%, fastest.
    Percent100,

    /// 83%.
    Percent83,

    /// 67%.
    Percent67,

    /// 50%, slowest.
    Percent50,
}

impl R5000DriverSlewRate {
    /// Creates an output driver slew rate from its boot-mode field value.
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0b10 => Self::Percent100,
            0b11 => Self::Percent83,
            0b00 => Self::Percent67,
            _ => Self::Percent50,
        }
    }

    /// Returns the raw boot-mode field value.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Percent100 => 0b10,
            Self::Percent83 => 0b11,
            Self::Percent67 => 0b00,
            Self::Percent50 => 0b01,
        }
    }
}

/// Secondary cache SRAM protocol selected by boot mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000SecondaryCacheSramProtocol {
    /// Pipelined SRAM protocol.
    Pipelined,

    /// Burst SRAM protocol.
    Burst,
}

/// Secondary cache size selected by boot mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000SecondaryCacheSize {
    /// 512 KiB secondary cache.
    Size512Kib,

    /// 1 MiB secondary cache.
    Size1Mib,

    /// 2 MiB secondary cache.
    Size2Mib,
}

impl R5000SecondaryCacheSize {
    /// Creates a secondary cache size from its boot-mode field value.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::Size512Kib),
            1 => Some(Self::Size1Mib),
            2 => Some(Self::Size2Mib),
            _ => None,
        }
    }

    /// Returns the raw boot-mode field value.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Size512Kib => 0,
            Self::Size1Mib => 1,
            Self::Size2Mib => 2,
        }
    }

    /// Returns the secondary cache size in bytes.
    pub const fn size_bytes(self) -> u32 {
        match self {
            Self::Size512Kib => 512 * 1024,
            Self::Size1Mib => 1024 * 1024,
            Self::Size2Mib => 2 * 1024 * 1024,
        }
    }
}

/// CP0 Count register update rate selected by boot mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum R5000CountUpdateRate {
    /// Count updates at one half of PClock.
    HalfPClock,

    /// Count updates at PClock.
    PClock,
}

/// Validated R5000 boot-mode serial stream.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct R5000BootMode {
    words_le: [u64; 4],
}

impl R5000BootMode {
    /// Creates a boot mode from the low stream bits.
    pub fn from_low_bits(bits: u64) -> Result<Self, R5000BootModeError> {
        Self::from_words_le([bits, 0, 0, 0])
    }

    /// Creates a boot mode from 256 stream bits stored little-endian by bit index.
    pub fn from_words_le(words_le: [u64; 4]) -> Result<Self, R5000BootModeError> {
        let boot_mode = Self { words_le };
        boot_mode.validate()?;
        Ok(boot_mode)
    }

    /// Returns the low 64 stream bits.
    pub const fn low_bits(self) -> u64 {
        self.words_le[0]
    }

    /// Returns the raw 256-bit stream words, little-endian by bit index.
    pub const fn words_le(self) -> [u64; 4] {
        self.words_le
    }

    /// Returns a stream bit by its documented bit index.
    pub const fn bit(self, index: usize) -> Result<bool, R5000BootModeError> {
        if index >= R5000_BOOT_MODE_BIT_COUNT {
            return Err(R5000BootModeError::BitIndexOutOfRange { index });
        }

        let word = self.words_le[index / 64];
        Ok(((word >> (index % 64)) & 1) != 0)
    }

    /// Returns the raw transmit data pattern field value.
    pub const fn transmit_data_pattern_bits(self) -> u8 {
        self.field_bits(FIELD_TRANSMIT_DATA_PATTERN_SHIFT, 4)
    }

    /// Returns the transmit data pattern.
    pub const fn transmit_data_pattern(self) -> R5000TransmitDataPattern {
        match R5000TransmitDataPattern::from_bits(self.transmit_data_pattern_bits()) {
            Some(pattern) => pattern,
            None => unreachable!(),
        }
    }

    /// Returns the raw SysCkRatio field value.
    pub const fn sys_clock_ratio_bits(self) -> u8 {
        self.field_bits(FIELD_SYS_CLOCK_RATIO_SHIFT, 3)
    }

    /// Returns the integer SysCkRatio multiplier, when the raw field is defined.
    pub const fn sys_clock_ratio_multiplier(self) -> Option<R5000ClockMultiplier> {
        R5000ClockMultiplier::from_sys_clock_ratio_bits(self.sys_clock_ratio_bits())
    }

    /// Returns whether the R5000A 2.5x clock multiplier override bit is set.
    pub const fn two_point_five_clock_multiplier_enabled(self) -> bool {
        self.bit_unchecked(BIT_R5000A_TWO_POINT_FIVE_MULTIPLIER)
    }

    /// Returns the effective PClock to SysClock multiplier.
    pub const fn effective_clock_multiplier(self) -> R5000ClockMultiplier {
        if self.two_point_five_clock_multiplier_enabled() {
            R5000ClockMultiplier::TimesTwoAndOneHalf
        } else {
            match self.sys_clock_ratio_multiplier() {
                Some(multiplier) => multiplier,
                None => unreachable!(),
            }
        }
    }

    /// Returns the effective byte order after ORing the mode bit with the pin.
    pub const fn endianness(self, big_endian_pin: bool) -> Mips4Endianness {
        if big_endian_pin || self.bit_unchecked(BIT_ENDIAN_MODE) {
            Mips4Endianness::Big
        } else {
            Mips4Endianness::Little
        }
    }

    /// Returns the raw non-block write field value.
    pub const fn non_block_write_bits(self) -> u8 {
        self.field_bits(FIELD_NON_BLOCK_WRITE_SHIFT, 2)
    }

    /// Returns the selected non-block write mode.
    pub const fn non_block_write_mode(self) -> R5000NonBlockWriteMode {
        match R5000NonBlockWriteMode::from_bits(self.non_block_write_bits()) {
            Some(mode) => mode,
            None => unreachable!(),
        }
    }

    /// Returns whether the timer interrupt is enabled on `Int*[5]`.
    pub const fn timer_interrupt_enabled(self) -> bool {
        !self.bit_unchecked(BIT_TIMER_INTERRUPT_DISABLE)
    }

    /// Returns whether the secondary cache enable bit is set.
    pub const fn secondary_cache_enabled(self) -> bool {
        self.bit_unchecked(BIT_SECONDARY_CACHE_ENABLE)
    }

    /// Returns the raw output driver slew rate field value.
    pub const fn driver_slew_rate_bits(self) -> u8 {
        self.field_bits(FIELD_DRIVER_SLEW_RATE_SHIFT, 2)
    }

    /// Returns the output driver slew rate.
    pub const fn driver_slew_rate(self) -> R5000DriverSlewRate {
        R5000DriverSlewRate::from_bits(self.driver_slew_rate_bits())
    }

    /// Returns the selected secondary cache SRAM protocol.
    pub const fn secondary_cache_sram_protocol(self) -> R5000SecondaryCacheSramProtocol {
        if self.bit_unchecked(BIT_SECONDARY_CACHE_SRAM_PROTOCOL) {
            R5000SecondaryCacheSramProtocol::Burst
        } else {
            R5000SecondaryCacheSramProtocol::Pipelined
        }
    }

    /// Returns the raw secondary cache size field value.
    pub const fn secondary_cache_size_bits(self) -> u8 {
        self.field_bits(FIELD_SECONDARY_CACHE_SIZE_SHIFT, 2)
    }

    /// Returns the selected secondary cache size.
    pub const fn secondary_cache_size(self) -> R5000SecondaryCacheSize {
        match R5000SecondaryCacheSize::from_bits(self.secondary_cache_size_bits()) {
            Some(size) => size,
            None => unreachable!(),
        }
    }

    /// Returns the CP0 Count register update rate.
    pub const fn count_update_rate(self) -> R5000CountUpdateRate {
        if self.bit_unchecked(BIT_COUNT_UPDATE_RATE) {
            R5000CountUpdateRate::PClock
        } else {
            R5000CountUpdateRate::HalfPClock
        }
    }

    /// Returns the legacy revision workaround bit at stream bit 20.
    pub const fn revision_2_41_or_lower_workaround_bit20(self) -> bool {
        self.bit_unchecked(BIT_REV_2_41_WORKAROUND_20)
    }

    /// Returns the legacy revision workaround bit at stream bit 33.
    pub const fn revision_2_41_or_lower_workaround_bit33(self) -> bool {
        self.bit_unchecked(BIT_REV_2_41_WORKAROUND_33)
    }

    /// Returns the legacy revision workaround bit at stream bit 37.
    pub const fn revision_2_x_or_lower_workaround_bit37(self) -> bool {
        self.bit_unchecked(BIT_REV_2_X_WORKAROUND_37)
    }

    fn validate(self) -> Result<(), R5000BootModeError> {
        if let Some(bit) = first_set_bit(self.low_bits() & LOW_RESERVED_MASK, 0) {
            return Err(R5000BootModeError::ReservedBitSet { bit });
        }

        if let Some(bit) = first_set_bit(self.low_bits() & !LOW_DEFINED_MASK, 0) {
            return Err(R5000BootModeError::ReservedBitSet { bit });
        }

        for word_index in 1..self.words_le.len() {
            if let Some(bit) = first_set_bit(self.words_le[word_index], word_index * 64) {
                return Err(R5000BootModeError::ReservedBitSet { bit });
            }
        }

        let transmit_data_pattern = self.transmit_data_pattern_bits();
        if R5000TransmitDataPattern::from_bits(transmit_data_pattern).is_none() {
            return Err(R5000BootModeError::ReservedFieldValue {
                field: R5000BootModeField::TransmitDataPattern,
                value: transmit_data_pattern,
            });
        }

        let sys_clock_ratio = self.sys_clock_ratio_bits();
        if sys_clock_ratio == 7 && !self.two_point_five_clock_multiplier_enabled() {
            return Err(R5000BootModeError::ReservedFieldValue {
                field: R5000BootModeField::SysClockRatio,
                value: sys_clock_ratio,
            });
        }

        let non_block_write = self.non_block_write_bits();
        if R5000NonBlockWriteMode::from_bits(non_block_write).is_none() {
            return Err(R5000BootModeError::ReservedFieldValue {
                field: R5000BootModeField::NonBlockWrite,
                value: non_block_write,
            });
        }

        let secondary_cache_size = self.secondary_cache_size_bits();
        if R5000SecondaryCacheSize::from_bits(secondary_cache_size).is_none() {
            return Err(R5000BootModeError::ReservedFieldValue {
                field: R5000BootModeField::SecondaryCacheSize,
                value: secondary_cache_size,
            });
        }

        Ok(())
    }

    const fn field_bits(self, shift: usize, width: usize) -> u8 {
        ((self.low_bits() >> shift) & ((1u64 << width) - 1)) as u8
    }

    const fn bit_unchecked(self, index: usize) -> bool {
        ((self.words_le[index / 64] >> (index % 64)) & 1) != 0
    }
}

fn first_set_bit(bits: u64, base: usize) -> Option<usize> {
    if bits == 0 {
        None
    } else {
        Some(base + bits.trailing_zeros() as usize)
    }
}

#[cfg(test)]
mod tests;
