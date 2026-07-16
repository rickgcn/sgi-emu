//! Dallas DS2502 1Kb add-only memory.
//!
//! The model implements the read-only command path used by system firmware:
//! reset and presence signaling, Read ROM, and Read Memory. Programming and
//! multidrop ROM selection commands are not accepted.

use core::fmt;
use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusDeviceRole;
use se_core::scheduler::{SimDuration, SimTime};

use crate::bus::one_wire::{OneWireDrive, OneWireLineDelivery};

const FAMILY_CODE: u8 = 0x09;
const READ_ROM_COMMAND: u8 = 0x33;
const READ_MEMORY_COMMAND: u8 = 0xf0;
const RESET_LOW_MICROSECONDS: u64 = 480;
const WRITE_ZERO_LOW_MICROSECONDS: u64 = 60;
const PRESENCE_DELAY_MICROSECONDS: u64 = 15;
const PRESENCE_LOW_MICROSECONDS: u64 = 60;
const READ_DATA_VALID_MICROSECONDS: u64 = 15;
const EPROM_SIZE_BYTES: usize = 128;

/// Deterministic DS2502 identity and EPROM contents.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ds2502Config {
    /// Six ROM serial-number bytes transmitted after the family code.
    pub rom_serial_number: [u8; 6],

    /// Complete 128-byte add-only EPROM image.
    #[serde(with = "crate::common::serde_array")]
    pub eprom: [u8; EPROM_SIZE_BYTES],
}

impl Default for Ds2502Config {
    fn default() -> Self {
        let mut eprom = [0xff; EPROM_SIZE_BYTES];
        eprom[..6].copy_from_slice(&[0x01, 0x00, 0x00, 0x69, 0x00, 0x08]);
        Self {
            rom_serial_number: [0x01, 0x00, 0x00, 0x69, 0x00, 0x08],
            eprom,
        }
    }
}

/// DS2502 construction error.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ds2502Error {
    /// The supplied machine timebase is zero.
    InvalidTimebase,
}

impl fmt::Display for Ds2502Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimebase => formatter.write_str("DS2502 timebase must be nonzero"),
        }
    }
}

impl std::error::Error for Ds2502Error {}

/// Scheduled DS2502 line transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ds2502Event {
    /// Begins the presence pulse after a valid reset.
    PresenceAssert { epoch: u64 },

    /// Ends the presence pulse.
    PresenceRelease { epoch: u64 },

    /// Releases a zero bit during a read-data slot.
    ReadSlotRelease { epoch: u64 },
}

/// Observable DS2502 action.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ds2502Action {
    /// Schedules a protocol timing transition.
    Schedule {
        delay: SimDuration,
        event: Ds2502Event,
    },

    /// Changes the device's open-drain output.
    Drive(OneWireDrive),

    /// No action is pending.
    Idle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum ProtocolPhase {
    AwaitRomCommand,
    TransmitRom {
        bit_index: u8,
    },
    AwaitMemoryCommand,
    ReceiveMemoryAddress {
        address: [u8; 2],
        received: u8,
    },
    TransmitCommandCrc {
        value: u8,
        address: u16,
        bit_index: u8,
    },
    TransmitMemory {
        address: u16,
        bit_index: u8,
        crc: u8,
    },
    TransmitDataCrc {
        value: u8,
        bit_index: u8,
    },
    TransmitOnes,
    Inactive,
}

impl ProtocolPhase {
    const fn receives_master_bits(self) -> bool {
        matches!(
            self,
            Self::AwaitRomCommand | Self::AwaitMemoryCommand | Self::ReceiveMemoryAddress { .. }
        )
    }

