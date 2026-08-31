//! Runtime control state shared with frontends.

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

/// A coherent runtime status sample.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeStatus {
    /// Current execution state.
    pub state: RuntimeState,
    /// Monotonic debugger-visible state revision.
    pub revision: u64,
    /// Most recent execution error, when present.
    pub last_error: Option<String>,
}
