//! Stateful CRIME memory-bus communication domain.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;
use se_core::scheduler::{SimDuration, SimTime};

use super::super::clock::CrimeClock;
use super::super::protocol::{
    CrimeBusAction, CrimeBusDisposition, CrimeMemoryClient, CrimeMemoryCompletion,
    CrimeMemoryTransaction, CrimeTransactionId,
};

/// Scheduled event interpreted by the CRIME memory bus.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeMemoryBusEvent {
    /// Selects and delivers the next arbitration winner.
    Service {
        /// Reset epoch.
        epoch: u64,
    },

    /// Makes one completed target response visible to its controller.
    Complete {
        /// Reset epoch.
        epoch: u64,
    },

    /// Marks one refresh request pending.
    Refresh {
        /// Reset epoch.
        epoch: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct PendingCompletion {
    controller: ComponentId,
    completion: CrimeMemoryCompletion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct InFlightMemory {
    controller: ComponentId,
    transaction_id: CrimeTransactionId,
}

/// Immutable timing and ownership proof for one idle CPU memory transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CrimeDirectMemoryPlan {
    epoch: u64,
    clock: se_core::scheduler::FractionalClockProjection,
    slot: u8,
    submission_time: SimTime,
    next_refresh_time: SimTime,
    service_delay: SimDuration,
    completion_delay: SimDuration,
}

impl CrimeDirectMemoryPlan {
    /// Returns the delay before SDRAM delivery.
    pub const fn service_delay(self) -> SimDuration {
        self.service_delay
    }

    /// Returns the delay before the memory completion becomes visible.
    pub const fn completion_delay(self) -> SimDuration {
        self.completion_delay
    }
}

/// CRIME MIU arbitration and ordering domain.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeMemoryBus {
    id: ComponentId,
    name: String,
    target: ComponentId,
    clock: CrimeClock,
    refresh_delay: SimDuration,
    epoch: u64,
    slot: u8,
    service_scheduled: bool,
    next_refresh_time: SimTime,
    refresh_debt: u64,
    gbe: VecDeque<CrimeMemoryTransaction>,
    mace: VecDeque<CrimeMemoryTransaction>,
    vice: VecDeque<CrimeMemoryTransaction>,
    render: VecDeque<CrimeMemoryTransaction>,
    cpu: VecDeque<CrimeMemoryTransaction>,
    in_flight: Option<InFlightMemory>,
    pending_completion: Option<PendingCompletion>,
    actions: VecDeque<CrimeBusAction<CrimeMemoryTransaction, CrimeMemoryCompletion>>,
}

se_core::component_state!(CrimeMemoryBusState, CrimeMemoryBus);