    const fn transmits_device_bits(self) -> bool {
        matches!(
            self,
            Self::TransmitRom { .. }
                | Self::TransmitCommandCrc { .. }
                | Self::TransmitMemory { .. }
                | Self::TransmitDataCrc { .. }
                | Self::TransmitOnes
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct MasterLowSlot {
    start: SimTime,
    receive_on_release: bool,
}

/// Read-only DS2502 device attached to one 1-Wire master.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ds2502 {
    id: ComponentId,
    name: String,
    master: ComponentId,
    timebase_hz: u64,
    config: Ds2502Config,
    now: SimTime,
    epoch: u64,
    phase: ProtocolPhase,
    receive_byte: u8,
    receive_bits: u8,
    master_low: Option<MasterLowSlot>,
    driving_low: bool,
    actions: VecDeque<Ds2502Action>,
}

se_core::component_state!(Ds2502State, Ds2502);

impl Ds2502 {
    /// Creates a DS2502 with immutable ROM and EPROM data.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        master: ComponentId,
        timebase_hz: u64,
        config: Ds2502Config,
    ) -> Result<Self, Ds2502Error> {
        if timebase_hz == 0 {
            return Err(Ds2502Error::InvalidTimebase);
        }
        Ok(Self {
            id,
            name: name.into(),
            master,
            timebase_hz,
            config,
            now: SimTime::ZERO,
            epoch: 0,
            phase: ProtocolPhase::Inactive,
            receive_byte: 0,
            receive_bits: 0,
            master_low: None,
            driving_low: false,
            actions: VecDeque::new(),
        })
    }

    /// Resets volatile protocol state without changing identity or EPROM data.
    pub fn power_on(&mut self, now: SimTime) {
        self.now = now;
        self.epoch = self.epoch.wrapping_add(1);
        self.phase = ProtocolPhase::Inactive;
        self.receive_byte = 0;
        self.receive_bits = 0;
        self.master_low = None;
        self.driving_low = false;
        self.actions.clear();
    }

    /// Cancels an in-flight command while preserving immutable contents.
    pub fn hard_reset(&mut self, now: SimTime) {
        self.power_on(now);
    }

    /// Handles one delayed presence or read-slot transition.
    pub fn handle_event(&mut self, now: SimTime, event: Ds2502Event) {
        self.now = now;
        let event_epoch = match event {
            Ds2502Event::PresenceAssert { epoch }
            | Ds2502Event::PresenceRelease { epoch }
            | Ds2502Event::ReadSlotRelease { epoch } => epoch,
        };
        if event_epoch != self.epoch {
            return;
        }
        match event {
            Ds2502Event::PresenceAssert { .. } => self.set_drive_low(true),
            Ds2502Event::PresenceRelease { .. } | Ds2502Event::ReadSlotRelease { .. } => {
                self.set_drive_low(false)
            }
        }
    }

    /// Polls one pending action.
    pub fn poll(&mut self) -> Ds2502Action {
        self.actions.pop_front().unwrap_or(Ds2502Action::Idle)
    }

    /// Returns the immutable EPROM image.
    pub const fn eprom(&self) -> &[u8; EPROM_SIZE_BYTES] {
        &self.config.eprom
    }

    /// Returns the complete family, serial-number, and CRC ROM stream.
    pub fn rom(&self) -> [u8; 8] {
        let mut rom = [0; 8];
        rom[0] = FAMILY_CODE;
        rom[1..7].copy_from_slice(&self.config.rom_serial_number);
        rom[7] = crc8(&rom[..7]);
        rom
    }

    fn microseconds(&self, microseconds: u64) -> SimDuration {
        let numerator = u128::from(self.timebase_hz) * u128::from(microseconds);
        let ticks = numerator.div_ceil(1_000_000);
        SimDuration::new(ticks.min(u128::from(u64::MAX)) as u64)
    }

    fn begin_reset_response(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.phase = ProtocolPhase::AwaitRomCommand;
        self.receive_byte = 0;
        self.receive_bits = 0;
        self.master_low = None;
        self.actions.clear();
        self.set_drive_low(false);
        self.actions.push_back(Ds2502Action::Schedule {
            delay: self.microseconds(PRESENCE_DELAY_MICROSECONDS),
            event: Ds2502Event::PresenceAssert { epoch: self.epoch },
        });
        self.actions.push_back(Ds2502Action::Schedule {
            delay: self.microseconds(PRESENCE_DELAY_MICROSECONDS + PRESENCE_LOW_MICROSECONDS),
            event: Ds2502Event::PresenceRelease { epoch: self.epoch },
        });
    }

    fn begin_master_low(&mut self, time: SimTime) {
        if self.master_low.is_some() {
            return;
        }
        let receive_on_release = self.phase.receives_master_bits();
        self.master_low = Some(MasterLowSlot {
            start: time,
            receive_on_release,
        });
        if self.phase.transmits_device_bits()
            && let Some(bit) = self.next_transmit_bit()
            && !bit
        {
            self.set_drive_low(true);
            self.actions.push_back(Ds2502Action::Schedule {
                delay: self.microseconds(READ_DATA_VALID_MICROSECONDS),
                event: Ds2502Event::ReadSlotRelease { epoch: self.epoch },
            });
        }
    }

    fn end_master_low(&mut self, time: SimTime) {
        let Some(slot) = self.master_low.take() else {
            return;
        };
        let low_ticks = time.get().saturating_sub(slot.start.get());
        if low_ticks >= self.microseconds(RESET_LOW_MICROSECONDS).get() {
            self.begin_reset_response();
            return;
        }
        if slot.receive_on_release {
            let bit = low_ticks < self.microseconds(WRITE_ZERO_LOW_MICROSECONDS).get();
            self.receive_bit(bit);
        }
    }

    fn receive_bit(&mut self, bit: bool) {
        self.receive_byte |= u8::from(bit) << self.receive_bits;
        self.receive_bits += 1;
        if self.receive_bits == 8 {
            let byte = self.receive_byte;
            self.receive_byte = 0;
            self.receive_bits = 0;
            self.receive_protocol_byte(byte);
        }
    }

    fn receive_protocol_byte(&mut self, byte: u8) {
        match self.phase {
            ProtocolPhase::AwaitRomCommand if byte == READ_ROM_COMMAND => {
                self.phase = ProtocolPhase::TransmitRom { bit_index: 0 };
            }
            ProtocolPhase::AwaitRomCommand => self.phase = ProtocolPhase::Inactive,
            ProtocolPhase::AwaitMemoryCommand if byte == READ_MEMORY_COMMAND => {
                self.phase = ProtocolPhase::ReceiveMemoryAddress {
                    address: [0; 2],
                    received: 0,
                };
            }
            ProtocolPhase::AwaitMemoryCommand => self.phase = ProtocolPhase::Inactive,
            ProtocolPhase::ReceiveMemoryAddress {
                mut address,
                received: 0,
            } => {
                address[0] = byte;
                self.phase = ProtocolPhase::ReceiveMemoryAddress {
                    address,
                    received: 1,
                };
            }
            ProtocolPhase::ReceiveMemoryAddress {
                mut address,
                received: 1,
            } => {
                address[1] = byte;
                let start = u16::from_le_bytes(address);
                self.phase = ProtocolPhase::TransmitCommandCrc {
                    value: crc8(&[READ_MEMORY_COMMAND, address[0], address[1]]),
                    address: start,
                    bit_index: 0,
                };
            }
            _ => self.phase = ProtocolPhase::Inactive,
        }
    }

    fn next_transmit_bit(&mut self) -> Option<bool> {
        match self.phase {
            ProtocolPhase::TransmitRom { bit_index } => {
                let rom = self.rom();
                let bit = rom[usize::from(bit_index / 8)] & (1 << (bit_index % 8)) != 0;
                self.phase = if bit_index == 63 {
                    ProtocolPhase::AwaitMemoryCommand
                } else {
                    ProtocolPhase::TransmitRom {
                        bit_index: bit_index + 1,
                    }
                };
                Some(bit)
            }
            ProtocolPhase::TransmitCommandCrc {
                value,
                address,
                bit_index,
            } => {
                let bit = value & (1 << bit_index) != 0;
                self.phase = if bit_index == 7 {
                    if usize::from(address) < self.config.eprom.len() {
                        ProtocolPhase::TransmitMemory {
                            address,
                            bit_index: 0,
                            crc: 0,
                        }
                    } else {
                        ProtocolPhase::TransmitOnes
                    }
                } else {
                    ProtocolPhase::TransmitCommandCrc {
                        value,
                        address,
                        bit_index: bit_index + 1,
                    }
                };
                Some(bit)
            }
            ProtocolPhase::TransmitMemory {
                address,
                bit_index,
                crc,
            } => {
                let byte = self.config.eprom[usize::from(address)];
                let bit = byte & (1 << bit_index) != 0;
                self.phase = if bit_index == 7 {
                    let crc = crc8_update(crc, byte);
                    if usize::from(address) + 1 == self.config.eprom.len() {
                        ProtocolPhase::TransmitDataCrc {
                            value: crc,
                            bit_index: 0,
                        }
                    } else {
                        ProtocolPhase::TransmitMemory {
                            address: address + 1,
                            bit_index: 0,
                            crc,
                        }
                    }
                } else {
                    ProtocolPhase::TransmitMemory {
                        address,
                        bit_index: bit_index + 1,
                        crc,
                    }
                };
                Some(bit)
            }
            ProtocolPhase::TransmitDataCrc { value, bit_index } => {
                let bit = value & (1 << bit_index) != 0;
                self.phase = if bit_index == 7 {
                    ProtocolPhase::TransmitOnes
                } else {
                    ProtocolPhase::TransmitDataCrc {
                        value,
                        bit_index: bit_index + 1,
                    }
                };
                Some(bit)
            }
            ProtocolPhase::TransmitOnes => Some(true),
            _ => None,
        }
    }

    fn set_drive_low(&mut self, drive_low: bool) {
        if self.driving_low == drive_low {
            return;
        }
        self.driving_low = drive_low;
        self.actions.push_back(Ds2502Action::Drive(OneWireDrive {
            source: self.id,
            time: self.now,
            drive_low,
        }));
    }
}

impl Component for Ds2502 {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.power_on(SimTime::ZERO);
    }
}

