//! Graphical control of an application-owned runtime.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use se_config::definition::MachineDefinition;
use se_config::draft::MachineDraft;
use se_cpu::mips1::r3000::debug::{
    CacheView, PendingCp0DebugSnapshot, PendingCp1DebugSnapshot, TlbView,
};
use se_machine::debug::{DebugRequest, DebugResponse};
use se_machine::indigo::ip12::debug::{
    DebugRequest as Ip12DebugRequest, DebugResponse as Ip12DebugResponse, MemoryAddressSpace,
};
use se_machine::input::MachineInput;
use se_machine::output::VideoOutput;
use se_machine::serial::SerialPort;
use se_runtime::control::{RuntimeMode, RuntimeState, RuntimeStatus};
use se_runtime::record::Replayer;
use se_runtime::runtime::{DebugReply, RuntimeConfiguration, RuntimeError, RuntimeHandle};

use crate::bridge::VideoFrameHandle;
use crate::bridge::ffi::{
    CacheDto, CacheEntryDto, DisassemblyDto, DisassemblyLineDto, MachineConfigurationEditDto,
    MachineConfigurationViewDto, MachineOutputSink, MemoryDto, NetworkConfiguration, RegistersDto,
    ReplaySnapshotCatalogDto, ReplaySnapshotInfoDto, RuntimeStatusDto, SerialPortDto,
    SgiMouseButtonDto, TlbDto, TlbEntryDto, UiExitState, UiStartupState, VideoOutputStateDto,
    run_gui,
};
use crate::configuration::{edit_from_dto, failed_view, view_dto};

/// Constructs a Normal machine from one owned configuration snapshot.
pub type NormalMachineBuilder = Box<
    dyn Fn(MachineDraft, &NetworkConfiguration) -> Result<RuntimeConfiguration, String>
        + Send
        + Sync
        + 'static,
>;

/// Constructs a cold Recording machine from one committed draft snapshot.
pub type RecordingMachineBuilder = Box<
    dyn Fn(MachineDraft, &NetworkConfiguration, PathBuf) -> Result<RuntimeConfiguration, String>
        + Send
        + Sync
        + 'static,
>;

/// Constructs a Replay machine using the current draft only for resource paths.
pub type ReplayMachineBuilder = Box<
    dyn Fn(MachineDraft, PathBuf, Option<String>) -> Result<RuntimeConfiguration, String>
        + Send
        + Sync
        + 'static,
>;

/// Validates editable network settings without constructing a machine or opening host resources.
pub type NetworkValidator =
    Box<dyn Fn(&NetworkConfiguration) -> Result<(), String> + Send + Sync + 'static>;

/// Controls the runtime during one Qt event loop.
pub struct UiSession {
    runtime: RuntimeHandle,
    definition: Arc<dyn MachineDefinition>,
    configuration: Mutex<MachineConfigurationState>,
    normal_builder: NormalMachineBuilder,
    recording_builder: RecordingMachineBuilder,
    replay_builder: ReplayMachineBuilder,
    network_validator: NetworkValidator,
}

struct MachineConfigurationState {
    committed: MachineDraft,
    editing: Option<MachineDraft>,
}

impl UiSession {
    /// Creates a session with application-provided construction and validation callbacks.
    #[must_use]
    pub fn new(
        runtime: RuntimeHandle,
        committed: MachineDraft,
        definition: Arc<dyn MachineDefinition>,
        normal_builder: NormalMachineBuilder,
        recording_builder: RecordingMachineBuilder,
        replay_builder: ReplayMachineBuilder,
        network_validator: NetworkValidator,
    ) -> Self {
        Self {
            runtime,
            definition,
            configuration: Mutex::new(MachineConfigurationState {
                committed,
                editing: None,
            }),
            normal_builder,
            recording_builder,
            replay_builder,
            network_validator,
        }
    }

    /// Runs the graphical user interface until its main window closes.
    pub fn run(&self, startup: &UiStartupState) -> UiExitState {
        run_gui(self, startup)
    }

