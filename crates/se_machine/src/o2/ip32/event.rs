//! Machine events for the IP32 profile.
//!
//! These events represent board-level control transitions handled by the IP32
//! machine integration.

use se_device::bus::isa::IsaBusEvent;
use se_device::bus::media::{MediaPayload, MediaPort};
use se_device::chipset::crime::protocol::CrimeEvent;
use se_device::chipset::gbe::protocol::{GbeEvent, GbeExternalInput};
use se_device::chipset::mace::protocol::MaceEvent;
use se_device::input::ps2::{Ps2KeyboardEvent, Ps2KeyboardInput, Ps2MouseEvent, Ps2MouseInput};
use se_device::memory::ds2502::Ds2502Event;
use se_device::serial::uart16550::Uart16550Event;

/// IP32 machine-level event payload.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ip32Event {
    /// Initial board power-on event.
    PowerOn,

    /// Board hard-reset event.
    HardReset,

    /// Executes one CPU architectural boundary for the active reset epoch.
    CpuStep {
        /// Reset generation that scheduled this step.
        generation: u64,
    },

    /// CRIME-internal event.
    Crime(CrimeEvent),

    /// GBE-internal timing transition.
    Gbe(GbeEvent),

    /// Deterministic external GBE pin or clock input.
    GbeInput(GbeExternalInput),

    /// MACE-internal event.
    Mace(MaceEvent),

    /// Keyboard-internal serial or timing transition.
    Keyboard(Ps2KeyboardEvent),

    /// Mouse-internal serial or timing transition.
    Mouse(Ps2MouseEvent),

    /// Board-identity DS2502 transition.
    Ds2502(Ds2502Event),

    /// MACE ISA-domain event.
    IsaBus(IsaBusEvent),

    /// One UART-internal event.
    Uart {
        port: Ip32SerialPort,
        event: Uart16550Event,
    },

    /// One deterministic host-neutral input.
    HostInput {
        /// Host-input generation that scheduled this event.
        generation: u64,
        /// Input payload and destination port.
        input: Ip32HostInput,
    },

    /// Deterministic physical keyboard or mouse input.
    Input {
        /// Host-input generation that scheduled this event.
        generation: u64,
        /// Physical input payload.
        input: Ip32InputEvent,
    },
}

/// Physical input accepted by the IP32 keyboard and mouse devices.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ip32InputEvent {
    /// Keyboard input.
    Keyboard(Ps2KeyboardInput),
    /// Mouse input.
    Mouse(Ps2MouseInput),
    /// Releases every currently pressed key and button.
    ReleaseAll,
}

/// Physical serial connectors exposed by the IP32 profile.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ip32SerialPort {
    /// First external serial connector.
    Serial1,
    /// Second external serial connector.
    Serial2,
}

/// Host-neutral input with no implicit wall-clock time.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32HostInput {
    /// Destination media port.
    pub port: MediaPort,
    /// Host-provided media payload.
    pub payload: MediaPayload,
}

/// Host-neutral output produced in deterministic simulation order.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32HostOutput {
    /// Source media port.
    pub port: MediaPort,
    /// Guest-produced media payload.
    pub payload: MediaPayload,
}

/// One host-bound byte chunk emitted by a serial connector.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32SerialOutput {
    /// Source serial connector.
    pub port: Ip32SerialPort,
    /// Bytes emitted in deterministic order.
    pub bytes: Vec<u8>,
}

/// Host-bound data-loss counters.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32HostIoStats {
    /// Dropped output bytes indexed by media port.
    pub dropped_output_bytes: [u64; 12],
}
