//! MIPS I system control coprocessor register primitives.
//!
//! This module models the R3000A-compatible CP0 register numbers, raw register
//! bit fields, and a minimal register file. It does not execute CP0
//! instructions, perform exception entry, manage TLB entries, translate
//! addresses, or model cache behavior.

use crate::cpu::mips1::exception::Mips1CoprocessorNumber;

const STATUS_CU0: u32 = 1 << 28;
const STATUS_RE: u32 = 1 << 25;
const STATUS_BEV: u32 = 1 << 22;
const STATUS_TS: u32 = 1 << 21;
const STATUS_PE: u32 = 1 << 20;
const STATUS_CM: u32 = 1 << 19;
const STATUS_PZ: u32 = 1 << 18;
const STATUS_SWC: u32 = 1 << 17;
const STATUS_ISC: u32 = 1 << 16;
const STATUS_IM_SHIFT: u32 = 8;
const STATUS_IM_MASK: u32 = 0x0000_ff00;
const STATUS_KU_IE_MASK: u32 = 0x0000_003f;
const STATUS_HARDWARE_MASK: u32 = STATUS_TS | STATUS_PE | STATUS_CM;
const STATUS_READABLE_MASK: u32 = 0xf27f_ff3f;
const STATUS_SOFTWARE_WRITABLE_MASK: u32 = STATUS_READABLE_MASK & !STATUS_HARDWARE_MASK;

const CAUSE_BD: u32 = 1 << 31;
const CAUSE_CE_SHIFT: u32 = 28;
const CAUSE_CE_MASK: u32 = 0x3000_0000;
const CAUSE_IP_SHIFT: u32 = 8;
const CAUSE_IP_MASK: u32 = 0x0000_ff00;
const CAUSE_SOFTWARE_IP_MASK: u32 = 0x0000_0300;
const CAUSE_EXC_CODE_SHIFT: u32 = 2;
const CAUSE_EXC_CODE_MASK: u32 = 0x0000_007c;
const CAUSE_READABLE_MASK: u32 = CAUSE_BD | CAUSE_CE_MASK | CAUSE_IP_MASK | CAUSE_EXC_CODE_MASK;

const INDEX_PROBE_FAILURE: u32 = 1 << 31;
const INDEX_FIELD_SHIFT: u32 = 8;
const INDEX_FIELD_MASK: u32 = 0x0000_3f00;
const INDEX_READABLE_MASK: u32 = INDEX_PROBE_FAILURE | INDEX_FIELD_MASK;

const RANDOM_FIELD_SHIFT: u32 = 8;
const RANDOM_READABLE_MASK: u32 = 0x0000_3f00;

const ENTRY_HI_VPN_SHIFT: u32 = 12;
const ENTRY_HI_ASID_SHIFT: u32 = 6;
const ENTRY_HI_READABLE_MASK: u32 = 0xffff_ffc0;

const ENTRY_LO_PFN_SHIFT: u32 = 12;
const ENTRY_LO_NONCACHEABLE: u32 = 1 << 11;
const ENTRY_LO_DIRTY: u32 = 1 << 10;
const ENTRY_LO_VALID: u32 = 1 << 9;
const ENTRY_LO_GLOBAL: u32 = 1 << 8;
const ENTRY_LO_READABLE_MASK: u32 = 0xffff_ff00;

const CONTEXT_PTE_BASE_SHIFT: u32 = 21;
const CONTEXT_BAD_VPN_SHIFT: u32 = 2;
const CONTEXT_BAD_VPN_MASK: u32 = 0x001f_fffc;
const CONTEXT_READABLE_MASK: u32 = 0xffff_fffc;

/// R3000A-compatible CP0 register number.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Mips1Cp0Register {
    /// TLB index register.
    Index,

    /// TLB random replacement register.
    Random,

    /// TLB low entry register.
    EntryLo,

    /// TLB refill context register.
    Context,

    /// Bad virtual address register.
    BadVaddr,

    /// TLB high entry register.
    EntryHi,

    /// Status register.
    Status,

    /// Cause register.
    Cause,

    /// Exception program counter register.
    Epc,

    /// Processor identification register.
    ProcessorId,
}

impl Mips1Cp0Register {
    /// Creates a CP0 register number from its raw instruction field value.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(Self::Index),
            1 => Some(Self::Random),
            2 => Some(Self::EntryLo),
            4 => Some(Self::Context),
            8 => Some(Self::BadVaddr),
            10 => Some(Self::EntryHi),
            12 => Some(Self::Status),
            13 => Some(Self::Cause),
            14 => Some(Self::Epc),
            15 => Some(Self::ProcessorId),
            _ => None,
        }
    }

    /// Returns the raw CP0 register number.
    pub const fn number(self) -> u8 {
        match self {
            Self::Index => 0,
            Self::Random => 1,
            Self::EntryLo => 2,
            Self::Context => 4,
            Self::BadVaddr => 8,
            Self::EntryHi => 10,
            Self::Status => 12,
            Self::Cause => 13,
            Self::Epc => 14,
            Self::ProcessorId => 15,
        }
    }
}

