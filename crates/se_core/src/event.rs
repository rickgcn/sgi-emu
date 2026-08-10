//! Provides deterministic virtual-time event scheduling.
//!
//! [`EventQueue`] orders events by virtual time and insertion sequence, so events
//! with the same deadline are delivered in strict FIFO order. [`ScheduleToken`]
//! combines a slab index with a generation to prevent stale cancellation after a
//! slot is reused.
//!
//! [`SchedulerShared`] adds device-bound handles and scoped CPU-burst
//! truncation. Scheduling an event before the active burst deadline requests an
//! event-truncate exit through that burst's [`InterruptWord`].

use std::cell::{Cell, RefCell};
use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap};
use std::error::Error;
use std::fmt;
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::device::DeviceId;
use crate::interrupt::InterruptWord;
use crate::save::{SNAPSHOT_VERSION, Saveable, StateError, StateReader, StateWriter};
use crate::time::{NO_DEADLINE, VTime};

/// Describes one deterministic event awaiting delivery to a device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScheduledEvent {
    /// Delivery time in virtual nanoseconds.
    pub vtime: VTime,
    /// Runtime identity of the destination device.
    pub device: DeviceId,
    /// Device-defined event selector interpreted by the destination.
    pub tag: u32,
    /// Device-defined scalar payload whose interpretation is selected by [`Self::tag`].
    pub payload: u64,
}

/// Identifies one live generation of an event-queue slot.
///
/// A token remains useful only while that generation is live. Cancellation or
/// delivery advances the slot generation, causing stale tokens to stop matching
/// even if the slot is later reused.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ScheduleToken {
    slot: u32,
    generation: u32,
}

impl ScheduleToken {
    /// Decodes a token from its snapshot-safe raw representation.
    ///
    /// The low 32 bits contain the slot and the high 32 bits contain the
    /// generation. Decoding does not establish that the token names a live event;
    /// use [`EventQueue::is_scheduled`] for that check.
    #[must_use]
    pub const fn from_raw(raw: u64) -> Self {
        Self {
            slot: raw as u32,
            generation: (raw >> 32) as u32,
        }
    }

    /// Encodes the token as a snapshot-safe raw representation.
    ///
    /// The low 32 bits contain the slot and the high 32 bits contain the
    /// generation.
    #[must_use]
    pub const fn to_raw(self) -> u64 {
        ((self.generation as u64) << 32) | self.slot as u64
    }

    /// Returns the slab slot index.
    #[must_use]
    pub const fn slot(self) -> u32 {
        self.slot
    }

    /// Returns the slab generation.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct LiveEvent {
    event: ScheduledEvent,
    seq: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct Slot {
    generation: u32,
    live: Option<LiveEvent>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeapEntry {
    vtime: VTime,
    seq: u64,
    slot: u32,
    generation: u32,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is max-first, so reverse every key to expose the minimum
        // deterministic (virtual time, sequence, slot, generation) entry.
        other
            .vtime
            .cmp(&self.vtime)
            .then_with(|| other.seq.cmp(&self.seq))
            .then_with(|| other.slot.cmp(&self.slot))
            .then_with(|| other.generation.cmp(&self.generation))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Reports a rejected event-queue operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventQueueError {
    /// Virtual time cannot move backwards.
    TimeReversal {
        /// Current queue time.
        current: VTime,
        /// Rejected earlier time.
        requested: VTime,
    },
    /// An event cannot be scheduled before the queue's current time.
    PastEvent {
        /// Current queue time.
        current: VTime,
        /// Rejected event time.
        requested: VTime,
    },
    /// The stable FIFO sequence counter is exhausted.
    SequenceExhausted,
    /// The slab cannot represent another slot with a `u32` index.
    SlotExhausted,
    /// A slot generation cannot be advanced without wrapping.
    GenerationExhausted(u32),
    /// A relative event deadline overflowed virtual time.
    DeadlineOverflow,
}

impl fmt::Display for EventQueueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimeReversal { current, requested } => write!(
                formatter,
                "virtual time cannot move backwards from {current} to {requested}"
            ),
            Self::PastEvent { current, requested } => write!(
                formatter,
                "event time {requested} precedes current virtual time {current}"
            ),
            Self::SequenceExhausted => formatter.write_str("event FIFO sequence is exhausted"),
            Self::SlotExhausted => formatter.write_str("event slab slot index is exhausted"),
            Self::GenerationExhausted(slot) => {
                write!(formatter, "event slot {slot} generation is exhausted")
            }
            Self::DeadlineOverflow => formatter.write_str("event deadline overflows virtual time"),
        }
    }
}

