//! Zilog Z85230 serial communications controller front end.

use se_core::bus::{BusFault, DeviceAddr};

const CHANNEL_B_CONTROL: u64 = 0x03;
const CHANNEL_B_DATA: u64 = 0x07;
const CHANNEL_A_CONTROL: u64 = 0x0b;
const CHANNEL_A_DATA: u64 = 0x0f;

const TRANSMIT_BUFFER_EMPTY: u8 = 1 << 2;
const WHOLE_CHIP_RESET: u8 = 0xc0;

/// The two-channel state needed by the IP12 serial initialization path.
pub struct Z85230 {
    channels: [Channel; 2],
}

impl Z85230 {
    /// Creates a serial controller in its reset state.
    #[must_use]
    #[allow(
        clippy::new_without_default,
        reason = "device construction is intentionally explicit"
    )]
    pub const fn new() -> Self {
        Self {
            channels: [Channel::new(), Channel::new()],
        }
    }

    /// Restores both channels to their reset state.
    pub fn reset(&mut self) {
        self.channels = [Channel::new(), Channel::new()];
    }

    /// Reads one byte from a channel control or data port.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Control(channel) => data[0] = self.channels[channel].read_control(),
            Port::Data => data[0] = 0,
        }
        Ok(())
    }

    /// Reads one byte without changing a register pointer.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn debug_read(&self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Control(channel) => data[0] = self.channels[channel].peek_control(),
            Port::Data => data[0] = 0,
        }
        Ok(())
    }

    /// Writes one byte to a channel control or data port.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn write(&mut self, address: DeviceAddr, data: &[u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Control(channel) => {
                if self.channels[channel].write_control(data[0]) {
                    self.reset();
                }
            }
            Port::Data => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct Channel {
    selected_register: u8,
    write_registers: [u8; 7],
}

impl Channel {
    const fn new() -> Self {
        Self {
            selected_register: 0,
            write_registers: [0; 7],
        }
    }

    fn read_control(&mut self) -> u8 {
        let value = self.peek_control();
        self.selected_register = 0;
        value
    }

    const fn peek_control(&self) -> u8 {
        if self.selected_register == 0 {
            TRANSMIT_BUFFER_EMPTY
        } else {
            0
        }
    }

    fn write_control(&mut self, value: u8) -> bool {
        if self.selected_register == 0 {
            self.selected_register = (value & 0x07) | (value & 0x08);
            return false;
        }

        let register = self.selected_register;
        self.selected_register = 0;
        if register == 9 && value & 0xc0 == WHOLE_CHIP_RESET {
            return true;
        }

        if let Some(index) = stored_register_index(register) {
            self.write_registers[index] = value;
        }
        false
    }
}

#[derive(Clone, Copy)]
enum Port {
    Control(usize),
    Data,
}

fn decode_port(address: DeviceAddr, length: usize) -> Result<Port, BusFault> {
    if length != 1 {
        return Err(BusFault::UnsupportedAccess);
    }

    match address.get() {
        CHANNEL_B_CONTROL => Ok(Port::Control(1)),
        CHANNEL_B_DATA => Ok(Port::Data),
        CHANNEL_A_CONTROL => Ok(Port::Control(0)),
        CHANNEL_A_DATA => Ok(Port::Data),
        _ => Err(BusFault::Unmapped),
    }
}

const fn stored_register_index(register: u8) -> Option<usize> {
    match register {
        3 => Some(0),
        4 => Some(1),
        5 => Some(2),
        11 => Some(3),
        12 => Some(4),
        13 => Some(5),
        14 => Some(6),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};

    use super::{
        CHANNEL_A_CONTROL, CHANNEL_A_DATA, CHANNEL_B_CONTROL, TRANSMIT_BUFFER_EMPTY, Z85230,
        stored_register_index,
    };

    fn read_port(serial: &mut Z85230, port: u64) -> Result<u8, BusFault> {
        let mut value = [0];
        serial.read(DeviceAddr::new(port), &mut value)?;
        Ok(value[0])
    }

    #[test]
    fn reset_status_has_no_receive_character_and_an_empty_transmitter() {
        let mut serial = Z85230::new();

        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(TRANSMIT_BUFFER_EMPTY)
        );
        assert_eq!(
            read_port(&mut serial, CHANNEL_B_CONTROL),
            Ok(TRANSMIT_BUFFER_EMPTY)
        );
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(0));
    }

    #[test]
    fn control_port_uses_select_then_write_protocol() {
        let mut serial = Z85230::new();
        let register = 5;

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[register])
            .unwrap();
        assert_eq!(serial.channels[0].selected_register, register);
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[0x6a])
            .unwrap();

        assert_eq!(serial.channels[0].selected_register, 0);
        assert_eq!(
            serial.channels[0].write_registers[stored_register_index(register).unwrap()],
            0x6a
        );
    }

    #[test]
    fn point_high_selects_registers_eight_through_fifteen() {
        let mut serial = Z85230::new();

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[0x0b])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[0x50])
            .unwrap();

        assert_eq!(
            serial.channels[0].write_registers[stored_register_index(11).unwrap()],
            0x50
        );
    }

    #[test]
    fn whole_chip_reset_clears_both_channels() {
        let mut serial = Z85230::new();
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[5])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[0x6a])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_B_CONTROL), &[3])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_B_CONTROL), &[0xc1])
            .unwrap();

        serial
            .write(DeviceAddr::new(CHANNEL_B_CONTROL), &[9])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_B_CONTROL), &[0xc0])
            .unwrap();

        for channel in &serial.channels {
            assert_eq!(channel.selected_register, 0);
            assert_eq!(channel.write_registers, [0; 7]);
        }
    }

    #[test]
    fn debug_read_preserves_the_selected_register() {
        let mut serial = Z85230::new();
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[5])
            .unwrap();
        let mut value = [0xff];

        serial
            .debug_read(DeviceAddr::new(CHANNEL_A_CONTROL), &mut value)
            .unwrap();

        assert_eq!(value, [0]);
        assert_eq!(serial.channels[0].selected_register, 5);
    }

    #[test]
    fn data_writes_leave_the_transmitter_empty() {
        let mut serial = Z85230::new();

        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();

        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(TRANSMIT_BUFFER_EMPTY)
        );
    }

    #[test]
    fn rejects_invalid_ports_and_widths() {
        let mut serial = Z85230::new();

        assert_eq!(
            serial.write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1, 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            serial.write(DeviceAddr::new(0), &[1]),
            Err(BusFault::Unmapped)
        );
    }
}
