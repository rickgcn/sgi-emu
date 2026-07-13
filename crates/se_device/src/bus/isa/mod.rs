//! Deterministic byte-oriented ISA communication domain.

use core::ops::{Deref, DerefMut};
use std::collections::VecDeque;

use super::transfer::{
    CompactByteEnable, CompactByteEnableView, CompactData, CompactTransfer, CompactTransferView,
};
use se_core::component::{Component, ComponentId};
use se_core::role::BusRole;
use se_core::scheduler::{SimDuration, SimTime};

/// Owned byte payload optimized for common ISA transfers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IsaData(CompactData);

impl IsaData {
    /// Returns whether the payload spilled beyond inline storage.
    pub fn spilled(&self) -> bool {
        self.0.spilled()
    }

    pub(crate) fn into_compact(self) -> CompactData {
        self.0
    }
}

impl Deref for IsaData {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for IsaData {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl AsRef<[u8]> for IsaData {
    fn as_ref(&self) -> &[u8] {
        self
    }
}

impl From<Vec<u8>> for IsaData {
    fn from(value: Vec<u8>) -> Self {
        Self(value.into())
    }
}

impl<const N: usize> From<[u8; N]> for IsaData {
    fn from(value: [u8; N]) -> Self {
        Self(value.into_iter().collect())
    }
}

impl FromIterator<u8> for IsaData {
    fn from_iter<T: IntoIterator<Item = u8>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl PartialEq<Vec<u8>> for IsaData {
    fn eq(&self, other: &Vec<u8>) -> bool {
        self.as_ref() == other.as_slice()
    }
}

impl<const N: usize> PartialEq<[u8; N]> for IsaData {
    fn eq(&self, other: &[u8; N]) -> bool {
        self.as_ref() == other
    }
}

/// Owned byte-enable payload optimized for common ISA transfers.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IsaByteEnable(CompactByteEnable);

impl IsaByteEnable {
    /// Returns whether the payload spilled beyond inline storage.
    pub fn spilled(&self) -> bool {
        self.0.spilled()
    }

    /// Returns the number of represented byte lanes.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Returns whether no byte lanes are represented.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns one enable bit, or `None` when the lane is out of range.
    pub fn is_enabled(&self, index: usize) -> Option<bool> {
        self.0.is_enabled(index)
    }

    /// Iterates over enable bits in ascending lane order.
    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        self.0.iter()
    }
}

impl From<Vec<bool>> for IsaByteEnable {
    fn from(value: Vec<bool>) -> Self {
        Self(value.into_iter().collect())
    }
}

impl<const N: usize> From<[bool; N]> for IsaByteEnable {
    fn from(value: [bool; N]) -> Self {
        Self(value.into_iter().collect())
    }
}

impl FromIterator<bool> for IsaByteEnable {
    fn from_iter<T: IntoIterator<Item = bool>>(iter: T) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl PartialEq<Vec<bool>> for IsaByteEnable {
    fn eq(&self, other: &Vec<bool>) -> bool {
        self.iter().eq(other.iter().copied())
    }
}

impl<const N: usize> PartialEq<[bool; N]> for IsaByteEnable {
    fn eq(&self, other: &[bool; N]) -> bool {
        self.iter().eq(other.iter().copied())
    }
}

/// Correlation identifier for an ISA transaction.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct IsaTransactionId(u128);

impl IsaTransactionId {
    /// Creates an identifier from its raw value.
    pub const fn new(value: u128) -> Self {
        Self(value)
    }

    /// Returns the raw value.
    pub const fn get(self) -> u128 {
        self.0
    }
}

/// Byte-oriented ISA transfer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsaTransfer(CompactTransfer);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Borrowed byte-enable view for an ISA write transfer.
pub struct IsaByteEnableView<'a>(CompactByteEnableView<'a>);

impl<'a> IsaByteEnableView<'a> {
    /// Returns the number of represented byte lanes.
    pub fn len(self) -> usize {
        self.0.len()
    }

