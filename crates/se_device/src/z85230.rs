//! Zilog Z85230 serial communications controller front end.

use se_core::bus::{BusFault, DeviceAddr};
use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

const CHANNEL_B_CONTROL: u64 = 0x03;
const CHANNEL_B_DATA: u64 = 0x07;
const CHANNEL_A_CONTROL: u64 = 0x0b;
const CHANNEL_A_DATA: u64 = 0x0f;

const RECEIVE_CHARACTER_AVAILABLE: u8 = 1;
const TRANSMIT_BUFFER_EMPTY: u8 = 1 << 2;
const ALL_SENT: u8 = 1;
const TRANSMIT_INTERRUPT_ENABLE: u8 = 1 << 1;
const TRANSMIT_INTERRUPT_FIFO_EMPTY: u8 = 1 << 5;
const ASYNC_EIGHT_BIT_RESIDUE: u8 = 0x06;
const RECEIVER_ENABLE: u8 = 1;
const AUTO_ENABLE: u8 = 1 << 5;
const TRANSMITTER_ENABLE: u8 = 1 << 3;
const LOCAL_LOOPBACK: u8 = 1 << 4;
const MASTER_INTERRUPT_ENABLE: u8 = 1 << 3;
const SOFTWARE_INTERRUPT_ACKNOWLEDGE: u8 = 1 << 5;
const SDLC_STATUS_FIFO_ENABLE: u8 = 1 << 2;
const EXTENDED_READ_ENABLE: u8 = 1 << 6;
const CHANNEL_B_RESET: u8 = 0x40;
const CHANNEL_A_RESET: u8 = 0x80;
const WHOLE_CHIP_RESET: u8 = 0xc0;
const RECEIVE_FIFO_BYTES: usize = 8;
const TRANSMIT_FIFO_BYTES: usize = 4;
const RESET_WRITE_REGISTERS: [u8; 16] = [0, 0, 0, 0, 0x04, 0, 0, 0, 0, 0, 0, 0x08, 0, 0, 0, 0xf8];
const RESET_WRITE_REGISTER_PRIME_SEVEN: u8 = 0x20;

/// A channel within one Z85230 controller.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Channel {
    /// Channel A.
    A,
    /// Channel B.
    B,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InterruptSource {
    AReceive,
    ATransmit,
    BReceive,
    BTransmit,
}

impl InterruptSource {
    const PRIORITY_ORDER: [Self; 4] = [
        Self::AReceive,
        Self::ATransmit,
        Self::BReceive,
        Self::BTransmit,
    ];

    const fn index(self) -> usize {
        match self {
            Self::AReceive => 0,
            Self::ATransmit => 1,
            Self::BReceive => 2,
            Self::BTransmit => 3,
        }
    }

    const fn channel(self) -> usize {
        match self {
            Self::AReceive | Self::ATransmit => 0,
            Self::BReceive | Self::BTransmit => 1,
        }
    }

    const fn pending_mask(self) -> u8 {
        match self {
            Self::AReceive => 1 << 5,
            Self::ATransmit => 1 << 4,
            Self::BReceive => 1 << 2,
            Self::BTransmit => 1 << 1,
        }
    }

    const fn vector_status(self) -> u8 {
        match self {
            Self::AReceive => 0b110,
            Self::ATransmit => 0b100,
            Self::BReceive => 0b010,
            Self::BTransmit => 0b000,
        }
    }
}

