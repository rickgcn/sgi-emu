//! Generic MIPS IV floating-point coprocessor primitives.
//!
//! This module models CP1 register numbers, raw floating-point general
//! registers, FPU control registers, FCSR field mapping, and glue between MIPS
//! IV FCSR state and raw-bit floating-point backends. It does not execute FPU
//! instructions, manage pipeline hazards, modify CP0 state, or model
//! processor-specific FPU restrictions.

pub mod decode;

use crate::cpu::mips4::exception::Mips4Exception;
use crate::cpu::mips4::instruction::Mips4Instruction;
use se_float::control::{FloatControl, FloatExceptionFlags, FloatRoundingMode, FloatTininessMode};

/// Primary opcode for the `COP1` instruction class.
pub const MIPS4_COP1_OPCODE: u8 = 0x11;

/// Primary opcode for the `COP1X` instruction class.
pub const MIPS4_COP1X_OPCODE: u8 = 0x13;

/// Primary opcode for `LWC1`.
pub const MIPS4_LWC1_OPCODE: u8 = 0x31;

/// Primary opcode for `LDC1`.
pub const MIPS4_LDC1_OPCODE: u8 = 0x35;

/// Primary opcode for `SWC1`.
pub const MIPS4_SWC1_OPCODE: u8 = 0x39;

/// Primary opcode for `SDC1`.
pub const MIPS4_SDC1_OPCODE: u8 = 0x3d;

const MIPS4_CP1_FGR_COUNT: usize = 32;

const FCSR_RM_MASK: u32 = 0x0000_0003;
const FCSR_FLAG_SHIFT: u8 = 2;
const FCSR_ENABLE_SHIFT: u8 = 7;
const FCSR_CAUSE_SHIFT: u8 = 12;
const FCSR_CAUSE_UNIMPLEMENTED: u32 = 1 << 17;
const FCSR_FCC0: u32 = 1 << 23;
const FCSR_FS: u32 = 1 << 24;
const FCSR_FCC7_TO_FCC1_MASK: u32 = 0xfe00_0000;
const FCSR_IEEE_FIELD_MASK: u32 = 0x1f;
const FCSR_FLAG_MASK: u32 = FCSR_IEEE_FIELD_MASK << FCSR_FLAG_SHIFT;
const FCSR_ENABLE_MASK: u32 = FCSR_IEEE_FIELD_MASK << FCSR_ENABLE_SHIFT;
const FCSR_CAUSE_IEEE_MASK: u32 = FCSR_IEEE_FIELD_MASK << FCSR_CAUSE_SHIFT;
const FCSR_CAUSE_MASK: u32 = FCSR_CAUSE_UNIMPLEMENTED | FCSR_CAUSE_IEEE_MASK;
const FCSR_READABLE_MASK: u32 = FCSR_FCC7_TO_FCC1_MASK
    | FCSR_FS
    | FCSR_FCC0
    | FCSR_CAUSE_MASK
    | FCSR_ENABLE_MASK
    | FCSR_FLAG_MASK
    | FCSR_RM_MASK;

const FCR0_READABLE_MASK: u32 = 0x0000_ffff;

const CP1_FMT_BRANCH: u8 = 0x08;
const CP1_FMT_SINGLE: u8 = 0x10;
const CP1_FMT_DOUBLE: u8 = 0x11;
const CP1_FMT_WORD: u8 = 0x14;
const CP1_FMT_LONG: u8 = 0x15;

/// CP1 floating-point register model selected by privileged CPU state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Cp1RegisterMode {
    /// 32-bit FGR model with 16 doubleword operand registers.
    ThirtyTwoBit,

    /// 64-bit FGR model with 32 doubleword operand registers.
    SixtyFourBit,
}

/// CP1 floating-point general register index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4Cp1FgrIndex(u8);

impl Mips4Cp1FgrIndex {
    /// Creates a FGR index from a raw instruction field value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        if value < MIPS4_CP1_FGR_COUNT as u8 {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Returns the raw FGR number.
    pub const fn number(self) -> u8 {
        self.0
    }

    const fn is_even(self) -> bool {
        self.0 & 1 == 0
    }
}