/// Error returned by CP0 register writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mips1Cp0WriteError {
    /// The selected CP0 register is read-only through the public register file.
    ReadOnlyRegister {
        /// Read-only register selected by the write.
        register: Mips1Cp0Register,
    },
}

impl Mips1Cp0WriteError {
    /// Returns the register rejected by the write.
    pub const fn register(self) -> Mips1Cp0Register {
        match self {
            Self::ReadOnlyRegister { register } => register,
        }
    }
}

/// CP0 `Status` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0Status(u32);

impl Mips1Cp0Status {
    /// Creates a `Status` register value from raw readable bits.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & STATUS_READABLE_MASK)
    }

    /// Returns the raw `Status` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the four coprocessor usability bits as `CU3..CU0`.
    pub const fn cu(self) -> u8 {
        ((self.0 >> 28) & 0x0f) as u8
    }

    /// Returns whether the selected coprocessor is usable.
    pub const fn coprocessor_usable(self, coprocessor: Mips1CoprocessorNumber) -> bool {
        self.0 & (STATUS_CU0 << coprocessor.number()) != 0
    }

    /// Returns the reverse-endianness bit.
    pub const fn reverse_endianness(self) -> bool {
        self.0 & STATUS_RE != 0
    }

    /// Returns the boot exception vectors bit.
    pub const fn boot_exception_vectors(self) -> bool {
        self.0 & STATUS_BEV != 0
    }

    /// Returns the TLB shutdown bit.
    pub const fn tlb_shutdown(self) -> bool {
        self.0 & STATUS_TS != 0
    }

    /// Returns the cache parity error bit.
    pub const fn parity_error(self) -> bool {
        self.0 & STATUS_PE != 0
    }

    /// Returns the cache miss bit.
    pub const fn cache_miss(self) -> bool {
        self.0 & STATUS_CM != 0
    }

    /// Returns the parity-zero bit.
    pub const fn parity_zero(self) -> bool {
        self.0 & STATUS_PZ != 0
    }

    /// Returns the swap-caches bit.
    pub const fn swap_caches(self) -> bool {
        self.0 & STATUS_SWC != 0
    }

    /// Returns the isolate-cache bit.
    pub const fn isolate_cache(self) -> bool {
        self.0 & STATUS_ISC != 0
    }

    /// Returns the interrupt mask field.
    pub const fn interrupt_mask(self) -> u8 {
        ((self.0 & STATUS_IM_MASK) >> STATUS_IM_SHIFT) as u8
    }

    /// Returns the current kernel/user bit.
    pub const fn kernel_user_current(self) -> bool {
        self.0 & 0x0000_0002 != 0
    }

    /// Returns the current interrupt-enable bit.
    pub const fn interrupt_enable_current(self) -> bool {
        self.0 & 0x0000_0001 != 0
    }

    /// Returns the previous kernel/user bit.
    pub const fn kernel_user_previous(self) -> bool {
        self.0 & 0x0000_0008 != 0
    }

    /// Returns the previous interrupt-enable bit.
    pub const fn interrupt_enable_previous(self) -> bool {
        self.0 & 0x0000_0004 != 0
    }

    /// Returns the old kernel/user bit.
    pub const fn kernel_user_old(self) -> bool {
        self.0 & 0x0000_0020 != 0
    }

    /// Returns the old interrupt-enable bit.
    pub const fn interrupt_enable_old(self) -> bool {
        self.0 & 0x0000_0010 != 0
    }

    /// Applies the `RFE` status-bit stack pop operation.
    pub const fn restore_from_exception(self) -> Self {
        let restored_low_bits = (self.0 & 0x0000_0030) | ((self.0 >> 2) & 0x0000_000f);
        Self::from_bits((self.0 & !STATUS_KU_IE_MASK) | restored_low_bits)
    }
}

/// CP0 `Cause` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0Cause(u32);

