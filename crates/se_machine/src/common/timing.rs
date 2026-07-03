//! Machine timing ABI definitions.
//!
//! `se_core::scheduler::SimTime` is an opaque integer tick. A machine profile
//! gives that tick a fixed physical meaning by publishing a timing ABI. That ABI
//! is part of the machine model and must remain stable for deterministic replay.
//!
//! Host wall-clock time, sleeping, frame pacing, and UI refresh cadence are not
//! part of this ABI. Outer layers may map host time onto machine time, but the
//! emulator core and runtime operate only on simulated time.

/// Fixed simulated-time configuration for one machine profile.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MachineTiming {
    timebase_hz: u64,
}

impl MachineTiming {
    /// Creates machine timing with a fixed simulated-time frequency.
    pub const fn new(timebase_hz: u64) -> Self {
        Self { timebase_hz }
    }

    /// Returns the number of simulated ticks per machine second.
    pub const fn timebase_hz(self) -> u64 {
        self.timebase_hz
    }
}

#[cfg(test)]
#[path = "timing/tests.rs"]
mod tests;
