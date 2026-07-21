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

use se_core::component::{
    Component, ComponentId, ComponentStateError, validate_component_state_id,
};
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
    DeviceInhibited {
        frame: u16,
        started: bool,
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

#[derive(Clone, Debug, Eq, PartialEq)]
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
    deferred_device_frame: Option<u16>,
    actions: VecDeque<LinkAction>,
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct Ps2DeviceLinkState {
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
    deferred_device_frame: Option<u16>,
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
            deferred_device_frame: None,
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
        self.deferred_device_frame = None;
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

    fn save_state(&self) -> Ps2DeviceLinkState {
        Ps2DeviceLinkState {
            id: self.id,
            wiring: self.wiring,
            timebase_hz: self.timebase_hz,
            half_clock_remainder: self.half_clock_remainder,
            now: self.now,
            epoch: self.epoch,
            output_clock_low: self.output_clock_low,
            output_data_low: self.output_data_low,
            observed_clock_low: self.observed_clock_low,
            observed_data_low: self.observed_data_low,
            transfer: self.transfer,
            deferred_device_frame: self.deferred_device_frame,
            actions: self.actions.clone(),
        }
    }

    fn validate_state(&self, state: &Ps2DeviceLinkState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        if self.wiring != state.wiring {
            return Err(ComponentStateError::ConfigurationMismatch {
                component: self.id,
                field: "wiring",
            });
        }
        if self.timebase_hz != state.timebase_hz {
            return Err(ComponentStateError::ConfigurationMismatch {
                component: self.id,
                field: "timebase_hz",
            });
        }
        if state.half_clock_remainder >= DEVICE_CLOCK_HZ * 2 {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "PS/2 clock remainder must be normalized",
            });
        }
        let valid_frame = |frame: u16| {
            let byte = ((frame >> 1) & 0xff) as u8;
            frame == serial_frame(byte)
        };
        let valid_transfer = match state.transfer {
            LinkTransfer::Idle | LinkTransfer::HostAcknowledge { .. } => true,
            LinkTransfer::DeviceTransmit { frame, bit, .. } => bit <= 10 && valid_frame(frame),
            LinkTransfer::DeviceInhibited { frame, .. } => valid_frame(frame),
            LinkTransfer::HostReceive {
                bit, parity_ones, ..
            } => bit <= 10 && parity_ones <= 9,
        };
        if !valid_transfer
            || state
                .deferred_device_frame
                .is_some_and(|frame| !valid_frame(frame))
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "PS/2 transfer frame and bit counters must be valid",
            });
        }
        if state.actions.iter().any(|action| match action {
            LinkAction::Schedule {
                event: Ps2LinkEvent::Clock { epoch },
                ..
            } => *epoch != state.epoch,
            LinkAction::Drive(drive) => drive.source != self.id || drive.time > state.now,
        }) {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "PS/2 link actions must originate from the device",
            });
        }
        Ok(())
    }

    fn apply_state(&mut self, state: Ps2DeviceLinkState) {
        self.half_clock_remainder = state.half_clock_remainder;
        self.now = state.now;
        self.epoch = state.epoch;
        self.output_clock_low = state.output_clock_low;
        self.output_data_low = state.output_data_low;
        self.observed_clock_low = state.observed_clock_low;
        self.observed_data_low = state.observed_data_low;
        self.transfer = state.transfer;
        self.deferred_device_frame = state.deferred_device_frame;
        self.actions = state.actions;
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
        self.start_device_frame(serial_frame(byte));
        true
    }

    fn resume_deferred_device_byte(&mut self) -> bool {
        if !self.can_start() {
            return false;
        }
        let Some(frame) = self.deferred_device_frame.take() else {
            return false;
        };
        self.start_device_frame(frame);
        true
    }

    fn has_deferred_device_byte(&self) -> bool {
        self.deferred_device_frame.is_some()
    }

    fn start_device_frame(&mut self, frame: u16) {
        self.epoch = self.epoch.wrapping_add(1);
        self.transfer = LinkTransfer::DeviceTransmit {
            frame,
            bit: 0,
            clock_low: false,
        };
        self.push_drive(false, true);
        self.schedule_half_clock();
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

        if let LinkTransfer::DeviceInhibited { frame, started } = self.transfer
            && previous_clock_low
            && !delivery.clock_low
            && delivery.source == self.wiring.controller
        {
            self.epoch = self.epoch.wrapping_add(1);
            if delivery.data_low {
                if started {
                    debug_assert!(self.deferred_device_frame.is_none());
                    self.deferred_device_frame = Some(frame);
                }
                self.transfer = LinkTransfer::HostReceive {
                    bit: 0,
                    byte: 0,
                    parity_ones: 0,
                    valid: true,
                    clock_low: false,
                };
            } else {
                self.transfer = LinkTransfer::DeviceTransmit {
                    frame,
                    bit: 0,
                    clock_low: false,
                };
                self.push_drive(false, true);
            }
            self.schedule_half_clock();
        } else if self.transfer == LinkTransfer::Idle
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
            if let LinkTransfer::DeviceTransmit {
                frame,
                bit,
                clock_low: false,
            } = self.transfer
            {
                self.epoch = self.epoch.wrapping_add(1);
                self.transfer = LinkTransfer::DeviceInhibited {
                    frame,
                    started: bit != 0,
                };
                self.push_drive(false, false);
                return None;
            }
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
            LinkTransfer::Idle | LinkTransfer::DeviceInhibited { .. } => None,
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
#[derive(Clone, Debug, Eq, PartialEq)]
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

/// Serializable dynamic state of a PS/2 keyboard.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2KeyboardState {
    id: ComponentId,
    link: Ps2DeviceLinkState,
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

impl Ps2Keyboard {
    /// Captures the keyboard's dynamic hardware state.
    pub fn save_state(&self) -> Ps2KeyboardState {
        Ps2KeyboardState {
            id: self.id,
            link: self.link.save_state(),
            responses: self.responses.clone(),
            scan_fifo: self.scan_fifo.clone(),
            scan_overrun: self.scan_overrun,
            pressed: self.pressed.clone(),
            set3_types: self.set3_types.clone(),
            command_parameter: self.command_parameter,
            scan_set: self.scan_set,
            leds: self.leds,
            typematic_parameter: self.typematic_parameter,
            typematic_key: self.typematic_key,
            typematic_epoch: self.typematic_epoch,
            scanning_enabled: self.scanning_enabled,
            resume_scanning_after_id: self.resume_scanning_after_id,
            bat_epoch: self.bat_epoch,
            bat_active: self.bat_active,
            last_sent: self.last_sent,
            actions: self.actions.clone(),
        }
    }

    /// Restores dynamic state after validating identity, wiring, and invariants.
    pub fn restore_state(&mut self, state: Ps2KeyboardState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        self.link.validate_state(&state.link)?;
        if state.scan_fifo.len() > KEYBOARD_FIFO_CAPACITY {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "keyboard scan FIFO must fit its fixed capacity",
            });
        }
        if !(1..=3).contains(&state.scan_set)
            || state.leds & !0x07 != 0
            || state.typematic_parameter & 0x80 != 0
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "keyboard programmable fields must use supported encodings",
            });
        }
        if state.actions.iter().any(|action| match action {
            Ps2KeyboardAction::Schedule { event, .. } => match event {
                Ps2KeyboardEvent::Link(Ps2LinkEvent::Clock { epoch }) => *epoch != state.link.epoch,
                Ps2KeyboardEvent::BatComplete { epoch } => *epoch != state.bat_epoch,
                Ps2KeyboardEvent::Typematic { epoch } => *epoch != state.typematic_epoch,
            },
            Ps2KeyboardAction::Drive(drive) => {
                drive.source != self.id || drive.time > state.link.now
            }
        }) {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "keyboard actions must originate from the keyboard",
            });
        }

        let mut restored = self.clone();
        restored.link.apply_state(state.link);
        restored.responses = state.responses;
        restored.scan_fifo = state.scan_fifo;
        restored.scan_overrun = state.scan_overrun;
        restored.pressed = state.pressed;
        restored.set3_types = state.set3_types;
        restored.command_parameter = state.command_parameter;
        restored.scan_set = state.scan_set;
        restored.leds = state.leds;
        restored.typematic_parameter = state.typematic_parameter;
        restored.typematic_key = state.typematic_key;
        restored.typematic_epoch = state.typematic_epoch;
        restored.scanning_enabled = state.scanning_enabled;
        restored.resume_scanning_after_id = state.resume_scanning_after_id;
        restored.bat_epoch = state.bat_epoch;
        restored.bat_active = state.bat_active;
        restored.last_sent = state.last_sent;
        restored.actions = state.actions;
        *self = restored;
        Ok(())
    }
}