/// The two-channel state needed by the IP12 serial path.
pub struct Z85230 {
    clock_hz: u64,
    channels: [ChannelState; 2],
    interrupt_vector: u8,
    master_interrupt_control: u8,
    interrupt_under_service: [bool; 4],
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
            interrupt_vector: 0,
            master_interrupt_control: 0,
            interrupt_under_service: [false; 4],
        }
    }

    /// Restores both channels to their reset state.
    pub fn reset(&mut self) {
        self.reset_except_master_interrupt_control();
        self.master_interrupt_control = 0;
    }

    fn reset_except_master_interrupt_control(&mut self) {
        self.channels = [ChannelState::new(), ChannelState::new()];
        self.interrupt_vector = 0;
        self.interrupt_under_service = [false; 4];
    }

    /// Supplies host bytes to one receiver.
    ///
    /// Returns the number of bytes consumed. A disabled receiver consumes and
    /// discards the complete slice. An enabled receiver consumes only the
    /// prefix that fits in its receive FIFO.
    pub fn receive(&mut self, channel: Channel, bytes: &[u8]) -> usize {
        self.channels[channel_index(channel)].receive(bytes)
    }

    /// Reports the controller interrupt output level.
    #[must_use]
    pub fn interrupt_asserted(&self) -> bool {
        self.master_interrupt_control & MASTER_INTERRUPT_ENABLE != 0
            && self.highest_pending_interrupt().is_some()
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

    /// Returns the virtual duration until the next transmitted character.
    #[must_use]
    pub fn time_until_event(&self) -> Option<VirtualDuration> {
        self.channels
            .iter()
            .filter_map(|channel| channel.active_character)
            .map(|character| VirtualDuration::from_attoseconds(character.remaining_attoseconds))
            .min()
    }

    /// Reads one byte from a channel control or data port.
    ///
    /// # Errors
    ///
    /// Returns [`BusFault`] unless the transaction selects exactly one byte
    /// port.
    pub fn read(&mut self, address: DeviceAddr, data: &mut [u8]) -> Result<(), BusFault> {
        match decode_port(address, data.len())? {
            Port::Control(channel) => data[0] = self.read_control(channel),
            Port::Data(channel) => data[0] = self.channels[channel].read_data(),
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
            Port::Control(channel) => data[0] = self.peek_control(channel),
            Port::Data(channel) => data[0] = self.channels[channel].peek_data(),
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
            Port::Control(channel) => self.write_control(channel, data[0]),
            Port::Data(channel) => {
                if self.channels[channel].write_data(data[0]) {
                    self.channels[channel].start_transmitter(self.clock_hz);
                }
            }
        }
        Ok(())
    }

    fn read_control(&mut self, channel: usize) -> u8 {
        let register = self.channels[channel].take_selected_register();
        let value = match register {
            8 => self.channels[channel].read_data(),
            _ => self.register_value(channel, register),
        };
        if channel == 1
            && register == 2
            && self.master_interrupt_control & SOFTWARE_INTERRUPT_ACKNOWLEDGE != 0
            && let Some(source) = self.highest_pending_interrupt()
        {
            self.interrupt_under_service[source.index()] = true;
        }
        value
    }

    fn peek_control(&self, channel: usize) -> u8 {
        self.register_value(channel, self.channels[channel].selected_register)
    }

    fn register_value(&self, channel: usize, register: u8) -> u8 {
        let state = &self.channels[channel];
        let extended_read = state.write_register_prime_seven & EXTENDED_READ_ENABLE != 0;
        match register {
            0 => state.read_register_zero(),
            1 => state.read_register_one(),
            2 => self.read_interrupt_vector(channel),
            3 if channel == 0 => self.interrupt_pending_register(),
            4 if extended_read => state.write_register(4),
            4 => state.read_register_zero(),
            5 if extended_read => state.write_register(5),
            5 => state.read_register_one(),
            6 if state.write_register(15) & SDLC_STATUS_FIFO_ENABLE == 0 => {
                self.read_interrupt_vector(channel)
            }
            7 if state.write_register(15) & SDLC_STATUS_FIFO_ENABLE == 0 && channel == 0 => {
                self.interrupt_pending_register()
            }
            8 => state.peek_data(),
            9 if extended_read => state.write_register(3),
            9 => state.write_register(13),
            10 => 0,
            11 if extended_read => state.write_register(10),
            11 => state.read_register_fifteen(),
            12 | 13 => state.write_register(register),
            14 if extended_read => state.write_register_prime_seven,
            14 => state.write_register(10),
            15 => state.read_register_fifteen(),
            _ => 0,
        }
    }

    fn write_control(&mut self, channel: usize, value: u8) {
        let register = self.channels[channel].selected_register;
        if register == 0 {
            self.channels[channel].select_register(value);
            self.execute_command(channel, value);
            return;
        }

        self.channels[channel].selected_register = 0;
        match register {
            2 => self.interrupt_vector = value,
            9 => self.write_master_interrupt_control(value),
            7 if self.channels[channel].write_register(15) & 1 != 0 => {
                self.channels[channel].write_register_prime_seven(value);
            }
            _ => self.channels[channel].set_write_register(register, value),
        }
        self.channels[channel].start_transmitter(self.clock_hz);
    }

    fn execute_command(&mut self, channel: usize, value: u8) {
        match (value >> 3) & 0x07 {
            4 => self.channels[channel].enable_interrupt_on_next_receive_character(),
            5 => self.channels[channel].reset_transmit_interrupt_pending(),
            6 => self.channels[channel].reset_receive_errors(),
            7 => self.reset_highest_interrupt_under_service(),
            _ => {}
        }
    }

    fn write_master_interrupt_control(&mut self, value: u8) {
        match value & 0xc0 {
            CHANNEL_B_RESET => {
                self.channels[1] = ChannelState::new();
                self.reset_channel_interrupts_under_service(1);
            }
            CHANNEL_A_RESET => {
                self.channels[0] = ChannelState::new();
                self.reset_channel_interrupts_under_service(0);
            }
            WHOLE_CHIP_RESET => self.reset_except_master_interrupt_control(),
            _ => {}
        }
        self.master_interrupt_control = value & 0x3f;
    }

    fn highest_pending_interrupt(&self) -> Option<InterruptSource> {
        for source in InterruptSource::PRIORITY_ORDER {
            if self.interrupt_under_service[source.index()] {
                return None;
            }
            if self.interrupt_requested(source) {
                return Some(source);
            }
        }
        None
    }

    fn interrupt_requested(&self, source: InterruptSource) -> bool {
        let channel = &self.channels[source.channel()];
        match source {
            InterruptSource::AReceive | InterruptSource::BReceive => {
                channel.receive_interrupt_pending()
            }
            InterruptSource::ATransmit | InterruptSource::BTransmit => {
                channel.transmit_interrupt_pending && channel.transmit_interrupt_enabled()
            }
        }
    }

    fn reset_highest_interrupt_under_service(&mut self) {
        for source in InterruptSource::PRIORITY_ORDER {
            let under_service = &mut self.interrupt_under_service[source.index()];
            if *under_service {
                *under_service = false;
                return;
            }
        }
    }

    fn reset_channel_interrupts_under_service(&mut self, channel: usize) {
        for source in InterruptSource::PRIORITY_ORDER {
            if source.channel() == channel {
                self.interrupt_under_service[source.index()] = false;
            }
        }
    }

    fn interrupt_pending_register(&self) -> u8 {
        InterruptSource::PRIORITY_ORDER
            .into_iter()
            .filter(|source| match source {
                InterruptSource::AReceive | InterruptSource::BReceive => {
                    self.channels[source.channel()].receive_interrupt_pending()
                }
                InterruptSource::ATransmit | InterruptSource::BTransmit => {
                    self.channels[source.channel()].transmit_interrupt_pending
                }
            })
            .fold(0, |value, source| value | source.pending_mask())
    }

    fn read_interrupt_vector(&self, channel: usize) -> u8 {
        if channel == 0 {
            return self.interrupt_vector;
        }

        let status = self
            .highest_pending_interrupt()
            .map_or(0b011, InterruptSource::vector_status);
        if self.master_interrupt_control & 0x10 != 0 {
            self.interrupt_vector & 0x8f | status << 4
        } else {
            self.interrupt_vector & 0xf1 | status << 1
        }
    }
}

