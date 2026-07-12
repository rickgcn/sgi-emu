//! CRIME 1.1 software-visible registers and address definitions.

/// First CRIME processor-interface register address.
pub const CRIME_BASE: u64 = 0x1400_0000;
/// First Rendering Engine register address.
pub const CRIME_RENDER_BASE: u64 = 0x1500_0000;
/// End of the CRIME register aperture.
pub const CRIME_REGISTER_END: u64 = 0x1600_0000;

/// CRIME ID and revision.
pub const ID: u64 = CRIME_BASE;
/// Processor-interface control.
pub const CONTROL: u64 = CRIME_BASE + 0x0008;
/// Combined interrupt status.
pub const INTERRUPT_STATUS: u64 = CRIME_BASE + 0x0010;
/// Interrupt enable mask.
pub const INTERRUPT_ENABLE: u64 = CRIME_BASE + 0x0018;
/// Software interrupt register.
pub const SOFTWARE_INTERRUPT: u64 = CRIME_BASE + 0x0020;
/// Hardware interrupt register.
pub const HARDWARE_INTERRUPT: u64 = CRIME_BASE + 0x0028;
/// Watchdog counter and timeout state.
pub const WATCHDOG: u64 = CRIME_BASE + 0x0030;
/// Free-running CRIME timer.
pub const TIMER: u64 = CRIME_BASE + 0x0038;
/// CPU error address.
pub const CPU_ERROR_ADDRESS: u64 = CRIME_BASE + 0x0040;
/// CRIME 1.1 CPU error status address.
pub const CPU_ERROR_STATUS: u64 = CRIME_BASE + 0x0048;
/// Reserved CRIME 1.1 write sink used by the IP32 PROM.
///
/// The CRIME register specification and operating-system ABI assign no state
/// to this address, while IP32 PROM revisions 4.3 and 4.18 issue an aligned
/// doubleword write during initialization. Writes are accepted without
/// storing data or producing hardware effects; reads remain unsupported.
pub const CPU_RESERVED_WRITE_SINK: u64 = CRIME_BASE + 0x0050;

/// Memory-controller status and control.
pub const MEMORY_CONTROL: u64 = CRIME_BASE + 0x0200;
/// First external-bank control register.
pub const MEMORY_BANK_CONTROL_BASE: u64 = CRIME_BASE + 0x0208;
/// Refresh counter.
pub const MEMORY_REFRESH_COUNTER: u64 = CRIME_BASE + 0x0248;
/// Memory error status.
pub const MEMORY_ERROR_STATUS: u64 = CRIME_BASE + 0x0250;
/// Memory error address.
pub const MEMORY_ERROR_ADDRESS: u64 = CRIME_BASE + 0x0258;
/// Four packed ECC syndromes.
pub const MEMORY_ECC_SYNDROME: u64 = CRIME_BASE + 0x0260;
/// Four packed generated ECC check bytes.
pub const MEMORY_ECC_CHECK: u64 = CRIME_BASE + 0x0268;
/// Four packed replacement ECC bytes.
pub const MEMORY_ECC_REPLACEMENT: u64 = CRIME_BASE + 0x0270;

/// CRIME 1.1 identity value: ASIC ID A, revision 1.
pub const ID_VALUE: u64 = 0xA1;
/// Software-visible processor-interface control bits.
pub const CONTROL_MASK: u64 = 0x3fff;
/// R5000 SysADC input checking.
pub const CONTROL_R5000_SYSADC: u64 = 1 << 13;
/// CRIME-generated SysADC checking.
pub const CONTROL_CRIME_SYSADC: u64 = 1 << 12;
/// Cold/hard-reset request.
pub const CONTROL_HARD_RESET: u64 = 1 << 11;
/// R5000 warm-reset request.
pub const CONTROL_SOFT_RESET: u64 = 1 << 10;
/// Watchdog enable.
pub const CONTROL_WATCHDOG_ENABLE: u64 = 1 << 9;
/// Big-endian bus mode.
pub const CONTROL_BIG_ENDIAN: u64 = 1 << 8;

/// Watchdog power-on-reset timeout flag.
pub const WATCHDOG_POWER_ON_RESET: u64 = 1 << 16;
/// Watchdog warm-reset timeout flag.
pub const WATCHDOG_WARM_RESET: u64 = 1 << 19;
/// Watchdog counter bits retained by CRIME 1.1 software.
pub const WATCHDOG_VALUE_MASK: u64 = 0x7fff;

/// CPU illegal-address error.
pub const CPU_ERROR_ILLEGAL_ADDRESS: u64 = 1 << 2;
/// VICE write-parity error.
pub const CPU_ERROR_VICE_WRITE_PARITY: u64 = 1 << 1;
/// CPU write-parity error.
pub const CPU_ERROR_CPU_WRITE_PARITY: u64 = 1;
/// Software-visible CPU error bits.
pub const CPU_ERROR_MASK: u64 = 0x7;