/// Standard three-button PS/2 mouse.
#[derive(Clone, Debug, Eq, PartialEq)]
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

/// Serializable dynamic state of a PS/2 mouse.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ps2MouseState {
    id: ComponentId,
    link: Ps2DeviceLinkState,
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

impl Ps2Mouse {
    /// Captures the mouse's dynamic hardware state.
    pub fn save_state(&self) -> Ps2MouseState {
        Ps2MouseState {
            id: self.id,
            link: self.link.save_state(),
            responses: self.responses.clone(),
            mode: self.mode,
            wrap_mode: self.wrap_mode,
            reporting_enabled: self.reporting_enabled,
            scaling_2_to_1: self.scaling_2_to_1,
            resolution: self.resolution,
            sample_rate: self.sample_rate,
            parameter: self.parameter,
            accumulated_x_eighths: self.accumulated_x_eighths,
            accumulated_y_eighths: self.accumulated_y_eighths,
            buttons: self.buttons,
            last_reported_buttons: self.last_reported_buttons,
            last_packet: self.last_packet,
            last_packet_valid: self.last_packet_valid,
            sample_epoch: self.sample_epoch,
            sample_remainder: self.sample_remainder,
            bat_epoch: self.bat_epoch,
            bat_active: self.bat_active,
            actions: self.actions.clone(),
        }
    }