impl BusDeviceRole<OneWireLineDelivery> for Ds2502 {
    type Response = ();

    fn accept(&mut self, delivery: OneWireLineDelivery) -> Self::Response {
        if delivery.source != self.master {
            return;
        }
        self.now = delivery.time;
        if delivery.source_drive_low {
            self.begin_master_low(delivery.time);
        } else {
            self.end_master_low(delivery.time);
        }
    }
}

fn crc8(bytes: &[u8]) -> u8 {
    bytes.iter().copied().fold(0, crc8_update)
}

fn crc8_update(mut crc: u8, mut byte: u8) -> u8 {
    for _ in 0..8 {
        let mix = (crc ^ byte) & 1;
        crc >>= 1;
        if mix != 0 {
            crc ^= 0x8c;
        }
        byte >>= 1;
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    fn component(value: u64) -> ComponentId {
        ComponentId::new(value)
    }

    fn device() -> Ds2502 {
        Ds2502::new(
            component(2),
            "DS2502",
            component(1),
            1_000_000,
            Ds2502Config::default(),
        )
        .unwrap()
    }

    fn next_byte(device: &mut Ds2502) -> u8 {
        (0..8).fold(0, |byte, bit| {
            byte | u8::from(device.next_transmit_bit().unwrap()) << bit
        })
    }

    #[test]
    fn default_identity_contains_an_sgi_mac_address_in_prom_order() {
        let device = device();
        let mut mac = device.eprom()[..6].to_vec();
        mac.reverse();
        assert_eq!(mac, [0x08, 0x00, 0x69, 0x00, 0x00, 0x01]);
        assert_eq!(device.rom()[0], FAMILY_CODE);
        assert_eq!(crc8(&device.rom()), 0);
    }

    #[test]
    fn reset_schedules_a_valid_presence_pulse() {
        let mut device = device();
        device.accept(OneWireLineDelivery {
            source: component(1),
            time: SimTime::ZERO,
            source_drive_low: true,
            line_low: true,
        });
        device.accept(OneWireLineDelivery {
            source: component(1),
            time: SimTime::new(500),
            source_drive_low: false,
            line_low: false,
        });
        let epoch = device.epoch;
        assert_eq!(
            device.poll(),
            Ds2502Action::Schedule {
                delay: SimDuration::new(15),
                event: Ds2502Event::PresenceAssert { epoch },
            }
        );
        assert_eq!(
            device.poll(),
            Ds2502Action::Schedule {
                delay: SimDuration::new(75),
                event: Ds2502Event::PresenceRelease { epoch },
            }
        );
    }

    #[test]
    fn read_rom_stream_has_family_serial_and_valid_crc() {
        let mut device = device();
        device.phase = ProtocolPhase::AwaitRomCommand;
        device.receive_protocol_byte(READ_ROM_COMMAND);
        let stream: Vec<_> = (0..8).map(|_| next_byte(&mut device)).collect();
        assert_eq!(stream, device.rom());
        assert_eq!(device.phase, ProtocolPhase::AwaitMemoryCommand);
    }

    #[test]
    fn read_memory_stream_has_command_and_data_crcs() {
        let mut device = device();
        device.phase = ProtocolPhase::AwaitMemoryCommand;
        device.receive_protocol_byte(READ_MEMORY_COMMAND);
        device.receive_protocol_byte(0);
        device.receive_protocol_byte(0);
        assert_eq!(next_byte(&mut device), crc8(&[READ_MEMORY_COMMAND, 0, 0]));
        let data: Vec<_> = (0..EPROM_SIZE_BYTES)
            .map(|_| next_byte(&mut device))
            .collect();
        assert_eq!(data, device.eprom());
        assert_eq!(next_byte(&mut device), crc8(device.eprom()));
        assert_eq!(next_byte(&mut device), 0xff);
    }

    #[test]
    fn unsupported_commands_wait_for_the_next_reset() {
        let mut device = device();
        device.phase = ProtocolPhase::AwaitRomCommand;
        device.receive_protocol_byte(0xcc);
        assert_eq!(device.phase, ProtocolPhase::Inactive);
        assert_eq!(device.next_transmit_bit(), None);
    }

    #[test]
    fn stale_line_events_do_not_change_the_output() {
        let mut device = device();
        device.epoch = 7;
        device.handle_event(SimTime::new(10), Ds2502Event::PresenceAssert { epoch: 6 });
        assert_eq!(device.poll(), Ds2502Action::Idle);
        assert!(!device.driving_low);
    }
}
