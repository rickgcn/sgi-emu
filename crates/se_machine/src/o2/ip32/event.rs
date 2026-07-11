//! Machine-level events for the IP32 profile.
//!
//! These events represent board-level control transitions handled by machine
//! orchestration.

/// IP32 machine-level event payload.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Ip32Event {
    /// Initial board power-on event.
    PowerOn,

    /// Board reset event.
    Reset,

    /// Executes one CPU architectural boundary for the active reset epoch.
    CpuStep {
        /// Reset generation that scheduled this step.
        generation: u64,
    },
}