impl Error for EventQueueError {}

/// Stores events in deterministic deadline and insertion order.
///
/// Events are delivered in ascending `(vtime, sequence)` order. Cancellation is
/// lazy in the heap but immediate in the slab, and the token generation prevents
/// a cancelled token from affecting a later occupant of the same slot.
///
/// Snapshot state preserves insertion sequences, slot generations, and free-slot
/// order. Restoration rebuilds the derived heap without changing delivery order
/// or the validity of live tokens.
#[derive(Debug)]
pub struct EventQueue {
    now: VTime,
    next_seq: u64,
    slots: Vec<Slot>,
    free: Vec<u32>,
    heap: BinaryHeap<HeapEntry>,
}

impl EventQueue {
    /// Creates an empty queue at virtual time zero.
    #[must_use]
    pub fn new() -> Self {
        Self {
            now: 0,
            next_seq: 0,
            slots: Vec::new(),
            free: Vec::new(),
            heap: BinaryHeap::new(),
        }
    }

    /// Returns the queue's current virtual time.
    #[must_use]
    pub const fn now(&self) -> VTime {
        self.now
    }

    /// Advances the queue's current virtual time monotonically.
    ///
    /// # Errors
    ///
    /// Returns [`EventQueueError::TimeReversal`] without modifying the queue when
    /// `now` precedes the current virtual time.
    pub fn advance_to(&mut self, now: VTime) -> Result<(), EventQueueError> {
        if now < self.now {
            return Err(EventQueueError::TimeReversal {
                current: self.now,
                requested: now,
            });
        }
        self.now = now;
        Ok(())
    }

    /// Schedules an event and returns its cancellation token.
    ///
    /// Events assigned the same virtual time are delivered in call order.
    ///
    /// # Errors
    ///
    /// Returns [`EventQueueError::PastEvent`] when the deadline precedes
    /// [`Self::now`], [`EventQueueError::SequenceExhausted`] when another FIFO
    /// sequence cannot be represented, or [`EventQueueError::SlotExhausted`] when
    /// another slab index cannot be represented. Rejection leaves the queue
    /// unchanged.
    pub fn schedule(&mut self, event: ScheduledEvent) -> Result<ScheduleToken, EventQueueError> {
        if event.vtime < self.now {
            return Err(EventQueueError::PastEvent {
                current: self.now,
                requested: event.vtime,
            });
        }

        let next_seq = self
            .next_seq
            .checked_add(1)
            .ok_or(EventQueueError::SequenceExhausted)?;
        let seq = self.next_seq;

        let slot_index = if let Some(slot_index) = self.free.pop() {
            debug_assert_ne!(self.slots[slot_index as usize].generation, u32::MAX);
            slot_index
        } else {
            let slot =
                u32::try_from(self.slots.len()).map_err(|_| EventQueueError::SlotExhausted)?;
            self.slots.push(Slot {
                generation: 0,
                live: None,
            });
            slot
        };
        let slot = &mut self.slots[slot_index as usize];
        debug_assert!(slot.live.is_none());
        let token = ScheduleToken {
            slot: slot_index,
            generation: slot.generation,
        };
        slot.live = Some(LiveEvent { event, seq });
        self.heap.push(HeapEntry {
            vtime: event.vtime,
            seq,
            slot: slot_index,
            generation: slot.generation,
        });
        self.next_seq = next_seq;
        Ok(token)
    }