impl Mips1Cp0Cause {
    /// Creates a `Cause` register value.
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
    pub const fn coprocessor_error(self) -> Mips1CoprocessorNumber {
        match ((self.0 & CAUSE_CE_MASK) >> CAUSE_CE_SHIFT) as u8 {
            0 => Mips1CoprocessorNumber::Cp0,
            1 => Mips1CoprocessorNumber::Cp1,
            2 => Mips1CoprocessorNumber::Cp2,
            _ => Mips1CoprocessorNumber::Cp3,
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
}

/// CP0 `PRId` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0ProcessorId(u32);

impl Mips1Cp0ProcessorId {
    /// Creates a `PRId` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
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

/// CP0 `Index` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0Index(u32);

impl Mips1Cp0Index {
    /// Creates an `Index` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & INDEX_READABLE_MASK)
    }

    /// Returns the raw `Index` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the TLB-probe failure bit.
    pub const fn probe_failure(self) -> bool {
        self.0 & INDEX_PROBE_FAILURE != 0
    }

    /// Returns the TLB index field.
    pub const fn index(self) -> u8 {
        ((self.0 & INDEX_FIELD_MASK) >> INDEX_FIELD_SHIFT) as u8
    }
}

/// CP0 `Random` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0Random(u32);

impl Mips1Cp0Random {
    /// Creates a `Random` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & RANDOM_READABLE_MASK)
    }

    /// Returns the raw `Random` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the random TLB index field.
    pub const fn random(self) -> u8 {
        ((self.0 & RANDOM_READABLE_MASK) >> RANDOM_FIELD_SHIFT) as u8
    }
}

/// CP0 `EntryHi` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0EntryHi(u32);

impl Mips1Cp0EntryHi {
    /// Creates an `EntryHi` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & ENTRY_HI_READABLE_MASK)
    }

    /// Returns the raw `EntryHi` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the virtual page number field.
    pub const fn virtual_page_number(self) -> u32 {
        self.0 >> ENTRY_HI_VPN_SHIFT
    }

    /// Returns the address-space identifier field.
    pub const fn address_space_identifier(self) -> u8 {
        ((self.0 >> ENTRY_HI_ASID_SHIFT) & 0x3f) as u8
    }
}

/// CP0 `EntryLo` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0EntryLo(u32);

impl Mips1Cp0EntryLo {
    /// Creates an `EntryLo` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & ENTRY_LO_READABLE_MASK)
    }

    /// Returns the raw `EntryLo` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the physical frame number field.
    pub const fn physical_frame_number(self) -> u32 {
        self.0 >> ENTRY_LO_PFN_SHIFT
    }

    /// Returns the noncacheable bit.
    pub const fn noncacheable(self) -> bool {
        self.0 & ENTRY_LO_NONCACHEABLE != 0
    }

    /// Returns the dirty bit.
    pub const fn dirty(self) -> bool {
        self.0 & ENTRY_LO_DIRTY != 0
    }

    /// Returns the valid bit.
    pub const fn valid(self) -> bool {
        self.0 & ENTRY_LO_VALID != 0
    }

    /// Returns the global bit.
    pub const fn global(self) -> bool {
        self.0 & ENTRY_LO_GLOBAL != 0
    }
}

/// CP0 `Context` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0Context(u32);

impl Mips1Cp0Context {
    /// Creates a `Context` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits & CONTEXT_READABLE_MASK)
    }

    /// Returns the raw `Context` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the page-table-entry base field.
    pub const fn page_table_entry_base(self) -> u16 {
        (self.0 >> CONTEXT_PTE_BASE_SHIFT) as u16
    }

    /// Returns the bad virtual page number field.
    pub const fn bad_virtual_page_number(self) -> u32 {
        (self.0 & CONTEXT_BAD_VPN_MASK) >> CONTEXT_BAD_VPN_SHIFT
    }
}

/// CP0 `BadVaddr` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0BadVaddr(u32);

impl Mips1Cp0BadVaddr {
    /// Creates a `BadVaddr` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw `BadVaddr` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the bad virtual address.
    pub const fn address(self) -> u32 {
        self.0
    }
}

/// CP0 `EPC` register.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Mips1Cp0Epc(u32);

impl Mips1Cp0Epc {
    /// Creates an `EPC` register value.
    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    /// Returns the raw `EPC` register bits.
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Returns the exception program counter.
    pub const fn address(self) -> u32 {
        self.0
    }
}

/// R3000A-compatible CP0 register file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Mips1Cp0 {
    index: Mips1Cp0Index,
    random: Mips1Cp0Random,
    entry_lo: Mips1Cp0EntryLo,
    context: Mips1Cp0Context,
    bad_vaddr: Mips1Cp0BadVaddr,
    entry_hi: Mips1Cp0EntryHi,
    status: Mips1Cp0Status,
    cause: Mips1Cp0Cause,
    epc: Mips1Cp0Epc,
    processor_id: Mips1Cp0ProcessorId,
}

impl Mips1Cp0 {
    /// Creates a CP0 register file with zeroed modeled state.
    pub const fn new(processor_id: u32) -> Self {
        Self {
            index: Mips1Cp0Index::from_bits(0),
            random: Mips1Cp0Random::from_bits(0),
            entry_lo: Mips1Cp0EntryLo::from_bits(0),
            context: Mips1Cp0Context::from_bits(0),
            bad_vaddr: Mips1Cp0BadVaddr::from_bits(0),
            entry_hi: Mips1Cp0EntryHi::from_bits(0),
            status: Mips1Cp0Status::from_bits(0),
            cause: Mips1Cp0Cause::from_bits(0),
            epc: Mips1Cp0Epc::from_bits(0),
            processor_id: Mips1Cp0ProcessorId::from_bits(processor_id),
        }
    }

