//! CRIME CMI and CGI communication domains.

use std::collections::{BTreeMap, VecDeque};

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;

use super::clock::CrimeClock;
use super::protocol::{
    CrimeBusAction, CrimeBusDisposition, CrimeCgiCompletion, CrimeCgiTransaction,
    CrimeCmiCompletion, CrimeCmiTransaction, CrimeLinkOperation, CrimeTransactionId,
};

/// Scheduled CMI bus event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeCmiBusEvent {
    /// Delivers the next request.
    Service {
        /// Reset epoch.
        epoch: u64,
    },

    /// Publishes the current completion.
    Complete {
        /// Reset epoch.
        epoch: u64,
    },
}

/// Scheduled CGI bus event.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum CrimeCgiBusEvent {
    /// Delivers the next request.
    Service {
        /// Reset epoch.
        epoch: u64,
    },

    /// Publishes the current completion.
    Complete {
        /// Reset epoch.
        epoch: u64,

        /// Component that accepted the matching request.
        target: ComponentId,

        /// Transaction identifier scoped by `target`.
        id: CrimeTransactionId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
struct LinkBus<T, C> {
    clock: CrimeClock,
    epoch: u64,
    service_scheduled: bool,
    queue: VecDeque<T>,
    in_flight: Option<T>,
    pending_completion: Option<(ComponentId, C)>,
    actions: VecDeque<CrimeBusAction<T, C>>,
}

impl<T, C> LinkBus<T, C>
where
    T: Clone,
{
    fn new(timebase_hz: u64) -> Self {
        Self {
            clock: CrimeClock::new(timebase_hz),
            epoch: 0,
            service_scheduled: false,
            queue: VecDeque::new(),
            in_flight: None,
            pending_completion: None,
            actions: VecDeque::new(),
        }
    }

    fn reset(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.clock.reset();
        self.service_scheduled = false;
        self.queue.clear();
        self.in_flight = None;
        self.pending_completion = None;
        self.actions.clear();
    }

    fn route(&mut self, transaction: T) -> CrimeBusDisposition {
        self.queue.push_back(transaction);
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

    fn service(&mut self, target: impl FnOnce(&T) -> ComponentId) {
        self.service_scheduled = false;
        if self.in_flight.is_some() || self.pending_completion.is_some() {
            return;
        }
        let Some(transaction) = self.queue.pop_front() else {
            return;
        };
        let destination = target(&transaction);
        self.in_flight = Some(transaction.clone());
        self.actions.push_back(CrimeBusAction::Deliver {
            target: destination,
            transaction,
        });
    }

    fn accept_completion(
        &mut self,
        completion: C,
        matches: impl FnOnce(&T, &C) -> bool,
        controller: impl FnOnce(&T) -> ComponentId,
    ) {
        let Some(transaction) = self.in_flight.take() else {
            return;
        };
        if !matches(&transaction, &completion) {
            self.in_flight = Some(transaction);
            return;
        }
        self.pending_completion = Some((controller(&transaction), completion));
        self.actions.push_back(CrimeBusAction::ScheduleService {
            delay: self.clock.next_cycle(),
        });
    }

    fn publish_completion(&mut self) {
        let Some((controller, completion)) = self.pending_completion.take() else {
            return;
        };
        self.actions.push_back(CrimeBusAction::Complete {
            controller,
            completion,
        });
        if !self.queue.is_empty() {
            self.service_scheduled = true;
            self.actions.push_back(CrimeBusAction::ScheduleService {
                delay: self.clock.next_cycle(),
            });
        }
    }

    fn poll(&mut self) -> CrimeBusAction<T, C> {
        self.actions.pop_front().unwrap_or(CrimeBusAction::Idle)
    }
}

/// CRIME-to-MACE CMI bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeCmiBus {
    id: ComponentId,
    name: String,
    inner: LinkBus<CrimeCmiTransaction, CrimeCmiCompletion>,
}

se_core::component_state!(CrimeCmiBusState, CrimeCmiBus);

impl CrimeCmiBus {
    /// Creates a CMI domain.
    pub fn new(id: ComponentId, name: impl Into<String>, timebase_hz: u64) -> Self {
        Self {
            id,
            name: name.into(),
            inner: LinkBus::new(timebase_hz),
        }
    }

    /// Handles a scheduled CMI event.
    pub fn handle_event(&mut self, event: CrimeCmiBusEvent) {
        let epoch = match event {
            CrimeCmiBusEvent::Service { epoch } | CrimeCmiBusEvent::Complete { epoch } => epoch,
        };
        if epoch != self.inner.epoch {
            return;
        }
        match event {
            CrimeCmiBusEvent::Service { .. } => {
                self.inner.service(|transaction| transaction.target)
            }
            CrimeCmiBusEvent::Complete { .. } => self.inner.publish_completion(),
        }
    }

    /// Accepts a CMI target response.
    pub fn accept_device_completion(&mut self, completion: CrimeCmiCompletion) {
        self.inner.accept_completion(
            completion,
            |transaction, completion| transaction.id == completion.id,
            |transaction| transaction.controller,
        );
    }

    /// Polls one CMI action.
    pub fn poll(&mut self) -> CrimeBusAction<CrimeCmiTransaction, CrimeCmiCompletion> {
        self.inner.poll()
    }

    /// Returns the active reset epoch.
    pub const fn epoch(&self) -> u64 {
        self.inner.epoch
    }

    /// Returns the event corresponding to the latest scheduling action.
    pub const fn next_scheduled_event(&self) -> CrimeCmiBusEvent {
        if self.inner.pending_completion.is_some() {
            CrimeCmiBusEvent::Complete {
                epoch: self.inner.epoch,
            }
        } else {
            CrimeCmiBusEvent::Service {
                epoch: self.inner.epoch,
            }
        }
    }

    /// Cancels all CMI work and advances its epoch.
    pub fn hard_reset(&mut self) {
        self.inner.reset();
    }

    /// Returns whether a stable fetch may bypass this otherwise idle link.
    pub fn stable_fetch_ready(&self) -> bool {
        !self.inner.service_scheduled
            && self.inner.queue.is_empty()
            && self.inner.in_flight.is_none()
            && self.inner.pending_completion.is_none()
            && self.inner.actions.is_empty()
    }

    /// Returns the exact current fractional clock used by stable fetches.
    pub fn stable_fetch_clock(&self) -> Option<se_core::scheduler::FractionalClockProjection> {
        self.stable_fetch_ready()
            .then(|| self.inner.clock.projection())
    }

    /// Plans idle request/completion cycle pairs without changing link state.
    pub fn plan_stable_fetches(
        &self,
        output: &mut [se_core::scheduler::SimDuration],
    ) -> Option<()> {
        if !self.stable_fetch_ready() {
            return None;
        }
        let mut clock = self.inner.clock;
        for delay in output {
            *delay = se_core::scheduler::SimDuration::new(
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
            .inner
            .clock
            .advance_cycles((fetches as u64).saturating_mul(2));
    }
}

impl Component for CrimeCmiBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

impl BusRole<CrimeCmiTransaction> for CrimeCmiBus {
    type Response = CrimeBusDisposition;

    fn route(&mut self, transaction: CrimeCmiTransaction) -> Self::Response {
        self.inner.route(transaction)
    }
}

/// CRIME-to-GBE CGI bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct CrimeCgiBus {
    id: ComponentId,
    name: String,
    clock: CrimeClock,
    epoch: u64,
    service_scheduled: bool,
    queue: VecDeque<CrimeCgiTransaction>,
    in_flight: BTreeMap<(ComponentId, CrimeTransactionId), CrimeCgiTransaction>,
    pending_completions:
        BTreeMap<(ComponentId, CrimeTransactionId), (ComponentId, CrimeCgiCompletion)>,
    scheduled_events: VecDeque<CrimeCgiBusEvent>,
    actions: VecDeque<CrimeBusAction<CrimeCgiTransaction, CrimeCgiCompletion>>,
}

se_core::component_state!(CrimeCgiBusState, CrimeCgiBus);

impl CrimeCgiBus {
    /// Creates a CGI domain.
    pub fn new(id: ComponentId, name: impl Into<String>, timebase_hz: u64) -> Self {
        Self {
            id,
            name: name.into(),
            clock: CrimeClock::new(timebase_hz),
            epoch: 0,
            service_scheduled: false,
            queue: VecDeque::new(),
            in_flight: BTreeMap::new(),
            pending_completions: BTreeMap::new(),
            scheduled_events: VecDeque::new(),
            actions: VecDeque::new(),
        }
    }

    /// Handles a scheduled CGI event.
    pub fn handle_event(&mut self, event: CrimeCgiBusEvent) {
        let epoch = match event {
            CrimeCgiBusEvent::Service { epoch } | CrimeCgiBusEvent::Complete { epoch, .. } => epoch,
        };
        if epoch != self.epoch {
            return;
        }
        match event {
            CrimeCgiBusEvent::Service { .. } => self.service(),
            CrimeCgiBusEvent::Complete { target, id, .. } => self.publish_completion(target, id),
        }
    }

    /// Accepts a CGI target response.
    pub fn accept_device_completion(
        &mut self,
        target: ComponentId,
        completion: CrimeCgiCompletion,
    ) {
        let key = (target, completion.id);
        let Some(transaction) = self.in_flight.remove(&key) else {
            return;
        };
        let controller = transaction.controller;
        let delay = self
            .clock
            .advance_cycles(cgi_completion_cycles(&transaction));
        self.pending_completions
            .insert(key, (controller, completion));
        let event = CrimeCgiBusEvent::Complete {
            epoch: self.epoch,
            target,
            id: transaction.id,
        };
        self.scheduled_events.push_back(event);
        self.actions
            .push_back(CrimeBusAction::ScheduleService { delay });
    }

    /// Polls one CGI action.
    pub fn poll(&mut self) -> CrimeBusAction<CrimeCgiTransaction, CrimeCgiCompletion> {
        self.actions.pop_front().unwrap_or(CrimeBusAction::Idle)
    }

    /// Returns the active reset epoch.
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    /// Removes the event corresponding to the oldest scheduling action.
    pub fn next_scheduled_event(&mut self) -> Option<CrimeCgiBusEvent> {
        self.scheduled_events.pop_front()
    }

    /// Cancels all CGI work and advances its epoch.
    pub fn hard_reset(&mut self) {
        self.reset_state();
    }

    fn reset_state(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.clock.reset();
        self.service_scheduled = false;
        self.queue.clear();
        self.in_flight.clear();
        self.pending_completions.clear();
        self.scheduled_events.clear();
        self.actions.clear();
    }

    fn schedule_service(&mut self) -> CrimeBusDisposition {
        if self.service_scheduled {
            return CrimeBusDisposition::Queued;
        }
        let Some(transaction) = self.queue.front() else {
            return CrimeBusDisposition::Queued;
        };
        self.service_scheduled = true;
        let delay = self.clock.advance_cycles(cgi_request_cycles(transaction));
        CrimeBusDisposition::QueuedAndNeedsService {
            delay,
            epoch: self.epoch,
        }
    }

    fn service(&mut self) {
        self.service_scheduled = false;
        let Some(transaction) = self.queue.pop_front() else {
            return;
        };
        let key = (transaction.target, transaction.id);
        if self.in_flight.contains_key(&key) {
            self.queue.push_front(transaction);
            return;
        }
        let target = transaction.target;
        self.in_flight.insert(key, transaction.clone());
        self.actions.push_back(CrimeBusAction::Deliver {
            target,
            transaction,
        });
        if !self.queue.is_empty() {
            self.service_scheduled = true;
            let delay = self.clock.advance_cycles(cgi_request_cycles(
                self.queue
                    .front()
                    .expect("a nonempty CGI queue must have a front transaction"),
            ));
            self.scheduled_events
                .push_back(CrimeCgiBusEvent::Service { epoch: self.epoch });
            self.actions
                .push_back(CrimeBusAction::ScheduleService { delay });
        }
    }

    fn publish_completion(&mut self, target: ComponentId, id: CrimeTransactionId) {
        let Some((controller, completion)) = self.pending_completions.remove(&(target, id)) else {
            return;
        };
        self.actions.push_back(CrimeBusAction::Complete {
            controller,
            completion,
        });
    }
}

impl Component for CrimeCgiBus {
    fn id(&self) -> ComponentId {
        self.id
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn reset(&mut self) {
        self.reset_state();
    }
}

impl BusRole<CrimeCgiTransaction> for CrimeCgiBus {
    type Response = CrimeBusDisposition;

    fn route(&mut self, transaction: CrimeCgiTransaction) -> Self::Response {
        self.queue.push_back(transaction);
        self.schedule_service()
    }
}

fn cgi_request_cycles(transaction: &CrimeCgiTransaction) -> u64 {
    match &transaction.operation {
        CrimeLinkOperation::Dma(request)
            if matches!(
                request.transfer.view(),
                crate::chipset::crime::protocol::CrimeTransferView::Write { .. }
            ) =>
        {
            1 + (request.transfer.length() as u64).div_ceil(16)
        }
        CrimeLinkOperation::Pio(_)
        | CrimeLinkOperation::Dma(_)
        | CrimeLinkOperation::InterruptPost(_) => 1,
    }
}

fn cgi_completion_cycles(transaction: &CrimeCgiTransaction) -> u64 {
    match &transaction.operation {
        CrimeLinkOperation::Dma(request)
            if matches!(
                request.transfer.view(),
                crate::chipset::crime::protocol::CrimeTransferView::Read { .. }
            ) =>
        {
            1 + (request.transfer.length() as u64).div_ceil(16)
        }
        CrimeLinkOperation::Pio(_)
        | CrimeLinkOperation::Dma(_)
        | CrimeLinkOperation::InterruptPost(_) => 1,
    }
}

#[cfg(test)]
mod tests;