/// CP1 condition code index.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4Cp1ConditionCode(u8);

impl Mips4Cp1ConditionCode {
    /// Creates a condition code index from a raw field value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        if value < 8 { Some(Self(value)) } else { None }
    }

    /// Returns the raw condition code number.
    pub const fn number(self) -> u8 {
        self.0
    }
}

/// CP1 formatted operand type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Cp1Format {
    /// Single-precision floating-point format.
    Single,

    /// Double-precision floating-point format.
    Double,

    /// 32-bit signed fixed-point format.
    Word,

    /// 64-bit signed fixed-point format.
    Long,
}

impl Mips4Cp1Format {
    /// Creates a CP1 format from the raw `fmt` field.
    pub const fn from_fmt_field(value: u8) -> Option<Self> {
        match value {
            CP1_FMT_SINGLE => Some(Self::Single),
            CP1_FMT_DOUBLE => Some(Self::Double),
            CP1_FMT_WORD => Some(Self::Word),
            CP1_FMT_LONG => Some(Self::Long),
            _ => None,
        }
    }

    /// Returns the raw `fmt` field value.
    pub const fn fmt_field(self) -> u8 {
        match self {
            Self::Single => CP1_FMT_SINGLE,
            Self::Double => CP1_FMT_DOUBLE,
            Self::Word => CP1_FMT_WORD,
            Self::Long => CP1_FMT_LONG,
        }
    }
}

/// CP1 FGR access error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Cp1FgrAccessError {
    /// A doubleword operand register used an odd FGR in 32-bit register mode.
    OddRegisterInThirtyTwoBitMode {
        /// Register requested by the access.
        index: Mips4Cp1FgrIndex,
    },
}

/// Raw CP1 floating-point general register file.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4Cp1FgrFile {
    registers: [u64; MIPS4_CP1_FGR_COUNT],
}

impl Mips4Cp1FgrFile {
    /// Creates a zeroed CP1 FGR file.
    pub const fn new() -> Self {
        Self {
            registers: [0; MIPS4_CP1_FGR_COUNT],
        }
    }

    /// Clears every FGR to zero.
    pub fn reset(&mut self) {
        self.registers = [0; MIPS4_CP1_FGR_COUNT];
    }

    /// Reads the low 32-bit word payload from an FGR.
    pub const fn read_word(&self, index: Mips4Cp1FgrIndex) -> u32 {
        self.registers[index.0 as usize] as u32
    }

    /// Writes a 32-bit word payload into an FGR.
    pub fn write_word(&mut self, index: Mips4Cp1FgrIndex, value: u32) {
        self.registers[index.0 as usize] = value as u64;
    }

    /// Reads a raw 64-bit FPR payload using the selected register model.
    pub const fn read_doubleword(
        &self,
        mode: Mips4Cp1RegisterMode,
        index: Mips4Cp1FgrIndex,
    ) -> Result<u64, Mips4Cp1FgrAccessError> {
        match self.doubleword_indices(mode, index) {
            Ok((low, Some(high))) => {
                Ok((self.registers[low] & 0xffff_ffff)
                    | ((self.registers[high] & 0xffff_ffff) << 32))
            }
            Ok((single, None)) => Ok(self.registers[single]),
            Err(error) => Err(error),
        }
    }

    /// Writes a raw 64-bit FPR payload using the selected register model.
    pub fn write_doubleword(
        &mut self,
        mode: Mips4Cp1RegisterMode,
        index: Mips4Cp1FgrIndex,
        value: u64,
    ) -> Result<(), Mips4Cp1FgrAccessError> {
        match self.doubleword_indices(mode, index) {
            Ok((low, Some(high))) => {
                self.registers[low] = value & 0xffff_ffff;
                self.registers[high] = value >> 32;
                Ok(())
            }
            Ok((single, None)) => {
                self.registers[single] = value;
                Ok(())
            }
            Err(error) => Err(error),
        }
    }

