//! Object-safe machine driving and construction interfaces.

use std::error::Error;
use std::fmt;

use crate::decode::DeviceDispatchError;
use crate::event::{EventQueueError, ScheduledEvent};
use crate::inspect::Introspect;
use crate::save::StateError;
use crate::snapshot::{ProfileFingerprint, SnapshotTarget};
use crate::time::VTime;

/// Reason a machine's CPU complex returned control to the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CpuExit {
    /// The CPU complex reached the requested virtual-time deadline.
    Deadline,
    /// A guest interrupt or burst truncation request ended the burst.
    Interrupt,
    /// A debugger breakpoint ended the burst.
    Breakpoint,
    /// The machine entered a halted state.
    Halted,
}

/// Canonical digest of all guest-visible machine state.
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

/// Errors returned by a machine's runtime driving surface.
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

/// Complete object-safe driving surface used by the runtime.
///
/// A future logical SMP implementation may interleave multiple guest CPUs
/// internally while retaining this single deterministic machine timeline.
pub trait Machine: SnapshotTarget + Introspect {
    /// Returns the current machine virtual time.
    fn now(&self) -> VTime;

    /// Returns the next scheduled event time after pruning cancelled entries.
    fn front_event_time(&mut self) -> Option<VTime>;

    /// Advances the complete CPU complex until a deadline or another exit reason.
    fn run_cpu_until(&mut self, deadline: VTime) -> Result<CpuExit, MachineError>;

    /// Pops the earliest event when it is due at the current machine time.
    fn pop_event(&mut self) -> Result<Option<ScheduledEvent>, MachineError>;

    /// Dispatches a previously popped event to its target device.
    fn dispatch_event(&mut self, event: ScheduledEvent) -> Result<(), MachineError>;

    /// Computes a canonical digest of guest-visible state.
    fn state_digest(&self) -> Result<StateDigest, MachineError>;
}

/// Errors produced while assembling a fresh machine instance.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MachineCreateError {
    reason: String,
}

impl MachineCreateError {
    /// Creates an assembly error with a stable human-readable reason.
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

/// Object-safe factory supplied by the application composition root.
pub trait MachineFactory {
    /// Returns the exact machine-profile fingerprint accepted by this factory.
    fn profile_fingerprint(&self) -> ProfileFingerprint;

    /// Constructs a fresh machine with its canonical initial topology.
    fn create(&self) -> Result<Box<dyn Machine>, MachineCreateError>;
}
