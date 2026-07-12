//! Machine-level events for the IP32 profile.
//!
//! These events represent board-level control transitions handled by machine
//! orchestration.

use se_device::bus::isa::IsaBusEvent;
use se_device::bus::media::{MediaPayload, MediaPort};
use se_device::chipset::crime::iou::{CrimeCgiBusEvent, CrimeCmiBusEvent};
use se_device::chipset::crime::memory::bus::CrimeMemoryBusEvent;
use se_device::chipset::crime::protocol::CrimeEvent;
use se_device::chipset::mace::protocol::MaceEvent;

use super::bus::Ip32SysAdBusEvent;

/// IP32 machine-level event payload.
#[derive(Clone, Debug, Eq, PartialEq)]
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

    /// CPU SysAD bus event.
    SysAdBus(Ip32SysAdBusEvent),

    /// CRIME memory-domain event.
    CrimeMemoryBus(CrimeMemoryBusEvent),

    /// CRIME-to-MACE link event.
    CrimeCmiBus(CrimeCmiBusEvent),

    /// CRIME-to-GBE link event.
    CrimeCgiBus(CrimeCgiBusEvent),

    /// MACE-internal event.
    Mace(MaceEvent),

    /// MACE ISA-domain event.
    IsaBus(IsaBusEvent),

    /// One PCI arbitration opportunity.
    PciBusService,

    /// One I2C arbitration opportunity.
    I2cBusService {
        /// I2C controller index.
        index: u8,
    },

    /// One deterministic host-neutral input.
    HostInput(Ip32HostInput),
}

/// Host-neutral input with no implicit wall-clock time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32HostInput {
    pub port: MediaPort,
    pub payload: MediaPayload,
}

/// Host-neutral output produced in deterministic simulation order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ip32HostOutput {
    pub port: MediaPort,
    pub payload: MediaPayload,
}
