//! Defines object-safe machine driving and construction interfaces.
//!
//! [`Machine`] exposes the deterministic boundary used by a runtime: inspect the
//! next event deadline, advance the complete CPU complex, remove due events, and
//! dispatch them. It also combines component snapshots, introspection, and a
//! canonical guest-visible state digest without exposing a concrete CPU, device,
//! or machine type.
//!
//! [`MachineFactory`] belongs to the application composition root and constructs a
//! fresh canonical topology for snapshot loading. This module defines neither the
//! runtime loop policy nor how a machine interleaves CPUs within its single virtual
//! timeline.

use std::error::Error;
use std::fmt;

use crate::decode::DeviceDispatchError;
use crate::event::{EventQueueError, ScheduledEvent};
use crate::inspect::Introspect;
use crate::save::StateError;
use crate::snapshot::{ProfileFingerprint, SnapshotTarget};
use crate::time::VTime;

/// Identifies why a machine's CPU complex returned control to the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuExit {
    /// The CPU complex reached the requested virtual-time deadline.
    Deadline,
    /// A scheduling change invalidated the current burst deadline.
    Reschedule,
    /// Pending host-control work ended the burst.
    HostWake,
    /// A debugger breakpoint ended the burst.
    Breakpoint,
    /// The machine entered a halted state.
    Halted,
}

/// Contains a canonical digest of all guest-visible machine state.
///
/// The concrete machine defines the canonical encoding being digested. Equal
/// deterministic states within one build and profile produce equal bytes under
/// that machine contract; this value is distinct from the snapshot container's
/// integrity digest.
#[repr(transparent)]
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct StateDigest([u8; 32]);

impl StateDigest {
    /// Creates a digest from its 32 canonical bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Returns the canonical digest bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

/// Reports a failure on a machine's runtime driving surface.
#[derive(Debug)]
pub enum MachineError {
    /// Event queue advancement or removal failed.
    Event(EventQueueError),
    /// Event delivery to a device failed.
    Dispatch(DeviceDispatchError),
    /// State encoding or validation failed.
    State(StateError),
    /// A machine-specific invariant or operation failed.
    Failed(String),
}

impl fmt::Display for MachineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Event(error) => write!(formatter, "machine event operation failed: {error}"),
            Self::Dispatch(error) => write!(formatter, "machine event dispatch failed: {error}"),
            Self::State(error) => write!(formatter, "machine state operation failed: {error}"),
            Self::Failed(reason) => write!(formatter, "machine operation failed: {reason}"),
        }
    }
}

impl Error for MachineError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Event(error) => Some(error),
            Self::Dispatch(error) => Some(error),
            Self::State(error) => Some(error),
            Self::Failed(_) => None,
        }
    }
}

impl From<EventQueueError> for MachineError {
    fn from(error: EventQueueError) -> Self {
        Self::Event(error)
    }
}

impl From<DeviceDispatchError> for MachineError {
    fn from(error: DeviceDispatchError) -> Self {
        Self::Dispatch(error)
    }
}

impl From<StateError> for MachineError {
    fn from(error: StateError) -> Self {
        Self::State(error)
    }
}

/// Defines the complete object-safe driving surface used by the runtime.
///
/// The CPU complex represents the whole machine rather than a distinguished CPU.
/// An implementation may deterministically interleave multiple guest CPUs while
/// retaining one machine virtual timeline and one guest-visible event order.
pub trait Machine: SnapshotTarget + Introspect {
    /// Returns the current machine virtual time.
    fn now(&self) -> VTime;

    /// Returns the earliest live event time after pruning cancelled entries.
    ///
    /// Returns `None` when no event is scheduled. The returned deadline may equal
    /// the current machine time when an event is already due.
    fn front_event_time(&mut self) -> Option<VTime>;

