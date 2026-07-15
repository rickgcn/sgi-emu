//! IP32 event-chain classification and compact bus-event encoding.

#[cfg(test)]
use se_core::component::ComponentId;
#[cfg(test)]
use se_core::scheduler::SimTime;
use se_runtime::runtime::event_chain::{EventChainContext, EventChainPolicy};

use se_device::bus::isa::IsaBusEvent;
use se_device::chipset::crime::iou::{CrimeCgiBusEvent, CrimeCmiBusEvent};
use se_device::chipset::crime::memory::bus::CrimeMemoryBusEvent;

use super::bus::Ip32SysAdBusEvent;
use super::event::Ip32Event;

/// Maximum number of logical bus transitions chained into one outer dispatch.
pub(super) const DEFAULT_EVENT_CHAIN_BUDGET: u8 = 16;

/// Event class controlled by the IP32 event-chain policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum Ip32EventChainClass {
    SysAd,
    Memory,
    Cmi,
    Cgi,
    Isa,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) enum LogicalTransitionPhase {
    Service,
    Complete,
}

#[cfg(test)]
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct LogicalTransition {
    pub(super) time: SimTime,
    pub(super) target: ComponentId,
    pub(super) class: Ip32EventChainClass,
    pub(super) phase: LogicalTransitionPhase,
}

/// Machine-private policy selecting inline IP32 bus transitions.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct Ip32EventChainPolicy {
    pub(super) sysad: bool,
    pub(super) memory: bool,
    pub(super) cmi: bool,
    pub(super) cgi: bool,
    pub(super) isa: bool,
    pub(super) budget: u8,
}

impl Ip32EventChainPolicy {
    pub(super) const fn disabled() -> Self {
        Self {
            sysad: false,
            memory: false,
            cmi: false,
            cgi: false,
            isa: false,
            budget: 0,
        }
    }

    pub(super) const fn all() -> Self {
        Self {
            sysad: true,
            memory: true,
            cmi: true,
            cgi: true,
            isa: true,
            budget: DEFAULT_EVENT_CHAIN_BUDGET,
        }
    }

    const fn enabled(self, class: Ip32EventChainClass) -> bool {
        match class {
            Ip32EventChainClass::SysAd => self.sysad,
            Ip32EventChainClass::Memory => self.memory,
            Ip32EventChainClass::Cmi => self.cmi,
            Ip32EventChainClass::Cgi => self.cgi,
            Ip32EventChainClass::Isa => self.isa,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
enum CompactBusPhase {
    Service,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(super) struct CompactIp32BusEvent {
    epoch: u64,
    class: Ip32EventChainClass,
    phase: CompactBusPhase,
}

impl CompactIp32BusEvent {
    fn decode(self) -> Ip32Event {
        match (self.class, self.phase) {
            (Ip32EventChainClass::SysAd, CompactBusPhase::Service) => {
                Ip32Event::SysAdBus(Ip32SysAdBusEvent::Service {
                    generation: self.epoch,
                })
            }
            (Ip32EventChainClass::SysAd, CompactBusPhase::Complete) => {
                Ip32Event::SysAdBus(Ip32SysAdBusEvent::Complete {
                    generation: self.epoch,
                })
            }
            (Ip32EventChainClass::Memory, CompactBusPhase::Service) => {
                Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Service { epoch: self.epoch })
            }
            (Ip32EventChainClass::Memory, CompactBusPhase::Complete) => {
                Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Complete { epoch: self.epoch })
            }
            (Ip32EventChainClass::Cmi, CompactBusPhase::Service) => {
                Ip32Event::CrimeCmiBus(CrimeCmiBusEvent::Service { epoch: self.epoch })
            }
            (Ip32EventChainClass::Cmi, CompactBusPhase::Complete) => {
                Ip32Event::CrimeCmiBus(CrimeCmiBusEvent::Complete { epoch: self.epoch })
            }
            (Ip32EventChainClass::Cgi, CompactBusPhase::Service) => {
                Ip32Event::CrimeCgiBus(CrimeCgiBusEvent::Service { epoch: self.epoch })
            }
            (Ip32EventChainClass::Cgi, CompactBusPhase::Complete) => {
                unreachable!("CGI completions carry correlation state and are never compacted")
            }
            (Ip32EventChainClass::Isa, CompactBusPhase::Service) => {
                Ip32Event::IsaBus(IsaBusEvent::Service { epoch: self.epoch })
            }
            (Ip32EventChainClass::Isa, CompactBusPhase::Complete) => {
                Ip32Event::IsaBus(IsaBusEvent::Complete { epoch: self.epoch })
            }
        }
    }

    fn encode(event: Ip32Event) -> Result<Self, Ip32Event> {
        let (class, phase, epoch) = match event {
            Ip32Event::SysAdBus(Ip32SysAdBusEvent::Service { generation }) => (
                Ip32EventChainClass::SysAd,
                CompactBusPhase::Service,
                generation,
            ),
            Ip32Event::SysAdBus(Ip32SysAdBusEvent::Complete { generation }) => (
                Ip32EventChainClass::SysAd,
                CompactBusPhase::Complete,
                generation,
            ),
            Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Service { epoch }) => {
                (Ip32EventChainClass::Memory, CompactBusPhase::Service, epoch)
            }
            Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Complete { epoch }) => (
                Ip32EventChainClass::Memory,
                CompactBusPhase::Complete,
                epoch,
            ),
            Ip32Event::CrimeCmiBus(CrimeCmiBusEvent::Service { epoch }) => {
                (Ip32EventChainClass::Cmi, CompactBusPhase::Service, epoch)
            }
            Ip32Event::CrimeCmiBus(CrimeCmiBusEvent::Complete { epoch }) => {
                (Ip32EventChainClass::Cmi, CompactBusPhase::Complete, epoch)
            }
            Ip32Event::CrimeCgiBus(CrimeCgiBusEvent::Service { epoch }) => {
                (Ip32EventChainClass::Cgi, CompactBusPhase::Service, epoch)
            }
            event @ Ip32Event::CrimeCgiBus(CrimeCgiBusEvent::Complete { .. }) => {
                return Err(event);
            }
            Ip32Event::IsaBus(IsaBusEvent::Service { epoch }) => {
                (Ip32EventChainClass::Isa, CompactBusPhase::Service, epoch)
            }
            Ip32Event::IsaBus(IsaBusEvent::Complete { epoch }) => {
                (Ip32EventChainClass::Isa, CompactBusPhase::Complete, epoch)
            }
            event => return Err(event),
        };
        Ok(Self {
            epoch,
            class,
            phase,
        })
    }
}