impl CrimeMemoryBus {
    /// Creates a memory domain with machine-calculated CRIME timing.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        target: ComponentId,
        timebase_hz: u64,
        refresh_delay: SimDuration,
    ) -> Self {
        Self {
            id,
            name: name.into(),
            target,
            clock: CrimeClock::new(timebase_hz),
            refresh_delay,
            epoch: 0,
            slot: 0,
            service_scheduled: false,
            next_refresh_time: SimTime::ZERO,
            refresh_debt: 0,
            gbe: VecDeque::new(),
            mace: VecDeque::new(),
            vice: VecDeque::new(),
            render: VecDeque::new(),
            cpu: VecDeque::new(),
            in_flight: None,
            pending_completion: None,
            actions: VecDeque::new(),
        }
    }

    /// Resets transient state and begins lazy refresh accounting.
    pub fn power_on(&mut self, now: SimTime) {
        self.reset_state(now);
    }

    /// Resets in-flight state while preserving SDRAM contents.
    pub fn hard_reset(&mut self, now: SimTime) {
        self.reset_state(now);
    }

    /// Returns whether a stable CPU fetch may bypass this otherwise idle bus.
    pub fn stable_fetch_ready(&self) -> bool {
        !self.service_scheduled
            && !self.has_pending()
            && self.in_flight.is_none()
            && self.pending_completion.is_none()
            && self.actions.is_empty()
    }

    /// Returns the exact current fractional clock used by stable fetches.
    pub fn stable_fetch_clock(&self) -> Option<se_core::scheduler::FractionalClockProjection> {
        self.stable_fetch_ready().then(|| self.clock.projection())
    }

    /// Returns the next lazy refresh boundary for stable fetch planning.
    pub fn stable_fetch_refresh_deadline(&self) -> Option<SimTime> {
        (self.refresh_delay != SimDuration::ZERO).then_some(self.next_refresh_time)
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

    /// Commits idle CPU request/completion cycles consumed by stable fetches.
    pub fn commit_stable_fetches(&mut self, fetches: usize) {
        assert!(self.stable_fetch_ready());
        let _ = self
            .clock
            .advance_cycles((fetches as u64).saturating_mul(2));
        self.slot = advance_cpu_slots(self.slot, fetches);
    }

    /// Plans one CPU request and completion on an idle memory domain.
    pub fn plan_direct_cpu_transaction(
        &self,
        submission_time: SimTime,
    ) -> Option<CrimeDirectMemoryPlan> {
        if !self.stable_fetch_ready()
            || (self.refresh_delay != SimDuration::ZERO
                && submission_time >= self.next_refresh_time)
        {
            return None;
        }
        let mut clock = self.clock;
        Some(CrimeDirectMemoryPlan {
            epoch: self.epoch,
            clock: self.clock.projection(),
            slot: self.slot,
            submission_time,
            next_refresh_time: self.next_refresh_time,
            service_delay: clock.next_cycle(),
            completion_delay: clock.next_cycle(),
        })
    }

    /// Starts a previously planned direct transaction through normal bus semantics.
    pub fn begin_direct_cpu_transaction(
        &mut self,
        plan: CrimeDirectMemoryPlan,
        transaction: CrimeMemoryTransaction,
    ) -> CrimeMemoryTransaction {
        assert!(self.direct_plan_valid(plan));
        assert_eq!(transaction.client, CrimeMemoryClient::Cpu);
        assert_eq!(transaction.time, plan.submission_time);
        assert_eq!(
            self.route(transaction.clone()),
            CrimeBusDisposition::QueuedAndNeedsService {
                delay: plan.service_delay,
                epoch: plan.epoch,
            }
        );
        self.handle_event(CrimeMemoryBusEvent::Service { epoch: plan.epoch });
        match self.poll() {
            CrimeBusAction::Deliver {
                target,
                transaction: delivered,
            } => {
                assert_eq!(target, self.target);
                assert_eq!(delivered, transaction);
                delivered
            }
            action => panic!("direct CRIME memory service produced {action:?}"),
        }
    }

    /// Completes a direct SDRAM response through normal bus semantics.
    pub fn finish_direct_cpu_transaction(
        &mut self,
        plan: CrimeDirectMemoryPlan,
        completion: CrimeMemoryCompletion,
    ) -> (ComponentId, CrimeMemoryCompletion) {
        self.accept_device_completion(completion);
        match self.poll() {
            CrimeBusAction::ScheduleService { delay } => {
                assert_eq!(delay, plan.completion_delay);
            }
            action => panic!("direct CRIME memory completion produced {action:?}"),
        }
        self.handle_event(CrimeMemoryBusEvent::Complete { epoch: plan.epoch });
        match self.poll() {
            CrimeBusAction::Complete {
                controller,
                completion,
            } => (controller, completion),
            action => panic!("direct CRIME memory publication produced {action:?}"),
        }
    }

    /// Handles one scheduled bus event.
    pub fn handle_event(&mut self, event: CrimeMemoryBusEvent) {
        let event_epoch = match event {
            CrimeMemoryBusEvent::Service { epoch }
            | CrimeMemoryBusEvent::Complete { epoch }
            | CrimeMemoryBusEvent::Refresh { epoch } => epoch,
        };
        if event_epoch != self.epoch {
            return;
        }
        match event {
            CrimeMemoryBusEvent::Service { .. } => self.service(),
            CrimeMemoryBusEvent::Complete { .. } => self.publish_completion(),
            CrimeMemoryBusEvent::Refresh { .. } => {
                self.refresh_debt = self.refresh_debt.saturating_add(1);
                if !self.service_scheduled
                    && self.in_flight.is_none()
                    && self.pending_completion.is_none()
                {
                    self.service_scheduled = true;
                    self.actions.push_back(CrimeBusAction::ScheduleService {
                        delay: self.clock.next_cycle(),
                    });
                }
            }
        }
    }

    /// Accepts the immediate response of the current SDRAM delivery.
    pub fn accept_device_completion(&mut self, completion: CrimeMemoryCompletion) {
        let Some(in_flight) = self.in_flight else {
            return;
        };
        if in_flight.transaction_id != completion.id {
            return;
        }
        self.in_flight = None;
        self.pending_completion = Some(PendingCompletion {
            controller: in_flight.controller,
            completion,
        });
        self.actions.push_back(CrimeBusAction::ScheduleService {
            delay: self.clock.next_cycle(),
        });
    }

    /// Polls one bus delivery, completion, or scheduling action.
    pub fn poll(&mut self) -> CrimeBusAction<CrimeMemoryTransaction, CrimeMemoryCompletion> {
        self.actions.pop_front().unwrap_or(CrimeBusAction::Idle)
    }

    /// Returns the active reset epoch.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Returns the event corresponding to the most recent scheduling action.
    pub const fn next_scheduled_event(&self) -> CrimeMemoryBusEvent {
        if self.pending_completion.is_some() {
            CrimeMemoryBusEvent::Complete { epoch: self.epoch }
        } else {
            CrimeMemoryBusEvent::Service { epoch: self.epoch }
        }
    }

    fn reset_state(&mut self, now: SimTime) {
        self.epoch = self.epoch.wrapping_add(1);
        self.clock.reset();
        self.slot = 0;
        self.service_scheduled = false;
        self.next_refresh_time = now
            .checked_add(self.refresh_delay)
            .unwrap_or(SimTime::new(u64::MAX));
        self.refresh_debt = 0;
        self.gbe.clear();
        self.mace.clear();
        self.vice.clear();
        self.render.clear();
        self.cpu.clear();
        self.in_flight = None;
        self.pending_completion = None;
        self.actions.clear();
    }

    fn direct_plan_valid(&self, plan: CrimeDirectMemoryPlan) -> bool {
        self.stable_fetch_ready()
            && self.epoch == plan.epoch
            && self.clock.projection() == plan.clock
            && self.slot == plan.slot
            && self.next_refresh_time == plan.next_refresh_time
            && (self.refresh_delay == SimDuration::ZERO
                || plan.submission_time < self.next_refresh_time)
    }

    fn service(&mut self) {
        self.service_scheduled = false;
        if self.in_flight.is_some() || self.pending_completion.is_some() {
            return;
        }
        let Some(transaction) = self.select_next() else {
            return;
        };
        let controller = transaction.controller;
        let transaction_id = transaction.id;
        self.in_flight = Some(InFlightMemory {
            controller,
            transaction_id,
        });
        self.actions.push_back(CrimeBusAction::Deliver {
            target: self.target,
            transaction,
        });
    }

    fn publish_completion(&mut self) {
        let Some(pending) = self.pending_completion.take() else {
            return;
        };
        self.actions.push_back(CrimeBusAction::Complete {
            controller: pending.controller,
            completion: pending.completion,
        });
        if self.has_pending() {
            self.service_scheduled = true;
            self.actions.push_back(CrimeBusAction::ScheduleService {
                delay: self.clock.next_cycle(),
            });
        }
    }

    fn select_next(&mut self) -> Option<CrimeMemoryTransaction> {
        for _ in 0..64 {
            let slot = self.slot;
            self.slot = self.slot.wrapping_add(1) & 63;
            match slot_client(slot) {
                Some(CrimeMemoryClient::Gbe) if !self.gbe.is_empty() => {
                    return self.gbe.pop_front();
                }
                Some(CrimeMemoryClient::Mace) if !self.mace.is_empty() => {
                    return self.mace.pop_front();
                }
                Some(CrimeMemoryClient::Vice) if !self.vice.is_empty() => {
                    return self.vice.pop_front();
                }
                Some(CrimeMemoryClient::Render) if !self.render.is_empty() => {
                    return self.render.pop_front();
                }
                Some(CrimeMemoryClient::Cpu) if !self.cpu.is_empty() => {
                    return self.cpu.pop_front();
                }
                None if slot == 31 && self.refresh_debt != 0 => {
                    self.refresh_debt -= 1;
                }
                _ => continue,
            }
            if self.has_pending() {
                self.service_scheduled = true;
                self.actions.push_back(CrimeBusAction::ScheduleService {
                    delay: self.clock.next_cycle(),
                });
            }
            return None;
        }
        None
    }

    fn has_pending(&self) -> bool {
        self.refresh_debt != 0
            || !self.gbe.is_empty()
            || !self.mace.is_empty()
            || !self.vice.is_empty()
            || !self.render.is_empty()
            || !self.cpu.is_empty()
    }

    fn queue_mut(&mut self, client: CrimeMemoryClient) -> &mut VecDeque<CrimeMemoryTransaction> {
        match client {
            CrimeMemoryClient::Gbe => &mut self.gbe,
            CrimeMemoryClient::Mace => &mut self.mace,
            CrimeMemoryClient::Vice => &mut self.vice,
            CrimeMemoryClient::Render => &mut self.render,
            CrimeMemoryClient::Cpu => &mut self.cpu,
        }
    }
}

