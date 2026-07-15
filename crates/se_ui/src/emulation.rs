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

use se_device::chipset::gbe::protocol::{
    GbeExternalClock, GbeExternalInput, GbeFrame, GbeFrameField,
};
use se_machine::o2::ip32::{
    address_map::IP32_PROM_IMAGE_SIZE_BYTES,
    event::{Ip32SerialOutput, Ip32SerialPort},
    machine::Ip32Machine,
};
use se_runtime::runtime::RunStatus;

use crate::{
    application::ffi::{
        EmulationSnapshot, EmulationState, PersistenceOutcome, TerminalInputStatus, UiDisplayField,
        UiDisplayUpdate, UiSerialPort, UiTerminalChunk, UiTerminalIoStats,
    },
    persistence::{
        EmulationConfig, PersistencePaths, RtcPersistenceMode, hash_bytes, host_utc_seconds,
        load_battery, load_emulation_config, load_prom, load_system_flash,
        preserve_system_flash_overlay, read_state_file, save_battery, save_emulation_config,
        save_system_flash, write_state_file,
    },
    tracing::{UiTraceSink, begin_application_trace_session},
};

const RUN_BATCH_SIZE: usize = 4_096;
const TERMINAL_QUEUE_CAPACITY: usize = 65_536;
const BATTERY_DEBOUNCE: Duration = Duration::from_secs(1);
const BATTERY_CHECKPOINT: Duration = Duration::from_secs(60);
const PERSISTENCE_POLL_INTERVAL: Duration = Duration::from_millis(250);
const CRT_INITIAL_PIXEL_CLOCK_HZ: u64 = 20_000_000;

fn connect_crt_display(machine: &mut Ip32Machine<UiTraceSink>) -> Result<(), String> {
    let now = machine.runtime().now();
    machine
        .schedule_gbe_external_input(now, GbeExternalInput::SenseN(false))
        .map_err(|error| format!("failed to connect the CRT display sense input: {error}"))?;
    machine
        .schedule_gbe_external_input(
            now,
            GbeExternalInput::PixelClock {
                source: GbeExternalClock::Ttl,
                numerator_hz: CRT_INITIAL_PIXEL_CLOCK_HZ,
                denominator: 1,
            },
        )
        .map(|_| ())
        .map_err(|error| format!("failed to connect the CRT display pixel clock: {error}"))
}

struct TerminalInputRequest {
    port: UiSerialPort,
    bytes: Vec<u8>,
}

struct ConfigureRequest {
    prom_path: PathBuf,
    prom: Vec<u8>,
    rtc_mode: RtcPersistenceMode,
    jit_enabled: bool,
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
    display: DisplayState,
    snapshot: SnapshotData,
}

