//! Graphical control of an application-owned runtime.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use se_config::definition::MachineDefinition;
use se_config::draft::MachineDraft;
use se_cpu::mips1::r3000::debug::{
    CacheView, PendingCp0DebugSnapshot, PendingCp1DebugSnapshot, TlbView,
};
use se_machine::debug::{DebugRequest, DebugResponse};
use se_machine::endpoint::{EndpointDirection, EndpointKind};
use se_machine::indigo::ip12::debug::{
    DebugRequest as Ip12DebugRequest, DebugResponse as Ip12DebugResponse, MemoryAddressSpace,
};
use se_machine::input::{KeyboardKey, KeyboardNamedKey, PointerButton};
use se_machine::output::VideoOutput;
use se_runtime::control::{RuntimeMode, RuntimeState, RuntimeStatus};
use se_runtime::endpoint::{EndpointHandle, RuntimeOutputPayload};
use se_runtime::record::Replayer;
use se_runtime::runtime::{DebugReply, RuntimeError, RuntimeHandle};
use se_session::frontend::{FrontendPlan, SessionBuild};

use crate::bridge::VideoFrameHandle;
use crate::bridge::ffi::{
    CacheDto, CacheEntryDto, DisassemblyDto, DisassemblyLineDto, EndpointCatalogDto,
    EndpointDescriptorDto, EndpointDirectionDto, EndpointHandleDto, EndpointKindDto,
    KeyboardKeyDto, KeyboardKeyKindDto, MachineConfigurationEditDto, MachineConfigurationViewDto,
    MachineOutputSink, MemoryDto, NetworkConfiguration, PointerButtonDto, RegistersDto,
    ReplaySnapshotCatalogDto, ReplaySnapshotInfoDto, RuntimeStatusDto, TlbDto, TlbEntryDto,
    UiExitState, UiStartupState, VideoOutputStateDto, run_gui,
};
use crate::configuration::{edit_from_dto, failed_view, view_dto};

/// Constructs a Normal machine from one owned configuration snapshot.
pub type NormalMachineBuilder = Box<
    dyn Fn(MachineDraft, &NetworkConfiguration) -> Result<SessionBuild, String>
        + Send
        + Sync
        + 'static,
>;

/// Constructs a cold Recording machine from one committed draft snapshot.
pub type RecordingMachineBuilder = Box<
    dyn Fn(MachineDraft, &NetworkConfiguration, PathBuf) -> Result<SessionBuild, String>
        + Send
        + Sync
        + 'static,
>;

/// Constructs a Replay machine using the current draft only for resource paths.
pub type ReplayMachineBuilder = Box<
    dyn Fn(MachineDraft, PathBuf, Option<String>) -> Result<SessionBuild, String>
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
    active_frontend: FrontendPlan,
}

