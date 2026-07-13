//! Host-side lifecycle control for the IP32 machine.

use std::{
    collections::VecDeque,
    fs,
    panic::{AssertUnwindSafe, catch_unwind},
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use se_machine::o2::ip32::{
    address_map::IP32_PROM_IMAGE_SIZE_BYTES,
    event::{Ip32SerialOutput, Ip32SerialPort},
    machine::Ip32Machine,
};
use se_runtime::runtime::RunStatus;

use crate::{
    application::ffi::{
        EmulationSnapshot, EmulationState, PersistenceOutcome, TerminalInputStatus, UiSerialPort,
        UiTerminalChunk, UiTerminalIoStats,
    },
    persistence::{
        EmulationConfig, PersistencePaths, RtcPersistenceMode, hash_bytes, host_utc_seconds,
        load_battery, load_emulation_config, load_prom, read_state_file, save_battery,
        save_emulation_config, write_state_file,
    },
    tracing::{UiTraceSink, begin_application_trace_session},
};

const RUN_BATCH_SIZE: usize = 4_096;
const TERMINAL_QUEUE_CAPACITY: usize = 65_536;
const BATTERY_DEBOUNCE: Duration = Duration::from_secs(1);
const BATTERY_CHECKPOINT: Duration = Duration::from_secs(60);

struct TerminalInputRequest {
    port: UiSerialPort,
    bytes: Vec<u8>,
}

struct ConfigureRequest {
    prom_path: PathBuf,
    prom: Vec<u8>,
    rtc_mode: RtcPersistenceMode,
}

struct SaveStateRequest {
    path: PathBuf,
    return_state: EmulationState,
}

struct LoadStateRequest {
    path: PathBuf,
    prom_override: Option<PathBuf>,
    return_state: EmulationState,
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
    configure_machine: Option<ConfigureRequest>,
    save_state: Option<SaveStateRequest>,
    load_state: Option<LoadStateRequest>,
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
    persistence_id: u64,
    persistence_outcome: PersistenceOutcome,
    persistence_message: String,
    prom_path: String,
    rtc_mode: RtcPersistenceMode,
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
            persistence_id: 0,
            persistence_outcome: PersistenceOutcome::None,
            persistence_message: String::new(),
            prom_path: String::new(),
            rtc_mode: RtcPersistenceMode::RealTime,
        }
    }
}

struct SharedController {
    state: Mutex<ControllerState>,
    wake: Condvar,
    paths: Option<PersistencePaths>,
    application_version: String,
}