#[derive(Clone, Copy)]
struct ActiveCharacter {
    value: u8,
    remaining_attoseconds: u128,
    local_loopback: bool,
}

#[derive(Clone, Copy)]
struct ReceiveCharacter {
    value: u8,
    status: u8,
}

impl ReceiveCharacter {
    const EMPTY: Self = Self {
        value: 0,
        status: ASYNC_EIGHT_BIT_RESIDUE,
    };
}

#[derive(Clone, Copy)]
struct ChannelState {
    selected_register: u8,
    write_registers: [u8; 16],
    write_register_prime_seven: u8,
    receive_fifo: [ReceiveCharacter; RECEIVE_FIFO_BYTES],
    receive_fifo_head: usize,
    receive_fifo_length: usize,
    first_character_interrupt_armed: bool,
    receive_error_latch: u8,
    receive_fifo_locked: bool,
    transmit_fifo: [u8; TRANSMIT_FIFO_BYTES],
    transmit_fifo_head: usize,
    transmit_fifo_length: usize,
    transmit_interrupt_pending: bool,
    active_character: Option<ActiveCharacter>,
    timing_remainder: u64,
}

impl ChannelState {
    const fn new() -> Self {
        Self {
            selected_register: 0,
            write_registers: RESET_WRITE_REGISTERS,
            write_register_prime_seven: RESET_WRITE_REGISTER_PRIME_SEVEN,
            receive_fifo: [ReceiveCharacter::EMPTY; RECEIVE_FIFO_BYTES],
            receive_fifo_head: 0,
            receive_fifo_length: 0,
            first_character_interrupt_armed: false,
            receive_error_latch: 0,
            receive_fifo_locked: false,
            transmit_fifo: [0; TRANSMIT_FIFO_BYTES],
            transmit_fifo_head: 0,
            transmit_fifo_length: 0,
            transmit_interrupt_pending: false,
            active_character: None,
            timing_remainder: 0,
        }
    }

    fn take_selected_register(&mut self) -> u8 {
        let register = self.selected_register;
        self.selected_register = 0;
        register
    }

    fn select_register(&mut self, value: u8) {
        let mut register = value & 0x07;
        if (value >> 3) & 0x07 == 1 {
            register += 8;
        }
        self.selected_register = register;
    }

    fn read_register_zero(&self) -> u8 {
        let mut value = 0;
        if self.receive_fifo_length != 0 {
            value |= RECEIVE_CHARACTER_AVAILABLE;
        }
        if self.transmit_fifo_length < TRANSMIT_FIFO_BYTES {
            value |= TRANSMIT_BUFFER_EMPTY;
        }
        value
    }

    fn read_register_one(&self) -> u8 {
        let status = if self.receive_fifo_length == 0 {
            ASYNC_EIGHT_BIT_RESIDUE
        } else {
            self.receive_fifo[self.receive_fifo_head].status | self.receive_error_latch
        };
        if self.transmit_fifo_length == 0 && self.active_character.is_none() {
            status | ALL_SENT
        } else {
            status
        }
    }

    const fn write_register(&self, register: u8) -> u8 {
        self.write_registers[register as usize]
    }

    const fn read_register_fifteen(&self) -> u8 {
        self.write_register(15) & 0xfa
    }

    fn set_write_register(&mut self, register: u8, value: u8) {
        self.write_registers[register as usize] = value;
        if register == 1 {
            self.first_character_interrupt_armed = self.receive_interrupt_mode() == 1;
        }
    }

    fn write_register_prime_seven(&mut self, value: u8) {
        self.write_register_prime_seven = value;
    }

    fn receive(&mut self, bytes: &[u8]) -> usize {
        if !self.receiver_enabled() {
            return bytes.len();
        }

        self.enqueue_receive(bytes)
    }

    fn enqueue_receive(&mut self, bytes: &[u8]) -> usize {
        let consumed = bytes
            .len()
            .min(RECEIVE_FIFO_BYTES - self.receive_fifo_length);
        for value in &bytes[..consumed] {
            let tail = (self.receive_fifo_head + self.receive_fifo_length) % RECEIVE_FIFO_BYTES;
            self.receive_fifo[tail] = ReceiveCharacter {
                value: *value,
                status: ASYNC_EIGHT_BIT_RESIDUE,
            };
            self.receive_fifo_length += 1;
        }
        consumed
    }