    /// Cancels the event named by a live token.
    ///
    /// Returns `false` without modifying the queue when the token is out of range,
    /// stale, or already inactive. A successful cancellation invalidates the
    /// token before returning.
    ///
    /// # Errors
    ///
    /// Returns [`EventQueueError::GenerationExhausted`] if the named live slot's
    /// generation cannot be advanced.
    pub fn cancel(&mut self, token: ScheduleToken) -> Result<bool, EventQueueError> {
        let Some(slot) = self.slots.get_mut(token.slot as usize) else {
            return Ok(false);
        };
        if slot.generation != token.generation || slot.live.is_none() {
            return Ok(false);
        }
        let next_generation = slot
            .generation
            .checked_add(1)
            .ok_or(EventQueueError::GenerationExhausted(token.slot))?;
        slot.live = None;
        slot.generation = next_generation;
        if next_generation != u32::MAX {
            self.free.push(token.slot);
        }
        Ok(true)
    }

    /// Returns the earliest live event time, pruning cancelled heap entries.
    pub fn front_time(&mut self) -> Option<VTime> {
        self.prune_top();
        self.heap.peek().map(|entry| entry.vtime)
    }

    /// Removes and returns the earliest event due at the current virtual time.
    ///
    /// Returns `None` when the queue has no live event whose deadline is at or
    /// before [`Self::now`]. A returned event's token is invalidated before this
    /// method returns.
    ///
    /// # Errors
    ///
    /// Returns [`EventQueueError::GenerationExhausted`] if the due event's slot
    /// generation cannot be advanced.
    pub fn pop_due(&mut self) -> Result<Option<ScheduledEvent>, EventQueueError> {
        self.prune_top();
        let Some(entry) = self.heap.peek().copied() else {
            return Ok(None);
        };
        if entry.vtime > self.now {
            return Ok(None);
        }
        let next_generation = self.slots[entry.slot as usize]
            .generation
            .checked_add(1)
            .ok_or(EventQueueError::GenerationExhausted(entry.slot))?;
        self.heap.pop();
        let slot = &mut self.slots[entry.slot as usize];
        let live = slot.live.take().expect("live heap entry must have a slot");
        slot.generation = next_generation;
        if next_generation != u32::MAX {
            self.free.push(entry.slot);
        }
        Ok(Some(live.event))
    }

    /// Returns whether a token still names a live event.
    #[must_use]
    pub fn is_scheduled(&self, token: ScheduleToken) -> bool {
        self.slots
            .get(token.slot as usize)
            .is_some_and(|slot| slot.generation == token.generation && slot.live.is_some())
    }

    /// Returns the number of live events.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.iter().filter(|slot| slot.live.is_some()).count()
    }

    /// Returns whether the queue has no live events.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Validates that every live event targets a registered device index.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::InvalidState`] when an event's raw device index is
    /// greater than or equal to `device_count`.
    pub fn validate_device_ids(&self, device_count: u32) -> Result<(), StateError> {
        if let Some(device) = self
            .slots
            .iter()
            .filter_map(|slot| slot.live)
            .map(|live| live.event.device.get())
            .find(|device| *device >= device_count)
        {
            return Err(StateError::InvalidState(format!(
                "event targets missing device {device}"
            )));
        }
        Ok(())
    }

    fn prune_top(&mut self) {
        while self.heap.peek().is_some_and(|entry| {
            self.slots.get(entry.slot as usize).and_then(|slot| {
                slot.live
                    .map(|live| slot.generation == entry.generation && live.seq == entry.seq)
            }) != Some(true)
        }) {
            self.heap.pop();
        }
    }

    fn from_state(state: EventQueueState) -> Result<Self, StateError> {
        if state.slots.len() > u32::MAX as usize {
            return Err(StateError::InvalidState(
                "event slab exceeds u32 slot space".to_owned(),
            ));
        }

        let mut free_set = BTreeSet::new();
        for &index in &state.free {
            if index as usize >= state.slots.len() {
                return Err(StateError::InvalidState(format!(
                    "free event slot {index} is out of range"
                )));
            }
            if !free_set.insert(index) {
                return Err(StateError::InvalidState(format!(
                    "free event slot {index} is duplicated"
                )));
            }
        }

        let mut sequence_set = BTreeSet::new();
        let mut slots = Vec::with_capacity(state.slots.len());
        let mut heap = BinaryHeap::new();
        for (expected_index, saved) in state.slots.into_iter().enumerate() {
            if saved.index as usize != expected_index {
                return Err(StateError::InvalidState(format!(
                    "event slot index {} appears at position {expected_index}",
                    saved.index
                )));
            }
            if saved.generation == u32::MAX && saved.live.is_some() {
                return Err(StateError::InvalidState(format!(
                    "live event slot {} has exhausted its generation",
                    saved.index
                )));
            }
            let live = saved.live.map(|entry| {
                let event = ScheduledEvent {
                    vtime: entry.vtime,
                    device: DeviceId::from_raw(entry.device),
                    tag: entry.tag,
                    payload: entry.payload,
                };
                LiveEvent {
                    event,
                    seq: entry.seq,
                }
            });
            let should_be_free = live.is_none() && saved.generation != u32::MAX;
            if should_be_free != free_set.contains(&saved.index) {
                return Err(StateError::InvalidState(format!(
                    "event slot {} lifecycle state is inconsistent with the free stack",
                    saved.index
                )));
            }
            if let Some(entry) = live {
                if entry.seq >= state.next_seq {
                    return Err(StateError::InvalidState(format!(
                        "event sequence {} is not below next sequence {}",
                        entry.seq, state.next_seq
                    )));
                }
                if !sequence_set.insert(entry.seq) {
                    return Err(StateError::InvalidState(format!(
                        "event sequence {} is duplicated",
                        entry.seq
                    )));
                }
                heap.push(HeapEntry {
                    vtime: entry.event.vtime,
                    seq: entry.seq,
                    slot: saved.index,
                    generation: saved.generation,
                });
            }
            slots.push(Slot {
                generation: saved.generation,
                live,
            });
        }

        Ok(Self {
            now: state.now,
            next_seq: state.next_seq,
            slots,
            free: state.free,
            heap,
        })
    }
}

