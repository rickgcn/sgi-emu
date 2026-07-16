//! SGI O2 IP32 SysAD domain and deterministic non-MACE peer endpoints.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;
use se_core::scheduler::{FractionalClockProjection, SimDuration, SimTime};
use se_device::chipset::crime::protocol::{
    CrimeBusDisposition, CrimeByteEnable, CrimeCompletionPayload, CrimeData, CrimeSysAdCompletion,
    CrimeSysAdRequest, CrimeTransactionId, CrimeTransfer,
};
use se_device::cpu::execution::protocol::{
    ExecutionCompletion, ExecutionTransaction, ExecutionTransactionId,
};
use se_device::cpu::mips4::execution::bus::{Mips4ExecutionCompletion, Mips4ExecutionTransaction};

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

        /// Physical request translated for CRIME.
        request: CrimeSysAdRequest,
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

/// Immutable timing proof for one direct transaction on an idle SysAD bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ip32DirectSysAdPlan {
    generation: u64,
    clock_remainder: u64,
    request_delay: SimDuration,
    completion_delay: SimDuration,
}

impl Ip32DirectSysAdPlan {
    /// Returns the delay from CPU request to CRIME delivery.
    pub const fn request_delay(self) -> SimDuration {
        self.request_delay
    }

    /// Returns the delay from CRIME completion to CPU delivery.
    pub const fn completion_delay(self) -> SimDuration {
        self.completion_delay
    }
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

    fn projection(self) -> FractionalClockProjection {
        FractionalClockProjection::new(self.timebase_hz, self.frequency_hz, self.remainder)
    }

