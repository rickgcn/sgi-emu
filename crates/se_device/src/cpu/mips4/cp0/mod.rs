//! Generic MIPS IV system control coprocessor register primitives.
//!
//! This module models CP0 register numbers, raw register bit fields, typed
//! wrappers, exception-entry register transitions, and a minimal register file.
//! It does not execute CP0 instructions, manage TLB replacement, translate
//! addresses, or own implementation-specific cache diagnostic behavior.

use crate::cpu::mips4::exception::{
    Mips4CoprocessorNumber, Mips4ErrorException, Mips4ErrorExceptionImage, Mips4Exception,
    Mips4ExceptionImage,
};
use crate::cpu::mips4::tlb::{Mips4TlbAsid, Mips4TlbEntryHi, Mips4TlbEntryLo, Mips4TlbPageMask};

const INDEX_PROBE_FAILURE: u64 = 1 << 31;
const INDEX_INDEX_MASK: u64 = 0x3f;
const INDEX_READABLE_MASK: u64 = INDEX_PROBE_FAILURE | INDEX_INDEX_MASK;

const RANDOM_INDEX_MASK: u64 = 0x3f;
const WIRED_INDEX_MASK: u64 = 0x3f;

const ENTRY_LO_PFN_SHIFT: u8 = 6;
const ENTRY_LO_PFN_MASK: u64 = 0x00ff_ffff;
const ENTRY_LO_CCA_SHIFT: u8 = 3;
const ENTRY_LO_CCA_MASK: u64 = 0x07 << ENTRY_LO_CCA_SHIFT;
const ENTRY_LO_DIRTY: u64 = 1 << 2;
const ENTRY_LO_VALID: u64 = 1 << 1;
const ENTRY_LO_GLOBAL: u64 = 1;
const ENTRY_LO_READABLE_MASK: u64 = 0x3fff_ffff;

const PAGE_MASK_MASK: u64 = 0x01ff_e000;

const ENTRY_HI_REGION_SHIFT: u8 = 62;
const ENTRY_HI_REGION_MASK: u64 = 0x3;
const ENTRY_HI_VPN2_SHIFT: u8 = 13;
const ENTRY_HI_VPN2_MASK: u64 = 0x07ff_ffff;
const ENTRY_HI_ASID_MASK: u64 = 0xff;
const ENTRY_HI_READABLE_MASK: u64 = (ENTRY_HI_REGION_MASK << ENTRY_HI_REGION_SHIFT)
    | (ENTRY_HI_VPN2_MASK << ENTRY_HI_VPN2_SHIFT)
    | ENTRY_HI_ASID_MASK;

const CONTEXT_BAD_VPN2_SHIFT: u8 = 4;
const CONTEXT_BAD_VPN2_MASK: u64 = 0x0007_ffff;
const CONTEXT_PTE_BASE_SHIFT: u8 = 23;
const CONTEXT_READABLE_MASK: u64 = !0x0f;

const XCONTEXT_BAD_VPN2_SHIFT: u8 = 4;
const XCONTEXT_BAD_VPN2_MASK: u64 = 0x07ff_ffff;
const XCONTEXT_REGION_SHIFT: u8 = 31;
const XCONTEXT_REGION_MASK: u64 = 0x03;
const XCONTEXT_PTE_BASE_SHIFT: u8 = 33;
const XCONTEXT_READABLE_MASK: u64 = !0x0f;

const STATUS_XX: u32 = 1 << 31;
const STATUS_CU_SHIFT: u8 = 28;
const STATUS_CU_MASK: u32 = 0x7000_0000;
const STATUS_FR: u32 = 1 << 26;
const STATUS_RE: u32 = 1 << 25;
const STATUS_BEV: u32 = 1 << 22;
const STATUS_SR: u32 = 1 << 20;
const STATUS_CE: u32 = 1 << 17;
const STATUS_DE: u32 = 1 << 16;
const STATUS_IM_SHIFT: u8 = 8;
const STATUS_IM_MASK: u32 = 0x0000_ff00;
const STATUS_KX: u32 = 1 << 7;
const STATUS_SX: u32 = 1 << 6;
const STATUS_UX: u32 = 1 << 5;
const STATUS_KSU_SHIFT: u8 = 3;
const STATUS_KSU_MASK: u32 = 0x0000_0018;
const STATUS_ERL: u32 = 1 << 2;
const STATUS_EXL: u32 = 1 << 1;
const STATUS_IE: u32 = 1;
const STATUS_READABLE_MASK: u32 = STATUS_CU_MASK
    | STATUS_XX
    | STATUS_FR
    | STATUS_RE
    | STATUS_BEV
    | STATUS_SR
    | STATUS_CE
    | STATUS_DE
    | STATUS_IM_MASK
    | STATUS_KX
    | STATUS_SX
    | STATUS_UX
    | STATUS_KSU_MASK
    | STATUS_ERL
    | STATUS_EXL
    | STATUS_IE;

const CAUSE_BD: u32 = 1 << 31;
const CAUSE_CE_SHIFT: u8 = 28;
const CAUSE_CE_MASK: u32 = 0x3000_0000;
const CAUSE_IP_SHIFT: u8 = 8;
const CAUSE_IP_MASK: u32 = 0x0000_ff00;
const CAUSE_SOFTWARE_IP_MASK: u32 = 0x0000_0300;
const CAUSE_TIMER_IP: u32 = 1 << 15;
const CAUSE_EXC_CODE_SHIFT: u8 = 2;
const CAUSE_EXC_CODE_MASK: u32 = 0x0000_007c;
const CAUSE_READABLE_MASK: u32 = CAUSE_BD | CAUSE_CE_MASK | CAUSE_IP_MASK | CAUSE_EXC_CODE_MASK;