    const fn doubleword_indices(
        &self,
        mode: Mips4Cp1RegisterMode,
        index: Mips4Cp1FgrIndex,
    ) -> Result<(usize, Option<usize>), Mips4Cp1FgrAccessError> {
        match mode {
            Mips4Cp1RegisterMode::ThirtyTwoBit => {
                if index.is_even() {
                    Ok((index.0 as usize, Some(index.0 as usize + 1)))
                } else {
                    Err(Mips4Cp1FgrAccessError::OddRegisterInThirtyTwoBitMode { index })
                }
            }
            Mips4Cp1RegisterMode::SixtyFourBit => Ok((index.0 as usize, None)),
        }
    }
}

impl Default for Mips4Cp1FgrFile {
    fn default() -> Self {
        Self::new()
    }
}

/// CP1 control register number.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips4Cp1ControlRegister {
    /// FPU implementation and revision register.
    ImplementationRevision,

    /// FPU control and status register.
    ControlStatus,
}

impl Mips4Cp1ControlRegister {
    /// Creates a CP1 control register number from a raw instruction field value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::ImplementationRevision),
            31 => Some(Self::ControlStatus),
            _ => None,
        }
    }

    /// Returns the raw CP1 control register number.
    pub const fn number(self) -> u8 {
        match self {
            Self::ImplementationRevision => 0,
            Self::ControlStatus => 31,
        }
    }
}

/// CP1 control register write error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Cp1WriteError {
    /// Public writes cannot modify this CP1 control register.
    ReadOnlyControlRegister {
        /// Register rejected by the write path.
        register: Mips4Cp1ControlRegister,
    },
}

/// CP1 implementation and revision register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4Cp1Fcr0(u32);

impl Mips4Cp1Fcr0 {
    /// Creates an implementation and revision register wrapper.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & FCR0_READABLE_MASK)
    }

    /// Returns the raw readable bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the implementation field.
    pub const fn implementation(self) -> u8 {
        ((self.0 >> 8) & 0xff) as u8
    }

    /// Returns the revision field.
    pub const fn revision(self) -> u8 {
        (self.0 & 0xff) as u8
    }
}

/// MIPS IV FCSR rounding mode.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Mips4Cp1RoundingMode {
    /// Round to nearest representable value, choosing an even low bit on ties.
    #[default]
    RoundToNearest,

    /// Round toward zero.
    RoundTowardZero,

    /// Round toward positive infinity.
    RoundTowardPositive,

    /// Round toward negative infinity.
    RoundTowardNegative,
}

impl Mips4Cp1RoundingMode {
    /// Creates a rounding mode from raw FCSR `RM` bits.
    pub const fn from_bits(bits: u8) -> Option<Self> {
        match bits {
            0 => Some(Self::RoundToNearest),
            1 => Some(Self::RoundTowardZero),
            2 => Some(Self::RoundTowardPositive),
            3 => Some(Self::RoundTowardNegative),
            _ => None,
        }
    }

    /// Returns raw FCSR `RM` bits.
    pub const fn bits(self) -> u8 {
        match self {
            Self::RoundToNearest => 0,
            Self::RoundTowardZero => 1,
            Self::RoundTowardPositive => 2,
            Self::RoundTowardNegative => 3,
        }
    }

    /// Converts this MIPS IV rounding mode to backend control.
    pub const fn to_float_rounding_mode(self) -> FloatRoundingMode {
        match self {
            Self::RoundToNearest => FloatRoundingMode::NearestEven,
            Self::RoundTowardZero => FloatRoundingMode::TowardZero,
            Self::RoundTowardPositive => FloatRoundingMode::TowardPositive,
            Self::RoundTowardNegative => FloatRoundingMode::TowardNegative,
        }
    }

    /// Creates backend control with MIPS IV tininess detection.
    pub const fn to_float_control(self) -> FloatControl {
        FloatControl::new(
            self.to_float_rounding_mode(),
            FloatTininessMode::AfterRounding,
        )
    }
}

/// Rounding source for MIPS IV floating-point conversion operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips4Cp1ConversionRoundingMode {
    /// Use the active FCSR rounding mode.
    Fcsr,

    /// Use `ROUND.W/L.fmt` nearest-even rounding.
    Round,

    /// Use `TRUNC.W/L.fmt` toward-zero rounding.
    Trunc,

    /// Use `CEIL.W/L.fmt` toward-positive rounding.
    Ceil,

    /// Use `FLOOR.W/L.fmt` toward-negative rounding.
    Floor,
}