#[derive(Default)]
struct DisplayState {
    generation: u64,
    pending_frame: Option<GbeFrame>,
    machine_dropped: u64,
    transport_dropped: u64,
    invalid_frames: u64,
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
    jit_enabled: bool,
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
            jit_enabled: false,
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
                display: DisplayState::default(),
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
                        reset_display_session(&mut state);
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
        self.configure_machine("", prom, RtcPersistenceMode::RealTime.as_u8(), false)
    }

    /// Replaces the current machine and persists its PROM path and RTC policy.
    pub fn configure_machine(
        &self,
        prom_path: &str,
        prom: &[u8],
        rtc_mode: u8,
        jit_enabled: bool,
    ) -> bool {
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
            jit_enabled,
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

    /// Moves at most one current-session display frame to the UI thread.
    pub fn take_display_update(&self) -> UiDisplayUpdate {
        let mut state = lock_state(&self.shared);
        let generation = state.display.generation;
        let session_id = state.snapshot.session_id;
        let machine_dropped = state.display.machine_dropped;
        let transport_dropped = state.display.transport_dropped;
        let invalid_frames = state.display.invalid_frames;
        let Some(frame) = state.display.pending_frame.take() else {
            return UiDisplayUpdate {
                generation,
                session_id,
                has_frame: false,
                sequence: 0,
                completed_at: 0,
                width: 0,
                height: 0,
                stride: 0,
                field: UiDisplayField::Progressive,
                rgba: Vec::new(),
                machine_dropped,
                transport_dropped,
                invalid_frames,
            };
        };
        let field = match frame.field {
            GbeFrameField::Progressive => UiDisplayField::Progressive,
            GbeFrameField::First => UiDisplayField::First,
            GbeFrameField::Second => UiDisplayField::Second,
        };
        UiDisplayUpdate {
            generation,
            session_id,
            has_frame: true,
            sequence: frame.sequence,
            completed_at: frame.completed_at.get(),
            width: frame.width,
            height: frame.height,
            stride: frame.stride,
            field,
            rgba: frame.rgba,
            machine_dropped,
            transport_dropped,
            invalid_frames,
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
            jit_enabled: snapshot.jit_enabled,
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
    let mut system_flash = SystemFlashCheckpoint::new();

    auto_configure_machine(
        shared,
        &mut machine,
        &mut active_config,
        &mut battery,
        &mut system_flash,
    );

    loop {
        let action = next_action(shared, machine.is_some());
        match action {
            WorkerAction::Shutdown => {
                force_battery_checkpoint(shared, machine.as_ref(), &mut battery);
                force_system_flash_checkpoint(
                    shared,
                    machine.as_ref(),
                    active_config.as_ref(),
                    &mut system_flash,
                );
                break;
            }
            WorkerAction::Configure(request) => configure_worker_machine(
                shared,
                &mut machine,
                &mut active_config,
                &mut battery,
                &mut system_flash,
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
                &mut system_flash,
                request,
            ),
            WorkerAction::HardReset => {
                hard_reset_machine(shared, machine.as_mut());
            }
            WorkerAction::TerminalInput(request) => {
                submit_machine_terminal_input(shared, machine.as_mut(), request);
            }
            WorkerAction::RunBatch => {
                run_machine_batch(shared, machine.as_mut());
                periodic_battery_checkpoint(shared, machine.as_ref(), &mut battery);
                periodic_system_flash_checkpoint(
                    shared,
                    machine.as_ref(),
                    active_config.as_ref(),
                    &mut system_flash,
                );
            }
            WorkerAction::PersistenceTick => {
                periodic_battery_checkpoint(shared, machine.as_ref(), &mut battery);
                periodic_system_flash_checkpoint(
                    shared,
                    machine.as_ref(),
                    active_config.as_ref(),
                    &mut system_flash,
                );
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
    system_flash: &mut SystemFlashCheckpoint,
    request: ConfigureRequest,
) {
    let result = build_configured_machine(shared, request);
    match result {
        Ok((new_machine, config, warning)) => {
            force_battery_checkpoint(shared, machine.as_ref(), battery);
            force_system_flash_checkpoint(
                shared,
                machine.as_ref(),
                active_config.as_ref(),
                system_flash,
            );
            let mut state = lock_state(shared);
            state.desired_running = false;
            let session_id = begin_application_trace_session();
            clear_terminal_session(&mut state);
            reset_display_session(&mut state);
            state.snapshot.session_id = session_id;
            state.snapshot.sim_time = 0;
            state.snapshot.prom_path = config.prom_path().to_string_lossy().into_owned();
            state.snapshot.rtc_mode = config.rtc_mode();
            state.snapshot.jit_enabled = config.jit_enabled();
            *machine = Some(new_machine);
            *active_config = Some(config);
            battery.reset();
            system_flash.reset();
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
    last_poll: Instant,
}

impl BatteryCheckpoint {
    fn new() -> Self {
        Self {
            last_revision: None,
            revision_changed_at: None,
            last_checkpoint: Instant::now(),
            last_poll: Instant::now() - PERSISTENCE_POLL_INTERVAL,
        }
    }

    fn reset(&mut self) {
        self.last_revision = None;
        self.revision_changed_at = None;
        self.last_checkpoint = Instant::now();
        self.last_poll = Instant::now() - PERSISTENCE_POLL_INTERVAL;
    }
}

struct SystemFlashCheckpoint {
    last_revision: Option<u64>,
    revision_changed_at: Option<Instant>,
    last_poll: Instant,
}

impl SystemFlashCheckpoint {
    fn new() -> Self {
        Self {
            last_revision: None,
            revision_changed_at: None,
            last_poll: Instant::now() - PERSISTENCE_POLL_INTERVAL,
        }
    }

    fn reset(&mut self) {
        self.last_revision = None;
        self.revision_changed_at = None;
        self.last_poll = Instant::now() - PERSISTENCE_POLL_INTERVAL;
    }
}

fn auto_configure_machine(
    shared: &Arc<SharedController>,
    machine: &mut Option<Ip32Machine<UiTraceSink>>,
    active_config: &mut Option<EmulationConfig>,
    battery: &mut BatteryCheckpoint,
    system_flash: &mut SystemFlashCheckpoint,
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
            state.snapshot.jit_enabled = config.jit_enabled();
            set_unconfigured_error(&mut state, error.to_string());
            return;
        }
    };
    let prom_hash = match config.prom_hash() {
        Ok(hash) => hash,
        Err(error) => {
            set_unconfigured_error(&mut lock_state(shared), error.to_string());
            return;
        }
    };
    let battery_load = load_battery(paths, config.rtc_mode(), host_utc_seconds());
    let flash_load = load_system_flash(paths, prom_hash);
    let mut warnings = Vec::new();
    if let Some(warning) = battery_load.warning {
        warnings.push(warning);
    }
    if let Some(warning) = flash_load.warning {
        warnings.push(warning);
    }
    let mut machine_config = config.machine().machine_config(
        prom,
        battery_load.state.unix_seconds(),
        battery_load.state.nvram().to_vec(),
    );
    machine_config.jit_enabled = config.jit_enabled();
    let result = (|| {
        let mut new_machine =
            Ip32Machine::from_config_with_trace_sink(machine_config, UiTraceSink::application())
                .map_err(|error| error.to_string())?;
        new_machine
            .restore_rtc_persistent_state(&battery_load.state)
            .map_err(|error| error.to_string())?;
        if let Some(flash) = flash_load.state
            && let Err(error) = new_machine.restore_system_flash_persistent_state(&flash)
        {
            let backup = preserve_system_flash_overlay(paths, prom_hash, host_utc_seconds());
            let suffix = backup
                .map(|path| format!("; preserved as {}", path.display()))
                .unwrap_or_default();
            warnings.push(format!(
                "failed to restore System Flash overlay: {error}{suffix}"
            ));
        }
        new_machine
            .schedule_power_on()
            .map_err(|error| format!("failed to schedule IP32 power-on: {error}"))?;
        connect_crt_display(&mut new_machine)?;
        Ok::<_, String>(new_machine)
    })();
    let mut state = lock_state(shared);
    match result {
        Ok(new_machine) => {
            let session_id = begin_application_trace_session();
            clear_terminal_session(&mut state);
            reset_display_session(&mut state);
            state.snapshot.session_id = session_id;
            state.snapshot.sim_time = 0;
            state.snapshot.has_machine = true;
            state.snapshot.state = EmulationState::Paused;
            state.snapshot.prom_path = config.prom_path().to_string_lossy().into_owned();
            state.snapshot.rtc_mode = config.rtc_mode();
            state.snapshot.jit_enabled = config.jit_enabled();
            state.snapshot.error_message.clear();
            if !warnings.is_empty() {
                set_persistence_result(
                    &mut state,
                    PersistenceOutcome::Warning,
                    warnings.join("\n"),
                );
            }
            *machine = Some(new_machine);
            *active_config = Some(config);
            battery.reset();
            system_flash.reset();
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
    let prom_hash = hash_bytes(&request.prom);
    let metadata_path = if persisted_path {
        request.prom_path.clone()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join("unpersisted-prom.bin")
    };
    let config = EmulationConfig::new(
        metadata_path,
        prom_hash,
        request.rtc_mode,
        persistent,
        request.jit_enabled,
    )
    .map_err(|error| error.to_string())?;
    let battery_load = shared
        .paths
        .as_ref()
        .map(|paths| load_battery(paths, request.rtc_mode, host_utc_seconds()));
    let flash_load = shared
        .paths
        .as_ref()
        .map(|paths| load_system_flash(paths, prom_hash));
    let mut warnings = Vec::new();
    if let Some(warning) = battery_load.as_ref().and_then(|load| load.warning.as_ref()) {
        warnings.push(warning.clone());
    }
    if let Some(warning) = flash_load.as_ref().and_then(|load| load.warning.as_ref()) {
        warnings.push(warning.clone());
    }
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
    let mut machine_config =
        config
            .machine()
            .machine_config(request.prom, rtc.unix_seconds(), rtc.nvram().to_vec());
    machine_config.jit_enabled = config.jit_enabled();
    let mut machine =
        Ip32Machine::from_config_with_trace_sink(machine_config, UiTraceSink::application())
            .map_err(|error| error.to_string())?;
    machine
        .restore_rtc_persistent_state(&rtc)
        .map_err(|error| error.to_string())?;
    if let Some(flash) = flash_load.as_ref().and_then(|load| load.state.as_ref())
        && let Err(error) = machine.restore_system_flash_persistent_state(flash)
    {
        let suffix = shared
            .paths
            .as_ref()
            .and_then(|paths| preserve_system_flash_overlay(paths, prom_hash, host_utc_seconds()))
            .map(|path| format!("; preserved as {}", path.display()))
            .unwrap_or_default();
        warnings.push(format!(
            "failed to restore System Flash overlay: {error}{suffix}"
        ));
    }
    machine
        .schedule_power_on()
        .map_err(|error| format!("failed to schedule IP32 power-on: {error}"))?;
    connect_crt_display(&mut machine)?;
    if persisted_path && let Some(paths) = shared.paths.as_ref() {
        save_emulation_config(paths, &config).map_err(|error| error.to_string())?;
    }
    Ok((
        machine,
        config,
        (!warnings.is_empty()).then(|| warnings.join("\n")),
    ))
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
    system_flash: &mut SystemFlashCheckpoint,
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
        let mut machine_config = config.machine().machine_config(
            prom,
            fallback_rtc.unix_seconds(),
            fallback_rtc.nvram().to_vec(),
        );
        machine_config.jit_enabled = config.jit_enabled();
        let mut new_machine = Ip32Machine::from_state_with_trace_sink(
            machine_config,
            loaded.state,
            UiTraceSink::application(),
        )
        .map_err(|error| LoadMachineError::Failed(error.to_string()))?;
        connect_crt_display(&mut new_machine).map_err(LoadMachineError::Failed)?;
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
            force_battery_checkpoint(shared, machine.as_ref(), battery);
            force_system_flash_checkpoint(
                shared,
                machine.as_ref(),
                active_config.as_ref(),
                system_flash,
            );
            let terminal_output = drain_machine_terminal_output(&mut new_machine);
            let display_frame = new_machine.take_display_frame();
            let machine_dropped = new_machine.dropped_display_frame_count();
            let session_id = begin_application_trace_session();
            let mut state = lock_state(shared);
            clear_terminal_session(&mut state);
            reset_display_session(&mut state);
            update_display_output(&mut state, display_frame, machine_dropped);
            state.snapshot.session_id = session_id;
            state.snapshot.sim_time = new_machine.runtime().now().get();
            state.snapshot.has_machine = true;
            state.snapshot.state = EmulationState::Paused;
            state.snapshot.prom_path = config.prom_path().to_string_lossy().into_owned();
            state.snapshot.rtc_mode = config.rtc_mode();
            state.snapshot.jit_enabled = config.jit_enabled();
            state.snapshot.error_message.clear();
            for (port, bytes) in terminal_output {
                enqueue_terminal_output(&mut state, port, bytes);
            }
            state.desired_running = false;
            *machine = Some(new_machine);
            *active_config = Some(config);
            battery.reset();
            system_flash.reset();
            set_persistence_result(
                &mut state,
                PersistenceOutcome::Loaded,
                format!("Loaded state from {}", request.path.display()),
            );
            drop(state);
            force_battery_checkpoint(shared, machine.as_ref(), battery);
            force_system_flash_checkpoint(
                shared,
                machine.as_ref(),
                active_config.as_ref(),
                system_flash,
            );
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
    let now = Instant::now();
    if now.duration_since(checkpoint.last_poll) < PERSISTENCE_POLL_INTERVAL {
        return;
    }
    checkpoint.last_poll = now;
    let Some(machine) = machine else {
        return;
    };
    let Ok(rtc) = machine.rtc_persistent_state() else {
        return;
    };
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

fn periodic_system_flash_checkpoint(
    shared: &SharedController,
    machine: Option<&Ip32Machine<UiTraceSink>>,
    config: Option<&EmulationConfig>,
    checkpoint: &mut SystemFlashCheckpoint,
) {
    let now = Instant::now();
    if now.duration_since(checkpoint.last_poll) < PERSISTENCE_POLL_INTERVAL {
        return;
    }
    checkpoint.last_poll = now;
    let (Some(machine), Some(config)) = (machine, config) else {
        return;
    };
    let Ok(prom_hash) = config.prom_hash() else {
        return;
    };
    let Ok(flash) = machine.system_flash_persistent_state() else {
        return;
    };
    if checkpoint.last_revision != Some(flash.revision()) {
        checkpoint.last_revision = Some(flash.revision());
        checkpoint.revision_changed_at = Some(now);
    }
    let revision_due = checkpoint
        .revision_changed_at
        .is_some_and(|changed| now.duration_since(changed) >= BATTERY_DEBOUNCE);
    if revision_due {
        write_system_flash_checkpoint(shared, prom_hash, flash, checkpoint);
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

fn force_system_flash_checkpoint(
    shared: &SharedController,
    machine: Option<&Ip32Machine<UiTraceSink>>,
    config: Option<&EmulationConfig>,
    checkpoint: &mut SystemFlashCheckpoint,
) {
    let (Some(machine), Some(config)) = (machine, config) else {
        return;
    };
    let prom_hash = match config.prom_hash() {
        Ok(hash) => hash,
        Err(error) => {
            set_persistence_result(
                &mut lock_state(shared),
                PersistenceOutcome::Warning,
                error.to_string(),
            );
            return;
        }
    };
    match machine.system_flash_persistent_state() {
        Ok(flash) => write_system_flash_checkpoint(shared, prom_hash, flash, checkpoint),
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

fn write_system_flash_checkpoint(
    shared: &SharedController,
    prom_hash: [u8; 32],
    flash: se_device::memory::flash::SystemFlashPersistentState,
    checkpoint: &mut SystemFlashCheckpoint,
) {
    let Some(paths) = shared.paths.as_ref() else {
        return;
    };
    let revision = flash.revision();
    match save_system_flash(paths, prom_hash, flash) {
        Ok(()) => {
            checkpoint.last_revision = Some(revision);
            checkpoint.revision_changed_at = None;
        }
        Err(error) => set_persistence_result(
            &mut lock_state(shared),
            PersistenceOutcome::Warning,
            format!("failed to persist System Flash: {error}"),
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
    let result = machine
        .hard_reset()
        .map_err(|error| error.to_string())
        .and_then(|()| connect_crt_display(machine));
    let sim_time = machine.runtime().now().get();
    let mut state = lock_state(shared);
    state.snapshot.sim_time = sim_time;
    match result {
        Ok(()) => {
            reset_display_session(&mut state);
            state.snapshot.error_message.clear();
            state.snapshot.state = if state.desired_running {
                EmulationState::Running
            } else {
                EmulationState::Paused
            };
        }
        Err(error) => {
            state.desired_running = false;
            set_fault(&mut state, error);
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
    let display_frame = machine.take_display_frame();
    let machine_dropped = machine.dropped_display_frame_count();
    let mut state = lock_state(shared);
    state.snapshot.sim_time = sim_time;
    for (port, bytes) in terminal_output {
        enqueue_terminal_output(&mut state, port, bytes);
    }
    update_display_output(&mut state, display_frame, machine_dropped);

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

fn reset_display_session(state: &mut ControllerState) {
    state.display.generation = state.display.generation.saturating_add(1);
    state.display.pending_frame = None;
    state.display.machine_dropped = 0;
    state.display.transport_dropped = 0;
    state.display.invalid_frames = 0;
}

fn update_display_output(
    state: &mut ControllerState,
    frame: Option<GbeFrame>,
    machine_dropped: u64,
) {
    state.display.machine_dropped = machine_dropped;
    let Some(frame) = frame else {
        return;
    };
    if !valid_display_frame(&frame) {
        state.display.invalid_frames = state.display.invalid_frames.saturating_add(1);
        return;
    }
    if state.display.pending_frame.replace(frame).is_some() {
        state.display.transport_dropped = state.display.transport_dropped.saturating_add(1);
    }
}

fn valid_display_frame(frame: &GbeFrame) -> bool {
    if frame.width == 0
        || frame.height == 0
        || frame.width > i32::MAX as u32
        || frame.height > i32::MAX as u32
        || frame.stride > i32::MAX as u32
    {
        return false;
    }
    let Some(row_bytes) = (frame.width as usize).checked_mul(4) else {
        return false;
    };
    let stride = frame.stride as usize;
    let Some(required_bytes) = stride.checked_mul(frame.height as usize) else {
        return false;
    };
    stride >= row_bytes && frame.rgba.len() >= required_bytes
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

    use se_core::role::BusDeviceRole;
    use se_core::scheduler::SimTime;
    use se_device::chipset::{
        crime::protocol::{
            CrimeByteEnable, CrimeCgiTransaction, CrimeCompletionPayload, CrimeLinkDeviceResponse,
            CrimeLinkOperation, CrimePioRequest, CrimeTransactionId, CrimeTransfer,
        },
        gbe::{
            Gbe,
            protocol::{GbeAction, GbePoll},
        },
    };
    use se_machine::o2::ip32::{component_ids, machine::Ip32MachineConfig};

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

    fn wait_for_display_generation(controller: &EmulationController, previous: u64) -> u64 {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let generation = controller.take_display_update().generation;
            if generation > previous {
                return generation;
            }
            thread::sleep(Duration::from_millis(1));
        }
        panic!("timed out waiting for a display generation after {previous}");
    }

    fn display_frame(sequence: u64, field: GbeFrameField) -> GbeFrame {
        GbeFrame {
            sequence,
            completed_at: se_core::scheduler::SimTime::new(sequence * 10),
            width: 2,
            height: 1,
            stride: 8,
            field,
            rgba: vec![1, 2, 3, 4, 5, 6, 7, 8],
        }
    }

    #[test]
    fn crt_display_connection_applies_inputs_after_power_on() {
        let mut prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        prom[..4].copy_from_slice(&WAIT.to_be_bytes());
        let config = Ip32MachineConfig {
            prom_image: prom,
            ..Ip32MachineConfig::default()
        };
        let mut machine =
            Ip32Machine::from_config_with_trace_sink(config, UiTraceSink::application()).unwrap();
        machine.schedule_power_on().unwrap();
        connect_crt_display(&mut machine).unwrap();
        machine.run_steps(3).unwrap();

        let gbe = machine
            .runtime_mut()
            .registry_mut()
            .get_typed_mut::<Gbe>(component_ids::GBE)
            .unwrap();
        let response = gbe.accept(CrimeCgiTransaction {
            id: CrimeTransactionId::new(1),
            controller: component_ids::CRIME,
            target: component_ids::GBE,
            operation: CrimeLinkOperation::Pio(CrimePioRequest {
                address: 0x1600_0000,
                transfer: CrimeTransfer::read(4),
            }),
        });
        let CrimeLinkDeviceResponse::Complete(completion) = response else {
            panic!("GBE control status read was unexpectedly deferred");
        };
        let CrimeCompletionPayload::ReadData(data) = completion.result.unwrap() else {
            panic!("GBE control status read returned the wrong payload");
        };
        let control_status = u32::from_be_bytes(data.as_ref().try_into().unwrap());
        assert_eq!(control_status & (1 << 4), 0);

        for (id, address, value) in [(2, 0x1603_000c, 1_u32), (3, 0x1601_0000, 0)] {
            let response = gbe.accept(CrimeCgiTransaction {
                id: CrimeTransactionId::new(id),
                controller: component_ids::CRIME,
                target: component_ids::GBE,
                operation: CrimeLinkOperation::Pio(CrimePioRequest {
                    address,
                    transfer: CrimeTransfer::write(
                        value.to_be_bytes().into(),
                        CrimeByteEnable::from([true; 4]),
                    ),
                }),
            });
            assert!(matches!(response, CrimeLinkDeviceResponse::Complete(_)));
        }
        let (delay, event) = loop {
            let GbePoll::Action(action) = gbe.poll() else {
                panic!("the connected pixel clock did not schedule GBE timing");
            };
            if let GbeAction::Schedule { delay, event } = action {
                break (delay, event);
            }
        };
        gbe.observe_time(SimTime::new(delay.get()));
        gbe.handle_event(event);

        let response = gbe.accept(CrimeCgiTransaction {
            id: CrimeTransactionId::new(4),
            controller: component_ids::CRIME,
            target: component_ids::GBE,
            operation: CrimeLinkOperation::Pio(CrimePioRequest {
                address: 0x1603_0008,
                transfer: CrimeTransfer::read(4),
            }),
        });
        let CrimeLinkDeviceResponse::Complete(completion) = response else {
            panic!("GBE frame control read was unexpectedly deferred");
        };
        let CrimeCompletionPayload::ReadData(data) = completion.result.unwrap() else {
            panic!("GBE frame control read returned the wrong payload");
        };
        assert_eq!(u32::from_be_bytes(data.as_ref().try_into().unwrap()), 1);
    }

    #[test]
    fn display_update_moves_exact_frame_metadata_and_bytes_once() {
        let controller = EmulationController::new();
        {
            let mut state = lock_state(&controller.shared);
            state.snapshot.session_id = 42;
            reset_display_session(&mut state);
            update_display_output(&mut state, Some(display_frame(7, GbeFrameField::Second)), 3);
        }

        let update = controller.take_display_update();
        assert_eq!(update.generation, 1);
        assert_eq!(update.session_id, 42);
        assert!(update.has_frame);
        assert_eq!(update.sequence, 7);
        assert_eq!(update.completed_at, 70);
        assert_eq!(update.width, 2);
        assert_eq!(update.height, 1);
        assert_eq!(update.stride, 8);
        assert_eq!(update.field, UiDisplayField::Second);
        assert_eq!(update.rgba, [1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(update.machine_dropped, 3);
        assert_eq!(update.transport_dropped, 0);
        assert_eq!(update.invalid_frames, 0);

        let empty = controller.take_display_update();
        assert_eq!(empty.generation, update.generation);
        assert_eq!(empty.session_id, update.session_id);
        assert!(!empty.has_frame);
        assert!(empty.rgba.is_empty());
    }

    #[test]
    fn display_slot_keeps_latest_valid_frame_and_counts_every_loss_domain() {
        let controller = EmulationController::new();
        {
            let mut state = lock_state(&controller.shared);
            reset_display_session(&mut state);
            update_display_output(
                &mut state,
                Some(display_frame(1, GbeFrameField::Progressive)),
                4,
            );
            update_display_output(&mut state, Some(display_frame(2, GbeFrameField::First)), 5);

            let mut invalid = display_frame(3, GbeFrameField::Second);
            invalid.stride = 4;
            update_display_output(&mut state, Some(invalid), 6);
            state.display.transport_dropped = u64::MAX;
            update_display_output(&mut state, Some(display_frame(4, GbeFrameField::Second)), 7);
        }

        let update = controller.take_display_update();
        assert!(update.has_frame);
        assert_eq!(update.sequence, 4);
        assert_eq!(update.field, UiDisplayField::Second);
        assert_eq!(update.machine_dropped, 7);
        assert_eq!(update.transport_dropped, u64::MAX);
        assert_eq!(update.invalid_frames, 1);
    }

    #[test]
    fn display_validation_rejects_invalid_qimage_layouts_without_clearing_valid_data() {
        let valid = display_frame(1, GbeFrameField::Progressive);
        assert!(valid_display_frame(&valid));

        let mut zero_width = valid.clone();
        zero_width.width = 0;
        assert!(!valid_display_frame(&zero_width));

        let mut oversized_width = valid.clone();
        oversized_width.width = i32::MAX as u32 + 1;
        assert!(!valid_display_frame(&oversized_width));

        let mut short_stride = valid.clone();
        short_stride.stride = 7;
        assert!(!valid_display_frame(&short_stride));

        let mut short_data = valid;
        short_data.rgba.pop();
        assert!(!valid_display_frame(&short_data));
    }

    #[test]
    fn display_session_reset_clears_pending_data_and_counters() {
        let controller = EmulationController::new();
        {
            let mut state = lock_state(&controller.shared);
            update_display_output(
                &mut state,
                Some(display_frame(1, GbeFrameField::Progressive)),
                9,
            );
            state.display.transport_dropped = 8;
            state.display.invalid_frames = 7;
            reset_display_session(&mut state);
        }

        let update = controller.take_display_update();
        assert_eq!(update.generation, 1);
        assert!(!update.has_frame);
        assert_eq!(update.machine_dropped, 0);
        assert_eq!(update.transport_dropped, 0);
        assert_eq!(update.invalid_frames, 0);
    }

    #[test]
    fn controller_configures_runs_resets_and_shuts_down() {
        let controller = EmulationController::new();
        let initial_display_generation = controller.take_display_update().generation;
        assert_eq!(controller.snapshot().state, EmulationState::Unconfigured);
        assert!(!controller.configure_prom(&[]));

        let mut prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        prom[..4].copy_from_slice(&WAIT.to_be_bytes());
        assert!(controller.configure_prom(&prom));
        wait_for_state(&controller, EmulationState::Paused);
        let configured_display_generation =
            wait_for_display_generation(&controller, initial_display_generation);
        let first_session_id = controller.snapshot().session_id;
        assert_ne!(first_session_id, 0);

        assert!(controller.request_run());
        wait_for_state(&controller, EmulationState::Idle);
        assert!(controller.request_hard_reset());
        let reset_display_generation =
            wait_for_display_generation(&controller, configured_display_generation);
        wait_for_state(&controller, EmulationState::Paused);

        let prom = vec![0; IP32_PROM_IMAGE_SIZE_BYTES];
        assert!(controller.configure_prom(&prom));
        wait_for_state(&controller, EmulationState::Paused);
        assert!(wait_for_display_generation(&controller, reset_display_generation) > 0);
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
            false,
        ));
        wait_for_state(&controller, EmulationState::Paused);
        let first_session = controller.snapshot().session_id;
        let first_display_generation = controller.take_display_update().generation;

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
        assert!(controller.take_display_update().generation > first_display_generation);
    }
}