const PROCESSOR_ID_READABLE_MASK: u32 = 0x0000_ffff;
const ECC_READABLE_MASK: u32 = 0x0000_00ff;
const WATCH_LO_READABLE_MASK: u32 = 0xffff_fffb;
const WATCH_HI_READABLE_MASK: u32 = 0x0000_00ff;
const CACHE_ERR_DATA_REFERENCE: u32 = 1 << 31;
const CACHE_ERR_CACHE_LEVEL: u32 = 1 << 30;
const CACHE_ERR_DATA_FIELD: u32 = 1 << 29;
const CACHE_ERR_TAG_FIELD: u32 = 1 << 28;
const CACHE_ERR_SYSTEM_BUS: u32 = 1 << 25;
const CACHE_ERR_ADDITIONAL_DATA: u32 = 1 << 24;
const CACHE_ERR_FILL_ON_STORE_MISS: u32 = 1 << 23;
const CACHE_ERR_PHYSICAL_INDEX_SHIFT: u8 = 3;
const CACHE_ERR_PHYSICAL_INDEX_MASK: u32 = 0x0007_ffff;
const CACHE_ERR_VIRTUAL_INDEX_MASK: u32 = 0x3;
const CACHE_ERR_READABLE_MASK: u32 = CACHE_ERR_DATA_REFERENCE
    | CACHE_ERR_CACHE_LEVEL
    | CACHE_ERR_DATA_FIELD
    | CACHE_ERR_TAG_FIELD
    | CACHE_ERR_SYSTEM_BUS
    | CACHE_ERR_ADDITIONAL_DATA
    | CACHE_ERR_FILL_ON_STORE_MISS
    | (CACHE_ERR_PHYSICAL_INDEX_MASK << CACHE_ERR_PHYSICAL_INDEX_SHIFT)
    | CACHE_ERR_VIRTUAL_INDEX_MASK;

/// MIPS IV CP0 register number.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4Cp0Register {
    /// TLB index register.
    Index,

    /// TLB random replacement register.
    Random,

    /// Even-page TLB low entry register.
    EntryLo0,

    /// Odd-page TLB low entry register.
    EntryLo1,

    /// TLB refill context register.
    Context,

    /// TLB page mask register.
    PageMask,

    /// TLB wired boundary register.
    Wired,

    /// Bad virtual address register.
    BadVaddr,

    /// Timer count register.
    Count,

    /// TLB high entry register.
    EntryHi,

    /// Timer compare register.
    Compare,

    /// Status register.
    Status,

    /// Cause register.
    Cause,

    /// Exception program counter register.
    Epc,

    /// Processor revision identifier register.
    ProcessorId,

    /// Processor configuration register.
    Config,

    /// Load-linked address register.
    LlAddr,

    /// Physical watchpoint address low register.
    WatchLo,

    /// Physical watchpoint address high register.
    WatchHi,

    /// Extended TLB refill context register.
    XContext,

    /// Cache ECC/parity register.
    Ecc,

    /// Cache error register.
    CacheErr,

    /// Cache tag low register.
    TagLo,

    /// Cache tag high register.
    TagHi,

    /// Error exception program counter register.
    ErrorEpc,
}

impl Mips4Cp0Register {
    /// Creates a CP0 register number from its raw instruction field value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Index),
            1 => Some(Self::Random),
            2 => Some(Self::EntryLo0),
            3 => Some(Self::EntryLo1),
            4 => Some(Self::Context),
            5 => Some(Self::PageMask),
            6 => Some(Self::Wired),
            8 => Some(Self::BadVaddr),
            9 => Some(Self::Count),
            10 => Some(Self::EntryHi),
            11 => Some(Self::Compare),
            12 => Some(Self::Status),
            13 => Some(Self::Cause),
            14 => Some(Self::Epc),
            15 => Some(Self::ProcessorId),
            16 => Some(Self::Config),
            17 => Some(Self::LlAddr),
            18 => Some(Self::WatchLo),
            19 => Some(Self::WatchHi),
            20 => Some(Self::XContext),
            26 => Some(Self::Ecc),
            27 => Some(Self::CacheErr),
            28 => Some(Self::TagLo),
            29 => Some(Self::TagHi),
            30 => Some(Self::ErrorEpc),
            _ => None,
        }
    }

    /// Returns the raw CP0 register number.
    pub const fn number(self) -> u8 {
        match self {
            Self::Index => 0,
            Self::Random => 1,
            Self::EntryLo0 => 2,
            Self::EntryLo1 => 3,
            Self::Context => 4,
            Self::PageMask => 5,
            Self::Wired => 6,
            Self::BadVaddr => 8,
            Self::Count => 9,
            Self::EntryHi => 10,
            Self::Compare => 11,
            Self::Status => 12,
            Self::Cause => 13,
            Self::Epc => 14,
            Self::ProcessorId => 15,
            Self::Config => 16,
            Self::LlAddr => 17,
            Self::WatchLo => 18,
            Self::WatchHi => 19,
            Self::XContext => 20,
            Self::Ecc => 26,
            Self::CacheErr => 27,
            Self::TagLo => 28,
            Self::TagHi => 29,
            Self::ErrorEpc => 30,
        }
    }
}

/// Error returned by CP0 register writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Mips4Cp0WriteError {
    /// The selected CP0 register is read-only through the public register file.
    ReadOnlyRegister {
        /// Read-only register selected by the write.
        register: Mips4Cp0Register,
    },
}

impl Mips4Cp0WriteError {
    /// Returns the register rejected by the write.
    pub const fn register(self) -> Mips4Cp0Register {
        match self {
            Self::ReadOnlyRegister { register } => register,
        }
    }
}

/// CP0 `Index` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Index(u32);

impl Mips4Cp0Index {
    /// Creates an `Index` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self((bits & INDEX_READABLE_MASK) as u32)
    }

    /// Returns the raw `Index` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the TLB-probe failure bit.
    pub const fn probe_failure(self) -> bool {
        self.0 & (INDEX_PROBE_FAILURE as u32) != 0
    }

    /// Returns the selected TLB index.
    pub const fn index(self) -> u8 {
        (self.0 & (INDEX_INDEX_MASK as u32)) as u8
    }
}

/// CP0 `Random` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Random(u32);

impl Mips4Cp0Random {
    /// Creates a `Random` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self((bits & RANDOM_INDEX_MASK) as u32)
    }

    /// Returns the raw `Random` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the random TLB index.
    pub const fn index(self) -> u8 {
        (self.0 & (RANDOM_INDEX_MASK as u32)) as u8
    }
}

/// CP0 `Wired` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Wired(u32);