    /// Samples current runtime status for Qt.
    pub fn runtime_status(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::status)
    }

    /// Validates network settings through the application without changing runtime state.
    ///
    /// Returns an empty string on success, or a user-visible validation error.
    pub fn validate_network_configuration(&self, configuration: &NetworkConfiguration) -> String {
        (self.network_validator)(configuration)
            .err()
            .unwrap_or_default()
    }

    /// Begins the single settings transaction and returns its resolved view.
    pub fn begin_machine_edit(&self) -> MachineConfigurationViewDto {
        let mut state = self
            .configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if state.editing.is_some() {
            return failed_view("machine editing is already active");
        }
        let candidate = state.committed.clone();
        let view = self.definition.resolve(&candidate);
        state.editing = Some(candidate);
        view_dto(view)
    }

    /// Applies an edit intent to the temporary draft and returns a fresh view.
    pub fn apply_machine_edit(
        &self,
        edit: &MachineConfigurationEditDto,
    ) -> MachineConfigurationViewDto {
        let edit = match edit_from_dto(edit) {
            Ok(edit) => edit,
            Err(error) => return failed_view(error),
        };
        let mut state = self
            .configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(editing) = state.editing.as_mut() else {
            return failed_view("machine editing is not active");
        };
        editing.apply(edit);
        view_dto(self.definition.resolve(editing))
    }

    /// Discards any temporary machine settings.
    pub fn cancel_machine_edit(&self) {
        self.configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .editing = None;
    }

    /// Reports whether the active transaction differs from committed settings.
    pub fn machine_edit_changed(&self) -> bool {
        let state = self
            .configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state
            .editing
            .as_ref()
            .is_some_and(|editing| *editing != state.committed)
    }

    /// Returns the committed Rust draft for application persistence.
    #[must_use]
    pub fn machine_draft_snapshot(&self) -> MachineDraft {
        self.configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .committed
            .clone()
    }

    /// Returns the current definition's display name.
    pub fn machine_display_name(&self) -> String {
        self.definition.display_name().to_owned()
    }

    /// Builds and installs the edited candidate, committing only after success.
    pub fn configure_edited_machine(&self, network: &NetworkConfiguration) -> RuntimeStatusDto {
        let candidate = {
            let state = self
                .configuration
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            let Some(candidate) = state.editing.as_ref() else {
                return failed_status(String::from("machine editing is not active"));
            };
            candidate.clone()
        };
        let result = self.build_and_configure(candidate.clone(), network);
        let mut state = self
            .configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        state.editing = None;
        if result.success {
            state.committed = candidate;
        }
        result
    }

    /// Rebuilds the committed Normal machine, including when leaving Replay.
    fn configure_machine(&self, network: &NetworkConfiguration) -> RuntimeStatusDto {
        self.build_and_configure(self.machine_draft_snapshot(), network)
    }

    fn build_and_configure(
        &self,
        draft: MachineDraft,
        network: &NetworkConfiguration,
    ) -> RuntimeStatusDto {
        let configuration = match (self.normal_builder)(draft, network) {
            Ok(configuration) => configuration,
            Err(error) => return failed_status(error),
        };
        self.runtime_command(|runtime| runtime.configure_with(configuration))
    }

    /// Starts continuous machine execution.
    pub fn run_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::run)
    }

    /// Resets and pauses the configured machine.
    pub fn reset_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::reset)
    }

    /// Pauses continuous machine execution.
    pub fn pause_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::pause)
    }

    /// Executes one instruction while paused.
    pub fn step_machine(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::step)
    }

    /// Cold-constructs a Recording machine and starts it from the first PROM
    /// instruction.
    pub fn run_with_record(&self, network: &NetworkConfiguration, path: &str) -> RuntimeStatusDto {
        let configuration = match (self.recording_builder)(
            self.machine_draft_snapshot(),
            network,
            PathBuf::from(path),
        ) {
            Ok(configuration) => configuration,
            Err(error) => return failed_status(error),
        };
        let status = self.runtime_command(|runtime| runtime.configure_with(configuration));
        if !status.success {
            return status;
        }
        self.runtime_command(RuntimeHandle::run)
    }

    /// Finalizes the active Record without changing Running or Paused state.
    pub fn stop_recording(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::stop_recording)
    }

    /// Cold-constructs and installs a paused Replay machine.
    pub fn open_replay(&self, path: &str, snapshot_id: &str) -> RuntimeStatusDto {
        let configuration = match (self.replay_builder)(
            self.machine_draft_snapshot(),
            PathBuf::from(path),
            (!snapshot_id.is_empty()).then(|| snapshot_id.to_owned()),
        ) {
            Ok(configuration) => configuration,
            Err(error) => return failed_status(error),
        };
        self.runtime_command(|runtime| runtime.configure_with(configuration))
    }

    /// Loads or rebuilds the manual snapshot catalog for one complete Record.
    #[must_use]
    pub fn replay_snapshot_catalog(&self, path: &str) -> ReplaySnapshotCatalogDto {
        match Replayer::snapshot_catalog(path) {
            Ok(snapshots) => ReplaySnapshotCatalogDto {
                success: true,
                error: String::new(),
                snapshots: snapshots
                    .into_iter()
                    .map(|snapshot| ReplaySnapshotInfoDto {
                        id: snapshot.id().to_owned(),
                        epoch: snapshot.position().epoch,
                        instructions: snapshot.position().completed_instructions,
                        pc: snapshot.pc(),
                    })
                    .collect(),
            },
            Err(error) => ReplaySnapshotCatalogDto {
                success: false,
                error: error.to_string(),
                snapshots: Vec::new(),
            },
        }
    }

    /// Creates a manual snapshot of the active paused Replay.
    pub fn create_replay_snapshot(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::create_replay_snapshot)
    }

    /// Discards the Replay machine and cold-constructs a paused Normal machine
    /// from current settings.
    pub fn stop_replay(&self, network: &NetworkConfiguration) -> RuntimeStatusDto {
        self.configure_machine(network)
    }

    /// Connects runtime machine output to the Qt delivery sink.
    pub fn attach_machine_output(
        &self,
        sink: cxx::SharedPtr<MachineOutputSink>,
    ) -> RuntimeStatusDto {
        if sink.is_null() {
            return failed_status(String::from("machine output sink is unavailable"));
        }

        self.runtime_command(|runtime| {
            runtime.set_output_handler(Box::new(move |output| {
                sink.publish_serial(output.serial(SerialPort::A), output.serial(SerialPort::B));
                let Some(video) = output.video() else {
                    return;
                };
                let (state, frame) = match video {
                    VideoOutput::NoGraphicsBoard => (
                        VideoOutputStateDto::NoGraphicsBoard,
                        VideoFrameHandle::empty(),
                    ),
                    VideoOutput::NoSignal => {
                        (VideoOutputStateDto::NoSignal, VideoFrameHandle::empty())
                    }
                    VideoOutput::Active { frame: None } => {
                        (VideoOutputStateDto::Blank, VideoFrameHandle::empty())
                    }
                    VideoOutput::Active { frame: Some(frame) } => (
                        VideoOutputStateDto::Frame,
                        VideoFrameHandle::new(frame.clone()),
                    ),
                };
                sink.publish_video(state, Box::new(frame));
            }))
        })
    }

    /// Disconnects the Qt delivery sink from the runtime worker.
    pub fn detach_machine_output(&self) {
        let _ = self.runtime.clear_output_handler();
    }

    /// Samples processor registers and pending effects.
    pub fn registers(&self) -> RegistersDto {
        let reply = match self.debug(Ip12DebugRequest::Registers) {
            Ok(reply) => reply,
            Err(error) => return failed_registers(error.to_string()),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Registers(snapshot)) = reply.response
        else {
            return failed_registers(String::from("runtime returned an unexpected response"));
        };
        let cpu = snapshot.cpu;
        let pending_effective = cpu.cp0.pending_functional.map_or_else(Vec::new, |state| {
            vec![
                state.coprocessor_usable,
                state.interrupt_control,
                state.software_interrupts,
            ]
        });

        RegistersDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            pc: cpu.pc,
            hi: cpu.hi,
            lo: cpu.lo,
            gpr: cpu.gpr.into(),
            delay_slot: cpu.delay_slot.map_or_else(
                || String::from("none"),
                |slot| format!("0x{:08x} -> 0x{:08x}", slot.origin_pc, slot.resume_pc),
            ),
            pending_gpr: cpu.pending_gpr.map_or_else(
                || String::from("none"),
                |write| format!("${} = 0x{:08x}", write.index, write.value),
            ),
            pending_cp0: format_pending_cp0(cpu.pending_cp0),
            pending_cp1: format_pending_cp1(cpu.pending_cp1),
            cp0: cpu.cp0.registers.into(),
            cp0_effective: vec![
                cpu.cp0.effective.coprocessor_usable,
                cpu.cp0.effective.interrupt_control,
                cpu.cp0.effective.software_interrupts,
            ],
            cp0_pending_effective: pending_effective,
            cp1: cpu.cp1.registers.into(),
            fcr0: cpu.cp1.fcr0,
            fcr30: cpu.cp1.fcr30,
            fcr31: cpu.cp1.fcr31,
            float_backend: format!("{:?}", cpu.cp1.backend),
            cp1_interrupt: cpu.cp1.interrupt_asserted,
        }
    }

    /// Samples one TLB view.
    pub fn tlb(&self, instruction_view: bool) -> TlbDto {
        let view = if instruction_view {
            TlbView::Instruction
        } else {
            TlbView::Main
        };
        let reply = match self.debug(Ip12DebugRequest::Tlb(view)) {
            Ok(reply) => reply,
            Err(error) => return failed_tlb(error.to_string(), instruction_view),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Tlb(snapshot)) = reply.response else {
            return failed_tlb(
                String::from("runtime returned an unexpected response"),
                instruction_view,
            );
        };

        TlbDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            instruction_view,
            shutdown: snapshot.shutdown,
            index: snapshot.index as u32,
            random: snapshot.random as u32,
            entries: snapshot
                .entries
                .into_iter()
                .map(|entry| TlbEntryDto {
                    index: entry.index as u32,
                    entry_hi: entry.entry_hi,
                    entry_lo: entry.entry_lo,
                    vpn: entry.vpn,
                    asid: entry.asid,
                    pfn: entry.pfn,
                    noncacheable: entry.noncacheable,
                    dirty: entry.dirty,
                    valid: entry.valid,
                    global: entry.global,
                })
                .collect(),
        }
    }

    /// Samples one physical cache bank.
    pub fn cache(&self, instruction_cache: bool) -> CacheDto {
        let view = if instruction_cache {
            CacheView::Instruction
        } else {
            CacheView::Data
        };
        let reply = match self.debug(Ip12DebugRequest::Cache(view)) {
            Ok(reply) => reply,
            Err(error) => return failed_cache(error.to_string(), instruction_cache),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Cache(snapshot)) = reply.response else {
            return failed_cache(
                String::from("runtime returned an unexpected response"),
                instruction_cache,
            );
        };

        CacheDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            instruction_cache,
            refill_bytes: snapshot.refill_bytes as u32,
            entries: snapshot
                .entries
                .into_iter()
                .map(|entry| CacheEntryDto {
                    index: entry.index as u32,
                    page_frame: entry.page_frame,
                    word: entry.word,
                    valid: entry.valid,
                })
                .collect(),
        }
    }

    /// Reads and disassembles virtual instructions.
    pub fn disassembly(&self, start: u32, row_count: u32) -> DisassemblyDto {
        let reply = match self.debug(Ip12DebugRequest::Disassembly {
            start,
            row_count: row_count as usize,
        }) {
            Ok(reply) => reply,
            Err(error) => return failed_disassembly(error.to_string()),
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Disassembly(lines)) = reply.response
        else {
            return failed_disassembly(String::from("runtime returned an unexpected response"));
        };
        let current_pc = reply.execution_address;

        DisassemblyDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            lines: lines
                .into_iter()
                .map(|line| DisassemblyLineDto {
                    address: line.address,
                    readable: line.word.is_some(),
                    word: line.word.unwrap_or(0),
                    text: line.text,
                    current: current_pc == line.address,
                    breakpoint: reply.breakpoints.binary_search(&line.address).is_ok(),
                })
                .collect(),
        }
    }

    /// Reads a side-effect-free physical or virtual memory range.
    pub fn memory(&self, virtual_address_space: bool, start: u64, length: u32) -> MemoryDto {
        let address_space = if virtual_address_space {
            MemoryAddressSpace::Virtual
        } else {
            MemoryAddressSpace::Physical
        };
        let reply = match self.debug(Ip12DebugRequest::Memory {
            address_space,
            start,
            length: length as usize,
        }) {
            Ok(reply) => reply,
            Err(error) => {
                return failed_memory(error.to_string(), virtual_address_space, start);
            }
        };
        let DebugResponse::IndigoIp12(Ip12DebugResponse::Memory(snapshot)) = reply.response else {
            return failed_memory(
                String::from("runtime returned an unexpected response"),
                virtual_address_space,
                start,
            );
        };
        let mut values = Vec::with_capacity(snapshot.bytes.len());
        let mut readable = Vec::with_capacity(snapshot.bytes.len());
        for byte in snapshot.bytes {
            values.push(byte.unwrap_or(0));
            readable.push(u8::from(byte.is_some()));
        }

        MemoryDto {
            success: true,
            error: String::new(),
            revision: reply.revision,
            virtual_address_space,
            start,
            values,
            readable,
        }
    }

    /// Adds or removes one virtual execution breakpoint.
    pub fn toggle_breakpoint(&self, address: u32) -> RuntimeStatusDto {
        match self.runtime.toggle_breakpoint(address) {
            Ok(status) => status_dto(status),
            Err(error) => failed_status(error.to_string()),
        }
    }

    /// Supplies one byte batch to an external serial port.
    pub fn send_serial(&self, port: SerialPortDto, bytes: &[u8]) -> RuntimeStatusDto {
        let port = match port {
            SerialPortDto::A => SerialPort::A,
            SerialPortDto::B => SerialPort::B,
            _ => return failed_status(String::from("unsupported serial port")),
        };
        for value in bytes {
            if let Err(error) = self.runtime.send_input(MachineInput::SerialByte {
                port,
                value: *value,
            }) {
                return failed_status(error.to_string());
            }
        }
        self.runtime_command(RuntimeHandle::status)
    }

    /// Enqueues one validated physical SGI keyboard transition.
    pub fn send_sgi_key(&self, code: u8, pressed: bool) -> bool {
        let Some(input) = MachineInput::sgi_keyboard(code, pressed) else {
            return false;
        };
        self.runtime.send_input(input).is_ok()
    }

    /// Enqueues normalized relative SGI mouse motion.
    pub fn send_sgi_mouse_motion(&self, delta_x: i32, delta_y: i32) -> bool {
        self.runtime
            .send_input(MachineInput::SgiMouseMotion { delta_x, delta_y })
            .is_ok()
    }

    /// Enqueues one physical SGI mouse button transition.
    pub fn send_sgi_mouse_button(&self, button: SgiMouseButtonDto, pressed: bool) -> bool {
        let code = match button {
            SgiMouseButtonDto::Left => 0,
            SgiMouseButtonDto::Middle => 1,
            SgiMouseButtonDto::Right => 2,
            _ => return false,
        };
        let Some(input) = MachineInput::sgi_mouse_button(code, pressed) else {
            return false;
        };
        self.runtime.send_input(input).is_ok()
    }

    fn runtime_command(
        &self,
        command: impl FnOnce(&RuntimeHandle) -> Result<RuntimeStatus, RuntimeError>,
    ) -> RuntimeStatusDto {
        match command(&self.runtime) {
            Ok(status) => status_dto(status),
            Err(error) => failed_status(error.to_string()),
        }
    }

    fn debug(&self, request: Ip12DebugRequest) -> Result<DebugReply, RuntimeError> {
        self.runtime.debug(DebugRequest::IndigoIp12(request))
    }
}