impl Default for EventQueue {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Serialize, Deserialize)]
struct EventQueueState {
    now: u64,
    next_seq: u64,
    slots: Vec<SlotState>,
    free: Vec<u32>,
}

#[derive(Serialize, Deserialize)]
struct SlotState {
    index: u32,
    generation: u32,
    live: Option<LiveEventState>,
}

#[derive(Serialize, Deserialize)]
struct LiveEventState {
    vtime: u64,
    device: u32,
    tag: u32,
    payload: u64,
    seq: u64,
}

impl Saveable for EventQueue {
    fn snapshot_version(&self) -> u32 {
        SNAPSHOT_VERSION
    }

    fn save(&self, writer: &mut StateWriter<'_>) -> Result<(), StateError> {
        let slots = self
            .slots
            .iter()
            .enumerate()
            .map(|(index, slot)| {
                let index = u32::try_from(index).map_err(|_| StateError::LengthOverflow)?;
                Ok(SlotState {
                    index,
                    generation: slot.generation,
                    live: slot.live.map(|live| LiveEventState {
                        vtime: live.event.vtime,
                        device: live.event.device.get(),
                        tag: live.event.tag,
                        payload: live.event.payload,
                        seq: live.seq,
                    }),
                })
            })
            .collect::<Result<Vec<_>, StateError>>()?;
        writer.serialize(&EventQueueState {
            now: self.now,
            next_seq: self.next_seq,
            slots,
            free: self.free.clone(),
        })
    }

    fn load(&mut self, version: u32, reader: &mut StateReader<'_>) -> Result<(), StateError> {
        if version != SNAPSHOT_VERSION {
            return Err(StateError::UnsupportedVersion(version));
        }
        let state: EventQueueState = reader.deserialize()?;
        let replacement = Self::from_state(state)?;
        *self = replacement;
        Ok(())
    }
}

struct SchedulerInner {
    queue: RefCell<EventQueue>,
    burst_deadline: Cell<VTime>,
    truncate_target: RefCell<Option<InterruptWord>>,
}

/// Shares one single-threaded event queue among machine components.
///
/// Clones refer to the same queue and active burst state. `Rc` ownership makes the
/// scheduler neither `Send` nor `Sync`; every clone remains on one host thread.
/// Snapshot operations persist the queue, while an active burst deadline and its
/// truncation target are transient runtime state.
#[derive(Clone)]
pub struct SchedulerShared {
    inner: Rc<SchedulerInner>,
}