impl Mips4Cp0Wired {
    /// Creates a `Wired` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self((bits & WIRED_INDEX_MASK) as u32)
    }

    /// Returns the raw `Wired` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the wired TLB boundary.
    pub const fn boundary(self) -> u8 {
        (self.0 & (WIRED_INDEX_MASK as u32)) as u8
    }
}

/// CP0 `EntryLo0` or `EntryLo1` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0EntryLo(u32);

impl Mips4Cp0EntryLo {
    /// Creates an `EntryLo` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self((bits & ENTRY_LO_READABLE_MASK) as u32)
    }

    /// Creates an `EntryLo` register value from generic TLB fields.
    pub const fn from_tlb_entry_lo(entry_lo: Mips4TlbEntryLo) -> Self {
        Self(entry_lo.bits() as u32)
    }

    /// Returns the raw `EntryLo` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Converts this register value into generic TLB fields.
    pub const fn to_tlb_entry_lo(self) -> Option<Mips4TlbEntryLo> {
        Mips4TlbEntryLo::from_bits(self.0 as u64)
    }

    /// Returns the page frame number.
    pub const fn page_frame_number(self) -> u32 {
        ((self.0 as u64 >> ENTRY_LO_PFN_SHIFT) & ENTRY_LO_PFN_MASK) as u32
    }

    /// Returns the raw cache-coherence algorithm field.
    pub const fn cache_coherence_algorithm(self) -> u8 {
        ((self.0 as u64 & ENTRY_LO_CCA_MASK) >> ENTRY_LO_CCA_SHIFT) as u8
    }

    /// Returns whether this page is writable.
    pub const fn dirty(self) -> bool {
        self.0 as u64 & ENTRY_LO_DIRTY != 0
    }

    /// Returns whether this page is valid.
    pub const fn valid(self) -> bool {
        self.0 as u64 & ENTRY_LO_VALID != 0
    }

    /// Returns whether this page has the global bit set.
    pub const fn global(self) -> bool {
        self.0 as u64 & ENTRY_LO_GLOBAL != 0
    }
}

/// CP0 `PageMask` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0PageMask(u32);

impl Mips4Cp0PageMask {
    /// Creates a `PageMask` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self((bits & PAGE_MASK_MASK) as u32)
    }

    /// Creates a `PageMask` register value from a defined TLB page mask.
    pub const fn from_tlb_page_mask(page_mask: Mips4TlbPageMask) -> Self {
        Self(page_mask.bits())
    }

    /// Returns the raw `PageMask` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Converts this register value into a defined TLB page mask.
    pub const fn to_tlb_page_mask(self) -> Option<Mips4TlbPageMask> {
        Mips4TlbPageMask::from_bits(self.0)
    }
}

/// CP0 `EntryHi` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0EntryHi(u64);

impl Mips4Cp0EntryHi {
    /// Creates an `EntryHi` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits & ENTRY_HI_READABLE_MASK)
    }

    /// Creates an `EntryHi` register value from generic TLB fields.
    pub const fn from_tlb_entry_hi(entry_hi: Mips4TlbEntryHi) -> Self {
        Self(
            ((entry_hi.region_bits() as u64) << ENTRY_HI_REGION_SHIFT)
                | (entry_hi.vpn2() << ENTRY_HI_VPN2_SHIFT)
                | entry_hi.asid().bits() as u64,
        )
    }

    /// Returns the raw `EntryHi` register bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Converts this register value into generic TLB fields.
    pub const fn to_tlb_entry_hi(self) -> Option<Mips4TlbEntryHi> {
        Mips4TlbEntryHi::from_parts(
            self.virtual_page_number2(),
            Mips4TlbAsid::new(self.address_space_identifier()),
            self.region_bits(),
        )
    }

    /// Returns the region bits.
    pub const fn region_bits(self) -> u8 {
        ((self.0 >> ENTRY_HI_REGION_SHIFT) & ENTRY_HI_REGION_MASK) as u8
    }

    /// Returns the VPN2 field.
    pub const fn virtual_page_number2(self) -> u64 {
        (self.0 >> ENTRY_HI_VPN2_SHIFT) & ENTRY_HI_VPN2_MASK
    }

    /// Returns the ASID field.
    pub const fn address_space_identifier(self) -> u8 {
        (self.0 & ENTRY_HI_ASID_MASK) as u8
    }
}

/// CP0 `Context` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Context(u64);

impl Mips4Cp0Context {
    /// Creates a `Context` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits & CONTEXT_READABLE_MASK)
    }

    /// Returns the raw `Context` register bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns the page-table-entry base field.
    pub const fn page_table_entry_base(self) -> u64 {
        self.0 >> CONTEXT_PTE_BASE_SHIFT
    }

    /// Returns the BadVPN2 field.
    pub const fn bad_virtual_page_number2(self) -> u32 {
        ((self.0 >> CONTEXT_BAD_VPN2_SHIFT) & CONTEXT_BAD_VPN2_MASK) as u32
    }
}

/// CP0 `BadVAddr` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0BadVaddr(u64);

impl Mips4Cp0BadVaddr {
    /// Creates a `BadVAddr` register value.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the raw `BadVAddr` register bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns the bad virtual address.
    pub const fn address(self) -> u64 {
        self.0
    }
}

/// CP0 `Count` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Count(u32);

impl Mips4Cp0Count {
    /// Creates a `Count` register value.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits as u32)
    }

    /// Returns the raw `Count` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// CP0 `Compare` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Compare(u32);

impl Mips4Cp0Compare {
    /// Creates a `Compare` register value.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits as u32)
    }

    /// Returns the raw `Compare` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// CP0 `Status` KSU mode field.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Mips4Cp0KernelUserMode {
    /// Kernel mode.
    Kernel,

    /// Supervisor mode.
    Supervisor,

    /// User mode.
    User,

    /// Reserved KSU field value.
    Reserved,
}

impl Mips4Cp0KernelUserMode {
    /// Creates a KSU mode value from raw field bits.
    pub const fn from_bits(bits: u8) -> Self {
        match bits & 0x03 {
            0 => Self::Kernel,
            1 => Self::Supervisor,
            2 => Self::User,
            _ => Self::Reserved,
        }
    }

