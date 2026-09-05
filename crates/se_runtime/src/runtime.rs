//! Lifetime management and commands for the host runtime worker.
//!
//! Record and Replay modes are fixed by [`RuntimeConfiguration`] when a new
//! machine is installed. They cannot be attached to a machine that has
//! already executed. All modes continue to use the same worker command queue,
//! instruction batching, and timed-instruction path.

use std::collections::{BTreeSet, VecDeque};
use std::error::Error;
use std::fmt;
use std::io;
use std::mem;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread::{self, JoinHandle};

use se_core::time::{ATTOSECONDS_PER_SECOND, VirtualDuration, VirtualInstant};
use se_machine::debug::{DebugRequest, DebugResponse};
use se_machine::machine::{ExecutionError, Machine, MachineNonvolatileState};
use se_machine::output::MachineOutput;
use se_machine::serial::SerialPort;

use crate::control::{RuntimeMode, RuntimeState, RuntimeStatus};
use crate::record::{
    ExecutionPosition, RecordOutcome, Recorder, ReplaySession, Replayer, TimelineAction,
};

type CommandReply<T> = Sender<Result<T, CommandRejection>>;

fn checkpoint_digest(machine: &Machine) -> [u8; 32] {
    let DebugResponse::MachineStateFingerprint(digest) =
        machine.debug(DebugRequest::MachineStateFingerprint)
    else {
        unreachable!("machine state fingerprint request returned the wrong response")
    };
    digest
}

enum Command {
    Configure {
        configuration: Box<RuntimeConfiguration>,
        reply: CommandReply<RuntimeStatus>,
    },
    Run(CommandReply<RuntimeStatus>),
    Reset(CommandReply<RuntimeStatus>),
    Pause(CommandReply<RuntimeStatus>),
    Step(CommandReply<RuntimeStatus>),
    StopRecording(CommandReply<RuntimeStatus>),
    CreateReplaySnapshot(CommandReply<RuntimeStatus>),
    Status(CommandReply<RuntimeStatus>),
    ToggleBreakpoint {
        address: u32,
        reply: CommandReply<RuntimeStatus>,
    },
    Debug {
        request: DebugRequest,
        reply: CommandReply<DebugReply>,
    },
    SendSerial {
        port: SerialPort,
        bytes: Vec<u8>,
        reply: CommandReply<RuntimeStatus>,
    },
    SetOutputHandler {
        handler: Box<dyn FnMut(MachineOutput) + Send + 'static>,
        reply: CommandReply<RuntimeStatus>,
    },
    ClearOutputHandler(CommandReply<RuntimeStatus>),
    Shutdown(Sender<Option<MachineNonvolatileState>>),
}

/// Complete cold-constructed machine and deterministic session mode.
pub struct RuntimeConfiguration {
    machine: Machine,
    mode: RuntimeConfigurationMode,
}

enum RuntimeConfigurationMode {
    Normal,
    Recording(Recorder),
    Replaying(Box<Replayer>),
}

impl RuntimeConfiguration {
    /// Creates an ordinary machine configuration.
    #[must_use]
    pub fn normal(machine: Machine) -> Self {
        Self {
            machine,
            mode: RuntimeConfigurationMode::Normal,
        }
    }

    /// Creates a cold-start recording configuration.
    #[must_use]
    pub fn recording(machine: Machine, recorder: Recorder) -> Self {
        Self {
            machine,
            mode: RuntimeConfigurationMode::Recording(recorder),
        }
    }

    /// Creates a cold-start replay configuration.
    #[must_use]
    pub fn replaying(machine: Machine, replayer: Replayer) -> Self {
        Self {
            machine,
            mode: RuntimeConfigurationMode::Replaying(Box::new(replayer)),
        }
    }
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
    /// The worker stopped before returning its final machine state.
    WorkerUnavailable,
    /// The worker thread panicked before it could exit.
    WorkerPanicked,
}

