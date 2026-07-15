//! Owned protocols emitted and accepted by MACE.

use core::ops::Deref;
use se_core::component::ComponentId;
use se_core::scheduler::{SimDuration, SimTime};
use se_core::tracing::TraceLevel;
use smallvec::SmallVec;

use crate::bus::i2c::I2cTransaction;
use crate::bus::isa::IsaTransaction;
use crate::bus::media::{MediaPayload, MediaPort, MediaTransaction};
use crate::bus::one_wire::OneWireDrive;
use crate::bus::pci::PciTransaction;
use crate::bus::two_wire::TwoWireDrive;
use crate::chipset::crime::protocol::{CrimeCmiCompletion, CrimeCmiTransaction};

/// Board wiring used by the MACE controller roles.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceExternalLinks {
    pub i2c: [ComponentId; 2],
    pub audio: ComponentId,
    pub video_input_ab: ComponentId,
    pub video_input_cd: ComponentId,
    pub video_output: ComponentId,
    pub ethernet: ComponentId,
}

/// Topological component identifiers used by MACE.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceWiring {
    pub crime: ComponentId,
    pub pci_bus: ComponentId,
    pub pci_devices: [ComponentId; 5],
    pub pci_absent: ComponentId,
    pub isa_bus: ComponentId,
    pub prom: ComponentId,
    pub rtc: ComponentId,
    pub serial: [ComponentId; 2],
    pub parallel: ComponentId,
    pub ps2_buses: [ComponentId; 2],
    pub external_links: MaceExternalLinks,
}

/// Scheduled internal MACE transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MaceEvent {
    TimerCompare {
        epoch: u64,
        timer: u8,
    },
    Ps2Transition {
        epoch: u64,
        port: u8,
        port_epoch: u64,
    },
    I2cComplete {
        epoch: u64,
        port: u8,
    },
    DmaStep {
        epoch: u64,
        channel: u8,
    },
    VideoLine {
        epoch: u64,
        channel: u8,
    },
    EthernetStep {
        epoch: u64,
    },
}

/// Host-neutral input scheduled by the machine profile.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceHostInput {
    pub time: SimTime,
    pub port: MediaPort,
    pub payload: MediaPayload,
}

/// Structured MACE trace value.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum MaceTraceValue {
    Bool(bool),
    U64(u64),
    Hex64(u64),
    String(&'static str),
}

/// Structured MACE trace field.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaceTraceField {
    pub key: &'static str,
    pub value: MaceTraceValue,
}

/// Ordered trace fields with inline storage for common MACE events.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MaceTraceFields(SmallVec<[MaceTraceField; 8]>);

impl MaceTraceFields {
    /// Returns whether the fields spilled beyond inline storage.
    pub fn spilled(&self) -> bool {
        self.0.spilled()
    }
}

impl Deref for MaceTraceFields {
    type Target = [MaceTraceField];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<Vec<MaceTraceField>> for MaceTraceFields {
    fn from(value: Vec<MaceTraceField>) -> Self {
        Self(SmallVec::from_vec(value))
    }
}

impl<const N: usize> From<[MaceTraceField; N]> for MaceTraceFields {
    fn from(value: [MaceTraceField; N]) -> Self {
        Self(value.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_fields_inline_eight_entries() {
        let fields = MaceTraceFields::from(core::array::from_fn::<_, 8, _>(|_| MaceTraceField {
            key: "field",
            value: MaceTraceValue::U64(0),
        }));
        assert!(!fields.spilled());
        let fields = MaceTraceFields::from(vec![
            MaceTraceField {
                key: "field",
                value: MaceTraceValue::U64(0),
            };
            9
        ]);
        assert!(fields.spilled());
    }
}

/// Structured MACE trace event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaceTraceEvent {
    pub level: TraceLevel,
    pub target: &'static str,
    pub event: &'static str,
    pub fields: MaceTraceFields,
}

/// Action emitted while polling MACE.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
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
    SetOneWire(OneWireDrive),
    SetTwoWire {
        bus: ComponentId,
        drive: TwoWireDrive,
    },
    CompleteCmiDevice(CrimeCmiCompletion),
    Trace(Box<MaceTraceEvent>),
}

/// Result of polling MACE.
#[derive(Clone, Debug, Eq, PartialEq)]
#[allow(clippy::large_enum_variant)]
pub enum MacePoll {
    Action(MaceAction),
    Idle,
}