    /// Returns the raw KSU field bits.
    pub const fn bits(self) -> u8 {
        match self {
            Self::Kernel => 0,
            Self::Supervisor => 1,
            Self::User => 2,
            Self::Reserved => 3,
        }
    }
}

/// CP0 `Status` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Status(u32);

impl Mips4Cp0Status {
    /// Creates a `Status` register value from raw readable bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & STATUS_READABLE_MASK)
    }

    /// Returns the raw `Status` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the MIPS IV user-mode enable bit.
    pub const fn xx(self) -> bool {
        self.0 & STATUS_XX != 0
    }

    /// Returns the coprocessor usability bits as `CU2..CU0`.
    pub const fn cu(self) -> u8 {
        ((self.0 & STATUS_CU_MASK) >> STATUS_CU_SHIFT) as u8
    }

    /// Returns whether the selected coprocessor is usable.
    pub const fn coprocessor_usable(self, coprocessor: Mips4CoprocessorNumber) -> bool {
        match coprocessor {
            Mips4CoprocessorNumber::Cp0
            | Mips4CoprocessorNumber::Cp1
            | Mips4CoprocessorNumber::Cp2 => {
                self.0 & (1 << (STATUS_CU_SHIFT + coprocessor.number())) != 0
            }
            Mips4CoprocessorNumber::Cp3 => false,
        }
    }

    /// Returns the additional floating-point register enable bit.
    pub const fn additional_float_registers(self) -> bool {
        self.0 & STATUS_FR != 0
    }

    /// Returns the reverse-endianness bit.
    pub const fn reverse_endianness(self) -> bool {
        self.0 & STATUS_RE != 0
    }

    /// Returns the boot exception vectors bit.
    pub const fn boot_exception_vectors(self) -> bool {
        self.0 & STATUS_BEV != 0
    }

    /// Returns the soft reset or NMI status bit.
    pub const fn soft_reset_or_nmi(self) -> bool {
        self.0 & STATUS_SR != 0
    }

    /// Returns the cache-check-bit override bit.
    pub const fn cache_check_bits(self) -> bool {
        self.0 & STATUS_CE != 0
    }

    /// Returns the cache error disable bit.
    pub const fn cache_error_disabled(self) -> bool {
        self.0 & STATUS_DE != 0
    }

    /// Returns the interrupt mask field.
    pub const fn interrupt_mask(self) -> u8 {
        ((self.0 & STATUS_IM_MASK) >> STATUS_IM_SHIFT) as u8
    }

    /// Returns the kernel 64-bit addressing enable bit.
    pub const fn kernel_64_bit_addressing(self) -> bool {
        self.0 & STATUS_KX != 0
    }

    /// Returns the supervisor 64-bit addressing and operation enable bit.
    pub const fn supervisor_64_bit_addressing(self) -> bool {
        self.0 & STATUS_SX != 0
    }

    /// Returns the user 64-bit addressing and operation enable bit.
    pub const fn user_64_bit_addressing(self) -> bool {
        self.0 & STATUS_UX != 0
    }

    /// Returns the raw KSU mode field.
    pub const fn kernel_user_bits(self) -> u8 {
        ((self.0 & STATUS_KSU_MASK) >> STATUS_KSU_SHIFT) as u8
    }

    /// Returns the KSU operating mode field.
    pub const fn kernel_user_mode(self) -> Mips4Cp0KernelUserMode {
        Mips4Cp0KernelUserMode::from_bits(self.kernel_user_bits())
    }

    /// Returns the error-level bit.
    pub const fn error_level(self) -> bool {
        self.0 & STATUS_ERL != 0
    }

    /// Returns the exception-level bit.
    pub const fn exception_level(self) -> bool {
        self.0 & STATUS_EXL != 0
    }

    /// Returns the interrupt-enable bit.
    pub const fn interrupt_enable(self) -> bool {
        self.0 & STATUS_IE != 0
    }

    /// Returns whether interrupts are globally enabled.
    pub const fn interrupts_enabled(self) -> bool {
        self.interrupt_enable() && !self.exception_level() && !self.error_level()
    }
}

/// CP0 `Cause` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Cause(u32);

impl Mips4Cp0Cause {
    /// Creates a `Cause` register value from raw readable bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & CAUSE_READABLE_MASK)
    }

    /// Returns the raw `Cause` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the branch-delay bit.
    pub const fn branch_delay(self) -> bool {
        self.0 & CAUSE_BD != 0
    }

    /// Returns the coprocessor selected by the coprocessor-error field.
    pub const fn coprocessor_error(self) -> Mips4CoprocessorNumber {
        match ((self.0 & CAUSE_CE_MASK) >> CAUSE_CE_SHIFT) as u8 {
            0 => Mips4CoprocessorNumber::Cp0,
            1 => Mips4CoprocessorNumber::Cp1,
            2 => Mips4CoprocessorNumber::Cp2,
            _ => Mips4CoprocessorNumber::Cp3,
        }
    }

    /// Returns the interrupt-pending field.
    pub const fn interrupt_pending(self) -> u8 {
        ((self.0 & CAUSE_IP_MASK) >> CAUSE_IP_SHIFT) as u8
    }

    /// Returns the raw 5-bit exception code field.
    pub const fn exception_code(self) -> u8 {
        ((self.0 & CAUSE_EXC_CODE_MASK) >> CAUSE_EXC_CODE_SHIFT) as u8
    }

    const fn with_only_software_interrupt_writes(self, value: u32) -> Self {
        Self::from_bits((self.0 & !CAUSE_SOFTWARE_IP_MASK) | (value & CAUSE_SOFTWARE_IP_MASK))
    }

    const fn clear_timer_interrupt(self) -> Self {
        Self::from_bits(self.0 & !CAUSE_TIMER_IP)
    }
}

/// CP0 `EPC` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Epc(u64);

impl Mips4Cp0Epc {
    /// Creates an `EPC` register value.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the raw `EPC` register bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns the exception program counter.
    pub const fn address(self) -> u64 {
        self.0
    }
}

/// CP0 `PRId` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0ProcessorId(u32);