impl SchedulerShared {
    /// Creates a scheduler with an empty queue.
    #[must_use]
    pub fn new() -> Self {
        Self::from_queue(EventQueue::new())
    }

    /// Takes ownership of an existing event queue and creates shared scheduler state.
    #[must_use]
    pub fn from_queue(queue: EventQueue) -> Self {
        Self {
            inner: Rc::new(SchedulerInner {
                queue: RefCell::new(queue),
                burst_deadline: Cell::new(NO_DEADLINE),
                truncate_target: RefCell::new(None),
            }),
        }
    }

    /// Creates a scheduling handle bound to one destination device.
    #[must_use]
    pub fn handle(&self, device: DeviceId) -> SchedulerHandle {
        SchedulerHandle {
            shared: self.clone(),
            device,
        }
    }

    /// Returns the current virtual time.
    #[must_use]
    pub fn now(&self) -> VTime {
        self.inner.queue.borrow().now()
    }

    /// Advances virtual time monotonically.
    ///
    /// # Errors
    ///
    /// Returns the error reported by [`EventQueue::advance_to`].
    pub fn advance_to(&self, now: VTime) -> Result<(), EventQueueError> {
        self.inner.queue.borrow_mut().advance_to(now)
    }

    /// Returns the earliest live event time.
    pub fn front_time(&self) -> Option<VTime> {
        self.inner.queue.borrow_mut().front_time()
    }

    /// Removes and returns the earliest due event.
    ///
    /// # Errors
    ///
    /// Returns the error reported by [`EventQueue::pop_due`].
    pub fn pop_due(&self) -> Result<Option<ScheduledEvent>, EventQueueError> {
        self.inner.queue.borrow_mut().pop_due()
    }

    /// Begins a CPU burst with its deadline and truncation target.
    ///
    /// Entry clears any stale event-truncate request on `truncate_target`.
    /// Keeping the returned [`BurstScope`] alive marks the burst as active;
    /// dropping it clears the request and removes the active deadline.
    /// [`NO_DEADLINE`] represents a burst with no finite deadline, which is still
    /// truncated by every newly scheduled finite event.
    ///
    /// # Errors
    ///
    /// Returns [`BurstScopeError::AlreadyActive`] if another burst scope is active
    /// on this shared scheduler.
    pub fn begin_burst(
        &self,
        deadline: VTime,
        truncate_target: InterruptWord,
    ) -> Result<BurstScope, BurstScopeError> {
        if self.inner.truncate_target.borrow().is_some() {
            return Err(BurstScopeError::AlreadyActive);
        }
        truncate_target.clear_event_truncate();
        self.inner.burst_deadline.set(deadline);
        *self.inner.truncate_target.borrow_mut() = Some(truncate_target);
        Ok(BurstScope {
            inner: Rc::clone(&self.inner),
        })
    }

    /// Validates all event destination identities against a device count.
    ///
    /// # Errors
    ///
    /// Returns the error reported by [`EventQueue::validate_device_ids`].
    pub fn validate_device_ids(&self, device_count: u32) -> Result<(), StateError> {
        self.inner.queue.borrow().validate_device_ids(device_count)
    }
}

impl Default for SchedulerShared {
    fn default() -> Self {
        Self::new()
    }
}

impl Saveable for SchedulerShared {
    fn snapshot_version(&self) -> u32 {
        SNAPSHOT_VERSION
    }

    fn save(&self, writer: &mut StateWriter<'_>) -> Result<(), StateError> {
        self.inner.queue.borrow().save(writer)
    }

    fn load(&mut self, version: u32, reader: &mut StateReader<'_>) -> Result<(), StateError> {
        self.inner.queue.borrow_mut().load(version, reader)
    }
}

/// Reports a rejected CPU-burst scope operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BurstScopeError {
    /// Only one CPU burst may be active on a scheduler at a time.
    AlreadyActive,
}

impl fmt::Display for BurstScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyActive => formatter.write_str("a scheduler burst is already active"),
        }
    }
}

