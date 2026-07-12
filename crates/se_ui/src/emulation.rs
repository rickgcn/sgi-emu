//! Host-side lifecycle control for the IP32 machine.

use std::{
    collections::VecDeque,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

use se_machine::o2::ip32::{
    address_map::IP32_PROM_IMAGE_SIZE_BYTES,
    event::{Ip32SerialOutput, Ip32SerialPort},
    machine::{Ip32Machine, Ip32MachineConfig},
};
use se_runtime::runtime::RunStatus;

use crate::{
    application::ffi::{
        EmulationSnapshot, EmulationState, TerminalInputStatus, UiSerialPort, UiTerminalChunk,
        UiTerminalIoStats,
    },
    tracing::{UiTraceSink, begin_application_trace_session},
};

const RUN_BATCH_SIZE: usize = 4_096;
const TERMINAL_QUEUE_CAPACITY: usize = 65_536;

struct TerminalInputRequest {
    port: UiSerialPort,
    bytes: Vec<u8>,
}

#[derive(Clone, Copy, Default)]
struct TerminalStatsData {
    sent: u64,
    received: u64,
    dropped: u64,
}

struct ControllerState {
    desired_running: bool,
    hard_reset_requested: bool,
    configure_prom: Option<Vec<u8>>,
    shutdown_requested: bool,
    terminal_inputs: VecDeque<TerminalInputRequest>,
    terminal_input_units: [usize; 2],
    terminal_outputs: VecDeque<UiTerminalChunk>,
    terminal_output_units: [usize; 2],
    terminal_stats: [TerminalStatsData; 2],
    snapshot: SnapshotData,
}

#[derive(Clone)]
struct SnapshotData {
    state: EmulationState,
    session_id: u64,
    sim_time: u64,
    has_machine: bool,
    error_id: u64,
    error_message: String,
}

impl Default for SnapshotData {
    fn default() -> Self {
        Self {
            state: EmulationState::Unconfigured,
            session_id: 0,
            sim_time: 0,
            has_machine: false,
            error_id: 0,
            error_message: String::new(),
        }
    }
}

struct SharedController {
    state: Mutex<ControllerState>,
    wake: Condvar,
}

/// Controls one IP32 worker without exposing the machine to the Qt thread.
pub struct EmulationController {
    shared: Arc<SharedController>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl EmulationController {
    /// Creates an unconfigured controller and starts its worker thread.
    pub fn new() -> Self {
        let shared = Arc::new(SharedController {
            state: Mutex::new(ControllerState {
                desired_running: false,
                hard_reset_requested: false,
                configure_prom: None,
                shutdown_requested: false,
                terminal_inputs: VecDeque::new(),
                terminal_input_units: [0; 2],
                terminal_outputs: VecDeque::new(),
                terminal_output_units: [0; 2],
                terminal_stats: [TerminalStatsData::default(); 2],
                snapshot: SnapshotData::default(),
            }),
            wake: Condvar::new(),
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("sgi-emu-ip32".to_owned())
            .spawn(move || {
                if catch_unwind(AssertUnwindSafe(|| worker_main(&worker_shared))).is_err() {
                    let mut state = lock_state(&worker_shared);
                    if !state.shutdown_requested {
                        state.shutdown_requested = true;
                        state.desired_running = false;
                        state.hard_reset_requested = false;
                        state.configure_prom = None;
                        state.snapshot.has_machine = false;
                        set_fault(&mut state, "IP32 worker thread panicked".to_owned());
                    }
                }
            });

        let worker = match worker {
            Ok(worker) => Some(worker),
            Err(error) => {
                let mut state = lock_state(&shared);
                state.shutdown_requested = true;
                set_fault(
                    &mut state,
                    format!("failed to start IP32 worker thread: {error}"),
                );
                None
            }
        };

        Self {
            shared,
            worker: Mutex::new(worker),
        }
    }

    /// Requests that a paused machine start running.
    pub fn request_run(&self) -> bool {
        let mut state = lock_state(&self.shared);
        if state.snapshot.state != EmulationState::Paused || state.shutdown_requested {
            return false;
        }
        state.desired_running = true;
        state.snapshot.state = EmulationState::Running;
        self.shared.wake.notify_one();
        true
    }

    /// Requests that a running machine pause after its current batch.
    pub fn request_pause(&self) -> bool {
        let mut state = lock_state(&self.shared);
        if state.snapshot.state != EmulationState::Running || state.shutdown_requested {
            return false;
        }
        state.desired_running = false;
        true
    }

    /// Requests a hard reset of the active machine.
    pub fn request_hard_reset(&self) -> bool {
        let mut state = lock_state(&self.shared);
        if !state.snapshot.has_machine
            || state.shutdown_requested
            || !matches!(
                state.snapshot.state,
                EmulationState::Paused
                    | EmulationState::Running
                    | EmulationState::Idle
                    | EmulationState::Faulted
            )
        {
            return false;
        }
        state.hard_reset_requested = true;
        clear_terminal_inputs(&mut state);
        self.shared.wake.notify_one();
        true
    }

    /// Replaces the current machine with one using the supplied System PROM.
    pub fn configure_prom(&self, prom: &[u8]) -> bool {
        if prom.len() != IP32_PROM_IMAGE_SIZE_BYTES {
            return false;
        }

        let mut state = lock_state(&self.shared);
        if state.shutdown_requested
            || !matches!(
                state.snapshot.state,
                EmulationState::Unconfigured
                    | EmulationState::Paused
                    | EmulationState::Idle
                    | EmulationState::Faulted
            )
        {
            return false;
        }
        state.desired_running = false;
        state.hard_reset_requested = false;
        state.configure_prom = Some(prom.to_vec());
        clear_terminal_inputs(&mut state);
        state.snapshot.state = EmulationState::Building;
        state.snapshot.error_message.clear();
        self.shared.wake.notify_one();
        true
    }

    /// Queues terminal input for delivery by the machine worker.
    pub fn submit_terminal_input(&self, port: UiSerialPort, bytes: &[u8]) -> TerminalInputStatus {
        if bytes.is_empty() {
            return TerminalInputStatus::Accepted;
        }
        let mut state = lock_state(&self.shared);
        if state.shutdown_requested
            || !state.snapshot.has_machine
            || !matches!(
                state.snapshot.state,
                EmulationState::Paused | EmulationState::Running | EmulationState::Idle
            )
        {
            return TerminalInputStatus::Unavailable;
        }
        let index = terminal_port_index(port);
        if state.terminal_input_units[index] + bytes.len() > TERMINAL_QUEUE_CAPACITY {
            return TerminalInputStatus::QueueFull;
        }
        state.terminal_input_units[index] += bytes.len();
        state.terminal_inputs.push_back(TerminalInputRequest {
            port,
            bytes: bytes.to_vec(),
        });
        if state.snapshot.state == EmulationState::Idle {
            state.snapshot.state = EmulationState::Paused;
        }
        self.shared.wake.notify_one();
        TerminalInputStatus::Accepted
    }

    /// Drains terminal output without touching the worker-owned machine.
    pub fn drain_terminal_output(&self, max_bytes: usize) -> Vec<UiTerminalChunk> {
        let mut state = lock_state(&self.shared);
        let mut remaining = max_bytes;
        let mut output = Vec::new();
        while remaining != 0 {
            let Some(mut chunk) = state.terminal_outputs.pop_front() else {
                break;
            };
            let index = terminal_port_index(chunk.port);
            if chunk.bytes.len() <= remaining {
                remaining -= chunk.bytes.len();
                state.terminal_output_units[index] =
                    state.terminal_output_units[index].saturating_sub(chunk.bytes.len());
                output.push(chunk);
            } else {
                let tail = chunk.bytes.split_off(remaining);
                state.terminal_output_units[index] =
                    state.terminal_output_units[index].saturating_sub(chunk.bytes.len());
                let session_id = chunk.session_id;
                let port = chunk.port;
                output.push(chunk);
                state.terminal_outputs.push_front(UiTerminalChunk {
                    session_id,
                    port,
                    bytes: tail,
                });
                remaining = 0;
            }
        }
        output
    }

    /// Returns cumulative terminal I/O counters for one session.
    pub fn terminal_io_stats(&self, port: UiSerialPort) -> UiTerminalIoStats {
        let stats = lock_state(&self.shared).terminal_stats[terminal_port_index(port)];
        UiTerminalIoStats {
            sent: stats.sent,
            received: stats.received,
            dropped: stats.dropped,
        }
    }

    /// Returns the latest worker state without touching the machine.
    pub fn snapshot(&self) -> EmulationSnapshot {
        let snapshot = lock_state(&self.shared).snapshot.clone();
        EmulationSnapshot {
            state: snapshot.state,
            session_id: snapshot.session_id,
            sim_time: snapshot.sim_time,
            has_machine: snapshot.has_machine,
            error_id: snapshot.error_id,
            error_message: snapshot.error_message,
        }
    }

    pub(crate) fn shutdown(&self) {
        {
            let mut state = lock_state(&self.shared);
            state.shutdown_requested = true;
            state.desired_running = false;
            clear_terminal_inputs(&mut state);
            state.snapshot.state = EmulationState::ShuttingDown;
            self.shared.wake.notify_one();
        }

        if let Some(worker) = self
            .worker
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
        {
            let _ = worker.join();
        }
    }
}

impl Default for EmulationController {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for EmulationController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum WorkerAction {
    Configure(Vec<u8>),
    HardReset,
    TerminalInput(TerminalInputRequest),
    RunBatch,
    Shutdown,
}

fn worker_main(shared: &Arc<SharedController>) {
    let mut machine: Option<Ip32Machine<UiTraceSink>> = None;

    loop {
        let action = next_action(shared, machine.is_some());
        match action {
            WorkerAction::Shutdown => break,
            WorkerAction::Configure(prom) => configure_machine(shared, &mut machine, prom),
            WorkerAction::HardReset => {
                hard_reset_machine(shared, machine.as_mut());
            }
            WorkerAction::TerminalInput(request) => {
                submit_machine_terminal_input(shared, machine.as_mut(), request);
            }
            WorkerAction::RunBatch => run_machine_batch(shared, machine.as_mut()),
        }
    }
}

fn next_action(shared: &Arc<SharedController>, has_machine: bool) -> WorkerAction {
    let mut state = lock_state(shared);
    loop {
        if state.shutdown_requested {
            state.snapshot.state = EmulationState::ShuttingDown;
            return WorkerAction::Shutdown;
        }
        if let Some(prom) = state.configure_prom.take() {
            return WorkerAction::Configure(prom);
        }
        if state.hard_reset_requested {
            state.hard_reset_requested = false;
            if has_machine {
                return WorkerAction::HardReset;
            }
        }
        if let Some(request) = state.terminal_inputs.pop_front() {
            let index = terminal_port_index(request.port);
            state.terminal_input_units[index] =
                state.terminal_input_units[index].saturating_sub(request.bytes.len());
            return WorkerAction::TerminalInput(request);
        }
        if state.desired_running && has_machine {
            state.snapshot.state = EmulationState::Running;
            return WorkerAction::RunBatch;
        }
        if state.snapshot.state == EmulationState::Running {
            state.snapshot.state = EmulationState::Paused;
        }
        state = shared
            .wake
            .wait(state)
            .unwrap_or_else(|error| error.into_inner());
    }
}

fn configure_machine(
    shared: &Arc<SharedController>,
    machine: &mut Option<Ip32Machine<UiTraceSink>>,
    prom: Vec<u8>,
) {
    *machine = None;
    let session_id = begin_application_trace_session();
    let config = Ip32MachineConfig {
        prom_image: prom,
        ..Ip32MachineConfig::default()
    };

    let result = Ip32Machine::from_config_with_trace_sink(config, UiTraceSink::application())
        .map_err(|error| error.to_string())
        .and_then(|mut machine| {
            machine
                .schedule_power_on()
                .map_err(|error| format!("failed to schedule IP32 power-on: {error}"))?;
            Ok(machine)
        });

    let mut state = lock_state(shared);
    clear_terminal_session(&mut state);
    state.snapshot.session_id = session_id;
    state.snapshot.sim_time = 0;
    state.desired_running = false;
    match result {
        Ok(new_machine) => {
            *machine = Some(new_machine);
            state.snapshot.state = EmulationState::Paused;
            state.snapshot.has_machine = true;
            state.snapshot.error_message.clear();
        }
        Err(error) => {
            state.snapshot.has_machine = false;
            set_fault(&mut state, error.to_string());
        }
    }
}

fn hard_reset_machine(
    shared: &Arc<SharedController>,
    machine: Option<&mut Ip32Machine<UiTraceSink>>,
) {
    let Some(machine) = machine else {
        return;
    };
    let result = machine.hard_reset();
    let sim_time = machine.runtime().now().get();
    let mut state = lock_state(shared);
    state.snapshot.sim_time = sim_time;
    match result {
        Ok(()) => {
            state.snapshot.error_message.clear();
            state.snapshot.state = if state.desired_running {
                EmulationState::Running
            } else {
                EmulationState::Paused
            };
        }
        Err(error) => {
            state.desired_running = false;
            set_fault(&mut state, error.to_string());
        }
    }
}

fn submit_machine_terminal_input(
    shared: &Arc<SharedController>,
    machine: Option<&mut Ip32Machine<UiTraceSink>>,
    request: TerminalInputRequest,
) {
    let Some(machine) = machine else {
        return;
    };
    let result = machine.schedule_serial_input(
        machine.runtime().now(),
        machine_serial_port(request.port),
        request.bytes.clone(),
    );
    let mut state = lock_state(shared);
    match result {
        Ok(_) => {
            let stats = &mut state.terminal_stats[terminal_port_index(request.port)];
            stats.sent = stats.sent.saturating_add(request.bytes.len() as u64);
            if state.snapshot.state == EmulationState::Idle {
                state.snapshot.state = EmulationState::Paused;
            }
        }
        Err(error) => {
            state.desired_running = false;
            set_fault(
                &mut state,
                format!("failed to schedule terminal input: {error}"),
            );
        }
    }
}

fn run_machine_batch(
    shared: &Arc<SharedController>,
    machine: Option<&mut Ip32Machine<UiTraceSink>>,
) {
    let Some(machine) = machine else {
        return;
    };
    let result = machine.run_steps(RUN_BATCH_SIZE);
    let sim_time = machine.runtime().now().get();
    let terminal_output = drain_machine_terminal_output(machine);
    let mut state = lock_state(shared);
    state.snapshot.sim_time = sim_time;
    for (port, bytes) in terminal_output {
        enqueue_terminal_output(&mut state, port, bytes);
    }

    match result {
        Ok(RunStatus::StepLimitReached | RunStatus::Dispatched | RunStatus::DeadlineReached) => {
            state.snapshot.state = if state.desired_running {
                EmulationState::Running
            } else {
                EmulationState::Paused
            };
        }
        Ok(RunStatus::Idle | RunStatus::Stopped) => {
            state.desired_running = false;
            state.snapshot.state = EmulationState::Idle;
        }
        Err(error) => {
            state.desired_running = false;
            set_fault(&mut state, error.to_string());
        }
    }
}

fn drain_machine_terminal_output(
    machine: &mut Ip32Machine<UiTraceSink>,
) -> Vec<(UiSerialPort, Vec<u8>)> {
    let mut output: Vec<(UiSerialPort, Vec<u8>)> = Vec::new();
    while let Some(Ip32SerialOutput { port, bytes }) = machine.poll_serial_output() {
        let serial_port = ui_serial_port(port);
        if let Some((last_port, last_bytes)) = output.last_mut()
            && *last_port == serial_port
            && last_bytes.len() + bytes.len() <= RUN_BATCH_SIZE
        {
            last_bytes.extend_from_slice(&bytes);
        } else {
            output.push((serial_port, bytes));
        }
    }
    output
}

fn enqueue_terminal_output(state: &mut ControllerState, port: UiSerialPort, bytes: Vec<u8>) {
    let index = terminal_port_index(port);
    if bytes.len() > TERMINAL_QUEUE_CAPACITY {
        state.terminal_stats[index].dropped = state.terminal_stats[index]
            .dropped
            .saturating_add(bytes.len() as u64);
        return;
    }
    while state.terminal_output_units[index] + bytes.len() > TERMINAL_QUEUE_CAPACITY {
        let Some(position) = state
            .terminal_outputs
            .iter()
            .position(|chunk| chunk.port == port)
        else {
            break;
        };
        let dropped = state
            .terminal_outputs
            .remove(position)
            .expect("located terminal output must remain queued");
        state.terminal_output_units[index] =
            state.terminal_output_units[index].saturating_sub(dropped.bytes.len());
        state.terminal_stats[index].dropped = state.terminal_stats[index]
            .dropped
            .saturating_add(dropped.bytes.len() as u64);
    }
    state.terminal_output_units[index] += bytes.len();
    state.terminal_stats[index].received = state.terminal_stats[index]
        .received
        .saturating_add(bytes.len() as u64);
    state.terminal_outputs.push_back(UiTerminalChunk {
        session_id: state.snapshot.session_id,
        port,
        bytes,
    });
}

fn terminal_port_index(port: UiSerialPort) -> usize {
    match port {
        UiSerialPort::Serial1 => 0,
        UiSerialPort::Serial2 => 1,
        _ => 0,
    }
}

fn machine_serial_port(port: UiSerialPort) -> Ip32SerialPort {
    match port {
        UiSerialPort::Serial1 => Ip32SerialPort::Serial1,
        UiSerialPort::Serial2 => Ip32SerialPort::Serial2,
        _ => Ip32SerialPort::Serial1,
    }
}

fn ui_serial_port(port: Ip32SerialPort) -> UiSerialPort {
    match port {
        Ip32SerialPort::Serial1 => UiSerialPort::Serial1,
        Ip32SerialPort::Serial2 => UiSerialPort::Serial2,
    }
}

fn clear_terminal_inputs(state: &mut ControllerState) {
    state.terminal_inputs.clear();
    state.terminal_input_units.fill(0);
}

fn clear_terminal_session(state: &mut ControllerState) {
    clear_terminal_inputs(state);
    state.terminal_outputs.clear();
    state.terminal_output_units.fill(0);
    state.terminal_stats.fill(TerminalStatsData::default());
}

fn set_fault(state: &mut ControllerState, message: String) {
    state.snapshot.state = EmulationState::Faulted;
    state.snapshot.error_id = state.snapshot.error_id.saturating_add(1);
    state.snapshot.error_message = message;
}

fn lock_state(shared: &SharedController) -> std::sync::MutexGuard<'_, ControllerState> {
    shared
        .state
        .lock()
        .unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;

    const WAIT: u32 = 0x4200_0020;

    const fn i_type(opcode: u8, rs: u8, rt: u8, immediate: u16) -> u32 {
        (opcode as u32) << 26 | (rs as u32) << 21 | (rt as u32) << 16 | immediate as u32
    }

    fn serial_prompt_prom() -> Vec<u8> {
        let mut prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        let program = [
            i_type(0x0f, 0, 1, 0xbf39),
            i_type(0x0d, 1, 1, 0x0007),
            i_type(0x09, 0, 2, 0x0080),
            i_type(0x28, 1, 2, 0x0300),
            i_type(0x09, 0, 2, 48),
            i_type(0x28, 1, 2, 0x0000),
            i_type(0x28, 1, 0, 0x0100),
            i_type(0x09, 0, 2, 3),
            i_type(0x28, 1, 2, 0x0700),
            i_type(0x09, 0, 2, 3),
            i_type(0x28, 1, 2, 0x0300),
            i_type(0x09, 0, 2, u16::from(b'>')),
            i_type(0x28, 1, 2, 0x0000),
            WAIT,
        ];
        for (index, instruction) in program.into_iter().enumerate() {
            let offset = index * 4;
            prom[offset..offset + 4].copy_from_slice(&instruction.to_be_bytes());
        }
        prom
    }

    fn wait_for_state(controller: &EmulationController, expected: EmulationState) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if controller.snapshot().state == expected {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!(
            "timed out waiting for {expected:?}; current state is {:?}",
            controller.snapshot().state
        );
    }

    #[test]
    fn controller_configures_runs_resets_and_shuts_down() {
        let controller = EmulationController::new();
        assert_eq!(controller.snapshot().state, EmulationState::Unconfigured);
        assert!(!controller.configure_prom(&[]));

        let mut prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        prom[..4].copy_from_slice(&WAIT.to_be_bytes());
        assert!(controller.configure_prom(&prom));
        wait_for_state(&controller, EmulationState::Paused);
        let first_session_id = controller.snapshot().session_id;
        assert_ne!(first_session_id, 0);

        assert!(controller.request_run());
        wait_for_state(&controller, EmulationState::Idle);
        assert!(controller.request_hard_reset());
        wait_for_state(&controller, EmulationState::Paused);

        let prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        assert!(controller.configure_prom(&prom));
        wait_for_state(&controller, EmulationState::Paused);
        assert!(controller.snapshot().session_id > first_session_id);
        assert!(controller.request_run());
        wait_for_state(&controller, EmulationState::Running);
        assert!(controller.request_pause());
        wait_for_state(&controller, EmulationState::Paused);

        controller.shutdown();
        assert_eq!(controller.snapshot().state, EmulationState::ShuttingDown);
    }

    #[test]
    fn terminal_input_output_and_reset_follow_the_controller_session() {
        let controller = EmulationController::new();
        assert!(controller.configure_prom(&serial_prompt_prom()));
        wait_for_state(&controller, EmulationState::Paused);

        assert_eq!(
            controller.submit_terminal_input(UiSerialPort::Serial1, &vec![0; 65_537]),
            TerminalInputStatus::QueueFull
        );
        assert_eq!(
            controller.submit_terminal_input(UiSerialPort::Serial1, b"A"),
            TerminalInputStatus::Accepted
        );
        assert!(controller.request_run());
        wait_for_state(&controller, EmulationState::Idle);

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut bytes = Vec::new();
        while Instant::now() < deadline && bytes.is_empty() {
            for chunk in controller.drain_terminal_output(4_096) {
                if chunk.port == UiSerialPort::Serial1 {
                    bytes.extend_from_slice(&chunk.bytes);
                }
            }
            thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(bytes, b">");
        let stats = controller.terminal_io_stats(UiSerialPort::Serial1);
        assert_eq!(stats.sent, 1);
        assert_eq!(stats.received, 1);

        assert!(controller.request_hard_reset());
        wait_for_state(&controller, EmulationState::Paused);
        assert_eq!(
            controller.terminal_io_stats(UiSerialPort::Serial1).received,
            1
        );
    }
}