impl Mips4Cp0ProcessorId {
    /// Creates a `PRId` register value from raw readable bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & PROCESSOR_ID_READABLE_MASK)
    }

    /// Returns the raw `PRId` register bits.
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

/// CP0 `Config` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Config(u32);

impl Mips4Cp0Config {
    /// Creates a `Config` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw `Config` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns whether ordinary references may use the secondary cache.
    pub const fn secondary_cache_enabled(self) -> bool {
        self.0 & (1 << 12) != 0
    }
}

/// CP0 `LLAddr` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0LlAddr(u32);

impl Mips4Cp0LlAddr {
    /// Creates an `LLAddr` register value.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits as u32)
    }

    /// Creates an `LLAddr` register value from a physical address.
    pub const fn from_physical_address(address: u64) -> Self {
        Self((address >> 4) as u32)
    }

    /// Returns the raw `LLAddr` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns physical address bits 35:4.
    pub const fn physical_address_bits_35_4(self) -> u32 {
        self.0
    }
}

/// CP0 `WatchLo` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0WatchLo(u32);

impl Mips4Cp0WatchLo {
    /// Creates a `WatchLo` value from its implemented bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self((bits as u32) & WATCH_LO_READABLE_MASK)
    }

    /// Returns the implemented register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// CP0 `WatchHi` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0WatchHi(u32);

impl Mips4Cp0WatchHi {
    /// Creates a `WatchHi` value from its implemented bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self((bits as u32) & WATCH_HI_READABLE_MASK)
    }

    /// Returns the implemented register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// CP0 `XContext` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0XContext(u64);

impl Mips4Cp0XContext {
    /// Creates an `XContext` register value from raw readable bits.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits & XCONTEXT_READABLE_MASK)
    }

    /// Returns the raw `XContext` register bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns the page-table-entry base field.
    pub const fn page_table_entry_base(self) -> u32 {
        (self.0 >> XCONTEXT_PTE_BASE_SHIFT) as u32
    }

    /// Returns the region bits.
    pub const fn region_bits(self) -> u8 {
        ((self.0 >> XCONTEXT_REGION_SHIFT) & XCONTEXT_REGION_MASK) as u8
    }

    /// Returns the BadVPN2 field.
    pub const fn bad_virtual_page_number2(self) -> u32 {
        ((self.0 >> XCONTEXT_BAD_VPN2_SHIFT) & XCONTEXT_BAD_VPN2_MASK) as u32
    }
}

/// CP0 `ECC` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0Ecc(u32);

impl Mips4Cp0Ecc {
    /// Creates an `ECC` register value from raw readable bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & ECC_READABLE_MASK)
    }

    /// Returns the raw `ECC` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// CP0 `CacheErr` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0CacheErr(u32);

impl Mips4Cp0CacheErr {
    /// Creates a `CacheErr` register value from raw readable bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & CACHE_ERR_READABLE_MASK)
    }

    /// Creates the R5000 record for a primary-cache parity error.
    pub const fn primary_cache_error(
        data_reference: bool,
        data_field_error: bool,
        tag_field_error: bool,
        physical_address: u64,
        virtual_address: u64,
    ) -> Self {
        let mut bits = (((physical_address >> 3) as u32) & CACHE_ERR_PHYSICAL_INDEX_MASK)
            << CACHE_ERR_PHYSICAL_INDEX_SHIFT;
        bits |= ((virtual_address >> 12) as u32) & CACHE_ERR_VIRTUAL_INDEX_MASK;
        if data_reference {
            bits |= CACHE_ERR_DATA_REFERENCE;
        }
        if data_field_error {
            bits |= CACHE_ERR_DATA_FIELD;
        }
        if tag_field_error {
            bits |= CACHE_ERR_TAG_FIELD;
        }
        Self::from_bits(bits)
    }

    /// Returns the raw `CacheErr` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns whether the failed reference was a data reference.
    pub const fn data_reference(self) -> bool {
        self.0 & CACHE_ERR_DATA_REFERENCE != 0
    }

    /// Returns the processor-specific cache-level field.
    pub const fn cache_level(self) -> bool {
        self.0 & CACHE_ERR_CACHE_LEVEL != 0
    }

    /// Returns whether a data-field check error occurred.
    pub const fn data_field_error(self) -> bool {
        self.0 & CACHE_ERR_DATA_FIELD != 0
    }

    /// Returns whether a tag-field check error occurred.
    pub const fn tag_field_error(self) -> bool {
        self.0 & CACHE_ERR_TAG_FIELD != 0
    }

    /// Returns whether the error was detected on the system bus.
    pub const fn system_bus_error(self) -> bool {
        self.0 & CACHE_ERR_SYSTEM_BUS != 0
    }

    /// Returns whether an additional data error accompanied an instruction error.
    pub const fn additional_data_error(self) -> bool {
        self.0 & CACHE_ERR_ADDITIONAL_DATA != 0
    }

    /// Returns whether the error occurred while filling after a store miss.
    pub const fn fill_on_store_miss(self) -> bool {
        self.0 & CACHE_ERR_FILL_ON_STORE_MISS != 0
    }

    /// Returns physical address bits 21:3 captured for the failed reference.
    pub const fn physical_index(self) -> u32 {
        (self.0 >> CACHE_ERR_PHYSICAL_INDEX_SHIFT) & CACHE_ERR_PHYSICAL_INDEX_MASK
    }

    /// Returns virtual address bits 13:12 captured for the failed reference.
    pub const fn virtual_index(self) -> u8 {
        (self.0 & CACHE_ERR_VIRTUAL_INDEX_MASK) as u8
    }
}

/// CP0 `TagLo` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0TagLo(u32);

impl Mips4Cp0TagLo {
    /// Creates a `TagLo` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw `TagLo` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// CP0 `TagHi` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0TagHi(u32);

impl Mips4Cp0TagHi {
    /// Creates a `TagHi` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw `TagHi` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// CP0 `ErrorEPC` register.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Deserialize,
    serde::Serialize,
)]
pub struct Mips4Cp0ErrorEpc(u64);

impl Mips4Cp0ErrorEpc {
    /// Creates an `ErrorEPC` register value.
    pub const fn from_bits(bits: u64) -> Self {
        Self(bits)
    }

