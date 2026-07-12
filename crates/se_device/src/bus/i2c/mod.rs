//! Deterministic I2C message protocol.

use std::collections::VecDeque;

use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;
use se_core::scheduler::SimDuration;

/// I2C bus rate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum I2cRate {
    Standard100Khz,
    Fast400Khz,
}

/// Atomic I2C message.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I2cTransaction {
    pub id: u128,
    pub controller: ComponentId,
    pub target: ComponentId,
    pub address: u8,
    pub read_length: u16,
    pub write_data: Vec<u8>,
    pub rate: I2cRate,
}

/// I2C completion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum I2cCompletion {
    Ack { id: u128, data: Vec<u8> },
    Nack { id: u128 },
    ArbitrationLost { id: u128 },
    BusError { id: u128 },
}

/// I2C bus action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum I2cBusAction {
    Deliver {
        target: ComponentId,
        transaction: I2cTransaction,
    },
    Complete {
        controller: ComponentId,
        completion: I2cCompletion,
    },
    Idle,
}

/// Serialized I2C party-line bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct I2cBus {
    id: ComponentId,
    name: String,
    queue: VecDeque<I2cTransaction>,
    in_flight: Option<I2cTransaction>,
    actions: VecDeque<I2cBusAction>,
}

impl I2cBus {
    pub fn new(id: ComponentId, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
            queue: VecDeque::new(),
            in_flight: None,
            actions: VecDeque::new(),
        }
    }

    /// Delivers the next message if the bus is idle.
    pub fn service(&mut self) {
        if self.in_flight.is_none()
            && let Some(transaction) = self.queue.pop_front()
        {
            let target = transaction.target;
            self.in_flight = Some(transaction.clone());
            self.actions.push_back(I2cBusAction::Deliver {
                target,
                transaction,
            });
        }
    }

    /// Returns an operation duration derived from bits transferred.
    pub fn duration(transaction: &I2cTransaction, timebase_hz: u64) -> SimDuration {
        let bytes = 1 + transaction.write_data.len() + usize::from(transaction.read_length);
        let bits = 2 + bytes * 9;
        let hz = match transaction.rate {
            I2cRate::Standard100Khz => 100_000,
            I2cRate::Fast400Khz => 400_000,
        };
        SimDuration::new(
            (timebase_hz
                .saturating_mul(bits as u64)
                .saturating_add(hz - 1))
                / hz,
        )
    }

    pub fn complete(&mut self, completion: I2cCompletion) -> bool {
        let id = match &completion {
            I2cCompletion::Ack { id, .. }
            | I2cCompletion::Nack { id }
            | I2cCompletion::ArbitrationLost { id }
            | I2cCompletion::BusError { id } => *id,
        };
        let Some(transaction) = self.in_flight.take() else {
            return false;
        };
        if transaction.id != id {
            self.in_flight = Some(transaction);
            return false;
        }
        self.actions.push_back(I2cBusAction::Complete {
            controller: transaction.controller,
            completion,
        });
        true
    }

    pub fn poll(&mut self) -> I2cBusAction {
        self.actions.pop_front().unwrap_or(I2cBusAction::Idle)
    }
}

impl Component for I2cBus {
    fn id(&self) -> ComponentId {
        self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn reset(&mut self) {
        self.queue.clear();
        self.in_flight = None;
        self.actions.clear();
    }
}

impl BusRole<I2cTransaction> for I2cBus {
    type Response = bool;
    fn route(&mut self, transaction: I2cTransaction) -> bool {
        let needs_service = self.in_flight.is_none() && self.queue.is_empty();
        self.queue.push_back(transaction);
        needs_service
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn duration_accounts_for_ack_bits() {
        let transaction = I2cTransaction {
            id: 0,
            controller: ComponentId::new(1),
            target: ComponentId::new(2),
            address: 0x50,
            read_length: 1,
            write_data: vec![0],
            rate: I2cRate::Standard100Khz,
        };
        assert_eq!(
            I2cBus::duration(&transaction, 1_000_000_000),
            SimDuration::new(290_000)
        );
    }
}
