//! Deterministic IBM PS/2 keyboard and mouse devices.
//!
//! The devices exchange complete 11-bit serial frames over independent
//! open-drain clock and data lines. Host input is expressed as physical key
//! positions and relative mouse motion; character translation belongs to the
//! guest operating system.

mod keyboard;
mod mouse;
#[cfg(test)]
mod tests;

use core::fmt;
use std::collections::{BTreeMap, BTreeSet, VecDeque};

use se_core::component::{Component, ComponentId};
use se_core::scheduler::{RationalClockProjection, SimDuration, SimTime};

use crate::bus::two_wire::{TwoWireDrive, TwoWireLineDelivery};

const DEVICE_CLOCK_HZ: u64 = 12_000;
const BAT_MILLISECONDS: u64 = 500;
const KEYBOARD_FIFO_CAPACITY: usize = 16;

/// One physical position on an IBM enhanced 101/102-key keyboard.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
pub enum Ps2KeyPosition {
    Escape,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    PrintScreen,
    ScrollLock,
    Pause,
    Grave,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Digit0,
    Minus,
    Equal,
    Backspace,
    Insert,
    Home,
    PageUp,
    NumLock,
    NumpadDivide,
    NumpadMultiply,
    NumpadSubtract,
    Tab,
    Q,
    W,
    E,
    R,
    T,
    Y,
    U,
    I,
    O,
    P,
    LeftBracket,
    RightBracket,
    Backslash,
    IsoHash,
    Delete,
    End,
    PageDown,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadAdd,
    CapsLock,
    A,
    S,
    D,
    F,
    G,
    H,
    J,
    K,
    L,
    Semicolon,
    Apostrophe,
    Enter,
    Numpad4,
    Numpad5,
    Numpad6,
    LeftShift,
    Iso102,
    Z,
    X,
    C,
    V,
    B,
    N,
    M,
    Comma,
    Period,
    Slash,
    RightShift,
    ArrowUp,
    Numpad1,
    Numpad2,
    Numpad3,
    NumpadEnter,
    LeftControl,
    LeftAlt,
    Space,
    RightAlt,
    RightControl,
    ArrowLeft,
    ArrowDown,
    ArrowRight,
    Numpad0,
    NumpadDecimal,
}

/// One host keyboard transition expressed as a physical key position.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2KeyboardInput {
    pub key: Ps2KeyPosition,
    pub pressed: bool,
}

/// Authoritative state of the three standard PS/2 mouse buttons.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2MouseButtons {
    pub left: bool,
    pub middle: bool,
    pub right: bool,
}

/// Relative mouse movement and resulting button state.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2MouseInput {
    pub delta_x: i32,
    pub delta_y: i32,
    pub buttons: Ps2MouseButtons,
}

/// Fixed wiring shared by one PS/2 device.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2Wiring {
    pub controller: ComponentId,
    pub bus: ComponentId,
}

/// Scheduled keyboard transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ps2KeyboardEvent {
    Link(Ps2LinkEvent),
    BatComplete { epoch: u64 },
    Typematic { epoch: u64 },
}

/// Scheduled mouse transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ps2MouseEvent {
    Link(Ps2LinkEvent),
    BatComplete { epoch: u64 },
    Sample { epoch: u64 },
}

/// One device-side PS/2 serial clock transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ps2LinkEvent {
    Clock { epoch: u64 },
}

/// Action emitted by the keyboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ps2KeyboardAction {
    Schedule {
        delay: SimDuration,
        event: Ps2KeyboardEvent,
    },
    Drive(TwoWireDrive),
}

/// Result of polling the keyboard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ps2KeyboardPoll {
    Action(Ps2KeyboardAction),
    Idle,
}

/// Action emitted by the mouse.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ps2MouseAction {
    Schedule {
        delay: SimDuration,
        event: Ps2MouseEvent,
    },
    Drive(TwoWireDrive),
}

/// Result of polling the mouse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ps2MousePoll {
    Action(Ps2MouseAction),
    Idle,
}

/// PS/2 device construction error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ps2DeviceBuildError {
    InvalidTimebase { timebase_hz: u64 },
}

impl fmt::Display for Ps2DeviceBuildError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimebase { timebase_hz } => {
                write!(formatter, "invalid PS/2 device timebase {timebase_hz} Hz")
            }
        }
    }
}