    /// Returns the raw `ErrorEPC` register bits.
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Returns the error exception program counter.
    pub const fn address(self) -> u64 {
        self.0
    }
}

/// MIPS IV CP0 register file.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Mips4Cp0 {
    index: Mips4Cp0Index,
    random: Mips4Cp0Random,
    random_upper_bound: u8,
    entry_lo0: Mips4Cp0EntryLo,
    entry_lo1: Mips4Cp0EntryLo,
    context: Mips4Cp0Context,
    page_mask: Mips4Cp0PageMask,
    wired: Mips4Cp0Wired,
    bad_vaddr: Mips4Cp0BadVaddr,
    count: Mips4Cp0Count,
    entry_hi: Mips4Cp0EntryHi,
    compare: Mips4Cp0Compare,
    status: Mips4Cp0Status,
    cause: Mips4Cp0Cause,
    epc: Mips4Cp0Epc,
    processor_id: Mips4Cp0ProcessorId,
    config: Mips4Cp0Config,
    ll_addr: Mips4Cp0LlAddr,
    watch_lo: Mips4Cp0WatchLo,
    watch_hi: Mips4Cp0WatchHi,
    x_context: Mips4Cp0XContext,
    ecc: Mips4Cp0Ecc,
    cache_err: Mips4Cp0CacheErr,
    tag_lo: Mips4Cp0TagLo,
    tag_hi: Mips4Cp0TagHi,
    error_epc: Mips4Cp0ErrorEpc,
}

impl Mips4Cp0 {
    /// Creates a CP0 register file with reset-visible modeled state.
    pub const fn new(processor_id: u32, config: u32, random_upper_bound: u8) -> Self {
        let random_upper_bound = random_upper_bound & (RANDOM_INDEX_MASK as u8);

        Self {
            index: Mips4Cp0Index::from_bits(0),
            random: Mips4Cp0Random::from_bits(random_upper_bound as u64),
            random_upper_bound,
            entry_lo0: Mips4Cp0EntryLo::from_bits(0),
            entry_lo1: Mips4Cp0EntryLo::from_bits(0),
            context: Mips4Cp0Context::from_bits(0),
            page_mask: Mips4Cp0PageMask::from_bits(0),
            wired: Mips4Cp0Wired::from_bits(0),
            bad_vaddr: Mips4Cp0BadVaddr::from_bits(0),
            count: Mips4Cp0Count::from_bits(0),
            entry_hi: Mips4Cp0EntryHi::from_bits(0),
            compare: Mips4Cp0Compare::from_bits(0),
            status: Mips4Cp0Status::from_bits(STATUS_ERL | STATUS_BEV),
            cause: Mips4Cp0Cause::from_bits(0),
            epc: Mips4Cp0Epc::from_bits(0),
            processor_id: Mips4Cp0ProcessorId::from_bits(processor_id),
            config: Mips4Cp0Config::from_bits(config),
            ll_addr: Mips4Cp0LlAddr::from_bits(0),
            watch_lo: Mips4Cp0WatchLo::from_bits(0),
            watch_hi: Mips4Cp0WatchHi::from_bits(0),
            x_context: Mips4Cp0XContext::from_bits(0),
            ecc: Mips4Cp0Ecc::from_bits(0),
            cache_err: Mips4Cp0CacheErr::from_bits(0),
            tag_lo: Mips4Cp0TagLo::from_bits(0),
            tag_hi: Mips4Cp0TagHi::from_bits(0),
            error_epc: Mips4Cp0ErrorEpc::from_bits(0),
        }
    }

    /// Returns the typed `Index` register.
    pub const fn index(self) -> Mips4Cp0Index {
        self.index
    }

    /// Returns the typed `Random` register.
    pub const fn random(self) -> Mips4Cp0Random {
        self.random
    }

    /// Returns the configured random upper bound.
    pub const fn random_upper_bound(self) -> u8 {
        self.random_upper_bound
    }

    /// Returns the typed `EntryLo0` register.
    pub const fn entry_lo0(self) -> Mips4Cp0EntryLo {
        self.entry_lo0
    }

    /// Returns the typed `EntryLo1` register.
    pub const fn entry_lo1(self) -> Mips4Cp0EntryLo {
        self.entry_lo1
    }

    /// Returns the typed `Context` register.
    pub const fn context(self) -> Mips4Cp0Context {
        self.context
    }

    /// Returns the typed `PageMask` register.
    pub const fn page_mask(self) -> Mips4Cp0PageMask {
        self.page_mask
    }

    /// Returns the typed `Wired` register.
    pub const fn wired(self) -> Mips4Cp0Wired {
        self.wired
    }

    /// Returns the typed `BadVAddr` register.
    pub const fn bad_vaddr(self) -> Mips4Cp0BadVaddr {
        self.bad_vaddr
    }

    /// Returns the typed `Count` register.
    pub const fn count(self) -> Mips4Cp0Count {
        self.count
    }

    /// Returns the typed `EntryHi` register.
    pub const fn entry_hi(self) -> Mips4Cp0EntryHi {
        self.entry_hi
    }

    /// Returns the typed `Compare` register.
    pub const fn compare(self) -> Mips4Cp0Compare {
        self.compare
    }

    /// Returns the typed `Status` register.
    pub const fn status(self) -> Mips4Cp0Status {
        self.status
    }

    /// Returns the typed `Cause` register.
    pub const fn cause(self) -> Mips4Cp0Cause {
        self.cause
    }

    /// Returns the typed `EPC` register.
    pub const fn epc(self) -> Mips4Cp0Epc {
        self.epc
    }

    /// Returns the typed `PRId` register.
    pub const fn processor_id(self) -> Mips4Cp0ProcessorId {
        self.processor_id
    }

    /// Returns the typed `Config` register.
    pub const fn config(self) -> Mips4Cp0Config {
        self.config
    }

    /// Returns the typed `LLAddr` register.
    pub const fn ll_addr(self) -> Mips4Cp0LlAddr {
        self.ll_addr
    }

    /// Returns the typed `WatchLo` register.
    pub const fn watch_lo(self) -> Mips4Cp0WatchLo {
        self.watch_lo
    }