impl EventChainPolicy<Ip32Event> for Ip32EventChainPolicy {
    type CompactEvent = CompactIp32BusEvent;

    fn is_active(&self) -> bool {
        self.sysad || self.memory || self.cmi || self.cgi || self.isa
    }

    fn budget(&self) -> u8 {
        self.budget
    }

    fn encode(&self, event: Ip32Event) -> Result<Self::CompactEvent, Ip32Event> {
        CompactIp32BusEvent::encode(event)
    }

    fn decode(&self, event: Self::CompactEvent) -> Ip32Event {
        event.decode()
    }

    fn is_enabled(&self, event: &Self::CompactEvent) -> bool {
        self.enabled(event.class)
    }

    fn is_barrier(&self, event: &Ip32Event) -> bool {
        matches!(
            event,
            Ip32Event::PowerOn
                | Ip32Event::HardReset
                | Ip32Event::Crime(_)
                | Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Refresh { .. })
                | Ip32Event::Mace(_)
                | Ip32Event::Ds2502(_)
                | Ip32Event::PciBusService
                | Ip32Event::I2cBusService { .. }
                | Ip32Event::Uart { .. }
                | Ip32Event::HostInput { .. }
        )
    }
}

pub(super) type Ip32DispatchContext<'runtime, 'context, S> =
    EventChainContext<'runtime, 'context, Ip32Event, S, Ip32EventChainPolicy>;

#[cfg(test)]
pub(super) fn logical_transition(
    time: SimTime,
    target: ComponentId,
    event: &Ip32Event,
) -> Option<LogicalTransition> {
    let (class, phase) = match event {
        Ip32Event::SysAdBus(Ip32SysAdBusEvent::Service { .. }) => {
            (Ip32EventChainClass::SysAd, LogicalTransitionPhase::Service)
        }
        Ip32Event::SysAdBus(Ip32SysAdBusEvent::Complete { .. }) => {
            (Ip32EventChainClass::SysAd, LogicalTransitionPhase::Complete)
        }
        Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Service { .. }) => {
            (Ip32EventChainClass::Memory, LogicalTransitionPhase::Service)
        }
        Ip32Event::CrimeMemoryBus(CrimeMemoryBusEvent::Complete { .. }) => (
            Ip32EventChainClass::Memory,
            LogicalTransitionPhase::Complete,
        ),
        Ip32Event::CrimeCmiBus(CrimeCmiBusEvent::Service { .. }) => {
            (Ip32EventChainClass::Cmi, LogicalTransitionPhase::Service)
        }
        Ip32Event::CrimeCmiBus(CrimeCmiBusEvent::Complete { .. }) => {
            (Ip32EventChainClass::Cmi, LogicalTransitionPhase::Complete)
        }
        Ip32Event::CrimeCgiBus(CrimeCgiBusEvent::Service { .. }) => {
            (Ip32EventChainClass::Cgi, LogicalTransitionPhase::Service)
        }
        Ip32Event::CrimeCgiBus(CrimeCgiBusEvent::Complete { .. }) => {
            (Ip32EventChainClass::Cgi, LogicalTransitionPhase::Complete)
        }
        Ip32Event::IsaBus(IsaBusEvent::Service { .. }) => {
            (Ip32EventChainClass::Isa, LogicalTransitionPhase::Service)
        }
        Ip32Event::IsaBus(IsaBusEvent::Complete { .. }) => {
            (Ip32EventChainClass::Isa, LogicalTransitionPhase::Complete)
        }
        _ => return None,
    };
    Some(LogicalTransition {
        time,
        target,
        class,
        phase,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_bus_transition_meets_hot_path_size_limit() {
        assert!(
            core::mem::size_of::<(
                se_core::scheduler::SimTime,
                ComponentId,
                u64,
                CompactIp32BusEvent,
            )>() <= 40
        );
    }
}
