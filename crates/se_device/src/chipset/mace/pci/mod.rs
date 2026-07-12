//! MACE 2.0 PCI host-bridge register and error state.

/// PCI error flags implemented by the MACE host bridge.
pub mod error {
    pub const SIGNALED_TARGET_ABORT: u32 = 1 << 4;
    pub const RETRY_ADDRESS: u32 = 1 << 16;
    pub const PARITY_ADDRESS: u32 = 1 << 17;
    pub const TARGET_ABORT_ADDRESS: u32 = 1 << 18;
    pub const MASTER_ABORT_ADDRESS: u32 = 1 << 19;
    pub const CONFIG_ADDRESS: u32 = 1 << 20;
    pub const MEMORY_ADDRESS: u32 = 1 << 21;
    pub const SIGNALED_SYSTEM_ERROR: u32 = 1 << 22;
    pub const OVERRUN: u32 = 1 << 23;
    pub const PARITY: u32 = 1 << 24;
    pub const INTERRUPT_TEST: u32 = 1 << 25;
    pub const SYSTEM_ERROR: u32 = 1 << 26;
    pub const ILLEGAL_TRANSACTION: u32 = 1 << 27;
    pub const RETRY: u32 = 1 << 28;
    pub const DATA_PARITY: u32 = 1 << 29;
    pub const TARGET_ABORT: u32 = 1 << 30;
    pub const MASTER_ABORT: u32 = 1 << 31;
    pub const CLEARABLE: u32 = SIGNALED_TARGET_ABORT
        | OVERRUN
        | PARITY
        | INTERRUPT_TEST
        | SYSTEM_ERROR
        | ILLEGAL_TRANSACTION
        | RETRY
        | DATA_PARITY
        | TARGET_ABORT
        | MASTER_ABORT;
}

/// MACE PCI host bridge state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MacePci {
    pub error_address: u32,
    pub error_flags: u32,
    pub control: u32,
    pub config_address: u32,
    pub prefetch_valid: [bool; 16],
}

impl MacePci {
    pub const fn new() -> Self {
        Self {
            error_address: 0,
            error_flags: 0x42,
            control: 0,
            config_address: 0,
            prefetch_valid: [false; 16],
        }
    }
    pub fn reset(&mut self) {
        *self = Self::new();
    }
    pub fn write_error_flags(&mut self, value: u32) {
        let clear = !value & error::CLEARABLE;
        if clear & error::RETRY != 0 {
            self.error_flags &= !error::RETRY_ADDRESS;
        }
        if clear & error::DATA_PARITY != 0 {
            self.error_flags &= !error::PARITY_ADDRESS;
        }
        if clear & error::TARGET_ABORT != 0 {
            self.error_flags &= !error::TARGET_ABORT_ADDRESS;
        }
        if clear & error::MASTER_ABORT != 0 {
            self.error_flags &= !error::MASTER_ABORT_ADDRESS;
        }
        self.error_flags &= !clear;
    }
    pub fn record_error(&mut self, address: u32, flag: u32, address_flag: u32) {
        if self.error_flags & 0xffff_0000 == 0 {
            self.error_address = address;
            self.error_flags |= address_flag;
        }
        self.error_flags |= flag;
    }
    pub fn flush_prefetch(&mut self) {
        self.prefetch_valid.fill(false);
    }
    pub const fn error_interrupt(&self) -> bool {
        self.error_flags & self.control & 0xff00_0000 != 0
    }
    pub const fn pci_irq_enabled(&self, input: u8) -> bool {
        input < 8 && self.control & (1 << input) != 0
    }
}

impl Default for MacePci {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn first_error_latches_address_and_clear_removes_valid_flag() {
        let mut pci = MacePci::new();
        pci.record_error(0x1234, error::MASTER_ABORT, error::MASTER_ABORT_ADDRESS);
        pci.record_error(0x5678, error::TARGET_ABORT, error::TARGET_ABORT_ADDRESS);
        assert_eq!(pci.error_address, 0x1234);
        pci.write_error_flags(!error::MASTER_ABORT);
        assert_eq!(pci.error_flags & error::MASTER_ABORT_ADDRESS, 0);
    }
}