impl Mips4Cp1ConversionRoundingMode {
    /// Resolves the conversion rounding mode against the current FCSR state.
    pub const fn rounding_mode(self, fcsr: Mips4Cp1Fcsr) -> Mips4Cp1RoundingMode {
        match self {
            Self::Fcsr => fcsr.rounding_mode(),
            Self::Round => Mips4Cp1RoundingMode::RoundToNearest,
            Self::Trunc => Mips4Cp1RoundingMode::RoundTowardZero,
            Self::Ceil => Mips4Cp1RoundingMode::RoundTowardPositive,
            Self::Floor => Mips4Cp1RoundingMode::RoundTowardNegative,
        }
    }

    /// Creates backend control for this conversion rounding source.
    pub const fn float_control(self, fcsr: Mips4Cp1Fcsr) -> FloatControl {
        self.rounding_mode(fcsr).to_float_control()
    }
}

/// CP1 floating-point control and status register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips4Cp1Fcsr(u32);

impl Mips4Cp1Fcsr {
    /// Creates an FCSR wrapper, discarding reserved bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & FCSR_READABLE_MASK)
    }

    /// Returns the raw readable FCSR bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the active FCSR rounding mode.
    pub const fn rounding_mode(self) -> Mips4Cp1RoundingMode {
        match self.0 & FCSR_RM_MASK {
            0 => Mips4Cp1RoundingMode::RoundToNearest,
            1 => Mips4Cp1RoundingMode::RoundTowardZero,
            2 => Mips4Cp1RoundingMode::RoundTowardPositive,
            _ => Mips4Cp1RoundingMode::RoundTowardNegative,
        }
    }

    /// Sets the active FCSR rounding mode.
    pub fn set_rounding_mode(&mut self, rounding_mode: Mips4Cp1RoundingMode) {
        self.0 = (self.0 & !FCSR_RM_MASK) | rounding_mode.bits() as u32;
    }

    /// Returns backend control for normal operations using FCSR `RM`.
    pub const fn float_control(self) -> FloatControl {
        self.rounding_mode().to_float_control()
    }

    /// Returns backend control for a conversion rounding source.
    pub const fn conversion_float_control(
        self,
        rounding_mode: Mips4Cp1ConversionRoundingMode,
    ) -> FloatControl {
        rounding_mode.float_control(self)
    }

    /// Returns whether flush-to-zero mode is selected.
    pub const fn flush_to_zero(self) -> bool {
        (self.0 & FCSR_FS) != 0
    }

    /// Sets or clears flush-to-zero mode.
    pub fn set_flush_to_zero(&mut self, enabled: bool) {
        self.set_bit(FCSR_FS, enabled);
    }

    /// Returns one floating-point condition code bit.
    pub const fn condition_code(self, condition_code: Mips4Cp1ConditionCode) -> bool {
        (self.0 & condition_code_mask(condition_code)) != 0
    }

    /// Sets or clears one floating-point condition code bit.
    pub fn set_condition_code(&mut self, condition_code: Mips4Cp1ConditionCode, value: bool) {
        self.set_bit(condition_code_mask(condition_code), value);
    }

    /// Returns whether the unimplemented-operation cause bit is set.
    pub const fn unimplemented_operation_cause(self) -> bool {
        (self.0 & FCSR_CAUSE_UNIMPLEMENTED) != 0
    }

    /// Returns IEEE exception cause bits.
    pub const fn cause_flags(self) -> FloatExceptionFlags {
        FloatExceptionFlags::from_bits_truncate(
            ((self.0 & FCSR_CAUSE_IEEE_MASK) >> FCSR_CAUSE_SHIFT) as u8,
        )
    }

    /// Returns IEEE exception enable bits.
    pub const fn enable_flags(self) -> FloatExceptionFlags {
        FloatExceptionFlags::from_bits_truncate(
            ((self.0 & FCSR_ENABLE_MASK) >> FCSR_ENABLE_SHIFT) as u8,
        )
    }

    /// Sets IEEE exception enable bits.
    pub fn set_enable_flags(&mut self, flags: FloatExceptionFlags) {
        self.0 = (self.0 & !FCSR_ENABLE_MASK)
            | ((flags.bits() as u32 & FCSR_IEEE_FIELD_MASK) << FCSR_ENABLE_SHIFT);
    }

    /// Returns sticky IEEE exception flag bits.
    pub const fn flag_flags(self) -> FloatExceptionFlags {
        FloatExceptionFlags::from_bits_truncate(
            ((self.0 & FCSR_FLAG_MASK) >> FCSR_FLAG_SHIFT) as u8,
        )
    }

    /// Sets sticky IEEE exception flag bits.
    pub fn set_flag_flags(&mut self, flags: FloatExceptionFlags) {
        self.0 = (self.0 & !FCSR_FLAG_MASK)
            | ((flags.bits() as u32 & FCSR_IEEE_FIELD_MASK) << FCSR_FLAG_SHIFT);
    }

    /// Records IEEE exception flags from one backend operation.
    pub fn record_float_flags(&mut self, flags: FloatExceptionFlags) -> Result<(), Mips4Exception> {
        self.replace_cause(flags);

        if self.enable_flags().bits() & flags.bits() != 0 {
            Err(Mips4Exception::FloatingPoint)
        } else {
            self.set_flag_flags(self.flag_flags().union(flags));
            Ok(())
        }
    }

    /// Records an unimplemented-operation cause.
    pub fn record_unimplemented_operation(&mut self) -> Mips4Exception {
        self.0 = (self.0 & !FCSR_CAUSE_MASK) | FCSR_CAUSE_UNIMPLEMENTED;
        Mips4Exception::FloatingPoint
    }

    fn replace_cause(&mut self, flags: FloatExceptionFlags) {
        self.0 = (self.0 & !FCSR_CAUSE_MASK)
            | ((flags.bits() as u32 & FCSR_IEEE_FIELD_MASK) << FCSR_CAUSE_SHIFT);
    }

    fn set_bit(&mut self, mask: u32, value: bool) {
        if value {
            self.0 |= mask;
        } else {
            self.0 &= !mask;
        }
    }
}