impl Error for BurstScopeError {}

/// Keeps one CPU burst's deadline and truncation target active.
///
/// Dropping the scope clears the target's event-truncate request and restores the
/// scheduler to its no-active-burst state.
pub struct BurstScope {
    inner: Rc<SchedulerInner>,
}

impl Drop for BurstScope {
    fn drop(&mut self) {
        if let Some(target) = self.inner.truncate_target.borrow_mut().take() {
            target.clear_event_truncate();
        }
        self.inner.burst_deadline.set(NO_DEADLINE);
    }
}

/// Schedules events for one bound destination device identity.
#[derive(Clone)]
pub struct SchedulerHandle {
    shared: SchedulerShared,
    device: DeviceId,
}

impl SchedulerHandle {
    /// Returns the destination device identity.
    #[must_use]
    pub const fn device(&self) -> DeviceId {
        self.device
    }

    /// Returns the scheduler's current virtual time.
    #[must_use]
    pub fn now(&self) -> VTime {
        self.shared.now()
    }

    /// Schedules an event at an absolute virtual time.
    ///
    /// If a burst is active and `vtime` precedes its deadline, successful insertion
    /// requests event truncation on that burst's target. An event exactly at the
    /// deadline does not request truncation.
    ///
    /// # Errors
    ///
    /// Returns the error reported by [`EventQueue::schedule`]. In particular, a
    /// time earlier than the scheduler's current time is rejected without
    /// requesting truncation.
    pub fn schedule_at(
        &self,
        vtime: VTime,
        tag: u32,
        payload: u64,
    ) -> Result<ScheduleToken, EventQueueError> {
        let token = self
            .shared
            .inner
            .queue
            .borrow_mut()
            .schedule(ScheduledEvent {
                vtime,
                device: self.device,
                tag,
                payload,
            })?;
        let deadline = self.shared.inner.burst_deadline.get();
        if vtime < deadline
            && let Some(target) = self.shared.inner.truncate_target.borrow().as_ref()
        {
            target.request_event_truncate();
        }
        Ok(token)
    }

    /// Schedules an event relative to the current virtual time.
    ///
    /// The base is the shared scheduler time, not time retained locally by a CPU.
    /// A machine therefore synchronizes its scheduler before entering a
    /// CPU-originated [`crate::bus::Bus`] transaction whose device may call this
    /// method.
    ///
    /// # Errors
    ///
    /// Returns [`EventQueueError::DeadlineOverflow`] when the current time plus
    /// `delay` is not representable, or an error reported by [`Self::schedule_at`].
    pub fn schedule_after(
        &self,
        delay: VTime,
        tag: u32,
        payload: u64,
    ) -> Result<ScheduleToken, EventQueueError> {
        let deadline = self
            .now()
            .checked_add(delay)
            .ok_or(EventQueueError::DeadlineOverflow)?;
        self.schedule_at(deadline, tag, payload)
    }

    /// Cancels an event previously scheduled through any scheduler handle.
    ///
    /// Returns `false` when `token` no longer names a live event.
    ///
    /// # Errors
    ///
    /// Returns the error reported by [`EventQueue::cancel`].
    pub fn cancel(&self, token: ScheduleToken) -> Result<bool, EventQueueError> {
        self.shared.inner.queue.borrow_mut().cancel(token)
    }

    /// Returns whether a token still names a live event.
    #[must_use]
    pub fn is_scheduled(&self, token: ScheduleToken) -> bool {
        self.shared.inner.queue.borrow().is_scheduled(token)
    }
}

#[cfg(test)]
mod tests {
    use crate::save::{SNAPSHOT_VERSION, Saveable, StateError, StateReader, StateWriter};

    use super::{
        DeviceId, EventQueue, EventQueueError, EventQueueState, LiveEventState, ScheduledEvent,
        SchedulerShared, SlotState,
    };
    use crate::interrupt::{EVENT_TRUNCATE, InterruptWord};
    use crate::time::NO_DEADLINE;

    fn event(vtime: u64, device: u32, tag: u32) -> ScheduledEvent {
        ScheduledEvent {
            vtime,
            device: DeviceId::from_raw(device),
            tag,
            payload: u64::from(tag) * 10,
        }
    }