fn status_dto(status: RuntimeStatus) -> RuntimeStatusDto {
    let replay_final_position = status.replay_final_position.unwrap_or_default();
    RuntimeStatusDto {
        can_execute: status.can_execute,
        success: true,
        state: state_identifier(status.state),
        revision: status.revision,
        completed_instructions: status.completed_instructions,
        mode: mode_identifier(status.mode),
        epoch: status.position.epoch,
        epoch_instructions: status.position.completed_instructions,
        has_replay_final_position: status.replay_final_position.is_some(),
        replay_final_epoch: replay_final_position.epoch,
        replay_final_instructions: replay_final_position.completed_instructions,
        session_error: status.session_error.unwrap_or_default(),
        execution_error: status.last_error.unwrap_or_default(),
        command_error: String::new(),
    }
}

const fn mode_identifier(mode: RuntimeMode) -> u8 {
    match mode {
        RuntimeMode::Normal => 0,
        RuntimeMode::Recording => 1,
        RuntimeMode::Replaying => 2,
        RuntimeMode::ReplayCompleted => 3,
        RuntimeMode::ReplayDiverged => 4,
        RuntimeMode::RecordCompleted => 5,
    }
}

const fn state_identifier(state: RuntimeState) -> u8 {
    match state {
        RuntimeState::Unconfigured => 0,
        RuntimeState::Paused => 1,
        RuntimeState::Running => 2,
    }
}