impl UiSession {
    /// Creates a session with application-provided construction and validation callbacks.
    #[must_use]
    #[allow(
        clippy::too_many_arguments,
        reason = "the UI session receives distinct lifecycle capabilities from the application"
    )]
    pub fn new(
        runtime: RuntimeHandle,
        committed: MachineDraft,
        definition: Arc<dyn MachineDefinition>,
        active_frontend: FrontendPlan,
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
                active_frontend,
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

    /// Samples the current endpoint catalog for Qt.
    pub fn endpoint_catalog(&self) -> EndpointCatalogDto {
        let state = self
            .configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let frontend = &state.active_frontend;
        match self.runtime.endpoint_catalog() {
            Ok(catalog) => EndpointCatalogDto {
                success: true,
                error: String::new(),
                generation: catalog.generation(),
                endpoints: catalog
                    .endpoints()
                    .iter()
                    .map(|descriptor| EndpointDescriptorDto {
                        handle: endpoint_handle_dto(descriptor.handle()),
                        label: descriptor.label().into(),
                        kind: endpoint_kind_dto(descriptor.kind()),
                        direction: endpoint_direction_dto(descriptor.direction()),
                        serial_console_attached: frontend
                            .serial_console_endpoints()
                            .contains(descriptor.handle().key()),
                    })
                    .collect(),
            },
            Err(error) => EndpointCatalogDto {
                success: false,
                error: error.to_string(),
                generation: 0,
                endpoints: Vec::new(),
            },
        }
    }

    /// Republishes current video states after endpoint widgets are rebuilt.
    pub fn refresh_outputs(&self) -> RuntimeStatusDto {
        self.runtime_command(RuntimeHandle::refresh_outputs)
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
        let build = match (self.normal_builder)(draft, network) {
            Ok(build) => build,
            Err(error) => return failed_status(error),
        };
        self.install(build)
    }

    fn install(&self, build: SessionBuild) -> RuntimeStatusDto {
        let (configuration, frontend) = build.into_parts();
        let mut state = self
            .configuration
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let status = self.runtime_command(|runtime| runtime.configure_with(configuration));
        if status.success {
            state.active_frontend = frontend;
        }
        status
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
        let build = match (self.recording_builder)(
            self.machine_draft_snapshot(),
            network,
            PathBuf::from(path),
        ) {
            Ok(build) => build,
            Err(error) => return failed_status(error),
        };
        let status = self.install(build);
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
        let build = match (self.replay_builder)(
            self.machine_draft_snapshot(),
            PathBuf::from(path),
            (!snapshot_id.is_empty()).then(|| snapshot_id.to_owned()),
        ) {
            Ok(build) => build,
            Err(error) => return failed_status(error),
        };
        self.install(build)
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
                for item in output {
                    let generation = item.handle().generation();
                    let key = item.handle().key().as_str();
                    match item.payload() {
                        RuntimeOutputPayload::Serial(bytes) => {
                            sink.publish_serial(generation, key, bytes)
                        }
                        RuntimeOutputPayload::Video(video) => {
                            let (state, frame) = match video {
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
                            sink.publish_video(generation, key, state, Box::new(frame));
                        }
                    }
                }
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

    /// Supplies one character to an exact live serial endpoint.
    pub fn send_serial(&self, handle: &EndpointHandleDto, value: u8) -> RuntimeStatusDto {
        match self
            .resolve_endpoint_handle(handle)
            .and_then(|handle| self.runtime.send_serial(handle, value))
        {
            Ok(status) => status_dto(status),
            Err(error) => failed_status(error.to_string()),
        }
    }

    /// Sends one frontend-neutral keyboard transition.
    pub fn send_keyboard(
        &self,
        handle: &EndpointHandleDto,
        key: KeyboardKeyDto,
        pressed: bool,
    ) -> bool {
        let Some(key) = keyboard_key_from_dto(key) else {
            return false;
        };
        self.resolve_endpoint_handle(handle)
            .and_then(|handle| self.runtime.send_keyboard(handle, key, pressed))
            .is_ok()
    }

    /// Sends normalized relative pointer motion.
    pub fn send_pointer_motion(
        &self,
        handle: &EndpointHandleDto,
        delta_x: i32,
        delta_y: i32,
    ) -> bool {
        self.resolve_endpoint_handle(handle)
            .and_then(|handle| self.runtime.send_pointer_motion(handle, delta_x, delta_y))
            .is_ok()
    }

    /// Sends one frontend-neutral pointer button transition.
    pub fn send_pointer_button(
        &self,
        handle: &EndpointHandleDto,
        button: PointerButtonDto,
        pressed: bool,
    ) -> bool {
        let button = match button {
            PointerButtonDto::Left => PointerButton::Left,
            PointerButtonDto::Middle => PointerButton::Middle,
            PointerButtonDto::Right => PointerButton::Right,
            _ => return false,
        };
        self.resolve_endpoint_handle(handle)
            .and_then(|handle| self.runtime.send_pointer_button(handle, button, pressed))
            .is_ok()
    }

    fn resolve_endpoint_handle(
        &self,
        handle: &EndpointHandleDto,
    ) -> Result<EndpointHandle, RuntimeError> {
        let catalog = self.runtime.endpoint_catalog()?;
        if handle.generation != catalog.generation() {
            return Err(RuntimeError::StaleEndpoint);
        }
        catalog
            .endpoints()
            .iter()
            .find(|descriptor| descriptor.handle().key().as_str() == handle.key)
            .map(|descriptor| descriptor.handle().clone())
            .ok_or(RuntimeError::UnknownEndpoint)
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

fn endpoint_handle_dto(handle: &EndpointHandle) -> EndpointHandleDto {
    EndpointHandleDto {
        generation: handle.generation(),
        key: handle.key().as_str().into(),
    }
}

const fn endpoint_kind_dto(kind: EndpointKind) -> EndpointKindDto {
    match kind {
        EndpointKind::Serial => EndpointKindDto::Serial,
        EndpointKind::Keyboard => EndpointKindDto::Keyboard,
        EndpointKind::Pointer => EndpointKindDto::Pointer,
        EndpointKind::Ethernet => EndpointKindDto::Ethernet,
        EndpointKind::Video => EndpointKindDto::Video,
    }
}

const fn endpoint_direction_dto(direction: EndpointDirection) -> EndpointDirectionDto {
    match direction {
        EndpointDirection::Input => EndpointDirectionDto::Input,
        EndpointDirection::Output => EndpointDirectionDto::Output,
        EndpointDirection::Bidirectional => EndpointDirectionDto::Bidirectional,
    }
}

fn keyboard_key_from_dto(key: KeyboardKeyDto) -> Option<KeyboardKey> {
    match key.kind {
        KeyboardKeyKindDto::Letter if key.value.is_ascii_uppercase() => {
            Some(KeyboardKey::Letter(key.value))
        }
        KeyboardKeyKindDto::Digit if key.value <= 9 => Some(KeyboardKey::Digit(key.value)),
        KeyboardKeyKindDto::KeypadDigit if key.value <= 9 => {
            Some(KeyboardKey::KeypadDigit(key.value))
        }
        KeyboardKeyKindDto::Function if (1..=12).contains(&key.value) => {
            Some(KeyboardKey::Function(key.value))
        }
        KeyboardKeyKindDto::Named => {
            use KeyboardNamedKey as K;
            let named = match key.value {
                0 => K::LeftControl,
                1 => K::RightControl,
                2 => K::LeftShift,
                3 => K::RightShift,
                4 => K::LeftAlt,
                5 => K::RightAlt,
                6 => K::CapsLock,
                7 => K::Escape,
                8 => K::Tab,
                9 => K::Enter,
                10 => K::Backspace,
                11 => K::Delete,
                12 => K::Space,
                13 => K::ArrowLeft,
                14 => K::ArrowRight,
                15 => K::ArrowUp,
                16 => K::ArrowDown,
                17 => K::Insert,
                18 => K::Home,
                19 => K::End,
                20 => K::PageUp,
                21 => K::PageDown,
                22 => K::PrintScreen,
                23 => K::ScrollLock,
                24 => K::Pause,
                25 => K::NumLock,
                26 => K::Semicolon,
                27 => K::Comma,
                28 => K::Minus,
                29 => K::LeftBracket,
                30 => K::RightBracket,
                31 => K::Apostrophe,
                32 => K::Period,
                33 => K::Slash,
                34 => K::Equal,
                35 => K::Grave,
                36 => K::Backslash,
                37 => K::KeypadPeriod,
                38 => K::KeypadMinus,
                39 => K::KeypadPlus,
                40 => K::KeypadSlash,
                41 => K::KeypadAsterisk,
                42 => K::KeypadEnter,
                _ => return None,
            };
            Some(KeyboardKey::Named(named))
        }
        _ => None,
    }
}

fn status_dto(status: RuntimeStatus) -> RuntimeStatusDto {
    let replay_final_position = status.replay_final_position.unwrap_or_default();
    RuntimeStatusDto {
        can_execute: status.can_execute,
        success: true,
        state: state_identifier(status.state),
        revision: status.revision,
        machine_generation: status.machine_generation,
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
        machine_generation: 0,
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
    use std::fs;
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex};

    use se_config::definition::MachineDefinition;
    use se_config::draft::{Edit, MachineDraft};
    use se_config::id::{NodeId, PropertyId};
    use se_config::value::PropertyValue;
    use se_machine::indigo::ip12::builder;
    use se_machine::indigo::ip12::definition::Ip12Definition;
    use se_machine::machine::Machine;
    use se_machine::resource::{PreparedResource, ResourceKind};
    use se_network::config::NatConfig;
    use se_runtime::runtime::Runtime;
    use se_session::frontend::FrontendPlan;

    use super::UiSession;
    use crate::bridge::ffi::{
        EndpointKindDto, KeyboardKeyDto, KeyboardKeyKindDto, MachineConfigurationEditDto,
        MachinePropertyValueDto, NetworkConfiguration,
    };

    static NEXT_FIRMWARE_ID: AtomicU64 = AtomicU64::new(0);

    struct TestFirmware {
        path: std::path::PathBuf,
    }

    impl TestFirmware {
        fn new() -> Self {
            let id = NEXT_FIRMWARE_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "sgi-emu-ui-session-prom-{}-{id}.bin",
                std::process::id()
            ));
            fs::write(&path, vec![0; 0x40000]).unwrap();
            Self { path }
        }
    }

    impl Drop for TestFirmware {
        fn drop(&mut self) {
            fs::remove_file(&self.path).unwrap();
        }
    }

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

    fn detach_port(port: &str) -> MachineConfigurationEditDto {
        set_port(port, "")
    }

    fn set_port(port: &str, device: &str) -> MachineConfigurationEditDto {
        MachineConfigurationEditDto {
            kind: 1,
            target_id: String::from(port),
            value: MachinePropertyValueDto {
                kind: 0,
                bool_value: false,
                integer_value: 0,
                text_value: String::new(),
            },
            device_id: String::from(device),
        }
    }

    fn attached_serial_keys(session: &UiSession) -> Vec<String> {
        session
            .endpoint_catalog()
            .endpoints
            .into_iter()
            .filter(|endpoint| {
                endpoint.kind == EndpointKindDto::Serial && endpoint.serial_console_attached
            })
            .map(|endpoint| endpoint.handle.key)
            .collect()
    }

    fn session(normal_succeeds: bool) -> (Runtime, UiSession) {
        let (runtime, session, _) = controlled_session(normal_succeeds);
        (runtime, session)
    }

    fn controlled_session(normal_succeeds: bool) -> (Runtime, UiSession, Arc<AtomicBool>) {
        let runtime = Runtime::new_unconfigured().unwrap();
        let firmware = Arc::new(TestFirmware::new());
        let builder_firmware = Arc::clone(&firmware);
        let succeeds = Arc::new(AtomicBool::new(normal_succeeds));
        let builder_succeeds = Arc::clone(&succeeds);
        let session = UiSession::new(
            runtime.handle(),
            draft(),
            Arc::new(Ip12Definition),
            FrontendPlan::default(),
            Box::new(move |draft, _network| {
                if !builder_succeeds.load(Ordering::Relaxed) {
                    return Err(String::from("injected builder stop"));
                }
                let mut build_draft = draft;
                build_draft.apply(Edit::SetProperty {
                    property: PropertyId(String::from("firmware.0.image-path")),
                    value: PropertyValue::Text(
                        builder_firmware.path.to_string_lossy().into_owned(),
                    ),
                });
                se_session::normal::build_configuration(build_draft, NatConfig::default())
                    .map_err(|error| error.to_string())
            }),
            Box::new(|_, _, _| Err(String::from("unused recording builder"))),
            Box::new(|_, _, _| Err(String::from("unused replay builder"))),
            Box::new(|_| Ok(())),
        );
        (runtime, session, succeeds)
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
    fn frontend_plan_changes_only_after_successful_machine_installation() {
        let (runtime, session, succeeds) = controlled_session(true);
        assert!(session.configure_machine(&network()).success);
        let attached_count = |session: &UiSession| {
            session
                .endpoint_catalog()
                .endpoints
                .iter()
                .filter(|endpoint| {
                    endpoint.kind == EndpointKindDto::Serial && endpoint.serial_console_attached
                })
                .count()
        };
        assert_eq!(attached_count(&session), 2);

        succeeds.store(false, Ordering::Relaxed);
        assert!(session.begin_machine_edit().success);
        assert!(
            session
                .apply_machine_edit(&detach_port("serial.1.channel.b.port"))
                .success
        );
        assert!(!session.configure_edited_machine(&network()).success);
        assert_eq!(attached_count(&session), 2);

        succeeds.store(true, Ordering::Relaxed);
        assert!(session.begin_machine_edit().success);
        assert!(
            session
                .apply_machine_edit(&detach_port("serial.1.channel.b.port"))
                .success
        );
        assert!(session.configure_edited_machine(&network()).success);
        let catalog = session.endpoint_catalog();
        assert_eq!(attached_count(&session), 1);
        assert!(catalog.endpoints.iter().any(|endpoint| {
            endpoint.handle.key == "serial.external.a" && endpoint.serial_console_attached
        }));
        assert!(catalog.endpoints.iter().any(|endpoint| {
            endpoint.handle.key == "serial.external.b" && !endpoint.serial_console_attached
        }));
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn recording_and_replay_transitions_install_matching_frontend_plans() {
        let firmware = TestFirmware::new();
        let record_path = firmware.path.with_extension("serec");
        let mut committed = draft();
        committed.apply(Edit::SetProperty {
            property: PropertyId(String::from("firmware.0.image-path")),
            value: PropertyValue::Text(firmware.path.to_string_lossy().into_owned()),
        });
        committed.apply(Edit::SetAttachment {
            slot: NodeId(String::from("serial.1.channel.b.port")),
            device: None,
        });
        let runtime = Runtime::new_unconfigured().unwrap();
        let session = UiSession::new(
            runtime.handle(),
            committed,
            Arc::new(Ip12Definition),
            FrontendPlan::default(),
            Box::new(|draft, _| {
                se_session::normal::build_configuration(draft, NatConfig::default())
                    .map_err(|error| error.to_string())
            }),
            Box::new(|draft, _, path| {
                se_session::recording::build_configuration(draft, NatConfig::default(), path)
                    .map_err(|error| error.to_string())
            }),
            Box::new(|draft, path, snapshot| {
                se_session::replay::build_configuration(draft, path, snapshot)
                    .map_err(|error| error.to_string())
            }),
            Box::new(|_| Ok(())),
        );

        assert!(session.configure_machine(&network()).success);
        assert_eq!(attached_serial_keys(&session), ["serial.external.a"]);
        assert!(
            session
                .run_with_record(&network(), record_path.to_str().unwrap())
                .success
        );
        assert_eq!(attached_serial_keys(&session), ["serial.external.a"]);
        assert!(session.stop_recording().success);

        assert!(session.begin_machine_edit().success);
        assert!(
            session
                .apply_machine_edit(&detach_port("serial.1.channel.a.port"))
                .success
        );
        assert!(
            session
                .apply_machine_edit(&set_port("serial.1.channel.b.port", "terminal.vt100"))
                .success
        );
        assert!(session.configure_edited_machine(&network()).success);
        assert_eq!(attached_serial_keys(&session), ["serial.external.b"]);

        assert!(
            session
                .open_replay(record_path.to_str().unwrap(), "")
                .success
        );
        assert_eq!(attached_serial_keys(&session), ["serial.external.a"]);
        assert!(session.stop_replay(&network()).success);
        assert_eq!(attached_serial_keys(&session), ["serial.external.b"]);

        drop(session);
        runtime.shutdown().unwrap();
        fs::remove_file(record_path).unwrap();
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
            FrontendPlan::default(),
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
            FrontendPlan::default(),
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
    fn keyboard_bridge_accepts_semantic_keys_for_the_live_endpoint() {
        let (runtime, session) = session(true);
        assert!(session.configure_machine(&network()).success);
        let catalog = session.endpoint_catalog();
        let keyboard = &catalog
            .endpoints
            .iter()
            .find(|endpoint| endpoint.kind == EndpointKindDto::Keyboard)
            .unwrap()
            .handle;
        assert!(session.send_keyboard(
            keyboard,
            KeyboardKeyDto {
                kind: KeyboardKeyKindDto::Letter,
                value: b'A',
            },
            true
        ));
        assert!(!session.send_keyboard(
            keyboard,
            KeyboardKeyDto {
                kind: KeyboardKeyKindDto::Letter,
                value: b'a',
            },
            true
        ));
        assert!(!session.send_keyboard(
            keyboard,
            KeyboardKeyDto {
                kind: KeyboardKeyKindDto::Function,
                value: 13,
            },
            true
        ));
        assert!(!session.send_keyboard(
            keyboard,
            KeyboardKeyDto {
                kind: KeyboardKeyKindDto::Named,
                value: 255,
            },
            true
        ));
        drop(session);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn endpoint_bridge_reflects_replacement_and_rejects_stale_handles() {
        let (runtime, session) = session(true);
        let empty = session.endpoint_catalog();
        assert!(empty.success);
        assert_eq!(empty.generation, 0);
        assert!(empty.endpoints.is_empty());

        assert!(session.configure_machine(&network()).success);
        let graphics = session.endpoint_catalog();
        let count = |kind| {
            graphics
                .endpoints
                .iter()
                .filter(|endpoint| endpoint.kind == kind)
                .count()
        };
        assert_eq!(count(EndpointKindDto::Keyboard), 1);
        assert_eq!(count(EndpointKindDto::Pointer), 1);
        assert_eq!(count(EndpointKindDto::Serial), 2);
        assert_eq!(count(EndpointKindDto::Ethernet), 1);
        assert_eq!(count(EndpointKindDto::Video), 1);
        assert!(
            graphics
                .endpoints
                .iter()
                .all(|endpoint| endpoint.handle.generation == graphics.generation)
        );
        let old_keyboard = &graphics
            .endpoints
            .iter()
            .find(|endpoint| endpoint.kind == EndpointKindDto::Keyboard)
            .unwrap()
            .handle;

        let mut headless_draft = draft();
        headless_draft.apply(Edit::SetAttachment {
            slot: NodeId(String::from("gio.0.slot.graphics")),
            device: None,
        });
        let plan = Ip12Definition.compile(&headless_draft).unwrap();
        let prepared = plan
            .prepare_with(|_, requirement| match requirement.kind {
                ResourceKind::Bytes => Ok::<_, String>(PreparedResource::Bytes(vec![0; 0x40000])),
                ResourceKind::Storage { .. } => Err(String::from("unexpected storage")),
            })
            .unwrap();
        runtime
            .configure(Machine::IndigoIp12(builder::build(prepared).unwrap()))
            .unwrap();
        let headless = session.endpoint_catalog();
        assert_eq!(headless.generation, graphics.generation + 1);
        assert_eq!(
            headless
                .endpoints
                .iter()
                .filter(|endpoint| endpoint.kind == EndpointKindDto::Serial)
                .count(),
            2
        );
        assert!(
            !headless
                .endpoints
                .iter()
                .any(|endpoint| endpoint.kind == EndpointKindDto::Video)
        );
        assert!(!session.send_keyboard(
            old_keyboard,
            KeyboardKeyDto {
                kind: KeyboardKeyKindDto::Letter,
                value: b'A',
            },
            true
        ));
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
