//! Western Digital WD33C93B indirect-register front end.

use se_core::bus::{BusFault, DeviceAddr};

const ADDRESS_PORT: u64 = 0;
const DATA_PORT: u64 = 4;

const TIMEOUT_PERIOD: u8 = 0x02;
const SCSI_STATUS: u8 = 0x17;
const COMMAND: u8 = 0x18;
const DATA: u8 = 0x19;
const AUXILIARY_STATUS: u8 = 0x1f;

const INTERRUPT_PENDING: u8 = 0x80;
const SOFTWARE_RESET: u8 = 0x00;
const RESET_COMPLETION_STATUS: u8 = 0x00;

/// The WD33C93B state needed by the IP12 reset diagnostics.
pub struct Wd33c93b {
    selected_register: u8,
    timeout_period: u8,
    scsi_status: u8,
    interrupt_pending: bool,
}

impl Wd33c93b {
    /// Creates a controller after reset completion.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            selected_register: 0,
            timeout_period: 0,
            scsi_status: RESET_COMPLETION_STATUS,
            interrupt_pending: true,
        }
    }

    /// Restores the controller reset state.
    pub fn reset(&mut self) {
        self.selected_register = 0;
        self.timeout_period = 0;
        self.scsi_status = RESET_COMPLETION_STATUS;
        self.interrupt_pending = true;
    }

    /// Reads one byte from the indirect interface.
    ///
    /// Reading SCSI Status acknowledges the current interrupt and advances the
    /// selector to Command.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Address => data[0] = self.auxiliary_status(),
            Port::Data => {
                let register = self.selected_register;
                data[0] = self.read_selected_register(register);
                if register == SCSI_STATUS {
                    self.interrupt_pending = false;
                    self.selected_register = COMMAND;
                } else if selector_advances(register) {
                    self.selected_register = register.wrapping_add(1) & 0x1f;
                }
            }
        }
        Ok(())
    }

    /// Reads one byte without acknowledging interrupts or advancing the selector.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Address => data[0] = self.auxiliary_status(),
            Port::Data => data[0] = self.read_selected_register(self.selected_register),
        }
        Ok(())
    }

    /// Writes one byte to the indirect interface.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Address => self.selected_register = data[0] & 0x1f,
            Port::Data => {
                let register = self.selected_register;
                match register {
                    TIMEOUT_PERIOD => self.timeout_period = data[0],
                    COMMAND if data[0] == SOFTWARE_RESET => self.software_reset(),
                    _ => {}
                }
                if selector_advances(register) {
                    self.selected_register = register.wrapping_add(1) & 0x1f;
                }
            }
        }
        Ok(())
    }

    const fn auxiliary_status(&self) -> u8 {
        if self.interrupt_pending {
            INTERRUPT_PENDING
        } else {
            0
        }
    }

    const fn read_selected_register(&self, register: u8) -> u8 {
        match register {
            TIMEOUT_PERIOD => self.timeout_period,
            SCSI_STATUS => self.scsi_status,
            AUXILIARY_STATUS => self.auxiliary_status(),
            _ => 0xff,
        }
    }

    fn software_reset(&mut self) {
        self.timeout_period = 0;
        self.scsi_status = RESET_COMPLETION_STATUS;
        self.interrupt_pending = true;
    }
}

#[derive(Clone, Copy)]
enum Port {
    Address,
    Data,
}

fn decode_port(address: DeviceAddr, length: usize) -> Result<Port, BusFault> {
    if length != 1 {
        return Err(BusFault::UnsupportedAccess);
    }

    match address.get() {
        ADDRESS_PORT => Ok(Port::Address),
        DATA_PORT => Ok(Port::Data),
        _ => Err(BusFault::Unmapped),
    }
}

