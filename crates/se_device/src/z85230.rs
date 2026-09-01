//! Zilog Z85230 serial communications controller front end.

use se_core::bus::{BusFault, DeviceAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

const CHANNEL_B_CONTROL: u64 = 0x03;
const CHANNEL_B_DATA: u64 = 0x07;
const CHANNEL_A_CONTROL: u64 = 0x0b;
const CHANNEL_A_DATA: u64 = 0x0f;

const TRANSMIT_BUFFER_EMPTY: u8 = 1 << 2;
const ALL_SENT: u8 = 1;
const WHOLE_CHIP_RESET: u8 = 0xc0;
const TRANSMIT_FIFO_BYTES: usize = 4;

/// A channel within one Z85230 controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Channel {
    /// Channel A.
    A,
    /// Channel B.
    B,
}

/// The two-channel state needed by the IP12 serial path.
pub struct Z85230 {
    clock_hz: u64,
    channels: [ChannelState; 2],
}

impl Z85230 {
    /// Creates a serial controller in its reset state.
    ///
    /// # Panics
    ///
    /// Panics if `clock_hz` is zero.
    #[must_use]
    pub const fn new(clock_hz: u64) -> Self {
        assert!(clock_hz != 0);
        Self {
            clock_hz,
            channels: [ChannelState::new(), ChannelState::new()],
        }
    }

    /// Restores both channels to their reset state.
    pub fn reset(&mut self) {
        self.channels = [ChannelState::new(), ChannelState::new()];
    }