impl fmt::Display for ShutdownError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WorkerUnavailable => {
                formatter.write_str("runtime worker stopped before returning machine state")
            }
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
        self.configure_with(RuntimeConfiguration::normal(machine))
    }

    /// Replaces the current machine with a complete cold-start configuration.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when Record/Replay initialization fails or the
    /// worker is unavailable.
    pub fn configure_with(
        &self,
        configuration: RuntimeConfiguration,
    ) -> Result<RuntimeStatus, RuntimeError> {
        self.request(|reply| Command::Configure {
            configuration: Box::new(configuration),
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

    /// Finalizes the active Record and keeps the current execution state.
    ///
    /// This method flushes and synchronizes the Record file before returning
    /// and may therefore block on host file I/O.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the runtime is not recording, finalizing
    /// the file fails, or the worker is unavailable.
    pub fn stop_recording(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::StopRecording)
    }

    /// Creates one restorable checkpoint at the current paused Replay
    /// boundary.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] unless an active healthy Replay is paused, or
    /// when machine capture or atomic cache output fails.
    pub fn create_replay_snapshot(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::CreateReplaySnapshot)
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

    /// Supplies host bytes to one external serial port.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when no machine is configured or the worker is unavailable.
    pub fn send_serial(
        &self,
        port: SerialPort,
        bytes: &[u8],
    ) -> Result<RuntimeStatus, RuntimeError> {
        self.request(|reply| Command::SendSerial {
            port,
            bytes: bytes.to_vec(),
            reply,
        })
    }

    /// Installs the frontend-neutral machine-output handler.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the worker is unavailable.
    pub fn set_output_handler(
        &self,
        handler: Box<dyn FnMut(MachineOutput) + Send + 'static>,
    ) -> Result<RuntimeStatus, RuntimeError> {
        self.request(|reply| Command::SetOutputHandler { handler, reply })
    }

    /// Removes the current machine-output handler.
    ///
    /// Once this method returns, the removed handler cannot be called again.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError`] when the worker is unavailable.
    pub fn clear_output_handler(&self) -> Result<RuntimeStatus, RuntimeError> {
        self.request(Command::ClearOutputHandler)
    }

    /// Requests shutdown and waits for the worker to exit.
    pub fn shutdown(mut self) -> Result<Option<MachineNonvolatileState>, ShutdownError> {
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

    fn shutdown_inner(&mut self) -> Result<Option<MachineNonvolatileState>, ShutdownError> {
        let mut final_state = None;
        let mut state_unavailable = false;
        if let Some(command_sender) = self.command_sender.take() {
            let (reply_sender, reply_receiver) = mpsc::channel();
            if command_sender
                .send(Command::Shutdown(reply_sender))
                .is_err()
            {
                state_unavailable = true;
            } else {
                match reply_receiver.recv() {
                    Ok(state) => final_state = state,
                    Err(_) => state_unavailable = true,
                }
            }
        }
        if let Some(worker) = self.worker.take() {
            worker.join().map_err(|_| ShutdownError::WorkerPanicked)?;
        }
        if state_unavailable {
            return Err(ShutdownError::WorkerUnavailable);
        }
        Ok(final_state)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        let _ = self.shutdown_inner();
    }
}

const EXECUTION_BATCH_SIZE: usize = 1024;
const CHECKPOINT_INTERVAL: u64 = 1_000_000;

struct Worker {
    machine: Option<Machine>,
    cpu_clock: Option<CpuClock>,
    virtual_instant: VirtualInstant,
    frontend_output: MachineOutput,
    output_handler: Option<Box<dyn FnMut(MachineOutput) + Send + 'static>>,
    pending_serial: [VecDeque<u8>; 2],
    state: RuntimeState,
    mode: ActiveMode,
    position: ExecutionPosition,
    preserved_nonvolatile_state: Option<MachineNonvolatileState>,
    revision: u64,
    completed_instructions: u64,
    last_error: Option<String>,
    session_error: Option<String>,
    breakpoints: BTreeSet<u32>,
    ignore_breakpoint_once: Option<u32>,
}

enum ActiveMode {
    Normal,
    Recording(RecordingSession),
    Replaying(ReplaySession),
    ReplayCompleted(ReplaySession),
    ReplayDiverged {
        session: ReplaySession,
        reason: String,
    },
}

struct RecordingSession {
    recorder: Recorder,
    next_checkpoint_instruction: Option<u64>,
}

impl RecordingSession {
    const fn new(recorder: Recorder) -> Self {
        Self {
            recorder,
            next_checkpoint_instruction: Some(CHECKPOINT_INTERVAL),
        }
    }

    fn checkpoint_due(&self, position: ExecutionPosition) -> bool {
        self.next_checkpoint_instruction == Some(position.completed_instructions)
    }

    fn advance_checkpoint_deadline(&mut self) {
        self.next_checkpoint_instruction = self
            .next_checkpoint_instruction
            .and_then(|deadline| deadline.checked_add(CHECKPOINT_INTERVAL));
    }

    const fn reset_checkpoint_deadline(&mut self) {
        self.next_checkpoint_instruction = Some(CHECKPOINT_INTERVAL);
    }
}

impl ActiveMode {
    const fn public_mode(&self) -> RuntimeMode {
        match self {
            Self::Normal => RuntimeMode::Normal,
            Self::Recording(_) => RuntimeMode::Recording,
            Self::Replaying(_) => RuntimeMode::Replaying,
            Self::ReplayCompleted(_) => RuntimeMode::ReplayCompleted,
            Self::ReplayDiverged { .. } => RuntimeMode::ReplayDiverged,
        }
    }

    const fn is_replay(&self) -> bool {
        matches!(
            self,
            Self::Replaying(_) | Self::ReplayCompleted(_) | Self::ReplayDiverged { .. }
        )
    }
}

impl Worker {
    fn new(machine: Option<Machine>) -> Self {
        let state = if machine.is_some() {
            RuntimeState::Paused
        } else {
            RuntimeState::Unconfigured
        };
        let cpu_clock = machine
            .as_ref()
            .map(|machine| CpuClock::new(machine.cpu_frequency_hz()));
        Self {
            machine,
            cpu_clock,
            virtual_instant: VirtualInstant::ZERO,
            frontend_output: MachineOutput::default(),
            output_handler: None,
            pending_serial: [VecDeque::new(), VecDeque::new()],
            state,
            mode: ActiveMode::Normal,
            position: ExecutionPosition::default(),
            preserved_nonvolatile_state: None,
            revision: 0,
            completed_instructions: 0,
            last_error: None,
            session_error: None,
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
            Command::Configure {
                configuration,
                reply,
            } => send_reply(reply, self.configure(*configuration)),
            Command::Run(reply) => {
                let result = self.require_runnable().map(|()| {
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
                let result = self.require_manual_reset().and_then(|()| {
                    let next_epoch = if matches!(self.mode, ActiveMode::Recording(_)) {
                        Some(
                            self.position
                                .epoch
                                .checked_add(1)
                                .ok_or_else(|| rejection("Record epoch counter overflow"))?,
                        )
                    } else {
                        None
                    };
                    if let ActiveMode::Recording(session) = &self.mode {
                        session
                            .recorder
                            .record_reset(self.position)
                            .map_err(|error| rejection_owned(error.to_string()))?;
                    }
                    self.reset_machine();
                    if let Some(next_epoch) = next_epoch {
                        self.position = ExecutionPosition {
                            epoch: next_epoch,
                            completed_instructions: 0,
                        };
                    }
                    self.state = RuntimeState::Paused;
                    self.record_checkpoint()?;
                    if let ActiveMode::Recording(session) = &mut self.mode {
                        session.reset_checkpoint_deadline();
                    }
                    self.advance_revision();
                    Ok(self.status())
                });
                self.check_record_failure();
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
            Command::StopRecording(reply) => {
                let result = self.stop_recording(RecordOutcome::UserStopped);
                self.check_record_failure();
                send_reply(reply, result);
            }
            Command::CreateReplaySnapshot(reply) => {
                let result = self.create_replay_snapshot();
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
            Command::SendSerial { port, bytes, reply } => {
                let result = self.require_live_serial().and_then(|()| {
                    self.pending_serial[serial_port_index(port)].extend(bytes);
                    self.refill_serial_input()?;
                    self.advance_revision();
                    Ok(self.status())
                });
                self.check_record_failure();
                send_reply(reply, result);
            }
            Command::SetOutputHandler { handler, reply } => {
                self.output_handler = Some(handler);
                send_reply(reply, Ok(self.status()));
            }
            Command::ClearOutputHandler(reply) => {
                self.output_handler = None;
                send_reply(reply, Ok(self.status()));
            }
            Command::Shutdown(reply) => {
                if matches!(self.mode, ActiveMode::Recording(_)) {
                    let _ = self.stop_recording(RecordOutcome::Shutdown);
                }
                let state = if self.mode.is_replay() {
                    self.preserved_nonvolatile_state.clone()
                } else {
                    self.machine.as_ref().map(Machine::nonvolatile_state)
                };
                let _ = reply.send(state);
                return true;
            }
        }
        false
    }

    fn configure(
        &mut self,
        configuration: RuntimeConfiguration,
    ) -> Result<RuntimeStatus, CommandRejection> {
        if matches!(self.mode, ActiveMode::Recording(_)) {
            return Err(rejection(
                "the current recording must be stopped before configuring another machine",
            ));
        }
        if self.mode.is_replay() && !matches!(&configuration.mode, RuntimeConfigurationMode::Normal)
        {
            return Err(rejection(
                "the current Replay must be stopped before starting another session",
            ));
        }
        let RuntimeConfiguration { mut machine, mode } = configuration;
        let retained_state = if self.mode.is_replay() {
            self.preserved_nonvolatile_state.clone()
        } else {
            self.machine.as_ref().map(Machine::nonvolatile_state)
        };
        let mut next_preserved_state = None;
        let mut restore_state = None;
        let next_mode = match mode {
            RuntimeConfigurationMode::Normal => {
                if let Some(state) = retained_state {
                    machine.restore_nonvolatile_state(state, 0);
                }
                ActiveMode::Normal
            }
            RuntimeConfigurationMode::Recording(recorder) => {
                let digest = checkpoint_digest(&machine);
                recorder
                    .record_checkpoint(ExecutionPosition::default(), digest)
                    .map_err(|error| rejection_owned(error.to_string()))?;
                ActiveMode::Recording(RecordingSession::new(recorder))
            }
            RuntimeConfigurationMode::Replaying(replayer) => {
                next_preserved_state = retained_state;
                let (session, restore) = (*replayer).into_session();
                restore_state = restore;
                ActiveMode::Replaying(session)
            }
        };
        let mut cpu_clock = CpuClock::new(machine.cpu_frequency_hz());
        let mut virtual_instant = VirtualInstant::ZERO;
        let mut position = ExecutionPosition::default();
        let mut completed_instructions = self.completed_instructions;
        if let Some(restore) = restore_state {
            cpu_clock
                .restore_remainder(restore.cpu_clock_remainder)
                .map_err(rejection_owned)?;
            machine
                .restore_snapshot(restore.machine)
                .map_err(|error| rejection_owned(error.to_string()))?;
            if checkpoint_digest(&machine) != restore.machine_fingerprint {
                return Err(rejection("Replay snapshot machine fingerprint mismatch"));
            }
            virtual_instant = restore.virtual_instant;
            position = restore.position;
            completed_instructions = restore.completed_instructions;
        }
        self.cpu_clock = Some(cpu_clock);
        self.machine = Some(machine);
        self.virtual_instant = virtual_instant;
        self.frontend_output = MachineOutput::default();
        self.pending_serial = [VecDeque::new(), VecDeque::new()];
        self.state = RuntimeState::Paused;
        self.mode = next_mode;
        self.position = position;
        self.completed_instructions = completed_instructions;
        self.preserved_nonvolatile_state = next_preserved_state;
        self.last_error = None;
        self.session_error = None;
        self.breakpoints.clear();
        self.ignore_breakpoint_once = None;
        if matches!(self.mode, ActiveMode::Replaying(_))
            && let Err(reason) = self.process_replay_boundary()
        {
            self.set_replay_divergence(reason);
        }
        self.advance_revision();
        Ok(self.status())
    }

    fn stop_recording(
        &mut self,
        outcome: RecordOutcome,
    ) -> Result<RuntimeStatus, CommandRejection> {
        let recorder = match &self.mode {
            ActiveMode::Recording(session) => session.recorder.clone(),
            _ => return Err(rejection("no recording is active")),
        };
        self.record_checkpoint()?;
        if let Err(error) = recorder.finalize(self.position, &outcome) {
            let reason = error.to_string();
            self.set_record_failure(recorder, reason.clone());
            return Err(rejection_owned(reason));
        }
        self.mode = ActiveMode::Normal;
        self.session_error = None;
        self.advance_revision();
        Ok(self.status())
    }

    fn create_replay_snapshot(&mut self) -> Result<RuntimeStatus, CommandRejection> {
        if self.state != RuntimeState::Paused {
            return Err(rejection(
                "Replay must be paused before creating a snapshot",
            ));
        }
        let ActiveMode::Replaying(session) = &self.mode else {
            return Err(rejection("no active Replay is available for a snapshot"));
        };
        if let Some(failure) = session.storage_failure() {
            return Err(rejection_owned(failure));
        }
        let machine = self
            .machine
            .as_ref()
            .ok_or_else(|| rejection("no machine is configured"))?;
        let machine_snapshot = machine
            .snapshot()
            .map_err(|error| rejection_owned(error.to_string()))?;
        let fingerprint = checkpoint_digest(machine);
        let pc = machine.execution_address();
        let cpu_clock_remainder = self
            .cpu_clock
            .as_ref()
            .expect("a configured machine has a CPU clock")
            .accumulated_remainder;
        session
            .create_snapshot(
                self.position,
                self.completed_instructions,
                self.virtual_instant,
                cpu_clock_remainder,
                fingerprint,
                machine_snapshot,
                pc,
            )
            .map_err(|error| rejection_owned(error.to_string()))?;
        self.advance_revision();
        Ok(self.status())
    }

    fn process_replay_boundary(&mut self) -> Result<(), String> {
        loop {
            let action = match &self.mode {
                ActiveMode::Replaying(session) => match session.next_entry() {
                    Some(entry) if entry.position < self.position => {
                        return Err(format!(
                            "Replay passed Timeline entry at epoch {}, instruction {}",
                            entry.position.epoch, entry.position.completed_instructions
                        ));
                    }
                    Some(entry) if entry.position == self.position => Some(entry.action.clone()),
                    _ => None,
                },
                _ => return Ok(()),
            };
            let Some(action) = action else {
                break;
            };
            let ActiveMode::Replaying(session) = &mut self.mode else {
                return Ok(());
            };
            session.advance();
            match action {
                TimelineAction::SerialByte { port, value } => {
                    let consumed = self
                        .machine
                        .as_mut()
                        .expect("Replay requires a configured machine")
                        .receive_serial(port, &[value]);
                    if consumed != 1 {
                        return Err(format!(
                            "Replay serial byte was not accepted at epoch {}, instruction {}",
                            self.position.epoch, self.position.completed_instructions
                        ));
                    }
                }
                TimelineAction::Reset => {
                    let next_epoch = self
                        .position
                        .epoch
                        .checked_add(1)
                        .ok_or_else(|| String::from("Replay epoch counter overflow"))?;
                    self.reset_machine();
                    self.position = ExecutionPosition {
                        epoch: next_epoch,
                        completed_instructions: 0,
                    };
                }
                TimelineAction::Checkpoint { digest } => {
                    let actual = checkpoint_digest(
                        self.machine
                            .as_ref()
                            .expect("Replay requires a configured machine"),
                    );
                    if actual != digest {
                        return Err(format!(
                            "Replay checkpoint mismatch at epoch {}, instruction {}",
                            self.position.epoch, self.position.completed_instructions
                        ));
                    }
                }
            }
        }

        let complete = match &self.mode {
            ActiveMode::Replaying(session) => {
                if self.position > session.final_position() {
                    return Err(String::from("Replay passed its final position"));
                }
                if self.position != session.final_position() {
                    false
                } else {
                    if !session.timeline_consumed() {
                        return Err(String::from(
                            "Replay reached its footer before consuming the Timeline",
                        ));
                    }
                    matches!(
                        session.outcome(),
                        RecordOutcome::UserStopped | RecordOutcome::Shutdown
                    )
                }
            }
            _ => return Ok(()),
        };
        if complete {
            self.complete_replay();
        }
        Ok(())
    }

    fn replay_boundary_due(&self) -> bool {
        match &self.mode {
            ActiveMode::Replaying(session) => self.position >= session.next_boundary_position(),
            _ => false,
        }
    }

    fn record_checkpoint(&self) -> Result<(), CommandRejection> {
        let ActiveMode::Recording(session) = &self.mode else {
            return Ok(());
        };
        let digest = checkpoint_digest(
            self.machine
                .as_ref()
                .expect("a deterministic session requires a machine"),
        );
        session
            .recorder
            .record_checkpoint(self.position, digest)
            .map_err(|error| rejection_owned(error.to_string()))
    }

    fn record_checkpoint_if_due(&mut self) -> Result<(), CommandRejection> {
        let due = match &self.mode {
            ActiveMode::Recording(session) => session.checkpoint_due(self.position),
            _ => false,
        };
        if !due {
            return Ok(());
        }
        self.record_checkpoint()?;
        let ActiveMode::Recording(session) = &mut self.mode else {
            unreachable!("a successful Recording checkpoint preserves the session mode");
        };
        session.advance_checkpoint_deadline();
        Ok(())
    }

    fn handle_execution_error(&mut self, error: ExecutionError) {
        let error_text = error.to_string();
        let address = self.machine.as_ref().map_or(0, Machine::execution_address);
        self.last_error = Some(error_text.clone());
        self.check_record_failure();
        self.check_replay_storage_failure();

        if matches!(self.mode, ActiveMode::Replaying(_)) {
            let expected = match &self.mode {
                ActiveMode::Replaying(session) => {
                    self.position == session.final_position()
                        && session.timeline_consumed()
                        && matches!(
                            session.outcome(),
                            RecordOutcome::ExecutionError {
                                address: expected_address,
                                description,
                            } if *expected_address == address && description == &error_text
                        )
                }
                _ => false,
            };
            if expected {
                self.complete_replay();
            } else {
                self.set_replay_divergence(format!(
                    "unexpected execution error at 0x{address:08x}: {error_text}"
                ));
            }
            return;
        }

        if matches!(self.mode, ActiveMode::Recording(_)) {
            let outcome = RecordOutcome::ExecutionError {
                address,
                description: error_text,
            };
            if let Err(rejection) = self.stop_recording(outcome) {
                self.session_error = Some(rejection.reason);
            }
        }
    }

    fn check_record_failure(&mut self) {
        let failure = match &self.mode {
            ActiveMode::Recording(session) => session
                .recorder
                .failure()
                .map(|reason| (session.recorder.clone(), reason)),
            _ => None,
        };
        if let Some((recorder, reason)) = failure {
            self.set_record_failure(recorder, reason);
        }
    }

    fn check_replay_storage_failure(&mut self) {
        let failure = match &self.mode {
            ActiveMode::Replaying(session) => session.storage_failure(),
            _ => None,
        };
        if let Some(reason) = failure {
            self.set_replay_divergence(reason);
        }
    }

    fn set_record_failure(&mut self, recorder: Recorder, reason: String) {
        recorder.disable();
        self.state = RuntimeState::Paused;
        self.mode = ActiveMode::Normal;
        self.session_error = Some(reason);
        self.advance_revision();
    }

    fn set_replay_divergence(&mut self, reason: String) {
        if let ActiveMode::Replaying(session) = mem::replace(&mut self.mode, ActiveMode::Normal) {
            self.state = RuntimeState::Paused;
            self.session_error = Some(reason.clone());
            self.mode = ActiveMode::ReplayDiverged { session, reason };
            self.advance_revision();
        }
    }

    fn complete_replay(&mut self) {
        if let ActiveMode::Replaying(session) = mem::replace(&mut self.mode, ActiveMode::Normal) {
            self.state = RuntimeState::Paused;
            self.session_error = None;
            self.mode = ActiveMode::ReplayCompleted(session);
            self.advance_revision();
        }
    }

    fn fail_session(&mut self, reason: String) {
        match mem::replace(&mut self.mode, ActiveMode::Normal) {
            ActiveMode::Recording(session) => {
                self.set_record_failure(session.recorder, reason);
            }
            ActiveMode::Replaying(session) => {
                self.state = RuntimeState::Paused;
                self.session_error = Some(reason.clone());
                self.mode = ActiveMode::ReplayDiverged { session, reason };
                self.advance_revision();
            }
            mode => self.mode = mode,
        }
    }

    fn reset_machine(&mut self) {
        self.machine
            .as_mut()
            .expect("reset requires a configured machine")
            .reset();
        self.cpu_clock
            .as_mut()
            .expect("reset requires a CPU clock")
            .reset();
        self.virtual_instant = VirtualInstant::ZERO;
        self.frontend_output = MachineOutput::default();
        self.pending_serial = [VecDeque::new(), VecDeque::new()];
        self.last_error = None;
        self.ignore_breakpoint_once = None;
    }

    fn step_once(&mut self) -> Result<RuntimeStatus, CommandRejection> {
        self.require_runnable()?;
        if self.state == RuntimeState::Running {
            return Err(rejection("single-step requires a paused machine"));
        }

        match self.execute_timed_instruction() {
            Ok(()) => self.last_error = None,
            Err(error) => self.handle_execution_error(error),
        }
        self.state = RuntimeState::Paused;
        self.ignore_breakpoint_once = None;
        self.advance_revision();
        Ok(self.status())
    }

    fn execute_batch(&mut self) {
        match self.mode.public_mode() {
            RuntimeMode::Normal => self.execute_normal_batch(),
            RuntimeMode::Recording => self.execute_recording_batch(),
            RuntimeMode::Replaying => self.execute_replay_batch(),
            RuntimeMode::ReplayCompleted | RuntimeMode::ReplayDiverged => {}
        }
    }

    fn execute_normal_batch(&mut self) {
        for _ in 0..EXECUTION_BATCH_SIZE {
            if self.pause_for_breakpoint() {
                return;
            }
            match self.execute_normal_instruction() {
                Ok(()) => {
                    self.last_error = None;
                    self.advance_revision();
                }
                Err(error) => {
                    self.handle_execution_error(error);
                    self.state = RuntimeState::Paused;
                    self.advance_revision();
                    return;
                }
            }
        }
    }

    fn execute_recording_batch(&mut self) {
        for _ in 0..EXECUTION_BATCH_SIZE {
            if self.pause_for_breakpoint() {
                return;
            }
            match self.execute_recording_instruction() {
                Ok(()) => {
                    self.last_error = None;
                    self.advance_revision();
                    if self.state != RuntimeState::Running {
                        return;
                    }
                }
                Err(error) => {
                    self.handle_execution_error(error);
                    self.state = RuntimeState::Paused;
                    self.advance_revision();
                    return;
                }
            }
        }
    }

    fn execute_replay_batch(&mut self) {
        for _ in 0..EXECUTION_BATCH_SIZE {
            if self.pause_for_breakpoint() {
                return;
            }
            match self.execute_replay_instruction() {
                Ok(()) => {
                    self.last_error = None;
                    self.advance_revision();
                    if self.state != RuntimeState::Running {
                        return;
                    }
                }
                Err(error) => {
                    self.handle_execution_error(error);
                    self.state = RuntimeState::Paused;
                    self.advance_revision();
                    return;
                }
            }
        }
    }

    fn execute_timed_instruction(&mut self) -> Result<(), ExecutionError> {
        match self.mode.public_mode() {
            RuntimeMode::Normal => self.execute_normal_instruction(),
            RuntimeMode::Recording => self.execute_recording_instruction(),
            RuntimeMode::Replaying => self.execute_replay_instruction(),
            RuntimeMode::ReplayCompleted | RuntimeMode::ReplayDiverged => Ok(()),
        }
    }

    fn execute_normal_instruction(&mut self) -> Result<(), ExecutionError> {
        self.execute_machine_instruction()?;
        if let Err(error) = self.refill_serial_input() {
            unreachable!(
                "Normal serial input cannot write a Record: {}",
                error.reason
            );
        }
        self.deliver_output();
        Ok(())
    }

    fn execute_recording_instruction(&mut self) -> Result<(), ExecutionError> {
        self.execute_machine_instruction()?;
        if !self.advance_session_position() {
            return Ok(());
        }
        if let Err(error) = self.refill_serial_input() {
            self.fail_session(error.reason);
            self.deliver_output();
            return Ok(());
        }
        self.check_record_failure();
        if let Err(error) = self.record_checkpoint_if_due() {
            self.fail_session(error.reason);
        }
        self.deliver_output();
        Ok(())
    }

    fn execute_replay_instruction(&mut self) -> Result<(), ExecutionError> {
        let expected_execution_error = match &self.mode {
            ActiveMode::Replaying(session) => {
                self.position == session.final_position()
                    && session.timeline_consumed()
                    && matches!(session.outcome(), RecordOutcome::ExecutionError { .. })
            }
            _ => false,
        };
        let instruction_result = self
            .machine
            .as_mut()
            .expect("Replay requires a configured machine")
            .execute_instruction();
        if instruction_result.is_ok() && expected_execution_error {
            self.set_replay_divergence(String::from(
                "Replay expected the recorded execution error, but the instruction succeeded",
            ));
            return Ok(());
        }
        instruction_result?;
        self.finish_machine_instruction();
        if !self.advance_session_position() {
            return Ok(());
        }
        self.check_replay_storage_failure();
        if self.replay_boundary_due()
            && let Err(reason) = self.process_replay_boundary()
        {
            self.set_replay_divergence(reason);
        }
        self.deliver_output();
        Ok(())
    }

    fn execute_machine_instruction(&mut self) -> Result<(), ExecutionError> {
        self.machine
            .as_mut()
            .expect("a timed instruction requires a configured machine")
            .execute_instruction()?;
        self.finish_machine_instruction();
        Ok(())
    }

    fn finish_machine_instruction(&mut self) {
        let elapsed = self
            .cpu_clock
            .as_mut()
            .expect("a configured machine requires a CPU clock")
            .advance_cycle();
        self.virtual_instant.advance(elapsed);
        self.machine
            .as_mut()
            .expect("a timed instruction requires a configured machine")
            .advance_time(elapsed, &mut self.frontend_output);
        self.completed_instructions = self.completed_instructions.wrapping_add(1);
    }

    fn advance_session_position(&mut self) -> bool {
        let Some(next) = self.position.completed_instructions.checked_add(1) else {
            self.fail_session(String::from("Record/Replay instruction counter overflow"));
            return false;
        };
        self.position.completed_instructions = next;
        true
    }

    fn pause_for_breakpoint(&mut self) -> bool {
        let Some(machine) = self.machine.as_ref() else {
            self.state = RuntimeState::Unconfigured;
            return true;
        };
        let address = machine.execution_address();
        if self.ignore_breakpoint_once == Some(address) {
            self.ignore_breakpoint_once = None;
            false
        } else if self.breakpoints.contains(&address) {
            self.state = RuntimeState::Paused;
            self.advance_revision();
            true
        } else {
            false
        }
    }

    fn refill_serial_input(&mut self) -> Result<(), CommandRejection> {
        if self.pending_serial[0].is_empty() && self.pending_serial[1].is_empty() {
            return Ok(());
        }

        let recorder = match &self.mode {
            ActiveMode::Recording(session) => Some(session.recorder.clone()),
            _ => None,
        };
        for (index, port) in [SerialPort::A, SerialPort::B].into_iter().enumerate() {
            let pending = &mut self.pending_serial[index];
            if pending.is_empty() {
                continue;
            }

            let consumed = self
                .machine
                .as_mut()
                .expect("serial input requires a configured machine")
                .receive_serial(port, pending.make_contiguous());
            if consumed != 0 {
                if let Some(recorder) = &recorder {
                    for value in pending.iter().take(consumed).copied() {
                        recorder
                            .record_serial_byte(self.position, port, value)
                            .map_err(|error| rejection_owned(error.to_string()))?;
                    }
                }
                pending.drain(..consumed);
            }
        }
        Ok(())
    }

    fn deliver_output(&mut self) {
        if self.frontend_output.is_empty() {
            return;
        }

        let output = mem::take(&mut self.frontend_output);
        if let Some(handler) = self.output_handler.as_mut() {
            handler(output);
        }
    }

    fn require_machine(&self) -> Result<(), CommandRejection> {
        if self.machine.is_some() {
            Ok(())
        } else {
            Err(rejection("no machine is configured"))
        }
    }

    fn require_runnable(&self) -> Result<(), CommandRejection> {
        self.require_machine()?;
        match self.mode {
            ActiveMode::ReplayCompleted(_) => Err(rejection("replay is complete")),
            ActiveMode::ReplayDiverged { .. } => Err(rejection("replay has diverged")),
            _ => Ok(()),
        }
    }

    fn require_manual_reset(&self) -> Result<(), CommandRejection> {
        self.require_machine()?;
        if self.mode.is_replay() {
            Err(rejection("manual reset is disabled during replay"))
        } else {
            Ok(())
        }
    }

    fn require_live_serial(&self) -> Result<(), CommandRejection> {
        self.require_machine()?;
        if self.mode.is_replay() {
            Err(rejection("live serial input is disabled during replay"))
        } else {
            Ok(())
        }
    }

    fn status(&self) -> RuntimeStatus {
        let session_error = match &self.mode {
            ActiveMode::ReplayDiverged { reason, .. } => Some(reason.clone()),
            _ => self.session_error.clone(),
        };
        let replay_final_position = match &self.mode {
            ActiveMode::Replaying(session)
            | ActiveMode::ReplayCompleted(session)
            | ActiveMode::ReplayDiverged { session, .. } => Some(session.final_position()),
            _ => None,
        };
        RuntimeStatus {
            state: self.state,
            revision: self.revision,
            completed_instructions: self.completed_instructions,
            mode: self.mode.public_mode(),
            position: self.position,
            replay_final_position,
            session_error,
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

struct CpuClock {
    frequency_hz: u128,
    whole_attoseconds_per_cycle: u128,
    remainder_per_cycle: u128,
    accumulated_remainder: u128,
}

impl CpuClock {
    const fn new(frequency_hz: u64) -> Self {
        assert!(frequency_hz != 0);
        let frequency_hz = frequency_hz as u128;
        Self {
            frequency_hz,
            whole_attoseconds_per_cycle: ATTOSECONDS_PER_SECOND / frequency_hz,
            remainder_per_cycle: ATTOSECONDS_PER_SECOND % frequency_hz,
            accumulated_remainder: 0,
        }
    }

    fn reset(&mut self) {
        self.accumulated_remainder = 0;
    }

    fn restore_remainder(&mut self, remainder: u128) -> Result<(), String> {
        if remainder >= self.frequency_hz {
            return Err(String::from(
                "Replay snapshot CPU clock remainder is out of range",
            ));
        }
        self.accumulated_remainder = remainder;
        Ok(())
    }

    fn advance_cycle(&mut self) -> VirtualDuration {
        let accumulated_remainder = self.accumulated_remainder + self.remainder_per_cycle;
        let carry = if accumulated_remainder >= self.frequency_hz {
            self.accumulated_remainder = accumulated_remainder - self.frequency_hz;
            1
        } else {
            self.accumulated_remainder = accumulated_remainder;
            0
        };
        VirtualDuration::from_attoseconds(self.whole_attoseconds_per_cycle + carry)
    }
}

fn rejection(reason: &str) -> CommandRejection {
    CommandRejection {
        reason: String::from(reason),
    }
}

fn rejection_owned(reason: String) -> CommandRejection {
    CommandRejection { reason }
}

const fn serial_port_index(port: SerialPort) -> usize {
    match port {
        SerialPort::A => 0,
        SerialPort::B => 1,
    }
}

fn send_reply<T>(reply: CommandReply<T>, result: Result<T, CommandRejection>) {
    let _ = reply.send(result);
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, mpsc};

    use se_core::time::ATTOSECONDS_PER_SECOND;
    use se_device::storage::BlockStorage;
    use se_float::backend::Backend;
    use se_machine::indigo::ip12::debug::{DebugRequest, DebugResponse, MemoryAddressSpace};
    use se_machine::indigo::ip12::{
        Ip12, Ip12MemoryConfiguration, Ip12NonvolatileState, Ip12NonvolatileStateParts,
    };
    use se_machine::machine::{Machine, MachineNonvolatileState, MachineStartupConfiguration};
    use se_machine::serial::SerialPort;

    use super::{
        CpuClock, Runtime, RuntimeConfiguration, RuntimeError, checkpoint_digest, serial_port_index,
    };
    use crate::control::{RuntimeMode, RuntimeState};
    use crate::record::{ExecutionPosition, MediaIdentity, RecordManifest, Recorder, Replayer};

    const PROM_BYTES: usize = 0x40000;
    const EXTERNAL_PROM_EXECUTION_BUDGET: usize = 400_000_000;
    const EXPECTED_PROMPT_INSTRUCTION: usize = 376_277_796;
    const POST_PROMPT_INSTRUCTIONS: usize = 2_000_000;
    const P5_START_PC: u32 = 0xbfc0_0fb0;
    const P5_COMPLETE_PC: u32 = 0xbfc0_1020;
    const HEADLESS_GRAPHICS_PROBE_PC: u32 = 0xbfc1_e69c;
    const SERIAL_RECEIVE_POLL_PC: u32 = 0xbfc2_28a0;
    const VOLUME_HEADER_MAGIC_CHECK_PC: u32 = 0xbfc2_b3fc;
    const VOLUME_HEADER_CHECKSUM_PC: u32 = 0xbfc2_b45c;
    const CPU_FREQUENCY_STRING_ADDRESS: u64 = 0x0038_06d8;
    const PIC1_GIO_BURST_ADDRESS: u64 = 0x1fa2_0008;
    const PIC1_GIO_DELAY_ADDRESS: u64 = 0x1fa2_000c;
    const EXPECTED_SERIAL_B: &[u8] =
        b"\r\nNVRAM checksum is incorrect: reinitializing the NVRAM.\r\n\
\r\ninitializing tod clock\r\n\
setting secs=0 min=0 hour=0 day=1 month=1 year=0\r\n\
\r\nSCSI device/cable diagnostic               *FAILED*\r\n\
\r\n        Check or replace:  Disk, Floppy, CDROM, or SCSI Cable\r\n\
\r\n\r\nError-- gfx(0) keyboard not responding\r\n\
\r\nError-- cannot open console \"gfx(0)\"\r\n\
\r\n\
\n\rDiagnostics failed.\n\
\r[Press any key to continue.]";

    #[derive(Debug, Eq, PartialEq)]
    struct ExternalPromRun {
        serial_a: Vec<u8>,
        serial_b: Vec<u8>,
        prompt_instruction: Option<usize>,
        receive_poll_count: usize,
        p5_started: bool,
        p5_completed: bool,
        headless_graphics_probe_observed: bool,
        volume_header_magic_checked: bool,
        volume_header_checksum_checked: bool,
        gio_burst: Option<u32>,
        gio_delay: Option<u32>,
        cpu_frequency_string: Vec<Option<u8>>,
    }

    fn read_physical_word(machine: &Ip12, address: u64) -> Option<u32> {
        let DebugResponse::Memory(memory) = machine.debug(DebugRequest::Memory {
            address_space: MemoryAddressSpace::Physical,
            start: address,
            length: 4,
        }) else {
            unreachable!();
        };
        let bytes: [Option<u8>; 4] = memory
            .bytes
            .try_into()
            .expect("a word debug request must return four byte slots");
        let [Some(first), Some(second), Some(third), Some(fourth)] = bytes else {
            return None;
        };
        Some(u32::from_be_bytes([first, second, third, fourth]))
    }

    fn machine_with_instructions(instructions: &[u32]) -> Machine {
        let mut raw_prom = vec![0; PROM_BYTES];
        for (destination, instruction) in raw_prom
            .chunks_exact_mut(4)
            .zip(instructions.iter().copied())
        {
            let [first, second, third, fourth] = instruction.to_be_bytes();
            destination.copy_from_slice(&[second, first, fourth, third]);
        }
        Machine::IndigoIp12(Ip12::new(raw_prom, Backend::SoftFloat, None, None).unwrap())
    }

    fn machine_that_transmits_serial_a(values: &[u8]) -> Machine {
        const LOAD_SERIAL_A_CONTROL: [u32; 2] = [0x3c08_bfb8, 0x3508_0d1b];
        const STORE_T1: u32 = 0xa109_0000;

        let mut instructions = Vec::from(LOAD_SERIAL_A_CONTROL);
        for (register, value) in [(4, 0x44), (11, 0x10), (12, 10), (13, 0), (14, 1), (5, 0x68)] {
            instructions.extend([0x2409_0000 | register, STORE_T1]);
            instructions.extend([0x2409_0000 | value, STORE_T1]);
        }
        instructions.push(0x2508_0004);
        for value in values {
            instructions.extend([0x2409_0000 | u32::from(*value), STORE_T1]);
        }
        instructions.extend([0x1000_ffff, 0]);

        machine_with_instructions(&instructions)
    }

    fn record_manifest() -> RecordManifest {
        RecordManifest::new(
            MachineStartupConfiguration::IndigoIp12 {
                floating_point_backend: Backend::SoftFloat,
                memory: Ip12MemoryConfiguration::default(),
            },
            MediaIdentity::from_bytes(Path::new("prom.bin"), &[0; PROM_BYTES]),
            None,
            None,
            distinct_nonvolatile_state(),
        )
    }

    fn started_recorder(path: &Path) -> Recorder {
        let recorder = Recorder::create(path).unwrap();
        recorder.start(&record_manifest()).unwrap();
        recorder
    }

    fn record_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "sgi-emu-runtime-{name}-{}.serec",
            std::process::id()
        ))
    }

    fn remove_record_artifacts(path: &Path) {
        let _ = fs::remove_file(path);
        let _ = fs::remove_file(format!("{}.partial", path.display()));
        let _ = fs::remove_file(format!("{}.idx", path.display()));
        let _ = fs::remove_file(format!("{}.idx.partial", path.display()));
        let _ = fs::remove_dir_all(format!("{}.ckpt", path.display()));
    }

    fn distinct_nonvolatile_state() -> MachineNonvolatileState {
        let mut words = [u16::MAX; 64];
        words[5] = 0x1234;
        MachineNonvolatileState::IndigoIp12(
            Ip12NonvolatileState::try_from_parts(Ip12NonvolatileStateParts {
                nvram_words: words,
                rtc_registers: [0; 32],
                rtc_alternate_control_registers: [0; 4],
                rtc_prescaler_phase_attoseconds: 0,
                rtc_millisecond_within_hundredth: 0,
                rtc_oscillator_failed: false,
                rtc_single_supply: false,
                rtc_alarm_match_active: false,
            })
            .unwrap(),
        )
    }

    fn execute_instructions(worker: &mut super::Worker, count: usize) {
        for _ in 0..count {
            worker.execute_timed_instruction().unwrap();
        }
    }

    #[test]
    fn serial_input_waits_in_the_runtime_until_the_guest_frees_fifo_space() {
        let instructions = [
            0x3c08_bfb8,
            0x3508_0d1b,
            0x2409_0003,
            0xa109_0000,
            0x2409_0001,
            0xa109_0000,
            0x910a_0004,
            0x1000_fffe,
            0,
        ];
        let mut worker = super::Worker::new(Some(machine_with_instructions(&instructions)));
        execute_instructions(&mut worker, 6);

        let (reply, response) = mpsc::channel();
        assert!(!worker.handle_command(crate::runtime::Command::SendSerial {
            port: SerialPort::A,
            bytes: (0..9).collect(),
            reply,
        }));
        response.recv().unwrap().unwrap();
        assert_eq!(worker.pending_serial[0].len(), 1);

        worker.execute_timed_instruction().unwrap();
        assert!(worker.pending_serial[0].is_empty());
    }

    #[test]
    fn reset_and_successful_configuration_discard_pending_serial_input() {
        let mut worker = super::Worker::new(Some(machine_with_instructions(&[0])));
        worker.execute_timed_instruction().unwrap();
        worker.pending_serial[0].extend(b"first");
        worker.pending_serial[1].extend(b"second");

        let (reset_reply, reset_response) = mpsc::channel();
        assert!(!worker.handle_command(crate::runtime::Command::Reset(reset_reply)));
        let reset_status = reset_response.recv().unwrap().unwrap();
        assert!(
            worker
                .pending_serial
                .iter()
                .all(|pending| pending.is_empty())
        );
        assert_eq!(reset_status.completed_instructions, 1);

        worker.pending_serial[0].extend(b"third");
        let (configure_reply, configure_response) = mpsc::channel();
        assert!(!worker.handle_command(crate::runtime::Command::Configure {
            configuration: Box::new(super::RuntimeConfiguration::normal(
                machine_with_instructions(&[0]),
            )),
            reply: configure_reply,
        }));
        let configure_status = configure_response.recv().unwrap().unwrap();
        assert!(
            worker
                .pending_serial
                .iter()
                .all(|pending| pending.is_empty())
        );
        assert_eq!(configure_status.completed_instructions, 1);
    }

    #[test]
    fn cpu_clock_accumulates_fractional_cycles_without_drift() {
        let mut clock = CpuClock::new(33_000_000);
        let mut elapsed = 0;

        for _ in 0..33_000 {
            elapsed += clock.advance_cycle().as_attoseconds();
        }

        assert_eq!(elapsed, ATTOSECONDS_PER_SECOND / 1_000);
        assert_eq!(clock.accumulated_remainder, 0);
    }

    #[test]
    fn cpu_clock_matches_the_division_reference_for_each_cycle() {
        for frequency_hz in [1, 3, 10, 33_000_000, 1_000_000_001, u64::MAX] {
            let mut clock = CpuClock::new(frequency_hz);
            let frequency_hz = u128::from(frequency_hz);
            let mut reference_remainder = 0;

            for _ in 0..1_000 {
                let numerator = ATTOSECONDS_PER_SECOND + reference_remainder;
                reference_remainder = numerator % frequency_hz;
                let expected = numerator / frequency_hz;

                assert_eq!(clock.advance_cycle().as_attoseconds(), expected);
                assert_eq!(clock.accumulated_remainder, reference_remainder);
            }
        }
    }

    #[test]
    fn cpu_clock_reset_discards_fractional_phase() {
        let mut clock = CpuClock::new(3);
        let first = clock.advance_cycle();
        clock.reset();

        assert_eq!(clock.advance_cycle(), first);
    }

    #[test]
    fn successful_instruction_and_guest_exception_advance_virtual_time() {
        for instruction in [0, 0x0000_000c] {
            let mut worker = super::Worker::new(Some(machine_with_instructions(&[instruction])));

            worker.execute_timed_instruction().unwrap();

            assert_eq!(
                worker.virtual_instant.as_attoseconds(),
                ATTOSECONDS_PER_SECOND / 33_000_000
            );
            assert_eq!(worker.completed_instructions, 1);
        }
    }

    #[test]
    fn step_error_does_not_advance_virtual_time() {
        let mut worker = super::Worker::new(Some(machine_with_instructions(&[
            0x3c08_4040,
            0x4088_6000,
            0,
            0,
            0x4a00_0000,
        ])));
        for _ in 0..4 {
            worker.execute_timed_instruction().unwrap();
        }
        let before = worker.virtual_instant;
        let completed_before = worker.completed_instructions;

        assert!(worker.execute_timed_instruction().is_err());

        assert_eq!(worker.virtual_instant, before);
        assert_eq!(worker.completed_instructions, completed_before);
    }

    #[test]
    fn completed_instruction_counter_wraps() {
        let mut worker = super::Worker::new(Some(machine_with_instructions(&[0])));
        worker.completed_instructions = u64::MAX;

        worker.execute_timed_instruction().unwrap();

        assert_eq!(worker.completed_instructions, 0);
    }

    #[test]
    fn recording_checkpoint_deadline_advances_and_resets() {
        let path = record_path("checkpoint-deadline");
        let partial = PathBuf::from(format!("{}.partial", path.display()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&partial);
        let mut session = super::RecordingSession::new(started_recorder(&path));

        assert!(!session.checkpoint_due(ExecutionPosition {
            epoch: 0,
            completed_instructions: super::CHECKPOINT_INTERVAL - 1,
        }));
        assert!(session.checkpoint_due(ExecutionPosition {
            epoch: 0,
            completed_instructions: super::CHECKPOINT_INTERVAL,
        }));
        session.advance_checkpoint_deadline();
        assert!(session.checkpoint_due(ExecutionPosition {
            epoch: 0,
            completed_instructions: super::CHECKPOINT_INTERVAL * 2,
        }));
        session.reset_checkpoint_deadline();
        assert!(session.checkpoint_due(ExecutionPosition {
            epoch: 1,
            completed_instructions: super::CHECKPOINT_INTERVAL,
        }));

        drop(session);
        fs::remove_file(partial).unwrap();
    }

    #[test]
    fn machine_output_handler_has_no_backlog_and_can_be_cleared() {
        const INSTRUCTIONS_PER_CHARACTER_INTERVAL: usize = 35_000;

        let mut worker = super::Worker::new(Some(machine_that_transmits_serial_a(b"ABC")));
        execute_instructions(&mut worker, INSTRUCTIONS_PER_CHARACTER_INTERVAL);

        let received = Arc::new(Mutex::new(Vec::new()));
        let handler_received = Arc::clone(&received);
        worker.output_handler = Some(Box::new(move |output| {
            handler_received
                .lock()
                .unwrap()
                .extend_from_slice(output.serial(SerialPort::A));
        }));
        execute_instructions(&mut worker, INSTRUCTIONS_PER_CHARACTER_INTERVAL);
        assert_eq!(*received.lock().unwrap(), b"B");

        worker.output_handler = None;
        execute_instructions(&mut worker, INSTRUCTIONS_PER_CHARACTER_INTERVAL);
        worker.output_handler = Some(Box::new(|_| {
            panic!("discarded output must not be retained for a later handler")
        }));
        worker.execute_timed_instruction().unwrap();
    }

    #[test]
    fn record_and_replay_preserve_event_positions_across_reset_epochs() {
        let path = record_path("event-positions");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(format!("{}.partial", path.display()));
        let recorder = started_recorder(&path);
        let runtime = Runtime::new_unconfigured().unwrap();
        let status = runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&[0, 0, 0, 0]),
                recorder,
            ))
            .unwrap();
        assert_eq!(status.mode, RuntimeMode::Recording);
        runtime.send_serial(SerialPort::A, b"before reset").unwrap();
        runtime.step().unwrap();
        runtime.step().unwrap();
        let reset = runtime.reset().unwrap();
        assert_eq!(reset.position.epoch, 1);
        assert_eq!(reset.position.completed_instructions, 0);
        runtime.send_serial(SerialPort::B, b"after reset").unwrap();
        runtime.step().unwrap();
        let stopped = runtime.stop_recording().unwrap();
        assert_eq!(stopped.mode, RuntimeMode::Normal);
        runtime.shutdown().unwrap();

        let replayer = Replayer::open(&path).unwrap();
        let runtime = Runtime::new_unconfigured().unwrap();
        let opened = runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&[0, 0, 0, 0]),
                replayer,
            ))
            .unwrap();
        assert_eq!(opened.mode, RuntimeMode::Replaying);
        assert!(runtime.send_serial(SerialPort::A, b"live").is_err());
        assert!(runtime.reset().is_err());
        runtime.step().unwrap();
        runtime.step().unwrap();
        let completed = runtime.step().unwrap();
        assert_eq!(completed.mode, RuntimeMode::ReplayCompleted);
        assert_eq!(completed.position.epoch, 1);
        assert_eq!(completed.position.completed_instructions, 1);
        assert!(runtime.step().is_err());
        runtime.shutdown().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn replay_processes_position_zero_input_before_completing() {
        let path = record_path("position-zero-input");
        let _ = fs::remove_file(&path);
        let recorder = started_recorder(&path);
        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&[0]),
                recorder,
            ))
            .unwrap();
        runtime.send_serial(SerialPort::A, b"").unwrap();
        runtime.send_serial(SerialPort::A, b"input").unwrap();
        runtime.stop_recording().unwrap();
        runtime.shutdown().unwrap();

        let runtime = Runtime::new_unconfigured().unwrap();
        let opened = runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&[0]),
                Replayer::open(&path).unwrap(),
            ))
            .unwrap();
        assert_eq!(opened.mode, RuntimeMode::ReplayCompleted);
        assert_eq!(opened.position, Default::default());
        assert!(runtime.step().is_err());
        runtime.shutdown().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn manual_replay_snapshot_restores_runtime_and_machine_state() {
        let path = record_path("manual-snapshot");
        remove_record_artifacts(&path);

        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&[0x2408_0001, 0x2508_0001, 0]),
                started_recorder(&path),
            ))
            .unwrap();
        runtime.step().unwrap();
        runtime.step().unwrap();
        runtime.stop_recording().unwrap();
        runtime.shutdown().unwrap();

        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&[0x2408_0001, 0x2508_0001, 0]),
                Replayer::open(&path).unwrap(),
            ))
            .unwrap();
        let before_snapshot = runtime.step().unwrap();
        assert_eq!(before_snapshot.position.completed_instructions, 1);
        let created = runtime.create_replay_snapshot().unwrap();
        assert_eq!(created.position, before_snapshot.position);
        runtime.create_replay_snapshot().unwrap();
        runtime.shutdown().unwrap();

        let snapshots = Replayer::snapshot_catalog(&path).unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].position(), before_snapshot.position);
        let snapshot_id = snapshots[0].id().to_owned();

        fs::write(format!("{}.idx", path.display()), b"invalid index").unwrap();
        let rebuilt = Replayer::snapshot_catalog(&path).unwrap();
        assert_eq!(rebuilt.len(), 1);
        assert_eq!(rebuilt[0].id(), snapshot_id);

        let runtime = Runtime::new_unconfigured().unwrap();
        let restored = runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&[0x2408_0001, 0x2508_0001, 0]),
                Replayer::open_snapshot(&path, &snapshot_id).unwrap(),
            ))
            .unwrap();
        assert_eq!(restored.state, RuntimeState::Paused);
        assert_eq!(restored.mode, RuntimeMode::Replaying);
        assert_eq!(restored.position, before_snapshot.position);
        assert_eq!(restored.completed_instructions, 1);
        let completed = runtime.step().unwrap();
        assert_eq!(completed.mode, RuntimeMode::ReplayCompleted);
        runtime.shutdown().unwrap();

        remove_record_artifacts(&path);
    }

    #[test]
    fn manual_replay_snapshot_requires_active_paused_replay() {
        let runtime = Runtime::new(Some(machine_with_instructions(&[0]))).unwrap();
        assert!(runtime.create_replay_snapshot().is_err());
        runtime.shutdown().unwrap();

        let path = record_path("snapshot-state-gate");
        remove_record_artifacts(&path);
        let recorder = started_recorder(&path);
        recorder
            .finalize(
                ExecutionPosition::default(),
                &crate::record::RecordOutcome::UserStopped,
            )
            .unwrap();
        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&[0x1000_ffff, 0]),
                Replayer::open(&path).unwrap(),
            ))
            .unwrap();
        assert!(runtime.create_replay_snapshot().is_err());
        runtime.shutdown().unwrap();
        remove_record_artifacts(&path);
    }

    #[test]
    fn stopping_a_running_record_preserves_running_execution_state() {
        let path = record_path("running-stop");
        let _ = fs::remove_file(&path);
        let recorder = started_recorder(&path);
        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&[0, 0x1000_fffe, 0]),
                recorder,
            ))
            .unwrap();
        runtime.run().unwrap();
        let stopped = runtime.stop_recording().unwrap();
        assert_eq!(stopped.state, RuntimeState::Running);
        assert_eq!(stopped.mode, RuntimeMode::Normal);
        runtime.shutdown().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn recording_storage_failure_pauses_and_returns_to_normal_mode() {
        let path = record_path("storage-failure");
        let _ = fs::remove_file(&path);
        let partial = PathBuf::from(format!("{}.partial", path.display()));
        let _ = fs::remove_file(&partial);
        let recorder = started_recorder(&path);
        let disk = recorder.disk();
        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&[0]),
                recorder,
            ))
            .unwrap();
        disk.report_storage_error(&io::Error::other("test failure"));
        let status = runtime.step().unwrap();

        assert_eq!(status.state, RuntimeState::Paused);
        assert_eq!(status.mode, RuntimeMode::Normal);
        assert_eq!(
            status.session_error.as_deref(),
            Some("host storage error: test failure")
        );
        runtime.shutdown().unwrap();
        fs::remove_file(partial).unwrap();
    }

    #[test]
    fn replay_shutdown_returns_the_pre_replay_nonvolatile_state() {
        let path = record_path("nonvolatile-isolation");
        let _ = fs::remove_file(&path);
        let recorder = started_recorder(&path);
        let record_runtime = Runtime::new_unconfigured().unwrap();
        record_runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&[0]),
                recorder,
            ))
            .unwrap();
        record_runtime.stop_recording().unwrap();
        record_runtime.shutdown().unwrap();

        let mut normal_machine = machine_with_instructions(&[0]);
        let expected = distinct_nonvolatile_state();
        normal_machine.restore_nonvolatile_state(expected.clone(), 0);
        let runtime = Runtime::new(Some(normal_machine)).unwrap();
        runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&[0]),
                Replayer::open(&path).unwrap(),
            ))
            .unwrap();
        assert_eq!(runtime.shutdown().unwrap(), Some(expected));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn final_checkpoint_detects_replay_cpu_divergence() {
        let path = record_path("checkpoint-divergence");
        let _ = fs::remove_file(&path);
        let recorder = started_recorder(&path);
        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&[0]),
                recorder,
            ))
            .unwrap();
        runtime.step().unwrap();
        runtime.stop_recording().unwrap();
        runtime.shutdown().unwrap();

        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&[0x2408_0001]),
                Replayer::open(&path).unwrap(),
            ))
            .unwrap();
        let status = runtime.step().unwrap();
        assert_eq!(status.mode, RuntimeMode::ReplayDiverged);
        assert!(status.session_error.is_some());
        runtime.shutdown().unwrap();
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn recorded_execution_error_finalizes_and_replays_at_the_same_boundary() {
        let path = record_path("execution-error");
        let _ = fs::remove_file(&path);
        let instructions = [0x3c08_4040, 0x4088_6000, 0, 0, 0x4a00_0000];
        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::recording(
                machine_with_instructions(&instructions),
                started_recorder(&path),
            ))
            .unwrap();
        for _ in 0..4 {
            runtime.step().unwrap();
        }
        let recorded = runtime.step().unwrap();
        assert_eq!(recorded.mode, RuntimeMode::Normal);
        assert_eq!(recorded.state, RuntimeState::Paused);
        assert!(recorded.last_error.is_some());
        runtime.shutdown().unwrap();

        let runtime = Runtime::new_unconfigured().unwrap();
        runtime
            .configure_with(RuntimeConfiguration::replaying(
                machine_with_instructions(&instructions),
                Replayer::open(&path).unwrap(),
            ))
            .unwrap();
        for _ in 0..4 {
            runtime.step().unwrap();
        }
        let replayed = runtime.step().unwrap();
        assert_eq!(replayed.mode, RuntimeMode::ReplayCompleted);
        assert!(replayed.last_error.is_some());
        runtime.shutdown().unwrap();
        fs::remove_file(path).unwrap();
    }

    struct RecordingStorage {
        bytes: Vec<u8>,
        reads: Arc<Mutex<Vec<(u64, usize)>>>,
    }

    impl BlockStorage for RecordingStorage {
        fn size_bytes(&self) -> u64 {
            self.bytes.len() as u64
        }

        fn read_exact_at(&mut self, offset: u64, buffer: &mut [u8]) -> io::Result<()> {
            let start = usize::try_from(offset)
                .map_err(|_| io::Error::other("storage offset does not fit usize"))?;
            let end = start
                .checked_add(buffer.len())
                .ok_or_else(|| io::Error::other("storage range overflow"))?;
            let source = self
                .bytes
                .get(start..end)
                .ok_or_else(|| io::Error::new(io::ErrorKind::UnexpectedEof, "short storage"))?;
            buffer.copy_from_slice(source);
            self.reads.lock().unwrap().push((offset, buffer.len()));
            Ok(())
        }

        fn write_all_at(&mut self, _offset: u64, _data: &[u8]) -> io::Result<()> {
            Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "recording storage is read-only",
            ))
        }
    }

    fn dynamic_sgi_volume_header() -> Vec<u8> {
        const BLOCK_COUNT: usize = 16;
        const VOLUME_HEADER_MAGIC: u32 = 0x0be5_a941;

        let mut bytes = vec![0; BLOCK_COUNT * 512];
        bytes[..4].copy_from_slice(&VOLUME_HEADER_MAGIC.to_be_bytes());
        bytes[508..512].copy_from_slice(&VOLUME_HEADER_MAGIC.wrapping_neg().to_be_bytes());
        let checksum = bytes[..512]
            .chunks_exact(4)
            .map(|word| u32::from_be_bytes(word.try_into().unwrap()))
            .fold(0_u32, u32::wrapping_add);
        assert_eq!(checksum, 0);
        bytes
    }

    fn run_external_ip12_prom(
        raw_prom: Vec<u8>,
        storage: Option<Box<dyn BlockStorage>>,
    ) -> ExternalPromRun {
        let continue_after_diagnostics = storage.is_some();
        let machine = Machine::IndigoIp12(
            Ip12::new(raw_prom, Backend::SoftFloat, storage, None)
                .expect("the PROM dump and storage should be valid"),
        );
        let serial_a = Arc::new(Mutex::new(Vec::new()));
        let serial_b = Arc::new(Mutex::new(Vec::new()));
        let output_a = Arc::clone(&serial_a);
        let output_b = Arc::clone(&serial_b);
        let mut worker = super::Worker::new(Some(machine));
        worker.output_handler = Some(Box::new(move |output| {
            output_a
                .lock()
                .unwrap()
                .extend_from_slice(output.serial(SerialPort::A));
            output_b
                .lock()
                .unwrap()
                .extend_from_slice(output.serial(SerialPort::B));
        }));

        let mut executed = 0;
        let mut prompt_completed_at = None;
        let mut receive_poll_count = 0;
        let mut p5_started = false;
        let mut p5_completed = false;
        let mut headless_graphics_probe_observed = false;
        let mut volume_header_magic_checked = false;
        let mut volume_header_checksum_checked = false;
        let mut system_start_requested = false;
        while executed < EXTERNAL_PROM_EXECUTION_BUDGET {
            let address = worker.machine.as_ref().unwrap().execution_address();
            if address == P5_START_PC {
                p5_started = true;
            }
            if address == P5_COMPLETE_PC {
                p5_completed = true;
            }
            if address == HEADLESS_GRAPHICS_PROBE_PC {
                headless_graphics_probe_observed = true;
            }
            if address == VOLUME_HEADER_MAGIC_CHECK_PC {
                volume_header_magic_checked = true;
            }
            if address == VOLUME_HEADER_CHECKSUM_PC {
                volume_header_checksum_checked = true;
            }
            if prompt_completed_at.is_some() && address == SERIAL_RECEIVE_POLL_PC {
                receive_poll_count += 1;
            }
            if let Err(error) = worker.execute_timed_instruction() {
                panic!(
                    "PROM execution failed at 0x{address:08x} after {executed} instructions: {error}"
                );
            }
            executed += 1;

            if prompt_completed_at.is_none()
                && serial_b
                    .lock()
                    .unwrap()
                    .ends_with(b"[Press any key to continue.]")
            {
                prompt_completed_at = Some(executed);
                if continue_after_diagnostics {
                    worker.pending_serial[serial_port_index(SerialPort::B)].push_back(b'\r');
                }
            }
            if continue_after_diagnostics
                && !system_start_requested
                && serial_b.lock().unwrap().ends_with(b"Option? ")
            {
                worker.pending_serial[serial_port_index(SerialPort::B)].extend(b"1\r");
                system_start_requested = true;
            }
            if continue_after_diagnostics {
                if volume_header_magic_checked && volume_header_checksum_checked {
                    break;
                }
            } else if let Some(prompt_instruction) = prompt_completed_at
                && executed - prompt_instruction == POST_PROMPT_INSTRUCTIONS
            {
                break;
            }
        }

        let prompt_instruction = prompt_completed_at.or_else(|| {
            if continue_after_diagnostics {
                return None;
            }
            let machine = worker.machine.as_ref().unwrap();
            let address = machine.execution_address();
            let Machine::IndigoIp12(machine) = machine;
            let DebugResponse::Registers(registers) = machine.debug(DebugRequest::Registers) else {
                unreachable!();
            };
            let cpu = &registers.cpu;
            let DebugResponse::Memory(saved_return_address) = machine.debug(DebugRequest::Memory {
                address_space: MemoryAddressSpace::Virtual,
                start: u64::from(cpu.gpr[29].wrapping_add(28)),
                length: 4,
            }) else {
                unreachable!();
            };
            let caller = saved_return_address
                .bytes
                .iter()
                .copied()
                .collect::<Option<Vec<_>>>()
                .and_then(|bytes| <[u8; 4]>::try_from(bytes).ok())
                .map(u32::from_be_bytes);
            let serial_b = serial_b.lock().unwrap();
            let tail_start = serial_b.len().saturating_sub(512);
            let serial_tail = String::from_utf8_lossy(&serial_b[tail_start..]);
            panic!(
                "PROM did not reach its input prompt within {EXTERNAL_PROM_EXECUTION_BUDGET} \
                 instructions; PC=0x{address:08x}; a0=0x{:08x}; ra=0x{:08x}; sp=0x{:08x}; \
                 caller={caller:?}; \
                 serial B tail:\n{serial_tail}",
                cpu.gpr[4], cpu.gpr[31], cpu.gpr[29]
            )
        });
        let Machine::IndigoIp12(machine) = worker.machine.as_ref().unwrap();
        let DebugResponse::Memory(cpu_frequency) = machine.debug(DebugRequest::Memory {
            address_space: MemoryAddressSpace::Physical,
            start: CPU_FREQUENCY_STRING_ADDRESS,
            length: 3,
        }) else {
            unreachable!();
        };

        ExternalPromRun {
            serial_a: serial_a.lock().unwrap().clone(),
            serial_b: serial_b.lock().unwrap().clone(),
            prompt_instruction,
            receive_poll_count,
            p5_started,
            p5_completed,
            headless_graphics_probe_observed,
            volume_header_magic_checked,
            volume_header_checksum_checked,
            gio_burst: read_physical_word(machine, PIC1_GIO_BURST_ADDRESS),
            gio_delay: read_physical_word(machine, PIC1_GIO_DELAY_ADDRESS),
            cpu_frequency_string: cpu_frequency.bytes,
        }
    }

    #[test]
    #[ignore = "requires an external 070-8088-002 IP12 PROM dump"]
    fn ip12_prom_reaches_serial_receive_wait_deterministically() {
        let path = env::var_os("SE_INDIGO_IP12_PROM")
            .expect("SE_INDIGO_IP12_PROM must name the external PROM dump");
        let raw_prom = fs::read(path).expect("the external PROM dump should be readable");

        let first = run_external_ip12_prom(raw_prom.clone(), None);
        let second = run_external_ip12_prom(raw_prom, None);

        assert_eq!(first, second);
        assert!(first.serial_a.is_empty());
        assert!(
            !first
                .serial_b
                .windows(b"can't set tod clock".len())
                .any(|window| window == b"can't set tod clock")
        );
        assert_eq!(first.serial_b, EXPECTED_SERIAL_B);
        assert_eq!(first.prompt_instruction, Some(EXPECTED_PROMPT_INSTRUCTION));
        assert!(first.receive_poll_count != 0);
        assert!(first.p5_started);
        assert!(first.p5_completed);
        assert!(first.headless_graphics_probe_observed);
        assert!(!first.volume_header_magic_checked);
        assert!(!first.volume_header_checksum_checked);
        assert_eq!(first.gio_burst, Some(1));
        assert_eq!(first.gio_delay, Some(0xf2));
        assert_eq!(
            first.cpu_frequency_string,
            [Some(b'3'), Some(b'3'), Some(0)]
        );
    }

    #[test]
    #[ignore = "requires an external 070-8088-002 IP12 PROM dump"]
    fn ip12_prom_validates_dynamic_volume_header_through_scsi_dma() {
        let path = env::var_os("SE_INDIGO_IP12_PROM")
            .expect("SE_INDIGO_IP12_PROM must name the external PROM dump");
        let raw_prom = fs::read(path).expect("the external PROM dump should be readable");
        let reads = Arc::new(Mutex::new(Vec::new()));
        let storage = RecordingStorage {
            bytes: dynamic_sgi_volume_header(),
            reads: Arc::clone(&reads),
        };

        let run = run_external_ip12_prom(raw_prom, Some(Box::new(storage)));

        let storage_reads = reads.lock().unwrap().clone();
        let serial_b = String::from_utf8_lossy(&run.serial_b);
        assert!(
            run.volume_header_magic_checked,
            "PROM did not check the volume-header magic; storage reads: {storage_reads:?}; serial B:\n{serial_b}"
        );
        assert!(
            run.volume_header_checksum_checked,
            "PROM did not check the volume-header checksum; storage reads: {storage_reads:?}; serial B:\n{serial_b}"
        );
        assert!(
            storage_reads
                .iter()
                .any(|&(offset, length)| offset == 0 && length >= 512)
        );
        assert!(
            !run.serial_b
                .windows(b"SCSI device/cable diagnostic".len())
                .any(|window| window == b"SCSI device/cable diagnostic")
        );
    }

    fn reset_machine() -> Machine {
        Machine::IndigoIp12(Ip12::new(vec![0; 0x40000], Backend::SoftFloat, None, None).unwrap())
    }

    #[test]
    fn unconfigured_runtime_rejects_machine_commands() {
        let runtime = Runtime::new_unconfigured().unwrap();

        assert_eq!(runtime.status().unwrap().state, RuntimeState::Unconfigured);
        assert!(matches!(
            runtime.step(),
            Err(RuntimeError::CommandRejected { .. })
        ));
        assert!(matches!(
            runtime.send_serial(SerialPort::A, b"A"),
            Err(RuntimeError::CommandRejected { .. })
        ));
        assert_eq!(runtime.shutdown(), Ok(None));
    }

    #[test]
    fn configured_runtime_returns_final_nonvolatile_state_on_shutdown() {
        let machine = reset_machine();
        let expected = machine.nonvolatile_state();
        let runtime = Runtime::new(Some(machine)).unwrap();

        assert_eq!(runtime.shutdown(), Ok(Some(expected)));
    }

    #[test]
    fn checkpoint_uses_the_machine_state_fingerprint() {
        let mut machine = reset_machine();
        let baseline = checkpoint_digest(&machine);

        assert_eq!(baseline, checkpoint_digest(&machine));
        machine.execute_instruction().unwrap();
        assert_ne!(baseline, checkpoint_digest(&machine));
    }
}