/// CP1 register file and control state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Mips4Cp1 {
    fgr: Mips4Cp1FgrFile,
    fcr0: Mips4Cp1Fcr0,
    fcsr: Mips4Cp1Fcsr,
}

impl Mips4Cp1 {
    /// Creates CP1 state with zeroed FGRs and FCSR.
    pub const fn new(implementation_revision: u32) -> Self {
        Self {
            fgr: Mips4Cp1FgrFile::new(),
            fcr0: Mips4Cp1Fcr0::from_bits(implementation_revision),
            fcsr: Mips4Cp1Fcsr::from_bits(0),
        }
    }

    /// Returns immutable FGR access.
    pub const fn fgr(&self) -> &Mips4Cp1FgrFile {
        &self.fgr
    }

    /// Returns mutable FGR access.
    pub fn fgr_mut(&mut self) -> &mut Mips4Cp1FgrFile {
        &mut self.fgr
    }

    /// Returns FCR0.
    pub const fn fcr0(&self) -> Mips4Cp1Fcr0 {
        self.fcr0
    }

    /// Returns FCSR.
    pub const fn fcsr(&self) -> Mips4Cp1Fcsr {
        self.fcsr
    }

    /// Returns mutable FCSR access.
    pub fn fcsr_mut(&mut self) -> &mut Mips4Cp1Fcsr {
        &mut self.fcsr
    }

    /// Reads a CP1 control register.
    pub const fn read_control(&self, register: Mips4Cp1ControlRegister) -> u32 {
        match register {
            Mips4Cp1ControlRegister::ImplementationRevision => self.fcr0.bits(),
            Mips4Cp1ControlRegister::ControlStatus => self.fcsr.bits(),
        }
    }

    /// Writes a CP1 control register through the public write path.
    pub fn write_control(
        &mut self,
        register: Mips4Cp1ControlRegister,
        value: u32,
    ) -> Result<(), Mips4Cp1WriteError> {
        match register {
            Mips4Cp1ControlRegister::ImplementationRevision => {
                Err(Mips4Cp1WriteError::ReadOnlyControlRegister { register })
            }
            Mips4Cp1ControlRegister::ControlStatus => {
                self.fcsr = Mips4Cp1Fcsr::from_bits(value);
                Ok(())
            }
        }
    }
}