    /// Restores dynamic state after validating identity, wiring, and invariants.
    pub fn restore_state(&mut self, state: Ps2MouseState) -> Result<(), ComponentStateError> {
        validate_component_state_id(self.id, state.id)?;
        self.link.validate_state(&state.link)?;
        if state.resolution > 3
            || !matches!(state.sample_rate, 10 | 20 | 40 | 60 | 80 | 100 | 200)
            || state.sample_remainder >= u64::from(state.sample_rate)
        {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "mouse resolution, sample rate, and clock remainder must be valid",
            });
        }
        if state.last_packet_valid && state.last_packet[0] & 0x08 == 0 {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "valid mouse packets must contain the fixed status bit",
            });
        }
        if state.actions.iter().any(|action| match action {
            Ps2MouseAction::Schedule { event, .. } => match event {
                Ps2MouseEvent::Link(Ps2LinkEvent::Clock { epoch }) => *epoch != state.link.epoch,
                Ps2MouseEvent::BatComplete { epoch } => *epoch != state.bat_epoch,
                Ps2MouseEvent::Sample { epoch } => *epoch != state.sample_epoch,
            },
            Ps2MouseAction::Drive(drive) => drive.source != self.id || drive.time > state.link.now,
        }) {
            return Err(ComponentStateError::InvalidState {
                component: self.id,
                invariant: "mouse actions must originate from the mouse",
            });
        }

        let mut restored = self.clone();
        restored.link.apply_state(state.link);
        restored.responses = state.responses;
        restored.mode = state.mode;
        restored.wrap_mode = state.wrap_mode;
        restored.reporting_enabled = state.reporting_enabled;
        restored.scaling_2_to_1 = state.scaling_2_to_1;
        restored.resolution = state.resolution;
        restored.sample_rate = state.sample_rate;
        restored.parameter = state.parameter;
        restored.accumulated_x_eighths = state.accumulated_x_eighths;
        restored.accumulated_y_eighths = state.accumulated_y_eighths;
        restored.buttons = state.buttons;
        restored.last_reported_buttons = state.last_reported_buttons;
        restored.last_packet = state.last_packet;
        restored.last_packet_valid = state.last_packet_valid;
        restored.sample_epoch = state.sample_epoch;
        restored.sample_remainder = state.sample_remainder;
        restored.bat_epoch = state.bat_epoch;
        restored.bat_active = state.bat_active;
        restored.actions = state.actions;
        *self = restored;
        Ok(())
    }
}

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