    fn save_queue(queue: &EventQueue) -> Vec<u8> {
        let mut payload = Vec::new();
        let mut writer = StateWriter::new(&mut payload);
        queue.save(&mut writer).unwrap();
        writer.finish().unwrap();
        payload
    }

    #[test]
    fn equal_time_events_are_strict_fifo() {
        let mut queue = EventQueue::new();
        queue.schedule(event(10, 0, 1)).unwrap();
        queue.schedule(event(10, 0, 2)).unwrap();
        queue.schedule(event(10, 0, 3)).unwrap();
        queue.advance_to(10).unwrap();
        assert_eq!(queue.pop_due().unwrap().unwrap().tag, 1);
        assert_eq!(queue.pop_due().unwrap().unwrap().tag, 2);
        assert_eq!(queue.pop_due().unwrap().unwrap().tag, 3);
    }

    #[test]
    fn cancellation_is_lazy_and_generation_safe() {
        let mut queue = EventQueue::new();
        let cancelled = queue.schedule(event(2, 0, 1)).unwrap();
        queue.schedule(event(3, 0, 2)).unwrap();
        assert!(queue.cancel(cancelled).unwrap());
        assert!(!queue.cancel(cancelled).unwrap());
        let reused = queue.schedule(event(1, 0, 3)).unwrap();
        assert_eq!(reused.slot(), cancelled.slot());
        assert_ne!(reused.generation(), cancelled.generation());
        assert_eq!(queue.front_time(), Some(1));
    }