/// Controls one IP32 worker without exposing the machine to the Qt thread.
pub struct EmulationController {
    shared: Arc<SharedController>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl EmulationController {
    /// Creates an unconfigured controller and starts its worker thread.
    pub fn new() -> Self {
        Self::new_internal(env!("CARGO_PKG_VERSION"), Ok(None))
    }

    /// Creates a controller using the application version stored in state metadata.
    pub fn new_with_version(application_version: &str) -> Self {
        Self::new_internal(application_version, PersistencePaths::discover().map(Some))
    }

    fn new_internal(
        application_version: &str,
        paths: Result<Option<PersistencePaths>, crate::persistence::PersistenceError>,
    ) -> Self {
        let startup_error = paths.as_ref().err().map(ToString::to_string);
        let shared = Arc::new(SharedController {
            state: Mutex::new(ControllerState {
                desired_running: false,
                hard_reset_requested: false,
                configure_machine: None,
                save_state: None,
                load_state: None,
                shutdown_requested: false,
                terminal_inputs: VecDeque::new(),
                terminal_input_units: [0; 2],
                terminal_outputs: VecDeque::new(),
                terminal_output_units: [0; 2],
                terminal_stats: [TerminalStatsData::default(); 2],
                snapshot: SnapshotData::default(),
            }),
            wake: Condvar::new(),
            paths: paths.ok().flatten(),
            application_version: application_version.to_owned(),
        });
        if let Some(error) = startup_error {
            let mut state = lock_state(&shared);
            set_unconfigured_error(&mut state, error);
        }
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
                        state.configure_machine = None;
                        state.save_state = None;
                        state.load_state = None;
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
        self.configure_machine("", prom, RtcPersistenceMode::RealTime.as_u8())
    }

    /// Replaces the current machine and persists its PROM path and RTC policy.
    pub fn configure_machine(&self, prom_path: &str, prom: &[u8], rtc_mode: u8) -> bool {
        if prom.len() != IP32_PROM_IMAGE_SIZE_BYTES {
            return false;
        }
        let Some(rtc_mode) = RtcPersistenceMode::from_u8(rtc_mode) else {
            return false;
        };
        let prom_path = if prom_path.is_empty() {
            PathBuf::new()
        } else {
            match absolute_path(Path::new(prom_path)) {
                Ok(path) => path,
                Err(_) => return false,
            }
        };

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
        state.configure_machine = Some(ConfigureRequest {
            prom_path,
            prom: prom.to_vec(),
            rtc_mode,
        });
        clear_terminal_inputs(&mut state);
        state.snapshot.state = EmulationState::Building;
        state.snapshot.error_message.clear();
        self.shared.wake.notify_one();
        true
    }

    /// Requests an asynchronous exact-state save on the worker thread.
    pub fn request_save_state(&self, path: &str) -> bool {
        let Ok(path) = absolute_path(Path::new(path)) else {
            return false;
        };
        let mut state = lock_state(&self.shared);
        if state.shutdown_requested
            || !state.snapshot.has_machine
            || state.save_state.is_some()
            || state.load_state.is_some()
            || !matches!(
                state.snapshot.state,
                EmulationState::Paused | EmulationState::Running | EmulationState::Idle
            )
        {
            return false;
        }
        let return_state = state.snapshot.state;
        state.save_state = Some(SaveStateRequest { path, return_state });
        state.snapshot.state = EmulationState::Saving;
        self.shared.wake.notify_one();
        true
    }

    /// Requests an asynchronous exact-state load on the worker thread.
    pub fn request_load_state(&self, path: &str, prom_override: &str) -> bool {
        let Ok(path) = absolute_path(Path::new(path)) else {
            return false;
        };
        let prom_override = if prom_override.is_empty() {
            None
        } else {
            match absolute_path(Path::new(prom_override)) {
                Ok(path) => Some(path),
                Err(_) => return false,
            }
        };
        let mut state = lock_state(&self.shared);
        if state.shutdown_requested
            || state.save_state.is_some()
            || state.load_state.is_some()
            || !matches!(
                state.snapshot.state,
                EmulationState::Unconfigured
                    | EmulationState::Paused
                    | EmulationState::Running
                    | EmulationState::Idle
                    | EmulationState::Faulted
            )
        {
            return false;
        }
        let return_state = state.snapshot.state;
        state.desired_running = false;
        state.load_state = Some(LoadStateRequest {
            path,
            prom_override,
            return_state,
        });
        state.snapshot.state = EmulationState::Loading;
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
            persistence_id: snapshot.persistence_id,
            persistence_outcome: snapshot.persistence_outcome,
            persistence_message: snapshot.persistence_message,
            prom_path: snapshot.prom_path,
            rtc_mode: snapshot.rtc_mode.as_u8(),
        }
    }

