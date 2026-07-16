//! PCI 2.1 transaction protocol and deterministic arbitration domain.

use std::collections::{BTreeMap, VecDeque};

use se_core::component::{Component, ComponentId};
use se_core::role::{BusDeviceRole, BusRole};
use se_core::scheduler::SimDuration;

/// PCI command transported by the bus model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PciCommand {
    IoRead,
    IoWrite,
    MemoryRead,
    MemoryWrite,
    MemoryReadLine,
    MemoryReadMultiple,
    MemoryWriteInvalidate,
    ConfigurationRead,
    ConfigurationWrite,
}

/// PCI configuration-space selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PciConfigurationAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub register: u8,
}

/// PCI transaction.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PciTransaction {
    pub id: u128,
    pub controller: ComponentId,
    pub target: ComponentId,
    pub command: PciCommand,
    pub address: u64,
    pub configuration: Option<PciConfigurationAddress>,
    pub data: Vec<u8>,
    pub byte_enable: Vec<bool>,
}

/// PCI completion status.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PciStatus {
    Complete,
    Retry,
    MasterAbort,
    TargetAbort,
    ParityError,
}

/// PCI completion.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PciCompletion {
    pub id: u128,
    pub status: PciStatus,
    pub data: Vec<u8>,
}

/// Result of routing a PCI request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PciBusDisposition {
    Queued,
    QueuedAndNeedsService { delay: SimDuration },
}

/// Action emitted by the PCI bus.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum PciBusAction {
    Deliver {
        target: ComponentId,
        transaction: PciTransaction,
    },
    Complete {
        controller: ComponentId,
        completion: PciCompletion,
    },
    Idle,
}

/// Deterministic PCI arbiter with fixed-priority and round-robin clients.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PciBus {
    id: ComponentId,
    name: String,
    cycle: SimDuration,
    fixed_priority: Vec<ComponentId>,
    queues: BTreeMap<ComponentId, VecDeque<PciTransaction>>,
    round_robin: Vec<ComponentId>,
    cursor: usize,
    in_flight: Option<PciTransaction>,
    actions: VecDeque<PciBusAction>,
}

se_core::component_state!(PciBusState, PciBus);

/// Protocol-correct PCI configuration endpoint with no device engine.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct PciConfigurationEndpoint {
    id: ComponentId,
    name: String,
    #[serde(with = "crate::common::serde_array")]
    configuration: [u8; 256],
}

se_core::component_state!(PciConfigurationEndpointState, PciConfigurationEndpoint);

impl PciConfigurationEndpoint {
    /// Creates an enumerable PCI function.
    pub fn new(
        id: ComponentId,
        name: impl Into<String>,
        vendor_id: u16,
        device_id: u16,
        class_code: u32,
        revision: u8,
    ) -> Self {
        let mut configuration = [0; 256];
        configuration[0..2].copy_from_slice(&vendor_id.to_le_bytes());
        configuration[2..4].copy_from_slice(&device_id.to_le_bytes());
        configuration[8] = revision;
        configuration[9] = class_code as u8;
        configuration[10] = (class_code >> 8) as u8;
        configuration[11] = (class_code >> 16) as u8;
        configuration[14] = 0;
        Self {
            id,
            name: name.into(),
            configuration,
        }
    }
}

impl Component for PciConfigurationEndpoint {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        self.configuration[4..8].fill(0);
        self.configuration[12..16].fill(0);
        self.configuration[16..40].fill(0);
    }
}

impl BusDeviceRole<PciTransaction> for PciConfigurationEndpoint {
    type Response = PciCompletion;

    fn accept(&mut self, transaction: PciTransaction) -> Self::Response {
        let mut completion = PciCompletion {
            id: transaction.id,
            status: PciStatus::Complete,
            data: vec![],
        };
        let Some(configuration) = transaction.configuration else {
            completion.status = PciStatus::TargetAbort;
            return completion;
        };
        if configuration.bus != 0 || configuration.function != 0 {
            completion.status = PciStatus::MasterAbort;
            return completion;
        }
        let start = usize::from(configuration.register) + (transaction.address as usize & 3);
        let length = transaction.data.len();
        let Some(end) = start
            .checked_add(length)
            .filter(|&end| end <= self.configuration.len())
        else {
            completion.status = PciStatus::TargetAbort;
            return completion;
        };
        match transaction.command {
            PciCommand::ConfigurationRead => {
                completion
                    .data
                    .extend_from_slice(&self.configuration[start..end]);
            }
            PciCommand::ConfigurationWrite => {
                for (index, (&value, &enabled)) in transaction
                    .data
                    .iter()
                    .zip(&transaction.byte_enable)
                    .enumerate()
                {
                    if enabled && start + index >= 4 {
                        self.configuration[start + index] = value;
                    }
                }
            }
            _ => completion.status = PciStatus::TargetAbort,
        }
        completion
    }
}