fn failed_status(error: String) -> RuntimeStatusDto {
    RuntimeStatusDto {
        can_execute: false,
        success: false,
        state: 0,
        revision: 0,
        completed_instructions: 0,
        mode: 0,
        epoch: 0,
        epoch_instructions: 0,
        has_replay_final_position: false,
        replay_final_epoch: 0,
        replay_final_instructions: 0,
        session_error: String::new(),
        execution_error: String::new(),
        command_error: error,
    }
}

fn failed_registers(error: String) -> RegistersDto {
    RegistersDto {
        success: false,
        error,
        revision: 0,
        pc: 0,
        hi: 0,
        lo: 0,
        gpr: Vec::new(),
        delay_slot: String::new(),
        pending_gpr: String::new(),
        pending_cp0: String::new(),
        pending_cp1: String::new(),
        cp0: Vec::new(),
        cp0_effective: Vec::new(),
        cp0_pending_effective: Vec::new(),
        cp1: Vec::new(),
        fcr0: 0,
        fcr30: 0,
        fcr31: 0,
        float_backend: String::new(),
        cp1_interrupt: false,
    }
}

fn failed_tlb(error: String, instruction_view: bool) -> TlbDto {
    TlbDto {
        success: false,
        error,
        revision: 0,
        instruction_view,
        shutdown: false,
        index: 0,
        random: 0,
        entries: Vec::new(),
    }
}

