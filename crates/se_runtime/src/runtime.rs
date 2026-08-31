//! Lifetime management and commands for the host runtime worker.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::io;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use se_machine::debug::{DebugRequest, DebugResponse};
use se_machine::machine::Machine;

use crate::control::{RuntimeState, RuntimeStatus};

const EXECUTION_BATCH_SIZE: usize = 1024;

type CommandReply<T> = Sender<Result<T, CommandRejection>>;

enum Command {
    Configure {
        machine: Box<Machine>,
        reply: CommandReply<RuntimeStatus>,
    },
    Run(CommandReply<RuntimeStatus>),
    Reset(CommandReply<RuntimeStatus>),
    Pause(CommandReply<RuntimeStatus>),
    Step(CommandReply<RuntimeStatus>),
    Status(CommandReply<RuntimeStatus>),
    ToggleBreakpoint {
        address: u32,
        reply: CommandReply<RuntimeStatus>,
    },
    Debug {
        request: DebugRequest,
        reply: CommandReply<DebugReply>,
    },
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CommandRejection {
    reason: String,
}

/// A debugger response associated with one runtime revision.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DebugReply {
    /// Revision at which the response was sampled.
    pub revision: u64,
    /// Machine response.
    pub response: DebugResponse,
    /// Virtual address of the next instruction at the same boundary.
    pub execution_address: u32,
    /// Current virtual execution breakpoints.
    pub breakpoints: Vec<u32>,
}

/// Error returned by a runtime command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeError {
    /// The runtime worker is no longer available.
    WorkerUnavailable,
    /// The command is not valid in the current runtime state.
    CommandRejected {
        /// Human-readable rejection reason.
        reason: String,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerUnavailable => formatter.write_str("runtime worker is unavailable"),
            Self::CommandRejected { reason } => formatter.write_str(reason),
        }
    }
}

impl Error for RuntimeError {}

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

/// Host-side execution runtime.
pub struct Runtime {
    command_sender: Option<Sender<Command>>,
    worker: Option<JoinHandle<()>>,
}

impl Runtime {
    /// Starts a runtime worker with an optional initial machine.
    pub fn new(machine: Option<Machine>) -> io::Result<Self> {
        let (command_sender, command_receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name(String::from("sgi-emu-runtime"))
            .spawn(move || Worker::new(machine).run(&command_receiver))?;

        Ok(Self {
            command_sender: Some(command_sender),
            worker: Some(worker),
        })
    }

    /// Starts an unconfigured runtime worker.
    pub fn new_unconfigured() -> io::Result<Self> {
        Self::new(None)
    }

    /// Replaces the current machine and leaves execution paused at reset.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the worker is unavailable.
    pub fn configure(&self, machine: Machine) -> Result<RuntimeStatus, RuntimeError> {
        self.request(|reply| Command::Configure {
            machine: Box::new(machine),
            reply,
        })
    }

    /// Starts continuous execution.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when no machine is configured or the worker is unavailable.
    pub fn run(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::Run)
    }

