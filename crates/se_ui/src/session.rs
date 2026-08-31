//! Top-level ownership of the runtime during a graphical session.

use se_runtime::runtime::{Runtime, ShutdownError};

use crate::bridge::ffi::{UiExitState, UiStartupState, run_gui};

/// Owns the emulator runtime for the lifetime of one Qt event loop.
pub struct UiSession {
    runtime: Option<Runtime>,
}

impl UiSession {
    /// Creates a session around an unconfigured runtime.
    #[must_use]
    pub fn new(runtime: Runtime) -> Self {
        Self {
            runtime: Some(runtime),
        }
    }

    /// Runs the graphical user interface until its main window closes.
    pub fn run(&mut self, startup: &UiStartupState) -> UiExitState {
        run_gui(startup)
    }

    /// Stops the runtime worker and waits for it to exit.
    pub fn shutdown(mut self) -> Result<(), ShutdownError> {
        match self.runtime.take() {
            Some(runtime) => runtime.shutdown(),
            None => Ok(()),
        }
    }
}