    /// Returns the typed `WatchHi` register.
    pub const fn watch_hi(self) -> Mips4Cp0WatchHi {
        self.watch_hi
    }

    /// Returns the typed `XContext` register.
    pub const fn x_context(self) -> Mips4Cp0XContext {
        self.x_context
    }

    /// Returns the typed `ECC` register.
    pub const fn ecc(self) -> Mips4Cp0Ecc {
        self.ecc
    }

    /// Returns the typed `CacheErr` register.
    pub const fn cache_err(self) -> Mips4Cp0CacheErr {
        self.cache_err
    }

    /// Returns the typed `TagLo` register.
    pub const fn tag_lo(self) -> Mips4Cp0TagLo {
        self.tag_lo
    }

    /// Returns the typed `TagHi` register.
    pub const fn tag_hi(self) -> Mips4Cp0TagHi {
        self.tag_hi
    }

    /// Returns the typed `ErrorEPC` register.
    pub const fn error_epc(self) -> Mips4Cp0ErrorEpc {
        self.error_epc
    }

    /// Reads a CP0 register as raw bits.
    pub const fn read(self, register: Mips4Cp0Register) -> u64 {
        match register {
            Mips4Cp0Register::Index => self.index.bits() as u64,
            Mips4Cp0Register::Random => self.random.bits() as u64,
            Mips4Cp0Register::EntryLo0 => self.entry_lo0.bits() as u64,
            Mips4Cp0Register::EntryLo1 => self.entry_lo1.bits() as u64,
            Mips4Cp0Register::Context => self.context.bits(),
            Mips4Cp0Register::PageMask => self.page_mask.bits() as u64,
            Mips4Cp0Register::Wired => self.wired.bits() as u64,
            Mips4Cp0Register::BadVaddr => self.bad_vaddr.bits(),
            Mips4Cp0Register::Count => self.count.bits() as u64,
            Mips4Cp0Register::EntryHi => self.entry_hi.bits(),
            Mips4Cp0Register::Compare => self.compare.bits() as u64,
            Mips4Cp0Register::Status => self.status.bits() as u64,
            Mips4Cp0Register::Cause => self.cause.bits() as u64,
            Mips4Cp0Register::Epc => self.epc.bits(),
            Mips4Cp0Register::ProcessorId => self.processor_id.bits() as u64,
            Mips4Cp0Register::Config => self.config.bits() as u64,
            Mips4Cp0Register::LlAddr => self.ll_addr.bits() as u64,
            Mips4Cp0Register::WatchLo => self.watch_lo.bits() as u64,
            Mips4Cp0Register::WatchHi => self.watch_hi.bits() as u64,
            Mips4Cp0Register::XContext => self.x_context.bits(),
            Mips4Cp0Register::Ecc => self.ecc.bits() as u64,
            Mips4Cp0Register::CacheErr => self.cache_err.bits() as u64,
            Mips4Cp0Register::TagLo => self.tag_lo.bits() as u64,
            Mips4Cp0Register::TagHi => self.tag_hi.bits() as u64,
            Mips4Cp0Register::ErrorEpc => self.error_epc.bits(),
        }
    }

    /// Writes a CP0 register as raw bits.
    pub const fn write(
        &mut self,
        register: Mips4Cp0Register,
        value: u64,
    ) -> Result<(), Mips4Cp0WriteError> {
        match register {
            Mips4Cp0Register::Index => self.index = Mips4Cp0Index::from_bits(value),
            Mips4Cp0Register::Random
            | Mips4Cp0Register::BadVaddr
            | Mips4Cp0Register::ProcessorId
            | Mips4Cp0Register::CacheErr => {
                return Err(Mips4Cp0WriteError::ReadOnlyRegister { register });
            }
            Mips4Cp0Register::EntryLo0 => self.entry_lo0 = Mips4Cp0EntryLo::from_bits(value),
            Mips4Cp0Register::EntryLo1 => self.entry_lo1 = Mips4Cp0EntryLo::from_bits(value),
            Mips4Cp0Register::Context => self.context = Mips4Cp0Context::from_bits(value),
            Mips4Cp0Register::PageMask => self.page_mask = Mips4Cp0PageMask::from_bits(value),
            Mips4Cp0Register::Wired => {
                self.wired = Mips4Cp0Wired::from_bits(value);
                self.random = Mips4Cp0Random::from_bits(self.random_upper_bound as u64);
            }
            Mips4Cp0Register::Count => self.count = Mips4Cp0Count::from_bits(value),
            Mips4Cp0Register::EntryHi => self.entry_hi = Mips4Cp0EntryHi::from_bits(value),
            Mips4Cp0Register::Compare => {
                self.compare = Mips4Cp0Compare::from_bits(value);
                self.cause = self.cause.clear_timer_interrupt();
            }
            Mips4Cp0Register::Status => self.status = Mips4Cp0Status::from_bits(value as u32),
            Mips4Cp0Register::Cause => {
                self.cause = self.cause.with_only_software_interrupt_writes(value as u32);
            }
            Mips4Cp0Register::Epc => self.epc = Mips4Cp0Epc::from_bits(value),
            Mips4Cp0Register::Config => self.config = Mips4Cp0Config::from_bits(value as u32),
            Mips4Cp0Register::LlAddr => self.ll_addr = Mips4Cp0LlAddr::from_bits(value),
            Mips4Cp0Register::WatchLo => self.watch_lo = Mips4Cp0WatchLo::from_bits(value),
            Mips4Cp0Register::WatchHi => self.watch_hi = Mips4Cp0WatchHi::from_bits(value),
            Mips4Cp0Register::XContext => self.x_context = Mips4Cp0XContext::from_bits(value),
            Mips4Cp0Register::Ecc => self.ecc = Mips4Cp0Ecc::from_bits(value as u32),
            Mips4Cp0Register::TagLo => self.tag_lo = Mips4Cp0TagLo::from_bits(value as u32),
            Mips4Cp0Register::TagHi => self.tag_hi = Mips4Cp0TagHi::from_bits(value as u32),
            Mips4Cp0Register::ErrorEpc => self.error_epc = Mips4Cp0ErrorEpc::from_bits(value),
        }

        Ok(())
    }