    fn advance_cycles(&mut self, cycles: u64) -> SimDuration {
        let mut projection = self.projection();
        let elapsed = projection
            .advance(cycles)
            .expect("a machine bus clock advance must fit simulated time");
        self.remainder = projection.remainder();
        elapsed
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

se_core::component_state!(Ip32SysAdBusState, Ip32SysAdBus);

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
    pub fn handle_event(&mut self, now: SimTime, event: Ip32SysAdBusEvent) {
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
                    request: Self::translate_cpu_transaction(&transaction, now),
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
    pub fn accept_device_completion(&mut self, completion: CrimeSysAdCompletion) {
        let Some(transaction) = self.in_flight.take() else {
            return;
        };
        let Some(completion) = Self::translate_crime_completion(&transaction, completion) else {
            self.in_flight = Some(transaction);
            return;
        };
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

    /// Translates one CPU execution transaction into a CRIME SysAD request.
    pub(super) fn translate_cpu_transaction(
        transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
        time: SimTime,
    ) -> CrimeSysAdRequest {
        let (address, transfer) = match transaction.payload {
            Mips4ExecutionTransaction::Read {
                physical_address,
                size,
                ..
            } => (
                physical_address,
                CrimeTransfer::read(u16::from(size.bytes())),
            ),
            Mips4ExecutionTransaction::Write {
                physical_address,
                size,
                data,
                byte_enable,
                ..
            } => {
                let length = usize::from(size.bytes());
                (
                    physical_address,
                    CrimeTransfer::write(
                        data.to_le_bytes()[..length].iter().copied().collect(),
                        (0..length)
                            .map(|lane| byte_enable & (1 << lane) != 0)
                            .collect::<CrimeByteEnable>(),
                    ),
                )
            }
        };
        CrimeSysAdRequest {
            id: Self::crime_transaction_id(transaction.id),
            time,
            address,
            transfer,
        }
    }

    /// Translates a correlated CRIME completion back into the CPU protocol.
    pub(super) fn translate_crime_completion(
        transaction: &ExecutionTransaction<Mips4ExecutionTransaction>,
        completion: CrimeSysAdCompletion,
    ) -> Option<ExecutionCompletion<Mips4ExecutionCompletion>> {
        if completion.id != Self::crime_transaction_id(transaction.id) {
            return None;
        }
        let payload = match (transaction.payload, completion.result) {
            (
                Mips4ExecutionTransaction::Read { size, .. },
                Ok(CrimeCompletionPayload::ReadData(data)),
            ) if data.len() == usize::from(size.bytes()) => Self::pack_read_data(&data)
                .map(Mips4ExecutionCompletion::ReadData)
                .unwrap_or(Mips4ExecutionCompletion::BusError),
            (
                Mips4ExecutionTransaction::Write { .. },
                Ok(CrimeCompletionPayload::WriteComplete),
            ) => Mips4ExecutionCompletion::WriteComplete,
            (_, Err(_))
            | (_, Ok(CrimeCompletionPayload::ReadData(_)))
            | (_, Ok(CrimeCompletionPayload::WriteComplete)) => Mips4ExecutionCompletion::BusError,
        };
        Some(ExecutionCompletion {
            id: transaction.id,
            payload,
        })
    }

    /// Packs ascending physical bytes into the CPU's low-order lane representation.
    pub(super) fn pack_read_data(data: &CrimeData) -> Option<u64> {
        if data.len() > 8 {
            return None;
        }
        let mut lanes = [0; 8];
        lanes[..data.len()].copy_from_slice(data);
        Some(u64::from_le_bytes(lanes))
    }

    const fn crime_transaction_id(id: ExecutionTransactionId) -> CrimeTransactionId {
        CrimeTransactionId::new(id.get())
    }

    /// Returns the active reset generation.
    pub const fn generation(&self) -> u64 {
        self.generation
    }

    /// Returns whether a stable fetch may bypass this otherwise idle bus.
    pub fn stable_fetch_ready(&self) -> bool {
        !self.service_scheduled
            && self.queue.is_empty()
            && self.in_flight.is_none()
            && self.pending_completion.is_none()
            && self.actions.is_empty()
    }

    /// Returns the exact current fractional clock used by stable fetches.
    pub fn stable_fetch_clock(&self) -> Option<FractionalClockProjection> {
        self.stable_fetch_ready().then(|| self.clock.projection())
    }

    /// Plans one request/completion pair on an otherwise idle bus.
    pub fn plan_direct_transaction(&self) -> Option<Ip32DirectSysAdPlan> {
        if !self.stable_fetch_ready() {
            return None;
        }
        let mut clock = self.clock;
        Some(Ip32DirectSysAdPlan {
            generation: self.generation,
            clock_remainder: self.clock.remainder,
            request_delay: clock.next_cycle(),
            completion_delay: clock.next_cycle(),
        })
    }

    /// Returns whether an idle-bus timing proof still owns the exact bus state.
    pub fn direct_transaction_plan_valid(&self, plan: Ip32DirectSysAdPlan) -> bool {
        self.stable_fetch_ready()
            && self.generation == plan.generation
            && self.clock.remainder == plan.clock_remainder
    }

    /// Commits one previously validated direct request/completion pair.
    #[must_use]
    pub fn commit_direct_transaction(&mut self, plan: Ip32DirectSysAdPlan) -> bool {
        if !self.direct_transaction_plan_valid(plan) {
            return false;
        }
        let mut clock = self.clock;
        if clock.next_cycle() != plan.request_delay || clock.next_cycle() != plan.completion_delay {
            return false;
        }
        self.clock = clock;
        true
    }

    /// Commits a proven batch of direct request/completion cycle pairs.
    pub fn commit_direct_transactions(&mut self, transactions: u64) {
        assert!(self.stable_fetch_ready());
        let _ = self.clock.advance_cycles(transactions.saturating_mul(2));
    }

    /// Plans idle request/completion cycle pairs without changing bus state.
    pub fn plan_stable_fetches(&self, output: &mut [SimDuration]) -> Option<()> {
        if !self.stable_fetch_ready() {
            return None;
        }
        let mut clock = self.clock;
        for delay in output {
            *delay = SimDuration::new(
                clock
                    .next_cycle()
                    .get()
                    .saturating_add(clock.next_cycle().get()),
            );
        }
        Some(())
    }

    /// Commits idle request/completion cycle pairs consumed by stable fetches.
    pub fn commit_stable_fetches(&mut self, fetches: usize) {
        assert!(self.stable_fetch_ready());
        let _ = self
            .clock
            .advance_cycles((fetches as u64).saturating_mul(2));
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

se_core::component_state!(Ip32StubEndpointState, Ip32StubEndpoint);

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