    fn receive_local_loopback(&mut self, value: u8) {
        if self.write_register(3) & RECEIVER_ENABLE != 0 {
            let _ = self.enqueue_receive(&[value]);
        }
    }

    fn read_data(&mut self) -> u8 {
        if self.receive_fifo_length == 0 {
            return 0;
        }
        let character = self.receive_fifo[self.receive_fifo_head];
        if self.receive_fifo_locked {
            return character.value;
        }
        let mode = self.receive_interrupt_mode();
        if matches!(mode, 1 | 3) && character.status & 0xf0 != 0 {
            self.receive_error_latch = character.status & 0xf0;
            self.receive_fifo_locked = true;
            self.first_character_interrupt_armed = false;
            return character.value;
        }
        let value = character.value;
        self.receive_fifo_head = (self.receive_fifo_head + 1) % RECEIVE_FIFO_BYTES;
        self.receive_fifo_length -= 1;
        if mode == 1 {
            self.first_character_interrupt_armed = false;
        }
        value
    }

    fn peek_data(&self) -> u8 {
        if self.receive_fifo_length == 0 {
            0
        } else {
            self.receive_fifo[self.receive_fifo_head].value
        }
    }

    fn receiver_enabled(&self) -> bool {
        let wr3 = self.write_register(3);
        wr3 & RECEIVER_ENABLE != 0 && (wr3 & AUTO_ENABLE == 0 || dcd_asserted())
    }

    fn receive_interrupt_pending(&self) -> bool {
        match self.receive_interrupt_mode() {
            0 => false,
            1 => self.first_character_interrupt_armed && self.receive_fifo_length != 0,
            2 => {
                let threshold = if self.write_register_prime_seven & (1 << 3) == 0 {
                    1
                } else {
                    4
                };
                self.receive_fifo_length >= threshold || self.has_special_receive_condition()
            }
            3 => self.has_special_receive_condition(),
            _ => unreachable!(),
        }
    }

    fn receive_interrupt_mode(&self) -> u8 {
        self.write_register(1) >> 3 & 0x03
    }

    fn has_special_receive_condition(&self) -> bool {
        self.receive_error_latch != 0
            || (0..self.receive_fifo_length).any(|offset| {
                let index = (self.receive_fifo_head + offset) % RECEIVE_FIFO_BYTES;
                self.receive_fifo[index].status & 0xf0 != 0
            })
    }

    fn enable_interrupt_on_next_receive_character(&mut self) {
        self.first_character_interrupt_armed = true;
    }

    fn reset_receive_errors(&mut self) {
        if self.receive_fifo_locked {
            self.receive_fifo_head = (self.receive_fifo_head + 1) % RECEIVE_FIFO_BYTES;
            self.receive_fifo_length -= 1;
        }
        self.receive_error_latch = 0;
        self.receive_fifo_locked = false;
    }

    fn write_data(&mut self, value: u8) -> bool {
        if self.transmit_fifo_length == TRANSMIT_FIFO_BYTES {
            return false;
        }

        self.transmit_interrupt_pending = false;
        let tail = (self.transmit_fifo_head + self.transmit_fifo_length) % TRANSMIT_FIFO_BYTES;
        self.transmit_fifo[tail] = value;
        self.transmit_fifo_length += 1;
        true
    }