    /// Advances the complete CPU complex until a deadline or another exit reason.
    ///
    /// `deadline` is an absolute virtual time. [`crate::time::NO_DEADLINE`] requests
    /// an unbounded burst; a newly scheduled finite event can still truncate that
    /// burst through the active CPU's event-truncate target.
    ///
    /// Machine time may stop at an event between CPU instruction boundaries.
    /// Implementations preserve each CPU's absolute clock phase across such a
    /// stop; resuming does not derive the next boundary from the current machine
    /// time. Per-CPU phase and any fractional-rate remainder are guest-visible
    /// state and therefore contribute to snapshots and state digests.
    ///
    /// An implementation may batch time in CPU-local state only while execution
    /// cannot call a device or otherwise expose stale machine time. Before every
    /// CPU-originated [`crate::bus::Bus`] call, including direct-span discovery, it
    /// advances the event scheduler to the transaction's architectural timestamp.
    /// It also commits local time before returning or switching guest CPUs. A
    /// synchronization target never exceeds a finite `deadline`.
    ///
    /// CPU work timestamped exactly at a finite deadline completes before the
    /// method returns [`CpuExit::Deadline`]. A runtime can then dispatch events at
    /// that timestamp, preserving CPU-before-event ordering for equal times.
    ///
    /// [`CpuExit::Deadline`] means the complex reached `deadline`. Other exit
    /// reasons may return earlier, and no successful call moves machine time beyond
    /// a finite deadline.
    ///
    /// [`CpuExit::Reschedule`] reports an event-truncation request. The runtime
    /// queries the next event deadline again before resuming the CPU complex.
    /// Guest interrupt lines do not produce a machine exit: the CPU architecture
    /// samples them, enters its guest exception state when enabled, and continues
    /// execution until another exit reason applies.
    ///
    /// Before returning [`CpuExit::HostWake`], the CPU complex consumes the
    /// corresponding [`crate::interrupt::HOST_WAKE`] doorbell with acquire
    /// ordering without changing guest interrupt lines. The runtime then drains
    /// its host-control channel before resuming guest execution.
    ///
    /// # Errors
    ///
    /// Returns [`MachineError`] when CPU advancement or a machine invariant fails.
    fn run_cpu_until(&mut self, deadline: VTime) -> Result<CpuExit, MachineError>;

    /// Removes the earliest event due at the current machine time.
    ///
    /// Returns `None` when no live event is due.
    ///
    /// # Errors
    ///
    /// Returns [`MachineError`] when queue maintenance or event removal fails.
    fn pop_event(&mut self) -> Result<Option<ScheduledEvent>, MachineError>;

    /// Dispatches a previously popped event to its target device.
    ///
    /// An error does not roll back device, bus, or scheduling effects already
    /// produced by the callback.
    ///
    /// # Errors
    ///
    /// Returns [`MachineError`] when the target is unavailable or its callback
    /// fails.
    fn dispatch_event(&mut self, event: ScheduledEvent) -> Result<(), MachineError>;

    /// Computes a canonical digest of guest-visible state.
    ///
    /// Host resources and nondeterministic presentation state do not contribute to
    /// the digest.
    ///
    /// # Errors
    ///
    /// Returns [`MachineError`] when canonical state cannot be encoded or
    /// validated.
    fn state_digest(&self) -> Result<StateDigest, MachineError>;
}

/// Describes a failure to assemble a fresh machine instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineCreateError {
    reason: String,
}

impl MachineCreateError {
    /// Creates an assembly error with a human-readable reason.
    #[must_use]
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }

    /// Returns the assembly failure reason.
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
}

impl fmt::Display for MachineCreateError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.reason)
    }
}

impl Error for MachineCreateError {}

/// Constructs fresh machines for the application composition root.
///
/// Repeated successful construction for one factory yields the same canonical
/// topology, component manifest, and initial profile configuration.
pub trait MachineFactory {
    /// Returns the exact machine-profile fingerprint accepted by this factory.
    fn profile_fingerprint(&self) -> ProfileFingerprint;

    /// Constructs a fresh machine with its canonical initial topology.
    ///
    /// # Errors
    ///
    /// Returns [`MachineCreateError`] when topology assembly or initial invariant
    /// validation fails.
    fn create(&self) -> Result<Box<dyn Machine>, MachineCreateError>;
}