impl PciBus {
    /// Creates an empty PCI domain.
    pub fn new(id: ComponentId, name: impl Into<String>, cycle: SimDuration) -> Self {
        Self {
            id,
            name: name.into(),
            cycle,
            fixed_priority: Vec::new(),
            queues: BTreeMap::new(),
            round_robin: Vec::new(),
            cursor: 0,
            in_flight: None,
            actions: VecDeque::new(),
        }
    }

    /// Replaces the high-priority controller order.
    pub fn set_fixed_priority(&mut self, controllers: Vec<ComponentId>) {
        self.fixed_priority = controllers;
    }

    /// Advances one arbitration opportunity.
    pub fn service(&mut self) {
        if self.in_flight.is_some() {
            return;
        }
        let controller = self
            .fixed_priority
            .iter()
            .copied()
            .find(|controller| {
                self.queues
                    .get(controller)
                    .is_some_and(|queue| !queue.is_empty())
            })
            .or_else(|| self.next_round_robin());
        let Some(controller) = controller else {
            return;
        };
        let transaction = self
            .queues
            .get_mut(&controller)
            .and_then(VecDeque::pop_front)
            .expect("the selected PCI controller must have queued work");
        let target = transaction.target;
        self.in_flight = Some(transaction.clone());
        self.actions.push_back(PciBusAction::Deliver {
            target,
            transaction,
        });
    }

    /// Accepts the response for the active request.
    pub fn complete(&mut self, completion: PciCompletion) -> bool {
        let Some(transaction) = self.in_flight.take() else {
            return false;
        };
        if transaction.id != completion.id {
            self.in_flight = Some(transaction);
            return false;
        }
        self.actions.push_back(PciBusAction::Complete {
            controller: transaction.controller,
            completion,
        });
        true
    }

    /// Polls one bus action.
    pub fn poll(&mut self) -> PciBusAction {
        self.actions.pop_front().unwrap_or(PciBusAction::Idle)
    }

    /// Returns the visible delay for one PCI arbitration cycle.
    pub const fn cycle(&self) -> SimDuration {
        self.cycle
    }

    fn next_round_robin(&mut self) -> Option<ComponentId> {
        if self.round_robin.is_empty() {
            return None;
        }
        for _ in 0..self.round_robin.len() {
            let index = self.cursor % self.round_robin.len();
            self.cursor = (index + 1) % self.round_robin.len();
            let controller = self.round_robin[index];
            if self
                .queues
                .get(&controller)
                .is_some_and(|queue| !queue.is_empty())
            {
                return Some(controller);
            }
        }
        None
    }

    fn clear(&mut self) {
        self.queues.clear();
        self.round_robin.clear();
        self.cursor = 0;
        self.in_flight = None;
        self.actions.clear();
    }
}

impl Component for PciBus {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        self.clear();
    }
}

impl BusRole<PciTransaction> for PciBus {
    type Response = PciBusDisposition;

    fn route(&mut self, transaction: PciTransaction) -> Self::Response {
        let idle = self.in_flight.is_none() && self.queues.values().all(VecDeque::is_empty);
        if !self.queues.contains_key(&transaction.controller) {
            self.round_robin.push(transaction.controller);
        }
        self.queues
            .entry(transaction.controller)
            .or_default()
            .push_back(transaction);
        if idle {
            PciBusDisposition::QueuedAndNeedsService { delay: self.cycle }
        } else {
            PciBusDisposition::Queued
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use se_core::role::BusRole;

    #[test]
    fn fixed_priority_precedes_round_robin() {
        let a = ComponentId::new(1);
        let b = ComponentId::new(2);
        let target = ComponentId::new(3);
        let mut bus = PciBus::new(ComponentId::new(4), "PCI", SimDuration::new(30));
        for (id, controller) in [(1, a), (2, b)] {
            bus.route(PciTransaction {
                id,
                controller,
                target,
                command: PciCommand::MemoryRead,
                address: 0,
                configuration: None,
                data: vec![],
                byte_enable: vec![true; 4],
            });
        }
        bus.set_fixed_priority(vec![b]);
        bus.service();
        assert!(matches!(
            bus.poll(),
            PciBusAction::Deliver {
                transaction: PciTransaction { id: 2, .. },
                ..
            }
        ));
    }

    #[test]
    fn configuration_endpoint_reports_identity_in_pci_byte_order() {
        let mut endpoint =
            PciConfigurationEndpoint::new(ComponentId::new(1), "SCSI", 0x9004, 0x8078, 0x010000, 0);
        let completion = endpoint.accept(PciTransaction {
            id: 1,
            controller: ComponentId::new(2),
            target: ComponentId::new(1),
            command: PciCommand::ConfigurationRead,
            address: 0,
            configuration: Some(PciConfigurationAddress {
                bus: 0,
                device: 0,
                function: 0,
                register: 0,
            }),
            data: vec![0; 4],
            byte_enable: vec![true; 4],
        });
        assert_eq!(completion.status, PciStatus::Complete);
        assert_eq!(completion.data, [0x04, 0x90, 0x78, 0x80]);
    }
}
