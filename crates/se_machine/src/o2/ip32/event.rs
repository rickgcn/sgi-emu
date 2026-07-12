//! Machine-level events for the IP32 profile.
//!
//! These events represent board-level control transitions handled by machine
//! orchestration.

use se_device::chipset::crime::iou::{CrimeCgiBusEvent, CrimeCmiBusEvent};
use se_device::chipset::crime::memory::bus::CrimeMemoryBusEvent;
use se_device::chipset::crime::protocol::CrimeEvent;

use super::bus::Ip32SysAdBusEvent;

/// IP32 machine-level event payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
}