/// CPU interface error interrupt.
pub const INTERRUPT_CPU_ERROR: u32 = 1 << 20;
/// Memory controller error interrupt.
pub const INTERRUPT_MEMORY_ERROR: u32 = 1 << 21;
/// Rendering Engine FIFO-empty edge interrupt.
pub const INTERRUPT_RE_EMPTY_EDGE: u32 = 1 << 22;
/// Rendering Engine FIFO-full edge interrupt.
pub const INTERRUPT_RE_FULL_EDGE: u32 = 1 << 23;
/// Rendering Engine idle edge interrupt.
pub const INTERRUPT_RE_IDLE_EDGE: u32 = 1 << 24;
/// Rendering Engine FIFO-empty level interrupt.
pub const INTERRUPT_RE_EMPTY_LEVEL: u32 = 1 << 25;
/// Rendering Engine FIFO-full level interrupt.
pub const INTERRUPT_RE_FULL_LEVEL: u32 = 1 << 26;
/// Rendering Engine idle level interrupt.
pub const INTERRUPT_RE_IDLE_LEVEL: u32 = 1 << 27;
/// Software interrupt zero.
pub const INTERRUPT_SOFTWARE_ZERO: u32 = 1 << 28;
/// Software interrupt one.
pub const INTERRUPT_SOFTWARE_ONE: u32 = 1 << 29;
/// CRIME 1.1 software interrupt two.
pub const INTERRUPT_SOFTWARE_TWO: u32 = 1 << 30;
/// VICE interrupt.
pub const INTERRUPT_VICE: u32 = 1 << 31;
/// Software-writable interrupt bits.
pub const SOFTWARE_INTERRUPT_MASK: u32 = u32::MAX;
/// Hardware-interrupt bits whose latched state is software writable.
pub const HARDWARE_INTERRUPT_WRITABLE_MASK: u32 = INTERRUPT_SOFTWARE_TWO
    | INTERRUPT_SOFTWARE_ONE
    | INTERRUPT_SOFTWARE_ZERO
    | INTERRUPT_RE_IDLE_EDGE
    | INTERRUPT_RE_FULL_EDGE
    | INTERRUPT_RE_EMPTY_EDGE
    | (0xF << 16);

/// Bank-control bits implemented by CRIME.
pub const MEMORY_BANK_CONTROL_MASK: u16 = 0x011f;
/// Bank-control base field.
pub const MEMORY_BANK_ADDRESS_MASK: u16 = 0x001f;
/// Bank-control 128-MiB decode bit.
pub const MEMORY_BANK_SIZE_128_MIB: u16 = 0x0100;

/// Memory error status mask.
pub const MEMORY_ERROR_STATUS_MASK: u32 = 0x0ff7_ffff;
/// CPU memory-access error.
pub const MEMORY_ERROR_CPU_ACCESS: u32 = 0x0004_0000;
/// Correctable ECC error.
pub const MEMORY_ERROR_SOFT: u32 = 0x0010_0000;
/// Uncorrectable ECC error.
pub const MEMORY_ERROR_HARD: u32 = 0x0020_0000;
/// More than one memory error was observed before clearing status.
pub const MEMORY_ERROR_MULTIPLE: u32 = 0x0040_0000;
/// ECC error occurred during a read.
pub const MEMORY_ERROR_ECC_READ: u32 = 0x0080_0000;
/// ECC error occurred during read-modify-write.
pub const MEMORY_ERROR_ECC_RMW: u32 = 0x0100_0000;
/// Invalid memory read address.
pub const MEMORY_ERROR_INVALID_READ: u32 = 0x0200_0000;
/// Invalid memory write address.
pub const MEMORY_ERROR_INVALID_WRITE: u32 = 0x0400_0000;
/// Invalid memory read-modify-write address.
pub const MEMORY_ERROR_INVALID_RMW: u32 = 0x0800_0000;

/// Returns one bank-control register address.
pub const fn memory_bank_control(bank: usize) -> Option<u64> {
    if bank < 8 {
        Some(MEMORY_BANK_CONTROL_BASE + bank as u64 * 8)
    } else {
        None
    }
}

/// Returns the bank index decoded by a bank-control address.
pub const fn memory_bank_index(address: u64) -> Option<usize> {
    if address < MEMORY_BANK_CONTROL_BASE || address > MEMORY_BANK_CONTROL_BASE + 7 * 8 {
        return None;
    }
    let offset = address - MEMORY_BANK_CONTROL_BASE;
    if !offset.is_multiple_of(8) {
        return None;
    }
    Some((offset / 8) as usize)
}

#[cfg(test)]
mod tests;