    pub(crate) fn shutdown(&self) {
        {
            let mut state = lock_state(&self.shared);
            state.shutdown_requested = true;
            state.desired_running = false;
            state.save_state = None;
            state.load_state = None;
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
    Configure(ConfigureRequest),
    SaveState(SaveStateRequest),
    LoadState(LoadStateRequest),
    HardReset,
    TerminalInput(TerminalInputRequest),
    RunBatch,
    PersistenceTick,
    Shutdown,
}

fn worker_main(shared: &Arc<SharedController>) {
    let mut machine: Option<Ip32Machine<UiTraceSink>> = None;
    let mut active_config = None;
    let mut battery = BatteryCheckpoint::new();

    auto_configure_machine(shared, &mut machine, &mut active_config, &mut battery);

    loop {
        let action = next_action(shared, machine.is_some());
        match action {
            WorkerAction::Shutdown => {
                force_battery_checkpoint(shared, machine.as_ref(), &mut battery);
                break;
            }
            WorkerAction::Configure(request) => configure_worker_machine(
                shared,
                &mut machine,
                &mut active_config,
                &mut battery,
                request,
            ),
            WorkerAction::SaveState(request) => {
                save_machine_state(shared, machine.as_ref(), active_config.as_ref(), request)
            }
            WorkerAction::LoadState(request) => load_machine_state(
                shared,
                &mut machine,
                &mut active_config,
                &mut battery,
                request,
            ),
            WorkerAction::HardReset => {
                hard_reset_machine(shared, machine.as_mut());
            }
            WorkerAction::TerminalInput(request) => {
                submit_machine_terminal_input(shared, machine.as_mut(), request);
            }
            WorkerAction::RunBatch => run_machine_batch(shared, machine.as_mut()),
            WorkerAction::PersistenceTick => {
                periodic_battery_checkpoint(shared, machine.as_ref(), &mut battery);
            }
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
        if let Some(request) = state.load_state.take() {
            return WorkerAction::LoadState(request);
        }
        if let Some(request) = state.configure_machine.take() {
            return WorkerAction::Configure(request);
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
        if let Some(request) = state.save_state.take() {
            return WorkerAction::SaveState(request);
        }
        if state.desired_running && has_machine {
            state.snapshot.state = EmulationState::Running;
            return WorkerAction::RunBatch;
        }
        if state.snapshot.state == EmulationState::Running {
            state.snapshot.state = EmulationState::Paused;
        }
        let (next_state, timeout) = shared
            .wake
            .wait_timeout(state, BATTERY_DEBOUNCE)
            .unwrap_or_else(|error| error.into_inner());
        state = next_state;
        if timeout.timed_out() {
            return WorkerAction::PersistenceTick;
        }
    }
}

fn configure_worker_machine(
    shared: &Arc<SharedController>,
    machine: &mut Option<Ip32Machine<UiTraceSink>>,
    active_config: &mut Option<EmulationConfig>,
    battery: &mut BatteryCheckpoint,
    request: ConfigureRequest,
) {
    let result = build_configured_machine(shared, request);
    match result {
        Ok((new_machine, config, warning)) => {
            force_battery_checkpoint(shared, machine.as_ref(), battery);
            let mut state = lock_state(shared);
            state.desired_running = false;
            let session_id = begin_application_trace_session();
            clear_terminal_session(&mut state);
            state.snapshot.session_id = session_id;
            state.snapshot.sim_time = 0;
            state.snapshot.prom_path = config.prom_path().to_string_lossy().into_owned();
            state.snapshot.rtc_mode = config.rtc_mode();
            *machine = Some(new_machine);
            *active_config = Some(config);
            battery.reset();
            state.snapshot.state = EmulationState::Paused;
            state.snapshot.has_machine = true;
            state.snapshot.error_message.clear();
            if let Some(warning) = warning {
                set_persistence_result(&mut state, PersistenceOutcome::Warning, warning);
            }
        }
        Err(error) => {
            let mut state = lock_state(shared);
            state.desired_running = false;
            set_fault(&mut state, error.to_string());
        }
    }
}

struct BatteryCheckpoint {
    last_revision: Option<u64>,
    revision_changed_at: Option<Instant>,
    last_checkpoint: Instant,
}

impl BatteryCheckpoint {
    fn new() -> Self {
        Self {
            last_revision: None,
            revision_changed_at: None,
            last_checkpoint: Instant::now(),
        }
    }

    fn reset(&mut self) {
        self.last_revision = None;
        self.revision_changed_at = None;
        self.last_checkpoint = Instant::now();
    }
}

fn auto_configure_machine(
    shared: &Arc<SharedController>,
    machine: &mut Option<Ip32Machine<UiTraceSink>>,
    active_config: &mut Option<EmulationConfig>,
    battery: &mut BatteryCheckpoint,
) {
    let Some(paths) = shared.paths.as_ref() else {
        return;
    };
    let config = match load_emulation_config(paths) {
        Ok(Some(config)) => config,
        Ok(None) => return,
        Err(error) => {
            set_unconfigured_error(&mut lock_state(shared), error.to_string());
            return;
        }
    };
    let prom = match load_prom(&config) {
        Ok(prom) => prom,
        Err(error) => {
            let mut state = lock_state(shared);
            state.snapshot.prom_path = config.prom_path().to_string_lossy().into_owned();
            state.snapshot.rtc_mode = config.rtc_mode();
            set_unconfigured_error(&mut state, error.to_string());
            return;
        }
    };
    let battery_load = load_battery(paths, config.rtc_mode(), host_utc_seconds());
    let machine_config = config.machine().machine_config(
        prom,
        battery_load.state.unix_seconds(),
        battery_load.state.nvram().to_vec(),
    );
    let result =
        Ip32Machine::from_config_with_trace_sink(machine_config, UiTraceSink::application())
            .map_err(|error| error.to_string())
            .and_then(|mut new_machine| {
                new_machine
                    .restore_rtc_persistent_state(&battery_load.state)
                    .map_err(|error| error.to_string())?;
                new_machine
                    .schedule_power_on()
                    .map_err(|error| format!("failed to schedule IP32 power-on: {error}"))?;
                Ok(new_machine)
            });
    let mut state = lock_state(shared);
    match result {
        Ok(new_machine) => {
            let session_id = begin_application_trace_session();
            clear_terminal_session(&mut state);
            state.snapshot.session_id = session_id;
            state.snapshot.sim_time = 0;
            state.snapshot.has_machine = true;
            state.snapshot.state = EmulationState::Paused;
            state.snapshot.prom_path = config.prom_path().to_string_lossy().into_owned();
            state.snapshot.rtc_mode = config.rtc_mode();
            state.snapshot.error_message.clear();
            if let Some(warning) = battery_load.warning {
                set_persistence_result(&mut state, PersistenceOutcome::Warning, warning);
            }
            *machine = Some(new_machine);
            *active_config = Some(config);
            battery.reset();
        }
        Err(error) => set_unconfigured_error(&mut state, error),
    }
}

fn build_configured_machine(
    shared: &SharedController,
    request: ConfigureRequest,
) -> Result<(Ip32Machine<UiTraceSink>, EmulationConfig, Option<String>), String> {
    let persistent = se_machine::o2::ip32::state::Ip32PersistentConfig::default();
    let persisted_path = !request.prom_path.as_os_str().is_empty();
    let metadata_path = if persisted_path {
        request.prom_path.clone()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join("unpersisted-prom.bin")
    };
    let config = EmulationConfig::new(
        metadata_path,
        hash_bytes(&request.prom),
        request.rtc_mode,
        persistent,
    )
    .map_err(|error| error.to_string())?;
    let battery_load = shared
        .paths
        .as_ref()
        .map(|paths| load_battery(paths, request.rtc_mode, host_utc_seconds()));
    let rtc = battery_load
        .as_ref()
        .map(|load| &load.state)
        .cloned()
        .unwrap_or_else(|| {
            se_device::rtc::ds1687::state::Ds1687PersistentState::new(
                host_utc_seconds(),
                vec![0; 256],
                0,
            )
            .expect("the fallback battery image has the hardware size")
        });
    let machine_config =
        config
            .machine()
            .machine_config(request.prom, rtc.unix_seconds(), rtc.nvram().to_vec());
    let mut machine =
        Ip32Machine::from_config_with_trace_sink(machine_config, UiTraceSink::application())
            .map_err(|error| error.to_string())?;
    machine
        .restore_rtc_persistent_state(&rtc)
        .map_err(|error| error.to_string())?;
    machine
        .schedule_power_on()
        .map_err(|error| format!("failed to schedule IP32 power-on: {error}"))?;
    if persisted_path && let Some(paths) = shared.paths.as_ref() {
        save_emulation_config(paths, &config).map_err(|error| error.to_string())?;
    }
    Ok((machine, config, battery_load.and_then(|load| load.warning)))
}

fn save_machine_state(
    shared: &SharedController,
    machine: Option<&Ip32Machine<UiTraceSink>>,
    active_config: Option<&EmulationConfig>,
    request: SaveStateRequest,
) {
    let result = machine
        .ok_or_else(|| "no IP32 machine is configured".to_owned())
        .and_then(|machine| machine.save_state().map_err(|error| error.to_string()))
        .and_then(|machine_state| {
            let config = active_config
                .ok_or_else(|| "the active emulation configuration is unavailable".to_owned())?;
            write_state_file(
                &request.path,
                &shared.application_version,
                config,
                &machine_state,
            )
            .map_err(|error| error.to_string())
        });
    let mut state = lock_state(shared);
    state.desired_running = request.return_state == EmulationState::Running;
    state.snapshot.state = request.return_state;
    match result {
        Ok(()) => set_persistence_result(
            &mut state,
            PersistenceOutcome::Saved,
            format!("Saved state to {}", request.path.display()),
        ),
        Err(error) => set_persistence_result(&mut state, PersistenceOutcome::Failed, error),
    }
}

fn load_machine_state(
    shared: &SharedController,
    machine: &mut Option<Ip32Machine<UiTraceSink>>,
    active_config: &mut Option<EmulationConfig>,
    battery: &mut BatteryCheckpoint,
    request: LoadStateRequest,
) {
    let loaded =
        read_state_file(&request.path).map_err(|error| LoadMachineError::Failed(error.to_string()));
    let result = loaded.and_then(|loaded| {
        let prom_path = request
            .prom_override
            .clone()
            .unwrap_or_else(|| loaded.metadata_config.prom_path().to_owned());
        let prom = fs::read(&prom_path)
            .map_err(|error| LoadMachineError::PromRequired(error.to_string()))?;
        let expected_hash = loaded
            .metadata_config
            .prom_hash()
            .map_err(|error| LoadMachineError::Failed(error.to_string()))?;
        if hash_bytes(&prom) != expected_hash {
            return Err(LoadMachineError::PromRequired(
                "the selected System PROM does not match the state file".to_owned(),
            ));
        }
        let config = loaded
            .metadata_config
            .with_prom_path(prom_path)
            .map_err(|error| LoadMachineError::Failed(error.to_string()))?;
        let fallback_rtc = load_battery_for_config(shared, &config);
        let machine_config = config.machine().machine_config(
            prom,
            fallback_rtc.unix_seconds(),
            fallback_rtc.nvram().to_vec(),
        );
        let new_machine = Ip32Machine::from_state_with_trace_sink(
            machine_config,
            loaded.state,
            UiTraceSink::application(),
        )
        .map_err(|error| LoadMachineError::Failed(error.to_string()))?;
        Ok((new_machine, config))
    });

    match result {
        Ok((mut new_machine, config)) => {
            if let Some(paths) = shared.paths.as_ref()
                && let Err(error) = save_emulation_config(paths, &config)
            {
                finish_failed_load(shared, request.return_state, error.to_string());
                return;
            }
            let terminal_output = drain_machine_terminal_output(&mut new_machine);
            let session_id = begin_application_trace_session();
            let mut state = lock_state(shared);
            clear_terminal_session(&mut state);
            state.snapshot.session_id = session_id;
            state.snapshot.sim_time = new_machine.runtime().now().get();
            state.snapshot.has_machine = true;
            state.snapshot.state = EmulationState::Paused;
            state.snapshot.prom_path = config.prom_path().to_string_lossy().into_owned();
            state.snapshot.rtc_mode = config.rtc_mode();
            state.snapshot.error_message.clear();
            for (port, bytes) in terminal_output {
                enqueue_terminal_output(&mut state, port, bytes);
            }
            state.desired_running = false;
            *machine = Some(new_machine);
            *active_config = Some(config);
            battery.reset();
            set_persistence_result(
                &mut state,
                PersistenceOutcome::Loaded,
                format!("Loaded state from {}", request.path.display()),
            );
            drop(state);
            force_battery_checkpoint(shared, machine.as_ref(), battery);
        }
        Err(LoadMachineError::PromRequired(error)) => {
            let mut state = lock_state(shared);
            state.desired_running = request.return_state == EmulationState::Running;
            state.snapshot.state = request.return_state;
            set_persistence_result(&mut state, PersistenceOutcome::PromRequired, error);
        }
        Err(LoadMachineError::Failed(error)) => {
            finish_failed_load(shared, request.return_state, error);
        }
    }
}

enum LoadMachineError {
    PromRequired(String),
    Failed(String),
}

fn finish_failed_load(shared: &SharedController, return_state: EmulationState, error: String) {
    let mut state = lock_state(shared);
    state.desired_running = return_state == EmulationState::Running;
    state.snapshot.state = return_state;
    set_persistence_result(&mut state, PersistenceOutcome::Failed, error);
}

fn load_battery_for_config(
    shared: &SharedController,
    config: &EmulationConfig,
) -> se_device::rtc::ds1687::state::Ds1687PersistentState {
    shared
        .paths
        .as_ref()
        .map(|paths| load_battery(paths, config.rtc_mode(), host_utc_seconds()).state)
        .unwrap_or_else(|| {
            se_device::rtc::ds1687::state::Ds1687PersistentState::new(
                host_utc_seconds(),
                vec![0; 256],
                0,
            )
            .expect("the fallback battery image has the hardware size")
        })
}

fn periodic_battery_checkpoint(
    shared: &SharedController,
    machine: Option<&Ip32Machine<UiTraceSink>>,
    checkpoint: &mut BatteryCheckpoint,
) {
    let Some(machine) = machine else {
        return;
    };
    let Ok(rtc) = machine.rtc_persistent_state() else {
        return;
    };
    let now = Instant::now();
    if checkpoint.last_revision != Some(rtc.revision()) {
        checkpoint.last_revision = Some(rtc.revision());
        checkpoint.revision_changed_at = Some(now);
    }
    let revision_due = checkpoint
        .revision_changed_at
        .is_some_and(|changed| now.duration_since(changed) >= BATTERY_DEBOUNCE);
    let anchor_due = now.duration_since(checkpoint.last_checkpoint) >= BATTERY_CHECKPOINT;
    if revision_due || anchor_due {
        write_battery_checkpoint(shared, rtc, checkpoint);
    }
}

fn force_battery_checkpoint(
    shared: &SharedController,
    machine: Option<&Ip32Machine<UiTraceSink>>,
    checkpoint: &mut BatteryCheckpoint,
) {
    let Some(machine) = machine else {
        return;
    };
    match machine.rtc_persistent_state() {
        Ok(rtc) => write_battery_checkpoint(shared, rtc, checkpoint),
        Err(error) => set_persistence_result(
            &mut lock_state(shared),
            PersistenceOutcome::Warning,
            error.to_string(),
        ),
    }
}

fn write_battery_checkpoint(
    shared: &SharedController,
    rtc: se_device::rtc::ds1687::state::Ds1687PersistentState,
    checkpoint: &mut BatteryCheckpoint,
) {
    let Some(paths) = shared.paths.as_ref() else {
        return;
    };
    match save_battery(paths, rtc.clone(), host_utc_seconds()) {
        Ok(()) => {
            checkpoint.last_revision = Some(rtc.revision());
            checkpoint.revision_changed_at = None;
            checkpoint.last_checkpoint = Instant::now();
        }
        Err(error) => set_persistence_result(
            &mut lock_state(shared),
            PersistenceOutcome::Warning,
            format!("failed to persist RTC/NVRAM: {error}"),
        ),
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

fn set_unconfigured_error(state: &mut ControllerState, message: String) {
    state.snapshot.state = EmulationState::Unconfigured;
    state.snapshot.has_machine = false;
    state.snapshot.error_id = state.snapshot.error_id.saturating_add(1);
    state.snapshot.error_message = message;
}

fn set_persistence_result(
    state: &mut ControllerState,
    outcome: PersistenceOutcome,
    message: String,
) {
    state.snapshot.persistence_id = state.snapshot.persistence_id.saturating_add(1);
    state.snapshot.persistence_outcome = outcome;
    state.snapshot.persistence_message = message;
}

fn absolute_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    if path.is_absolute() {
        Ok(path.to_owned())
    } else {
        std::env::current_dir().map(|directory| directory.join(path))
    }
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

    fn wait_for_persistence(controller: &EmulationController, expected: PersistenceOutcome) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let snapshot = controller.snapshot();
            if snapshot.persistence_id != 0 && snapshot.persistence_outcome == expected {
                return;
            }
            thread::sleep(Duration::from_millis(1));
        }
        let snapshot = controller.snapshot();
        panic!(
            "timed out waiting for persistence outcome {expected:?}; got {:?}: {}",
            snapshot.persistence_outcome, snapshot.persistence_message
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

    #[test]
    fn controller_saves_and_replaces_a_session_asynchronously() {
        let directory = tempfile::tempdir().unwrap();
        let prom_path = directory.path().join("prom.bin");
        let state_path = directory.path().join("machine.sestate");
        let mut prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        prom[..4].copy_from_slice(&WAIT.to_be_bytes());
        fs::write(&prom_path, &prom).unwrap();

        let controller = EmulationController::new();
        assert!(controller.configure_machine(
            prom_path.to_str().unwrap(),
            &prom,
            RtcPersistenceMode::Frozen.as_u8(),
        ));
        wait_for_state(&controller, EmulationState::Paused);
        let first_session = controller.snapshot().session_id;

        assert!(controller.request_save_state(state_path.to_str().unwrap()));
        wait_for_persistence(&controller, PersistenceOutcome::Saved);
        assert!(state_path.is_file());

        assert!(
            controller
                .request_load_state(state_path.to_str().unwrap(), prom_path.to_str().unwrap(),)
        );
        wait_for_persistence(&controller, PersistenceOutcome::Loaded);
        let snapshot = controller.snapshot();
        assert_eq!(snapshot.state, EmulationState::Paused);
        assert!(snapshot.session_id > first_session);
    }
}