/// Raw fields of a MIPS IV CP1-related instruction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips4Cp1Instruction {
    instruction: Mips4Instruction,
}

impl Mips4Cp1Instruction {
    /// Creates a CP1 instruction wrapper from a raw instruction word.
    pub const fn from_bits(bits: u32) -> Option<Self> {
        Self::from_instruction(Mips4Instruction::from_bits(bits))
    }

    /// Creates a CP1 instruction wrapper for CP1-related primary opcodes.
    pub const fn from_instruction(instruction: Mips4Instruction) -> Option<Self> {
        match instruction.opcode() {
            MIPS4_COP1_OPCODE | MIPS4_COP1X_OPCODE | MIPS4_LWC1_OPCODE | MIPS4_LDC1_OPCODE
            | MIPS4_SWC1_OPCODE | MIPS4_SDC1_OPCODE => Some(Self { instruction }),
            _ => None,
        }
    }

    /// Returns the wrapped instruction.
    pub const fn instruction(self) -> Mips4Instruction {
        self.instruction
    }

    /// Returns the raw instruction bits.
    pub const fn bits(self) -> u32 {
        self.instruction.bits()
    }

    /// Returns the primary opcode field.
    pub const fn opcode(self) -> u8 {
        self.instruction.opcode()
    }

    /// Returns the CP1 `fmt` field.
    pub const fn fmt(self) -> u8 {
        self.instruction.fmt()
    }

    /// Returns the CP1 `ft` field.
    pub const fn ft(self) -> u8 {
        self.instruction.ft()
    }

    /// Returns the CP1 `fs` field.
    pub const fn fs(self) -> u8 {
        self.instruction.fs()
    }

    /// Returns the CP1 `fd` field.
    pub const fn fd(self) -> u8 {
        self.instruction.fd()
    }

    /// Returns the CP1 function field.
    pub const fn funct(self) -> u8 {
        self.instruction.funct()
    }

    /// Returns the base register for CP1 offset load/store instructions.
    pub const fn base(self) -> u8 {
        self.instruction.rs()
    }

    /// Returns the signed offset for CP1 offset load/store instructions.
    pub const fn offset(self) -> i16 {
        self.instruction.signed_immediate()
    }

    /// Returns whether this instruction uses the `COP1` primary opcode.
    pub const fn is_cop1(self) -> bool {
        self.opcode() == MIPS4_COP1_OPCODE
    }

    /// Returns whether this instruction uses the `COP1X` primary opcode.
    pub const fn is_cop1x(self) -> bool {
        self.opcode() == MIPS4_COP1X_OPCODE
    }

    /// Returns whether this instruction uses the CP1 branch format.
    pub const fn is_branch_format(self) -> bool {
        self.is_cop1() && self.fmt() == CP1_FMT_BRANCH
    }

    /// Returns the raw CP1 branch condition-code field.
    pub const fn branch_condition_code_bits(self) -> u8 {
        (self.instruction.rt() >> 2) & 0x07
    }

    /// Returns the CP1 branch nullify-delay-slot bit.
    pub const fn branch_nullify_delay_slot_bit(self) -> bool {
        (self.instruction.rt() & 0x02) != 0
    }

    /// Returns the CP1 branch true/false bit.
    pub const fn branch_true_bit(self) -> bool {
        (self.instruction.rt() & 0x01) != 0
    }

    /// Returns the condition-code bits used by formatted compare and move fields.
    pub const fn condition_code_bits(self) -> u8 {
        (self.fd() >> 2) & 0x07
    }

    /// Returns the true/false selector bit used by `MOVCF` and `MOVCI` encodings.
    pub const fn condition_true_bit(self) -> bool {
        (self.instruction.rt() & 0x01) != 0
    }
}

const fn condition_code_mask(condition_code: Mips4Cp1ConditionCode) -> u32 {
    if condition_code.0 == 0 {
        FCSR_FCC0
    } else {
        1 << (24 + condition_code.0)
    }
}

#[cfg(test)]
mod tests;
