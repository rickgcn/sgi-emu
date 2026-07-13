//! SGI O2 IP32 SysAD domain and deterministic non-MACE peer endpoints.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;
use se_core::scheduler::SimDuration;
use se_device::chipset::crime::protocol::CrimeBusDisposition;
use se_device::cpu::execution::protocol::{ExecutionCompletion, ExecutionTransaction};
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};

macro_rules! component_state {
    ($state:ident, $component:ty) => {
        #[doc = "Serializable deterministic component state."]
        #[derive(Clone, serde::Deserialize, serde::Serialize)]
        pub struct $state($component);

        impl $component {
            #[doc = "Captures all hardware-visible and in-flight component state."]
            pub fn save_state(&self) -> $state {
                $state(self.clone())
            }

            #[doc = "Restores validated component state without changing topology identity."]
            pub fn restore_state(
                &mut self,
                state: $state,
            ) -> Result<(), se_device::state::DeviceStateError> {
                let expected = Component::id(self);
                let actual = Component::id(&state.0);
                if actual != expected {
                    return Err(se_device::state::DeviceStateError::ComponentIdMismatch {
                        expected,
                        actual,
                    });
                }
                *self = state.0;
                Ok(())
            }
        }
    };
}

/// Scheduled SysAD bus event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ip32SysAdBusEvent {
    /// Delivers the queued CPU transaction to CRIME.
    Service {
        /// Reset generation.
        generation: u64,
    },

    /// Delivers the CRIME completion to the CPU.
    Complete {
        /// Reset generation.
        generation: u64,
    },
}

/// Action emitted by the IP32 SysAD domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum Ip32SysAdBusAction {
    /// Delivers one CPU request to CRIME.
    Deliver {
        /// CRIME component.
        target: ComponentId,

        /// Original CPU transaction.
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    },

    /// Delivers one CRIME completion to the CPU controller.
    Complete {
        /// CPU component.
        controller: ComponentId,

        /// Original CPU request used for tracing and correlation checks.
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,

        /// Correlated completion.
        completion: ExecutionCompletion<Mips4ExecutionCompletion>,
    },

    /// Requests another bus event.
    Schedule {
        /// Delivery delay.
        delay: SimDuration,

        /// Event payload.
        event: Ip32SysAdBusEvent,
    },

    /// No action is ready.
    Idle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct BusClock {
    timebase_hz: u64,
    frequency_hz: u64,
    remainder: u64,
}

impl BusClock {
    const fn new(timebase_hz: u64, frequency_hz: u64) -> Self {
        assert!(timebase_hz != 0, "the machine timebase must be nonzero");
        assert!(frequency_hz != 0, "the bus frequency must be nonzero");
        Self {
            timebase_hz,
            frequency_hz,
            remainder: 0,
        }
    }

    fn reset(&mut self) {
        self.remainder = 0;
    }

    fn next_cycle(&mut self) -> SimDuration {
        let base = self.timebase_hz / self.frequency_hz;
        self.remainder += self.timebase_hz % self.frequency_hz;
        let carry = self.remainder / self.frequency_hz;
        self.remainder %= self.frequency_hz;
        SimDuration::new(base + carry)
    }
}

/// CPU-facing IP32 SysAD communication domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32SysAdBus {
    id: ComponentId,
    name: String,
    controller: ComponentId,
    target: ComponentId,
    clock: BusClock,
    generation: u64,
    service_scheduled: bool,
    queue: VecDeque<ExecutionTransaction<Mips4ExecutionTransaction>>,
    in_flight: Option<ExecutionTransaction<Mips4ExecutionTransaction>>,
    pending_completion: Option<(
        ExecutionTransaction<Mips4ExecutionTransaction>,
        ExecutionCompletion<Mips4ExecutionCompletion>,
    )>,
    actions: VecDeque<Ip32SysAdBusAction>,
}

component_state!(Ip32SysAdBusState, Ip32SysAdBus);