impl std::error::Error for Ps2DeviceBuildError {}

/// PS/2 keyboard protocol error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ps2KeyboardError {
    InvalidBus(ComponentId),
}

impl fmt::Display for Ps2KeyboardError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBus(bus) => write!(formatter, "keyboard observed unexpected bus {bus}"),
        }
    }
}

impl std::error::Error for Ps2KeyboardError {}

/// PS/2 mouse protocol error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ps2MouseError {
    InvalidBus(ComponentId),
}

impl fmt::Display for Ps2MouseError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidBus(bus) => write!(formatter, "mouse observed unexpected bus {bus}"),
        }
    }
}

impl std::error::Error for Ps2MouseError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum LinkTransfer {
    Idle,
    DeviceTransmit {
        frame: u16,
        bit: u8,
        clock_low: bool,
    },
    HostReceive {
        bit: u8,
        byte: u8,
        parity_ones: u8,
        valid: bool,
        clock_low: bool,
    },
    HostAcknowledge {
        byte: u8,
        valid: bool,
        clock_low: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LinkSignal {
    HostByte { byte: u8, valid: bool },
    DeviceByteComplete { byte: u8 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum LinkAction {
    Schedule {
        delay: SimDuration,
        event: Ps2LinkEvent,
    },
    Drive(TwoWireDrive),
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct Ps2DeviceLink {
    id: ComponentId,
    wiring: Ps2Wiring,
    timebase_hz: u64,
    half_clock_remainder: u64,
    now: SimTime,
    epoch: u64,
    output_clock_low: bool,
    output_data_low: bool,
    observed_clock_low: bool,
    observed_data_low: bool,
    transfer: LinkTransfer,
    actions: VecDeque<LinkAction>,
}

impl Ps2DeviceLink {
    fn new(
        id: ComponentId,
        wiring: Ps2Wiring,
        timebase_hz: u64,
    ) -> Result<Self, Ps2DeviceBuildError> {
        if timebase_hz < DEVICE_CLOCK_HZ * 2 {
            return Err(Ps2DeviceBuildError::InvalidTimebase { timebase_hz });
        }
        Ok(Self {
            id,
            wiring,
            timebase_hz,
            half_clock_remainder: 0,
            now: SimTime::ZERO,
            epoch: 0,
            output_clock_low: false,
            output_data_low: false,
            observed_clock_low: false,
            observed_data_low: false,
            transfer: LinkTransfer::Idle,
            actions: VecDeque::new(),
        })
    }

    fn reset(&mut self) {
        let release_needed = self.output_clock_low || self.output_data_low;
        self.epoch = self.epoch.wrapping_add(1);
        self.half_clock_remainder = 0;
        self.output_clock_low = false;
        self.output_data_low = false;
        self.observed_clock_low = false;
        self.observed_data_low = false;
        self.transfer = LinkTransfer::Idle;
        self.actions.clear();
        if release_needed {
            self.actions.push_back(LinkAction::Drive(TwoWireDrive {
                source: self.id,
                time: self.now,
                clock_low: false,
                data_low: false,
            }));
        }
    }

    fn observe_time(&mut self, now: SimTime) {
        self.now = now;
    }

    fn can_start(&self) -> bool {
        self.transfer == LinkTransfer::Idle && !self.observed_clock_low
    }

    fn start_device_byte(&mut self, byte: u8) -> bool {
        if !self.can_start() {
            return false;
        }
        self.epoch = self.epoch.wrapping_add(1);
        let frame = serial_frame(byte);
        self.transfer = LinkTransfer::DeviceTransmit {
            frame,
            bit: 0,
            clock_low: false,
        };
        self.push_drive(false, true);
        self.schedule_half_clock();
        true
    }

    fn observe_lines(
        &mut self,
        delivery: TwoWireLineDelivery,
    ) -> Result<Option<LinkSignal>, ComponentId> {
        if delivery.bus != self.wiring.bus {
            return Err(delivery.bus);
        }
        self.now = delivery.time;
        let previous_clock_low = self.observed_clock_low;
        self.observed_clock_low = delivery.clock_low;
        self.observed_data_low = delivery.data_low;

        if let LinkTransfer::HostReceive {
            bit,
            byte,
            parity_ones,
            valid,
            clock_low: true,
        } = self.transfer
            && !previous_clock_low
            && delivery.clock_low
        {
            let data_high = !delivery.data_low;
            let mut next_byte = byte;
            let mut next_ones = parity_ones;
            let mut next_valid = valid;
            match bit {
                0 => next_valid &= !data_high,
                1..=8 => {
                    if data_high {
                        next_byte |= 1 << (bit - 1);
                        next_ones += 1;
                    }
                }
                9 => {
                    if data_high {
                        next_ones += 1;
                    }
                    next_valid &= next_ones & 1 == 1;
                }
                10 => next_valid &= data_high,
                _ => {}
            }
            self.transfer = LinkTransfer::HostReceive {
                bit,
                byte: next_byte,
                parity_ones: next_ones,
                valid: next_valid,
                clock_low: true,
            };
        }

        if self.transfer == LinkTransfer::Idle
            && previous_clock_low
            && !delivery.clock_low
            && delivery.data_low
            && delivery.source == self.wiring.controller
        {
            self.epoch = self.epoch.wrapping_add(1);
            self.transfer = LinkTransfer::HostReceive {
                bit: 0,
                byte: 0,
                parity_ones: 0,
                valid: true,
                clock_low: false,
            };
            self.schedule_half_clock();
        }
        Ok(None)
    }

    fn handle_event(&mut self, event: Ps2LinkEvent) -> Option<LinkSignal> {
        let Ps2LinkEvent::Clock { epoch } = event;
        if epoch != self.epoch {
            return None;
        }
        if self.observed_clock_low && !self.output_clock_low {
            self.schedule_inhibit_poll();
            return None;
        }
        match self.transfer {
            LinkTransfer::DeviceTransmit {
                frame,
                bit,
                clock_low: false,
            } => {
                self.transfer = LinkTransfer::DeviceTransmit {
                    frame,
                    bit,
                    clock_low: true,
                };
                self.push_drive(true, frame & (1 << bit) == 0);
                self.schedule_half_clock();
                None
            }
            LinkTransfer::DeviceTransmit {
                frame,
                bit,
                clock_low: true,
            } => {
                if bit == 10 {
                    let byte = ((frame >> 1) & 0xff) as u8;
                    self.transfer = LinkTransfer::Idle;
                    self.push_drive(false, false);
                    Some(LinkSignal::DeviceByteComplete { byte })
                } else {
                    let next = bit + 1;
                    self.transfer = LinkTransfer::DeviceTransmit {
                        frame,
                        bit: next,
                        clock_low: false,
                    };
                    self.push_drive(false, frame & (1 << next) == 0);
                    self.schedule_half_clock();
                    None
                }
            }
            LinkTransfer::HostReceive {
                bit,
                byte,
                parity_ones,
                valid,
                clock_low: false,
            } => {
                self.transfer = LinkTransfer::HostReceive {
                    bit,
                    byte,
                    parity_ones,
                    valid,
                    clock_low: true,
                };
                self.push_drive(true, false);
                self.schedule_half_clock();
                None
            }
            LinkTransfer::HostReceive {
                bit,
                byte,
                parity_ones,
                valid,
                clock_low: true,
            } => {
                self.push_drive(false, false);
                if bit == 10 {
                    self.transfer = LinkTransfer::HostAcknowledge {
                        byte,
                        valid,
                        clock_low: false,
                    };
                } else {
                    self.transfer = LinkTransfer::HostReceive {
                        bit: bit + 1,
                        byte,
                        parity_ones,
                        valid,
                        clock_low: false,
                    };
                }
                self.schedule_half_clock();
                None
            }
            LinkTransfer::HostAcknowledge {
                byte,
                valid,
                clock_low: false,
            } => {
                self.transfer = LinkTransfer::HostAcknowledge {
                    byte,
                    valid,
                    clock_low: true,
                };
                self.push_drive(true, true);
                self.schedule_half_clock();
                None
            }
            LinkTransfer::HostAcknowledge {
                byte,
                valid,
                clock_low: true,
            } => {
                self.transfer = LinkTransfer::Idle;
                self.push_drive(false, false);
                Some(LinkSignal::HostByte { byte, valid })
            }
            LinkTransfer::Idle => None,
        }
    }

    fn poll(&mut self) -> Option<LinkAction> {
        self.actions.pop_front()
    }

    fn push_drive(&mut self, clock_low: bool, data_low: bool) {
        if self.output_clock_low == clock_low && self.output_data_low == data_low {
            return;
        }
        self.output_clock_low = clock_low;
        self.output_data_low = data_low;
        self.actions.push_back(LinkAction::Drive(TwoWireDrive {
            source: self.id,
            time: self.now,
            clock_low,
            data_low,
        }));
    }

    fn schedule_half_clock(&mut self) {
        let mut projection = RationalClockProjection::new(
            self.timebase_hz,
            DEVICE_CLOCK_HZ * 2,
            1,
            self.half_clock_remainder,
        );
        let delay = projection
            .advance(1)
            .expect("the fixed PS/2 clock projection cannot overflow");
        self.half_clock_remainder = projection.remainder();
        self.actions.push_back(LinkAction::Schedule {
            delay,
            event: Ps2LinkEvent::Clock { epoch: self.epoch },
        });
    }

    fn schedule_inhibit_poll(&mut self) {
        self.actions.push_back(LinkAction::Schedule {
            delay: SimDuration::new(self.timebase_hz / 10_000),
            event: Ps2LinkEvent::Clock { epoch: self.epoch },
        });
    }
}

fn serial_frame(byte: u8) -> u16 {
    let parity = u16::from(byte.count_ones() & 1 == 0);
    (u16::from(byte) << 1) | (parity << 9) | (1 << 10)
}

fn milliseconds(timebase_hz: u64, value: u64) -> SimDuration {
    SimDuration::new(timebase_hz.saturating_mul(value) / 1_000)
}

/// IBM enhanced PS/2 keyboard.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2Keyboard {
    id: ComponentId,
    name: String,
    link: Ps2DeviceLink,
    responses: VecDeque<u8>,
    scan_fifo: VecDeque<u8>,
    scan_overrun: bool,
    pressed: BTreeSet<Ps2KeyPosition>,
    set3_types: BTreeMap<Ps2KeyPosition, Ps2KeyType>,
    command_parameter: KeyboardParameter,
    scan_set: u8,
    leds: u8,
    typematic_parameter: u8,
    typematic_key: Option<Ps2KeyPosition>,
    typematic_epoch: u64,
    scanning_enabled: bool,
    resume_scanning_after_id: bool,
    bat_epoch: u64,
    bat_active: bool,
    last_sent: Option<u8>,
    actions: VecDeque<Ps2KeyboardAction>,
}

se_core::component_state!(Ps2KeyboardState, Ps2Keyboard);

/// Standard three-button PS/2 mouse.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2Mouse {
    id: ComponentId,
    name: String,
    link: Ps2DeviceLink,
    responses: VecDeque<u8>,
    mode: MouseMode,
    wrap_mode: bool,
    reporting_enabled: bool,
    scaling_2_to_1: bool,
    resolution: u8,
    sample_rate: u16,
    parameter: MouseParameter,
    accumulated_x_eighths: i64,
    accumulated_y_eighths: i64,
    buttons: Ps2MouseButtons,
    last_reported_buttons: Ps2MouseButtons,
    last_packet: [u8; 3],
    last_packet_valid: bool,
    sample_epoch: u64,
    sample_remainder: u64,
    bat_epoch: u64,
    bat_active: bool,
    actions: VecDeque<Ps2MouseAction>,
}

se_core::component_state!(Ps2MouseState, Ps2Mouse);

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum KeyboardParameter {
    None,
    Leds { was_enabled: bool },
    ScanSet { was_enabled: bool },
    Typematic { was_enabled: bool },
    KeyType(Ps2KeyType),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum Ps2KeyType {
    Typematic,
    MakeBreak,
    MakeOnly,
    TypematicMakeBreak,
}

impl Ps2KeyType {
    fn sends_break(self) -> bool {
        matches!(self, Self::MakeBreak | Self::TypematicMakeBreak)
    }

    fn repeats(self) -> bool {
        matches!(self, Self::Typematic | Self::TypematicMakeBreak)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum MouseMode {
    Stream,
    Remote,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum MouseParameter {
    None,
    Resolution,
    SampleRate,
}
