//! Drives a complete machine through deterministic execution boundaries.
//!
//! [`Runtime`] owns an object-safe [`Machine`] and obtains fresh machines from a
//! [`MachineFactory`]. Each CPU deadline is single-use: every CPU exit returns to
//! arbitration, while a deadline exit drains all due events at that virtual
//! instant before CPU execution resumes. Snapshot loading validates an isolated
//! candidate before replacing the current machine.
//!
//! This crate depends only on `se_core`. It contains no instruction-set, machine,
//! device, user-interface, host wall-clock, or worker-thread policy.

#![forbid(unsafe_op_in_unsafe_fn)]
#![deny(missing_docs)]

use se_core::machine::{Machine, MachineCreateError, MachineError, MachineFactory};
use se_core::snapshot::{BuildFingerprint, SnapshotError};
use se_core::time::VTime;

mod error;
mod runtime;

/// Identifies the runtime's host-visible execution state.
///
/// This state is not guest-visible and does not contribute to snapshots or the
/// machine state digest.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeState {
    /// The machine is stopped at a resumable boundary.
    Paused,
    /// A synchronous execution request is actively driving the machine.
    Running,
    /// The machine reported that guest execution halted.
    Halted,
    /// A machine drive operation failed after it may have changed state.
    Faulted,
}

/// Identifies why a resumable run request stopped.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PauseReason {
    /// A pending host-control request asked the runtime to pause.
    HostRequest,
    /// The machine stopped at a debugger breakpoint.
    Breakpoint,
}

/// Describes the normal endpoint of one explicit run request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunOutcome {
    /// Execution stopped at a resumable boundary.
    Paused(PauseReason),
    /// The machine halted and cannot resume without successful snapshot loading.
    Halted,
    /// Bounded execution reached and quiesced the requested virtual time.
    ReachedTime(VTime),
}

/// Reports a runtime construction, execution, state, or snapshot failure.
#[derive(Debug)]
pub enum RuntimeError {
    /// A machine drive or state-digest operation failed.
    Machine(MachineError),
    /// The factory could not create the initial machine.
    MachineCreate(MachineCreateError),
    /// Snapshot saving or fresh-machine loading failed.
    Snapshot(SnapshotError),
    /// A bounded run target precedes the current machine time.
    TargetBeforeNow {
        /// Current machine virtual time.
        now: VTime,
        /// Rejected earlier target time.
        target: VTime,
    },
    /// The requested operation is not valid in the current runtime state.
    InvalidState {
        /// Name of the rejected operation.
        operation: &'static str,
        /// Runtime state at the time of rejection.
        state: RuntimeState,
    },
}

/// Owns and synchronously drives one complete machine.
///
/// Execution and machine state remain on the caller's host thread. The runtime
/// does not require either itself or the machine to implement `Send` or `Sync`.
pub struct Runtime {
    machine: Box<dyn Machine>,
    factory: Box<dyn MachineFactory>,
    build: BuildFingerprint,
    state: RuntimeState,
    host_pause_requests: usize,
}