    /// Applies precise architectural exception state updates.
    pub(crate) fn enter_exception(&mut self, image: Mips4ExceptionImage) {
        let status_before = self.status;
        let mut cause_bits = self.cause.bits() & CAUSE_IP_MASK;
        cause_bits |= (image.reason.cause_code() as u32) << CAUSE_EXC_CODE_SHIFT;

        if !status_before.exception_level() {
            self.epc = Mips4Cp0Epc::from_bits(image.restart.restart_pc);
            if image.restart.in_branch_delay_slot {
                cause_bits |= CAUSE_BD;
            }
        } else if self.cause.branch_delay() {
            cause_bits |= CAUSE_BD;
        }

        if let Mips4Exception::CoprocessorUnusable { coprocessor } = image.reason {
            cause_bits |= (coprocessor.number() as u32) << CAUSE_CE_SHIFT;
        }

        self.cause = Mips4Cp0Cause::from_bits(cause_bits);
        self.status = Mips4Cp0Status::from_bits(status_before.bits() | STATUS_EXL);

        if let Some(address) = image.bad_virtual_address {
            self.bad_vaddr = Mips4Cp0BadVaddr::from_bits(address);
            if matches!(
                image.reason,
                Mips4Exception::TlbModification
                    | Mips4Exception::TlbLoad
                    | Mips4Exception::TlbStore
            ) {
                self.record_tlb_fault_context(address);
            }
        }
    }

    /// Applies processor error-level exception state updates.
    pub(crate) fn enter_error_exception(&mut self, image: Mips4ErrorExceptionImage) {
        self.error_epc = Mips4Cp0ErrorEpc::from_bits(image.restart.restart_pc);
        let status = match image.reason {
            Mips4ErrorException::SoftReset | Mips4ErrorException::NonMaskableInterrupt => {
                self.status.bits() | STATUS_ERL | STATUS_BEV | STATUS_SR
            }
            Mips4ErrorException::CacheError => {
                self.cache_err = Mips4Cp0CacheErr::from_bits(image.cache_error.unwrap_or(0));
                self.status.bits() | STATUS_ERL
            }
        };
        self.status = Mips4Cp0Status::from_bits(status);
    }

    /// Clears exception level for `ERET` and returns the saved program counter.
    pub fn return_from_exception(&mut self) -> u64 {
        let (pc, clear) = if self.status.error_level() {
            (self.error_epc.address(), STATUS_ERL)
        } else {
            (self.epc.address(), STATUS_EXL)
        };
        self.status = Mips4Cp0Status::from_bits(self.status.bits() & !clear);
        pc
    }

    /// Advances the architecturally visible pseudo-random TLB replacement index.
    pub fn advance_random(&mut self, increments: u64) {
        let lower = self.wired.boundary().min(self.random_upper_bound);
        let span = u64::from(self.random_upper_bound - lower) + 1;
        let current_offset = u64::from(self.random.index() - lower);
        let decrement = increments % span;
        let next_offset = (current_offset + span - decrement) % span;
        self.random = Mips4Cp0Random::from_bits(u64::from(lower) + next_offset);
    }

    /// Replaces the external hardware interrupt portion of Cause IP.
    pub(crate) fn set_external_interrupts(&mut self, pending: u8) {
        let software = self.cause.bits() & CAUSE_SOFTWARE_IP_MASK;
        let timer = self.cause.bits() & CAUSE_TIMER_IP;
        let hardware = ((pending as u32) << CAUSE_IP_SHIFT)
            & (CAUSE_IP_MASK & !CAUSE_SOFTWARE_IP_MASK & !CAUSE_TIMER_IP);
        self.cause = Mips4Cp0Cause::from_bits(
            (self.cause.bits() & !CAUSE_IP_MASK) | software | timer | hardware,
        );
    }

    /// Advances Count and raises the timer interrupt on a Compare match.
    pub fn advance_count(&mut self, increments: u64, timer_interrupt_enabled: bool) {
        let old_count = self.count.bits();
        let new_count = old_count.wrapping_add(increments as u32);
        self.count = Mips4Cp0Count::from_bits(new_count as u64);

        if timer_interrupt_enabled
            && count_range_contains(old_count, increments, self.compare.bits())
        {
            self.cause = Mips4Cp0Cause::from_bits(self.cause.bits() | CAUSE_TIMER_IP);
        }
    }

    fn record_tlb_fault_context(&mut self, address: u64) {
        let asid = self.entry_hi.address_space_identifier() as u64;
        let entry_hi_bits = (address
            & ((ENTRY_HI_REGION_MASK << ENTRY_HI_REGION_SHIFT)
                | (ENTRY_HI_VPN2_MASK << ENTRY_HI_VPN2_SHIFT)))
            | asid;
        self.entry_hi = Mips4Cp0EntryHi::from_bits(entry_hi_bits);

        let context_bits = (self.context.page_table_entry_base() << CONTEXT_PTE_BASE_SHIFT)
            | (((address >> ENTRY_HI_VPN2_SHIFT) & CONTEXT_BAD_VPN2_MASK)
                << CONTEXT_BAD_VPN2_SHIFT);
        self.context = Mips4Cp0Context::from_bits(context_bits);

        let x_context_bits = ((self.x_context.page_table_entry_base() as u64)
            << XCONTEXT_PTE_BASE_SHIFT)
            | (((address >> ENTRY_HI_VPN2_SHIFT) & XCONTEXT_BAD_VPN2_MASK)
                << XCONTEXT_BAD_VPN2_SHIFT)
            | (((address >> ENTRY_HI_REGION_SHIFT) & XCONTEXT_REGION_MASK)
                << XCONTEXT_REGION_SHIFT);
        self.x_context = Mips4Cp0XContext::from_bits(x_context_bits);
    }
}

fn count_range_contains(start: u32, increments: u64, target: u32) -> bool {
    if increments == 0 {
        return false;
    }

    if increments > u32::MAX as u64 {
        return true;
    }

    let distance = target.wrapping_sub(start) as u64;
    distance != 0 && distance <= increments
}

#[cfg(test)]
mod tests;