fn failed_cache(error: String, instruction_cache: bool) -> CacheDto {
    CacheDto {
        success: false,
        error,
        revision: 0,
        instruction_cache,
        refill_bytes: 0,
        entries: Vec::new(),
    }
}

fn failed_disassembly(error: String) -> DisassemblyDto {
    DisassemblyDto {
        success: false,
        error,
        revision: 0,
        lines: Vec::new(),
    }
}

fn failed_memory(error: String, virtual_address_space: bool, start: u64) -> MemoryDto {
    MemoryDto {
        success: false,
        error,
        revision: 0,
        virtual_address_space,
        start,
        values: Vec::new(),
        readable: Vec::new(),
    }
}

fn format_pending_cp0(pending: Option<PendingCp0DebugSnapshot>) -> String {
    match pending {
        None => String::from("none"),
        Some(PendingCp0DebugSnapshot { index, value }) => {
            format!("${index} = 0x{value:08x}")
        }
    }
}

fn format_pending_cp1(pending: Option<PendingCp1DebugSnapshot>) -> String {
    match pending {
        None => String::from("none"),
        Some(PendingCp1DebugSnapshot::General { index, value }) => {
            format!("$f{index} = 0x{value:08x}")
        }
        Some(PendingCp1DebugSnapshot::Control { index, value }) => {
            format!("FCR{index} = 0x{value:08x}")
        }
        Some(PendingCp1DebugSnapshot::Condition { value }) => {
            format!("condition = {}", u8::from(value))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use se_config::definition::MachineDefinition;
    use se_config::draft::{Edit, MachineDraft};
    use se_config::id::PropertyId;
    use se_config::value::PropertyValue;
    use se_machine::indigo::ip12::builder;
    use se_machine::indigo::ip12::definition::Ip12Definition;
    use se_machine::machine::Machine;
    use se_machine::resource::{PreparedResource, ResourceKind};
    use se_runtime::runtime::{Runtime, RuntimeConfiguration};

    use super::UiSession;
    use crate::bridge::ffi::{
        MachineConfigurationEditDto, MachinePropertyValueDto, NetworkConfiguration,
    };

    fn draft() -> MachineDraft {
        let mut draft = Ip12Definition.default_draft();
        draft.apply(Edit::SetProperty {
            property: PropertyId(String::from("firmware.0.image-path")),
            value: PropertyValue::Text(String::from("prom.bin")),
        });
        draft
    }

    fn network() -> NetworkConfiguration {
        NetworkConfiguration {
            subnet: String::new(),
            gateway: String::new(),
            dns: String::new(),
            dhcp_start: String::new(),
            forwards: Vec::new(),
        }
    }

    fn edit_firmware(path: &str) -> MachineConfigurationEditDto {
        MachineConfigurationEditDto {
            kind: 0,
            target_id: String::from("firmware.0.image-path"),
            value: MachinePropertyValueDto {
                kind: 2,
                bool_value: false,
                integer_value: 0,
                text_value: path.into(),
            },
            device_id: String::new(),
        }
    }

    fn session(normal_succeeds: bool) -> (Runtime, UiSession) {
        let runtime = Runtime::new_unconfigured().unwrap();
        let session = UiSession::new(
            runtime.handle(),
            draft(),
            Arc::new(Ip12Definition),
            Box::new(move |draft, _network| {
                if !normal_succeeds {
                    return Err(String::from("injected builder stop"));
                }
                let plan = Ip12Definition
                    .compile(&draft)
                    .map_err(|error| error.to_string())?;
                let prepared = plan
                    .prepare_with(|_, requirement| match requirement.kind {
                        ResourceKind::Bytes => {
                            Ok::<_, String>(PreparedResource::Bytes(vec![0; 0x40000]))
                        }
                        ResourceKind::Storage { .. } => Err(String::from("unexpected storage")),
                    })
                    .map_err(|error| error.to_string())?;
                let machine = Machine::IndigoIp12(
                    builder::build(prepared).map_err(|error| error.to_string())?,
                );
                Ok(RuntimeConfiguration::normal(machine))
            }),
            Box::new(|_, _, _| Err(String::from("unused recording builder"))),
            Box::new(|_, _, _| Err(String::from("unused replay builder"))),
            Box::new(|_| Ok(())),
        );
        (runtime, session)
    }

    #[test]
    fn edit_transaction_keeps_committed_draft_until_success() {
        let (runtime, session) = session(true);
        let original = session.machine_draft_snapshot();
        let view = session.begin_machine_edit();
        assert!(view.success);
        assert!(!session.begin_machine_edit().success);
        assert!(view.nodes.iter().any(|node| node.id == "indigo-ip12"));
        let updated = session.apply_machine_edit(&edit_firmware("other.bin"));
        assert!(updated.success);
        assert!(session.machine_edit_changed());
        assert_eq!(session.machine_draft_snapshot(), original);
        session.cancel_machine_edit();
        assert!(!session.machine_edit_changed());
        assert_eq!(session.machine_draft_snapshot(), original);
        assert!(session.begin_machine_edit().success);
        assert!(
            session
                .apply_machine_edit(&edit_firmware("accepted.bin"))
                .success
        );
        assert!(session.configure_edited_machine(&network()).success);
        assert_eq!(
            session
                .machine_draft_snapshot()
                .properties
                .get(&PropertyId(String::from("firmware.0.image-path"))),
            Some(&PropertyValue::Text(String::from("accepted.bin")))
        );
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn failed_build_discards_edit_without_changing_committed_draft() {
        let (runtime, session) = session(false);
        let original = session.machine_draft_snapshot();
        assert!(session.begin_machine_edit().success);
        assert!(
            session
                .apply_machine_edit(&edit_firmware("missing.bin"))
                .success
        );
        let status = session.configure_edited_machine(&network());
        assert!(!status.success);
        assert_eq!(status.command_error, "injected builder stop");
        assert_eq!(session.machine_draft_snapshot(), original);
        assert!(!session.machine_edit_changed());
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn semantically_invalid_edit_still_returns_a_view() {
        let (runtime, session) = session(false);
        assert!(session.begin_machine_edit().success);
        let result = session.apply_machine_edit(&MachineConfigurationEditDto {
            kind: 0,
            target_id: String::from("memory.bank.a.simm-mib"),
            value: MachinePropertyValueDto {
                kind: 1,
                bool_value: false,
                integer_value: 0,
                text_value: String::new(),
            },
            device_id: String::new(),
        });
        assert!(result.success);
        assert!(
            result
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.code == "ip12.memory.no-installed-bank")
        );
        session.cancel_machine_edit();
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn network_validation_uses_application_callback_without_building() {
        let observed = Arc::new(Mutex::new(Vec::new()));
        let validator_observed = Arc::clone(&observed);
        let runtime = Runtime::new_unconfigured().unwrap();
        let session = UiSession::new(
            runtime.handle(),
            draft(),
            Arc::new(Ip12Definition),
            Box::new(|_, _| panic!("validation must not construct a machine")),
            Box::new(|_, _, _| panic!("validation must not construct a Recording machine")),
            Box::new(|_, _, _| panic!("validation must not construct a Replay machine")),
            Box::new(move |configuration| {
                validator_observed
                    .lock()
                    .unwrap()
                    .push(configuration.subnet.clone());
                if configuration.subnet == "rejected" {
                    Err(String::from("application validation error"))
                } else {
                    Ok(())
                }
            }),
        );
        let mut network = network();
        network.subnet = String::from("rejected");
        assert_eq!(
            session.validate_network_configuration(&network),
            "application validation error"
        );
        network.subnet = String::from("accepted");
        assert!(session.validate_network_configuration(&network).is_empty());
        assert_eq!(*observed.lock().unwrap(), ["rejected", "accepted"]);
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn replay_bridge_preserves_selected_snapshot_identifier() {
        let observed = Arc::new(Mutex::new(None));
        let builder_observed = Arc::clone(&observed);
        let runtime = Runtime::new_unconfigured().unwrap();
        let session = UiSession::new(
            runtime.handle(),
            draft(),
            Arc::new(Ip12Definition),
            Box::new(|_, _| Err(String::from("unused normal builder"))),
            Box::new(|_, _, _| Err(String::from("unused recording builder"))),
            Box::new(move |_draft, path, snapshot_id| {
                *builder_observed.lock().unwrap() = Some((path, snapshot_id));
                Err(String::from("injected builder stop"))
            }),
            Box::new(|_| Ok(())),
        );
        let status = session.open_replay("recording.serec", "point.ckpt");
        assert!(!status.success);
        let observed = observed.lock().unwrap().take().unwrap();
        assert_eq!(observed.0, Path::new("recording.serec"));
        assert_eq!(observed.1.as_deref(), Some("point.ckpt"));
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn replay_snapshot_bridge_reports_catalog_and_runtime_errors() {
        let (runtime, session) = session(false);
        let catalog = session.replay_snapshot_catalog("missing-record.serec");
        assert!(!catalog.success);
        assert!(catalog.snapshots.is_empty());
        assert!(!session.create_replay_snapshot().success);
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn sgi_key_bridge_accepts_exactly_protocol_keycodes() {
        let (runtime, session) = session(false);
        let accepted: Vec<_> = (0..=u8::MAX)
            .filter(|code| session.send_sgi_key(*code, true))
            .collect();
        assert_eq!(accepted.len(), 101);
        assert_eq!(accepted.first().copied(), Some(2));
        assert_eq!(accepted.last().copied(), Some(109));
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn dropping_ui_session_keeps_the_application_runtime_alive() {
        let (runtime, session) = session(true);
        assert!(session.configure_machine(&network()).success);
        drop(session);
        assert!(runtime.handle().status().is_ok());
        assert!(runtime.shutdown().unwrap().is_some());
    }
}