const fn selector_advances(register: u8) -> bool {
    !matches!(register, COMMAND | DATA | AUXILIARY_STATUS)
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{
        ADDRESS_PORT, AUXILIARY_STATUS, COMMAND, DATA_PORT, INTERRUPT_PENDING, SCSI_STATUS,
        TIMEOUT_PERIOD, Wd33c93b,
    };

    fn read_port(scsi: &mut Wd33c93b, port: u64) -> Result<u8, BusFault> {
        let mut value = [0];
        scsi.read(DeviceAddr::new(port), &mut value)?;
        Ok(value[0])
    }

    #[test]
    fn address_port_selects_low_five_bits_and_reads_auxiliary_status() {
        let mut scsi = Wd33c93b::new();

        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(INTERRUPT_PENDING));
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[0xe2]).unwrap();
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD);
    }

    #[test]
    fn timeout_period_round_trips_and_auto_increments() {
        let mut scsi = Wd33c93b::new();

        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[TIMEOUT_PERIOD])
            .unwrap();
        scsi.write(DeviceAddr::new(DATA_PORT), &[0xa5]).unwrap();
        assert_eq!(scsi.timeout_period, 0xa5);
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD + 1);

        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[TIMEOUT_PERIOD])
            .unwrap();
        assert_eq!(read_port(&mut scsi, DATA_PORT), Ok(0xa5));
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD + 1);
    }

    #[test]
    fn status_read_acknowledges_interrupt_and_selects_command() {
        let mut scsi = Wd33c93b::new();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[SCSI_STATUS])
            .unwrap();

        assert_eq!(read_port(&mut scsi, DATA_PORT), Ok(0));
        assert!(!scsi.interrupt_pending);
        assert_eq!(scsi.selected_register, COMMAND);
        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(0));
    }

    #[test]
    fn debug_status_read_has_no_side_effects() {
        let mut scsi = Wd33c93b::new();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[SCSI_STATUS])
            .unwrap();
        let mut value = [0xff];

        assert_eq!(
            scsi.debug_read(DeviceAddr::new(DATA_PORT), &mut value),
            Ok(())
        );
        assert_eq!(value, [0]);
        assert!(scsi.interrupt_pending);
        assert_eq!(scsi.selected_register, SCSI_STATUS);
    }

    #[test]
    fn software_reset_publishes_completion_interrupt() {
        let mut scsi = Wd33c93b::new();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[SCSI_STATUS])
            .unwrap();
        read_port(&mut scsi, DATA_PORT).unwrap();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[TIMEOUT_PERIOD])
            .unwrap();
        scsi.write(DeviceAddr::new(DATA_PORT), &[0xa5]).unwrap();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[COMMAND])
            .unwrap();

        scsi.write(DeviceAddr::new(DATA_PORT), &[0]).unwrap();

        assert_eq!(scsi.timeout_period, 0);
        assert_eq!(scsi.selected_register, COMMAND);
        assert!(scsi.interrupt_pending);
        assert_eq!(read_port(&mut scsi, ADDRESS_PORT), Ok(INTERRUPT_PENDING));
    }

    #[test]
    fn undefined_registers_read_as_all_ones() {
        let mut scsi = Wd33c93b::new();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[0x03]).unwrap();

        assert_eq!(read_port(&mut scsi, DATA_PORT), Ok(0xff));
        assert_eq!(scsi.selected_register, 0x04);
    }

    #[test]
    fn special_registers_do_not_auto_increment() {
        let mut scsi = Wd33c93b::new();

        for register in [COMMAND, 0x19, AUXILIARY_STATUS] {
            scsi.write(DeviceAddr::new(ADDRESS_PORT), &[register])
                .unwrap();
            scsi.write(DeviceAddr::new(DATA_PORT), &[1]).unwrap();
            assert_eq!(scsi.selected_register, register);
        }
    }

    #[test]
    fn rejects_invalid_ports_and_widths_atomically() {
        let mut scsi = Wd33c93b::new();
        scsi.write(DeviceAddr::new(ADDRESS_PORT), &[TIMEOUT_PERIOD])
            .unwrap();

        assert_eq!(
            scsi.write(DeviceAddr::new(DATA_PORT), &[1, 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            scsi.write(DeviceAddr::new(2), &[1]),
            Err(BusFault::Unmapped)
        );
        assert_eq!(scsi.timeout_period, 0);
        assert_eq!(scsi.selected_register, TIMEOUT_PERIOD);
    }
}
