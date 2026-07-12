//! Owned protocols emitted and accepted by MACE.

use se_core::component::ComponentId;
use se_core::scheduler::{SimDuration, SimTime};
use se_core::tracing::TraceLevel;

use crate::bus::i2c::I2cTransaction;
use crate::bus::isa::IsaTransaction;
use crate::bus::media::{MediaPayload, MediaPort, MediaTransaction};
use crate::bus::pci::PciTransaction;
use crate::chipset::crime::protocol::{CrimeCmiCompletion, CrimeCmiTransaction};

/// Board wiring used by the MACE controller roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaceExternalLinks {
    pub i2c: [ComponentId; 2],
    pub audio: ComponentId,
    pub video_input_ab: ComponentId,
    pub video_input_cd: ComponentId,
    pub video_output: ComponentId,
    pub ethernet: ComponentId,
    pub keyboard: ComponentId,
    pub mouse: ComponentId,
}

/// Topological component identifiers used by MACE.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaceWiring {
    pub crime: ComponentId,
    pub pci_bus: ComponentId,
    pub pci_devices: [ComponentId; 5],
    pub isa_bus: ComponentId,
    pub prom: ComponentId,
    pub rtc: ComponentId,
    pub serial: [ComponentId; 2],
    pub parallel: ComponentId,
    pub external_links: MaceExternalLinks,
}

/// Scheduled internal MACE transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaceEvent {
    TimerCompare { epoch: u64, timer: u8 },
    Ps2Transmit { epoch: u64, port: u8 },
    I2cComplete { epoch: u64, port: u8 },
    DmaStep { epoch: u64, channel: u8 },
    VideoLine { epoch: u64, channel: u8 },
    EthernetStep { epoch: u64 },
}

/// Host-neutral input scheduled by the machine profile.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaceHostInput {
    pub time: SimTime,
    pub port: MediaPort,
    pub payload: MediaPayload,
}

/// Structured MACE trace value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaceTraceValue {
    Bool(bool),
    U64(u64),
    Hex64(u64),
    String(&'static str),
}

/// Structured MACE trace field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaceTraceField {
    pub key: &'static str,
    pub value: MaceTraceValue,
}

/// Structured MACE trace event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaceTraceEvent {
    pub level: TraceLevel,
    pub target: &'static str,
    pub event: &'static str,
    pub fields: Vec<MaceTraceField>,
}

/// Action emitted while polling MACE.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MaceAction {
    Schedule {
        delay: SimDuration,
        event: MaceEvent,
    },
    StartCmi(CrimeCmiTransaction),
    StartPci(PciTransaction),
    StartIsa(IsaTransaction),
    StartI2c(I2cTransaction),
    StartExternal(MediaTransaction),
    CompleteCmiDevice(CrimeCmiCompletion),
    Trace(MaceTraceEvent),
}

/// Result of polling MACE.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MacePoll {
    Action(MaceAction),
    Idle,
}
