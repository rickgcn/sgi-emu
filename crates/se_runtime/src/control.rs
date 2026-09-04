//! Runtime control state shared with frontends.

use crate::record::ExecutionPosition;

/// Current execution state of the emulator runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeState {
    /// No emulated machine has been constructed.
    Unconfigured,
    /// A machine exists and is stopped at an instruction boundary.
    Paused,
    /// The runtime is continuously executing instructions.
    Running,
}

/// Deterministic session mode installed with the current machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeMode {
    /// Ordinary execution without a record file.
    Normal,
    /// External inputs are being recorded.
    Recording,
    /// Recorded inputs are being replayed and checkpoints are being verified.
    Replaying,
    /// Replay reached the record footer.
    ReplayCompleted,
    /// Replay stopped at the first deterministic mismatch.
    ReplayDiverged,
}

/// A coherent runtime status sample.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeStatus {
    /// Current execution state.
    pub state: RuntimeState,
    /// Monotonic debugger-visible state revision.
    pub revision: u64,
    /// Instructions completed during this runtime's lifetime.
    pub completed_instructions: u64,
    /// Deterministic session mode.
    pub mode: RuntimeMode,
    /// Position within the current Record or Replay epoch.
    pub position: ExecutionPosition,
    /// Final Replay position from the footer, when a Replay is installed.
    pub replay_final_position: Option<ExecutionPosition>,
    /// Record failure or Replay divergence detail, when present.
    pub session_error: Option<String>,
    /// Most recent execution error, when present.
    pub last_error: Option<String>,
}