    fn start_transmitter(&mut self, clock_hz: u64) -> bool {
        if self.active_character.is_some() || self.transmit_fifo_length == 0 {
            return false;
        }
        let Some(frame_units) = self.frame_units() else {
            return false;
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
            local_loopback: self.write_register(14) & LOCAL_LOOPBACK != 0,
        });
        true
    }

    fn transmit_interrupt_enabled(&self) -> bool {
        self.write_register(1) & TRANSMIT_INTERRUPT_ENABLE != 0
    }

    fn latch_transmit_interrupt_if_ready(&mut self) {
        if !self.transmit_interrupt_enabled() {
            return;
        }
        let fifo_ready = if self.write_register_prime_seven & TRANSMIT_INTERRUPT_FIFO_EMPTY != 0 {
            self.transmit_fifo_length == 0
        } else {
            self.transmit_fifo_length < TRANSMIT_FIFO_BYTES
        };
        self.transmit_interrupt_pending |= fifo_ready;
    }

    fn reset_transmit_interrupt_pending(&mut self) {
        self.transmit_interrupt_pending = false;
    }

    fn advance_time(
        &mut self,
        clock_hz: u64,
        elapsed: VirtualDuration,
        mut output: impl FnMut(u8),
    ) {
        let mut remaining = elapsed.as_attoseconds();
        loop {
            if self.start_transmitter(clock_hz) {
                self.latch_transmit_interrupt_if_ready();
            }
            let Some(active) = self.active_character.as_mut() else {
                return;
            };
            if remaining < active.remaining_attoseconds {
                active.remaining_attoseconds -= remaining;
                return;
            }

            remaining -= active.remaining_attoseconds;
            let value = active.value;
            let local_loopback = active.local_loopback;
            self.active_character = None;
            output(value);
            self.latch_transmit_interrupt_if_ready();
            if local_loopback {
                self.receive_local_loopback(value);
            }
        }
    }

    fn frame_units(&self) -> Option<u128> {
        let wr4 = self.write_register(4);
        let wr5 = self.write_register(5);
        let wr11 = self.write_register(11);
        let wr14 = self.write_register(14);

        if wr5 & TRANSMITTER_ENABLE == 0
            || self.write_register(3) & AUTO_ENABLE != 0 && !cts_asserted()
            || wr11 & 0x18 != 0x10
            || wr14 & 0x01 == 0
        {
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

const fn channel_index(channel: Channel) -> usize {
    match channel {
        Channel::A => 0,
        Channel::B => 1,
    }
}

const fn dcd_asserted() -> bool {
    true
}

const fn cts_asserted() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use se_core::bus::{BusFault, DeviceAddr};
    use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration};

    use super::{
        ALL_SENT, ASYNC_EIGHT_BIT_RESIDUE, CHANNEL_A_CONTROL, CHANNEL_A_DATA, CHANNEL_B_CONTROL,
        CHANNEL_B_DATA, Channel, InterruptSource, MASTER_INTERRUPT_ENABLE,
        RECEIVE_CHARACTER_AVAILABLE, RESET_WRITE_REGISTER_PRIME_SEVEN, RESET_WRITE_REGISTERS,
        TRANSMIT_BUFFER_EMPTY, TRANSMIT_INTERRUPT_ENABLE, WHOLE_CHIP_RESET, Z85230,
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

    fn read_register(serial: &mut Z85230, control: u64, register: u8) -> u8 {
        serial.write(DeviceAddr::new(control), &[register]).unwrap();
        read_port(serial, control).unwrap()
    }

    fn configure_9600_8n1(serial: &mut Z85230, control: u64) {
        write_register(serial, control, 4, 0x44);
        write_register(serial, control, 11, 0x10);
        write_register(serial, control, 12, 10);
        write_register(serial, control, 13, 0);
        write_register(serial, control, 14, 1);
        write_register(serial, control, 5, 0x68);
    }

    fn configure_transmit_interrupt(serial: &mut Z85230, control: u64) {
        configure_9600_8n1(serial, control);
        write_register(serial, control, 1, TRANSMIT_INTERRUPT_ENABLE);
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
            serial.channels[0].write_registers[usize::from(register)],
            0x6a
        );
    }

    #[test]
    fn rr1_all_sent_tracks_the_active_transmitter() {
        let mut serial = Z85230::new(CLOCK_HZ);

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1])
            .unwrap();
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(ASYNC_EIGHT_BIT_RESIDUE | ALL_SENT)
        );

        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1])
            .unwrap();
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(ASYNC_EIGHT_BIT_RESIDUE)
        );

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[1])
            .unwrap();
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(ASYNC_EIGHT_BIT_RESIDUE | ALL_SENT)
        );
    }

    #[test]
    fn point_high_selects_registers_eight_through_fifteen() {
        let mut serial = Z85230::new(CLOCK_HZ);

        write_register(&mut serial, CHANNEL_A_CONTROL, 0x0b, 0x50);

        assert_eq!(serial.channels[0].write_registers[11], 0x50);
    }

    #[test]
    fn prom_enhanced_scc_detection_reads_wr7_prime_through_rr14() {
        let mut serial = Z85230::new(CLOCK_HZ);

        write_register(&mut serial, CHANNEL_A_CONTROL, 15, 0x01);
        write_register(&mut serial, CHANNEL_A_CONTROL, 7, 0x40);

        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 14), 0x40);
    }

    #[test]
    fn extended_read_maps_write_registers_to_the_documented_read_registers() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 0xa3);
        write_register(&mut serial, CHANNEL_A_CONTROL, 4, 0xa4);
        write_register(&mut serial, CHANNEL_A_CONTROL, 5, 0xa5);
        write_register(&mut serial, CHANNEL_A_CONTROL, 10, 0xaa);
        write_register(&mut serial, CHANNEL_A_CONTROL, 15, 0x01);
        write_register(&mut serial, CHANNEL_A_CONTROL, 7, 0x40);

        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 4), 0xa4);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 5), 0xa5);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 9), 0xa3);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 11), 0xaa);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 14), 0x40);
    }

    #[test]
    fn ordinary_read_aliases_follow_the_base_register_map() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 2, 0xa2);
        write_register(&mut serial, CHANNEL_A_CONTROL, 10, 0xaa);
        write_register(&mut serial, CHANNEL_A_CONTROL, 13, 0xad);
        write_register(&mut serial, CHANNEL_A_CONTROL, 15, 0xfb);
        write_register(&mut serial, CHANNEL_A_CONTROL, 7, 0x00);

        assert_eq!(
            read_register(&mut serial, CHANNEL_A_CONTROL, 4),
            TRANSMIT_BUFFER_EMPTY
        );
        assert_eq!(
            read_register(&mut serial, CHANNEL_A_CONTROL, 5),
            ASYNC_EIGHT_BIT_RESIDUE | ALL_SENT
        );
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 6), 0xa2);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 7), 0);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 9), 0xad);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 11), 0xfa);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 14), 0xaa);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 15), 0xfa);
    }

    #[test]
    fn whole_chip_reset_restores_timing_and_both_channels() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        serial.advance_time(VirtualDuration::from_attoseconds(1), |_, _| {});

        write_register(&mut serial, CHANNEL_B_CONTROL, 9, 0xc0);

        for channel in &serial.channels {
            assert_eq!(channel.selected_register, 0);
            assert_eq!(channel.write_registers, RESET_WRITE_REGISTERS);
            assert_eq!(
                channel.write_register_prime_seven,
                RESET_WRITE_REGISTER_PRIME_SEVEN
            );
            assert_eq!(channel.receive_fifo_length, 0);
            assert_eq!(channel.transmit_fifo_length, 0);
            assert!(!channel.transmit_interrupt_pending);
            assert!(channel.active_character.is_none());
            assert_eq!(channel.timing_remainder, 0);
        }
        assert_eq!(serial.master_interrupt_control, 0);
        assert_eq!(serial.interrupt_under_service, [false; 4]);
    }

    #[test]
    fn whole_chip_reset_programs_wr9_control_bits_from_the_same_write() {
        let mut serial = Z85230::new(CLOCK_HZ);

        write_register(
            &mut serial,
            CHANNEL_A_CONTROL,
            9,
            WHOLE_CHIP_RESET | MASTER_INTERRUPT_ENABLE,
        );

        assert_eq!(serial.master_interrupt_control, MASTER_INTERRUPT_ENABLE);
        configure_transmit_interrupt(&mut serial, CHANNEL_B_CONTROL);
        serial.write(DeviceAddr::new(CHANNEL_B_DATA), b"A").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert!(serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 1 << 1);
    }

    #[test]
    fn debug_read_preserves_the_selected_register() {
        let mut serial = Z85230::new(CLOCK_HZ);
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[12])
            .unwrap();
        let mut value = [0xff];

        serial
            .debug_read(DeviceAddr::new(CHANNEL_A_CONTROL), &mut value)
            .unwrap();

        assert_eq!(value, [0]);
        assert_eq!(serial.channels[0].selected_register, 12);
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
    fn local_loopback_delivers_a_completed_character_to_output_and_receive_fifo() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 14, 0x11);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        let mut output = Vec::new();

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS - 1),
            |channel, value| output.push((channel, value)),
        );
        assert!(output.is_empty());
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(TRANSMIT_BUFFER_EMPTY)
        );

        serial.advance_time(VirtualDuration::from_attoseconds(1), |channel, value| {
            output.push((channel, value));
        });

        assert_eq!(output, [(Channel::A, 0xa5)]);
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(RECEIVE_CHARACTER_AVAILABLE | TRANSMIT_BUFFER_EMPTY)
        );
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(0xa5));
    }

    #[test]
    fn local_loopback_does_not_deliver_to_a_disabled_receiver() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        write_register(&mut serial, CHANNEL_A_CONTROL, 14, 0x11);
        serial
            .write(DeviceAddr::new(CHANNEL_A_DATA), &[0xa5])
            .unwrap();
        let mut output = Vec::new();

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |channel, value| output.push((channel, value)),
        );

        assert_eq!(output, [(Channel::A, 0xa5)]);
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(TRANSMIT_BUFFER_EMPTY)
        );
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(0));
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
    fn default_transmit_interrupt_waits_for_the_fifo_to_empty() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_transmit_interrupt(&mut serial, CHANNEL_A_CONTROL);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);
        let mut output = Vec::new();

        for value in b"The " {
            serial
                .write(DeviceAddr::new(CHANNEL_A_DATA), &[*value])
                .unwrap();
            assert!(!serial.interrupt_asserted());
            assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 0);
        }

        serial.advance_time(
            VirtualDuration::from_attoseconds(2 * ATTOSECONDS_PER_SECOND / 960),
            |_, value| output.push(value),
        );
        assert_eq!(output, b"Th");
        assert!(!serial.interrupt_asserted());

        serial.advance_time(
            VirtualDuration::from_attoseconds(
                3 * ATTOSECONDS_PER_SECOND / 960 - 2 * ATTOSECONDS_PER_SECOND / 960,
            ),
            |_, value| output.push(value),
        );
        assert_eq!(output, b"The");
        assert!(serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 1 << 4);

        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"s").unwrap();
        assert!(!serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 0);
    }

    #[test]
    fn reset_transmit_interrupt_pending_waits_for_new_transmit_progression() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_transmit_interrupt(&mut serial, CHANNEL_A_CONTROL);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"A").unwrap();
        assert!(!serial.interrupt_asserted());
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert!(serial.interrupt_asserted());

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[5 << 3])
            .unwrap();
        assert!(!serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 0);

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert!(!serial.interrupt_asserted());

        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"B").unwrap();
        assert!(!serial.interrupt_asserted());
        serial.advance_time(
            VirtualDuration::from_attoseconds(2 * CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert!(serial.interrupt_asserted());
    }

    #[test]
    fn disabled_transmit_interrupt_does_not_accumulate_pending() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_9600_8n1(&mut serial, CHANNEL_A_CONTROL);
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"A").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );

        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 0);
        write_register(&mut serial, CHANNEL_A_CONTROL, 1, TRANSMIT_INTERRUPT_ENABLE);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);
        assert!(!serial.interrupt_asserted());

        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"B").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(2 * CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert!(serial.interrupt_asserted());
    }

    #[test]
    fn non_fifo_empty_mode_interrupts_when_the_fifo_is_not_full() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_transmit_interrupt(&mut serial, CHANNEL_A_CONTROL);
        write_register(&mut serial, CHANNEL_A_CONTROL, 15, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 7, 0);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);

        for value in 0..5 {
            serial
                .write(DeviceAddr::new(CHANNEL_A_DATA), &[value])
                .unwrap();
        }
        assert!(!serial.interrupt_asserted());
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), &[5]).unwrap();
        assert!(!serial.interrupt_asserted());

        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert!(serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 1 << 4);
    }

    #[test]
    fn rr3_reports_raw_transmit_pending_while_wr1_and_mie_gate_the_output() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_transmit_interrupt(&mut serial, CHANNEL_A_CONTROL);
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"A").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );

        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 0x10);
        assert!(!serial.interrupt_asserted());

        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);
        assert!(serial.interrupt_asserted());

        write_register(&mut serial, CHANNEL_A_CONTROL, 1, 0);
        assert!(!serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 0x10);

        write_register(&mut serial, CHANNEL_A_CONTROL, 1, TRANSMIT_INTERRUPT_ENABLE);
        assert!(serial.interrupt_asserted());
    }

    #[test]
    fn rr2_reports_receive_and_transmit_sources_in_priority_order() {
        let mut serial = Z85230::new(CLOCK_HZ);
        for control in [CHANNEL_A_CONTROL, CHANNEL_B_CONTROL] {
            configure_9600_8n1(&mut serial, control);
            write_register(&mut serial, control, 3, 1);
            write_register(
                &mut serial,
                control,
                1,
                (2 << 3) | TRANSMIT_INTERRUPT_ENABLE,
            );
        }
        write_register(&mut serial, CHANNEL_A_CONTROL, 2, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"A").unwrap();
        serial.write(DeviceAddr::new(CHANNEL_B_DATA), b"B").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert_eq!(serial.receive(Channel::A, b"a"), 1);
        assert_eq!(serial.receive(Channel::B, b"b"), 1);

        assert_eq!(read_register(&mut serial, CHANNEL_B_CONTROL, 2), 0x0d);
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(b'a'));
        assert_eq!(read_register(&mut serial, CHANNEL_B_CONTROL, 2), 0x09);

        write_register(&mut serial, CHANNEL_A_CONTROL, 9, (1 << 4) | (1 << 3));
        assert_eq!(read_register(&mut serial, CHANNEL_B_CONTROL, 2), 0x41);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);

        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"C").unwrap();
        assert_eq!(read_register(&mut serial, CHANNEL_B_CONTROL, 2), 0x05);
        assert_eq!(read_port(&mut serial, CHANNEL_B_DATA), Ok(b'b'));
        assert_eq!(read_register(&mut serial, CHANNEL_B_CONTROL, 2), 0x01);
    }

    #[test]
    fn higher_priority_transmit_interrupt_preempts_a_lower_ius() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_transmit_interrupt(&mut serial, CHANNEL_A_CONTROL);
        configure_transmit_interrupt(&mut serial, CHANNEL_B_CONTROL);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, (1 << 5) | (1 << 3));
        serial.write(DeviceAddr::new(CHANNEL_B_DATA), b"B").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 2), 0);
        assert_eq!(serial.interrupt_under_service, [false; 4]);
        assert!(serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_B_CONTROL, 2), 0);
        assert_eq!(serial.interrupt_under_service, [false, false, false, true]);
        assert!(!serial.interrupt_asserted());

        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"A").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        assert!(serial.interrupt_asserted());
        assert_eq!(read_register(&mut serial, CHANNEL_B_CONTROL, 2), 0x08);
        assert_eq!(serial.interrupt_under_service, [false, true, false, true]);
        assert!(!serial.interrupt_asserted());

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[7 << 3])
            .unwrap();
        assert_eq!(serial.interrupt_under_service, [false, false, false, true]);
        assert!(serial.interrupt_asserted());

        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"C").unwrap();
        assert!(!serial.interrupt_asserted());
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[7 << 3])
            .unwrap();
        assert_eq!(serial.interrupt_under_service, [false; 4]);
        assert!(serial.interrupt_asserted());
        assert_eq!(
            serial.highest_pending_interrupt(),
            Some(InterruptSource::BTransmit)
        );
    }

    #[test]
    fn channel_reset_clears_receive_and_transmit_ius_for_only_that_channel() {
        let mut serial = Z85230::new(CLOCK_HZ);
        configure_transmit_interrupt(&mut serial, CHANNEL_A_CONTROL);
        configure_transmit_interrupt(&mut serial, CHANNEL_B_CONTROL);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, (1 << 5) | (1 << 3));
        serial.write(DeviceAddr::new(CHANNEL_B_DATA), b"B").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        let _ = read_register(&mut serial, CHANNEL_B_CONTROL, 2);
        serial.write(DeviceAddr::new(CHANNEL_A_DATA), b"A").unwrap();
        serial.advance_time(
            VirtualDuration::from_attoseconds(CHARACTER_ATTOSECONDS),
            |_, _| {},
        );
        let _ = read_register(&mut serial, CHANNEL_B_CONTROL, 2);
        assert_eq!(serial.interrupt_under_service, [false, true, false, true]);

        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 0x80);
        assert_eq!(serial.interrupt_under_service, [false, false, false, true]);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 1 << 1);

        write_register(&mut serial, CHANNEL_B_CONTROL, 9, 0x40);
        assert_eq!(serial.interrupt_under_service, [false; 4]);
        assert_eq!(read_register(&mut serial, CHANNEL_A_CONTROL, 3), 0);
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
    fn disabled_receiver_discards_all_host_input() {
        let mut serial = Z85230::new(CLOCK_HZ);

        assert_eq!(serial.receive(Channel::A, &[1, 2, 3]), 3);
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(TRANSMIT_BUFFER_EMPTY)
        );
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(0));
    }

    #[test]
    fn enabled_receiver_accepts_only_the_available_fifo_capacity() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 1);

        assert_eq!(serial.receive(Channel::A, &[0, 1, 2, 3, 4, 5, 6, 7, 8]), 8);
        assert_eq!(
            read_port(&mut serial, CHANNEL_A_CONTROL),
            Ok(RECEIVE_CHARACTER_AVAILABLE | TRANSMIT_BUFFER_EMPTY)
        );
        for expected in 0..8 {
            assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(expected));
        }
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(0));
    }

    #[test]
    fn all_character_interrupt_obeys_the_programmed_fifo_threshold() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 1, 0x10);
        write_register(&mut serial, CHANNEL_A_CONTROL, 15, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 7, 1 << 3);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);

        assert_eq!(serial.receive(Channel::A, &[1, 2, 3]), 3);
        assert!(!serial.interrupt_asserted());
        assert_eq!(serial.receive(Channel::A, &[4]), 1);
        assert!(serial.interrupt_asserted());

        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(1));
        assert!(!serial.interrupt_asserted());
    }

    #[test]
    fn first_character_interrupt_requires_rearming_after_data_is_read() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 1, 1 << 3);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);

        assert_eq!(serial.receive(Channel::A, b"A"), 1);
        assert!(serial.interrupt_asserted());
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(b'A'));
        assert!(!serial.interrupt_asserted());

        assert_eq!(serial.receive(Channel::A, b"B"), 1);
        assert!(!serial.interrupt_asserted());
        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[4 << 3])
            .unwrap();
        assert!(serial.interrupt_asserted());
    }

    #[test]
    fn special_condition_mode_locks_data_until_error_reset() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 1, 3 << 3);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);
        assert_eq!(serial.receive(Channel::A, b"E"), 1);
        serial.channels[0].receive_fifo[0].status |= 1 << 4;

        assert!(serial.interrupt_asserted());
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(b'E'));
        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(b'E'));
        assert_eq!(serial.channels[0].receive_fifo_length, 1);

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[6 << 3])
            .unwrap();
        assert_eq!(serial.channels[0].receive_fifo_length, 0);
        assert!(!serial.interrupt_asserted());
    }

    #[test]
    fn all_character_mode_does_not_lock_special_condition_data() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 1, 2 << 3);
        assert_eq!(serial.receive(Channel::A, b"E"), 1);
        serial.channels[0].receive_fifo[0].status |= 1 << 4;

        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(b'E'));
        assert_eq!(serial.channels[0].receive_fifo_length, 0);
        assert!(!serial.channels[0].receive_fifo_locked);
    }

    #[test]
    fn software_acknowledge_requires_resetting_the_highest_ius() {
        let mut serial = Z85230::new(CLOCK_HZ);
        write_register(&mut serial, CHANNEL_A_CONTROL, 3, 1);
        write_register(&mut serial, CHANNEL_A_CONTROL, 1, 0x10);
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, (1 << 5) | (1 << 3));
        assert_eq!(serial.receive(Channel::A, &[0xa5]), 1);
        assert!(serial.interrupt_asserted());

        serial
            .write(DeviceAddr::new(CHANNEL_B_CONTROL), &[2])
            .unwrap();
        let _ = read_port(&mut serial, CHANNEL_B_CONTROL).unwrap();
        assert!(!serial.interrupt_asserted());

        serial
            .write(DeviceAddr::new(CHANNEL_A_CONTROL), &[7 << 3])
            .unwrap();
        assert!(serial.interrupt_asserted());
    }

    #[test]
    fn receive_interrupt_output_combines_both_channels() {
        let mut serial = Z85230::new(CLOCK_HZ);
        for control_port in [CHANNEL_A_CONTROL, CHANNEL_B_CONTROL] {
            write_register(&mut serial, control_port, 3, 1);
            write_register(&mut serial, control_port, 1, 0x10);
        }
        write_register(&mut serial, CHANNEL_A_CONTROL, 9, 1 << 3);

        assert_eq!(serial.receive(Channel::B, &[0xb0]), 1);
        assert!(serial.interrupt_asserted());
        assert_eq!(serial.receive(Channel::A, &[0xa0]), 1);
        assert!(serial.interrupt_asserted());

        assert_eq!(read_port(&mut serial, CHANNEL_A_DATA), Ok(0xa0));
        assert!(serial.interrupt_asserted());
        assert_eq!(read_port(&mut serial, CHANNEL_B_DATA), Ok(0xb0));
        assert!(!serial.interrupt_asserted());
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