impl Component for CrimeMemoryBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.reset_state(SimTime::ZERO);
    }
}

impl BusRole<CrimeMemoryTransaction> for CrimeMemoryBus {
    type Response = CrimeBusDisposition;

    fn route(&mut self, transaction: CrimeMemoryTransaction) -> Self::Response {
        self.account_refreshes(transaction.time);
        let client = transaction.client;
        self.queue_mut(client).push_back(transaction);
        if self.service_scheduled || self.in_flight.is_some() || self.pending_completion.is_some() {
            CrimeBusDisposition::Queued
        } else {
            self.service_scheduled = true;
            CrimeBusDisposition::QueuedAndNeedsService {
                delay: self.clock.next_cycle(),
                epoch: self.epoch,
            }
        }
    }
}

impl CrimeMemoryBus {
    fn account_refreshes(&mut self, now: SimTime) {
        if now < self.next_refresh_time || self.refresh_delay == SimDuration::ZERO {
            return;
        }
        let elapsed = now.get() - self.next_refresh_time.get();
        let periods = elapsed / self.refresh_delay.get() + 1;
        self.refresh_debt = self.refresh_debt.saturating_add(periods);
        let advance = periods.saturating_mul(self.refresh_delay.get());
        self.next_refresh_time = SimTime::new(self.next_refresh_time.get().saturating_add(advance));
    }
}

const fn slot_client(slot: u8) -> Option<CrimeMemoryClient> {
    if slot & 1 == 0 {
        Some(CrimeMemoryClient::Gbe)
    } else if slot & 3 == 1 {
        Some(CrimeMemoryClient::Mace)
    } else if slot & 7 == 3 {
        Some(CrimeMemoryClient::Vice)
    } else if slot & 15 == 7 {
        Some(CrimeMemoryClient::Render)
    } else if slot & 31 == 15 {
        Some(CrimeMemoryClient::Cpu)
    } else {
        None
    }
}

const fn advance_cpu_slots(slot: u8, fetches: usize) -> u8 {
    if fetches == 0 {
        return slot;
    }
    let distance = 15_u8.wrapping_sub(slot & 31) & 31;
    let after_first = slot.wrapping_add(distance).wrapping_add(1) & 63;
    if (fetches - 1) & 1 == 0 {
        after_first
    } else {
        after_first ^ 32
    }
}

#[cfg(test)]
mod tests;