    /// Returns whether no byte lanes are represented.
    pub fn is_empty(self) -> bool {
        self.0.is_empty()
    }

    /// Returns one enable bit, or `None` when the lane is out of range.
    pub fn is_enabled(self, index: usize) -> Option<bool> {
        self.0.is_enabled(index)
    }

    /// Iterates over enable bits in ascending lane order.
    pub fn iter(self) -> impl Iterator<Item = bool> + 'a {
        self.0.iter()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Borrowed view of an ISA byte transfer.
pub enum IsaTransferView<'a> {
    /// Read request with a byte length.
    Read {
        /// Requested byte count.
        length: u16,
    },
    /// Write request with independent data and byte-enable lengths.
    Write {
        /// Bytes in ascending address order.
        data: &'a [u8],
        /// Per-byte write enables.
        byte_enable: IsaByteEnableView<'a>,
    },
}

impl IsaTransfer {
    /// Creates a read transfer without write-side storage.
    pub const fn read(length: u16) -> Self {
        Self(CompactTransfer::read(length))
    }

    /// Creates a write transfer while preserving independent payload lengths.
    pub fn write(data: IsaData, byte_enable: IsaByteEnable) -> Self {
        Self(CompactTransfer::write(data.0, byte_enable.0))
    }

    /// Returns the transfer length.
    pub fn length(&self) -> usize {
        self.0.length()
    }

    /// Borrows the strongly typed transfer contents.
    pub fn view(&self) -> IsaTransferView<'_> {
        match self.0.view() {
            CompactTransferView::Read { length } => IsaTransferView::Read { length },
            CompactTransferView::Write { data, byte_enable } => IsaTransferView::Write {
                data,
                byte_enable: IsaByteEnableView(byte_enable),
            },
        }
    }

    pub(crate) fn from_compact(value: CompactTransfer) -> Self {
        Self(value)
    }
}

/// Transaction routed across an ISA bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsaTransaction {
    /// Correlation identifier.
    pub id: IsaTransactionId,
    /// Submission time.
    pub time: SimTime,
    /// Component receiving completion.
    pub controller: ComponentId,
    /// Target component.
    pub target: ComponentId,
    /// Target-local byte address.
    pub address: u32,
    /// Data transfer.
    pub transfer: IsaTransfer,
}

/// Successful ISA response.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IsaCompletionPayload {
    /// Bytes in ascending address order.
    ReadData(IsaData),
    /// A write completed.
    WriteComplete,
}

/// ISA target or routing error.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsaBusError {
    /// No device decoded the address.
    Address,
    /// Width, alignment, or byte enables are invalid.
    Access,
    /// The target is read-only.
    ReadOnly,
    /// The target exists but does not implement the operation.
    Unsupported,
}

/// Completion returned to an ISA controller.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsaCompletion {
    /// Correlation identifier.
    pub id: IsaTransactionId,
    /// Device result.
    pub result: Result<IsaCompletionPayload, IsaBusError>,
}

/// Response returned immediately by an ISA target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IsaDeviceResponse {
    /// The target completed synchronously.
    Complete(IsaCompletion),
    /// The target retained the request.
    Deferred,
}

/// Result of routing an ISA transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsaBusDisposition {
    /// The bus was already active.
    Queued,
    /// The idle bus needs a service event.
    QueuedAndNeedsService {
        delay: SimDuration,
        event: IsaBusEvent,
    },
}

/// Scheduled ISA bus transition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IsaBusEvent {
    /// Delivers a queued request.
    Service { epoch: u64 },
    /// Publishes an accepted completion.
    Complete { epoch: u64 },
}

/// Action emitted by the ISA bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IsaBusAction {
    /// Delivers one transaction.
    Deliver {
        target: ComponentId,
        transaction: IsaTransaction,
    },
    /// Returns a completion.
    Complete {
        controller: ComponentId,
        completion: IsaCompletion,
    },
    /// Requests a future transition.
    Schedule {
        delay: SimDuration,
        event: IsaBusEvent,
    },
    /// No action is ready.
    Idle,
}

