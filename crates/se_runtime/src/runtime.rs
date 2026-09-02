//! Lifetime management and commands for the host runtime worker.

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

struct Worker {
    machine: Option<Machine>,
    cpu_clock: Option<CpuClock>,
    virtual_instant: VirtualInstant,
    machine_output: MachineOutput,
    output_handler: Option<Box<dyn FnMut(MachineOutput) + Send + 'static>>,
    pending_serial: [VecDeque<u8>; 2],
    state: RuntimeState,
    revision: u64,
    completed_instructions: u64,
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
        let cpu_clock = machine
            .as_ref()
            .map(|machine| CpuClock::new(machine.cpu_frequency_hz()));
        Self {
            machine,
            cpu_clock,
            virtual_instant: VirtualInstant::ZERO,
            machine_output: MachineOutput::default(),
            output_handler: None,
            pending_serial: [VecDeque::new(), VecDeque::new()],
            state,
            revision: 0,
            completed_instructions: 0,
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
                let mut machine = *machine;
                if let Some(state) = self.machine.as_ref().map(Machine::nonvolatile_state) {
                    machine.restore_nonvolatile_state(state, 0);
                }
                self.cpu_clock = Some(CpuClock::new(machine.cpu_frequency_hz()));
                self.machine = Some(machine);
                self.virtual_instant = VirtualInstant::ZERO;
                self.machine_output = MachineOutput::default();
                self.pending_serial = [VecDeque::new(), VecDeque::new()];
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
                    if let Some(clock) = self.cpu_clock.as_mut() {
                        clock.reset();
                    }
                    self.virtual_instant = VirtualInstant::ZERO;
                    self.machine_output = MachineOutput::default();
                    self.pending_serial = [VecDeque::new(), VecDeque::new()];
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
            Command::SendSerial { port, bytes, reply } => {
                let result = self.require_machine().map(|()| {
                    self.pending_serial[serial_port_index(port)].extend(bytes);
                    self.refill_serial_input();
                    self.advance_revision();
                    self.status()
                });
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
                let state = self.machine.as_ref().map(Machine::nonvolatile_state);
                let _ = reply.send(state);
                return true;
            }
        }
        false
    }

    fn step_once(&mut self) -> Result<RuntimeStatus, CommandRejection> {
        self.require_machine()?;
        if self.state == RuntimeState::Running {
            return Err(rejection("single-step requires a paused machine"));
        }

        match self.execute_timed_instruction() {
            Ok(()) => self.last_error = None,
            Err(error) => self.last_error = Some(error.to_string()),
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

            match self.execute_timed_instruction() {
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

    fn execute_timed_instruction(&mut self) -> Result<(), ExecutionError> {
        self.machine
            .as_mut()
            .expect("a timed instruction requires a configured machine")
            .execute_instruction()?;
        self.refill_serial_input();
        let elapsed = self
            .cpu_clock
            .as_mut()
            .expect("a configured machine requires a CPU clock")
            .advance_cycle();
        self.virtual_instant.advance(elapsed);
        self.machine
            .as_mut()
            .expect("a timed instruction requires a configured machine")
            .advance_time(elapsed, &mut self.machine_output);
        self.deliver_output();
        self.completed_instructions = self.completed_instructions.wrapping_add(1);
        Ok(())
    }

    fn refill_serial_input(&mut self) {
        if self.pending_serial[0].is_empty() && self.pending_serial[1].is_empty() {
            return;
        }

        let Some(machine) = self.machine.as_mut() else {
            return;
        };
        for (index, port) in [SerialPort::A, SerialPort::B].into_iter().enumerate() {
            let pending = &mut self.pending_serial[index];
            if pending.is_empty() {
                continue;
            }

            let consumed = machine.receive_serial(port, pending.make_contiguous());
            if consumed != 0 {
                pending.drain(..consumed);
            }
        }
    }

    fn deliver_output(&mut self) {
        if self.machine_output.is_empty() {
            return;
        }

        let output = mem::take(&mut self.machine_output);
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

    fn status(&self) -> RuntimeStatus {
        RuntimeStatus {
            state: self.state,
            revision: self.revision,
            completed_instructions: self.completed_instructions,
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
    use std::sync::{Arc, Mutex, mpsc};

    use se_core::time::ATTOSECONDS_PER_SECOND;
    use se_float::backend::Backend;
    use se_machine::indigo::ip12::Ip12;
    use se_machine::indigo::ip12::debug::{DebugRequest, DebugResponse, MemoryAddressSpace};
    use se_machine::machine::Machine;
    use se_machine::serial::SerialPort;
    use se_machine::storage::BlockStorage;

    use super::{CpuClock, Runtime, RuntimeError, serial_port_index};
    use crate::control::RuntimeState;

    const PROM_BYTES: usize = 0x40000;
    const EXTERNAL_PROM_EXECUTION_BUDGET: usize = 400_000_000;
    const EXPECTED_PROMPT_INSTRUCTION: usize = 376_277_712;
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
        Machine::IndigoIp12(Ip12::new(raw_prom, Backend::SoftFloat, None).unwrap())
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

    fn execute_instructions(worker: &mut super::Worker, count: usize) {
        for _ in 0..count {
            worker.execute_timed_instruction().unwrap();
        }
    }

    #[test]
    fn unconfigured_runtime_rejects_execution_and_shuts_down_cleanly() {
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
        let machine = machine_with_instructions(&[0]);
        let expected = machine.nonvolatile_state();
        let runtime = Runtime::new(Some(machine)).unwrap();

        assert_eq!(runtime.shutdown(), Ok(Some(expected)));
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
        assert!(!worker.handle_command(super::Command::SendSerial {
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
        assert!(!worker.handle_command(super::Command::Reset(reset_reply)));
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
        assert!(!worker.handle_command(super::Command::Configure {
            machine: Box::new(machine_with_instructions(&[0])),
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
            Ip12::new(raw_prom, Backend::SoftFloat, storage)
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
}