    /// Resets the configured machine and pauses execution.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when no machine is configured or the worker is unavailable.
    pub fn reset(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::Reset)
    }

    /// Pauses continuous execution at an instruction boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when no machine is configured or the worker is unavailable.
    pub fn pause(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::Pause)
    }

    /// Executes exactly one instruction while paused.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime is not paused or the worker is unavailable.
    pub fn step(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::Step)
    }

    /// Samples runtime status.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the worker is unavailable.
    pub fn status(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::Status)
    }

    /// Adds or removes one virtual execution breakpoint.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when no machine is configured or the worker is unavailable.
    pub fn toggle_breakpoint(&self, address: u32) -> Result<RuntimeStatus, RuntimeError> {
        self.request(|reply| Command::ToggleBreakpoint { address, reply })
    }

    /// Performs one side-effect-free machine debugger query.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when no machine is configured or the worker is unavailable.
    pub fn debug(&self, request: DebugRequest) -> Result<DebugReply, RuntimeError> {
        self.request(|reply| Command::Debug { request, reply })
    }

    /// Requests shutdown and waits for the worker to exit.
    pub fn shutdown(mut self) -> Result<(), ShutdownError> {
        self.shutdown_inner()
    }

    fn request<T>(
        &self,
        make_command: impl FnOnce(CommandReply<T>) -> Command,
    ) -> Result<T, RuntimeError> {
        let sender = self
            .command_sender
            .as_ref()
            .ok_or(RuntimeError::WorkerUnavailable)?;
        let (reply_sender, reply_receiver) = mpsc::channel();
        sender
            .send(make_command(reply_sender))
            .map_err(|_| RuntimeError::WorkerUnavailable)?;
        reply_receiver
            .recv()
            .map_err(|_| RuntimeError::WorkerUnavailable)?
            .map_err(|rejection| RuntimeError::CommandRejected {
                reason: rejection.reason,
            })
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

struct Worker {
    machine: Option<Machine>,
    state: RuntimeState,
    revision: u64,
    last_error: Option<String>,
    breakpoints: BTreeSet<u32>,
    ignore_breakpoint_once: Option<u32>,
}

impl Worker {
    fn new(machine: Option<Machine>) -> Self {
        let state = if machine.is_some() {
            RuntimeState::Paused
        } else {
            RuntimeState::Unconfigured
        };
        Self {
            machine,
            state,
            revision: 0,
            last_error: None,
            breakpoints: BTreeSet::new(),
            ignore_breakpoint_once: None,
        }
    }

    fn run(mut self, receiver: &Receiver<Command>) {
        loop {
            if self.state == RuntimeState::Running {
                match receiver.try_recv() {
                    Ok(command) => {
                        if self.handle_command(command) {
                            return;
                        }
                    }
                    Err(TryRecvError::Empty) => {}
                    Err(TryRecvError::Disconnected) => return,
                }
                if self.state == RuntimeState::Running {
                    self.execute_batch();
                }
                thread::yield_now();
            } else {
                match receiver.recv() {
                    Ok(command) => {
                        if self.handle_command(command) {
                            return;
                        }
                    }
                    Err(_) => return,
                }
            }
        }
    }

    fn handle_command(&mut self, command: Command) -> bool {
        match command {
            Command::Configure { machine, reply } => {
                self.machine = Some(*machine);
                self.state = RuntimeState::Paused;
                self.last_error = None;
                self.breakpoints.clear();
                self.ignore_breakpoint_once = None;
                self.advance_revision();
                send_reply(reply, Ok(self.status()));
            }
            Command::Run(reply) => {
                let result = self.require_machine().map(|()| {
                    let address = self.machine.as_ref().map(Machine::execution_address);
                    self.ignore_breakpoint_once =
                        address.filter(|current| self.breakpoints.contains(current));
                    self.state = RuntimeState::Running;
                    self.advance_revision();
                    self.status()
                });
                send_reply(reply, result);
            }
            Command::Reset(reply) => {
                let result = self.require_machine().map(|()| {
                    if let Some(machine) = self.machine.as_mut() {
                        machine.reset();
                    }
                    self.state = RuntimeState::Paused;
                    self.last_error = None;
                    self.ignore_breakpoint_once = None;
                    self.advance_revision();
                    self.status()
                });
                send_reply(reply, result);
            }
            Command::Pause(reply) => {
                let result = self.require_machine().map(|()| {
                    self.state = RuntimeState::Paused;
                    self.ignore_breakpoint_once = None;
                    self.advance_revision();
                    self.status()
                });
                send_reply(reply, result);
            }
            Command::Step(reply) => {
                let result = self.step_once();
                send_reply(reply, result);
            }
            Command::Status(reply) => send_reply(reply, Ok(self.status())),
            Command::ToggleBreakpoint { address, reply } => {
                let result = self.require_machine().map(|()| {
                    if !self.breakpoints.remove(&address) {
                        self.breakpoints.insert(address);
                    }
                    self.advance_revision();
                    self.status()
                });
                send_reply(reply, result);
            }
            Command::Debug { request, reply } => {
                let result = self.machine.as_ref().map_or_else(
                    || Err(rejection("no machine is configured")),
                    |machine| Ok(self.debug_reply(machine.debug(request))),
                );
                send_reply(reply, result);
            }
            Command::Shutdown => return true,
        }
        false
    }

    fn step_once(&mut self) -> Result<RuntimeStatus, CommandRejection> {
        self.require_machine()?;
        if self.state == RuntimeState::Running {
            return Err(rejection("single-step requires a paused machine"));
        }

        if let Some(machine) = self.machine.as_mut() {
            match machine.execute_instruction() {
                Ok(()) => self.last_error = None,
                Err(error) => self.last_error = Some(error.to_string()),
            }
        }
        self.state = RuntimeState::Paused;
        self.ignore_breakpoint_once = None;
        self.advance_revision();
        Ok(self.status())
    }

    fn execute_batch(&mut self) {
        for _ in 0..EXECUTION_BATCH_SIZE {
            let Some(machine) = self.machine.as_mut() else {
                self.state = RuntimeState::Unconfigured;
                return;
            };
            let address = machine.execution_address();
            if self.ignore_breakpoint_once == Some(address) {
                self.ignore_breakpoint_once = None;
            } else if self.breakpoints.contains(&address) {
                self.state = RuntimeState::Paused;
                self.advance_revision();
                return;
            }

            match machine.execute_instruction() {
                Ok(()) => {
                    self.last_error = None;
                    self.advance_revision();
                }
                Err(error) => {
                    self.last_error = Some(error.to_string());
                    self.state = RuntimeState::Paused;
                    self.advance_revision();
                    return;
                }
            }
        }
    }

    fn require_machine(&self) -> Result<(), CommandRejection> {
        if self.machine.is_some() {
            Ok(())
        } else {
            Err(rejection("no machine is configured"))
        }
    }

    fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            state: self.state,
            revision: self.revision,
            last_error: self.last_error.clone(),
        }
    }

    fn debug_reply(&self, response: DebugResponse) -> DebugReply {
        DebugReply {
            revision: self.revision,
            response,
            execution_address: self.machine.as_ref().map_or(0, Machine::execution_address),
            breakpoints: self.breakpoints.iter().copied().collect(),
        }
    }

    fn advance_revision(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }
}

fn rejection(reason: &str) -> CommandRejection {
    CommandRejection {
        reason: String::from(reason),
    }
}

fn send_reply<T>(reply: CommandReply<T>, result: Result<T, CommandRejection>) {
    let _ = reply.send(result);
}

#[cfg(test)]
mod tests {
    use super::{Runtime, RuntimeError};
    use crate::control::RuntimeState;

    #[test]
    fn unconfigured_runtime_rejects_execution_and_shuts_down_cleanly() {
        let runtime = Runtime::new_unconfigured().unwrap();

        assert_eq!(runtime.status().unwrap().state, RuntimeState::Unconfigured);
        assert!(matches!(
            runtime.step(),
            Err(RuntimeError::CommandRejected { .. })
        ));
        assert_eq!(runtime.shutdown(), Ok(()));
    }
}