    #[test]
    fn queue_snapshot_preserves_tokens_and_reuse_order() {
        let mut original = EventQueue::new();
        let first = original.schedule(event(20, 0, 1)).unwrap();
        let second = original.schedule(event(10, 1, 2)).unwrap();
        let third = original.schedule(event(30, 1, 3)).unwrap();
        original.cancel(first).unwrap();
        original.cancel(third).unwrap();

        let payload = save_queue(&original);
        let mut restored = EventQueue::new();
        let mut reader = StateReader::new(&payload);
        restored.load(1, &mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(save_queue(&restored), payload);
        assert!(restored.is_scheduled(second));

        let original_reuse = original.schedule(event(40, 0, 4)).unwrap();
        let restored_reuse = restored.schedule(event(40, 0, 4)).unwrap();
        assert_eq!(original_reuse, restored_reuse);
        original.advance_to(10).unwrap();
        restored.advance_to(10).unwrap();
        assert_eq!(original.pop_due().unwrap(), restored.pop_due().unwrap());
    }

    #[test]
    fn queue_restore_rejects_exhausted_live_generation_atomically() {
        let mut queue = EventQueue::new();
        let original_token = queue.schedule(event(10, 0, 1)).unwrap();
        let original_payload = save_queue(&queue);
        let invalid_state = EventQueueState {
            now: 0,
            next_seq: 1,
            slots: vec![SlotState {
                index: 0,
                generation: u32::MAX,
                live: Some(LiveEventState {
                    vtime: 5,
                    device: 0,
                    tag: 2,
                    payload: 20,
                    seq: 0,
                }),
            }],
            free: Vec::new(),
        };
        let mut invalid_payload = Vec::new();
        let mut writer = StateWriter::new(&mut invalid_payload);
        writer.serialize(&invalid_state).unwrap();
        writer.finish().unwrap();

        let mut reader = StateReader::new(&invalid_payload);
        assert!(matches!(
            queue.load(SNAPSHOT_VERSION, &mut reader),
            Err(StateError::InvalidState(reason))
                if reason == "live event slot 0 has exhausted its generation"
        ));
        reader.finish().unwrap();
        assert_eq!(save_queue(&queue), original_payload);
        assert!(queue.is_scheduled(original_token));
    }

    #[test]
    fn exhausted_generations_retire_slots_and_round_trip() {
        let mut queue = EventQueue::from_state(EventQueueState {
            now: 0,
            next_seq: 0,
            slots: vec![
                SlotState {
                    index: 0,
                    generation: u32::MAX - 1,
                    live: None,
                },
                SlotState {
                    index: 1,
                    generation: u32::MAX - 1,
                    live: None,
                },
            ],
            free: vec![0, 1],
        })
        .unwrap();

        let cancelled = queue.schedule(event(5, 0, 1)).unwrap();
        assert_eq!(cancelled.slot(), 1);
        assert_eq!(cancelled.generation(), u32::MAX - 1);
        assert!(queue.cancel(cancelled).unwrap());
        assert_eq!(queue.free, vec![0]);

        let due = queue.schedule(event(1, 0, 2)).unwrap();
        assert_eq!(due.slot(), 0);
        assert_eq!(due.generation(), u32::MAX - 1);
        queue.advance_to(1).unwrap();
        assert_eq!(queue.pop_due().unwrap(), Some(event(1, 0, 2)));
        assert!(queue.free.is_empty());

        let replacement = queue.schedule(event(10, 0, 3)).unwrap();
        assert_eq!(replacement.slot(), 2);
        assert_eq!(replacement.generation(), 0);

        let payload = save_queue(&queue);
        let mut restored = EventQueue::new();
        let mut reader = StateReader::new(&payload);
        restored.load(SNAPSHOT_VERSION, &mut reader).unwrap();
        reader.finish().unwrap();
        assert_eq!(save_queue(&restored), payload);
        assert!(restored.is_scheduled(replacement));
    }

    #[test]
    fn queue_restore_rejects_retired_slots_in_the_free_stack() {
        assert!(matches!(
            EventQueue::from_state(EventQueueState {
                now: 0,
                next_seq: 0,
                slots: vec![SlotState {
                    index: 0,
                    generation: u32::MAX,
                    live: None,
                }],
                free: vec![0],
            }),
            Err(StateError::InvalidState(_))
        ));
    }

    #[test]
    fn scheduler_truncates_active_bursts_before_their_deadlines() {
        let scheduler = SchedulerShared::new();
        let handle = scheduler.handle(DeviceId::from_raw(7));
        let cpu_one = InterruptWord::new();
        let cpu_two = InterruptWord::new();

        handle.schedule_at(5, 1, 0).unwrap();
        assert_eq!(cpu_one.load_relaxed(), 0);
        {
            let _burst = scheduler.begin_burst(100, cpu_two.clone()).unwrap();
            handle.schedule_at(100, 2, 0).unwrap();
            assert_eq!(cpu_two.load_relaxed(), 0);
            handle.schedule_at(50, 3, 0).unwrap();
            assert_eq!(cpu_one.load_relaxed(), 0);
            assert_eq!(cpu_two.load_relaxed(), EVENT_TRUNCATE);
        }
        assert_eq!(cpu_two.load_relaxed(), 0);
        {
            let _burst = scheduler.begin_burst(NO_DEADLINE, cpu_one.clone()).unwrap();
            handle.schedule_at(NO_DEADLINE, 4, 0).unwrap();
            assert_eq!(cpu_one.load_relaxed(), 0);
            handle.schedule_at(1, 5, 0).unwrap();
            assert_eq!(cpu_one.load_relaxed(), EVENT_TRUNCATE);
        }
        assert_eq!(cpu_one.load_relaxed(), 0);
    }

    #[test]
    fn queue_rejects_past_events_without_mutation() {
        let mut queue = EventQueue::new();
        queue.advance_to(10).unwrap();
        let before = save_queue(&queue);

        assert_eq!(
            queue.schedule(event(9, 0, 1)),
            Err(EventQueueError::PastEvent {
                current: 10,
                requested: 9,
            })
        );
        assert_eq!(save_queue(&queue), before);

        let token = queue.schedule(event(10, 0, 2)).unwrap();
        assert_eq!(token.slot(), 0);
        assert_eq!(token.generation(), 0);
        assert_eq!(queue.front_time(), Some(10));
    }

    #[test]
    fn queue_rejects_time_reversal_and_unknown_event_targets() {
        let mut queue = EventQueue::new();
        queue.advance_to(10).unwrap();
        assert!(queue.advance_to(9).is_err());
        queue.schedule(event(10, 3, 1)).unwrap();
        assert!(queue.validate_device_ids(3).is_err());
    }
}