    /// Advances both transmitters and reports completed characters.
    pub fn advance_time(&mut self, elapsed: VirtualDuration, mut output: impl FnMut(Channel, u8)) {
        self.channels[0].advance_time(self.clock_hz, elapsed, |value| {
            output(Channel::A, value);
        });
        self.channels[1].advance_time(self.clock_hz, elapsed, |value| {
            output(Channel::B, value);
        });
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
            Port::Data(_) => data[0] = 0,
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
            Port::Data(_) => data[0] = 0,
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
                } else {
                    self.channels[channel].start_transmitter(self.clock_hz);
                }
            }
            Port::Data(channel) => {
                self.channels[channel].write_data(data[0]);
                self.channels[channel].start_transmitter(self.clock_hz);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct ActiveCharacter {
    value: u8,
    remaining_attoseconds: u128,
}

#[derive(Clone, Copy)]
struct ChannelState {
    selected_register: u8,
    write_registers: [u8; 7],
    transmit_fifo: [u8; TRANSMIT_FIFO_BYTES],
    transmit_fifo_head: usize,
    transmit_fifo_length: usize,
    active_character: Option<ActiveCharacter>,
    timing_remainder: u64,
}

impl ChannelState {
    const fn new() -> Self {
        Self {
            selected_register: 0,
            write_registers: [0; 7],
            transmit_fifo: [0; TRANSMIT_FIFO_BYTES],
            transmit_fifo_head: 0,
            transmit_fifo_length: 0,
            active_character: None,
            timing_remainder: 0,
        }
    }

    fn read_control(&mut self) -> u8 {
        let value = self.peek_control();
        self.selected_register = 0;
        value
    }

    const fn peek_control(&self) -> u8 {
        match self.selected_register {
            0 if self.transmit_fifo_length < TRANSMIT_FIFO_BYTES => TRANSMIT_BUFFER_EMPTY,
            1 if self.transmit_fifo_length == 0 && self.active_character.is_none() => ALL_SENT,
            _ => 0,
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

    fn write_data(&mut self, value: u8) {
        if self.transmit_fifo_length == TRANSMIT_FIFO_BYTES {
            return;
        }

        let tail = (self.transmit_fifo_head + self.transmit_fifo_length) % TRANSMIT_FIFO_BYTES;
        self.transmit_fifo[tail] = value;
        self.transmit_fifo_length += 1;
    }

    fn start_transmitter(&mut self, clock_hz: u64) {
        if self.active_character.is_some() || self.transmit_fifo_length == 0 {
            return;
        }
        let Some(frame_units) = self.frame_units() else {
            return;
        };

        let value = self.transmit_fifo[self.transmit_fifo_head];
        self.transmit_fifo_head = (self.transmit_fifo_head + 1) % TRANSMIT_FIFO_BYTES;
        self.transmit_fifo_length -= 1;

        let numerator = frame_units * ATTOSECONDS_PER_SECOND + u128::from(self.timing_remainder);
        let clock_hz = u128::from(clock_hz);
        let remaining_attoseconds = numerator / clock_hz;
        self.timing_remainder = u64::try_from(numerator % clock_hz)
            .expect("serial timing remainder must be smaller than its clock frequency");
        self.active_character = Some(ActiveCharacter {
            value,
            remaining_attoseconds,
        });
    }

    fn advance_time(
        &mut self,
        clock_hz: u64,
        elapsed: VirtualDuration,
        mut output: impl FnMut(u8),
    ) {
        let mut remaining = elapsed.as_attoseconds();
        loop {
            self.start_transmitter(clock_hz);
            let Some(active) = self.active_character.as_mut() else {
                return;
            };
            if remaining < active.remaining_attoseconds {
                active.remaining_attoseconds -= remaining;
                return;
            }

            remaining -= active.remaining_attoseconds;
            let value = active.value;
            self.active_character = None;
            output(value);
        }
    }

    fn frame_units(&self) -> Option<u128> {
        let wr4 = self.write_register(4);
        let wr5 = self.write_register(5);
        let wr11 = self.write_register(11);
        let wr14 = self.write_register(14);

        if wr5 & 0x08 == 0 || wr11 & 0x18 != 0x10 || wr14 & 0x01 == 0 {
            return None;
        }

        let stop_half_bits: u128 = match (wr4 >> 2) & 0x03 {
            0 => return None,
            1 => 2,
            2 => 3,
            3 => 4,
            _ => unreachable!(),
        };
        let data_bits: u128 = match (wr5 >> 5) & 0x03 {
            0 => 5,
            1 => 7,
            2 => 6,
            3 => 8,
            _ => unreachable!(),
        };
        let clock_factor: u128 = match wr4 >> 6 {
            0 => 1,
            1 => 16,
            2 => 32,
            3 => 64,
            _ => unreachable!(),
        };
        let parity_half_bits: u128 = if wr4 & 1 == 0 { 0 } else { 2 };
        let frame_half_bits = 2 + data_bits * 2 + parity_half_bits + stop_half_bits;
        let time_constant = u16::from_le_bytes([self.write_register(12), self.write_register(13)]);

        Some(frame_half_bits * (u128::from(time_constant) + 2) * clock_factor)
    }

    const fn write_register(&self, register: u8) -> u8 {
        match stored_register_index(register) {
            Some(index) => self.write_registers[index],
            None => 0,
        }
    }
}

#[derive(Clone, Copy)]
enum Port {
    Control(usize),
    Data(usize),
}

fn decode_port(address: DeviceAddr, length: usize) -> Result<Port, BusFault> {
    if length != 1 {
        return Err(BusFault::UnsupportedAccess);
    }

    match address.get() {
        CHANNEL_B_CONTROL => Ok(Port::Control(1)),
        CHANNEL_B_DATA => Ok(Port::Data(1)),
        CHANNEL_A_CONTROL => Ok(Port::Control(0)),
        CHANNEL_A_DATA => Ok(Port::Data(0)),
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
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use super::{
        ALL_SENT, CHANNEL_A_CONTROL, CHANNEL_A_DATA, CHANNEL_B_CONTROL, CHANNEL_B_DATA, Channel,
        TRANSMIT_BUFFER_EMPTY, Z85230, stored_register_index,
    };

    const CLOCK_HZ: u64 = 3_686_400;
    const CHARACTER_ATTOSECONDS: u128 = ATTOSECONDS_PER_SECOND / 960;

    fn read_port(serial: &mut Z85230, port: u64) -> Result<u8, BusFault> {
        let mut value = [0];
        serial.read(DeviceAddr::new(port), &mut value)?;
        Ok(value[0])
    }

    fn write_register(serial: &mut Z85230, control: u64, register: u8, value: u8) {
        serial.write(DeviceAddr::new(control), &[register]).unwrap();
        serial.write(DeviceAddr::new(control), &[value]).unwrap();
    }

    fn configure_9600_8n1(serial: &mut Z85230, control: u64) {
        write_register(serial, control, 4, 0x44);
        write_register(serial, control, 11, 0x10);
        write_register(serial, control, 12, 10);
        write_register(serial, control, 13, 0);
        write_register(serial, control, 14, 1);
        write_register(serial, control, 5, 0x68);
    }

    #[test]
    fn reset_status_has_no_receive_character_and_an_available_transmit_fifo() {
        let mut serial = Z85230::new(CLOCK_HZ);

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
        let mut serial = Z85230::new(CLOCK_HZ);
        let register = 5;

        write_register(&mut serial, CHANNEL_A_CONTROL, register, 0x6a);

        assert_eq!(serial.channels[0].selected_register, 0);
        assert_eq!(
            serial.channels[0].write_registers[stored_register_index(register).unwrap()],
            0x6a
        );
    }

    #[test]
    fn rr1_all_sent_tracks_the_active_transmitter() {
        let mut serial = Z85230::new(CLOCK_HZ);

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1])
            .unwrap();
        assert_eq!(read_port(&mut serial, CHANNEL_A_CONTROL), Ok(ALL_SENT));

        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1])
            .unwrap();
        assert_eq!(read_port(&mut serial, CHANNEL_A_CONTROL), Ok(0));

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1])
            .unwrap();
        assert_eq!(read_port(&mut serial, CHANNEL_A_CONTROL), Ok(ALL_SENT));
    }

    #[test]
    fn point_high_selects_registers_eight_through_fifteen() {
        let mut serial = Z85230::new(CLOCK_HZ);

        write_register(&mut serial, CHANNEL_A_CONTROL, 0x0b, 0x50);

        assert_eq!(
            serial.channels[0].write_registers[stored_register_index(11).unwrap()],
            0x50
        );
    }

    #[test]
    fn whole_chip_reset_clears_timing_and_both_channels() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        serial.advance_time(VirtualDuration::from_attoseconds(1), |_, _| {});

        write_register(&mut serial, CHANNEL_B_CONTROL, 9, 0xc0);

        for channel in &serial.channels {
            assert_eq!(channel.selected_register, 0);
            assert_eq!(channel.write_registers, [0; 7]);
            assert_eq!(channel.transmit_fifo_length, 0);
            assert!(channel.active_character.is_none());
            assert_eq!(channel.timing_remainder, 0);
        }
    }

    #[test]
    fn debug_read_preserves_the_selected_register() {
        let mut serial = Z85230::new(CLOCK_HZ);
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
    fn prom_programming_derives_9600_8n1_timing() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        let active = serial.channels[0].active_character.unwrap();

        assert_eq!(active.remaining_attoseconds, CHARACTER_ATTOSECONDS);
    }

    #[test]
    fn character_is_delivered_only_at_its_timing_boundary() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        let mut output = Vec::new();

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS - 1),
            |channel, value| output.push((channel, value)),
        );
        assert!(output.is_empty());
        serial.advance_time(VirtualDuration::from_attoseconds(1), |channel, value| {
            output.push((channel, value));
        });

        assert_eq!(output, [(Channel::A, 0xa5)]);
    }

    #[test]
    fn timing_remainder_keeps_multiple_characters_exact() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        let mut output = Vec::new();

        for value in 0..4 {
            serial
                .write(DeviceAddr::new(CHANNEL_A_DATA), &[value])
                .unwrap();
        }
        serial.advance_time(
            VirtualDuration::from_attoseconds(4 * ATTOSECONDS_PER_SECOND / 960),
            |_, value| output.push(value),
        );

        assert_eq!(output, [0, 1, 2, 3]);
    }

    #[test]
    fn transmit_fifo_status_changes_only_when_the_fifo_is_full() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        for value in 0..5 {
            serial
                .write(DeviceAddr::new(CHANNEL_A_DATA), &[value])
                .unwrap();
        }

        assert_eq!(read_port(&mut serial, CHANNEL_A_CONTROL), Ok(0));

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(TRANSMIT_BUFFER_EMPTY)
        );
    }

    #[test]
    fn active_character_keeps_frozen_timing_after_reprogramming() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), &[1]).unwrap();
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), &[2]).unwrap();
        write_register(&mut serial, CHANNEL_A_CONTROL, 12, 4);
        let mut output = Vec::new();

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, value| output.push(value),
        );
        assert_eq!(output, [1]);
        let second_duration = serial.channels[0]
            .active_character
            .expect("the second character should have started")
            .remaining_attoseconds;
        serial.advance_time(
            VirtualDuration::from_attoseconds(second_duration),
            |_, value| output.push(value),
        );

        assert_eq!(output, [1, 2]);
    }

    #[test]
    fn both_channel_identities_are_reported() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        configure_9600_8n1(&mut serial, CHANNEL_B_CONTROL);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_B_DATA), &[0x5a])
            .unwrap();
        let mut output = Vec::new();

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |channel, value| output.push((channel, value)),
        );

        assert_eq!(output, [(Channel::A, 0xa5), (Channel::B, 0x5a)]);
    }

    #[test]
    fn rejects_invalid_ports_and_widths() {
        let mut serial = Z85230::new(CLOCK_HZ);

        assert_eq!(
            serial.write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1, 2]),
            Err(BusFault::UnsupportedAccess)
        );
        assert_eq!(
            serial.write(DeviceAddr::new(0), &[1]),
            Err(BusFault::Unmapped)
        );
    }

    #[test]
    #[should_panic]
    fn rejects_zero_clock_frequency() {
        let _ = Z85230::new(0);
    }
}
