//! Lifetime management for the host runtime worker.

use std::error::Error;
use std::fmt;
use std::io;
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};

use crate::control::RuntimeState;

enum Command {
    Shutdown,
}

/// Error returned when the runtime worker cannot be shut down cleanly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownError {
    /// The worker thread panicked before it could exit.
    WorkerPanicked,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerPanicked => formatter.write_str("runtime worker panicked during shutdown"),
        }
    }
}

impl Error for ShutdownError {}

/// Host-side runtime in the unconfigured state.
pub struct Runtime {
    state: RuntimeState,
    command_sender: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
}

impl Runtime {
    /// Starts an unconfigured runtime worker.
    pub fn new_unconfigured() -> io::Result<Self> {
        let (command_sender, command_receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(String::from("sgi-emu-runtime"))
            .spawn(move || {
                let _ = command_receiver.recv();
            })?;

        Ok(Self {
            state: RuntimeState::Unconfigured,
            command_sender: Some(command_sender),
            worker: Some(worker),
        })
    }

    /// Returns the current runtime state.
    #[must_use]
    pub const fn state(&self) -> RuntimeState {
        self.state
    }

    /// Requests shutdown and waits for the worker to exit.
    pub fn shutdown(mut self) -> Result<(), ShutdownError> {
        self.shutdown_inner()
    }

    fn shutdown_inner(&mut self) -> Result<(), ShutdownError> {
        if let Some(command_sender) = self.command_sender.take() {
            let _ = command_sender.send(Command::Shutdown);
        }
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| ShutdownError::WorkerPanicked)?;
        }
        Ok(())
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

#[cfg(test)]
mod tests {
    use super::Runtime;
    use crate::control::RuntimeState;

    #[test]
    fn unconfigured_runtime_shuts_down_cleanly() {
        let runtime = Runtime::new_unconfigured().unwrap();

        assert_eq!(runtime.state(), RuntimeState::Unconfigured);
        assert_eq!(runtime.shutdown(), Ok(()));
    }
}