impl Ip32SysAdBus {
    /// Creates a SysAD domain connecting one CPU controller to CRIME.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        controller: ComponentId,
        target: ComponentId,
        timebase_hz: u64,
        frequency_hz: u64,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            controller,
            target,
            clock: BusClock::new(timebase_hz, frequency_hz),
            generation: 0,
            service_scheduled: false,
            queue: VecDeque::new(),
            in_flight: None,
            pending_completion: None,
            actions: VecDeque::new(),
        }
    }

    /// Cancels traffic and advances the bus generation.
    pub fn hard_reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.clock.reset();
        self.service_scheduled = false;
        self.queue.clear();
        self.in_flight = None;
        self.pending_completion = None;
        self.actions.clear();
    }

    /// Handles one scheduled SysAD event.
    pub fn handle_event(&mut self, event: Ip32SysAdBusEvent) {
        let generation = match event {
            Ip32SysAdBusEvent::Service { generation }
            | Ip32SysAdBusEvent::Complete { generation } => generation,
        };
        if generation != self.generation {
            return;
        }
        match event {
            Ip32SysAdBusEvent::Service { .. } => {
                self.service_scheduled = false;
                if self.in_flight.is_some() || self.pending_completion.is_some() {
                    return;
                }
                let Some(transaction) = self.queue.pop_front() else {
                    return;
                };
                self.in_flight = Some(transaction.clone());
                self.actions.push_back(Ip32SysAdBusAction::Deliver {
                    target: self.target,
                    transaction,
                });
            }
            Ip32SysAdBusEvent::Complete { .. } => {
                let Some((transaction, completion)) = self.pending_completion.take() else {
                    return;
                };
                self.actions.push_back(Ip32SysAdBusAction::Complete {
                    controller: self.controller,
                    transaction,
                    completion,
                });
                if !self.queue.is_empty() {
                    self.service_scheduled = true;
                    self.actions.push_back(Ip32SysAdBusAction::Schedule {
                        delay: self.clock.next_cycle(),
                        event: Ip32SysAdBusEvent::Service {
                            generation: self.generation,
                        },
                    });
                }
            }
        }
    }

    /// Accepts CRIME's response to the current CPU request.
    pub fn accept_device_completion(
        &mut self,
        completion: ExecutionCompletion<Mips4ExecutionCompletion>,
    ) {
        let Some(transaction) = self.in_flight.take() else {
            return;
        };
        if transaction.id != completion.id {
            self.in_flight = Some(transaction);
            return;
        }
        self.pending_completion = Some((transaction, completion));
        self.actions.push_back(Ip32SysAdBusAction::Schedule {
            delay: self.clock.next_cycle(),
            event: Ip32SysAdBusEvent::Complete {
                generation: self.generation,
            },
        });
    }

    /// Polls one SysAD action.
    pub fn poll(&mut self) -> Ip32SysAdBusAction {
        self.actions.pop_front().unwrap_or(Ip32SysAdBusAction::Idle)
    }

    /// Returns the active reset generation.
    pub const fn generation(&self) -> u64 {
        self.generation
    }
}

impl Component for Ip32SysAdBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.hard_reset();
    }
}

impl BusRole<ExecutionTransaction<Mips4ExecutionTransaction>> for Ip32SysAdBus {
    type Response = CrimeBusDisposition;

    fn route(
        &mut self,
        transaction: ExecutionTransaction<Mips4ExecutionTransaction>,
    ) -> Self::Response {
        self.queue.push_back(transaction);
        if self.service_scheduled || self.in_flight.is_some() || self.pending_completion.is_some() {
            CrimeBusDisposition::Queued
        } else {
            self.service_scheduled = true;
            CrimeBusDisposition::QueuedAndNeedsService {
                delay: self.clock.next_cycle(),
                epoch: self.generation,
            }
        }
    }
}

/// Identity-only endpoint for an unimplemented board device.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct Ip32StubEndpoint {
    id: ComponentId,
    name: String,
}

component_state!(Ip32StubEndpointState, Ip32StubEndpoint);

impl Ip32StubEndpoint {
    /// Creates an identity-only endpoint.
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

impl Component for Ip32StubEndpoint {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {}
}

#[cfg(test)]
mod tests;