/// Single-transaction deterministic ISA bus.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IsaBus {
    id: ComponentId,
    name: String,
    cycle: SimDuration,
    epoch: u64,
    service_scheduled: bool,
    queue: VecDeque<IsaTransaction>,
    in_flight: Option<IsaTransaction>,
    completion: Option<IsaCompletion>,
    actions: VecDeque<IsaBusAction>,
}

impl IsaBus {
    /// Creates an ISA domain with a fixed visible bus-cycle delay.
    pub fn new(id: ComponentId, name: impl Into<String>, cycle: SimDuration) -> Self {
        Self {
            id,
            name: name.into(),
            cycle,
            epoch: 0,
            service_scheduled: false,
            queue: VecDeque::new(),
            in_flight: None,
            completion: None,
            actions: VecDeque::new(),
        }
    }

    /// Handles a scheduled bus transition.
    pub fn handle_event(&mut self, event: IsaBusEvent) {
        let epoch = match event {
            IsaBusEvent::Service { epoch } | IsaBusEvent::Complete { epoch } => epoch,
        };
        if epoch != self.epoch {
            return;
        }
        match event {
            IsaBusEvent::Service { .. } => {
                self.service_scheduled = false;
                if self.in_flight.is_none()
                    && let Some(transaction) = self.queue.pop_front()
                {
                    let target = transaction.target;
                    self.in_flight = Some(transaction.clone());
                    self.actions.push_back(IsaBusAction::Deliver {
                        target,
                        transaction,
                    });
                }
            }
            IsaBusEvent::Complete { .. } => {
                let Some(completion) = self.completion.take() else {
                    return;
                };
                let Some(transaction) = self.in_flight.take() else {
                    return;
                };
                self.actions.push_back(IsaBusAction::Complete {
                    controller: transaction.controller,
                    completion,
                });
                if !self.queue.is_empty() {
                    self.service_scheduled = true;
                    self.actions.push_back(IsaBusAction::Schedule {
                        delay: self.cycle,
                        event: IsaBusEvent::Service { epoch: self.epoch },
                    });
                }
            }
        }
    }

    /// Accepts a target completion for the active transaction.
    pub fn accept_device_completion(&mut self, completion: IsaCompletion) -> bool {
        let Some(transaction) = &self.in_flight else {
            return false;
        };
        if transaction.id != completion.id || self.completion.is_some() {
            return false;
        }
        self.completion = Some(completion);
        self.actions.push_back(IsaBusAction::Schedule {
            delay: self.cycle,
            event: IsaBusEvent::Complete { epoch: self.epoch },
        });
        true
    }

    /// Polls one pending action.
    pub fn poll(&mut self) -> IsaBusAction {
        self.actions.pop_front().unwrap_or(IsaBusAction::Idle)
    }

    /// Cancels all work and advances the reset epoch.
    pub fn hard_reset(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.service_scheduled = false;
        self.queue.clear();
        self.in_flight = None;
        self.completion = None;
        self.actions.clear();
    }

    /// Returns a service event for the active reset epoch.
    pub const fn next_service_event(&self) -> IsaBusEvent {
        IsaBusEvent::Service { epoch: self.epoch }
    }
}

impl Component for IsaBus {
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

impl BusRole<IsaTransaction> for IsaBus {
    type Response = IsaBusDisposition;

    fn route(&mut self, transaction: IsaTransaction) -> Self::Response {
        self.queue.push_back(transaction);
        if self.service_scheduled || self.in_flight.is_some() || self.completion.is_some() {
            IsaBusDisposition::Queued
        } else {
            self.service_scheduled = true;
            IsaBusDisposition::QueuedAndNeedsService {
                delay: self.cycle,
                event: self.next_service_event(),
            }
        }
    }
}

#[cfg(test)]
mod tests;
