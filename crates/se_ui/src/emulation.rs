//! Host-side lifecycle control for the IP32 machine.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
};

use se_machine::o2::ip32::{
    address_map::IP32_PROM_IMAGE_SIZE_BYTES,
    machine::{Ip32Machine, Ip32MachineConfig},
};
use se_runtime::runtime::RunStatus;

use crate::{
    application::ffi::{EmulationSnapshot, EmulationState},
    tracing::{UiTraceSink, begin_application_trace_session},
};

const RUN_BATCH_SIZE: usize = 4_096;

struct ControllerState {
    desired_running: bool,
    hard_reset_requested: bool,
    configure_prom: Option<Vec<u8>>,
    shutdown_requested: bool,
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
        state.snapshot.state = EmulationState::Building;
        state.snapshot.error_message.clear();
        self.shared.wake.notify_one();
        true
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

fn run_machine_batch(
    shared: &Arc<SharedController>,
    machine: Option<&mut Ip32Machine<UiTraceSink>>,
) {
    let Some(machine) = machine else {
        return;
    };
    let result = machine.run_steps(RUN_BATCH_SIZE);
    let sim_time = machine.runtime().now().get();
    let mut state = lock_state(shared);
    state.snapshot.sim_time = sim_time;

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
        assert_eq!(controller.snapshot().session_id, 1);

        assert!(controller.request_run());
        wait_for_state(&controller, EmulationState::Idle);
        assert!(controller.request_hard_reset());
        wait_for_state(&controller, EmulationState::Paused);

        let prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        assert!(controller.configure_prom(&prom));
        wait_for_state(&controller, EmulationState::Paused);
        assert_eq!(controller.snapshot().session_id, 2);
        assert!(controller.request_run());
        wait_for_state(&controller, EmulationState::Running);
        assert!(controller.request_pause());
        wait_for_state(&controller, EmulationState::Paused);

        controller.shutdown();
        assert_eq!(controller.snapshot().state, EmulationState::ShuttingDown);
    }
}
