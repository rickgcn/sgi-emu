//! CRIME CMI and CGI communication domains.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;

use super::clock::CrimeClock;
use super::protocol::{
    CrimeBusAction, CrimeBusDisposition, CrimeCgiCompletion, CrimeCgiTransaction,
    CrimeCmiCompletion, CrimeCmiTransaction,
};

/// Scheduled CMI bus event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeCmiBus {
    id: ComponentId,
    name: String,
    inner: LinkBus<CrimeCmiTransaction, CrimeCmiCompletion>,
}

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
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CrimeCgiBus {
    id: ComponentId,
    name: String,
    inner: LinkBus<CrimeCgiTransaction, CrimeCgiCompletion>,
}

impl CrimeCgiBus {
    /// Creates a CGI domain.
    pub fn new(id: ComponentId, name: impl Into<String>, timebase_hz: u64) -> Self {
        Self {
            id,
            name: name.into(),
            inner: LinkBus::new(timebase_hz),
        }
    }

    /// Handles a scheduled CGI event.
    pub fn handle_event(&mut self, event: CrimeCgiBusEvent) {
        let epoch = match event {
            CrimeCgiBusEvent::Service { epoch } | CrimeCgiBusEvent::Complete { epoch } => epoch,
        };
        if epoch != self.inner.epoch {
            return;
        }
        match event {
            CrimeCgiBusEvent::Service { .. } => {
                self.inner.service(|transaction| transaction.target)
            }
            CrimeCgiBusEvent::Complete { .. } => self.inner.publish_completion(),
        }
    }

    /// Accepts a CGI target response.
    pub fn accept_device_completion(&mut self, completion: CrimeCgiCompletion) {
        self.inner.accept_completion(
            completion,
            |transaction, completion| transaction.id == completion.id,
            |transaction| transaction.controller,
        );
    }

    /// Polls one CGI action.
    pub fn poll(&mut self) -> CrimeBusAction<CrimeCgiTransaction, CrimeCgiCompletion> {
        self.inner.poll()
    }

    /// Returns the active reset epoch.
    pub const fn epoch(&self) -> u64 {
        self.inner.epoch
    }

    /// Returns the event corresponding to the latest scheduling action.
    pub const fn next_scheduled_event(&self) -> CrimeCgiBusEvent {
        if self.inner.pending_completion.is_some() {
            CrimeCgiBusEvent::Complete {
                epoch: self.inner.epoch,
            }
        } else {
            CrimeCgiBusEvent::Service {
                epoch: self.inner.epoch,
            }
        }
    }

    /// Cancels all CGI work and advances its epoch.
    pub fn hard_reset(&mut self) {
        self.inner.reset();
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
        self.inner.reset();
    }
}

impl BusRole<CrimeCgiTransaction> for CrimeCgiBus {
    type Response = CrimeBusDisposition;

    fn route(&mut self, transaction: CrimeCgiTransaction) -> Self::Response {
        self.inner.route(transaction)
    }
}

#[cfg(test)]
mod tests;