    /// Returns the typed `Index` register.
    pub const fn index(self) -> Mips1Cp0Index {
        self.index
    }

    /// Returns the typed `Random` register.
    pub const fn random(self) -> Mips1Cp0Random {
        self.random
    }

    /// Returns the typed `EntryLo` register.
    pub const fn entry_lo(self) -> Mips1Cp0EntryLo {
        self.entry_lo
    }

    /// Returns the typed `Context` register.
    pub const fn context(self) -> Mips1Cp0Context {
        self.context
    }

    /// Returns the typed `BadVaddr` register.
    pub const fn bad_vaddr(self) -> Mips1Cp0BadVaddr {
        self.bad_vaddr
    }

    /// Returns the typed `EntryHi` register.
    pub const fn entry_hi(self) -> Mips1Cp0EntryHi {
        self.entry_hi
    }

    /// Returns the typed `Status` register.
    pub const fn status(self) -> Mips1Cp0Status {
        self.status
    }

    /// Returns the typed `Cause` register.
    pub const fn cause(self) -> Mips1Cp0Cause {
        self.cause
    }

    /// Returns the typed `EPC` register.
    pub const fn epc(self) -> Mips1Cp0Epc {
        self.epc
    }

    /// Returns the typed `PRId` register.
    pub const fn processor_id(self) -> Mips1Cp0ProcessorId {
        self.processor_id
    }

    /// Reads a CP0 register as raw bits.
    pub const fn read(self, register: Mips1Cp0Register) -> u32 {
        match register {
            Mips1Cp0Register::Index => self.index.bits(),
            Mips1Cp0Register::Random => self.random.bits(),
            Mips1Cp0Register::EntryLo => self.entry_lo.bits(),
            Mips1Cp0Register::Context => self.context.bits(),
            Mips1Cp0Register::BadVaddr => self.bad_vaddr.bits(),
            Mips1Cp0Register::EntryHi => self.entry_hi.bits(),
            Mips1Cp0Register::Status => self.status.bits(),
            Mips1Cp0Register::Cause => self.cause.bits(),
            Mips1Cp0Register::Epc => self.epc.bits(),
            Mips1Cp0Register::ProcessorId => self.processor_id.bits(),
        }
    }

    /// Writes a CP0 register as raw bits.
    ///
    /// `Status` writes update only software-writable fields. The TLB shutdown
    /// and cache-miss bits are preserved, and writing `1` to the parity-error
    /// bit clears it.
    pub const fn write(
        &mut self,
        register: Mips1Cp0Register,
        value: u32,
    ) -> Result<(), Mips1Cp0WriteError> {
        match register {
            Mips1Cp0Register::Index => self.index = Mips1Cp0Index::from_bits(value),
            Mips1Cp0Register::Random => self.random = Mips1Cp0Random::from_bits(value),
            Mips1Cp0Register::EntryLo => self.entry_lo = Mips1Cp0EntryLo::from_bits(value),
            Mips1Cp0Register::Context => self.context = Mips1Cp0Context::from_bits(value),
            Mips1Cp0Register::BadVaddr | Mips1Cp0Register::ProcessorId => {
                return Err(Mips1Cp0WriteError::ReadOnlyRegister { register });
            }
            Mips1Cp0Register::EntryHi => self.entry_hi = Mips1Cp0EntryHi::from_bits(value),
            Mips1Cp0Register::Status => self.status = write_status(self.status, value),
            Mips1Cp0Register::Cause => {
                let bits = (self.cause.bits() & !CAUSE_SOFTWARE_IP_MASK)
                    | (value & CAUSE_SOFTWARE_IP_MASK);
                self.cause = Mips1Cp0Cause::from_bits(bits);
            }
            Mips1Cp0Register::Epc => self.epc = Mips1Cp0Epc::from_bits(value),
        }

        Ok(())
    }
}

const fn write_status(previous: Mips1Cp0Status, value: u32) -> Mips1Cp0Status {
    let software_bits = value & STATUS_SOFTWARE_WRITABLE_MASK;
    let hardware_bits = previous.bits() & STATUS_HARDWARE_MASK;
    let parity_error = if value & STATUS_PE != 0 {
        0
    } else {
        hardware_bits & STATUS_PE
    };

    Mips1Cp0Status::from_bits(
        software_bits | (hardware_bits & (STATUS_TS | STATUS_CM)) | parity_error,
    )
}

#[cfg(test)]
mod tests;
